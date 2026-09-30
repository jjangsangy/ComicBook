use anyhow::{Context, Result};
use fast_image_resize::images::Image as FastImage;
use fast_image_resize::images::ImageRef;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, GenericImageView, RgbImage};
use std::fs;
use std::path::Path;

use crate::units::{Pixels, Size};

pub const IMG_EXTENSIONS: &[&str] = &[
    "jpeg", "jpg", "png", "tiff", "tif", "bmp", "webp", "gif", "pgm",
];

pub fn is_image_extension(ext: &str) -> bool {
    let lower = ext.trim_start_matches('.').to_ascii_lowercase();
    IMG_EXTENSIONS.contains(&lower.as_str())
}

/// The extension of a path's final component (no leading dot), if it has a non-empty
/// stem before it (docs/refactor.md E16).
///
/// A leading dot is not an extension (`.png` has none). This is `std::path`'s own
/// `Path::extension` (the path is a host filesystem path, not an archive-relative
/// name, so `std::path` is the right model here); a non-UTF-8 name yields `None`.
pub fn path_extension(path: &Path) -> Option<&str> {
    path.extension()?.to_str()
}

pub fn is_image_file<P: AsRef<Path>>(path: P) -> bool {
    path_extension(path.as_ref()).is_some_and(is_image_extension)
}

/// Resize an image using high-quality SIMD-accelerated Lanczos3 convolution.
///
/// The internal `fast_image_resize` steps only fail on a buffer/dimension mismatch
/// that the dimensions taken from the source make impossible; that state is reported
/// as an error rather than silently returning a full copy of the original image
/// (docs/refactor.md E15).
pub fn resize_lanczos3(img: &DynamicImage, size: Size) -> Result<DynamicImage> {
    // Borrow the samples when the source is already RGB8 instead of cloning them
    // into an owned `RgbImage`; only other pixel types pay for the conversion.
    let owned;
    let rgb = match img.as_rgb8() {
        Some(rgb) => rgb,
        None => {
            owned = img.to_rgb8();
            &owned
        }
    };
    let (w, h) = (rgb.width(), rgb.height());
    let src_image = ImageRef::new(w, h, rgb.as_raw(), PixelType::U8x3)
        .context("Lanczos3 resize source buffer does not match its dimensions")?;
    let mut dst_image = FastImage::new(size.width, size.height, PixelType::U8x3);

    let mut resizer = Resizer::new();
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    resizer
        .resize(&src_image, &mut dst_image, &options)
        .context("Lanczos3 resize failed")?;

    let buffer = RgbImage::from_raw(size.width, size.height, dst_image.into_vec())
        .context("Lanczos3 resize produced an inconsistent buffer")?;
    Ok(DynamicImage::ImageRgb8(buffer))
}

/// Iteratively split an image horizontally until all segments have total pixels < size_threshold.
/// Keeps top-to-bottom reading order.
pub fn split_image_iterative(img: DynamicImage, size_threshold: Pixels) -> Vec<DynamicImage> {
    let mut result_images = Vec::new();
    let mut stack = vec![img];

    while let Some(current) = stack.pop() {
        let (w, h) = current.dimensions();
        let total_pixels = Pixels::new((w as u64) * (h as u64));
        if total_pixels < size_threshold || h <= 1 {
            result_images.push(current);
        } else {
            let middle = h / 2;
            let top_half = current.crop_imm(0, 0, w, middle);
            let bottom_half = current.crop_imm(0, middle, w, h - middle);
            // Push bottom half first so top half is popped next (LIFO stack)
            stack.push(bottom_half);
            stack.push(top_half);
        }
    }

    result_images
}

/// Proportional resize so total pixels <= size_threshold.
pub fn resize_image_by_total_pixels(
    img: DynamicImage,
    size_threshold: Pixels,
) -> Result<DynamicImage> {
    let (w, h) = img.dimensions();
    let total_pixels = Pixels::new((w as u64) * (h as u64));
    if total_pixels <= size_threshold {
        return Ok(img);
    }
    let scale_factor = ((size_threshold.raw() as f64) / (total_pixels.raw() as f64)).sqrt();
    let new_w = ((w as f64 * scale_factor).round() as u32).max(1);
    let new_h = ((h as f64 * scale_factor).round() as u32).max(1);
    resize_lanczos3(&img, Size::new(new_w, new_h))
}

