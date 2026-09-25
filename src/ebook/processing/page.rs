//! `ComicPage` transform and encode pipeline (AGENTS.md §11.3–§11.7, §8).
//!
//! This is the Rust counterpart of KCC's `ComicPageParser` + `ComicPage`:
//!
//! 1. [`split_check`] classifies a decoded page and, when it is a double-page
//!    spread, produces the `-kcc-a/-kcc-b/-kcc-c/-kcc-d` payloads.
//! 2. each payload runs through gamma → grayscale → autocontrast/autolevel →
//!    resize → encode, in the same order KCC applies them.
//!
//! Cropping (margin, page-number, inter-panel) and the moiré eraser are Phase 3;
//! their hooks live in [`super::crop`] / [`super::interpanel`] / [`super::rainbow`].

use anyhow::{bail, Context, Result};
use fast_image_resize::images::Image as FastImage;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{
    DynamicImage, ExtendedColorType, GenericImageView, GrayImage, ImageEncoder, Luma, Rgb,
    RgbImage, RgbaImage,
};
use quantette::deps::palette::Srgb;
use quantette::{dither::FloydSteinberg, ImageRef, PaletteSize, Pipeline, QuantizeMethod};
use std::array;

use super::color::{color_check, luma601, rgb_to_ycbcr, to_luma601, ycbcr_to_rgb};
use crate::ebook::model::{Background, EncodedPage, MediaType, OrderClass, Page, PageFlags};
use crate::ebook::options::{BorderColor, Format, Options};
use crate::ebook::profiles::Profile;

/// Split a page wider than this multiple of the target aspect ratio (AGENTS.md §11.3).
const SPLIT_THRESHOLD: f64 = 1.16;
/// At or above this ratio a spread is only rotated, never bisected.
const BISECT_THRESHOLD: f64 = 1.8;
/// Aspect-ratio tolerance for crop-to-fill resizes (KCC's `AUTO_CROP_THRESHOLD`).
const AUTO_CROP_THRESHOLD: f64 = 0.015;

/// The profile's output geometry, enlarged by 1.5× in `--hq` panel-view mode.
pub fn profile_size(options: &Options) -> (u32, u32) {
    let (mut width, mut height) = (options.profile_data.width, options.profile_data.height);
    if options.hq {
        width = (f64::from(width) * 1.5) as u32;
        height = (f64::from(height) * 1.5) as u32;
    }
    (width, height)
}

/// One payload produced by the splitter: an image plus the order class it maps to.
struct Payload {
    order: OrderClass,
    image: DynamicImage,
    rotated: bool,
}

/// Process one decoded page into its encoded output page(s).
pub fn process_page(page: &Page, options: &Options, size: (u32, u32)) -> Result<Vec<EncodedPage>> {
    if options.no_processing {
        return passthrough(page, options);
    }

    let fill = page_fill(page, options);
    split_check(&page.image, options, size)
        .into_iter()
        .map(|payload| encode_payload(payload, options, size, page, fill))
        .collect()
}

/// `--no-processing`: emit the source unchanged, ignoring the profile entirely.
fn passthrough(page: &Page, options: &Options) -> Result<Vec<EncodedPage>> {
    let media_type = page.source_media_type.unwrap_or(MediaType::Jpeg);
    let bytes = match &page.raw {
        Some(raw) => raw.clone(),
        // Trees built without a source payload still round-trip through the codec.
        None => encode_dynamic(&page.image, media_type, options.jpeg_quality)?,
    };
    let (width, height) = page.image.dimensions();
    Ok(vec![EncodedPage {
        name: output_name(&page.source_name, OrderClass::Normal, media_type),
        order_class: OrderClass::Normal,
        media_type,
        bytes,
        width,
        height,
        flags: PageFlags::default(),
    }])
}

/// The padding colour: an explicit `--black-borders`/`--white-borders` wins over
/// the detected page background.
pub fn page_fill(page: &Page, options: &Options) -> Background {
    match options.borders_color {
        Some(BorderColor::White) => Background::White,
        Some(BorderColor::Black) => Background::Black,
        None => page.background,
    }
}

/// Classify a page, returning the payloads the spread splitter produced.
fn split_check(image: &DynamicImage, options: &Options, size: (u32, u32)) -> Vec<Payload> {
    let (width, height) = image.dimensions();
    let (dst_width, dst_height) = size;
    let right_to_left = options.right_to_left;
    let landscape_mismatch = (width > height) != (dst_width > dst_height);

    if options.maximize_strips {
        return vec![maximize_strips(image, right_to_left)];
    }
    if options.webtoon {
        return vec![Payload {
            order: OrderClass::Normal,
            image: image.clone(),
            rotated: false,
        }];
    }
    if landscape_mismatch && width <= dst_height && height <= dst_width && options.splitter == 1 {
        return vec![rotate_payload(image, options)];
    }
    if landscape_mismatch && f64::from(width) / f64::from(height) > SPLIT_THRESHOLD {
        let ratio = f64::from(width) / f64::from(height);
        let mut payloads = Vec::new();

        if options.splitter != 1 && ratio < BISECT_THRESHOLD {
            let (first, second) = bisect(image, right_to_left);
            payloads.push(Payload {
                order: OrderClass::SplitLeft,
                image: first,
                rotated: false,
            });
            payloads.push(Payload {
                order: OrderClass::SplitRight,
                image: second,
                rotated: false,
            });
        }
        if options.splitter > 0 || (options.splitter == 0 && ratio >= BISECT_THRESHOLD) {
            payloads.push(rotate_payload(image, options));
        }
        return payloads;
    }

    vec![Payload {
        order: OrderClass::Normal,
        image: image.clone(),
        rotated: false,
    }]
}

