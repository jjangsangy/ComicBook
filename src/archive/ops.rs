use super::kind::ArchiveKind;
use super::path::{copy_dir_all, find_single_root_dir, is_matching_root};
use super::reader::{open_reader, read_entries_with_scratch};
use super::writer::ArchiveWriter;
use crate::image_ops::is_image_file;
use anyhow::{anyhow, Context, Result};
use image::DynamicImage;
use std::fs;
use std::path::Path;

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
    F: FnMut(&str, bool, &[u8]) -> Result<()>,
{
    let mut scratch = Vec::new();
    read_entries_with_scratch(kind, path, &mut scratch, &mut on_entry)
}

/// List all entry names and whether they are directories from an archive or folder.
pub fn list_archive_entry_names<P: AsRef<Path>>(
    kind: ArchiveKind,
    path: P,
) -> Result<Vec<(String, bool)>> {
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
    convert_archive_ext(src_kind, src_path, target_kind, dest_path, false)
}

/// Convert an archive or directory to a destination format, optionally stripping a single common root folder on extraction.
pub fn convert_archive_ext<P: AsRef<Path>, Q: AsRef<Path>>(
    src_kind: ArchiveKind,
    src_path: P,
    target_kind: ArchiveKind,
    dest_path: Q,
    strip_common_root: bool,
) -> Result<()> {
    let mut scratch = Vec::new();
    convert_archive_ext_with_scratch(
        src_kind,
        src_path,
        target_kind,
        dest_path,
        strip_common_root,
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
    strip_common_root: bool,
    scratch: &mut Vec<u8>,
) -> Result<()> {
    let src = src_path.as_ref();
    let dest = dest_path.as_ref();

    if same_file::is_same_file(src, dest).unwrap_or(false) {
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

    let dest_name = dest.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let src_stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    // Open the source once. The single reader is used both to enumerate entry names
    // (for root detection) and to stream the entries themselves, so extraction no longer
    // opens/parses the source archive twice.
    let mut reader = open_reader(src_kind, src)?;

    let root_to_strip: Option<String> =
        if target_kind == ArchiveKind::Directory && src_kind != ArchiveKind::Directory {
            reader
                .list_entries()
                .ok()
                .and_then(|entries| find_single_root_dir(&entries))
                .filter(|root| strip_common_root || is_matching_root(root, dest_name, src_stem))
        } else {
            None
        };

    let root_prefix = root_to_strip.as_ref().map(|root| format!("{}/", root));

    let mut writer = ArchiveWriter::new(target_kind, dest)?;
    let result = (|| -> Result<()> {
        reader.read_entries(scratch, &mut |name, is_dir, data| {
            if let Some(ref root) = root_to_strip {
                if name == root {
                    return Ok(());
                }
                if let Some(ref prefix) = root_prefix {
                    if let Some(stripped) = name.strip_prefix(prefix) {
                        if stripped.is_empty() {
                            return Ok(());
                        }
                        // `name` was normalized by the reader, so `stripped` is too.
                        return writer.add_entry_normalized(stripped, is_dir, data);
                    }
                }
            }
            // Reader names are already normalized; avoid normalizing a second time.
            writer.add_entry_normalized(name, is_dir, data)
        })?;
        writer.finish()?;
        Ok(())
    })();

    if result.is_err() && target_kind != ArchiveKind::Directory && dest.is_file() {
        let _ = fs::remove_file(dest);
    }

    result
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
) -> Result<Vec<(String, DynamicImage)>> {
    let path = path.as_ref();
    let mut images = Vec::new();

    read_archive_entries(kind, path, |name, is_dir, data| {
        if !is_dir && is_image_file(name) {
            let img = image::load_from_memory(data)
                .with_context(|| format!("Failed to decode image {}", name))?;
            let filename = name.rsplit(['/', '\\']).next().unwrap_or(name).to_string();
            images.push((filename, img));
        }
        Ok(())
    })?;

    images.sort_by(|a, b| natord::compare(&a.0, &b.0));
    Ok(images)
}
