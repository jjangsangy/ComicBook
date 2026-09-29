//! In-memory comic data model (see docs/architecture.md).
//!
//! The pipeline is built around a [`ComicTree`] of chapters and pages rather
//! than KCC's double temp-directory tree, so a source is decoded once and never
//! copied through the filesystem in the common case.
//!
//! To keep peak memory linear in the *output* rather than in the decoded book, a
//! [`Page`] does not decode its pixels at ingest: it carries the encoded source
//! bytes as [`PageData::Encoded`], and [`Page::ensure_decoded`] moves it to
//! [`PageData::EncodedDecoded`]. The processing pass releases each page's pixels
//! again as soon as it has been encoded, so only the in-flight pages (one per
//! `rayon` worker) are ever decoded at once.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView};
use relative_path::{RelativePath, RelativePathBuf};
use std::fmt;

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
/// The KCC order suffix (`-cb-x`, `-cb-a` … `-cb-d`) is derived from this and
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

/// Whether a processed page was rotated by the spread splitter.
///
/// Kept separate from [`OrderClass`] because `--no-rotate` can tag a page
/// `RotateFirst`/`RotateLast` while leaving it upright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    #[default]
    Upright,
    Rotated,
}

/// Which half of a Kindle Scribe tall-page split a page is.
///
/// The two `bool`s this replaces admitted an impossible `(above, below) ==
/// (true, true)`; the splitter only ever produces one half at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScribeHalf {
    /// An ordinary page, or a Scribe page that fits without splitting (`-whole`).
    #[default]
    NotSplit,
    /// The upper half (`-above`).
    Above,
    /// The lower half (`-below`).
    Below,
}

/// The background a processed page is padded with: the resolved `--borders` fill
/// (which wins over the detected [`Background`]) produced by `page_fill`.
///
/// A distinct type from [`Background`] so the *resolved* fill cannot be confused
/// with [`Page::background`], the *detected* value that drives fill/crop decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResolvedFill(Background);

impl ResolvedFill {
    /// Wrap a resolved fill value.
    pub fn new(fill: Background) -> Self {
        ResolvedFill(fill)
    }

    /// Whether the resolved fill is black (drives the black XHTML body style).
    pub fn is_black(self) -> bool {
        matches!(self.0, Background::Black)
    }
}

/// Page-level flags carried through processing into output naming.
///
/// The spread order lives on [`EncodedPage::order_class`] rather than here, so it
/// has a single owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PageFlags {
    pub orientation: Orientation,
    /// The resolved fill the page is padded with (the `--borders` override if
    /// set, otherwise the detected [`Page::background`]): drives the black XHTML
    /// body style.
    pub background: ResolvedFill,
    /// The Kindle Scribe tall-page split this page belongs to, if any.
    pub half: ScribeHalf,
}

/// Declare a `#[repr(transparent)]` newtype over a [`RelativePathBuf`] naming a
/// distinct identity, so two confusable names cannot be swapped at a call site.
///
/// The backing store is the `relative-path` crate's relative, `/`-separated path
/// (docs/refactor.md §8.1), which is the shape of every name in this pipeline; the newtype
/// still carries the pipeline-specific meaning on top. `as_relative()` hands out the
/// borrowed [`RelativePath`] so callers use `file_name`/`parent`/`file_stem`/`extension`
/// instead of splitting strings. Each newtype is layout-identical to `RelativePathBuf`
/// (zero cost) and renders through [`fmt::Display`]. It deliberately does **not**
/// implement `Deref<Target = str>`: an implicit coercion would let a name flow into any
/// `&str` slot, defeating the wrapper. Reaching the borrowed string is an explicit
/// `as_str()` at each boundary, and comparisons against string literals go through the
/// `PartialEq<str>` impls, which compare the raw bytes as the old `String` backing store
/// did (the derived `Eq`/`Hash`, by contrast, are the crate's component-wise equality).
/// No allocating conversion is exposed on a hot path.
macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name(RelativePathBuf);

        impl $name {
            /// Wrap an owned or borrowed relative path.
            pub fn new(value: impl AsRef<RelativePath>) -> Self {
                $name(value.as_ref().to_relative_path_buf())
            }

            /// The name as a borrowed string.
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }

            /// The name as a borrowed, `/`-separated [`RelativePath`].
            pub fn as_relative(&self) -> &RelativePath {
                self.0.as_relative_path()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0.as_str())
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0.as_str() == *other
            }
        }
    };
}