/// Turn a 1×4 strip into a 2×2 one by stacking the two halves (KCC's
/// `--maximize-strips`).
fn maximize_strips(image: &DynamicImage, right_to_left: bool) -> Payload {
    let (width, height) = image.dimensions();
    let half = width / 2;

    let left = image.crop_imm(0, 0, half, height).to_rgb8();
    let right = image.crop_imm(half, 0, half, height).to_rgb8();
    let (first, second) = if right_to_left {
        (&right, &left)
    } else {
        (&left, &right)
    };

    let mut composed = RgbImage::new(half, height * 2);
    image::imageops::replace(&mut composed, first, 0, 0);
    image::imageops::replace(&mut composed, second, 0, i64::from(height));

    Payload {
        order: OrderClass::Normal,
        image: DynamicImage::ImageRgb8(composed),
        rotated: false,
    }
}

/// The rotated-spread payload (`-kcc-a`/`-kcc-d`).
fn rotate_payload(image: &DynamicImage, options: &Options) -> Payload {
    Payload {
        order: if options.rotate_first {
            OrderClass::RotateFirst
        } else {
            OrderClass::RotateLast
        },
        image: rotate_spread(image, options),
        rotated: !options.no_rotate,
    }
}

/// Rotate a double-page spread 90°, in the direction KCC picked.
fn rotate_spread(image: &DynamicImage, options: &Options) -> DynamicImage {
    if options.no_rotate {
        return image.clone();
    }
    if options.rotate_right {
        image.rotate90()
    } else {
        image.rotate270()
    }
}

/// Bisect a spread into two reading-order halves (`-kcc-b`, `-kcc-c`).
fn bisect(image: &DynamicImage, right_to_left: bool) -> (DynamicImage, DynamicImage) {
    let (width, height) = image.dimensions();
    let (first, second) = if width > height {
        let half = width / 2;
        (
            image.crop_imm(0, 0, half, height),
            image.crop_imm(half, 0, half, height),
        )
    } else {
        let half = height / 2;
        (
            image.crop_imm(0, 0, width, half),
            image.crop_imm(0, half, width, half),
        )
    };

    if right_to_left {
        (second, first)
    } else {
        (first, second)
    }
}

/// Run one payload through the transform pipeline and encode it.
fn encode_payload(
    payload: Payload,
    options: &Options,
    size: (u32, u32),
    page: &Page,
    fill: Background,
) -> Result<EncodedPage> {
    let original_is_grayscale = is_grayscale_image(&payload.image);
    let rgb = payload.image.to_rgb8();
    let color = color_check(&rgb, original_is_grayscale, options);
    let color_output = color && options.force_color;

    let image = prepare_image(
        DynamicImage::ImageRgb8(rgb),
        options,
        size,
        payload.order,
        fill,
        color,
        color_output,
    );

    let (media_type, bytes) = encode_image(&image, options, color_output)?;
    let (width, height) = image.dimensions();

    Ok(EncodedPage {
        name: output_name(&page.source_name, payload.order, media_type),
        order_class: payload.order,
        media_type,
        bytes,
        width,
        height,
        flags: PageFlags {
            order_class: payload.order,
            rotated: payload.rotated,
            black_background: fill == Background::Black,
            above: false,
            below: false,
        },
    })
}

/// gamma → grayscale → autocontrast/autolevel → resize, in KCC's order.
fn prepare_image(
    mut image: DynamicImage,
    options: &Options,
    size: (u32, u32),
    order: OrderClass,
    fill: Background,
    color: bool,
    color_output: bool,
) -> DynamicImage {
    gamma_correct(&mut image, options, color);
    if !color_output {
        image = DynamicImage::ImageLuma8(to_luma601(&image));
    }
    autocontrast_image(&mut image, options, color);
    resize_image(&mut image, options, size, order, fill);
    // The moiré eraser runs on the resized plane, after autocontrast and before
    // quantization (KCC's `optimizeForDisplay`).
    if options.erase_rainbow && image.width() > 1 && image.height() > 1 {
        image = super::rainbow::erase_rainbow_artifacts(&image, color_output);
    }
    image
}

/// Whether a decoded image came from a grayscale source (KCC's `L`/`1` modes).
pub(crate) fn is_grayscale_image(image: &DynamicImage) -> bool {
    matches!(
        image,
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageLuma16(_)
    )
}

// --- transforms -----------------------------------------------------------------

/// Gamma correction, applied to the RGB image before grayscale conversion.
///
/// `--gamma` defaults to 0, which falls back to the profile gamma (1.0 today, so
/// a no-op). See AGENTS.md §11.6.
fn gamma_correct(image: &mut DynamicImage, options: &Options, color: bool) {
    let mut gamma = f64::from(options.gamma);
    if gamma < 0.1 {
        gamma = f64::from(options.profile_data.gamma);
        if (gamma - 1.0).abs() > f64::EPSILON && color {
            gamma = 1.0;
        }
    }
    if (gamma - 1.0).abs() < f64::EPSILON {
        return;
    }

    let lut: [u8; 256] = array::from_fn(|i| {
        (255.0 * (i as f64 / 255.0).powf(gamma))
            .round()
            .clamp(0.0, 255.0) as u8
    });

    match image {
        DynamicImage::ImageRgb8(buffer) => {
            imageproc::map::map_subpixels_mut(buffer, |value| lut[value as usize]);
        }
        DynamicImage::ImageLuma8(buffer) => {
            imageproc::map::map_subpixels_mut(buffer, |value| lut[value as usize]);
        }
        other => {
            let mut rgb = other.to_rgb8();
            imageproc::map::map_subpixels_mut(&mut rgb, |value| lut[value as usize]);
            *other = DynamicImage::ImageRgb8(rgb);
        }
    }
}

