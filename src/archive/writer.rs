use super::formats::{
    DirectoryArchiveWriter, RarArchiveWriter, SevenZipArchiveWriter, TarArchiveWriter,
    ZipArchiveWriter,
};
use super::kind::ArchiveKind;
use super::path::normalize_archive_path;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// Universal archive writer supporting all comic archive formats and directories.
pub enum ArchiveWriter {
    Cbz(Box<ZipArchiveWriter>),
    Cbt(TarArchiveWriter),
    Cb7(SevenZipArchiveWriter),
    Directory(DirectoryArchiveWriter),
    Cbr(RarArchiveWriter),
}

impl ArchiveWriter {
    /// Create a new archive writer for the specified destination and format.
    ///
    /// Automatically ensures any parent directory of `dest_path` exists.
    pub fn new<P: AsRef<Path>>(kind: ArchiveKind, dest_path: P) -> Result<Self> {
        let dest = dest_path.as_ref();
        if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }

        match kind {
            ArchiveKind::Cbz => Ok(ArchiveWriter::Cbz(Box::new(ZipArchiveWriter::create(
                dest,
            )?))),
            ArchiveKind::Cbt => Ok(ArchiveWriter::Cbt(TarArchiveWriter::create(dest)?)),
            ArchiveKind::Cb7 => Ok(ArchiveWriter::Cb7(SevenZipArchiveWriter::create(dest)?)),
            ArchiveKind::Directory => Ok(ArchiveWriter::Directory(DirectoryArchiveWriter::create(
                dest,
            )?)),
            ArchiveKind::Cbr => Ok(ArchiveWriter::Cbr(RarArchiveWriter::create(dest)?)),
        }
    }

    /// Add an entry (directory or file) to the archive.
    ///
    /// Normalizes path separators and traversal segments before writing. Callers that
    /// already hold a normalized entry name (for example the reader pipeline, whose names
    /// have gone through [`crate::archive::path::normalize_archive_path`] once already)
    /// should use [`ArchiveWriter::add_entry_normalized`] to avoid re-normalizing.
    pub fn add_entry(&mut self, name: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        let normalized = normalize_archive_path(name);
        if normalized.is_empty() {
            return Ok(());
        }
        self.dispatch(&normalized, is_dir, data)
    }

    /// Add an entry whose name is already normalized.
    ///
    /// This skips the per-entry re-normalization that [`ArchiveWriter::add_entry`]
    /// performs, which is redundant for names produced by the archive readers.
    pub(crate) fn add_entry_normalized(
        &mut self,
        normalized_name: &str,
        is_dir: bool,
        data: &[u8],
    ) -> Result<()> {
        if normalized_name.is_empty() {
            return Ok(());
        }
        self.dispatch(normalized_name, is_dir, data)
    }

    fn dispatch(&mut self, normalized: &str, is_dir: bool, data: &[u8]) -> Result<()> {
        match self {
            ArchiveWriter::Cbz(w) => w.add_entry(normalized, is_dir, data),
            ArchiveWriter::Cbt(w) => w.add_entry(normalized, is_dir, data),
            ArchiveWriter::Cb7(w) => w.add_entry(normalized, is_dir, data),
            ArchiveWriter::Directory(w) => w.add_entry(normalized, is_dir, data),
            ArchiveWriter::Cbr(w) => w.add_entry(normalized, is_dir, data),
        }
    }

    /// Finish writing and finalize the archive file.
    pub fn finish(self) -> Result<()> {
        match self {
            ArchiveWriter::Cbz(w) => w.finish(),
            ArchiveWriter::Cbt(w) => w.finish(),
            ArchiveWriter::Cb7(w) => w.finish(),
            ArchiveWriter::Directory(w) => w.finish(),
            ArchiveWriter::Cbr(w) => w.finish(),
        }
    }
}
