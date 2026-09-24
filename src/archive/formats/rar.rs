use crate::archive::path::parse_entry_info;
use crate::archive::reader::{ArchiveReader, EntryCallback};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

// ===================================================================
// RAR / CBR Reader
// ===================================================================

pub struct RarReader {
    path: PathBuf,
}

impl RarReader {
    pub fn open(path: &Path) -> Result<Self> {
        let _ = unrar::Archive::new(path)
            .open_for_processing()
            .map_err(|e| anyhow!("Failed to open RAR archive {}: {:?}", path.display(), e))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl ArchiveReader for RarReader {
    fn read_entries(&mut self, on_entry: EntryCallback) -> Result<()> {
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
    builder: rars::Builder,
}

impl RarArchiveWriter {
    pub fn create(dest: &Path) -> Result<Self> {
        // RAR 4.0 is the universal CBR standard supported across comic book readers.
        // It also avoids rars' RAR 5 streaming implementation which opens a temporary
        // spool file descriptor for each entry and can exhaust the OS file descriptor limit.
        let builder = rars::Builder::new(rars::ArchiveVersion::Rar40).store(true);
        Ok(Self {
            dest_path: dest.to_path_buf(),
            builder,
        })
    }

    pub fn add_entry(&mut self, normalized_name: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        if !is_dir {
            // RAR 4.0 uses backslash as path separator internally
            let rar_name = normalized_name.replace('/', "\\");
            // Explicitly set regular file permissions (S_IFREG | 0644 = 0o100644) so that
            // extracted files have readable permissions rather than rars's default
            // 0x20 which is interpreted on Unix as 0o040 (unreadable by user).
            self.builder
                .add_bytes(
                    rar_name.as_bytes().to_vec(),
                    data.to_vec(),
                    None,
                    Some(0o100644),
                )
                .map_err(|e| {
                    anyhow!(
                        "Failed to add entry {} to RAR archive: {:?}",
                        normalized_name,
                        e
                    )
                })?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.builder
            .write_to_path(&self.dest_path, None)
            .map_err(|e| {
                anyhow!(
                    "Failed to write RAR archive {}: {:?}",
                    self.dest_path.display(),
                    e
                )
            })?;
        Ok(())
    }
}