/// Autocontrast, plus the optional autolevel pass (AGENTS.md §11.6).
///
/// `color` is the page's colour *detection* result (not whether colour is kept):
/// KCC only autocontrasts detected-colour pages with `--color-autocontrast`.
fn autocontrast_image(image: &mut DynamicImage, options: &Options, color: bool) {
    if options.webtoon || options.no_auto_contrast {
        return;
    }
    if color && !options.color_auto_contrast {
        return;
    }

    // "Extremely low contrast is probably intentional": 255 - 32 * 3.
    let range = luma_range(image);
    if range.max - range.min < 255 - 32 * 3 {
        return;
    }

    if options.auto_level {
        autolevel_image(image, color);
    }

    // Pillow's autocontrast recomputes the range on the current pixels.
    let range = luma_range(image);
    if range.min >= range.max {
        return;
    }
    stretch_contrast(image, range.min, range.max);
}

/// The Rec. 601 luma minimum and maximum of an image.
fn luma_range(image: &DynamicImage) -> imageproc::stats::MinMax<u8> {
    let gray = to_luma601(image);
    imageproc::stats::min_max(&gray)[0]
}

/// Stretch `[min, max]` to `[0, 255]` in every channel, preserving tone.
fn stretch_contrast(image: &mut DynamicImage, min: u8, max: u8) {
    match image {
        DynamicImage::ImageLuma8(buffer) => {
            let stretched = imageproc::contrast::stretch_contrast(buffer, min, max, 0, 255);
            *buffer = stretched;
        }
        DynamicImage::ImageRgb8(buffer) => {
            let stretched = imageproc::contrast::stretch_contrast(buffer, min, max, 0, 255);
            *buffer = stretched;
        }
        other => {
            let rgb = other.to_rgb8();
            let stretched = imageproc::contrast::stretch_contrast(&rgb, min, max, 0, 255);
            *other = DynamicImage::ImageRgb8(stretched);
        }
    }
}

/// `--auto-level`: clamp everything below the most common dark value up to it.
fn autolevel_image(image: &mut DynamicImage, color: bool) {
    let black_point = black_point(image, color);

    if color {
        let rgb = image.to_rgb8();
        let leveled = RgbImage::from_fn(rgb.width(), rgb.height(), |x, y| {
            let pixel = rgb.get_pixel(x, y);
            let (y, cb, cr) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
            let (r, g, b) = ycbcr_to_rgb(y.max(black_point), cb, cr);
            Rgb([r, g, b])
        });
        *image = DynamicImage::ImageRgb8(leveled);
    } else {
        let gray = to_luma601(image);
        let leveled = GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
            Luma([gray.get_pixel(x, y)[0].max(black_point)])
        });
        *image = DynamicImage::ImageLuma8(leveled);
    }
}

/// The most common dark-pixel value, KCC's black point.
fn black_point(image: &DynamicImage, color: bool) -> u8 {
    let mut histogram = [0u32; 256];

    if color {
        let rgb = image.to_rgb8();
        for pixel in rgb.pixels() {
            let (y, _, _) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
            histogram[y as usize] += 1;
        }
    } else {
        let gray = to_luma601(image);
        for pixel in gray.pixels() {
            histogram[pixel[0] as usize] += 1;
        }
    }

    let dark = histogram[..64].iter().copied().max().unwrap_or(0);
    // KCC searches the whole histogram for the first bin with that count.
    histogram
        .iter()
        .position(|&count| count == dark)
        .unwrap_or(0) as u8
}

/// Resize to the profile, following KCC's branch order (AGENTS.md §11.6).
fn resize_image(
    image: &mut DynamicImage,
    options: &Options,
    size: (u32, u32),
    order: OrderClass,
    fill: Background,
) {
    let (width, height) = image.dimensions();
    let method = resize_method(image, size);

    if options.stretch {
        *image = resize_to(image, size.0, size.1, method);
        return;
    }
    if options.wallpaper {
        // KCC 9.x leaves this branch unreachable (a bare `pass`); we implement the
        // documented intent. See AGENTS.md §13.5.
        *image = fit(image, size, method);
        return;
    }
    if options.no_rotate
        && matches!(order, OrderClass::RotateFirst | OrderClass::RotateLast)
        && !options.kindle_scribe_azw3
    {
        if options.kindle_azw3 && (width > 1920 || height > 1920) {
            *image = contain(image, (1920, 1920), Method::Lanczos);
        } else if width > size.0 * 2 || height > size.1 {
            *image = contain(image, (size.0 * 2, size.1), Method::Lanczos);
        }
        return;
    }
    if method == Method::Bicubic && !options.upscale {
        // The page already fits the profile and upscaling is off.
        return;
    }

    let ratio_device = f64::from(size.1) / f64::from(size.0);
    let ratio_image = f64::from(height) / f64::from(width);
    let white_borders = matches!(options.borders_color, Some(BorderColor::White));
    let kdx = options.profile == Profile::Kdx && !options.custom_profile;

    if kdx && (ratio_image - ratio_device).abs() < AUTO_CROP_THRESHOLD * 3.0
        || (ratio_image - ratio_device).abs() < AUTO_CROP_THRESHOLD
    {
        *image = fit(image, size, method);
    } else if matches!(options.format, Format::Cbz | Format::Pdf) && !white_borders {
        *image = pad(image, size, method, fill);
    } else {
        *image = contain(image, size, method);
    }
}

