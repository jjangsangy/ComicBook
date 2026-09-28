//! Fixed-layout EPUB 3 packaging (see docs/output.md).
//!
//! [`build_epub`] is the Rust counterpart of KCC's `buildEPUB`: it turns the
//! processed pages, the resolved metadata and the cover into the OEBPS tree and
//! zips it as an EPUB. Where KCC writes the tree to a temp directory and then
//! walks it, every derived document here is built in memory and streamed into the
//! archive, so there is no intermediate copy of the book (see docs/architecture.md).

pub mod nav;
pub mod opf;
pub mod package;
pub mod xhtml;

mod templates;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use anyhow::Result;
use time::macros::format_description;
use time::OffsetDateTime;
use uuid::Uuid;

use self::package::{EpubEntries, ZipEntry};
use super::Tomes;
use crate::ebook::model::{EncodedPage, MediaType, OrderClass, PageFlags, ScribeHalf};
use crate::ebook::options::{Options, Splitter};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;
use crate::units::Size;

/// A page's chapter directory relative to `OEBPS/Images` (`""` at the root).
///
/// `#[repr(transparent)]` over `&str`: layout-identical to the old bare field, no
/// allocation, still `Copy`. Distinct from [`FileName`]/[`Stem`] so the three
/// `PageRef` strings can no longer be swapped (REFACTOR.md D12).
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImageDir<'a>(&'a str);

impl<'a> ImageDir<'a> {
    pub(crate) const fn new(value: &'a str) -> Self {
        ImageDir(value)
    }

    pub(crate) const fn as_str(self) -> &'a str {
        self.0
    }
}

impl fmt::Display for ImageDir<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// An image file name including its extension (`kcc-0001-kcc-x.jpg`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileName<'a>(&'a str);

impl<'a> FileName<'a> {
    pub(crate) const fn new(value: &'a str) -> Self {
        FileName(value)
    }

    pub(crate) const fn as_str(self) -> &'a str {
        self.0
    }
}

impl fmt::Display for FileName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A file name without its extension (`kcc-0001-kcc-x`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Stem<'a>(&'a str);

impl<'a> Stem<'a> {
    pub(crate) const fn new(value: &'a str) -> Self {
        Stem(value)
    }

    pub(crate) const fn as_str(self) -> &'a str {
        self.0
    }
}

impl fmt::Display for Stem<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// One page as the EPUB builders see it (see docs/output.md).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageRef<'a> {
    /// Chapter directory relative to `OEBPS/Images` (`""` at the root).
    pub image_dir: ImageDir<'a>,
    /// Image file name including its extension (`kcc-0001-kcc-x.jpg`).
    pub file: FileName<'a>,
    pub size: Size,
    pub flags: PageFlags,
    /// How the page participates in the spread split (drives the OPF page-spread
    /// algorithm); carried as the enum so `opf` never re-parses the name suffix.
    pub order_class: OrderClass,
    pub media_type: MediaType,
    /// The `-below` companion of a Kindle Scribe `-above` page, if any.
    pub below: Option<&'a EncodedPage>,
}

impl<'a> PageRef<'a> {
    /// The page image's file name without its extension (the `<title>` and the
    /// XHTML file name).
    ///
    /// Derived from [`PageRef::file`] rather than stored beside it, so the two can
    /// never disagree (a stored copy was only kept in sync by convention).
    pub(crate) fn stem(&self) -> Stem<'a> {
        stem_of(self.file)
    }
}

/// Build the EPUB for `book` and write it to `dest`.
///
/// `title` is the tome's title (the base title for a single-tome book, or
/// `base [i/n]` when the book was split); `tomes` distinguishes a split book,
/// whose `ComicInfo.xml` bookmarks are discarded because their global page
/// indices do not survive chunking (KCC's `ischunked`).
pub fn build_epub(
    dest: &Path,
    book: &ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
    title: &str,
    tomes: Tomes,
) -> Result<()> {
    let entries = build_entries(book, prepared, source, options, title, tomes)?;
    package::write_epub(dest, &entries)
}

