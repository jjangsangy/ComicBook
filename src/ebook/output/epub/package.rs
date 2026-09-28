//! OEBPS layout and the EPUB zip container (mimetype first, stored) (see docs/output.md).
//!
//! The whole book — images and derived XHTML/NCX/NAV/OPF — is built in memory and
//! streamed straight into the archive, so there is no temp tree and no second copy
//! of every page (see docs/architecture.md). KCC stores every entry (its payloads are
//! already-compressed images); we keep the same method so the archive layout stays
//! comparable.

use std::borrow::Cow;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// The EPUB OCF media type, written as the first, stored entry.
pub(crate) const MIMETYPE: &str = "application/epub+zip";

/// The fixed archive path of the OCF `mimetype` entry.
const MIMETYPE_PATH: &str = "mimetype";

/// The archive path of one [`ZipEntry`].
///
/// `#[repr(transparent)]` over `String`: layout-identical, no allocation, no
/// indirection, and move-only (no `Clone`).
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct ZipPath(String);

impl ZipPath {
    pub(super) fn new(path: impl Into<String>) -> Self {
        ZipPath(path.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// One archive entry: a path and its bytes.
///
/// `data` is normally `Cow::Borrowed` from an
/// [`EncodedPage`](crate::ebook::model::EncodedPage), so the entry list does not
/// duplicate the encoded book. Deliberately **not** `Clone` and with no `to_vec`
/// (docs/refactor.md §4 non-goal 2): cloning would duplicate the borrowed payload.
#[derive(Debug)]
pub(crate) struct ZipEntry<'a> {
    pub(crate) path: ZipPath,
    pub(crate) data: Cow<'a, [u8]>,
}

impl<'a> ZipEntry<'a> {
    /// An entry whose bytes are borrowed from the processed book.
    pub(super) fn borrowed(path: impl Into<String>, bytes: &'a [u8]) -> Self {
        ZipEntry {
            path: ZipPath::new(path),
            data: Cow::Borrowed(bytes),
        }
    }

    /// An entry whose bytes are a small derived document built in memory.
    pub(super) fn owned(path: impl Into<String>, bytes: Vec<u8>) -> Self {
        ZipEntry {
            path: ZipPath::new(path),
            data: Cow::Owned(bytes),
        }
    }
}

/// The archive contents: the container-level `mimetype` entry plus the OEBPS
/// `documents`.
///
/// The two are distinct fields, so the OCF "`mimetype` first and stored" rule is
/// structural: a `mimetype` entry can be neither omitted nor duplicated, and no
/// positional indexing is involved. Only [`EpubEntries::new`] constructs this, and
/// its visibility is the `epub` module, so `build_entries` is the sole caller.
/// Move-only — the list borrows the encoded book, so there is no `Clone`, no
/// `to_vec`, and no owned conversion.
#[derive(Debug)]
pub(crate) struct EpubEntries<'a> {
    /// The fixed container-level `mimetype` entry, written first.
    mimetype: ZipEntry<'a>,
    /// The OEBPS documents, in emission order.
    documents: Vec<ZipEntry<'a>>,
}

impl<'a> EpubEntries<'a> {
    /// Wrap the OEBPS `documents` and supply the fixed `mimetype` entry.
    ///
    /// The `documents` vector is reused as-is (the image payloads are borrowed
    /// `Cow`s, so only the small entry structs move — no book copy).
    pub(super) fn new(documents: Vec<ZipEntry<'a>>) -> Self {
        EpubEntries {
            mimetype: ZipEntry {
                path: ZipPath::new(MIMETYPE_PATH),
                data: Cow::Borrowed(MIMETYPE.as_bytes()),
            },
            documents,
        }
    }

    /// The OEBPS documents, excluding the container-level `mimetype`.
    ///
    /// `kindle::write_tree` materialises this set: the scratch tree handed to
    /// `kindling` never contained `mimetype`, and keeping it that way avoids
    /// perturbing the byte-frozen MOBI output.
    pub(crate) fn documents(&self) -> impl Iterator<Item = &ZipEntry<'a>> {
        self.documents.iter()
    }
}

/// Write `entries` as an EPUB zip to `dest`.
///
/// The `mimetype` entry is written first by construction ([`EpubEntries`]); it is
/// stored, as the EPUB specification requires, and so is every other entry (KCC's
/// payloads are already-compressed images).
pub(crate) fn write_epub(dest: &Path, entries: &EpubEntries<'_>) -> Result<()> {
    if let Some(parent) = dest.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let file =
        File::create(dest).with_context(|| format!("Failed to create {}", dest.display()))?;
    let mut zip = ZipWriter::new(BufWriter::with_capacity(128 * 1024, file));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for entry in std::iter::once(&entries.mimetype).chain(&entries.documents) {
        zip.start_file(entry.path.as_str(), options)?;
        zip.write_all(&entry.data[..])?;
    }

    zip.finish()?;
    Ok(())
}
