use crate::archive::path::{parse_entry_info, ArchiveEntry, EntryKind, NormalizedArchivePath};
use crate::archive::reader::{ArchiveReader, EntryCallback, EntryContent};
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

    /// Open the archive for sequential reading, buffered so tar's small header reads
    /// don't each become a syscall.
    fn open_archive(&self) -> Result<tar::Archive<io::BufReader<File>>> {
        let file = File::open(&self.path)
            .with_context(|| format!("Failed to open {}", self.path.display()))?;
        Ok(tar::Archive::new(io::BufReader::with_capacity(
            128 * 1024,
            file,
        )))
    }

    /// Open the archive for name-only enumeration. A seekable archive lets the iterator
    /// skip entry bodies with `lseek` instead of reading through every byte of the tar.
    fn open_archive_seekable(&self) -> Result<tar::Archive<File>> {
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
            let is_dir_header = entry.header().entry_type().is_dir();
            // Borrow the path rather than allocating an owned `String` per entry.
            if let Some(parsed) = parse_entry_info(&entry.path()?.to_string_lossy(), is_dir_header)
            {
                match parsed.kind {
                    EntryKind::Directory => {
                        on_entry(&parsed.name, EntryContent::Directory)?;
                    }
                    EntryKind::File => {
                        // Reuse the caller's buffer instead of allocating per entry.
                        scratch.clear();
                        entry.read_to_end(scratch)?;
                        on_entry(&parsed.name, EntryContent::File(scratch))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<ArchiveEntry>> {
        let mut archive = self.open_archive_seekable()?;
        let mut entries = Vec::new();
        for entry in archive.entries_with_seek()? {
            let entry = entry?;
            let is_dir_header = entry.header().entry_type().is_dir();
            if let Some(parsed) = parse_entry_info(&entry.path()?.to_string_lossy(), is_dir_header)
            {
                entries.push(parsed);
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

    pub fn add_entry(
        &mut self,
        normalized_name: &NormalizedArchivePath,
        content: EntryContent,
    ) -> Result<()> {
        let name = normalized_name.as_str();
        let mut header = tar::Header::new_gnu();
        match content {
            EntryContent::Directory => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_cksum();
                let dir_name = format!("{}/", name);
                self.builder
                    .append_data(&mut header, dir_name, &mut io::empty())?;
            }
            EntryContent::File(data) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                self.builder.append_data(&mut header, name, data)?;
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.builder.finish()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tar_entry_whose_name_normalizes_away_is_skipped() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("book.cbt");
        let file = File::create(&path)?;
        let mut builder = tar::Builder::new(file);

        // A root `./` entry normalizes to nothing, so the reader drops it rather than
        // emitting an empty name.
        let mut root = tar::Header::new_gnu();
        root.set_entry_type(tar::EntryType::Regular);
        root.set_size(0);
        root.set_mode(0o644);
        root.set_path("./")?;
        root.set_cksum();
        builder.append(&root, io::empty())?;

        // One real file so the reader still yields an entry.
        let mut page = tar::Header::new_gnu();
        page.set_entry_type(tar::EntryType::Regular);
        page.set_size(1);
        page.set_mode(0o644);
        page.set_cksum();
        builder.append_data(&mut page, "page.jpg", &b"x"[..])?;
        builder.finish()?;

        let mut reader = TarReader::open(&path)?;
        let mut scratch = Vec::new();
        let mut names = Vec::new();
        reader.read_entries(&mut scratch, &mut |name, _| {
            names.push(name.as_str().to_string());
            Ok(())
        })?;
        assert_eq!(names, vec!["page.jpg".to_string()]);
        Ok(())
    }
}
