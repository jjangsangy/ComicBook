use super::formats::{DirectoryReader, RarReader, SevenZipReader, TarReader, ZipReader};
use super::kind::ArchiveKind;
use super::path::{ArchiveEntry, NormalizedArchivePath};
use anyhow::Result;
use std::path::Path;

/// The payload of a single archive entry.
///
/// A directory carries no bytes, so the two cases cannot be confused the way a `bool` plus an
/// empty-slice sentinel could: a reader that means "directory" cannot accidentally emit an empty
/// file, and a consumer cannot mistake one for the other.
#[derive(Debug, Clone, Copy)]
pub enum EntryContent<'a> {
    Directory,
    File(&'a [u8]),
}

/// Callback invoked for each archive entry: `(normalized_name, content)`.
pub type EntryCallback<'a> =
    &'a mut dyn FnMut(&NormalizedArchivePath, EntryContent<'_>) -> Result<()>;

/// Common interface for reading and listing archive contents across formats.
pub trait ArchiveReader {
    /// Stream entries from the archive, invoking `on_entry(normalized_name, content)` for each
    /// entry. Directory entries carry [`EntryContent::Directory`] and no data.
    ///
    /// The caller supplies `scratch`, a reusable byte buffer owned for the lifetime of the
    /// whole run. Implementations clear and refill it for each file entry so that converting
    /// many archives (or many pages within one archive) does not allocate a fresh buffer per
    /// file. The slice handed to `on_entry` is only valid for the duration of the call.
    fn read_entries(&mut self, scratch: &mut Vec<u8>, on_entry: EntryCallback) -> Result<()>;

    /// List entry names and kinds without reading full file contents into memory.
    fn list_entries(&mut self) -> Result<Vec<ArchiveEntry>>;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::writer::ArchiveWriter;

    #[test]
    fn entries_carry_their_kind_and_payload() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("book.cbz");
        let mut writer = ArchiveWriter::new(ArchiveKind::Cbz, &path)?;
        writer.add_entry("Chapter", EntryContent::Directory)?;
        writer.add_entry("Chapter/page.jpg", EntryContent::File(b"page bytes"))?;
        writer.finish()?;

        let mut reader = open_reader(ArchiveKind::Cbz, &path)?;
        let mut scratch = Vec::new();
        let mut seen: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        reader.read_entries(&mut scratch, &mut |name, content| {
            // `Directory` carries no payload; only `File` does.
            let payload = match content {
                EntryContent::Directory => None,
                EntryContent::File(data) => Some(data.to_vec()),
            };
            seen.push((name.as_str().to_string(), payload));
            Ok(())
        })?;

        assert_eq!(
            seen,
            vec![
                ("Chapter".to_string(), None),
                ("Chapter/page.jpg".to_string(), Some(b"page bytes".to_vec())),
            ]
        );
        Ok(())
    }
}
