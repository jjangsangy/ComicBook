use crate::archive::path::{parse_entry_info, ArchiveEntry, EntryKind, NormalizedArchivePath};
use crate::archive::reader::{ArchiveReader, EntryCallback, EntryContent};
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
            // Parse straight from the borrowed name; the previous `.to_string()` was a
            // throwaway allocation per entry.
            if let Some(entry) = parse_entry_info(file_entry.name(), file_entry.is_dir()) {
                match entry.kind {
                    EntryKind::Directory => {
                        on_entry(&entry.name, EntryContent::Directory)?;
                    }
                    EntryKind::File => {
                        // Reuse the caller's buffer instead of allocating per entry.
                        scratch.clear();
                        file_entry.read_to_end(scratch)?;
                        on_entry(&entry.name, EntryContent::File(scratch))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<ArchiveEntry>> {
        let mut entries = Vec::with_capacity(self.archive.len());
        for i in 0..self.archive.len() {
            let file_entry = self.archive.by_index(i)?;
            if let Some(entry) = parse_entry_info(file_entry.name(), file_entry.is_dir()) {
                entries.push(entry);
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

    pub fn add_entry(
        &mut self,
        normalized_name: &NormalizedArchivePath,
        content: EntryContent,
    ) -> Result<()> {
        let name = normalized_name.as_str();
        match content {
            EntryContent::Directory => {
                let dir_name = format!("{}/", name);
                if self.seen_dirs.insert(dir_name.clone()) {
                    self.zip
                        .add_directory(dir_name, SimpleFileOptions::default())?;
                }
            }
            EntryContent::File(data) => {
                // Ensure intermediate parent directories are registered in the zip.
                // Walk the separators in place instead of collecting a `Vec<&str>` per entry.
                for (idx, _) in name.match_indices('/') {
                    let dir_entry = format!("{}/", &name[..idx]);
                    if self.seen_dirs.insert(dir_entry.clone()) {
                        self.zip
                            .add_directory(dir_entry, SimpleFileOptions::default())?;
                    }
                }

                let options =
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
                self.zip.start_file(name, options)?;
                self.zip.write_all(data)?;
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.zip.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::path::normalize_archive_path;

    #[test]
    fn a_duplicate_directory_entry_is_written_once() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("book.cbz");
        let mut writer = ZipArchiveWriter::create(&path)?;
        let name = normalize_archive_path("Chapter").context("a non-empty name normalizes")?;
        // The first insert registers the directory; the second is a no-op (`seen_dirs`
        // already holds it) rather than a duplicate zip entry.
        writer.add_entry(&name, EntryContent::Directory)?;
        writer.add_entry(&name, EntryContent::Directory)?;
        writer.finish()?;

        let file = File::open(&path)?;
        let archive = ZipArchive::new(file)?;
        let chapter_dirs = archive.file_names().filter(|n| *n == "Chapter/").count();
        assert_eq!(chapter_dirs, 1);
        Ok(())
    }
}
