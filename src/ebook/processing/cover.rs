//! Cover processing — KCC's `Cover` (AGENTS.md §11.8).
//!
//! [`process`] reproduces the reference's `Cover.process`: the selected image is
//! flattened to RGB, autocontrasted with preserved tone, optionally converted to
//! grayscale, optionally cropped out of a wide spread (`--smart-cover-crop`), then
//! fitted to the device (or `--cover-fill`-cropped to fill it) and encoded as the
//! `cover.jpg` the EPUB layout expects.
//!
//! The selection itself — a sibling `Covers/` override, else the first page — is
//! Phase 4's [`crate::ebook::naming::select_cover`].
//!
//! The tome `N/M` label KCC draws on split covers is deferred to Phase 9 with
//! chunking, the only path that produces more than one tome (AGENTS.md §13.11).

use anyhow::{Context, Result};
use image::DynamicImage;
use std::path::Path;

use crate::ebook::model::{ComicTree, EncodedPage, MediaType, OrderClass, PageFlags};
use crate::ebook::options::Options;
use crate::ebook::processing::color::to_luma601;
use crate::ebook::processing::crop;
use crate::ebook::processing::page::{self, Method};

/// The processed cover plus whether `--smart-cover-crop` actually cropped it
/// (KCC's `Cover.smartcover`, which CBZ/PDF output tests before writing a cover).
pub struct Cover {
    pub page: EncodedPage,
    pub smart_cropped: bool,
}

/// Process the book's cover image, if any.
///
/// The cover is the sibling `Covers/` override when one was selected
/// ([`crate::ebook::naming::select_cover`]), otherwise the first page of the first
/// non-empty chapter — KCC's `cover_path`.
pub fn process(
    tree: &ComicTree,
    cover_override: Option<&Path>,
    options: &Options,
) -> Result<Option<Cover>> {
    let source = match cover_override {
        Some(path) => {
            image::open(path).with_context(|| format!("Failed to read cover {}", path.display()))?
        }
        None => match first_page_image(tree) {
            Some(image) => image,
            None => return Ok(None),
        },
    };

    // `Cover.process`: RGB → autocontrast → optional grayscale → smart crop → fit.
    let mut image = DynamicImage::ImageRgb8(source.to_rgb8());
    page::autocontrast_preserve_tone(&mut image);
    if !options.force_color {
        image = DynamicImage::ImageLuma8(to_luma601(&image));
    }

    let smart_cropped = if options.smart_cover_crop {
        crop_main_cover(&mut image, options.right_to_left)
    } else {
        false
    };

    let size = cover_size(options);
    let image = if options.cover_fill && !options.kindle_scribe_azw3 {
        page::fit(&image, size, Method::Lanczos)
    } else {
        page::thumbnail(&image, size, Method::Lanczos)
    };

    // The OPF advertises the cover as `image/jpeg`, so it is always JPEG whatever
    // the source page's format was.
    let (width, height) = (image.width(), image.height());
    let bytes = page::encode_jpeg(&image, options.jpeg_quality)?;

    Ok(Some(Cover {
        page: EncodedPage {
            name: "cover.jpg".to_string(),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            bytes,
            width,
            height,
            flags: PageFlags::default(),
        },
        smart_cropped,
    }))
}

/// The cover's target size: the profile, with both dimensions capped at 1920 for
/// Kindle Scribe KF8 output (`Cover.process`).
fn cover_size(options: &Options) -> (u32, u32) {
    let (width, height) = (options.profile_data.width, options.profile_data.height);
    if options.kindle_scribe_azw3 {
        (
            width.min(page::SCRIBE_MAX_DIMENSION),
            height.min(page::SCRIBE_MAX_DIMENSION),
        )
    } else {
        (width, height)
    }
}

