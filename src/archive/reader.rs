use super::formats::{DirectoryReader, RarReader, SevenZipReader, TarReader, ZipReader};
use super::kind::ArchiveKind;
use anyhow::Result;
use std::path::Path;

/// Callback invoked for each archive entry: `(entry_name, is_dir, file_bytes)`.
pub type EntryCallback<'a> = &'a mut dyn FnMut(&str, bool, &[u8]) -> Result<()>;

/// Common interface for reading and listing archive contents across formats.
pub trait ArchiveReader {
    /// Stream entries from the archive, invoking `on_entry(normalized_name, is_dir, data)`
    /// for each entry. For directory entries, `data` is empty.
    fn read_entries(&mut self, on_entry: EntryCallback) -> Result<()>;

    /// List entry names and directory flags without reading full file contents into memory.
    fn list_entries(&mut self) -> Result<Vec<(String, bool)>>;
}

/// Open an appropriate reader for the given archive kind and path.
pub fn open_reader<P: AsRef<Path>>(kind: ArchiveKind, path: P) -> Result<Box<dyn ArchiveReader>> {
    let p = path.as_ref();
    match kind {
        ArchiveKind::Cbz => Ok(Box::new(ZipReader::open(p)?)),
        ArchiveKind::Cbt => Ok(Box::new(TarReader::open(p)?)),
        ArchiveKind::Cb7 => Ok(Box::new(SevenZipReader::open(p)?)),
        ArchiveKind::Cbr => Ok(Box::new(RarReader::open(p)?)),
        ArchiveKind::Directory => Ok(Box::new(DirectoryReader::open(p)?)),
    }
}
