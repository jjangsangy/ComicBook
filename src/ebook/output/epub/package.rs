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

/// Write `entries` as an EPUB zip to `dest`.
///
/// `entries` are `(zip path, bytes)` pairs other than `mimetype`, which is always
/// written first and uncompressed as the EPUB specification requires.
pub(crate) fn write_epub(dest: &Path, entries: &[(String, Cow<'_, [u8]>)]) -> Result<()> {
    if let Some(parent) = dest.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let file =
        File::create(dest).with_context(|| format!("Failed to create {}", dest.display()))?;
    let mut zip = ZipWriter::new(BufWriter::with_capacity(128 * 1024, file));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    zip.start_file("mimetype", options)?;
    zip.write_all(MIMETYPE.as_bytes())?;

    for (name, bytes) in entries {
        zip.start_file(name.as_str(), options)?;
        zip.write_all(&bytes[..])?;
    }

    zip.finish()?;
    Ok(())
}