/// Build the OEBPS entries (`zip path` → bytes) for `book`.
///
/// Split out from [`build_epub`] so the Kindle path can materialise
/// the exact same tree into a scratch directory for `kindling`, without a zip
/// round-trip (see docs/architecture.md and docs/output.md).
///
/// Page images are borrowed from the [`ProcessedBook`], so building the entry list
/// does not duplicate the encoded book in memory; only the small derived documents
/// (XHTML/NCX/NAV/OPF) are owned.
pub(crate) fn build_entries<'a>(
    book: &'a ProcessedBook,
    prepared: &PreparedBook,
    source: &Path,
    options: &Options,
    title: &str,
    tomes: Tomes,
) -> Result<EpubEntries<'a>> {
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
        let dir = ImageDir::new(chapter.name.as_str().trim_matches('/'));
        chapter_starts.push(filelist.len());
        let mut index = 0;
        while index < chapter.pages.len() {
            let page = &chapter.pages[index];
            // A Scribe `-below` image is emitted only through its `-above` page's
            // XHTML/manifest entry, never as a spine item of its own.
            let below = match page.flags.half {
                ScribeHalf::Below => {
                    index += 1;
                    continue;
                }
                ScribeHalf::Above => chapter
                    .pages
                    .get(index + 1)
                    .filter(|next| next.flags.half == ScribeHalf::Below),
                ScribeHalf::NotSplit => None,
            };
            let file = file_name(page.name.as_str());
            filelist.push(PageRef {
                image_dir: dir,
                file,
                size: page.size,
                flags: page.flags,
                order_class: page.order_class,
                media_type: page.media_type,
                below,
            });
            index += 1 + usize::from(below.is_some());
        }
    }

    // One navigation entry per chapter, or one per ComicInfo bookmark when the
    // book carries them (KCC's `comicinfo_chapters`). A split book drops the
    // bookmarks entirely (KCC resets `comicinfo_chapters` per split tome).
    let bookmarks: &[(usize, String)] = match tomes {
        Tomes::Single => &prepared.metadata.bookmarks,
        Tomes::Split => &[],
    };
    let mut page_titles: HashMap<String, String> = HashMap::new();
    let entries = if bookmarks.is_empty() {
        chapter_starts
    } else {
        bookmark_entries(
            &filelist,
            bookmarks,
            options.processing.splitter,
            &mut page_titles,
        )
    };

    let cover = book.cover.as_ref().map(|cover| cover.page.bytes.as_slice());

    let mut documents: Vec<ZipEntry<'a>> = Vec::new();
    documents.push(ZipEntry::borrowed(
        "META-INF/container.xml",
        opf::CONTAINER_XML.as_bytes(),
    ));
    documents.push(ZipEntry::owned(
        "OEBPS/Text/style.css",
        opf::style_css(options)?.into_bytes(),
    ));
    if let Some(bytes) = cover {
        documents.push(ZipEntry::borrowed("OEBPS/Images/cover.jpg", bytes));
    }
    // Every processed page — including a Scribe `-below` companion — is written to
    // `OEBPS/Images`, whether or not it is a spine item.
    for chapter in &book.chapters {
        let dir = ImageDir::new(chapter.name.as_str().trim_matches('/'));
        for page in &chapter.pages {
            let file = file_name(page.name.as_str());
            documents.push(ZipEntry::borrowed(
                format!("OEBPS/{}/{}", images_dir(dir), file),
                page.bytes.as_slice(),
            ));
        }
    }
    for page in &filelist {
        let bytes = xhtml::build_xhtml(page, options)?;
        documents.push(ZipEntry::owned(
            format!("OEBPS/{}/{}.xhtml", text_dir(page.image_dir), page.stem()),
            bytes,
        ));
    }

    documents.push(ZipEntry::owned(
        "OEBPS/toc.ncx",
        nav::build_ncx(
            title,
            &entries,
            &filelist,
            &prepared.sanitized.chapter_titles,
            &page_titles,
            &options.output.language,
            &uuid,
        )?
        .into_bytes(),
    ));
    documents.push(ZipEntry::owned(
        "OEBPS/nav.xhtml",
        nav::build_nav(
            title,
            &entries,
            &filelist,
            &prepared.sanitized.chapter_titles,
            &page_titles,
        )?
        .into_bytes(),
    ));
    documents.push(ZipEntry::owned(
        "OEBPS/content.opf",
        opf::build_opf(
            title,
            &filelist,
            cover.is_some(),
            source,
            &prepared.metadata,
            &options.output.language,
            &uuid,
            &modified,
            options,
        )?
        .into_bytes(),
    ));

    Ok(EpubEntries::new(documents))
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
/// past every [`OrderClass::SplitLeft`] half encountered, so bookmark indices keep
/// pointing at the right page after spreads were bisected.
fn bookmark_entries(
    filelist: &[PageRef<'_>],
    bookmarks: &[(usize, String)],
    splitter: Splitter,
    page_titles: &mut HashMap<String, String>,
) -> Vec<usize> {
    let diff_delta = match splitter {
        Splitter::Split => 1,
        Splitter::Both => 2,
        // Rotated spreads keep their original indices.
        Splitter::Rotate => 0,
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
                Some(entry) if entry.order_class == OrderClass::SplitLeft => {
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
        page_titles.insert(entry.file.as_str().to_string(), title.clone());
    }
    entries
}

/// The last `/`-separated segment of a page name.
///
/// `rsplit_once` returns `None` only when there is no `/`, so no dead `unwrap_or`
/// fallback is needed (REFACTOR.md E5).
fn file_name(name: &str) -> FileName<'_> {
    match name.rsplit_once('/') {
        Some((_, file)) => FileName::new(file),
        None => FileName::new(name),
    }
}

/// A file name without its extension (Python's `os.path.splitext(...)[0]`).
fn stem_of(file: FileName<'_>) -> Stem<'_> {
    let name = file.as_str();
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => Stem::new(stem),
        _ => Stem::new(name),
    }
}

/// `OEBPS/Images` or `OEBPS/Images/<chapter>`.
pub(crate) fn images_dir(image_dir: ImageDir<'_>) -> String {
    if image_dir.as_str().is_empty() {
        "Images".to_string()
    } else {
        format!("Images/{image_dir}")
    }
}

/// `OEBPS/Text` or `OEBPS/Text/<chapter>`.
pub(crate) fn text_dir(image_dir: ImageDir<'_>) -> String {
    if image_dir.as_str().is_empty() {
        "Text".to_string()
    } else {
        format!("Text/{image_dir}")
    }
}

/// KCC's manifest `uniqueid` for a page: the Images path with separators folded.
pub(crate) fn unique_id(entry: &PageRef<'_>) -> String {
    format!("{}/{}", images_dir(entry.image_dir), entry.stem()).replace('/', "_")
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
        assert_eq!(
            stem_of(FileName::new("kcc-0001-kcc-x.jpg")).as_str(),
            "kcc-0001-kcc-x"
        );
        assert_eq!(
            stem_of(FileName::new("archive.tar.gz")).as_str(),
            "archive.tar"
        );
        // A dotless or leading-dot name keeps its whole name, as `splitext` does.
        assert_eq!(stem_of(FileName::new("kcc-0001")).as_str(), "kcc-0001");
        assert_eq!(stem_of(FileName::new(".hidden")).as_str(), ".hidden");
    }

    #[test]
    fn directories_are_prefixed_and_joined() {
        assert_eq!(images_dir(ImageDir::new("")), "Images");
        assert_eq!(
            images_dir(ImageDir::new("Chapter 1/Sub")),
            "Images/Chapter 1/Sub"
        );
        assert_eq!(text_dir(ImageDir::new("")), "Text");
        assert_eq!(
            text_dir(ImageDir::new("Chapter 1/Sub")),
            "Text/Chapter 1/Sub"
        );
    }
}
