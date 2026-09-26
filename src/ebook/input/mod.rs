//! Input adapters: turn each supported source into a [`ComicTree`].
//!
//! Archives and image folders are handled by [`archive`]; the EPUB (spine-ordered)
//! and PDF (embedded-image/rasterised) adapters live alongside it.

pub mod archive;
pub mod epub;
pub mod fusion;
pub mod pdf;

use anyhow::{bail, Result};
use std::path::Path;

use crate::archive::{detect_archive_kind, ArchiveKind};
use crate::ebook::model::ComicTree;
use crate::ebook::options::Options;

/// The kind of input a source path resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A comic archive or an image folder.
    Archive(ArchiveKind),
    /// A fixed-layout EPUB.
    Epub,
    /// A PDF.
    Pdf,
}

/// Classify a source path.
///
/// Returns `None` when the path is neither an existing archive/image folder nor
/// an `.epub`/`.pdf` file. EPUB and PDF are recognised by extension *before* the
/// magic-byte archive sniff, because an EPUB is itself a ZIP.
pub fn detect_source_kind(source: &Path) -> Option<SourceKind> {
    if source.is_dir() {
        return Some(SourceKind::Archive(ArchiveKind::Directory));
    }
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("epub") => Some(SourceKind::Epub),
        Some("pdf") => Some(SourceKind::Pdf),
        _ => detect_archive_kind(source).map(SourceKind::Archive),
    }
}

/// Load one source into a [`ComicTree`].
///
/// `options` supplies the PDF adapter's render geometry (`--pdf-width`,
/// `--legacy-extract`, the profile size and the crop multiplier) and lets the
/// EPUB adapter fall back to plain extraction under `--legacy-extract`/
/// `--light-novel`, exactly as KCC's `getWorkFolder` does.
pub fn load_tree(source: &Path, options: &Options) -> Result<ComicTree> {
    if !source.exists() {
        bail!("Failed to open source file/directory: {}", source.display());
    }
    match detect_source_kind(source) {
        Some(SourceKind::Archive(kind)) => archive::load(source, kind),
        Some(SourceKind::Epub) => epub::load(source, options),
        Some(SourceKind::Pdf) => pdf::load(source, options),
        None => bail!(
            "Unsupported input '{}': expected a comic archive (.cbz/.cbr/.cb7/.cbt), \
             an image folder, or an .epub/.pdf file",
            source.display()
        ),
    }
}
