use crate::archive::path::{parse_entry_info, ArchiveEntry, EntryKind, NormalizedArchivePath};
use crate::archive::reader::{ArchiveReader, EntryCallback, EntryContent};
use anyhow::{anyhow, Context, Result};
use rars::rar15_40::{write_streaming_archive_to, StreamingEntry, WriterOptions};
use rars::{ArchiveVersion, EntrySource, FeatureSet, MemberCoding, WriterResources};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ===================================================================
// RAR / CBR Reader
// ===================================================================

pub struct RarReader {
    path: PathBuf,
}

impl RarReader {
    pub fn open(path: &Path) -> Result<Self> {
        // Cheap readability check; the RAR headers are parsed lazily by the first real
        // operation. Parsing here would duplicate the parse that read/list already do.
        let _ = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl ArchiveReader for RarReader {
    fn read_entries(&mut self, _scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()> {
        let mut archive = unrar::Archive::new(&self.path)
            .open_for_processing()
            .map_err(|e| {
                anyhow!(
                    "Failed to open RAR archive {}: {:?}",
                    self.path.display(),
                    e
                )
            })?;

        while let Some(header) = archive.read_header().map_err(|e| {
            anyhow!(
                "Error reading RAR header in {}: {:?}",
                self.path.display(),
                e
            )
        })? {
            let raw_name = header.entry().filename.to_string_lossy();
            let is_dir_flag = header.entry().is_directory();
            match parse_entry_info(&raw_name, is_dir_flag) {
                None => {
                    archive = header.skip().map_err(|e| {
                        anyhow!(
                            "Error skipping RAR entry in {}: {:?}",
                            self.path.display(),
                            e
                        )
                    })?;
                }
                Some(parsed) => match parsed.kind {
                    EntryKind::Directory => {
                        archive = header.skip().map_err(|e| {
                            anyhow!(
                                "Error skipping RAR directory in {}: {:?}",
                                self.path.display(),
                                e
                            )
                        })?;
                        on_entry(&parsed.name, EntryContent::Directory)?;
                    }
                    EntryKind::File => {
                        // unrar allocates (and returns) its own Vec for each entry and offers no
                        // slice-into-buffer API, so we read straight into that allocation rather than
                        // copying it into `scratch`. This avoids a redundant full-entry memcpy.
                        let (data, next_archive) = header.read().map_err(|e| {
                            anyhow!(
                                "Error reading RAR entry in {}: {:?}",
                                self.path.display(),
                                e
                            )
                        })?;
                        archive = next_archive;
                        on_entry(&parsed.name, EntryContent::File(&data))?;
                    }
                },
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<ArchiveEntry>> {
        let mut archive = unrar::Archive::new(&self.path)
            .open_for_processing()
            .map_err(|e| {
                anyhow!(
                    "Failed to open RAR archive {}: {:?}",
                    self.path.display(),
                    e
                )
            })?;

        let mut entries = Vec::new();
        while let Some(header) = archive.read_header().map_err(|e| {
            anyhow!(
                "Error reading RAR header in {}: {:?}",
                self.path.display(),
                e
            )
        })? {
            let raw_name = header.entry().filename.to_string_lossy();
            let is_dir_flag = header.entry().is_directory();
            if let Some(parsed) = parse_entry_info(&raw_name, is_dir_flag) {
                entries.push(parsed);
            }
            archive = header.skip().map_err(|e| {
                anyhow!(
                    "Error skipping RAR entry in {}: {:?}",
                    self.path.display(),
                    e
                )
            })?;
        }
        Ok(entries)
    }
}

// ===================================================================
// RAR / CBR Writer
// ===================================================================

pub struct RarArchiveWriter {
    dest_path: PathBuf,
    entries: Vec<StreamingEntry>,
}

impl RarArchiveWriter {
    pub fn create(dest: &Path) -> Result<Self> {
        Ok(Self {
            dest_path: dest.to_path_buf(),
            entries: Vec::new(),
        })
    }

    pub fn add_entry(
        &mut self,
        normalized_name: &NormalizedArchivePath,
        content: EntryContent,
    ) -> Result<()> {
        if let EntryContent::File(data) = content {
            // RAR 4.0 uses backslash as path separator internally
            let rar_name = normalized_name.as_str().replace('/', "\\");
            // RAR 1.5-4.0 stores pre-compressed images verbatim; there is no point compressing
            // already-compressed image bytes. `from_bytes` hands rars a reopenable in-memory
            // source, so a stored member is copied straight to the output as the archive is
            // written and never duplicated into a second in-memory archive buffer.
            let source = EntrySource::from_bytes(Arc::<[u8]>::from(data));
            // Explicitly set regular file permissions (S_IFREG | 0644 = 0o100644) so that
            // extracted files have readable permissions rather than rars's default
            // 0x20 which is interpreted on Unix as 0o040 (unreadable by user).
            // host_os 3 (Unix) matches what rars' high-level Builder produces for RAR 4.0.
            self.entries.push(
                StreamingEntry::new(rar_name.into_bytes(), source)
                    .with_file_attr(0o100644)
                    .with_host_os(3),
            );
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        // rars' high-level `Builder` has no streaming writer for the RAR 1.5-4.0 family: it
        // encodes the entire archive into a `Vec<u8>` and then copies that to disk, so a CBR
        // conversion briefly held the whole archive twice over. The lower-level streaming
        // writer emits members straight to the destination and copies stored members from
        // their source, so the archive is never materialized as a whole.
        let options = WriterOptions::new(ArchiveVersion::Rar40, FeatureSet::store_only());
        let file = File::create(&self.dest_path).map_err(|e| {
            anyhow!(
                "Failed to create RAR archive {}: {e}",
                self.dest_path.display()
            )
        })?;
        let mut output = BufWriter::with_capacity(128 * 1024, file);
        write_streaming_archive_to(
            &self.entries,
            options,
            MemberCoding::Stored,
            None,
            &WriterResources::default(),
            None,
            &mut output,
        )
        .map_err(|e| {
            anyhow!(
                "Failed to write RAR archive {}: {e:?}",
                self.dest_path.display()
            )
        })?;
        output.flush()?;
        output.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::kind::ArchiveKind;
    use crate::archive::ops::{list_archive_entry_names, read_archive_entries};
    use crate::archive::writer::ArchiveWriter;
    use rars::{ArchiveVersion, EntrySource, FeatureSet, MemberCoding, WriterResources};

    fn valid_cbr(dir: &Path) -> Result<PathBuf> {
        let path = dir.join("book.cbr");
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbr, &path)?;
        writer.add_entry("page.jpg", EntryContent::File(b"bytes"))?;
        writer.finish()?;
        Ok(path)
    }

    /// Write a RAR holding one entry, with the streaming writer's raw knobs.
    fn write_streaming(path: &Path, name: &[u8], file_attr: u32, host_os: u8) -> Result<()> {
        let mut out = BufWriter::new(File::create(path)?);
        // A DOS host OS with the directory attribute bit (0x10) is what unrar reads
        // back as a directory entry.
        let entry = StreamingEntry::new(
            name.to_vec(),
            EntrySource::from_bytes(Arc::<[u8]>::from(Vec::new())),
        )
        .with_file_attr(file_attr)
        .with_host_os(host_os);
        write_streaming_archive_to(
            &[entry],
            WriterOptions::new(ArchiveVersion::Rar15, FeatureSet::store_only()),
            MemberCoding::Stored,
            None,
            &WriterResources::default(),
            None,
            &mut out,
        )
        .map_err(|e| anyhow!("failed to write rar: {e:?}"))?;
        out.flush()?;
        Ok(())
    }

    #[test]
    fn opening_a_non_rar_reports_an_error() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let bad = tmp.path().join("bad.cbr");
        std::fs::write(&bad, b"this is not a rar archive")?;
        // Both the streaming and the listing entry points report the open failure.
        assert!(read_archive_entries(ArchiveKind::Cbr, &bad, |_, _| Ok(())).is_err());
        assert!(list_archive_entry_names(ArchiveKind::Cbr, &bad).is_err());
        Ok(())
    }

    #[test]
    fn listing_a_rar_reports_its_entries() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let cbr = valid_cbr(tmp.path())?;
        let entries = list_archive_entry_names(ArchiveKind::Cbr, &cbr)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name.as_str(), "page.jpg");
        Ok(())
    }

    #[test]
    fn a_rar_directory_entry_is_reported_as_a_directory() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("dirs.cbr");
        write_streaming(&path, b"Chapter", 0x10, 0)?;

        let entries = list_archive_entry_names(ArchiveKind::Cbr, &path)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, EntryKind::Directory);

        // The streaming reader surfaces the same directory entry to its callback.
        let mut seen: Vec<(String, bool)> = Vec::new();
        read_archive_entries(ArchiveKind::Cbr, &path, |name, content| {
            seen.push((
                name.as_str().to_string(),
                matches!(content, EntryContent::Directory),
            ));
            Ok(())
        })?;
        assert_eq!(seen, vec![("Chapter".to_string(), true)]);
        Ok(())
    }

    #[test]
    fn finishing_over_a_directory_reports_an_error() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbr, tmp.path())?;
        writer.add_entry("page.jpg", EntryContent::File(b"x"))?;
        let err = writer
            .finish()
            .err()
            .context("a directory cannot be the RAR destination")?;
        assert!(err.to_string().contains("Failed to create RAR archive"));
        Ok(())
    }
}
