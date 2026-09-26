//! Output builders, dispatched by the resolved [`Format`](super::options::Format).
//!
//! Phase 5 shipped the fixed-layout EPUB/KePub builder, Phase 7 added CBZ and
//! PDF, and Phase 8 adds the Kindle formats (`azw3`/`mobi` via `kindling`);
//! size-capped/batch-split output lands in Phase 9 (AGENTS.md §15).

pub mod cbz;
pub mod epub;
pub mod kepub;
pub mod kindle;
pub mod lightnovel;
pub mod pdf;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

use crate::ebook::naming;
use crate::ebook::options::{Format, Options};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

/// Write a processed book in the requested format, returning the output paths.
///
/// KePub is not handled here: `Options::resolve` folds it into [`Format::Epub`]
/// (the KePub differences live in the shared EPUB builder). Light-novel mode never
/// reaches this function — [`super::convert_source`] dispatches to
/// [`lightnovel::convert`] before the normal pipeline (AGENTS.md §12.3).
pub fn write_book(
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
) -> Result<Vec<PathBuf>> {
    // Splitting a book into tomes is Phase 9 (AGENTS.md §15). MOBI forces
    // `batch_split` on for legacy reasons, but that default is not a user
    // request and produces a single tome, so only an explicit `--batch-split`
    // (or a size cap) is a reason to bail.
    if options.target_size.is_some() || options.batch_split_explicit {
        bail!(
            "size-capped or batch-split output is not implemented yet \
             (AGENTS.md §15, Phase 9)"
        );
    }

    match options.format {
        Format::Epub => {
            let dest =
                naming::output_filename(source, options.output.as_deref(), ".epub", "", options);
            epub::build_epub(&dest, book, prepared, source, options)?;
            Ok(vec![dest])
        }
        Format::Cbz => {
            let dest =
                naming::output_filename(source, options.output.as_deref(), ".cbz", "", options);
            cbz::build_cbz(&dest, book, prepared)?;
            Ok(vec![dest])
        }
        Format::Pdf => {
            let dest =
                naming::output_filename(source, options.output.as_deref(), ".pdf", "", options);
            pdf::build_pdf(&dest, book, prepared)?;
            Ok(vec![dest])
        }
        Format::Mobi | Format::Azw3 => {
            // KCC always builds the fixed-layout EPUB first and derives the
            // Kindle file name from it by replacing the extension
            // (`makeMOBIFix`); the intermediate EPUB survives only under
            // `mobi+epub` (AGENTS.md §9).
            let epub_dest =
                naming::output_filename(source, options.output.as_deref(), ".epub", "", options);
            let kindle_ext = if options.format == Format::Azw3 {
                "azw3"
            } else {
                "mobi"
            };
            let kindle_dest = epub_dest.with_extension(kindle_ext);
            kindle::build_kindle(&epub_dest, &kindle_dest, book, prepared, source, options)?;

            let mut written = vec![kindle_dest];
            if options.keep_epub {
                written.push(epub_dest);
            }
            Ok(written)
        }
        // `Options::resolve` expands these presets before a format reaches here.
        other => bail!("internal error: unresolved output format {other:?}"),
    }
}
