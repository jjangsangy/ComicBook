//! In-memory comic data model (AGENTS.md §10).
//!
//! The pipeline is built around a [`ComicTree`] of chapters and pages rather
//! than KCC's double temp-directory tree, so a source is decoded once and never
//! copied through the filesystem in the common case.

use image::DynamicImage;
use std::path::PathBuf;

/// Detected page background, used for fill/crop decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Background {
    #[default]
    White,
    Black,
}

/// How a page participates in spread splitting.
///
/// The KCC order suffix (`-kcc-x`, `-kcc-a` … `-kcc-d`) is derived from this and
/// is load-bearing for the OPF spread algorithm (AGENTS.md §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrderClass {
    #[default]
    Normal,
    RotateFirst,
    RotateLast,
    SplitLeft,
    SplitRight,
}

impl OrderClass {
    /// The KCC order suffix (without the leading dash).
    pub fn suffix(self) -> &'static str {
        match self {
            OrderClass::Normal => "x",
            OrderClass::RotateFirst => "a",
            OrderClass::RotateLast => "d",
            OrderClass::SplitLeft => "b",
            OrderClass::SplitRight => "c",
        }
    }
}

/// An encoded page image's media type.
///
/// Kept alongside the bytes so the OPF manifest (AGENTS.md §12.2) can emit the
/// matching `media-type` without re-sniffing the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Jpeg,
    Png,
    Gif,
    WebP,
}

impl MediaType {
    /// Canonical file extension (without the dot).
    pub fn extension(self) -> &'static str {
        match self {
            MediaType::Jpeg => "jpg",
            MediaType::Png => "png",
            MediaType::Gif => "gif",
            MediaType::WebP => "webp",
        }
    }

    /// The MIME type written into the EPUB OPF manifest.
    pub fn mime(self) -> &'static str {
        match self {
            MediaType::Jpeg => "image/jpeg",
            MediaType::Png => "image/png",
            MediaType::Gif => "image/gif",
            MediaType::WebP => "image/webp",
        }
    }

    /// Map a (dotless, lower-cased) file extension to a media type.
    pub fn from_extension(extension: &str) -> Option<MediaType> {
        match extension {
            "jpg" | "jpeg" => Some(MediaType::Jpeg),
            "png" => Some(MediaType::Png),
            "gif" => Some(MediaType::Gif),
            "webp" => Some(MediaType::WebP),
            _ => None,
        }
    }
}

/// Page-level flags carried through processing into output naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageFlags {
    pub order_class: OrderClass,
    pub rotated: bool,
    pub black_background: bool,
    /// Kindle Scribe tall-page split: the upper half.
    pub above: bool,
    /// Kindle Scribe tall-page split: the lower half.
    pub below: bool,
}

/// A single decoded source page.
#[derive(Debug, Clone)]
pub struct Page {
    /// Source path within the book's image tree (see [`crate::ebook::input`]).
    ///
    /// This is the archive entry or folder-relative path *after* a single
    /// redundant root directory has been stripped, so equivalent CBZ/folder
    /// inputs yield the same value.
    pub source_name: String,
    /// Chapter-relative path (the file name within [`Chapter::name`]).
    pub rel_path: String,
    /// Decoded pixels.
    pub image: DynamicImage,
    pub background: Background,
    pub flags: PageFlags,
    /// The source's original encoded bytes.
    ///
    /// Retained so `--no-processing` can emit the page byte-for-byte instead of
    /// re-encoding the decoded pixels (AGENTS.md §5.1.5). `None` for trees built
    /// without a source payload.
    pub raw: Option<Vec<u8>>,
    /// Media type of [`Page::raw`], inferred from the source extension.
    pub source_media_type: Option<MediaType>,
}

/// A chapter (a source subdirectory, or the single implicit chapter of a file).
#[derive(Debug, Clone)]
pub struct Chapter {
    /// Source directory path relative to the image root, before slugification
    /// (empty for pages that sit directly in the root).
    pub name: String,
    pub pages: Vec<Page>,
}

/// One page produced by the processing stage.
///
/// A single source page can yield several encoded pages when the splitter
/// bisects a double-page spread, so this is the unit the output builders consume
/// (AGENTS.md §10).
#[derive(Debug, Clone)]
pub struct EncodedPage {
    /// Output file name, including the `-kcc-<order>` suffix and the media
    /// extension. Phase 4 replaces the source stem with the sanitized page name.
    pub name: String,
    /// The `-kcc-<order>` class that drives the OPF spread algorithm.
    pub order_class: OrderClass,
    pub media_type: MediaType,
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub flags: PageFlags,
}

/// Where a book's cover comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverSource {
    /// The first page of the first chapter.
    FirstPage,
    /// A sibling image (e.g. a `Covers/` file).
    Sibling(PathBuf),
    /// A cover synthesized by `--file-fusion`.
    Fused(PathBuf),
}

/// A fully decoded source, ready for processing.
#[derive(Debug, Clone)]
pub struct ComicTree {
    pub chapters: Vec<Chapter>,
    pub cover: Option<CoverSource>,
    /// Raw bytes of a discovered `ComicInfo.xml`, if any.
    ///
    /// The tree keeps the original document (rather than parsed fields) so that
    /// `--keep-comicinfo` can round-trip it and the metadata pass in Phase 4 can
    /// parse it once, on demand.
    pub comicinfo: Option<Vec<u8>>,
}

impl ComicTree {
    /// An empty tree.
    pub fn new() -> Self {
        ComicTree {
            chapters: Vec::new(),
            cover: None,
            comicinfo: None,
        }
    }

    /// Total number of pages across all chapters.
    pub fn page_count(&self) -> usize {
        self.chapters
            .iter()
            .map(|chapter| chapter.pages.len())
            .sum()
    }

    /// True when no chapter holds any pages.
    pub fn is_empty(&self) -> bool {
        self.chapters.iter().all(|chapter| chapter.pages.is_empty())
    }
}

impl Default for ComicTree {
    fn default() -> Self {
        Self::new()
    }
}
