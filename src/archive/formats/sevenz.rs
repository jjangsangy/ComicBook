use crate::archive::path::{parse_entry_info, ArchiveEntry, EntryKind, NormalizedArchivePath};
use crate::archive::reader::{ArchiveReader, EntryCallback, EntryContent};
use anyhow::{anyhow, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

// ===================================================================
// 7z / CB7 Reader
// ===================================================================

pub struct SevenZipReader {
    path: PathBuf,
    // Keep the opened archive so enumeration and streaming share one header parse
    // instead of reopening (and re-parsing) the container for each operation.
    reader: sevenz_rust2::ArchiveReader<File>,
}

impl SevenZipReader {
    pub fn open(path: &Path) -> Result<Self> {
        let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
            .map_err(|e| anyhow!("Failed to open 7z archive {}: {:?}", path.display(), e))?;
        Ok(Self {
            path: path.to_path_buf(),
            reader,
        })
    }
}

impl ArchiveReader for SevenZipReader {
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()> {
        self.reader
            .for_each_entries(|entry, r| {
                let is_dir_flag = entry.is_directory()
                    || (entry.has_windows_attributes && (entry.windows_attributes & 0x10) != 0);
                if let Some(parsed) = parse_entry_info(entry.name(), is_dir_flag) {
                    match parsed.kind {
                        EntryKind::Directory => {
                            on_entry(&parsed.name, EntryContent::Directory)
                                .map_err(|e| sevenz_rust2::Error::Other(e.to_string().into()))?;
                        }
                        EntryKind::File => {
                            // Reuse the caller's buffer instead of allocating per entry.
                            scratch.clear();
                            r.read_to_end(scratch)
                                .map_err(|e| sevenz_rust2::Error::Other(e.to_string().into()))?;
                            on_entry(&parsed.name, EntryContent::File(scratch))
                                .map_err(|e| sevenz_rust2::Error::Other(e.to_string().into()))?;
                        }
                    }
                }
                Ok(true)
            })
            .map_err(|e| anyhow!("Error reading 7z archive {}: {:?}", self.path.display(), e))?;
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<ArchiveEntry>> {
        let mut entries = Vec::new();
        for entry in self.reader.archive().files.iter() {
            let is_dir_flag = entry.is_directory()
                || (entry.has_windows_attributes && (entry.windows_attributes & 0x10) != 0);
            if let Some(parsed) = parse_entry_info(entry.name(), is_dir_flag) {
                entries.push(parsed);
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

    pub fn add_entry(
        &mut self,
        normalized_name: &NormalizedArchivePath,
        content: EntryContent,
    ) -> Result<()> {
        let name = normalized_name.as_str();
        let mut entry = sevenz_rust2::ArchiveEntry::new();
        entry.name = name.to_string();
        match content {
            EntryContent::Directory => {
                entry.has_stream = false;
                entry.is_directory = true;
                entry.has_windows_attributes = true;
                entry.windows_attributes = 0x10; // FILE_ATTRIBUTE_DIRECTORY
                self.writer
                    .push_archive_entry(entry, None::<&[u8]>)
                    .map_err(|e| anyhow!("Failed to add 7z directory entry {}: {:?}", name, e))?;
            }
            EntryContent::File(data) => {
                entry.has_stream = true;
                entry.is_directory = false;
                entry.has_windows_attributes = true;
                entry.windows_attributes = 0x20; // FILE_ATTRIBUTE_ARCHIVE
                entry.size = data.len() as u64;
                self.writer
                    .push_archive_entry(entry, Some(data))
                    .map_err(|e| anyhow!("Failed to add 7z entry {}: {:?}", name, e))?;
            }
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