string_newtype! {
    /// The book-relative source path of a page: the archive entry or
    /// folder-relative path *after* a single redundant root directory has been
    /// stripped, so equivalent CBZ/folder inputs yield the same value.
    SourceName
}

string_newtype! {
    /// A page's file name within its [`Chapter`] (the basename of its
    /// [`SourceName`]).
    RelPath
}

string_newtype! {
    /// An [`EncodedPage`]'s output file name, including the `-cb-<order>` suffix
    /// and the media extension.
    PageName
}

/// A chapter's image-root-relative directory path.
///
/// The root chapter is the explicit [`ChapterName::Root`], not an empty
/// directory: `new("")` returns it, so a `Dir` is never empty and "is this the
/// root?" cannot be spelled as an empty-string comparison that a typo could
/// invert. `Dir` carries the (non-empty) directory path verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChapterName {
    /// The root chapter, whose pages sit directly in the image root.
    Root,
    /// A non-empty directory path relative to the image root.
    Dir(RelativePathBuf),
}

impl ChapterName {
    /// Build a chapter name from a directory path; the empty path is the root.
    pub fn new(path: impl AsRef<RelativePath>) -> Self {
        let path = path.as_ref();
        if path.as_str().is_empty() {
            ChapterName::Root
        } else {
            ChapterName::Dir(path.to_relative_path_buf())
        }
    }

    /// The root chapter, whose pages sit directly in the image root.
    pub fn root() -> Self {
        ChapterName::Root
    }

    /// The directory path as a borrowed string (`""` for the root).
    pub fn as_str(&self) -> &str {
        match self {
            ChapterName::Root => "",
            ChapterName::Dir(path) => path.as_str(),
        }
    }

    /// The directory path as a borrowed, `/`-separated [`RelativePath`] (`""` for
    /// the root).
    pub fn as_relative(&self) -> &RelativePath {
        match self {
            ChapterName::Root => RelativePath::new(""),
            ChapterName::Dir(path) => path.as_relative_path(),
        }
    }

    /// Whether this is the root chapter.
    pub fn is_root(&self) -> bool {
        matches!(self, ChapterName::Root)
    }
}

impl fmt::Display for ChapterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Encoded source bytes plus the media type they decode to.
///
/// Kept together so a page's lazy-decode source and the OPF manifest's
/// `media-type` cannot drift apart (see docs/architecture.md).
#[derive(Debug)]
pub struct Source {
    raw: Vec<u8>,
    media_type: MediaType,
}

impl Source {
    /// Pair encoded bytes with their media type.
    pub fn new(raw: Vec<u8>, media_type: MediaType) -> Self {
        Source { raw, media_type }
    }

    /// The encoded bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.raw
    }

    /// The media type the bytes decode to.
    pub fn media_type(&self) -> MediaType {
        self.media_type
    }
}

/// A page's image payload, as an explicit state machine.
///
/// The old representation (`image: Option<DynamicImage>`, `raw: Option<Vec<u8>>`
/// and `source_media_type: Option<MediaType>`) admitted a `(None, None)` page
/// that four call sites defended against. Every state below is reachable and
/// meaningful; the transitions (`Encoded → EncodedDecoded → Encoded`, then to
/// `Consumed`) are moves, never copies.
///
/// [`PageData::EncodedDecoded`] deliberately keeps the encoded [`Source`]
/// alongside the pixels so a decode never discards the bytes (see
/// docs/architecture.md; the memory regression tests pin this).
///
/// Deliberately **not** `Clone`: duplicating a decoded frame plus the encoded
/// bytes would silently double peak memory.
#[derive(Debug)]
pub enum PageData {
    /// Encoded bytes, not yet decoded.
    Encoded(Source),
    /// A cached decode over the retained bytes.
    EncodedDecoded(Source, DynamicImage),
    /// Pixel-only (a webtoon strip): no source bytes exist.
    Pixels(MediaType, DynamicImage),
    /// Bytes and/or pixels have been moved out; the page carries no image.
    Consumed,
}

