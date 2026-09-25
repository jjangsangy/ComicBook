//! Cover selection and processing, including smart cover crop and tome labels
//! (Phase 6).
//!
//! Phase 5 only needs a usable cover: [`make_cover`] selects the cover image (the
//! first page, or a sibling `Covers/` override) and encodes it as the `cover.jpg`
//! the EPUB layout expects. KCC's `Cover` pipeline — autocontrast, optional
//! grayscale, smart cover crop, fit-to-profile and the tome `N/M` label — replaces
//! this in Phase 6 (AGENTS.md §11.8, §15).

use anyhow::{Context, Result};
use image::DynamicImage;
use std::path::Path;

use crate::ebook::model::{ComicTree, EncodedPage, MediaType, OrderClass, PageFlags};
use crate::ebook::options::Options;
use crate::ebook::processing::page;

/// Select and encode the book's cover image, if any.
///
/// The cover is the sibling `Covers/` override when one was selected
/// ([`crate::ebook::naming::select_cover`]), otherwise the first page of the first
/// non-empty chapter — KCC's `cover_path`.
pub fn make_cover(
    tree: &ComicTree,
    cover_override: Option<&Path>,
    options: &Options,
) -> Result<Option<EncodedPage>> {
    let image = match cover_override {
        Some(path) => {
            image::open(path).with_context(|| format!("Failed to read cover {}", path.display()))?
        }
        None => match first_page_image(tree) {
            Some(image) => image,
            None => return Ok(None),
        },
    };

    let width = image.width();
    let height = image.height();
    // The OPF advertises `cover.jpg` as `image/jpeg`, so the cover is flattened to
    // RGB and encoded as JPEG whatever the source page's format was.
    let rgb = DynamicImage::ImageRgb8(image.to_rgb8());
    let bytes = page::encode_jpeg(&rgb, options.jpeg_quality)?;

    Ok(Some(EncodedPage {
        name: "cover.jpg".to_string(),
        order_class: OrderClass::Normal,
        media_type: MediaType::Jpeg,
        bytes,
        width,
        height,
        flags: PageFlags::default(),
    }))
}

/// The first page of the first non-empty chapter.
fn first_page_image(tree: &ComicTree) -> Option<DynamicImage> {
    tree.chapters
        .iter()
        .find_map(|chapter| chapter.pages.first())
        .map(|page| page.image.clone())
}