/// Proportional resize so width <= max_width.
pub fn resize_image_by_width(img: DynamicImage, max_width: Pixels) -> Result<DynamicImage> {
    let (w, h) = img.dimensions();
    if Pixels::new(u64::from(w)) <= max_width {
        return Ok(img);
    }
    let max_width = max_width.raw() as u32;
    let scale_factor = (max_width as f64) / (w as f64);
    let new_h = ((h as f64 * scale_factor).round() as u32).max(1);
    resize_lanczos3(&img, Size::new(max_width, new_h))
}

/// Encode and save image as WebP (RGB, quality 90).
pub fn save_image_as_webp<P: AsRef<Path>>(
    img: &DynamicImage,
    dest_path: P,
    quality: f32,
) -> Result<()> {
    let owned;
    let rgb = match img.as_rgb8() {
        Some(rgb) => rgb,
        None => {
            owned = img.to_rgb8();
            &owned
        }
    };
    let encoder = webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height());
    let memory = encoder.encode(quality);
    if let Some(parent) = dest_path
        .as_ref()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(dest_path, &*memory)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Property-based checks for the resize/split geometry (docs/development.md): the
    // splitter must partition the page without reordering or losing pixels, and the
    // pixel-budget resize may only shrink (never grow, never vanish) the image.
    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// A test image whose every pixel encodes its own `(x, y)`, so a misplaced row
        /// or column is caught by an exact byte comparison. Dimensions keep the
        /// coordinates inside `u8`.
        fn marked_image(w: u32, h: u32) -> DynamicImage {
            DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
                image::Rgb([x as u8, y as u8, 0x5A])
            }))
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(64))]

            /// Total-pixel resize is the identity at or under the threshold; above it
            /// it only ever shrinks, keeping at least one pixel on each axis and never
            /// exceeding the source area. (It deliberately does *not* guarantee the
            /// result is under the threshold: the `.max(1)` clamp wins on extreme
            /// aspect ratios.)
            #[test]
            fn resize_by_total_pixels_only_shrinks(
                w in 1u32..=1024,
                h in 1u32..=1024,
                band in 0u8..=4,
            ) {
                let area = u64::from(w) * u64::from(h);
                let threshold = match band {
                    0 => 0,
                    1 => area.saturating_sub(1),
                    2 => area,
                    3 => area.saturating_add(1),
                    _ => area / 2 + 1,
                };
                let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
                // `anyhow::Error` does not implement `std::error::Error`, so convert it
                // explicitly rather than relying on proptest's blanket `From<E: Error>`.
                let out = resize_image_by_total_pixels(img, Pixels::new(threshold))
                    .map_err(|error| TestCaseError::fail(error.to_string()))?;
                let (new_w, new_h) = out.dimensions();
                if area <= threshold {
                    prop_assert_eq!((new_w, new_h), (w, h));
                } else {
                    prop_assert!(new_w >= 1 && new_h >= 1);
                    prop_assert!(new_w <= w && new_h <= h);
                    prop_assert!(u64::from(new_w) * u64::from(new_h) <= area);
                }
            }

            /// Horizontal splitting is a lossless, order-preserving partition of the
            /// page: every piece keeps the full width, the heights sum to the source
            /// height, each piece is terminal (`area < threshold` or a single row), and
            /// stacking the pieces top-to-bottom reproduces the source pixels exactly.
            #[test]
            fn split_image_iterative_reconstructs_the_source(
                w in 1u32..=256,
                h in 1u32..=256,
                band in 0u8..=3,
            ) {
                let area = u64::from(w) * u64::from(h);
                let threshold = match band {
                    0 => 0,
                    1 => area / 2,
                    2 => area,
                    _ => area.saturating_add(1),
                };
                let source = marked_image(w, h);
                let pieces = split_image_iterative(source.clone(), Pixels::new(threshold));

                let mut offset = 0u32;
                for piece in &pieces {
                    let (piece_w, piece_h) = piece.dimensions();
                    prop_assert_eq!(piece_w, w);
                    let piece_area = u64::from(piece_w) * u64::from(piece_h);
                    prop_assert!(piece_area < threshold || piece_h == 1);
                    let expected = source.crop_imm(0, offset, w, piece_h);
                    prop_assert_eq!(piece.to_rgb8().into_raw(), expected.to_rgb8().into_raw());
                    offset += piece_h;
                }
                prop_assert_eq!(offset, h);

                // Under the threshold the whole page is already one piece.
                if area < threshold {
                    prop_assert_eq!(pieces.len(), 1);
                    if let Some(first) = pieces.first() {
                        prop_assert_eq!(first.to_rgb8().into_raw(), source.to_rgb8().into_raw());
                    }
                }
            }
        }
    }
}