/// A single source page.
///
/// The pixels are decoded lazily: a freshly ingested page is
/// [`PageData::Encoded`], and processing moves it through
/// [`PageData::EncodedDecoded`] and back, freeing the pixels as soon as the page
/// has been encoded. This bounds peak memory to the encoded book plus the few
/// pages a `rayon` batch is actively decoding, instead of the whole decoded book
/// (see docs/architecture.md).
#[derive(Debug)]
pub struct Page {
    /// Source path within the book's image tree (see [`crate::ebook::input`]).
    pub source_name: SourceName,
    /// Chapter-relative path (the file name within [`Chapter::name`]).
    pub rel_path: RelPath,
    /// The encoded/pixels payload; see [`PageData`].
    pub data: PageData,
    /// The source image's dimensions, read from the codec header at ingest so an
    /// undersized-page check does not need to decode the whole image.
    pub dimensions: Size,
    pub background: Background,
}

impl Page {
    /// The source image's dimensions, available without decoding.
    pub fn dimensions(&self) -> Size {
        self.dimensions
    }

    /// The decoded pixels, if this page has already been decoded.
    pub fn decoded(&self) -> Option<&DynamicImage> {
        match &self.data {
            PageData::EncodedDecoded(_, image) | PageData::Pixels(_, image) => Some(image),
            PageData::Encoded(_) | PageData::Consumed => None,
        }
    }

    /// The decoded pixels, if this page has already been decoded.
    fn decoded_mut(&mut self) -> Option<&mut DynamicImage> {
        match &mut self.data {
            PageData::EncodedDecoded(_, image) | PageData::Pixels(_, image) => Some(image),
            PageData::Encoded(_) | PageData::Consumed => None,
        }
    }

    /// The page's source media type, while it still carries bytes or pixels.
    pub fn media_type(&self) -> Option<MediaType> {
        match &self.data {
            PageData::Encoded(source) | PageData::EncodedDecoded(source, _) => {
                Some(source.media_type())
            }
            PageData::Pixels(media_type, _) => Some(*media_type),
            PageData::Consumed => None,
        }
    }

    /// The retained encoded bytes, if the page still holds any.
    pub fn source_bytes(&self) -> Option<&[u8]> {
        match &self.data {
            PageData::Encoded(source) | PageData::EncodedDecoded(source, _) => Some(source.bytes()),
            PageData::Pixels(_, _) | PageData::Consumed => None,
        }
    }

    /// Decode the source bytes into cached pixels, if not already decoded.
    ///
    /// A page backed by bytes moves `Encoded → EncodedDecoded`, keeping the bytes;
    /// a pixel-only page is returned as-is. A decode failure restores the encoded
    /// bytes rather than dropping the book. A [`PageData::Consumed`] page (its
    /// bytes were moved into an output) is an error, not a panic.
    pub fn ensure_decoded(&mut self) -> Result<&mut DynamicImage> {
        // Decode only an `Encoded` page; every other state is restored untouched.
        let decoded = match std::mem::replace(&mut self.data, PageData::Consumed) {
            PageData::Encoded(source) => match image::load_from_memory(source.bytes()) {
                Ok(decoded) => Some((source, decoded)),
                Err(error) => {
                    self.data = PageData::Encoded(source);
                    return Err(error).context("image could not be decoded");
                }
            },
            // Already decoded (reuse the cache), pixel-only, or consumed (no
            // payload to decode): restore the state unchanged.
            already
            @ (PageData::EncodedDecoded(..) | PageData::Pixels(..) | PageData::Consumed) => {
                self.data = already;
                None
            }
        };
        if let Some((source, decoded)) = decoded {
            self.dimensions = Size::from_dimensions(decoded.dimensions());
            self.data = PageData::EncodedDecoded(source, decoded);
        }
        match self.decoded_mut() {
            Some(image) => Ok(image),
            None => Err(anyhow::anyhow!(
                "page holds neither decoded pixels nor source bytes"
            )),
        }
    }

