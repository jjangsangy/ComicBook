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
    /// Original archive entry or filesystem path.
    pub source_name: String,
    /// Chapter-relative path.
    pub rel_path: String,
    /// Decoded pixels.
    pub image: DynamicImage,
    pub background: Background,
    pub flags: PageFlags,
}

/// A chapter (a source subdirectory, or the single implicit chapter of a file).
#[derive(Debug, Clone)]
pub struct Chapter {
    /// Source directory name, before slugification.
    pub name: String,
    pub pages: Vec<Page>,
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
}

impl ComicTree {
    /// An empty tree.
    pub fn new() -> Self {
        ComicTree {
            chapters: Vec::new(),
            cover: None,
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
