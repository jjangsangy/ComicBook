//! Output builders, dispatched by the resolved [`OutputEncoding`].
//!
//! Every format dispatches through [`write_book`], which splits the processed book
//! into tomes with [`crate::ebook::chunk`] and writes one file per tome, each with
//! its own title, UUID and labelled cover.

pub mod cbz;
pub mod epub;
pub mod kepub;
pub mod kindle;
pub mod lightnovel;
pub mod pdf;

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::ebook::chunk;
use crate::ebook::naming;
use crate::ebook::options::{Options, OutputEncoding};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

/// Whether a book was written as one file or split into several tomes.
///
/// Replaces KCC's `ischunked` boolean (REFACTOR.md A17): a split book drops its
/// global `ComicInfo.xml` bookmarks because their page indices do not survive
/// chunking (see docs/output.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tomes {
    /// One output file.
    Single,
    /// More than one output file, one per tome.
    Split,
}

impl Tomes {
    /// Classify a tome count (`total > 1` is a split book).
    fn from_count(total: usize) -> Self {
        if total > 1 {
            Tomes::Split
        } else {
            Tomes::Single
        }
    }
}

/// Write a processed book in the requested format, returning the output paths.
///
/// The book is split into tomes first ([`chunk::split`]); a single-tome book is
/// written exactly as before, while a split book produces one file per tome with
/// KCC's `[i/n]` title and ` <i>` filename suffix. KePub is not handled here:
/// `Options::resolve` folds it into [`OutputEncoding::Epub`] (the KePub differences live
/// in the shared EPUB builder). Light-novel mode never reaches this function —
/// [`super::convert_source`] dispatches to [`lightnovel::convert`] before the
/// normal pipeline (see docs/output.md).
pub fn write_book(
    book: ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
) -> Result<Vec<PathBuf>> {
    let tomes = chunk::split(book, options)?;
    let total = tomes.len();
    // A split book drops its `ComicInfo.xml` bookmarks: their page indices are
    // global and do not survive chunking (KCC's `ischunked`).
    let tome_kind = Tomes::from_count(total);

    let mut written = Vec::new();
    for (index, tome) in tomes.iter().enumerate() {
        let number = index + 1;
        let title = tome_title(&prepared.metadata.title, number, total, tome_kind);
        let suffix = match tome_kind {
            Tomes::Split => format!(" {number}"),
            Tomes::Single => String::new(),
        };
        written.extend(write_tome(
            tome, prepared, source, options, &title, &suffix, tome_kind,
        )?);
    }
    Ok(written)
}

/// KCC's per-tome title: `base [i/n]`, zero-padded once there are ten or more
/// tomes, and the bare base title for a single tome (`makeBook`). The split
/// decision comes from [`Tomes`] rather than a second `total > 1` test.
fn tome_title(base: &str, number: usize, total: usize, tomes: Tomes) -> String {
    match tomes {
        Tomes::Single => base.to_string(),
        Tomes::Split if total > 9 => format!("{base} [{number:02}/{total:02}]"),
        Tomes::Split => format!("{base} [{number}/{total}]"),
    }
}

/// Write one tome, resolving its file name from the tome suffix.
#[allow(clippy::too_many_arguments)]
fn write_tome(
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
    title: &str,
    suffix: &str,
    tomes: Tomes,
) -> Result<Vec<PathBuf>> {
    match options.output.encoding {
        OutputEncoding::Epub { .. } | OutputEncoding::Kepub { .. } => {
            let dest = naming::output_filename(
                source,
                options.output.destination.as_deref(),
                ".epub",
                suffix,
                options,
            );
            epub::build_epub(&dest, book, prepared, source, options, title, tomes)?;
            Ok(vec![dest])
        }
        OutputEncoding::Cbz => {
            let dest = naming::output_filename(
                source,
                options.output.destination.as_deref(),
                ".cbz",
                suffix,
                options,
            );
            cbz::build_cbz(&dest, book, prepared)?;
            Ok(vec![dest])
        }
        OutputEncoding::Pdf => {
            let dest = naming::output_filename(
                source,
                options.output.destination.as_deref(),
                ".pdf",
                suffix,
                options,
            );
            pdf::build_pdf(&dest, book, prepared, title)?;
            Ok(vec![dest])
        }
        OutputEncoding::Mobi { keep_epub } => {
            // KCC always builds the fixed-layout EPUB first and derives the
            // Kindle file name from it by replacing the extension
            // (`makeMOBIFix`); the intermediate EPUB survives only under
            // `mobi+epub` (see docs/output.md).
            let epub_dest = naming::output_filename(
                source,
                options.output.destination.as_deref(),
                ".epub",
                suffix,
                options,
            );
            let kindle_dest = epub_dest.with_extension("mobi");
            kindle::build_kindle(
                &epub_dest,
                &kindle_dest,
                book,
                prepared,
                source,
                options,
                title,
                tomes,
            )?;

            let mut written = vec![kindle_dest];
            if keep_epub {
                written.push(epub_dest);
            }
            Ok(written)
        }
        OutputEncoding::Azw3 => {
            // As [`OutputEncoding::Mobi`], but the intermediate EPUB is never
            // kept (`makeMOBIFix` picks the `.azw3` extension).
            let epub_dest = naming::output_filename(
                source,
                options.output.destination.as_deref(),
                ".epub",
                suffix,
                options,
            );
            let kindle_dest = epub_dest.with_extension("azw3");
            kindle::build_kindle(
                &epub_dest,
                &kindle_dest,
                book,
                prepared,
                source,
                options,
                title,
                tomes,
            )?;

            Ok(vec![kindle_dest])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{tome_title, Tomes};

    #[test]
    fn tome_titles_follow_kccs_numbering() {
        assert_eq!(tome_title("Book", 1, 1, Tomes::from_count(1)), "Book");
        assert_eq!(tome_title("Book", 1, 2, Tomes::from_count(2)), "Book [1/2]");
        assert_eq!(tome_title("Book", 2, 9, Tomes::from_count(9)), "Book [2/9]");
        assert_eq!(
            tome_title("Book", 1, 10, Tomes::from_count(10)),
            "Book [01/10]"
        );
        assert_eq!(
            tome_title("Book", 12, 100, Tomes::from_count(100)),
            "Book [12/100]"
        );
    }
}
