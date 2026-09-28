//! PDF input: embedded-image extraction and rasterisation.
//!
//! KCC's `getWorkFolder` PDF branch offers two extraction strategies:
//!
//! - `--legacy-extract` runs the classic raw-JPEG stream scan (`pdfjpgextract`),
//!   which recovers the embedded JPEGs verbatim from a PDF whose streams carry
//!   them unfiltered.
//! - otherwise it decides per page between extracting the page's single embedded
//!   image and rendering the whole page (with MuPDF) at the device resolution.
//!
//! This port reproduces both, but uses the pure-Rust `pdfboss` rasterizer instead
//! of MuPDF (see docs/porting.md). The default path extracts a page's image
//! when it draws exactly one, and rasterises the page otherwise; the reference's
//! additional text/CCITT render triggers are not reproduced (we cannot cheaply
//! detect page text, and pdfboss decodes CCITT where MuPDF could not), so this is
//! a documented deviation.

use anyhow::{bail, Context, Result};
use pdfboss_core::Document;
use pdfboss_render::{extract_page_images, render_page, Pixmap};
use std::path::Path;

use crate::ebook::model::{ComicTree, MediaType};
use crate::ebook::options::{Cropping, Options};
use crate::units::Size;

use super::archive::{build_tree, LoadedPage, RootStrip};

/// Load a PDF source into a [`ComicTree`].
pub fn load(source: &Path, options: &Options) -> Result<ComicTree> {
    if options.processing.source.legacy_extract {
        return legacy_extract(source);
    }
    rasterize(source, options)
}

/// The default path: per page, extract the single embedded image or render it.
fn rasterize(source: &Path, options: &Options) -> Result<ComicTree> {
    let doc = Document::open(source)
        .with_context(|| format!("Failed to open PDF file '{}'", source.display()))?;
    let (target_width, target_height) = render_target(options);

    let mut pages = Vec::new();
    for index in 0..doc.page_count() {
        let page = doc
            .page(index)
            .with_context(|| format!("Failed to read PDF page {index}"))?;

        // KCC extracts the page's image only when there is exactly one; anything
        // else (vector art, multiple images) is rendered instead.
        let images = extract_page_images(&doc, &page)
            .with_context(|| format!("Failed to extract images from PDF page {index}"))?;
        let mut extracted = images.into_iter();
        let pixmap = match (extracted.next(), extracted.next()) {
            (Some(pixmap), None) => pixmap,
            _ => {
                let (page_width, page_height) = page.size();
                let zoom = render_zoom(
                    options.processing.source.pdf_width,
                    target_width,
                    target_height,
                    page_width,
                    page_height,
                );
                render_page(&doc, &page, zoom)
                    .with_context(|| format!("Failed to render PDF page {index}"))?
            }
        };

        // Render always names the payload `p-<i>.png`; extraction follows the
        // same scheme for a uniform, naturally ordered flat chapter.
        pages.push(pixmap_page(format!("p-{index}.png"), pixmap)?);
    }

    if pages.is_empty() {
        bail!("Failed to extract images from PDF file.");
    }
    Ok(build_tree(pages, None, RootStrip::Keep))
}

/// The device target size KCC renders PDF pages against, widened to leave room
/// for the margin/page-number crop (`getWorkFolder`'s `cropping` multipliers).
/// The float device target used by the rasteriser, enlarged by the crop mode.
///
/// Deliberately `f32`: PDF rendering scales by a zoom factor, so this is float
/// render geometry rather than an integer [`Size`]; it is intentionally outside the
/// `Size`/`Pixels` toolkit (which names the `u32` page dimensions).
fn render_target(options: &Options) -> (f32, f32) {
    let width = options.device.data.width as f32;
    let height = options.device.data.height as f32;
    match options.processing.cropping {
        Cropping::Margins => (width * 1.2, height * 1.2),
        Cropping::PageNumbers => (width * 1.25, height * 1.25),
        Cropping::Off => (width, height),
    }
}

