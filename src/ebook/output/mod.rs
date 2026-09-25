//! Output builders, dispatched by the resolved [`Format`](super::options::Format).

pub mod cbz;
pub mod epub;
pub mod kepub;
pub mod kindle;
pub mod pdf;

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

use crate::ebook::naming;
use crate::ebook::options::{Format, Options};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

/// Write a processed book in the requested format, returning the output paths.
///
/// `Format::Epub` (and its KePub variant, which `Options::resolve` folds into it)
/// is implemented; the remaining formats land in later phases (AGENTS.md §15).
pub fn write_book(
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
) -> Result<Vec<PathBuf>> {
    match options.format {
        Format::Epub => {
            // Splitting a book into tomes is Phase 9 (AGENTS.md §15).
            if options.target_size.is_some() || options.batch_split > 0 {
                bail!(
                    "size-capped or batch-split output is not implemented yet \
                     (AGENTS.md §15, Phase 9)"
                );
            }
            let dest =
                naming::output_filename(source, options.output.as_deref(), ".epub", "", options);
            epub::build_epub(&dest, book, prepared, source, options)?;
            Ok(vec![dest])
        }
        Format::Cbz => bail!("CBZ output is not implemented yet (AGENTS.md §15, Phase 7)"),
        Format::Pdf => bail!("PDF output is not implemented yet (AGENTS.md §15, Phase 7)"),
        Format::Mobi | Format::Azw3 => {
            bail!("Kindle output is not implemented yet (AGENTS.md §15, Phase 8)")
        }
        // `Options::resolve` expands these presets before a format reaches here.
        other => bail!("internal error: unresolved output format {other:?}"),
    }
}