/// The resampling filter KCC picks: bicubic when the page already fits the
/// profile, Lanczos when it must be scaled down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Bicubic,
    Lanczos,
}

impl Method {
    fn filter(self) -> FilterType {
        match self {
            // Pillow's BICUBIC is a Catmull-Rom cubic convolution.
            Method::Bicubic => FilterType::CatmullRom,
            Method::Lanczos => FilterType::Lanczos3,
        }
    }
}

/// Pillow's `resize_method`: BICUBIC when the page fits, LANCZOS otherwise.
fn resize_method(image: &DynamicImage, size: (u32, u32)) -> Method {
    let (width, height) = image.dimensions();
    if width <= size.0 && height <= size.1 {
        Method::Bicubic
    } else {
        Method::Lanczos
    }
}

/// Resize the whole image to an exact size (Pillow's `resize`).
fn resize_to(image: &DynamicImage, width: u32, height: u32, method: Method) -> DynamicImage {
    let alg = ResizeAlg::Convolution(method.filter());
    match image {
        DynamicImage::ImageLuma8(buffer) => {
            DynamicImage::ImageLuma8(resize_luma(buffer, width, height, alg))
        }
        DynamicImage::ImageRgb8(buffer) => {
            DynamicImage::ImageRgb8(resize_rgb(buffer, width, height, alg))
        }
        DynamicImage::ImageRgba8(buffer) => {
            DynamicImage::ImageRgba8(resize_rgba(buffer, width, height, alg))
        }
        other => {
            let rgb = other.to_rgb8();
            DynamicImage::ImageRgb8(resize_rgb(&rgb, width, height, alg))
        }
    }
}

fn resizer() -> Resizer {
    Resizer::new()
}

fn resize_luma(source: &GrayImage, width: u32, height: u32, alg: ResizeAlg) -> GrayImage {
    let (src_w, src_h) = source.dimensions();
    let src = FastImage::from_vec_u8(src_w, src_h, source.as_raw().clone(), PixelType::U8)
        .expect("valid source image");
    let mut dst = FastImage::new(width, height, PixelType::U8);
    let options = ResizeOptions::new().resize_alg(alg);
    resizer()
        .resize(&src, &mut dst, &options)
        .expect("resize succeeds");
    GrayImage::from_raw(width, height, dst.into_vec()).expect("valid destination image")
}

fn resize_rgb(source: &RgbImage, width: u32, height: u32, alg: ResizeAlg) -> RgbImage {
    let (src_w, src_h) = source.dimensions();
    let src = FastImage::from_vec_u8(src_w, src_h, source.as_raw().clone(), PixelType::U8x3)
        .expect("valid source image");
    let mut dst = FastImage::new(width, height, PixelType::U8x3);
    let options = ResizeOptions::new().resize_alg(alg);
    resizer()
        .resize(&src, &mut dst, &options)
        .expect("resize succeeds");
    RgbImage::from_raw(width, height, dst.into_vec()).expect("valid destination image")
}

fn resize_rgba(source: &RgbaImage, width: u32, height: u32, alg: ResizeAlg) -> RgbaImage {
    let (src_w, src_h) = source.dimensions();
    let src = FastImage::from_vec_u8(src_w, src_h, source.as_raw().clone(), PixelType::U8x4)
        .expect("valid source image");
    let mut dst = FastImage::new(width, height, PixelType::U8x4);
    let options = ResizeOptions::new().resize_alg(alg);
    resizer()
        .resize(&src, &mut dst, &options)
        .expect("resize succeeds");
    RgbaImage::from_raw(width, height, dst.into_vec()).expect("valid destination image")
}

/// Pillow's `ImageOps.fit`: crop to the target aspect ratio, then resize exactly.
fn fit(image: &DynamicImage, size: (u32, u32), method: Method) -> DynamicImage {
    let (width, height) = image.dimensions();
    let image_ratio = f64::from(width) / f64::from(height);
    let box_ratio = f64::from(size.0) / f64::from(size.1);

    if (box_ratio - image_ratio).abs() < f64::EPSILON {
        return resize_to(image, size.0, size.1, method);
    }

    if box_ratio > image_ratio {
        let cropped_height = (f64::from(width) / box_ratio).round() as u32;
        let top = (height.saturating_sub(cropped_height)) / 2;
        let cropped = image.crop_imm(0, top, width, cropped_height.max(1));
        resize_to(&cropped, size.0, size.1, method)
    } else {
        let cropped_width = (f64::from(height) * box_ratio).round() as u32;
        let left = (width.saturating_sub(cropped_width)) / 2;
        let cropped = image.crop_imm(left, 0, cropped_width.max(1), height);
        resize_to(&cropped, size.0, size.1, method)
    }
}

/// Pillow's `ImageOps.contain`: scale to fit within `size`, preserving aspect.
fn contain(image: &DynamicImage, size: (u32, u32), method: Method) -> DynamicImage {
    let (width, height) = image.dimensions();
    let (target_w, target_h) = contain_size(width, height, size);
    resize_to(image, target_w, target_h, method)
}

