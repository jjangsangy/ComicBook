//! Per-page image processing pipeline (AGENTS.md §11).
//!
//! [`process_tree`] is the Rust counterpart of KCC's `imgDirectoryProcessing`: it
//! detects each page's background, runs the per-page transform/encode pipeline in
//! parallel with `rayon`, and returns the encoded pages in reading order.
//!
//! Cropping and the moiré eraser are Phase 3; their modules exist as documented
//! skeletons so the later phase can slot them into [`page::process_page`].

pub mod color;
pub mod cover;
pub mod crop;
pub mod fill;
pub mod interpanel;
pub mod page;
pub mod rainbow;
pub mod webtoon;

pub use page::process_page;

use anyhow::Result;
use rayon::prelude::*;

use crate::ebook::model::{ComicTree, EncodedPage};
use crate::ebook::options::Options;
use crate::ebook::progress;

/// The encoded pages of one chapter, in reading order.
#[derive(Debug, Clone)]
pub struct ProcessedChapter {
    /// The chapter's source directory path (before slugification).
    pub name: String,
    pub pages: Vec<EncodedPage>,
}

/// Everything the processing stage produces for one book.
#[derive(Debug, Clone)]
pub struct ProcessedBook {
    pub chapters: Vec<ProcessedChapter>,
    /// The processed cover, once Phase 6's `Cover::process` lands.
    pub cover: Option<EncodedPage>,
    /// Total encoded pages, which may exceed the source page count when spreads
    /// were bisected.
    pub page_count: usize,
}

/// Process every page of a tree into encoded images.
///
/// The tree is taken mutably because background detection is cached on each page
/// for the later cropping phases, mirroring KCC's `ComicPageParser`.
pub fn process_tree(tree: &mut ComicTree, options: &Options) -> Result<ProcessedBook> {
    let size = page::profile_size(options);
    let bar = progress::bar(tree.page_count() as u64, "Processing images");

    let mut chapters = Vec::with_capacity(tree.chapters.len());
    for chapter in &mut tree.chapters {
        let encoded = chapter
            .pages
            .par_iter_mut()
            .map(|page| {
                if !options.no_processing {
                    page.background = fill::fill_check(&page.image);
                }
                let result = page::process_page(page, options, size);
                bar.inc(1);
                result
            })
            .collect::<Result<Vec<_>>>()?;

        chapters.push(ProcessedChapter {
            name: chapter.name.clone(),
            pages: encoded.into_iter().flatten().collect(),
        });
    }
    bar.finish_and_clear();

    let page_count = chapters.iter().map(|chapter| chapter.pages.len()).sum();

    Ok(ProcessedBook {
        chapters,
        cover: None,
        page_count,
    })
}
