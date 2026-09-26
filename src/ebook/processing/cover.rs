//! Cover processing — KCC's `Cover` (see docs/processing.md).
//!
//! [`process`] reproduces the reference's `Cover.process`: the selected image is
//! flattened to RGB, autocontrasted with preserved tone, optionally converted to
//! grayscale, optionally cropped out of a wide spread (`--smart-cover-crop`), then
//! fitted to the device (or `--cover-fill`-cropped to fill it) and encoded as the
//! `cover.jpg` the EPUB layout expects.
//!
//! The selection itself — a sibling `Covers/` override, else the first page — is
//! [`crate::ebook::naming::select_cover`].
//!
//! [`labelled`] adds the tome `N/M` label KCC draws on a split book's cover
//! (`Cover.save_to_folder`). The glyphs come from the MIT `font8x8` bitmap font
//! rather than Pillow's built-in face, so the label's *position, size and colours*
//! match the reference while the exact glyph shapes do not (see docs/porting.md).

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
        page::fit(&image, size, Method::Lanczos)?
    } else {
        page::thumbnail(&image, size, Method::Lanczos)?
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

/// KCC's `Cover.save_to_folder` tome label: re-encode `cover` with the `N/M` tome
/// number drawn near the bottom (see docs/processing.md and docs/porting.md).
///
/// KCC increments its `tomeid` before saving as soon as there is more than one
/// tome, so *every* tome of a split book is labelled (including the first). A
/// single-tome book has no label and keeps its cover bytes untouched.
pub fn labelled(
    cover: &EncodedPage,
    tome: usize,
    total: usize,
    quality: u8,
) -> Result<EncodedPage> {
    // KCC's `tomeid == 0` branch saves the cover unlabelled; its caller only
    // increments `tomeid` once a book splits into more than one tome.
    if total <= 1 || tome == 0 {
        return Ok(cover.clone());
    }
    let decoded = image::load_from_memory(&cover.bytes)
        .context("Failed to decode the cover for the tome label")?;
    // `Cover.process` leaves the cover as 8-bit grey (or RGB under `--force-color`),
    // so keep whichever of the two the JPEG carried.
    let mut image = match decoded {
        DynamicImage::ImageLuma8(_) => decoded,
        other => DynamicImage::ImageRgb8(other.to_rgb8()),
    };
    draw_label(&mut image, &format!("{tome}/{total}"));
    let (width, height) = (image.width(), image.height());
    let bytes = page::encode_jpeg(&image, quality)?;

    Ok(EncodedPage {
        name: cover.name.clone(),
        order_class: cover.order_class,
        media_type: MediaType::Jpeg,
        bytes,
        width,
        height,
        flags: cover.flags,
    })
}

/// Pillow's `stroke_width` for the tome label (`Cover.save_to_folder`).
const LABEL_STROKE: i64 = 25;

/// Draw the label the way `ImageDraw.text(..., anchor='ms')` places it: centred
/// horizontally, its baseline at `h * 0.85`, sized `h // 7`, in white with a black
/// outline.
fn draw_label(image: &mut DynamicImage, text: &str) {
    let height = image.height();
    let font_size = height / 7;
    let scale = (font_size / 8).max(1);
    let glyph = 8 * scale;
    let text_width = glyph * text.chars().count() as u32;
    let x = (i64::from(image.width()) - i64::from(text_width)) / 2;
    let baseline = (f64::from(height) * 0.85) as i64;
    let y = baseline - i64::from(glyph);

    // A thick outline is approximated by stamping the glyphs in black around the
    // white text, matching the reference's `stroke_fill=0, stroke_width=25`.
    for dx in [-LABEL_STROKE, 0, LABEL_STROKE] {
        for dy in [-LABEL_STROKE, 0, LABEL_STROKE] {
            if dx == 0 && dy == 0 {
                continue;
            }
            draw_text(image, text, scale, x + dx, y + dy, [0, 0, 0]);
        }
    }
    draw_text(image, text, scale, x, y, [255, 255, 255]);
}

/// Stamp `text` at `(x, y)`, each 8x8 glyph scaled by `scale`.
fn draw_text(image: &mut DynamicImage, text: &str, scale: u32, x: i64, y: i64, color: [u8; 3]) {
    use font8x8::{UnicodeFonts, BASIC_FONTS};

    let scale = i64::from(scale);
    for (index, ch) in text.chars().enumerate() {
        let Some(rows) = BASIC_FONTS.get(ch) else {
            continue;
        };
        let gx = x + index as i64 * 8 * scale;
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..8u32 {
                if bits & (1 << col) == 0 {
                    continue;
                }
                fill_block(
                    image,
                    gx + i64::from(col) * scale,
                    y + row as i64 * scale,
                    scale as u32,
                    color,
                );
            }
        }
    }
}

