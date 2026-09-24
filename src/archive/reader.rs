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
    ///
    /// The caller supplies `scratch`, a reusable byte buffer owned for the lifetime of the
    /// whole run. Implementations clear and refill it for each file entry so that converting
    /// many archives (or many pages within one archive) does not allocate a fresh buffer per
    /// file. The slice handed to `on_entry` is only valid for the duration of the call.
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()>;

    /// List entry names and directory flags without reading full file contents into memory.
    fn list_entries(&mut self) -> Result<Vec<(String, bool)>>;
}

/// Read every entry of an archive, reusing the caller-provided `scratch` buffer for entry data.
///
/// Passing a long-lived buffer lets callers stream a whole directory of archives without a
/// fresh allocation per file. See [`ArchiveReader::read_entries`].
pub fn read_entries_with_scratch<P: AsRef<Path>>(
    kind: ArchiveKind,
    path: P,
    scratch: &mut Vec<u8>,
    on_entry: EntryCallback,
) -> Result<()> {
    let mut reader = open_reader(kind, path.as_ref())?;
    reader.read_entries(scratch, on_entry)
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
