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
