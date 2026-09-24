use crate::archive::path::parse_entry_info;
use crate::archive::reader::{ArchiveReader, EntryCallback};
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
                Some((clean_name, true)) => {
                    archive = header.skip().map_err(|e| {
                        anyhow!(
                            "Error skipping RAR directory in {}: {:?}",
                            self.path.display(),
                            e
                        )
                    })?;
                    on_entry(&clean_name, true, &[])?;
                }
                Some((clean_name, false)) => {
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
                    on_entry(&clean_name, false, &data)?;
                }
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<(String, bool)>> {
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
            if let Some((clean_name, is_dir)) = parse_entry_info(&raw_name, is_dir_flag) {
                entries.push((clean_name, is_dir));
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

    pub fn add_entry(&mut self, normalized_name: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        if !is_dir {
            // RAR 4.0 uses backslash as path separator internally
            let rar_name = normalized_name.replace('/', "\\");
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
