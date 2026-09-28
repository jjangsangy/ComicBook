//! AZW3 / MOBI output via the `kindling` crate (see docs/output.md).
//!
//! KCC's Kindle path is `buildEPUB` → `kindlegen` → `dualmetafix`. This port
//! keeps the first step — the fixed-layout EPUB that [`super::epub`] builds —
//! and replaces the last two with `kindling`'s MIT, pure-Rust MOBI builder, so
//! no external program is spawned and the GPL `dualmetafix` is never ported
//! (see docs/dependencies.md and docs/output.md). `kindling` is used as a *builder* only: the
//! pages it
//! encodes are the ones our own ported KCC pipeline produced, not its comic
//! pipeline's.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::ebook::options::{DocType, Options, OutputEncoding, SessionOptions};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

use super::epub;

/// Build the intermediate EPUB (only when kept) and the AZW3/MOBI at `kindle_dest`.
///
/// KCC writes the EPUB to disk and runs `kindlegen` on it; here the same OEBPS
/// tree is materialised into a scratch directory and handed to `kindling`'s OPF
/// builder directly (no zip round-trip; see docs/architecture.md). The EPUB zip is only
/// written when `--format mobi+epub` asked for it.
#[allow(clippy::too_many_arguments)]
pub fn build_kindle(
    epub_dest: &Path,
    kindle_dest: &Path,
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
    title: &str,
    drop_bookmarks: bool,
) -> Result<()> {
    let entries = epub::build_entries(book, prepared, source, options, title, drop_bookmarks)?;

    let keep_epub = matches!(
        options.output.encoding,
        OutputEncoding::Mobi { keep_epub: true }
    );
    if keep_epub {
        epub::package::write_epub(epub_dest, &entries)?;
    }

    // `kindling` reads the OPF, its XHTML and its images from disk, so unpack
    // the in-memory tree into a scratch directory for it. `--temp-dir` puts that
    // directory on the source's drive, as KCC's `getWorkFolder` does; the in-memory
    // pipeline has no other temp tree to relocate.
    let scratch = create_scratch(source, &options.session)?;
    write_tree(scratch.path(), &entries)?;
    let opf_path = scratch.path().join("OEBPS/content.opf");

    if let Some(parent) = kindle_dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    let extracted = kindling::extracted::ExtractedEpub::from_opf_path(&opf_path)
        .map_err(|error| anyhow!("Failed to read the intermediate EPUB: {error}"))?;

    // `--doc-type` maps onto EXTH 501; the default (`none`) omits it, avoiding
    // the firmware "back to library" issue (see docs/output.md).
    let doc_type = doc_type_tag(options.output.doc_type);
    build_mobi(&extracted, kindle_dest, options, doc_type.as_deref())
        .map_err(|error| anyhow!("Kindle output failed: {error}"))?;

    Ok(())
}

/// Encode `extracted` into a KF8-only `.azw3` or a dual MOBI7+KF8 `.mobi`.
///
/// The flag choice mirrors `kindling comic`: no SRCS source embedding (it
/// duplicates every image and can overflow a PalmDB record), no CMET, no HD
/// container, no creator tag, no Kindle publishing limits (comics default them
/// off), and the build-time HTML self-check left on.
fn build_mobi(
    extracted: &kindling::extracted::ExtractedEpub,
    kindle_dest: &Path,
    options: &Options,
    doc_type: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    kindling::mobi::build_mobi_from_extracted(
        extracted,
        kindle_dest,
        false,                                                   // no_compress
        false,                                                   // headwords_only
        None,                                                    // srcs_data
        false,                                                   // include_cmet
        true,                                                    // no_hd_images
        false,                                                   // creator_tag
        matches!(options.output.encoding, OutputEncoding::Azw3), // kf8_only
        doc_type,                                                // doc_type (EXTH 501)
        false,                                                   // kindle_limits
        true,                                                    // self_check
        false,                                                   // kindlegen_parity
        false,                                                   // strict_accents
        false,                                                   // fold_accents
        false,                                                   // force_user_fonts
    )
}

/// `--doc-type` as kindling's EXTH 501 value (see docs/output.md).
fn doc_type_tag(doc_type: DocType) -> Option<String> {
    match doc_type {
        DocType::None => None,
        DocType::Ebok => Some("EBOK".to_string()),
        DocType::Pdoc => Some("PDOC".to_string()),
    }
}

/// Create the scratch directory `kindling` reads, honouring `--temp-dir`.
///
/// Without `--temp-dir` it is created in the system temp directory; with it, next
/// to the source, so the spooled images live on the same drive as the input.
fn create_scratch(source: &Path, session: &SessionOptions) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("comic-book-kindle-");
    let scratch = match session.temp_dir.then(|| source.parent()).flatten() {
        Some(parent) if !parent.as_os_str().is_empty() => builder.tempdir_in(parent),
        _ => builder.tempdir(),
    };
    scratch.context("Failed to create a scratch directory for Kindle output")
}

/// Materialise the OEBPS entries under `root`.
fn write_tree(root: &Path, entries: &[(String, Cow<'_, [u8]>)]) -> Result<()> {
    for (name, bytes) in entries {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::write(&path, &bytes[..])
            .with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(())
}