    /// Decode into an owned image without caching the result.
    ///
    /// Used by one-off passes (the cover) that must not pin a decoded book. The
    /// `Clone` only fires for a page that was already decoded (never the cover
    /// path, which runs before processing): see docs/architecture.md.
    pub fn to_decoded(&self) -> Result<DynamicImage> {
        match &self.data {
            PageData::EncodedDecoded(_, image) | PageData::Pixels(_, image) => Ok(image.clone()),
            PageData::Encoded(source) => {
                image::load_from_memory(source.bytes()).context("image could not be decoded")
            }
            PageData::Consumed => Err(anyhow::anyhow!(
                "page holds neither decoded pixels nor source bytes"
            )),
        }
    }

    /// Move the decoded pixels out, keeping any encoded source bytes.
    ///
    /// `EncodedDecoded → Encoded`; a pixel-only page becomes [`PageData::Consumed`]
    /// (it has no bytes to keep).
    pub fn take_image(&mut self) -> Option<DynamicImage> {
        match std::mem::replace(&mut self.data, PageData::Consumed) {
            PageData::EncodedDecoded(source, image) => {
                self.data = PageData::Encoded(source);
                Some(image)
            }
            PageData::Pixels(_, image) => Some(image),
            // An encoded page has no pixels to take; a consumed page has neither.
            already @ (PageData::Encoded(..) | PageData::Consumed) => {
                self.data = already;
                None
            }
        }
    }

    /// Move the encoded source bytes out.
    ///
    /// An [`PageData::Encoded`] page becomes [`PageData::Consumed`]; an
    /// already-decoded page keeps its pixels ([`PageData::EncodedDecoded`] →
    /// [`PageData::Pixels`]) so both halves of its payload are never dropped at
    /// once. Returns `None` for a pixel-only or already-consumed page, which has
    /// no bytes to give.
    pub fn take_source(&mut self) -> Option<Vec<u8>> {
        match std::mem::replace(&mut self.data, PageData::Consumed) {
            PageData::Encoded(source) => Some(source.raw),
            PageData::EncodedDecoded(source, image) => {
                self.data = PageData::Pixels(source.media_type, image);
                Some(source.raw)
            }
            // A pixel-only page has no bytes; a consumed page has neither.
            already @ (PageData::Pixels(..) | PageData::Consumed) => {
                self.data = already;
                None
            }
        }
    }
}

/// A chapter (a source subdirectory, or the single implicit chapter of a file).
#[derive(Debug)]
pub struct Chapter {
    /// Source directory path relative to the image root, before slugification
    /// ([`ChapterName::root`] for pages that sit directly in the root).
    pub name: ChapterName,
    pub pages: Vec<Page>,
}

/// One page produced by the processing stage.
///
/// A single source page can yield several encoded pages when the splitter
/// bisects a double-page spread, so this is the unit the output builders consume
/// (see docs/architecture.md).
#[derive(Debug, Clone)]
pub struct EncodedPage {
    /// Output file name, including the `-cb-<order>` suffix and the media
    /// extension. Set by the naming pass from the sanitized page name.
    pub name: PageName,
    /// The `-cb-<order>` class that drives the OPF spread algorithm.
    pub order_class: OrderClass,
    pub media_type: MediaType,
    pub bytes: Vec<u8>,
    pub size: Size,
    pub flags: PageFlags,
}