/// Fill a `size`x`size` square, clipped to the image bounds.
fn fill_block(image: &mut DynamicImage, x: i64, y: i64, size: u32, color: [u8; 3]) {
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));
    let size = i64::from(size);
    for py in y.max(0)..(y + size).min(height) {
        for px in x.max(0)..(x + size).min(width) {
            put_pixel(image, px as u32, py as u32, color);
        }
    }
}

/// Write one pixel into a grey or RGB cover.
fn put_pixel(image: &mut DynamicImage, x: u32, y: u32, color: [u8; 3]) {
    match image {
        DynamicImage::ImageLuma8(buffer) => buffer.put_pixel(x, y, image::Luma([color[0]])),
        DynamicImage::ImageRgb8(buffer) => buffer.put_pixel(x, y, image::Rgb(color)),
        _ => {}
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
    use anyhow::bail;
    use clap::Parser;
    use image::{Rgb, RgbImage};

    /// Resolve options from a `comic-book ebook` command line.
    fn options(args: &[&str]) -> Result<Options> {
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = crate::cli::Cli::try_parse_from(full)?;
        match cli.command {
            crate::cli::Commands::Ebook(args) => Options::resolve(&args),
            _ => bail!("expected the ebook subcommand"),
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
    fn cover_is_capped_to_the_profile_and_named_cover_jpg() -> Result<()> {
        let tree = tree_with_page(2000, 3000, [255, 255, 255]);
        let cover = process(&tree, None, &options(&["-f", "epub", "-p", "KoE"])?)?
            .context("a cover is produced")?;
        assert_eq!(cover.page.name, "cover.jpg");
        assert_eq!(cover.page.media_type, MediaType::Jpeg);
        // The Kobo Elipsa is 1404x1872, so the 2000x3000 page is thumbnailed to fit.
        assert!(cover.page.width <= 1404 && cover.page.height <= 1872);
        assert!(!cover.smart_cropped);
        Ok(())
    }

    #[test]
    fn smart_cover_crop_takes_the_right_half_of_a_wide_spread() -> Result<()> {
        let tree = tree_with_page(2000, 1000, [255, 255, 255]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--smart-cover-crop"])?,
        )?
        .context("a cover is produced")?;
        assert!(cover.smart_cropped);
        // A 2:1 spread (> 1.83 ratio) keeps the right 42.5%–81% band, so the
        // cropped source is 770x1000 before the 1404x1872 thumbnail.
        assert!(cover.page.width < cover.page.height);
        Ok(())
    }

    #[test]
    fn smart_cover_crop_is_a_no_op_on_a_page_shaped_cover() -> Result<()> {
        let tree = tree_with_page(600, 900, [10, 10, 10]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--smart-cover-crop"])?,
        )?
        .context("a cover is produced")?;
        assert!(!cover.smart_cropped);
        Ok(())
    }

    #[test]
    fn cover_fill_crops_to_the_exact_profile_size() -> Result<()> {
        // A portrait page against a landscape profile: thumbnail would leave the
        // height short, `--cover-fill` fills it exactly.
        let tree = tree_with_page(600, 900, [128, 128, 128]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--cover-fill"])?,
        )?
        .context("a cover is produced")?;
        assert_eq!((cover.page.width, cover.page.height), (1404, 1872));
        Ok(())
    }

    #[test]
    fn force_color_keeps_the_cover_colourful() -> Result<()> {
        let tree = tree_with_page(100, 150, [200, 30, 30]);
        let cover = process(
            &tree,
            None,
            &options(&["-f", "epub", "-p", "KoE", "--force-color"])?,
        )?
        .context("a cover is produced")?;
        let decoded = image::load_from_memory(&cover.page.bytes)?;
        assert!(matches!(decoded, DynamicImage::ImageRgb8(_)));
        Ok(())
    }

    #[test]
    fn tome_label_changes_the_cover_bytes_and_stays_grey() -> Result<()> {
        // A dark cover so the white label is actually visible in the pixels.
        let tree = tree_with_page(600, 900, [20, 20, 20]);
        let cover = process(&tree, None, &options(&["-f", "epub", "-p", "KoE"])?)?
            .context("a cover is produced")?
            .page;

        let plain = labelled(&cover, 0, 1, 85)?;
        assert_eq!(plain.bytes, cover.bytes, "a single tome is left untouched");

        let first = labelled(&cover, 1, 3, 85)?;
        let second = labelled(&cover, 2, 3, 85)?;
        assert_ne!(first.bytes, cover.bytes, "the label re-encodes the cover");
        assert_ne!(first.bytes, second.bytes, "each tome gets its own number");
        assert!(matches!(
            image::load_from_memory(&first.bytes)?,
            DynamicImage::ImageLuma8(_)
        ));
        // A bright label pixel appears where the cover was uniformly dark.
        let decoded = image::load_from_memory(&first.bytes)?.to_luma8();
        assert!(decoded.pixels().any(|pixel| pixel[0] == 255));
        Ok(())
    }
}
