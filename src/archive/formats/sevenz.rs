use crate::archive::path::parse_entry_info;
use crate::archive::reader::{ArchiveReader, EntryCallback};
use anyhow::{anyhow, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

// ===================================================================
// 7z / CB7 Reader
// ===================================================================

pub struct SevenZipReader {
    path: PathBuf,
}

impl SevenZipReader {
    pub fn open(path: &Path) -> Result<Self> {
        let _ = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
            .map_err(|e| anyhow!("Failed to open 7z archive {}: {:?}", path.display(), e))?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl ArchiveReader for SevenZipReader {
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()> {
        let mut reader =
            sevenz_rust2::ArchiveReader::open(&self.path, sevenz_rust2::Password::empty())
                .map_err(|e| {
                    anyhow!("Failed to open 7z archive {}: {:?}", self.path.display(), e)
                })?;

        reader
            .for_each_entries(|entry, r| {
                let is_dir_flag = entry.is_directory()
                    || (entry.has_windows_attributes && (entry.windows_attributes & 0x10) != 0);
                if let Some((clean_name, is_dir)) = parse_entry_info(entry.name(), is_dir_flag) {
                    if is_dir {
                        on_entry(&clean_name, true, &[])
                            .map_err(|e| sevenz_rust2::Error::Other(e.to_string().into()))?;
                    } else {
                        // Reuse the caller's buffer instead of allocating per entry.
                        scratch.clear();
                        r.read_to_end(scratch)
                            .map_err(|e| sevenz_rust2::Error::Other(e.to_string().into()))?;
                        on_entry(&clean_name, false, scratch)
                            .map_err(|e| sevenz_rust2::Error::Other(e.to_string().into()))?;
                    }
                }
                Ok(true)
            })
            .map_err(|e| anyhow!("Error reading 7z archive {}: {:?}", self.path.display(), e))?;
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<(String, bool)>> {
        let reader = sevenz_rust2::ArchiveReader::open(&self.path, sevenz_rust2::Password::empty())
            .map_err(|e| anyhow!("Failed to open 7z archive {}: {:?}", self.path.display(), e))?;
        let mut entries = Vec::new();
        for entry in reader.archive().files.iter() {
            let is_dir_flag = entry.is_directory()
                || (entry.has_windows_attributes && (entry.windows_attributes & 0x10) != 0);
            if let Some((clean_name, is_dir)) = parse_entry_info(entry.name(), is_dir_flag) {
                entries.push((clean_name, is_dir));
            }
        }
        Ok(entries)
    }
}

// ===================================================================
// 7z / CB7 Writer
// ===================================================================

pub struct SevenZipArchiveWriter {
    writer: sevenz_rust2::ArchiveWriter<File>,
}

impl SevenZipArchiveWriter {
    pub fn create(dest: &Path) -> Result<Self> {
        let mut sz = sevenz_rust2::ArchiveWriter::create(dest)
            .map_err(|e| anyhow!("Failed to create 7z archive {}: {:?}", dest.display(), e))?;
        // Comic book archives store pre-compressed images (JPEG, PNG, WebP).
        // Using compression on already compressed image files provides no meaningful
        // space savings and causes significant CPU overhead. We use EncoderMethod::COPY
        // (no compression / store mode) and disable header encryption for instant conversion.
        sz.set_content_methods(vec![sevenz_rust2::EncoderMethod::COPY.into()]);
        sz.set_encrypt_header(false);
        Ok(Self { writer: sz })
    }

    pub fn add_entry(&mut self, normalized_name: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        let mut entry = sevenz_rust2::ArchiveEntry::new();
        entry.name = normalized_name.to_string();
        if is_dir {
            entry.has_stream = false;
            entry.is_directory = true;
            entry.has_windows_attributes = true;
            entry.windows_attributes = 0x10; // FILE_ATTRIBUTE_DIRECTORY
            self.writer
                .push_archive_entry(entry, None::<&[u8]>)
                .map_err(|e| {
                    anyhow!(
                        "Failed to add 7z directory entry {}: {:?}",
                        normalized_name,
                        e
                    )
                })?;
        } else {
            entry.has_stream = true;
            entry.is_directory = false;
            entry.has_windows_attributes = true;
            entry.windows_attributes = 0x20; // FILE_ATTRIBUTE_ARCHIVE
            entry.size = data.len() as u64;
            self.writer
                .push_archive_entry(entry, Some(data))
                .map_err(|e| anyhow!("Failed to add 7z entry {}: {:?}", normalized_name, e))?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.writer
            .finish()
            .map_err(|e| anyhow!("Failed to finish 7z archive: {:?}", e))?;
        Ok(())
    }
}
