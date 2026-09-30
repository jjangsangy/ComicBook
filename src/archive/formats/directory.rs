use crate::archive::path::{
    parse_entry_info, safe_join, ArchiveEntry, EntryKind, NormalizedArchivePath,
};
use crate::archive::reader::{ArchiveReader, EntryCallback, EntryContent};
use anyhow::Result;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ===================================================================
// Directory Reader
// ===================================================================

pub struct DirectoryReader {
    path: PathBuf,
}

impl DirectoryReader {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    fn walk_sorted(&self) -> impl Iterator<Item = walkdir::DirEntry> {
        WalkDir::new(&self.path)
            .sort_by(|a, b| {
                natord::compare(
                    &a.file_name().to_string_lossy(),
                    &b.file_name().to_string_lossy(),
                )
            })
            .into_iter()
            .filter_map(|e| e.ok())
    }
}

impl ArchiveReader for DirectoryReader {
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()> {
        for entry in self.walk_sorted() {
            let ft = entry.file_type();
            let p = entry.path();
            let rel_path = match p.strip_prefix(&self.path) {
                Ok(rp) => rp,
                Err(_) => continue,
            };
            let rel_str = rel_path.to_string_lossy();
            // Branch on the `EntryKind` parsed from the entry instead of discarding it and
            // re-deriving directory-ness from `file_type()` a second time.
            if let Some(parsed) = parse_entry_info(&rel_str, ft.is_dir()) {
                match parsed.kind {
                    EntryKind::Directory => {
                        on_entry(&parsed.name, EntryContent::Directory)?;
                    }
                    // `EntryKind::File` also covers other node types (sockets, fifos); skip those,
                    // as opening a fifo would block.
                    EntryKind::File if ft.is_file() || ft.is_symlink() => {
                        // Reuse the caller's buffer instead of a fresh `fs::read` allocation per file.
                        if let Ok(mut file) = fs::File::open(p) {
                            scratch.clear();
                            if file.read_to_end(scratch).is_ok() {
                                on_entry(&parsed.name, EntryContent::File(scratch))?;
                            }
                        }
                    }
                    EntryKind::File => {}
                }
            }
        }
        Ok(())
    }

    fn list_entries(&mut self) -> Result<Vec<ArchiveEntry>> {
        let mut entries = Vec::new();
        for entry in self.walk_sorted() {
            let p = entry.path();
            let rel_path = match p.strip_prefix(&self.path) {
                Ok(rp) => rp,
                Err(_) => continue,
            };
            let rel_str = rel_path.to_string_lossy();
            if let Some(parsed) = parse_entry_info(&rel_str, entry.file_type().is_dir()) {
                entries.push(parsed);
            }
        }
        Ok(entries)
    }
}

// ===================================================================
// Directory Writer
// ===================================================================

pub struct DirectoryArchiveWriter {
    dest_dir: PathBuf,
}

impl DirectoryArchiveWriter {
    pub fn create(dest: &Path) -> Result<Self> {
        fs::create_dir_all(dest)?;
        Ok(Self {
            dest_dir: dest.to_path_buf(),
        })
    }

    pub fn add_entry(
        &mut self,
        normalized_name: &NormalizedArchivePath,
        content: EntryContent,
    ) -> Result<()> {
        let target = safe_join(&self.dest_dir, normalized_name.as_str());
        if target == self.dest_dir {
            return Ok(());
        }
        match content {
            EntryContent::Directory => {
                fs::create_dir_all(&target)?;
            }
            EntryContent::File(data) => {
                if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&target, data)?;
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_a_directory_reports_files_and_directories() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        fs::create_dir_all(tmp.path().join("Chapter"))?;
        fs::write(tmp.path().join("Chapter/page.jpg"), b"page")?;
        fs::write(tmp.path().join("cover.jpg"), b"cover")?;

        let mut reader = DirectoryReader::open(tmp.path())?;
        let entries = reader.list_entries()?;
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"cover.jpg"));
        assert!(names.contains(&"Chapter"));
        assert!(names.contains(&"Chapter/page.jpg"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_node_is_skipped_without_blocking() -> Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let tmp = tempfile::tempdir()?;
        let fifo = tmp.path().join("pipe.jpg");
        let c_path = CString::new(fifo.as_os_str().as_bytes())?;
        // SAFETY: `c_path` is a valid NUL-terminated path and the mode is a plain
        // permission bitmask; `mkfifo` only creates the node.
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) };
        assert_eq!(rc, 0, "mkfifo failed");

        let mut reader = DirectoryReader::open(tmp.path())?;
        let mut scratch = Vec::new();
        let mut count = 0usize;
        // The FIFO is a non-file, non-symlink node; opening it would block, so it must
        // be skipped rather than read.
        reader.read_entries(&mut scratch, &mut |_, _| {
            count += 1;
            Ok(())
        })?;
        assert_eq!(count, 0);
        Ok(())
    }
}
