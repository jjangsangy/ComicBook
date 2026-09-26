//! Fixed-layout EPUB 3 packaging (AGENTS.md §12.2).
//!
//! [`build_epub`] is the Rust counterpart of KCC's `buildEPUB`: it turns the
//! processed pages, the resolved metadata and the cover into the OEBPS tree and
//! zips it as an EPUB. Where KCC writes the tree to a temp directory and then
//! walks it, every derived document here is built in memory and streamed into the
//! archive, so there is no intermediate copy of the book (AGENTS.md §5.1).

pub mod nav;
pub mod opf;
pub mod package;
pub mod xhtml;

mod templates;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use time::macros::format_description;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ebook::model::{EncodedPage, MediaType, PageFlags};
use crate::ebook::options::Options;
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

/// One page as the EPUB builders see it (AGENTS.md §12.2).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageRef<'a> {
    /// Chapter directory relative to `OEBPS/Images` (`""` at the root).
    pub image_dir: &'a str,
    /// Image file name including its extension (`kcc-0001-kcc-x.jpg`).
    pub file: &'a str,
    /// File name without its extension (`kcc-0001-kcc-x`).
    pub stem: &'a str,
    pub width: u32,
    pub height: u32,
    pub flags: PageFlags,
    pub media_type: MediaType,
    /// The `-below` companion of a Kindle Scribe `-above` page, if any.
    pub below: Option<&'a EncodedPage>,
}

/// Build the EPUB for `book` and write it to `dest`.
pub fn build_epub(
    dest: &Path,
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
) -> Result<()> {
    let entries = build_entries(book, prepared, source, options);
    package::write_epub(dest, &entries)
}

/// Build the OEBPS entries (`zip path` → bytes) for `book`.
///
/// Split out from [`build_epub`] so the Kindle path (Phase 8) can materialise
/// the exact same tree into a scratch directory for `kindling`, without a zip
/// round-trip (AGENTS.md §5.1, §9).
pub(crate) fn build_entries(
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
) -> Vec<(String, Vec<u8>)> {
    let uuid = Uuid::new_v4().to_string();
    let modified = modified_timestamp();

    // Flatten the processed chapters into the page list KCC's `os.walk` produces.
    // A Scribe `-below` image is skipped: it is only referenced from its `-above`
    // page's XHTML/manifest entry, never a spine item of its own.
    let mut filelist: Vec<PageRef<'_>> = Vec::new();
    let mut chapter_starts: Vec<usize> = Vec::new();
    for chapter in &book.chapters {
        if chapter.pages.is_empty() {
            continue;
        }
        let dir = chapter.name.trim_matches('/');
        chapter_starts.push(filelist.len());
        let mut index = 0;
        while index < chapter.pages.len() {
            let page = &chapter.pages[index];
            if page.flags.below {
                index += 1;
                continue;
            }
            let below = if page.flags.above {
                chapter.pages.get(index + 1).filter(|next| next.flags.below)
            } else {
                None
            };
            let file = page.name.rsplit('/').next().unwrap_or(page.name.as_str());
            filelist.push(PageRef {
                image_dir: dir,
                file,
                stem: stem_of(file),
                width: page.width,
                height: page.height,
                flags: page.flags,
                media_type: page.media_type,
                below,
            });
            index += 1 + usize::from(below.is_some());
        }
    }

    // One navigation entry per chapter, or one per ComicInfo bookmark when the
    // book carries them (KCC's `comicinfo_chapters`).
    let mut page_titles: HashMap<String, String> = HashMap::new();
    let entries = if prepared.metadata.bookmarks.is_empty() {
        chapter_starts
    } else {
        bookmark_entries(
            &filelist,
            &prepared.metadata.bookmarks,
            options.splitter,
            &mut page_titles,
        )
    };

    let cover = book.cover.as_ref().map(|cover| cover.bytes.as_slice());

    let mut zip_entries: Vec<(String, Vec<u8>)> = Vec::new();
    zip_entries.push((
        "META-INF/container.xml".to_string(),
        opf::CONTAINER_XML.as_bytes().to_vec(),
    ));
    zip_entries.push((
        "OEBPS/Text/style.css".to_string(),
        opf::style_css(options).into_bytes(),
    ));
    if let Some(bytes) = cover {
        zip_entries.push(("OEBPS/Images/cover.jpg".to_string(), bytes.to_vec()));
    }
    // Every processed page — including a Scribe `-below` companion — is written to
    // `OEBPS/Images`, whether or not it is a spine item.
    for chapter in &book.chapters {
        let dir = chapter.name.trim_matches('/');
        for page in &chapter.pages {
            let file = page.name.rsplit('/').next().unwrap_or(page.name.as_str());
            zip_entries.push((
                format!("OEBPS/{}/{}", images_dir(dir), file),
                page.bytes.clone(),
            ));
        }
    }
    for page in &filelist {
        let bytes = xhtml::build_xhtml(page, options);
        zip_entries.push((
            format!("OEBPS/{}/{}.xhtml", text_dir(page.image_dir), page.stem),
            bytes,
        ));
    }

    let title = &prepared.metadata.title;
    zip_entries.push((
        "OEBPS/toc.ncx".to_string(),
        nav::build_ncx(
            title,
            &entries,
            &filelist,
            &prepared.sanitized.chapter_titles,
            &page_titles,
            &options.language,
            &uuid,
        )
        .into_bytes(),
    ));
    zip_entries.push((
        "OEBPS/nav.xhtml".to_string(),
        nav::build_nav(
            title,
            &entries,
            &filelist,
            &prepared.sanitized.chapter_titles,
            &page_titles,
        )
        .into_bytes(),
    ));
    zip_entries.push((
        "OEBPS/content.opf".to_string(),
        opf::build_opf(
            title,
            &filelist,
            cover.is_some(),
            source,
            &prepared.metadata,
            &options.language,
            &uuid,
            &modified,
            options,
        )
        .into_bytes(),
    ));

    zip_entries
}