/// KCC's zoom choice: fit the page height, unless `--pdf-width` is set and the
/// page is portrait, in which case fit the page width.
fn render_zoom(
    pdf_width: bool,
    target_width: f32,
    target_height: f32,
    page_width: f32,
    page_height: f32,
) -> f32 {
    if (!pdf_width || page_width > page_height) && page_height > 0.0 {
        target_height / page_height
    } else if page_width > 0.0 {
        target_width / page_width
    } else {
        1.0
    }
}

/// Turn a [`Pixmap`] into a page, retaining only its PNG encoding and its
/// dimensions so `--no-processing` emits it untouched and ingest does not pin the
/// decoded pixels (which the lazy pipeline re-decodes on demand).
fn pixmap_page(name: String, pixmap: Pixmap) -> Result<LoadedPage> {
    let dimensions = Size::new(pixmap.width, pixmap.height);
    let raw = pixmap
        .encode_png()
        .map_err(|error| anyhow::anyhow!("Failed to encode PDF page: {error}"))?;
    Ok(LoadedPage {
        name,
        media_type: Some(MediaType::Png),
        raw,
        dimensions,
    })
}

/// The `pdfjpgextract` fallback: recover the embedded JPEG streams verbatim.
fn legacy_extract(source: &Path) -> Result<ComicTree> {
    /// Skip stray images a few pixels in size in some PDFs (KCC's threshold).
    const STRAY_IMAGE_LENGTH_THRESHOLD: usize = 300;
    const START_MARK: &[u8] = b"\xff\xd8";
    const END_MARK: &[u8] = b"\xff\xd9";

    let pdf = std::fs::read(source)
        .with_context(|| format!("Failed to open PDF file '{}'", source.display()))?;

    let mut pages = Vec::new();
    let mut cursor = 0usize;
    while let Some(stream) = find(&pdf, b"stream", cursor) {
        let Some(start) = find_in_range(&pdf, START_MARK, stream, stream + 20) else {
            cursor = stream + 20;
            continue;
        };
        let Some(stream_end) = find(&pdf, b"endstream", start) else {
            bail!("Didn't find end of stream!");
        };
        let Some(end_mark) = find(&pdf, END_MARK, stream_end.saturating_sub(20)) else {
            bail!("Didn't find end of JPG!");
        };
        let end = end_mark + END_MARK.len();
        cursor = end;

        if end - start < STRAY_IMAGE_LENGTH_THRESHOLD {
            continue;
        }
        let name = format!("jpg{}.jpg", pages.len());
        pages.push(super::archive::load_page(&name, &pdf[start..end])?);
    }

    if pages.is_empty() {
        bail!("Failed to extract images from PDF file.");
    }
    Ok(build_tree(pages, None, RootStrip::Keep))
}

/// Index of the first occurrence of `needle` in `haystack` at or after `from`.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() || needle.is_empty() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// Index of the first occurrence of `needle` in `haystack[from..end]`.
///
/// `end` is clamped to the haystack length, mirroring Python's `bytes.find`.
fn find_in_range(haystack: &[u8], needle: &[u8], from: usize, end: usize) -> Option<usize> {
    let end = end.min(haystack.len());
    if from >= end || needle.is_empty() {
        return None;
    }
    let slice = &haystack[from..end];
    if needle.len() > slice.len() {
        return None;
    }
    slice
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_fits_height_unless_pdf_width_portrait() {
        // Default (no --pdf-width): always fit the page height.
        assert_eq!(
            render_zoom(false, 1000.0, 1500.0, 600.0, 800.0),
            1500.0 / 800.0
        );
        // --pdf-width portrait: fit width.
        assert_eq!(
            render_zoom(true, 1000.0, 1500.0, 500.0, 800.0),
            1000.0 / 500.0
        );
        // --pdf-width landscape: still fit height.
        assert_eq!(
            render_zoom(true, 1000.0, 1500.0, 900.0, 600.0),
            1500.0 / 600.0
        );
    }

    #[test]
    fn find_scans_from_the_offset() {
        let data = b"aaastreamXYZ";
        assert_eq!(find(data, b"stream", 0), Some(3));
        assert_eq!(find(data, b"stream", 4), None);
        assert_eq!(find_in_range(data, b"XYZ", 3, 13), Some(9));
        assert_eq!(find_in_range(data, b"XYZ", 3, 5), None);
    }
}