/// A fully decoded source, ready for processing.
#[derive(Debug)]
pub struct ComicTree {
    pub chapters: Vec<Chapter>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};
    use std::io::Cursor;

    /// A page whose payload is the PNG encoding of a solid image.
    fn encoded_png_page(width: u32, height: u32) -> Result<Page> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(width, height));
        let mut png = Vec::new();
        image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
        Ok(Page {
            source_name: SourceName::new("cb-0001.png"),
            rel_path: RelPath::new("cb-0001.png"),
            data: PageData::Encoded(Source::new(png, MediaType::Png)),
            dimensions: Size::new(width, height),
            background: Background::White,
        })
    }

    #[test]
    fn decoding_keeps_the_encoded_bytes_and_only_releases_the_pixels() -> Result<()> {
        let mut page = encoded_png_page(2, 3)?;
        assert!(page.decoded().is_none());

        page.ensure_decoded()?;
        assert!(page.decoded().is_some());
        assert!(
            page.source_bytes().is_some(),
            "EncodedDecoded retains the encoded bytes"
        );

        let image = page.take_image().context("the decoded pixels move out")?;
        assert_eq!(image.dimensions(), (2, 3));
        assert!(page.decoded().is_none());
        assert!(
            page.source_bytes().is_some(),
            "take_image returns the page to Encoded, not Consumed"
        );
        Ok(())
    }

    #[test]
    fn taking_the_source_bytes_consumes_the_page() -> Result<()> {
        let mut page = encoded_png_page(2, 3)?;
        assert!(page.take_source().is_some());
        assert!(page.source_bytes().is_none());
        assert!(page.decoded().is_none());
        assert_eq!(page.media_type(), None);
        assert!(
            page.ensure_decoded().is_err(),
            "a consumed page cannot be decoded"
        );
        Ok(())
    }

    #[test]
    fn a_failed_decode_restores_the_encoded_bytes() {
        let mut page = Page {
            source_name: SourceName::new("page.png"),
            rel_path: RelPath::new("page.png"),
            data: PageData::Encoded(Source::new(vec![0, 1, 2, 3], MediaType::Png)),
            dimensions: Size::new(1, 1),
            background: Background::White,
        };
        assert!(page.ensure_decoded().is_err());
        assert!(
            page.source_bytes().is_some(),
            "the encoded bytes survive a decode failure"
        );
    }

    #[test]
    fn a_pixel_only_page_has_no_source_bytes_but_keeps_its_media_type() {
        let page = Page {
            source_name: SourceName::new("cb-0001.png"),
            rel_path: RelPath::new("cb-0001.png"),
            data: PageData::Pixels(
                MediaType::WebP,
                DynamicImage::ImageRgb8(RgbImage::new(1, 1)),
            ),
            dimensions: Size::new(1, 1),
            background: Background::White,
        };
        assert!(page.source_bytes().is_none());
        assert_eq!(page.media_type(), Some(MediaType::WebP));
        assert!(page.decoded().is_some());
    }

    #[test]
    fn an_empty_chapter_path_is_the_root_variant() {
        assert_eq!(ChapterName::new(""), ChapterName::Root);
        assert_eq!(
            ChapterName::new("Chapter 1"),
            ChapterName::Dir("Chapter 1".into())
        );
        assert!(ChapterName::new("").is_root());
        assert!(!ChapterName::new("Chapter 1").is_root());
        assert_eq!(ChapterName::root().as_str(), "");
        assert_eq!(ChapterName::root().as_relative().as_str(), "");
        assert_eq!(
            ChapterName::new("Chapter 1/sub").as_relative().as_str(),
            "Chapter 1/sub"
        );
    }

    #[test]
    fn name_newtypes_expose_a_relative_view() {
        let name = SourceName::new("Chapter 1/page.jpg");
        assert_eq!(name.as_str(), "Chapter 1/page.jpg");
        assert_eq!(name.as_relative().as_str(), "Chapter 1/page.jpg");
        assert_eq!(name.as_relative().file_name(), Some("page.jpg"));
        assert_eq!(
            name.as_relative()
                .parent()
                .map(relative_path::RelativePath::as_str),
            Some("Chapter 1")
        );
    }
}
