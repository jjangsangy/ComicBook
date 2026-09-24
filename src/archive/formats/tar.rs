use crate::archive::path::parse_entry_info;
use crate::archive::reader::{ArchiveReader, EntryCallback};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

// ===================================================================
// TAR / CBT Reader
// ===================================================================

pub struct TarReader {
    path: PathBuf,
}

impl TarReader {
    pub fn open(path: &Path) -> Result<Self> {
        let _ = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    fn open_archive(&self) -> Result<tar::Archive<File>> {
        let file = File::open(&self.path)
            .with_context(|| format!("Failed to open {}", self.path.display()))?;
        Ok(tar::Archive::new(file))
    }
}

impl ArchiveReader for TarReader {
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()> {
        let mut archive = self.open_archive()?;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path_str = entry.path()?.to_string_lossy().to_string();
            let is_dir_header = entry.header().entry_type().is_dir();
            if let Some((clean_name, is_dir)) = parse_entry_info(&path_str, is_dir_header) {
                if is_dir {
                    on_entry(&clean_name, true, &[])?;
                } else {
                    // Reuse the caller's buffer instead of allocating per entry.
                    scratch.clear();
                    entry.read_to_end(scratch)?;
                    on_entry(&clean_name, false, scratch)?;
                }
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<(String, bool)>> {
        let mut archive = self.open_archive()?;
        let mut entries = Vec::new();
        for entry in archive.entries()? {
            let entry = entry?;
            let path_str = entry.path()?.to_string_lossy().to_string();
            let is_dir_header = entry.header().entry_type().is_dir();
            if let Some((clean_name, is_dir)) = parse_entry_info(&path_str, is_dir_header) {
                entries.push((clean_name, is_dir));
            }
        }
        Ok(entries)
    }
}

// ===================================================================
// TAR / CBT Writer
// ===================================================================

pub struct TarArchiveWriter {
    builder: tar::Builder<io::BufWriter<File>>,
}

impl TarArchiveWriter {
    pub fn create(dest: &Path) -> Result<Self> {
        let file =
            File::create(dest).with_context(|| format!("Failed to create {}", dest.display()))?;
        let builder = tar::Builder::new(io::BufWriter::with_capacity(128 * 1024, file));
        Ok(Self { builder })
    }

    pub fn add_entry(&mut self, normalized_name: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        let mut header = tar::Header::new_gnu();
        if is_dir {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            let dir_name = format!("{}/", normalized_name);
            self.builder
                .append_data(&mut header, dir_name, &mut io::empty())?;
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            self.builder
                .append_data(&mut header, normalized_name, data)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.builder.finish()?;
        Ok(())
    }
}