/// Pillow's `ImageOps.contain` size calculation (`get_contain_resolution`).
fn contain_size(width: u32, height: u32, size: (u32, u32)) -> (u32, u32) {
    let image_ratio = f64::from(width) / f64::from(height);
    let dest_ratio = f64::from(size.0) / f64::from(size.1);

    if image_ratio != dest_ratio {
        if image_ratio > dest_ratio {
            let new_height =
                (f64::from(height) / f64::from(width) * f64::from(size.0)).round() as u32;
            if new_height != size.1 {
                return (size.0, new_height.max(1));
            }
        } else {
            let new_width =
                (f64::from(width) / f64::from(height) * f64::from(size.1)).round() as u32;
            if new_width != size.0 {
                return (new_width.max(1), size.1);
            }
        }
    }
    size
}

/// Pillow's `ImageOps.pad`: contain, then centre on a fill-coloured canvas.
fn pad(image: &DynamicImage, size: (u32, u32), method: Method, fill: Background) -> DynamicImage {
    let contained = contain(image, size, method);
    let (width, height) = contained.dimensions();
    let mut canvas = filled_like(&contained, size.0, size.1, fill_value(fill));

    let x = i64::from((size.0.saturating_sub(width)) / 2);
    let y = i64::from((size.1.saturating_sub(height)) / 2);
    match (&mut canvas, &contained) {
        (DynamicImage::ImageLuma8(c), DynamicImage::ImageLuma8(s)) => {
            image::imageops::replace(c, s, x, y);
        }
        (DynamicImage::ImageRgb8(c), DynamicImage::ImageRgb8(s)) => {
            image::imageops::replace(c, s, x, y);
        }
        (DynamicImage::ImageRgba8(c), DynamicImage::ImageRgba8(s)) => {
            image::imageops::replace(c, s, x, y);
        }
        _ => {}
    }
    canvas
}

/// The fill value for a background colour.
fn fill_value(fill: Background) -> u8 {
    match fill {
        Background::White => 255,
        Background::Black => 0,
    }
}

/// A solid image of the same pixel type as `template`.
fn filled_like(template: &DynamicImage, width: u32, height: u32, value: u8) -> DynamicImage {
    match template {
        DynamicImage::ImageLuma8(_) => {
            DynamicImage::ImageLuma8(GrayImage::from_pixel(width, height, Luma([value])))
        }
        DynamicImage::ImageRgb8(_) => {
            DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([value; 3])))
        }
        DynamicImage::ImageRgba8(_) => DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([value, value, value, 255]),
        )),
        _ => DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([value; 3]))),
    }
}

// --- encoding -------------------------------------------------------------------

/// One page prepared for a PNG writer.
enum PreparedPng {
    Gray(GrayImage),
    Rgb(RgbImage),
    Indexed(Quantized),
}

/// A page mapped onto a fixed palette.
struct Quantized {
    width: u32,
    height: u32,
    palette: Vec<[u8; 3]>,
    indices: Vec<u8>,
}

/// Encode a prepared image, following KCC's `save_with_codec` branch order.
fn encode_image(
    image: &DynamicImage,
    options: &Options,
    color_output: bool,
) -> Result<(MediaType, Vec<u8>)> {
    let png_branch = options.force_png && (!color_output || options.force_png_rgb);

    if png_branch {
        if options.webp_output {
            let rgb = image.to_rgb8();
            return Ok((MediaType::WebP, encode_webp_lossless(&rgb)));
        }
        if options.kindle_azw3 {
            return Ok((MediaType::Gif, encode_gif(image)?));
        }

        let prepared = if !color_output {
            // Grayscale page under `--force-png`: optionally quantise to the
            // profile palette, then fall back to grayscale where KCC does.
            if options.no_quantize {
                PreparedPng::Gray(to_luma601(image))
            } else {
                let quantized = quantize(&image.to_rgb8(), options.profile_data.palette)?;
                if matches!(options.format, Format::Pdf)
                    || (options.profile == Profile::Kdx
                        && options.format == Format::Cbz
                        && !options.custom_profile)
                    || options.png_legacy
                {
                    PreparedPng::Gray(quantized_to_luma(&quantized))
                } else {
                    PreparedPng::Indexed(quantized)
                }
            }
        } else {
            PreparedPng::Rgb(image.to_rgb8())
        };

        let bytes = match &prepared {
            PreparedPng::Gray(gray) => encode_png_gray(gray)?,
            PreparedPng::Rgb(rgb) => encode_png_rgb(rgb)?,
            PreparedPng::Indexed(quantized) => encode_png_indexed(quantized)?,
        };
        return Ok((MediaType::Png, bytes));
    }

    if options.webp_output {
        let rgb = image.to_rgb8();
        return Ok((
            MediaType::WebP,
            encode_webp_lossy(&rgb, options.jpeg_quality),
        ));
    }
    Ok((MediaType::Jpeg, encode_jpeg(image, options.jpeg_quality)?))
}

