use super::kind::ArchiveKind;
use super::path::{
    copy_dir_all, find_single_root_dir, is_matching_root, ArchiveEntry, NormalizedArchivePath,
};
use super::reader::{open_reader, read_entries_with_scratch, ArchiveReader, EntryContent};
use super::writer::ArchiveWriter;
use crate::image_ops::is_image_file;
use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use std::fs;
use std::path::Path;

/// The final path component of an archive entry (its file name), as a stored image is named.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(transparent)]
pub struct BaseName(String);

impl BaseName {
    /// The file name as a borrowed string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A decoded image together with the base name it was stored under.
pub struct DecodedImage {
    pub name: BaseName,
    pub image: DynamicImage,
}

/// When a single redundant root folder should be stripped while extracting an archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootStripPolicy {
    /// Never collapse a wrapper folder, even if one exists.
    Never,
    /// Collapse the wrapper folder whenever the archive has exactly one.
    Always,
    /// Collapse the wrapper folder only when its name matches the destination folder or the
    /// source file stem.
    IfMatchingDestination,
}

/// A wrapper folder to collapse on extraction: its normalized name and the `"<name>/"`
/// prefix every member shares.
///
/// The two were previously separate `Option`s (`root_to_strip` and a derived
/// `root_prefix`) whose mixed `(Some, None)` state could not occur but was still
/// representable in the type (REFACTOR.md E3).
struct RootStrip {
    name: String,
    prefix: String,
}

impl RootStrip {
    fn new(name: String) -> Self {
        let prefix = format!("{name}/");
        RootStrip { name, prefix }
    }
}