/// KCC's `Cover.crop_main_cover`: pull the main cover out of a double-page spread
/// by cropping one side, based on how wide the image is. Returns whether a crop
/// happened (`smartcover`).
///
/// The ratios and side selection are load-bearing (a wrong box cuts into the
/// art), so they are reproduced literally. `crop_rounded` rounds each edge
/// half-to-even, as Pillow's `Image.crop` does.
fn crop_main_cover(image: &mut DynamicImage, right_to_left: bool) -> bool {
    let (width, height) = (image.width(), image.height());
    let (w, h) = (f64::from(width), f64::from(height));
    let ratio = w / h;

    let (left, upper, right, lower) = if ratio > 2.0 {
        if right_to_left {
            (w / 6.0, 0.0, w / 2.0 - w * 0.02, h)
        } else {
            (w / 2.0 + w * 0.02, 0.0, 5.0 / 6.0 * w, h)
        }
    } else if ratio > 1.83 {
        if right_to_left {
            (w * 0.19, 0.0, w * 0.575, h)
        } else {
            (w * 0.425, 0.0, 0.81 * w, h)
        }
    } else if ratio > 1.7 {
        if right_to_left {
            (w * 0.2, 0.0, w * 0.583, h)
        } else {
            (w * 0.417, 0.0, 0.8 * w, h)
        }
    } else if ratio > 1.34 {
        if right_to_left {
            (0.0, 0.0, w / 2.0 - w * 0.03, h)
        } else {
            (w / 2.0 + w * 0.03, 0.0, w, h)
        }
    } else if ratio > 1.0 {
        if right_to_left {
            (w * 0.36, 0.0, w, h)
        } else {
            (0.0, 0.0, 0.64 * w, h)
        }
    } else {
        return false;
    };

    *image = crop::crop_rounded(image, left, upper, right, lower);
    true
}

/// The first page of the first non-empty chapter.
fn first_page_image(tree: &ComicTree) -> Option<DynamicImage> {
    tree.chapters
        .iter()
        .find_map(|chapter| chapter.pages.first())
        .map(|page| page.image.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use image::{Rgb, RgbImage};

    /// Resolve options from a `comic-book ebook` command line.
    fn options(args: &[&str]) -> Options {
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = crate::cli::Cli::try_parse_from(full).expect("CLI parses");
        match cli.command {
            crate::cli::Commands::Ebook(args) => Options::resolve(&args).expect("resolves"),
            _ => unreachable!(),
        }
    }

    /// A tree whose only page is a solid-colour image of the given size.
    fn tree_with_page(width: u32, height: u32, color: [u8; 3]) -> ComicTree {
        use crate::ebook::model::{Chapter, Page};

        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb(color)));
        let page = Page {
            source_name: "kcc-0001.png".to_string(),
            rel_path: "kcc-0001.png".to_string(),
            image,
            background: crate::ebook::model::Background::White,
            flags: PageFlags::default(),
            raw: None,
            source_media_type: Some(MediaType::Png),
        };
        ComicTree {
            chapters: vec![Chapter {
                name: String::new(),
                pages: vec![page],
            }],
            cover: None,
            comicinfo: None,
        }
    }

    #[test]
    fn cover_is_capped_to_the_profile_and_named_cover_jpg() {
        let tree = tree_with_page(2000, 3000, [255, 255, 255]);
        let cover = process(&tree, None, &options(&["-f", "epub", "-p", "KoE"]))
            .unwrap()
            .unwrap();
        assert_eq!(cover.page.name, "cover.jpg");
        assert_eq!(cover.page.media_type, MediaType::Jpeg);
        // The Kobo Elipsa is 1404x1872, so the 2000x3000 page is thumbnailed to fit.
        assert!(cover.page.width <= 1404 && cover.page.height <= 1872);
        assert!(!cover.smart_cropped);
    }

    #[test]
    fn smart_cover_crop_takes_the_right_half_of_a_wide_spread() {
        let tree = tree_with_page(2000, 1000, [255, 255, 255]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--smart-cover-crop"]),
        )
        .unwrap()
        .unwrap();
        assert!(cover.smart_cropped);
        // A 2:1 spread (> 1.83 ratio) keeps the right 42.5%–81% band, so the
        // cropped source is 770x1000 before the 1404x1872 thumbnail.
        assert!(cover.page.width < cover.page.height);
    }

    #[test]
    fn smart_cover_crop_is_a_no_op_on_a_page_shaped_cover() {
        let tree = tree_with_page(600, 900, [10, 10, 10]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--smart-cover-crop"]),
        )
        .unwrap()
        .unwrap();
        assert!(!cover.smart_cropped);
    }

    #[test]
    fn cover_fill_crops_to_the_exact_profile_size() {
        // A portrait page against a landscape profile: thumbnail would leave the
        // height short, `--cover-fill` fills it exactly.
        let tree = tree_with_page(600, 900, [128, 128, 128]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--cover-fill"]),
        )
        .unwrap()
        .unwrap();
        assert_eq!((cover.page.width, cover.page.height), (1404, 1872));
    }

    #[test]
    fn force_color_keeps_the_cover_colourful() {
        let tree = tree_with_page(100, 150, [200, 30, 30]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--force-color"]),
        )
        .unwrap()
        .unwrap();
        let decoded = image::load_from_memory(&cover.page.bytes).unwrap();
        assert!(matches!(decoded, DynamicImage::ImageRgb8(_)));
    }
}