/// The current UTC time as KCC's `dcterms:modified` (`%Y-%m-%dT%H:%M:%SZ`).
fn modified_timestamp() -> String {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    OffsetDateTime::now_utc()
        .format(format)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Rebuild the navigation entries from `ComicInfo.xml` bookmarks.
///
/// Mirrors KCC's `comicinfo_chapters` loop: the `Page/@Image` index is advanced
/// past every `-kcc-b` split half encountered, so bookmark indices keep pointing
/// at the right page after spreads were bisected.
fn bookmark_entries(
    filelist: &[PageRef<'_>],
    bookmarks: &[(usize, String)],
    splitter: u8,
    page_titles: &mut HashMap<String, String>,
) -> Vec<usize> {
    let diff_delta = match splitter {
        0 => 1,
        2 => 2,
        _ => 0,
    };
    let mut entries = Vec::new();
    let mut global_diff = 0usize;

    for (image, title) in bookmarks {
        let mut pageid = *image;
        let cur_diff = global_diff;
        global_diff = 0;

        let mut in_range = true;
        for x in 0..=pageid.saturating_add(cur_diff) {
            match filelist.get(x) {
                Some(entry) if entry.file.contains("-kcc-b") => {
                    pageid += diff_delta;
                    global_diff += diff_delta;
                }
                Some(_) => {}
                None => {
                    in_range = false;
                    break;
                }
            }
        }
        // A bookmark past the end of the book is skipped rather than panicking
        // (KCC would raise an `IndexError`).
        if !in_range {
            continue;
        }
        let Some(entry) = filelist.get(pageid) else {
            continue;
        };
        entries.push(pageid);
        page_titles.insert(entry.file.to_string(), title.clone());
    }
    entries
}

/// A file name without its extension (Python's `os.path.splitext(...)[0]`).
fn stem_of(file: &str) -> &str {
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    }
}

/// `OEBPS/Images` or `OEBPS/Images/<chapter>`.
pub(crate) fn images_dir(image_dir: &str) -> String {
    if image_dir.is_empty() {
        "Images".to_string()
    } else {
        format!("Images/{image_dir}")
    }
}

/// `OEBPS/Text` or `OEBPS/Text/<chapter>`.
pub(crate) fn text_dir(image_dir: &str) -> String {
    if image_dir.is_empty() {
        "Text".to_string()
    } else {
        format!("Text/{image_dir}")
    }
}

/// KCC's manifest `uniqueid` for a page: the Images path with separators folded.
pub(crate) fn unique_id(entry: &PageRef<'_>) -> String {
    format!("{}/{}", images_dir(entry.image_dir), entry.stem).replace('/', "_")
}

/// Python's `html.escape(value)` with `quote=True`.
pub(crate) fn html_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_drops_the_last_extension() {
        assert_eq!(stem_of("kcc-0001-kcc-x.jpg"), "kcc-0001-kcc-x");
        assert_eq!(stem_of("archive.tar.gz"), "archive.tar");
        // A dotless or leading-dot name keeps its whole name, as `splitext` does.
        assert_eq!(stem_of("kcc-0001"), "kcc-0001");
        assert_eq!(stem_of(".hidden"), ".hidden");
    }

    #[test]
    fn directories_are_prefixed_and_joined() {
        assert_eq!(images_dir(""), "Images");
        assert_eq!(images_dir("Chapter 1/Sub"), "Images/Chapter 1/Sub");
        assert_eq!(text_dir(""), "Text");
        assert_eq!(text_dir("Chapter 1/Sub"), "Text/Chapter 1/Sub");
    }
}