/// Stream or read archive entries in memory without extracting loose files to the filesystem.
///
/// This allocates a throwaway scratch buffer for the duration of the call. Callers that process
/// many archives in a row should use [`read_entries_with_scratch`] to reuse one buffer instead.
pub fn read_archive_entries<P: AsRef<Path>, F>(
    kind: ArchiveKind,
    path: P,
    mut on_entry: F,
) -> Result<()>
where
    F: FnMut(&NormalizedArchivePath, EntryContent<'_>) -> Result<()>,
{
    let mut scratch = Vec::new();
    read_entries_with_scratch(kind, path, &mut scratch, &mut on_entry)
}

/// List all entries (names and kinds) from an archive or folder.
pub fn list_archive_entry_names<P: AsRef<Path>>(
    kind: ArchiveKind,
    path: P,
) -> Result<Vec<ArchiveEntry>> {
    let mut reader = open_reader(kind, path.as_ref())?;
    reader.list_entries()
}

/// Convert an archive or directory to a destination format efficiently.
/// Conversions between archive formats or extractions are performed directly in-memory and streamed.
pub fn convert_archive<P: AsRef<Path>, Q: AsRef<Path>>(
    src_kind: ArchiveKind,
    src_path: P,
    target_kind: ArchiveKind,
    dest_path: Q,
) -> Result<()> {
    convert_archive_ext(
        src_kind,
        src_path,
        target_kind,
        dest_path,
        RootStripPolicy::IfMatchingDestination,
    )
}

/// Convert an archive or directory to a destination format, optionally stripping a single common root folder on extraction.
pub fn convert_archive_ext<P: AsRef<Path>, Q: AsRef<Path>>(
    src_kind: ArchiveKind,
    src_path: P,
    target_kind: ArchiveKind,
    dest_path: Q,
    policy: RootStripPolicy,
) -> Result<()> {
    let mut scratch = Vec::new();
    convert_archive_ext_with_scratch(
        src_kind,
        src_path,
        target_kind,
        dest_path,
        policy,
        &mut scratch,
    )
}

/// Like [`convert_archive_ext`], but reuses a caller-owned `scratch` buffer for entry data.
///
/// Threading one buffer through a whole batch keeps the memory footprint bounded by the largest
/// single entry instead of allocating (and freeing) per file.
pub(crate) fn convert_archive_ext_with_scratch<P: AsRef<Path>, Q: AsRef<Path>>(
    src_kind: ArchiveKind,
    src_path: P,
    target_kind: ArchiveKind,
    dest_path: Q,
    policy: RootStripPolicy,
    scratch: &mut Vec<u8>,
) -> Result<()> {
    let src = src_path.as_ref();
    let dest = dest_path.as_ref();

    // A failed `is_same_file` (for instance when the destination does not exist yet) means
    // "not the same file"; only a positive match blocks the conversion.
    if matches!(same_file::is_same_file(src, dest), Ok(true)) {
        return Err(anyhow!("Cannot convert '{}' into itself", src.display()));
    }

    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    // Direct copy optimization if source and target formats are identical
    if src_kind == target_kind {
        if src_kind == ArchiveKind::Directory {
            copy_dir_all(src, dest)?;
            return Ok(());
        } else if src.is_file() {
            fs::copy(src, dest)?;
            return Ok(());
        }
    }

    // An absent destination file name or source stem stays `None` rather than collapsing to a
    // (matching-irrelevant) empty string.
    let dest_name = dest.file_name().and_then(|s| s.to_str());
    let src_stem = src.file_stem().and_then(|s| s.to_str());

    // Open the source once. The single reader is used both to enumerate entry names
    // (for root detection) and to stream the entries themselves, so extraction no longer
    // opens/parses the source archive twice.
    let mut reader = open_reader(src_kind, src)?;

    // A wrapper folder can only be collapsed when extracting an archive into a directory.
    let root_strip: Option<RootStrip> = match policy {
        RootStripPolicy::Never => None,
        RootStripPolicy::Always => {
            single_collapsible_root(reader.as_mut(), target_kind, src_kind).map(RootStrip::new)
        }
        RootStripPolicy::IfMatchingDestination => {
            single_collapsible_root(reader.as_mut(), target_kind, src_kind)
                .filter(|root| is_matching_root(root, dest_name, src_stem))
                .map(RootStrip::new)
        }
    };

    let mut writer = ArchiveWriter::new(target_kind, dest)?;
    let result = (|| -> Result<()> {
        reader.read_entries(scratch, &mut |name, content| {
            if let Some(strip) = root_strip.as_ref() {
                if name.as_str() == strip.name {
                    return Ok(());
                }
                if let Some(stripped) = name.strip_prefix(&strip.prefix) {
                    // `name` and `prefix` are normalized, so `stripped` is too, and it is
                    // never empty (the bare-root entry is returned early above).
                    return writer.add_entry_normalized(&stripped, content);
                }
            }
            // Reader names are already normalized; avoid normalizing a second time.
            writer.add_entry_normalized(name, content)
        })?;
        writer.finish()?;
        Ok(())
    })();

    if result.is_err() && target_kind != ArchiveKind::Directory && dest.is_file() {
        let _ = fs::remove_file(dest);
    }

    result
}

/// The archive's single wrapper directory, when the destination layout can collapse it
/// (an archive extracted into a directory). `None` otherwise, without listing.
fn single_collapsible_root(
    reader: &mut dyn ArchiveReader,
    target_kind: ArchiveKind,
    src_kind: ArchiveKind,
) -> Option<String> {
    if !target_kind.is_archive() && src_kind.is_archive() {
        single_root_dir(reader)
    } else {
        None
    }
}

/// The archive's single wrapper directory, if listing succeeds.
///
/// A listing failure is deliberately non-fatal: the caller is about to stream the same archive,
/// so root-stripping is skipped and extraction proceeds — `read_entries` will surface the real
/// error if the archive is genuinely unreadable.
fn single_root_dir(reader: &mut dyn ArchiveReader) -> Option<String> {
    reader
        .list_entries()
        .ok()
        .and_then(|entries| find_single_root_dir(&entries))
}

/// Extract an archive or directory to a destination folder.
pub fn extract_archive<P: AsRef<Path>, Q: AsRef<Path>>(
    kind: ArchiveKind,
    archive_path: P,
    dest_dir: Q,
) -> Result<()> {
    convert_archive(kind, archive_path, ArchiveKind::Directory, dest_dir)
}

/// Compress a directory into the destination archive format.
pub fn compress_archive<P: AsRef<Path>, Q: AsRef<Path>>(
    kind: ArchiveKind,
    source_dir: P,
    dest_path: Q,
) -> Result<()> {
    convert_archive(ArchiveKind::Directory, source_dir, kind, dest_path)
}

/// Retrieve all images from an archive or directory, sorted in natural alphanumeric order.
/// Decodes images directly in memory without writing temporary files to disk.
pub fn get_images_from_source<P: AsRef<Path>>(
    kind: ArchiveKind,
    path: P,
) -> Result<Vec<DecodedImage>> {
    let path = path.as_ref();
    let mut images = Vec::new();

    read_archive_entries(kind, path, |name, content| {
        if let EntryContent::File(data) = content {
            if is_image_file(name.as_str()) {
                let image = image::load_from_memory(data)
                    .with_context(|| format!("Failed to decode image {name}"))?;
                let basename = name.as_relative().file_name().unwrap_or("").to_string();
                images.push(DecodedImage {
                    name: BaseName(basename),
                    image,
                });
            }
        }
        Ok(())
    })?;

    images.sort_by(|a, b| natord::compare(a.name.as_str(), b.name.as_str()));
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::writer::ArchiveWriter;

    fn write_cbz(path: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbz, path)?;
        for (name, data) in entries {
            writer.add_entry(name, EntryContent::File(data))?;
        }
        writer.finish()
    }

    #[test]
    fn root_strip_policy_controls_wrapper_collapse() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("book.cbz");
        write_cbz(&src, &[("Root/a.jpg", b"a"), ("Root/b.jpg", b"b")])?;

        // `Never` keeps the wrapper directory.
        let kept = tmp.path().join("kept");
        convert_archive_ext(
            ArchiveKind::Cbz,
            &src,
            ArchiveKind::Directory,
            &kept,
            RootStripPolicy::Never,
        )?;
        assert!(kept.join("Root/a.jpg").is_file());

        // `Always` collapses a single wrapper directory.
        let collapsed = tmp.path().join("collapsed");
        convert_archive_ext(
            ArchiveKind::Cbz,
            &src,
            ArchiveKind::Directory,
            &collapsed,
            RootStripPolicy::Always,
        )?;
        assert!(collapsed.join("a.jpg").is_file());
        assert!(!collapsed.join("Root").exists());

        // `IfMatchingDestination` collapses only when the destination matches the root.
        let matching = tmp.path().join("Root");
        convert_archive_ext(
            ArchiveKind::Cbz,
            &src,
            ArchiveKind::Directory,
            &matching,
            RootStripPolicy::IfMatchingDestination,
        )?;
        assert!(matching.join("a.jpg").is_file());

        let unmatched = tmp.path().join("elsewhere");
        convert_archive_ext(
            ArchiveKind::Cbz,
            &src,
            ArchiveKind::Directory,
            &unmatched,
            RootStripPolicy::IfMatchingDestination,
        )?;
        assert!(unmatched.join("Root/a.jpg").is_file());
        Ok(())
    }
}
