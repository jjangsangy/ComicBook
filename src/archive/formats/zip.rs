use crate::archive::path::parse_entry_info;
use crate::archive::reader::{ArchiveReader, EntryCallback};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

// ===================================================================
// ZIP / CBZ Reader
// ===================================================================

pub struct ZipReader {
    archive: ZipArchive<File>,
}

impl ZipReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
        let archive = ZipArchive::new(file)
            .with_context(|| format!("Failed to read zip {}", path.display()))?;
        Ok(Self { archive })
    }
}

impl ArchiveReader for ZipReader {
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()> {
        for i in 0..self.archive.len() {
            let mut file_entry = self.archive.by_index(i)?;
            let raw_name = file_entry.name().to_string();
            if let Some((clean_name, is_dir)) = parse_entry_info(&raw_name, file_entry.is_dir()) {
                if is_dir {
                    on_entry(&clean_name, true, &[])?;
                } else {
                    // Reuse the caller's buffer instead of allocating per entry.
                    scratch.clear();
                    file_entry.read_to_end(scratch)?;
                    on_entry(&clean_name, false, scratch)?;
                }
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<(String, bool)>> {
        let mut entries = Vec::with_capacity(self.archive.len());
        for i in 0..self.archive.len() {
            let file_entry = self.archive.by_index(i)?;
            if let Some((clean_name, is_dir)) =
                parse_entry_info(file_entry.name(), file_entry.is_dir())
            {
                entries.push((clean_name, is_dir));
            }
        }
        Ok(entries)
    }
}

// ===================================================================
// ZIP / CBZ Writer
// ===================================================================

pub struct ZipArchiveWriter {
    zip: ZipWriter<io::BufWriter<File>>,
    seen_dirs: HashSet<String>,
}

impl ZipArchiveWriter {
    pub fn create(dest: &Path) -> Result<Self> {
        let file =
            File::create(dest).with_context(|| format!("Failed to create {}", dest.display()))?;
        let zip = ZipWriter::new(io::BufWriter::with_capacity(128 * 1024, file));
        Ok(Self {
            zip,
            seen_dirs: HashSet::new(),
        })
    }

    pub fn add_entry(&mut self, normalized_name: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        if is_dir {
            let dir_name = format!("{}/", normalized_name);
            if self.seen_dirs.insert(dir_name.clone()) {
                self.zip
                    .add_directory(dir_name, SimpleFileOptions::default())?;
            }
        } else {
            // Ensure intermediate parent directories are registered in the zip
            let mut prefix = String::new();
            let segments: Vec<&str> = normalized_name.split('/').collect();
            if segments.len() > 1 {
                for seg in &segments[..segments.len() - 1] {
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(seg);
                    let dir_entry = format!("{}/", prefix);
                    if self.seen_dirs.insert(dir_entry.clone()) {
                        self.zip
                            .add_directory(dir_entry, SimpleFileOptions::default())?;
                    }
                }
            }

            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            self.zip.start_file(normalized_name, options)?;
            self.zip.write_all(data)?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.zip.finish()?;
        Ok(())
    }
}