/// Quantise an RGB image onto a fixed palette with Floyd–Steinberg dithering,
/// matching Pillow's `quantize(palette=...)`.
fn quantize(image: &RgbImage, palette: &[u8]) -> Result<Quantized> {
    if palette.len() < 3 || !palette.len().is_multiple_of(3) {
        bail!("profile palette must be a non-empty list of RGB triples");
    }
    let colors: Vec<Srgb<u8>> = palette
        .as_chunks::<3>()
        .0
        .iter()
        .map(|triple| Srgb::new(triple[0], triple[1], triple[2]))
        .collect();
    let palette_size = PaletteSize::from_u8_clamped(colors.len() as u8);
    let method = QuantizeMethod::try_from(colors).map_err(|err| anyhow::anyhow!("{err}"))?;
    let input = ImageRef::try_from(image).context("image is too large to quantise")?;

    let indexed = Pipeline::new()
        .palette_size(palette_size)
        .quantize_method(method)
        .ditherer(FloydSteinberg::new())
        .input_image(input)
        .output_srgb8_indexed_image();

    Ok(Quantized {
        width: indexed.width(),
        height: indexed.height(),
        palette: indexed
            .palette()
            .iter()
            .map(|color| [color.red, color.green, color.blue])
            .collect(),
        indices: indexed.indices().to_vec(),
    })
}

/// Rebuild a grayscale image from a quantised page (KCC's P→L conversion).
fn quantized_to_luma(quantized: &Quantized) -> GrayImage {
    GrayImage::from_fn(quantized.width, quantized.height, |x, y| {
        let index = usize::from(quantized.indices[(y * quantized.width + x) as usize]);
        let color = quantized.palette[index];
        Luma([luma601(color[0], color[1], color[2])])
    })
}

fn encode_jpeg(image: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let image = encodable(image);
    let image = image.as_ref();
    let mut buffer = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buffer, quality);
    encoder
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            dynamic_color_type(image),
        )
        .context("JPEG encoding failed")?;
    Ok(buffer)
}

fn encode_png_gray(image: &GrayImage) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    PngEncoder::new(&mut buffer)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::L8,
        )
        .context("PNG encoding failed")?;
    Ok(buffer)
}

fn encode_png_rgb(image: &RgbImage) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    PngEncoder::new(&mut buffer)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .context("PNG encoding failed")?;
    Ok(buffer)
}

/// Write a palette PNG at the smallest bit depth the palette fits in.
fn encode_png_indexed(quantized: &Quantized) -> Result<Vec<u8>> {
    let palette: Vec<u8> = quantized
        .palette
        .iter()
        .flat_map(|color| [color[0], color[1], color[2]])
        .collect();
    let depth = match quantized.palette.len() {
        0..=2 => png::BitDepth::One,
        3..=4 => png::BitDepth::Two,
        5..=16 => png::BitDepth::Four,
        _ => png::BitDepth::Eight,
    };
    let packed = pack_indices(&quantized.indices, quantized.width, quantized.height, depth);

    let mut buffer = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buffer, quantized.width, quantized.height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(depth);
        encoder.set_palette(palette);
        let mut writer = encoder.write_header().context("PNG header failed")?;
        writer
            .write_image_data(&packed)
            .context("PNG data failed")?;
    }
    Ok(buffer)
}

/// Pack one-byte palette indices into the sub-byte layout the `png` crate writes.
fn pack_indices(indices: &[u8], width: u32, height: u32, depth: png::BitDepth) -> Vec<u8> {
    let bits = depth as u32;
    if bits == 8 {
        return indices.to_vec();
    }
    let per_byte = 8 / bits;
    let mask = (1u8 << bits) - 1;
    let row_bytes = width.div_ceil(per_byte);
    let mut packed = vec![0u8; (row_bytes * height) as usize];

    for y in 0..height {
        for x in 0..width {
            let value = indices[(y * width + x) as usize] & mask;
            let shift = 8 - bits * ((x % per_byte) + 1);
            packed[(y * row_bytes + x / per_byte) as usize] |= value << shift;
        }
    }
    packed
}

fn encode_gif(image: &DynamicImage) -> Result<Vec<u8>> {
    // `image`'s GIF encoder rejects L8, and GIF is palette-based anyway.
    let rgb = image.to_rgb8();
    let mut buffer = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut buffer);
        encoder
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            )
            .context("GIF encoding failed")?;
    }
    Ok(buffer)
}

fn encode_webp_lossy(image: &RgbImage, quality: u8) -> Vec<u8> {
    let encoder = webp::Encoder::from_rgb(image.as_raw(), image.width(), image.height());
    encoder.encode(f32::from(quality)).to_vec()
}

fn encode_webp_lossless(image: &RgbImage) -> Vec<u8> {
    let encoder = webp::Encoder::from_rgb(image.as_raw(), image.width(), image.height());
    encoder.encode_lossless().to_vec()
}

/// Encode an image in a given media type at `quality` (used for `--no-processing`
/// trees that have no retained source bytes).
fn encode_dynamic(image: &DynamicImage, media_type: MediaType, quality: u8) -> Result<Vec<u8>> {
    match media_type {
        MediaType::Jpeg => encode_jpeg(image, quality),
        MediaType::Png => match image {
            DynamicImage::ImageLuma8(gray) => encode_png_gray(gray),
            other => encode_png_rgb(&other.to_rgb8()),
        },
        MediaType::Gif => encode_gif(image),
        MediaType::WebP => Ok(encode_webp_lossy(&image.to_rgb8(), quality)),
    }
}

/// A borrowable 8/16-bit view of `image`.
///
/// The loaders only ever decode 8- or 16-bit images, but `DynamicImage` can also
/// hold 32-bit float buffers; those are normalised so the byte-oriented encoders
/// stay correct.
fn encodable(image: &DynamicImage) -> std::borrow::Cow<'_, DynamicImage> {
    match image {
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            std::borrow::Cow::Owned(DynamicImage::ImageRgb8(image.to_rgb8()))
        }
        _ => std::borrow::Cow::Borrowed(image),
    }
}

