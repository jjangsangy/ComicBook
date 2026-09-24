use anyhow::Result;
use fast_image_resize::images::Image as FastImage;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::{DynamicImage, GenericImageView, RgbImage};
use std::fs;
use std::path::Path;

pub const IMG_EXTENSIONS: &[&str] = &[
    "jpeg", "jpg", "png", "tiff", "tif", "bmp", "webp", "gif", "pgm",
];

pub fn is_image_extension(ext: &str) -> bool {
    let lower = ext.trim_start_matches('.').to_ascii_lowercase();
    IMG_EXTENSIONS.contains(&lower.as_str())
}

pub fn is_image_file<P: AsRef<Path>>(path: P) -> bool {
    let p = path.as_ref();
    if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
        if is_image_extension(ext) {
            return true;
        }
    }
    let lossy = p.to_string_lossy();
    if let Some(file_part) = lossy.rsplit(['/', '\\']).next() {
        if let Some(ext) = file_part.rsplit('.').next() {
            if ext != file_part && is_image_extension(ext) {
                return true;
            }
        }
    }
    false
}

/// Resize an image using high-quality SIMD-accelerated Lanczos3 convolution.
pub fn resize_lanczos3(img: &DynamicImage, new_w: u32, new_h: u32) -> DynamicImage {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let src_image =
        FastImage::from_vec_u8(w, h, rgb.into_raw(), PixelType::U8x3).expect("valid src image");
    let mut dst_image = FastImage::new(new_w, new_h, PixelType::U8x3);

    let mut resizer = Resizer::new();
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    resizer
        .resize(&src_image, &mut dst_image, &options)
        .expect("resize failed");

    let dst_raw = dst_image.into_vec();
    let rgb_buf = RgbImage::from_raw(new_w, new_h, dst_raw).expect("valid dst image");
    DynamicImage::ImageRgb8(rgb_buf)
}

/// Iteratively split an image horizontally until all segments have total pixels < size_threshold.
/// Keeps top-to-bottom reading order.
pub fn split_image_iterative(img: DynamicImage, size_threshold: u64) -> Vec<DynamicImage> {
    let mut result_images = Vec::new();
    let mut stack = vec![img];

    while let Some(current) = stack.pop() {
        let (w, h) = current.dimensions();
        let total_pixels = (w as u64) * (h as u64);
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
pub fn resize_image_by_total_pixels(img: DynamicImage, size_threshold: u64) -> DynamicImage {
    let (w, h) = img.dimensions();
    let total_pixels = (w as u64) * (h as u64);
    if total_pixels <= size_threshold {
        return img;
    }
    let scale_factor = ((size_threshold as f64) / (total_pixels as f64)).sqrt();
    let new_w = ((w as f64 * scale_factor).round() as u32).max(1);
    let new_h = ((h as f64 * scale_factor).round() as u32).max(1);
    resize_lanczos3(&img, new_w, new_h)
}

/// Proportional resize so width <= max_width.
pub fn resize_image_by_width(img: DynamicImage, max_width: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w <= max_width {
        return img;
    }
    let scale_factor = (max_width as f64) / (w as f64);
    let new_h = ((h as f64 * scale_factor).round() as u32).max(1);
    resize_lanczos3(&img, max_width, new_h)
}

/// Encode and save image as WebP (RGB, quality 90).
pub fn save_image_as_webp<P: AsRef<Path>>(
    img: &DynamicImage,
    dest_path: P,
    quality: f32,
) -> Result<()> {
    let rgb = img.to_rgb8();
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
