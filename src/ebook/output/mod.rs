//! Output builders, dispatched by the resolved [`Format`](super::options::Format).
//!
//! Phase 5 shipped the fixed-layout EPUB/KePub builder and Phase 7 adds CBZ and
//! PDF; the Kindle formats land in Phase 8 and size-capped/batch-split output in
//! Phase 9 (AGENTS.md §15).

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
    // Splitting a book into tomes is Phase 9 (AGENTS.md §15).
    if options.target_size.is_some() || options.batch_split > 0 {
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
            bail!("Kindle output is not implemented yet (AGENTS.md §15, Phase 8)")
        }
        // `Options::resolve` expands these presets before a format reaches here.
        other => bail!("internal error: unresolved output format {other:?}"),
    }
}