fn dynamic_color_type(image: &DynamicImage) -> ExtendedColorType {
    match image {
        DynamicImage::ImageLuma8(_) => ExtendedColorType::L8,
        DynamicImage::ImageLumaA8(_) => ExtendedColorType::La8,
        DynamicImage::ImageRgb8(_) => ExtendedColorType::Rgb8,
        DynamicImage::ImageRgba8(_) => ExtendedColorType::Rgba8,
        DynamicImage::ImageLuma16(_) => ExtendedColorType::L16,
        DynamicImage::ImageLumaA16(_) => ExtendedColorType::La16,
        DynamicImage::ImageRgb16(_) => ExtendedColorType::Rgb16,
        DynamicImage::ImageRgba16(_) => ExtendedColorType::Rgba16,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => ExtendedColorType::Rgb8,
        // `DynamicImage` is `#[non_exhaustive]`.
        _ => ExtendedColorType::Rgb8,
    }
}

/// The output file name for a payload, keeping the source directory and adding
/// the `-kcc-<order>` suffix (AGENTS.md §10).
fn output_name(source_name: &str, order: OrderClass, media_type: MediaType) -> String {
    let (directory, file_name) = match source_name.rsplit_once('/') {
        Some((directory, file_name)) => (Some(directory), file_name),
        None => (None, source_name),
    };
    let stem = match file_name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file_name,
    };
    let name = format!("{stem}-kcc-{}.{}", order.suffix(), media_type.extension());
    match directory {
        Some(directory) => format!("{directory}/{name}"),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::options::Options;
    use clap::Parser;

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

    /// A page of a solid colour.
    fn page(width: u32, height: u32, color: [u8; 3]) -> Page {
        Page {
            source_name: "page.png".to_string(),
            rel_path: "page.png".to_string(),
            image: DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb(color))),
            background: Background::White,
            flags: PageFlags::default(),
            raw: None,
            source_media_type: Some(MediaType::Png),
        }
    }

    fn order_of(payloads: &[Payload]) -> Vec<OrderClass> {
        payloads.iter().map(|payload| payload.order).collect()
    }

    #[test]
    fn normal_page_is_passed_through() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(100, 150));
        let payloads = split_check(&image, &options(&[]), (1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::Normal]);
    }

    #[test]
    fn wide_spread_below_bisect_is_split() {
        // 1.5:1 landscape against a portrait profile sits between the split and
        // bisect thresholds, so it is bisected rather than rotated.
        let image = DynamicImage::ImageRgb8(RgbImage::new(300, 200));
        let payloads = split_check(&image, &options(&[]), (1072, 1448));
        assert_eq!(
            order_of(&payloads),
            vec![OrderClass::SplitLeft, OrderClass::SplitRight]
        );
        // Both halves are half as wide.
        assert_eq!(payloads[0].image.width(), 150);
        assert_eq!(payloads[1].image.width(), 150);
    }

    #[test]
    fn wide_spread_at_bisect_is_rotated() {
        // 2.5:1 exceeds BISECT_THRESHOLD, so the spread only rotates.
        let image = DynamicImage::ImageRgb8(RgbImage::new(500, 200));
        let payloads = split_check(&image, &options(&[]), (1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::RotateLast]);
        assert!(payloads[0].rotated);
        // Rotated 90°: dimensions swap.
        assert_eq!(payloads[0].image.dimensions(), (200, 500));
    }

    #[test]
    fn rotate_first_puts_the_rotated_spread_first() {
        // 2.5:1 exceeds the bisect threshold, so it only rotates.
        let image = DynamicImage::ImageRgb8(RgbImage::new(500, 200));
        let payloads = split_check(&image, &options(&["--rotate-first"]), (1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::RotateFirst]);
    }

    #[test]
    fn no_rotate_keeps_the_spread_upright() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(500, 200));
        let payloads = split_check(&image, &options(&["--no-rotate"]), (1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::RotateLast]);
        assert!(!payloads[0].rotated);
        assert_eq!(payloads[0].image.dimensions(), (500, 200));
    }

    #[test]
    fn right_to_left_swaps_the_split_halves() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(300, 200, |x, _| {
            if x < 150 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 0, 0])
            }
        }));
        let ltr = split_check(&image, &options(&[]), (1072, 1448));
        let rtl = split_check(&image, &options(&["--manga"]), (1072, 1448));
        // Left-to-right reads left half first; right-to-left reads right half.
        assert_eq!(ltr[0].image.get_pixel(0, 0)[0], 0);
        assert_eq!(rtl[0].image.get_pixel(0, 0)[0], 255);
    }

    #[test]
    fn maximize_strips_stacks_the_halves() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(400, 100));
        let payloads = split_check(&image, &options(&["--maximize-strips"]), (1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::Normal]);
        assert_eq!(payloads[0].image.dimensions(), (200, 200));
    }

    #[test]
    fn contains_scales_down_to_fit() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(2000, 1000));
        let contained = contain(&image, (1000, 1000), Method::Lanczos);
        assert_eq!(contained.dimensions(), (1000, 500));
    }

    #[test]
    fn pad_fills_to_the_exact_profile_size() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(2000, 1000));
        let padded = pad(&image, (1000, 1000), Method::Lanczos, Background::White);
        assert_eq!(padded.dimensions(), (1000, 1000));
        // The letterbox is white.
        assert_eq!(padded.to_rgb8().get_pixel(500, 0)[0], 255);
    }

    #[test]
    fn fit_crops_then_scales() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
        let fitted = fit(&image, (500, 1000), Method::Lanczos);
        assert_eq!(fitted.dimensions(), (500, 1000));
    }

    #[test]
    fn grayscale_pages_encode_as_jpeg_by_default() {
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&[]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].media_type, MediaType::Jpeg);
        assert_eq!(encoded[0].name, "page-kcc-x.jpg");
        assert_eq!((encoded[0].width, encoded[0].height), (40, 40));
    }

    #[test]
    fn force_png_emits_an_indexed_png() {
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&["-p", "KoE", "--force-png"]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        assert_eq!(encoded[0].media_type, MediaType::Png);
        // Palette PNG signature + IHDR bit depth 4 (16-colour palette).
        assert_eq!(&encoded[0].bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(encoded[0].bytes[24], 4, "IHDR bit depth");
    }

    #[test]
    fn png_legacy_emits_grayscale_png() {
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&["-p", "KoE", "--force-png", "--png-legacy"]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        assert_eq!(encoded[0].media_type, MediaType::Png);
        // IHDR colour type 0 (grayscale).
        assert_eq!(encoded[0].bytes[25], 0, "IHDR colour type");
    }

    #[test]
    fn force_png_on_kindle_emits_gif() {
        // Kindle output replaces monochrome PNGs with GIFs (KCC's AZW3 path).
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&["--force-png"]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        assert_eq!(encoded[0].media_type, MediaType::Gif);
        assert!(encoded[0].bytes.starts_with(b"GIF"));
    }

    #[test]
    fn force_color_keeps_a_colour_page_as_jpeg() {
        let source = page(40, 40, [255, 0, 0]);
        let options = options(&["--force-color", "--force-png"]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        // A colour page stays JPEG unless `--force-png-rgb` is given.
        assert_eq!(encoded[0].media_type, MediaType::Jpeg);
    }

    #[test]
    fn force_png_rgb_keeps_a_colour_page_as_png() {
        let source = page(40, 40, [255, 0, 0]);
        let options = options(&[
            "-p",
            "KoE",
            "--force-color",
            "--force-png",
            "--force-png-rgb",
        ]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        assert_eq!(encoded[0].media_type, MediaType::Png);
    }

    #[test]
    fn no_processing_copies_the_source_bytes() {
        let mut source = page(10, 10, [1, 2, 3]);
        source.raw = Some(vec![1, 2, 3, 4]);
        let options = options(&["--no-processing"]);
        let encoded = process_page(&source, &options, profile_size(&options)).unwrap();
        assert_eq!(encoded[0].bytes, vec![1, 2, 3, 4]);
        assert_eq!(encoded[0].media_type, MediaType::Png);
        assert_eq!(encoded[0].order_class, OrderClass::Normal);
    }

    #[test]
    fn gamma_correction_darkens_midtones() {
        let mut image = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 2, Rgb([128, 128, 128])));
        gamma_correct(&mut image, &options(&["--gamma", "2.0"]), false);
        let value = image.to_rgb8().get_pixel(0, 0)[0];
        // 255 * (128/255)^2 ≈ 64 (KCC rounds to 64).
        assert_eq!(value, 64);
    }

    #[test]
    fn autocontrast_stretches_a_full_range_image() {
        // A high-contrast ramp (range 220 > 159) is stretched to [0, 255].
        let mut image = DynamicImage::ImageLuma8(GrayImage::from_fn(256, 1, |x, _| {
            Luma([(10 + x * 220 / 255) as u8])
        }));
        autocontrast_image(&mut image, &options(&[]), false);
        let gray = to_luma601(&image);
        assert_eq!(gray.get_pixel(0, 0)[0], 0);
        assert_eq!(gray.get_pixel(255, 0)[0], 255);
    }

    #[test]
    fn autocontrast_leaves_a_low_contrast_image_alone() {
        // Range 100 < 159: probably intentional, so it is left untouched.
        let mut image = DynamicImage::ImageLuma8(GrayImage::from_fn(16, 1, |x, _| {
            Luma([(x as u8).saturating_add(100)])
        }));
        let before = image.clone();
        autocontrast_image(&mut image, &options(&[]), false);
        assert_eq!(image.to_luma8(), before.to_luma8());
    }

    #[test]
    fn autolevel_raises_the_black_point() {
        // Most dark pixels sit at 30, so everything below 30 is clamped up.
        let mut values = vec![0u8; 40];
        values.extend(std::iter::repeat_n(30u8, 200));
        values.extend(std::iter::repeat_n(200u8, 16));
        let mut image = DynamicImage::ImageLuma8(GrayImage::from_raw(1, 256, values).unwrap());
        autolevel_image(&mut image, false);
        let gray = image.to_luma8();
        assert!(gray.pixels().all(|pixel| pixel[0] >= 30));
        assert_eq!(gray.get_pixel(0, 0)[0], 30);
    }

    #[test]
    fn indexed_packing_places_the_high_nibble_first() {
        // Two 4-bit indices per byte: 0x1 and 0x2 -> 0x12.
        let packed = pack_indices(&[1, 2], 2, 1, png::BitDepth::Four);
        assert_eq!(packed, vec![0x12]);
    }
}
