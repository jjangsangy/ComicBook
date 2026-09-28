//! In-memory comic data model (see docs/architecture.md).
//!
//! The pipeline is built around a [`ComicTree`] of chapters and pages rather
//! than KCC's double temp-directory tree, so a source is decoded once and never
//! copied through the filesystem in the common case.
//!
//! To keep peak memory linear in the *output* rather than in the decoded book, a
//! [`Page`] does not decode its pixels at ingest: it carries the encoded source
//! bytes plus the header dimensions, and [`Page::ensure_decoded`] decodes on
//! demand. The processing pass releases each page's pixels again as soon as it
//! has been encoded, so only the in-flight pages (one per `rayon` worker) are
//! ever decoded at once.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView};
use std::path::PathBuf;

use crate::units::Size;

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
/// is load-bearing for the OPF spread algorithm (see docs/architecture.md).
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
/// Kept alongside the bytes so the OPF manifest (see docs/output.md) can emit the
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

/// A single source page.
///
/// The pixels are decoded lazily: [`Page::image`] is `None` between ingest and
/// processing (and again once a processed page has been encoded), while
/// [`Page::raw`] holds the encoded source bytes and [`Page::dimensions`] the
/// header dimensions. This bounds peak memory to the encoded book plus the few
/// pages a `rayon` batch is actively decoding, instead of the whole decoded book
/// (see docs/architecture.md).
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
    /// Decoded pixels, or `None` until the page is processed.
    ///
    /// Exists only while the page is being transformed/encoded; see the type
    /// docs. A page may instead be *pixel-only* (webtoon strips): then `image` is
    /// `Some` and `raw` is `None`.
    pub image: Option<DynamicImage>,
    /// The source image's dimensions, read from the codec header at ingest so an
    /// undersized-page check does not need to decode the whole image.
    pub dimensions: Size,
    pub background: Background,
    pub flags: PageFlags,
    /// The source's original encoded bytes.
    ///
    /// Retained as the lazy decode source and so `--no-processing` can emit the
    /// page byte-for-byte instead of re-encoding the decoded pixels (see
    /// docs/architecture.md). `None` for a page that exists only as pixels (the
    /// webtoon merge) or once `--no-processing` has moved the bytes into its
    /// output page.
    pub raw: Option<Vec<u8>>,
    /// Media type of [`Page::raw`], inferred from the source extension.
    pub source_media_type: Option<MediaType>,
}

impl Page {
    /// The source image's dimensions, available without decoding.
    pub fn dimensions(&self) -> Size {
        self.dimensions
    }

    /// The decoded pixels, if this page has already been decoded.
    pub fn decoded(&self) -> Option<&DynamicImage> {
        self.image.as_ref()
    }

    /// Decode the source bytes into [`Page::image`] if not already decoded.
    ///
    /// A page backed by pixels (webtoon output, test helpers) is returned as-is;
    /// a page backed only by [`Page::raw`] is decoded once and cached until
    /// [`Page::take_image`] releases it.
    pub fn ensure_decoded(&mut self) -> Result<&DynamicImage> {
        if self.image.is_none() {
            let raw = self
                .raw
                .as_deref()
                .context("page holds neither decoded pixels nor source bytes")?;
            let decoded = image::load_from_memory(raw).context("image could not be decoded")?;
            self.dimensions = Size::from_dimensions(decoded.dimensions());
            self.image = Some(decoded);
        }
        self.image.as_ref().context("page has no decoded image")
    }

    /// Decode into an owned image without caching the result.
    ///
    /// Used by one-off passes (the cover) that must not pin a decoded book.
    pub fn to_decoded(&self) -> Result<DynamicImage> {
        match &self.image {
            Some(image) => Ok(image.clone()),
            None => {
                let raw = self
                    .raw
                    .as_deref()
                    .context("page holds neither decoded pixels nor source bytes")?;
                image::load_from_memory(raw).context("image could not be decoded")
            }
        }
    }

    /// Release the decoded pixels, keeping any encoded source bytes.
    pub fn take_image(&mut self) -> Option<DynamicImage> {
        self.image.take()
    }
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
/// (see docs/architecture.md).
#[derive(Debug, Clone)]
pub struct EncodedPage {
    /// Output file name, including the `-kcc-<order>` suffix and the media
    /// extension. Set by the naming pass from the sanitized page name.
    pub name: String,
    /// The `-kcc-<order>` class that drives the OPF spread algorithm.
    pub order_class: OrderClass,
    pub media_type: MediaType,
    pub bytes: Vec<u8>,
    pub size: Size,
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
    /// `--keep-comicinfo` can round-trip it and the metadata pass can parse it
    /// once, on demand.
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
