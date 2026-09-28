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

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::ebook::options::{DocType, Options, OutputEncoding, SessionOptions};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

use super::epub;
use super::Tomes;

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
    tomes: Tomes,
) -> Result<()> {
    let entries = epub::build_entries(book, prepared, source, options, title, tomes)?;

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
    let flags = MobiFlags::resolve(options.output.encoding);
    build_mobi(&extracted, kindle_dest, flags, doc_type.as_deref())
        .map_err(|error| anyhow!("Kindle output failed: {error}"))?;

    Ok(())
}

/// The `kindling` MOBI builder switches, named so the single positional call
/// cannot transpose them (docs/refactor.md A15).
///
/// Stack-only and `Copy`; no allocation and no dynamic dispatch. The `srcs_data`
/// and `doc_type` parameters are kept out of the struct: they are `Option`s of
/// distinct types (`Option<&[u8]>` / `Option<&str>`), which the compiler cannot
/// confuse with the `bool` cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MobiFlags {
    /// `no_compress`: comics keep the default (compressed) PDB records.
    no_compress: bool,
    /// `headwords_only`: dictionary-only switch, unused for comics.
    headwords_only: bool,
    /// `include_cmet`: Kindle "sample" marker, never set.
    include_cmet: bool,
    /// `no_hd_images`: no HD container (comics default it off).
    no_hd_images: bool,
    /// `creator_tag`: no creator EXTH tag.
    creator_tag: bool,
    /// `kf8_only`: AZW3 is KF8-only; MOBI is dual MOBI7+KF8.
    kf8_only: bool,
    /// `kindle_limits`: no Kindle publishing size limits.
    kindle_limits: bool,
    /// `self_check`: keep the build-time HTML self-check on.
    self_check: bool,
    /// `kindlegen_parity`: off.
    kindlegen_parity: bool,
    /// `strict_accents`: dictionary INDX only.
    strict_accents: bool,
    /// `fold_accents`: dictionary INDX only.
    fold_accents: bool,
    /// `force_user_fonts`: off.
    force_user_fonts: bool,
}

impl MobiFlags {
    /// Resolve the flags for the resolved output encoding, mirroring `kindling comic`.
    fn resolve(encoding: OutputEncoding) -> Self {
        // `OutputEncoding` is our own exhaustive enum, so a future variant must make
        // an explicit choice here rather than fall into `matches!`'s silent `false`.
        let kf8_only = match encoding {
            OutputEncoding::Azw3 => true,
            OutputEncoding::Epub { .. }
            | OutputEncoding::Kepub { .. }
            | OutputEncoding::Mobi { .. }
            | OutputEncoding::Cbz
            | OutputEncoding::Pdf => false,
        };
        Self {
            no_compress: false,
            headwords_only: false,
            include_cmet: false,
            no_hd_images: true,
            creator_tag: false,
            kf8_only,
            kindle_limits: false,
            self_check: true,
            kindlegen_parity: false,
            strict_accents: false,
            fold_accents: false,
            force_user_fonts: false,
        }
    }
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
    flags: MobiFlags,
    doc_type: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    kindling::mobi::build_mobi_from_extracted(
        extracted,
        kindle_dest,
        flags.no_compress,
        flags.headwords_only,
        None, // srcs_data: never embedded (it would duplicate every image)
        flags.include_cmet,
        flags.no_hd_images,
        flags.creator_tag,
        flags.kf8_only,
        doc_type, // doc_type (EXTH 501)
        flags.kindle_limits,
        flags.self_check,
        flags.kindlegen_parity,
        flags.strict_accents,
        flags.fold_accents,
        flags.force_user_fonts,
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
    let scratch = match source.parent().filter(|_| session.temp_dir) {
        Some(parent) if !parent.as_os_str().is_empty() => builder.tempdir_in(parent),
        _ => builder.tempdir(),
    };
    scratch.context("Failed to create a scratch directory for Kindle output")
}

/// Materialise the OEBPS documents under `root` for `kindling`.
///
/// The container-level `mimetype` entry is deliberately skipped: the scratch tree
/// has never contained it, and keeping it out leaves `kindling`'s input (and thus
/// the byte-frozen MOBI output) unchanged.
fn write_tree(root: &Path, entries: &epub::package::EpubEntries<'_>) -> Result<()> {
    for entry in entries.documents() {
        let path = root.join(entry.path.as_str());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        fs::write(&path, &entry.data[..])
            .with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eleven non-varying `kindling` flags mirror `kindling comic`; only
    /// `kf8_only` follows the format. Pinning the whole cluster here keeps a flipped
    /// constant or a swapped match arm from silently changing the MOBI/AZW3 bytes —
    /// nothing else in the tree reads `MobiFlags`.
    #[test]
    fn mobi_flags_follow_kindling_comic_and_only_kf8_varies() {
        let mobi = MobiFlags {
            no_compress: false,
            headwords_only: false,
            include_cmet: false,
            no_hd_images: true,
            creator_tag: false,
            kf8_only: false,
            kindle_limits: false,
            self_check: true,
            kindlegen_parity: false,
            strict_accents: false,
            fold_accents: false,
            force_user_fonts: false,
        };

        let non_azw3 = [
            OutputEncoding::Epub { kfx: false },
            OutputEncoding::Epub { kfx: true },
            OutputEncoding::Kepub { short_ext: false },
            OutputEncoding::Kepub { short_ext: true },
            OutputEncoding::Mobi { keep_epub: false },
            OutputEncoding::Mobi { keep_epub: true },
            OutputEncoding::Cbz,
            OutputEncoding::Pdf,
        ];
        for encoding in non_azw3 {
            assert_eq!(MobiFlags::resolve(encoding), mobi, "flags for {encoding:?}");
        }

        assert_eq!(
            MobiFlags::resolve(OutputEncoding::Azw3),
            MobiFlags {
                kf8_only: true,
                ..mobi
            },
            "AZW3 must be KF8-only"
        );
    }
}
