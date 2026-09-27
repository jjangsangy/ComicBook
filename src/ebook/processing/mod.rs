//! Per-page image processing pipeline (see docs/processing.md).
//!
//! [`process_tree`] is the Rust counterpart of KCC's `imgDirectoryProcessing`: it
//! detects each page's background, applies the configured cropping, runs the
//! per-page transform/encode pipeline in parallel with `rayon`, and returns the
//! encoded pages in reading order.

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
use image::{DynamicImage, GenericImageView};
use rayon::prelude::*;

use crate::ebook::model::{ComicTree, EncodedPage, Page};
use crate::ebook::options::Options;
use crate::ebook::progress;

/// Fraction of each inter-panel gutter KCC retains after cropping.
const INTER_PANEL_KEEP: f64 = 0.04;

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
    /// The processed cover, set by [`crate::ebook::convert_source`].
    pub cover: Option<EncodedPage>,
    /// Whether `--smart-cover-crop` actually cropped the cover (KCC's
    /// `Cover.smartcover`), which CBZ/PDF output tests before writing a cover.
    pub cover_smart_crop: bool,
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
    let first_chapter = tree
        .chapters
        .iter()
        .position(|chapter| !chapter.pages.is_empty());

    for (chapter_index, chapter) in tree.chapters.iter_mut().enumerate() {
        let encoded = chapter
            .pages
            .par_iter_mut()
            .enumerate()
            .map(|(page_index, page)| {
                if !options.no_processing {
                    let is_first_page = first_chapter == Some(chapter_index) && page_index == 0;
                    prepare_page(page, options, is_first_page);
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
        cover_smart_crop: false,
        page_count,
    })
}

/// Detect the background and crop a page before it is split and encoded.
///
/// This mirrors KCC's `ComicPageParser.__init__`: the fill is detected first, the
/// page-number/margin crop runs (from the *detected* background, not the border
/// override), then the inter-panel crop. A colour first page (the cover) is left
/// untouched, and webtoon mode skips the margin/page-number crops but still runs
/// the inter-panel pass, exactly as the reference does.
fn prepare_page(page: &mut Page, options: &Options, is_first_page: bool) {
    page.background = fill::fill_check(&page.image);
    let background = page.background;

    if is_first_page && is_colour_page(&page.image, options) {
        return;
    }

    let power = f64::from(options.cropping_power);
    let minimum = f64::from(options.cropping_minimum);
    if !options.webtoon {
        match options.cropping {
            2 => crop::crop_page_number(
                &mut page.image,
                power,
                minimum,
                options.preserve_margin,
                background,
            ),
            1 => crop::crop_margin(
                &mut page.image,
                power,
                minimum,
                options.preserve_margin,
                background,
            ),
            _ => {}
        }
    }

    if options.inter_panel_crop > 0 {
        let direction = if options.inter_panel_crop == 1 {
            interpanel::Direction::Horizontal
        } else {
            interpanel::Direction::Both
        };
        page.image = interpanel::crop_empty_inter_panel(
            &page.image,
            direction,
            INTER_PANEL_KEEP,
            background,
        );
    }
}

/// Whether a page is detected as colour, for the first-page crop exemption.
fn is_colour_page(image: &DynamicImage, options: &Options) -> bool {
    color::color_check(&image.to_rgb8(), page::is_grayscale_image(image), options)
}

/// KCC's `detectSuboptimalProcessing`: warnings about a source that is likely to
/// convert poorly, emitted before the pages are renamed and processed.
///
/// Two conditions are checked (see docs/porting.md):
///
/// - any source page name already carries KCC's `-kcc` order suffix, so it is
///   probably KCC output and a second conversion will lose quality;
/// - more than 25% of pages are smaller than the target device resolution, and
///   neither `--upscale`/`--stretch` nor a Scribe (`KS*`) profile is in effect.
///
/// The reference's third behaviour — rejecting zero-byte or undecodable images —
/// is already enforced while the source is decoded (see docs/porting.md).
pub fn detect_suboptimal_processing(tree: &ComicTree, options: &Options) -> Vec<String> {
    let mut warnings = Vec::new();

    let mut image_number: u64 = 0;
    let mut image_smaller: u64 = 0;
    let mut already_processed = false;
    let mut any_page = false;

    for chapter in &tree.chapters {
        for page in &chapter.pages {
            any_page = true;
            if !already_processed && file_stem(&page.rel_path).contains("-kcc") {
                already_processed = true;
            }
            let (width, height) = page.image.dimensions();
            image_number += 1;
            if options.profile_data.width > width && options.profile_data.height > height {
                image_smaller += 1;
            }
        }
    }

    if !any_page {
        return warnings;
    }

    if already_processed {
        warnings.push(
            "WARNING: Source files are probably created by KCC. \
             The second conversion will decrease quality."
                .to_string(),
        );
    }

    // `imageSmaller > imageNumber * 0.25` compares floats in the reference; the
    // integer form below is exact and avoids the exact-multiple edge case.
    if image_smaller * 4 > image_number
        && !options.upscale
        && !options.stretch
        && !options.profile.is_scribe()
    {
        warnings.push(
            "WARNING: More than 25% of images are smaller than target device resolution. \
             Consider enabling stretching or upscaling to improve readability."
                .to_string(),
        );
    }

    warnings
}

/// A page file name without its final extension.
fn file_stem(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(index) if index > 0 => &base[..index],
        _ => base,
    }
}
