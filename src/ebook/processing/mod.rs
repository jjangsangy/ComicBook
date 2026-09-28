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
pub mod kernels;
pub mod page;
pub mod rainbow;
pub mod webtoon;

pub use page::process_page;

use anyhow::{Context, Result};
use image::DynamicImage;
use rayon::prelude::*;

use crate::ebook::model::{ChapterName, ComicTree, EncodedPage, Page};
use crate::ebook::options::{Cropping, InterPanelCrop, Options};
use crate::ebook::progress;
use crate::units::{Fraction, Size};

/// Fraction of each inter-panel gutter KCC retains after cropping.
const INTER_PANEL_KEEP: Fraction = Fraction::new(0.04);

/// The encoded pages of one chapter, in reading order.
#[derive(Debug)]
pub struct ProcessedChapter {
    /// The chapter's source directory path (before slugification).
    pub name: ChapterName,
    pub pages: Vec<EncodedPage>,
}

/// Everything the processing stage produces for one book.
#[derive(Debug)]
pub struct ProcessedBook {
    pub chapters: Vec<ProcessedChapter>,
    /// The processed cover (its [`cover::Cover::smart_cropped`] flag gates the
    /// CBZ/PDF cover write), set by [`crate::ebook::convert_source`].
    pub cover: Option<cover::Cover>,
    /// Total encoded pages, which may exceed the source page count when spreads
    /// were bisected.
    pub page_count: usize,
}

/// Process every page of a tree into encoded images.
///
/// This is the single-file entry point; it reports through a standalone
/// [`progress::Reporter`]. Batch callers use [`process_tree_with`] so the per-file
/// bar nests under the overall bar.
pub fn process_tree(tree: &mut ComicTree, options: &Options) -> Result<ProcessedBook> {
    process_tree_with(tree, options, &progress::Reporter::standalone())
}

/// [`process_tree`] with progress reported through `reporter`.
///
/// The tree is taken mutably because background detection is cached on each page
/// for the later cropping phases, mirroring KCC's `ComicPageParser`.
pub fn process_tree_with(
    tree: &mut ComicTree,
    options: &Options,
    reporter: &progress::Reporter,
) -> Result<ProcessedBook> {
    let size = options.profile_size();
    let bar = reporter.child(tree.page_count() as u64, "Processing images");

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
                let is_first_page = first_chapter == Some(chapter_index) && page_index == 0;
                let result = if options.processing.no_processing {
                    // Passthrough needs only the source bytes and the header
                    // dimensions, so the page is never decoded; its bytes are moved
                    // into the output rather than copied.
                    page::passthrough_in_place(page, options)
                } else {
                    process_page_owned(page, options, size, is_first_page)
                };
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

/// Prepare one page (background/crop) and encode it, releasing the decoded
/// pixels before returning so peak memory stays bounded to the in-flight batch.
fn process_page_owned(
    page: &mut Page,
    options: &Options,
    size: Size,
    is_first_page: bool,
) -> Result<Vec<crate::ebook::model::EncodedPage>> {
    prepare_page(page, options, is_first_page)?;
    let image = page
        .take_image()
        .context("page has no decoded image after prepare")?;
    page::process_decoded(page, image, options, size)
}

/// Detect the background and crop a page before it is split and encoded.
///
/// This mirrors KCC's `ComicPageParser.__init__`: the fill is detected first, the
/// page-number/margin crop runs (from the *detected* background, not the border
/// override), then the inter-panel crop. A colour first page (the cover) is left
/// untouched, and webtoon mode skips the margin/page-number crops but still runs
/// the inter-panel pass, exactly as the reference does.
fn prepare_page(page: &mut Page, options: &Options, is_first_page: bool) -> Result<()> {
    // Detect the fill from the decoded pixels; `ensure_decoded` is the state
    // transition, so no defensive "is there an image" guard is needed.
    let background = fill::fill_check(page.ensure_decoded()?);
    page.background = background;

    let image = page.ensure_decoded()?;
    if is_first_page && is_colour_page(image, options) {
        return Ok(());
    }

    let power = f64::from(options.processing.cropping_power);
    let minimum = options.processing.cropping_minimum;
    if !options.main.webtoon {
        match options.processing.cropping {
            Cropping::PageNumbers => crop::crop_page_number(
                image,
                power,
                minimum,
                options.processing.preserve_margin,
                background,
            ),
            Cropping::Margins => crop::crop_margin(
                image,
                power,
                minimum,
                options.processing.preserve_margin,
                background,
            ),
            Cropping::Off => {}
        }
    }

    let direction = match options.processing.inter_panel_crop {
        InterPanelCrop::Off => None,
        InterPanelCrop::Horizontal => Some(interpanel::Direction::Horizontal),
        InterPanelCrop::Both => Some(interpanel::Direction::Both),
    };
    if let Some(direction) = direction {
        let cropped =
            interpanel::crop_empty_inter_panel(image, direction, INTER_PANEL_KEEP, background);
        *image = cropped;
    }

    Ok(())
}

/// Whether a page is detected as colour, for the first-page crop exemption.
fn is_colour_page(image: &DynamicImage, options: &Options) -> bool {
    // A grayscale source is never colour (KCC's `colorCheck` shortcut), so skip
    // the RGB conversion entirely for the common manga scan.
    if page::is_grayscale_image(image) {
        return false;
    }
    match image.as_rgb8() {
        Some(rgb) => color::color_check(rgb, options).is_color(),
        None => color::color_check(&image.to_rgb8(), options).is_color(),
    }
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
            if !already_processed && file_stem(page.rel_path.as_str()).contains("-kcc") {
                already_processed = true;
            }
            let size = page.dimensions();
            image_number += 1;
            if options.device.data.width > size.width && options.device.data.height > size.height {
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
        && !options.processing.sizing.upscale
        && !options.processing.sizing.stretch
        && !options.device.profile.is_scribe()
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
