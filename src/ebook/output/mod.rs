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
    let drop_bookmarks = total > 1;

    let mut written = Vec::new();
    for (index, tome) in tomes.iter().enumerate() {
        let number = index + 1;
        let title = tome_title(&prepared.metadata.title, number, total);
        let suffix = if total > 1 {
            format!(" {number}")
        } else {
            String::new()
        };
        written.extend(write_tome(
            tome,
            prepared,
            source,
            options,
            &title,
            &suffix,
            drop_bookmarks,
        )?);
    }
    Ok(written)
}

/// KCC's per-tome title: `base [i/n]`, zero-padded once there are ten or more
/// tomes, and the bare base title for a single tome (`makeBook`).
fn tome_title(base: &str, number: usize, total: usize) -> String {
    if total > 9 {
        format!("{base} [{number:02}/{total:02}]")
    } else if total > 1 {
        format!("{base} [{number}/{total}]")
    } else {
        base.to_string()
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
    drop_bookmarks: bool,
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
            epub::build_epub(
                &dest,
                book,
                prepared,
                source,
                options,
                title,
                drop_bookmarks,
            )?;
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
                drop_bookmarks,
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
                drop_bookmarks,
            )?;

            Ok(vec![kindle_dest])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tome_title;

    #[test]
    fn tome_titles_follow_kccs_numbering() {
        assert_eq!(tome_title("Book", 1, 1), "Book");
        assert_eq!(tome_title("Book", 1, 2), "Book [1/2]");
        assert_eq!(tome_title("Book", 2, 9), "Book [2/9]");
        assert_eq!(tome_title("Book", 1, 10), "Book [01/10]");
        assert_eq!(tome_title("Book", 12, 100), "Book [12/100]");
    }
}
