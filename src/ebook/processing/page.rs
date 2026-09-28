//! `ComicPage` transform and encode pipeline (see docs/processing.md).
//!
//! This is the Rust counterpart of KCC's `ComicPageParser` + `ComicPage`:
//!
//! 1. [`split_check`] classifies a decoded page and, when it is a double-page
//!    spread, produces the `-kcc-a/-kcc-b/-kcc-c/-kcc-d` payloads.
//! 2. each payload runs through gamma → grayscale → autocontrast/autolevel →
//!    resize → encode, in the same order KCC applies them.
//!
//! Cropping (margin, page-number, inter-panel) and the moiré eraser run from
//! [`super::prepare_page`] and live in [`super::crop`] / [`super::interpanel`] /
//! [`super::rainbow`].

use anyhow::{bail, Context, Result};
use bitvec::prelude::{BitVec, Msb0};
use fast_image_resize::images::Image as FastImage;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{
    DynamicImage, ExtendedColorType, GenericImageView, GrayImage, ImageBuffer, ImageEncoder, Luma,
    Pixel, Rgb, RgbImage, RgbaImage,
};
use quantette::deps::palette::Srgb;
use quantette::{dither::FloydSteinberg, ImageRef, PaletteSize, Pipeline, QuantizeMethod};
use std::array;
use std::borrow::Cow;

use super::color::{color_check, luma601, luma_view, rgb_to_ycbcr, to_luma601, ycbcr_to_rgb};
use crate::ebook::model::{Background, EncodedPage, MediaType, OrderClass, Page, PageFlags};
use crate::ebook::options::{
    Autocontrast, BorderColor, Gamma, Geometry, Layout, Options, OutputEncoding, Splitter,
};
use crate::ebook::processing::kernels;
use crate::ebook::profiles::Profile;
use crate::units::{Quality, Range, Size};

/// Split a page wider than this multiple of the target aspect ratio (see docs/processing.md).
const SPLIT_THRESHOLD: f64 = 1.16;
/// At or above this ratio a spread is only rotated, never bisected.
const BISECT_THRESHOLD: f64 = 1.8;
/// Aspect-ratio tolerance for crop-to-fill resizes (KCC's `AUTO_CROP_THRESHOLD`).
const AUTO_CROP_THRESHOLD: f64 = 0.015;
/// Kindle Scribe KF8 pages taller than this are split into `-above`/`-below`
/// halves (KCC's literal `1920` in `saveToDir`).
pub(crate) const SCRIBE_MAX_DIMENSION: u32 = 1920;

/// One payload produced by the splitter: an image plus the order class it maps to.
struct Payload {
    order: OrderClass,
    image: DynamicImage,
    rotated: bool,
}

/// Process one decoded page into its encoded output page(s).
///
/// Used by tests and one-off callers; the tree pipeline hands its pixels in
/// directly through [`process_decoded`] so it never holds two decoded copies.
pub fn process_page(page: &Page, options: &Options, size: Size) -> Result<Vec<EncodedPage>> {
    if options.processing.no_processing {
        return passthrough(page, options);
    }

    let image = page
        .decoded()
        .context("page has no decoded image to process")?
        .clone();
    process_decoded(page, image, options, size)
}

/// Encode a page whose decoded pixels have already been moved out of the tree.
///
/// Taking ownership lets the common (non-split) path move its RGB buffer straight
/// into the transform instead of cloning it, which halves the per-page working
/// set (see docs/architecture.md).
pub(crate) fn process_decoded(
    page: &Page,
    image: DynamicImage,
    options: &Options,
    size: Size,
) -> Result<Vec<EncodedPage>> {
    let fill = page_fill(page, options);
    let mut out = Vec::new();
    for payload in split_check(image, options, size) {
        out.extend(encode_payload(payload, options, size, page, fill)?);
    }
    Ok(out)
}

/// `--no-processing`: emit the source unchanged, ignoring the profile entirely.
fn passthrough(page: &Page, options: &Options) -> Result<Vec<EncodedPage>> {
    let media_type = page.source_media_type.unwrap_or(MediaType::Jpeg);
    let bytes = match &page.raw {
        Some(raw) => raw.clone(),
        // Trees built without a source payload (webtoon strips) still round-trip
        // through the codec.
        None => {
            let image = page
                .decoded()
                .context("page has neither source bytes nor decoded pixels")?;
            encode_dynamic(image, media_type, options.processing.jpeg_quality)?
        }
    };
    Ok(vec![passthrough_page(page, media_type, bytes)])
}

/// The tree pipeline's `--no-processing`: like [`passthrough`], but the source
/// bytes are *moved* out of the page (the tree does not need them again), so the
/// archive is not duplicated while the output is assembled (see
/// docs/architecture.md).
pub(crate) fn passthrough_in_place(page: &mut Page, options: &Options) -> Result<Vec<EncodedPage>> {
    let media_type = page.source_media_type.unwrap_or(MediaType::Jpeg);
    let bytes = match page.raw.take() {
        Some(raw) => raw,
        None => {
            let image = page
                .decoded()
                .context("page has neither source bytes nor decoded pixels")?;
            encode_dynamic(image, media_type, options.processing.jpeg_quality)?
        }
    };
    Ok(vec![passthrough_page(page, media_type, bytes)])
}

/// The untouched-source [`EncodedPage`].
///
/// Under `--no-processing` KCC never runs `ComicPage`, so the sanitized name keeps
/// no `-kcc-x` order suffix (see docs/porting.md).
fn passthrough_page(page: &Page, media_type: MediaType, bytes: Vec<u8>) -> EncodedPage {
    EncodedPage {
        name: unsuffixed_name(&page.source_name, media_type),
        order_class: OrderClass::Normal,
        media_type,
        bytes,
        size: page.dimensions(),
        flags: PageFlags::default(),
    }
}

/// The padding colour: an explicit `--black-borders`/`--white-borders` wins over
/// the detected page background.
pub fn page_fill(page: &Page, options: &Options) -> Background {
    match options.processing.borders {
        Some(BorderColor::White) => Background::White,
        Some(BorderColor::Black) => Background::Black,
        None => page.background,
    }
}

/// Classify a page, returning the payloads the spread splitter produced.
///
/// Takes the decoded image by value so the common path can move it into its
/// single [`Payload`] rather than cloning; split/rotate paths still read from the
/// borrowed image to derive their outputs.
fn split_check(image: DynamicImage, options: &Options, size: Size) -> Vec<Payload> {
    let (width, height) = image.dimensions();
    let right_to_left = options.main.right_to_left();
    let landscape_mismatch = (width > height) != (size.width > size.height);

    if options.processing.strips.maximize {
        return vec![maximize_strips(&image, right_to_left)];
    }
    if options.main.webtoon {
        return vec![Payload {
            order: OrderClass::Normal,
            image,
            rotated: false,
        }];
    }
    if landscape_mismatch
        && width <= size.height
        && height <= size.width
        && options.processing.splitter == Splitter::Rotate
    {
        return vec![rotate_payload(image, options)];
    }
    if landscape_mismatch && f64::from(width) / f64::from(height) > SPLIT_THRESHOLD {
        let ratio = f64::from(width) / f64::from(height);
        let mut payloads = Vec::new();

        if options.processing.splitter != Splitter::Rotate && ratio < BISECT_THRESHOLD {
            let (first, second) = bisect(&image, right_to_left);
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
        if options.processing.splitter != Splitter::Split
            || (options.processing.splitter == Splitter::Split && ratio >= BISECT_THRESHOLD)
        {
            payloads.push(rotate_payload(image, options));
        }
        return payloads;
    }

    vec![Payload {
        order: OrderClass::Normal,
        image,
        rotated: false,
    }]
}

/// Turn a 1×4 strip into a 2×2 one by stacking the two halves (KCC's
/// `--maximize-strips`).
fn maximize_strips(image: &DynamicImage, right_to_left: bool) -> Payload {
    let (width, height) = image.dimensions();
    let half = width / 2;

    let left = image.crop_imm(0, 0, half, height).into_rgb8();
    let right = image.crop_imm(half, 0, half, height).into_rgb8();
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
///
/// Takes the image by value: under `--no-rotate` the page is passed through
/// unchanged, so it can be moved instead of copied.
fn rotate_payload(image: DynamicImage, options: &Options) -> Payload {
    let order = if options.processing.rotation.first {
        OrderClass::RotateFirst
    } else {
        OrderClass::RotateLast
    };
    let rotated = if options.processing.rotation.no_rotate {
        image
    } else if options.processing.rotation.right {
        image.rotate90()
    } else {
        image.rotate270()
    };
    Payload {
        order,
        image: rotated,
        rotated: !options.processing.rotation.no_rotate,
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
///
/// A single payload normally yields one [`EncodedPage`], but Kindle Scribe KF8
/// output splits a page taller than [`SCRIBE_MAX_DIMENSION`] into two images and
/// names a shorter one `-whole` (KCC's `saveToDir`).
fn encode_payload(
    payload: Payload,
    options: &Options,
    size: Size,
    page: &Page,
    fill: Background,
) -> Result<Vec<EncodedPage>> {
    let original_is_grayscale = is_grayscale_image(&payload.image);
    // `color_check` short-circuits to `false` for a grayscale source without ever
    // looking at the pixels, so keep the luma plane instead of expanding it to RGB
    // and back — one less full-image copy for the common manga scan.
    let (image, color) = if original_is_grayscale {
        (payload.image, false)
    } else {
        let rgb = match payload.image {
            DynamicImage::ImageRgb8(buffer) => buffer,
            other => other.to_rgb8(),
        };
        let color = color_check(&rgb, false, options);
        (DynamicImage::ImageRgb8(rgb), color)
    };
    let color_output = color && options.processing.color.force_color;

    let image = prepare_image(
        image,
        options,
        size,
        payload.order,
        fill,
        color,
        color_output,
    )?;

    let flags = |above: bool, below: bool| PageFlags {
        order_class: payload.order,
        rotated: payload.rotated,
        black_background: fill == Background::Black,
        above,
        below,
    };

    if options.processing.scribe {
        let image_size = Size::from_dimensions(image.dimensions());
        if image_size.height > SCRIBE_MAX_DIMENSION {
            let above = image.crop_imm(0, 0, image_size.width, SCRIBE_MAX_DIMENSION);
            let below = image.crop_imm(
                0,
                SCRIBE_MAX_DIMENSION,
                image_size.width,
                image_size.height - SCRIBE_MAX_DIMENSION,
            );
            let (above_type, above_bytes) = encode_image(&above, options, color_output)?;
            let (below_type, below_bytes) = encode_image(&below, options, color_output)?;
            return Ok(vec![
                EncodedPage {
                    name: split_name(&page.source_name, payload.order, "above", above_type),
                    order_class: payload.order,
                    media_type: above_type,
                    bytes: above_bytes,
                    size: Size::new(image_size.width, SCRIBE_MAX_DIMENSION),
                    flags: flags(true, false),
                },
                EncodedPage {
                    name: split_name(&page.source_name, payload.order, "below", below_type),
                    order_class: payload.order,
                    media_type: below_type,
                    bytes: below_bytes,
                    size: Size::new(image_size.width, image_size.height - SCRIBE_MAX_DIMENSION),
                    flags: flags(false, true),
                },
            ]);
        }

        let (media_type, bytes) = encode_image(&image, options, color_output)?;
        return Ok(vec![EncodedPage {
            name: split_name(&page.source_name, payload.order, "whole", media_type),
            order_class: payload.order,
            media_type,
            bytes,
            size: image_size,
            flags: flags(false, false),
        }]);
    }

    let (media_type, bytes) = encode_image(&image, options, color_output)?;
    let image_size = Size::from_dimensions(image.dimensions());

    Ok(vec![EncodedPage {
        name: output_name(&page.source_name, payload.order, media_type),
        order_class: payload.order,
        media_type,
        bytes,
        size: image_size,
        flags: flags(false, false),
    }])
}

/// gamma → grayscale → autocontrast/autolevel → resize, in KCC's order.
fn prepare_image(
    mut image: DynamicImage,
    options: &Options,
    size: Size,
    order: OrderClass,
    fill: Background,
    color: bool,
    color_output: bool,
) -> Result<DynamicImage> {
    gamma_correct(&mut image, options, color);
    if !color_output && !matches!(image, DynamicImage::ImageLuma8(_)) {
        // `to_luma601` is the identity on an L8 plane, so skipping the round-trip
        // keeps a grayscale page's working set to one buffer.
        image = DynamicImage::ImageLuma8(to_luma601(&image));
    }
    autocontrast_image(&mut image, options, color);
    resize_image(&mut image, options, size, order, fill)?;
    // The moiré eraser runs on the resized plane, after autocontrast and before
    // quantization (KCC's `optimizeForDisplay`).
    if options.processing.erase_rainbow && image.width() > 1 && image.height() > 1 {
        image = super::rainbow::erase_rainbow_artifacts(&image, color_output);
    }
    Ok(image)
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
/// a no-op). See docs/processing.md.
fn gamma_correct(image: &mut DynamicImage, options: &Options, color: bool) {
    let mut gamma = match options.processing.gamma {
        Gamma::Auto => f64::from(options.device.data.gamma),
        Gamma::Linear(value) => f64::from(value),
    };
    if matches!(options.processing.gamma, Gamma::Auto)
        && (gamma - 1.0).abs() > f64::EPSILON
        && color
    {
        gamma = 1.0;
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

/// Autocontrast, plus the optional autolevel pass (see docs/processing.md).
///
/// `color` is the page's colour *detection* result (not whether colour is kept):
/// KCC only autocontrasts detected-colour pages with `--color-autocontrast`.
fn autocontrast_image(image: &mut DynamicImage, options: &Options, color: bool) {
    if options.main.webtoon || options.processing.autocontrast == Autocontrast::Off {
        return;
    }
    if color && !options.processing.color.autocontrast_color {
        return;
    }

    // "Extremely low contrast is probably intentional": 255 - 32 * 3.
    let range = luma_range(image);
    if range.spread() < 255 - 32 * 3 {
        return;
    }

    if options.processing.autocontrast == Autocontrast::Level {
        autolevel_image(image, color);
    }

    // Pillow's autocontrast recomputes the range on the current pixels.
    let range = luma_range(image);
    if range.is_degenerate() {
        return;
    }
    stretch_contrast(image, range);
}

/// The Rec. 601 luma range of an image.
fn luma_range(image: &DynamicImage) -> Range {
    kernels::luma_min_max(image).unwrap_or(Range::new(0, 0))
}

/// Pillow's unconditional `ImageOps.autocontrast(preserve_tone=True)`: stretch
/// the luminance range to `[0, 255]` using one range for every channel.
///
/// This is the cover's autocontrast (KCC's `Cover.process`), which — unlike the
/// per-page pass — has no low-contrast guard and is not gated by `--no-` /
/// `--color-autocontrast`. A flat image is left untouched (Pillow's zero-width
/// range would divide by zero).
pub(crate) fn autocontrast_preserve_tone(image: &mut DynamicImage) {
    let range = luma_range(image);
    if range.is_degenerate() {
        return;
    }
    stretch_contrast(image, range);
}

/// Stretch the range to `[0, 255]` in every channel, preserving tone.
///
/// Equivalent to `imageproc::contrast::stretch_contrast` with a 0..255 output
/// range (its formula depends only on the input value), but precomputed as a LUT
/// and applied in place so the reference call's extra clone and per-pixel
/// division both disappear.
fn stretch_contrast(image: &mut DynamicImage, range: Range) {
    let lut = kernels::stretch_contrast_lut(range);
    match image {
        DynamicImage::ImageLuma8(buffer) => kernels::apply_lut_in_place(buffer, &lut),
        DynamicImage::ImageRgb8(buffer) => kernels::apply_lut_in_place(buffer, &lut),
        other => {
            let mut rgb = other.to_rgb8();
            kernels::apply_lut_in_place(&mut rgb, &lut);
            *other = DynamicImage::ImageRgb8(rgb);
        }
    }
}

/// `--auto-level`: clamp everything below the most common dark value up to it.
fn autolevel_image(image: &mut DynamicImage, color: bool) {
    let black_point = black_point(image, color);

    if color {
        // Level the plane in place when it is already RGB; only other pixel
        // types pay for the `to_rgb8` conversion and a fresh buffer.
        match image {
            DynamicImage::ImageRgb8(buffer) => {
                for pixel in buffer.pixels_mut() {
                    let (y, cb, cr) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
                    let (r, g, b) = ycbcr_to_rgb(y.max(black_point), cb, cr);
                    *pixel = Rgb([r, g, b]);
                }
            }
            other => {
                let mut rgb = other.to_rgb8();
                for pixel in rgb.pixels_mut() {
                    let (y, cb, cr) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
                    let (r, g, b) = ycbcr_to_rgb(y.max(black_point), cb, cr);
                    *pixel = Rgb([r, g, b]);
                }
                *other = DynamicImage::ImageRgb8(rgb);
            }
        }
    } else {
        match image {
            DynamicImage::ImageLuma8(buffer) => {
                for value in buffer.iter_mut() {
                    *value = (*value).max(black_point);
                }
            }
            other => {
                let mut gray = to_luma601(other);
                for value in gray.iter_mut() {
                    *value = (*value).max(black_point);
                }
                *other = DynamicImage::ImageLuma8(gray);
            }
        }
    }
}

/// The most common dark-pixel value, KCC's black point.
fn black_point(image: &DynamicImage, color: bool) -> u8 {
    let mut histogram = [0u32; 256];

    if color {
        if let Some(rgb) = image.as_rgb8() {
            for pixel in rgb.pixels() {
                let (y, _, _) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
                histogram[y as usize] += 1;
            }
        } else {
            let rgb = image.to_rgb8();
            for pixel in rgb.pixels() {
                let (y, _, _) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
                histogram[y as usize] += 1;
            }
        }
    } else if let Some(gray) = image.as_luma8() {
        for value in gray.iter() {
            histogram[*value as usize] += 1;
        }
    } else {
        let gray = to_luma601(image);
        for value in gray.iter() {
            histogram[*value as usize] += 1;
        }
    }

    let dark = histogram[..64].iter().copied().max().unwrap_or(0);
    // KCC searches the whole histogram for the first bin with that count.
    histogram
        .iter()
        .position(|&count| count == dark)
        .unwrap_or(0) as u8
}

/// Resize to the profile, following KCC's branch order (see docs/processing.md).
fn resize_image(
    image: &mut DynamicImage,
    options: &Options,
    size: Size,
    order: OrderClass,
    fill: Background,
) -> Result<()> {
    let (width, height) = image.dimensions();
    let method = resize_method(image, size);

    if options.processing.sizing.stretch {
        *image = resize_to(image, size, method)?;
        return Ok(());
    }
    if options.main.layout == Layout::Wallpaper {
        // KCC 9.x leaves this branch unreachable (a bare `pass`); we implement the
        // documented intent. See docs/porting.md.
        *image = fit(image, size, method)?;
        return Ok(());
    }
    if options.processing.rotation.no_rotate
        && matches!(order, OrderClass::RotateFirst | OrderClass::RotateLast)
        && !options.processing.scribe
    {
        if options.kindle_azw3() && (width > SCRIBE_MAX_DIMENSION || height > SCRIBE_MAX_DIMENSION)
        {
            *image = contain(
                image,
                Size::new(SCRIBE_MAX_DIMENSION, SCRIBE_MAX_DIMENSION),
                Method::Lanczos,
            )?;
        } else if width > size.width * 2 || height > size.height {
            *image = contain(
                image,
                Size::new(size.width * 2, size.height),
                Method::Lanczos,
            )?;
        }
        return Ok(());
    }
    if method == Method::Bicubic && !options.processing.sizing.upscale {
        // The page already fits the profile and upscaling is off.
        return Ok(());
    }

    let ratio_device = f64::from(size.height) / f64::from(size.width);
    let ratio_image = f64::from(height) / f64::from(width);
    let white_borders = matches!(options.processing.borders, Some(BorderColor::White));
    let kdx = options.device.profile == Profile::Kdx
        && !matches!(options.device.geometry, Geometry::Custom { .. });

    if kdx && (ratio_image - ratio_device).abs() < AUTO_CROP_THRESHOLD * 3.0
        || (ratio_image - ratio_device).abs() < AUTO_CROP_THRESHOLD
    {
        *image = fit(image, size, method)?;
    } else if matches!(
        options.output.encoding,
        OutputEncoding::Cbz | OutputEncoding::Pdf
    ) && !white_borders
    {
        *image = pad(image, size, method, fill)?;
    } else {
        *image = contain(image, size, method)?;
    }
    Ok(())
}

/// The resampling filter KCC picks: bicubic when the page already fits the
/// profile, Lanczos when it must be scaled down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
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
fn resize_method(image: &DynamicImage, size: Size) -> Method {
    let (width, height) = image.dimensions();
    if width <= size.width && height <= size.height {
        Method::Bicubic
    } else {
        Method::Lanczos
    }
}

/// Resize the whole image to an exact size (Pillow's `resize`).
fn resize_to(image: &DynamicImage, size: Size, method: Method) -> Result<DynamicImage> {
    let alg = ResizeAlg::Convolution(method.filter());
    Ok(match image {
        DynamicImage::ImageLuma8(buffer) => {
            DynamicImage::ImageLuma8(resize_buffer(buffer, size, alg, PixelType::U8)?)
        }
        DynamicImage::ImageRgb8(buffer) => {
            DynamicImage::ImageRgb8(resize_buffer(buffer, size, alg, PixelType::U8x3)?)
        }
        DynamicImage::ImageRgba8(buffer) => {
            DynamicImage::ImageRgba8(resize_buffer(buffer, size, alg, PixelType::U8x4)?)
        }
        other => {
            let rgb = other.to_rgb8();
            DynamicImage::ImageRgb8(resize_buffer(&rgb, size, alg, PixelType::U8x3)?)
        }
    })
}

/// Resize an 8-bit L8/Rgb8/Rgba8 buffer with `fast_image_resize`, preserving the type.
fn resize_buffer<P>(
    source: &ImageBuffer<P, Vec<u8>>,
    size: Size,
    alg: ResizeAlg,
    pixel_type: PixelType,
) -> Result<ImageBuffer<P, Vec<u8>>>
where
    P: Pixel<Subpixel = u8> + 'static,
{
    let (src_w, src_h) = source.dimensions();
    // Borrow the source samples instead of copying them into an owned image:
    // `ImageRef` implements `IntoImageView`, so the resizer reads straight from
    // the caller's buffer (no full-image duplicate, see docs/architecture.md).
    let src = fast_image_resize::images::ImageRef::new(src_w, src_h, source.as_raw(), pixel_type)?;
    let mut dst = FastImage::new(size.width, size.height, pixel_type);
    let options = ResizeOptions::new().resize_alg(alg);
    Resizer::new().resize(&src, &mut dst, &options)?;
    ImageBuffer::from_raw(size.width, size.height, dst.into_vec())
        .context("resized image buffer has an unexpected length")
}

/// Pillow's `ImageOps.fit`: crop to the target aspect ratio, then resize exactly
/// (kept bespoke, see docs/dependencies.md — Pillow's half-to-even rounding is pinned).
pub(crate) fn fit(image: &DynamicImage, size: Size, method: Method) -> Result<DynamicImage> {
    let (width, height) = image.dimensions();
    let image_ratio = f64::from(width) / f64::from(height);
    let box_ratio = f64::from(size.width) / f64::from(size.height);

    if (box_ratio - image_ratio).abs() < f64::EPSILON {
        return resize_to(image, size, method);
    }

    if box_ratio > image_ratio {
        let cropped_height = (f64::from(width) / box_ratio).round() as u32;
        let top = (height.saturating_sub(cropped_height)) / 2;
        let cropped = image.crop_imm(0, top, width, cropped_height.max(1));
        resize_to(&cropped, size, method)
    } else {
        let cropped_width = (f64::from(height) * box_ratio).round() as u32;
        let left = (width.saturating_sub(cropped_width)) / 2;
        let cropped = image.crop_imm(left, 0, cropped_width.max(1), height);
        resize_to(&cropped, size, method)
    }
}

/// Pillow's `ImageOps.contain`: scale to fit within `size`, preserving aspect.
pub(crate) fn contain(image: &DynamicImage, size: Size, method: Method) -> Result<DynamicImage> {
    let (width, height) = image.dimensions();
    let target = contain_size(width, height, size);
    resize_to(image, target, method)
}

/// Pillow's `Image.thumbnail`: shrink to fit `size`, preserving aspect, never
/// enlarging (a no-op when the image already fits). Used for the cover.
///
/// Takes the image by value so a thumbnail that is already small enough can be
/// returned untouched instead of copied.
pub(crate) fn thumbnail(image: DynamicImage, size: Size, method: Method) -> Result<DynamicImage> {
    let (width, height) = image.dimensions();
    if width <= size.width && height <= size.height {
        return Ok(image);
    }
    let target = contain_size(width, height, size);
    resize_to(&image, target, method)
}

/// Pillow's `ImageOps.contain` size calculation (`get_contain_resolution`; kept
/// bespoke, see docs/dependencies.md).
fn contain_size(width: u32, height: u32, size: Size) -> Size {
    let image_ratio = f64::from(width) / f64::from(height);
    let dest_ratio = f64::from(size.width) / f64::from(size.height);

    if image_ratio != dest_ratio {
        if image_ratio > dest_ratio {
            let new_height =
                (f64::from(height) / f64::from(width) * f64::from(size.width)).round() as u32;
            if new_height != size.height {
                return Size::new(size.width, new_height.max(1));
            }
        } else {
            let new_width =
                (f64::from(width) / f64::from(height) * f64::from(size.height)).round() as u32;
            if new_width != size.width {
                return Size::new(new_width.max(1), size.height);
            }
        }
    }
    size
}

/// Pillow's `ImageOps.pad`: contain, then centre on a fill-coloured canvas.
fn pad(image: &DynamicImage, size: Size, method: Method, fill: Background) -> Result<DynamicImage> {
    let contained = contain(image, size, method)?;
    let (width, height) = contained.dimensions();
    let mut canvas = filled_like(&contained, size, fill_value(fill));

    let x = i64::from((size.width.saturating_sub(width)) / 2);
    let y = i64::from((size.height.saturating_sub(height)) / 2);
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
    Ok(canvas)
}

/// The fill value for a background colour.
fn fill_value(fill: Background) -> u8 {
    match fill {
        Background::White => 255,
        Background::Black => 0,
    }
}

/// A solid image of the same pixel type as `template`.
fn filled_like(template: &DynamicImage, size: Size, value: u8) -> DynamicImage {
    match template {
        DynamicImage::ImageLuma8(_) => DynamicImage::ImageLuma8(GrayImage::from_pixel(
            size.width,
            size.height,
            Luma([value]),
        )),
        DynamicImage::ImageRgb8(_) => DynamicImage::ImageRgb8(RgbImage::from_pixel(
            size.width,
            size.height,
            Rgb([value; 3]),
        )),
        DynamicImage::ImageRgba8(_) => DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            size.width,
            size.height,
            image::Rgba([value, value, value, 255]),
        )),
        _ => DynamicImage::ImageRgb8(RgbImage::from_pixel(
            size.width,
            size.height,
            Rgb([value; 3]),
        )),
    }
}

// --- encoding -------------------------------------------------------------------

/// One page prepared for a PNG writer. The pixel buffer is borrowed from the
/// pipeline image whenever it already has the right type, so no copy is made.
enum PreparedPng<'a> {
    Gray(Cow<'a, GrayImage>),
    Rgb(Cow<'a, RgbImage>),
    Indexed(Quantized),
}

/// A fixed RGB palette as the quantiser returns it: 8-bit triples, wrapped by move
/// so the index plane can never be confused with pixel bytes. Never cloned.
#[derive(Debug)]
struct Palette(Vec<[u8; 3]>);

impl Palette {
    fn new(triples: Vec<[u8; 3]>) -> Self {
        Self(triples)
    }

    fn as_triples(&self) -> &[[u8; 3]] {
        &self.0
    }

    /// Flatten to the byte layout `png::Encoder::set_palette` expects.
    fn flatten(&self) -> Vec<u8> {
        self.0
            .iter()
            .flat_map(|color| [color[0], color[1], color[2]])
            .collect()
    }

    /// The smallest PNG bit depth the palette fits in.
    fn bit_depth(&self) -> png::BitDepth {
        match self.0.len() {
            0..=2 => png::BitDepth::One,
            3..=4 => png::BitDepth::Two,
            5..=16 => png::BitDepth::Four,
            _ => png::BitDepth::Eight,
        }
    }
}

/// One palette index per pixel (an index into a [`Palette`], not a colour).
#[derive(Debug)]
struct PaletteIndices(Vec<u8>);

impl PaletteIndices {
    fn new(indices: Vec<u8>) -> Self {
        Self(indices)
    }

    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// A page mapped onto a fixed palette.
struct Quantized {
    width: u32,
    height: u32,
    palette: Palette,
    indices: PaletteIndices,
}

/// Encode a prepared image, following KCC's `save_with_codec` branch order.
fn encode_image(
    image: &DynamicImage,
    options: &Options,
    color_output: bool,
) -> Result<(MediaType, Vec<u8>)> {
    let png_branch =
        options.processing.png.force && (!color_output || options.processing.png.force_rgb);

    if png_branch {
        if options.processing.webp_output {
            return Ok((MediaType::WebP, encode_webp_lossless(&rgb_view(image))));
        }
        if options.kindle_azw3() {
            return Ok((MediaType::Gif, encode_gif(image)?));
        }

        let prepared = if !color_output {
            // Grayscale page under `--force-png`: optionally quantise to the
            // profile palette, then fall back to grayscale where KCC does.
            if options.processing.png.no_quantize {
                PreparedPng::Gray(luma_view(image))
            } else {
                let quantized = quantize(&rgb_view(image), options.device.data.palette)?;
                if matches!(options.output.encoding, OutputEncoding::Pdf)
                    || (options.device.profile == Profile::Kdx
                        && matches!(options.output.encoding, OutputEncoding::Cbz)
                        && !matches!(options.device.geometry, Geometry::Custom { .. }))
                    || options.processing.png.legacy
                {
                    PreparedPng::Gray(Cow::Owned(quantized_to_luma(&quantized)))
                } else {
                    PreparedPng::Indexed(quantized)
                }
            }
        } else {
            PreparedPng::Rgb(rgb_view(image))
        };

        let bytes = match &prepared {
            PreparedPng::Gray(gray) => encode_png(
                gray.as_raw(),
                gray.width(),
                gray.height(),
                ExtendedColorType::L8,
            )?,
            PreparedPng::Rgb(rgb) => encode_png(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            )?,
            PreparedPng::Indexed(quantized) => encode_png_indexed(quantized)?,
        };
        return Ok((MediaType::Png, bytes));
    }

    if options.processing.webp_output {
        return Ok((
            MediaType::WebP,
            encode_webp_lossy(&rgb_view(image), options.processing.jpeg_quality),
        ));
    }
    Ok((
        MediaType::Jpeg,
        encode_jpeg(image, options.processing.jpeg_quality)?,
    ))
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
        palette: Palette::new(
            indexed
                .palette()
                .iter()
                .map(|color| [color.red, color.green, color.blue])
                .collect(),
        ),
        indices: PaletteIndices::new(indexed.indices().to_vec()),
    })
}

/// Rebuild a grayscale image from a quantised page (KCC's P→L conversion).
fn quantized_to_luma(quantized: &Quantized) -> GrayImage {
    let palette = quantized.palette.as_triples();
    let indices = quantized.indices.as_slice();
    GrayImage::from_fn(quantized.width, quantized.height, |x, y| {
        let index = usize::from(indices[(y * quantized.width + x) as usize]);
        let color = palette[index];
        Luma([luma601(color[0], color[1], color[2])])
    })
}

pub(crate) fn encode_jpeg(image: &DynamicImage, quality: Quality) -> Result<Vec<u8>> {
    let image = encodable(image);
    let image = image.as_ref();
    let mut buffer = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buffer, quality.get());
    encoder
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            ExtendedColorType::from(image.color()),
        )
        .context("JPEG encoding failed")?;
    Ok(buffer)
}

fn encode_png(raw: &[u8], width: u32, height: u32, color: ExtendedColorType) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    PngEncoder::new(&mut buffer)
        .write_image(raw, width, height, color)
        .context("PNG encoding failed")?;
    Ok(buffer)
}

/// Write a palette PNG at the smallest bit depth the palette fits in.
fn encode_png_indexed(quantized: &Quantized) -> Result<Vec<u8>> {
    let palette = quantized.palette.flatten();
    let depth = quantized.palette.bit_depth();
    let packed = pack_indices(
        quantized.indices.as_slice(),
        quantized.width,
        quantized.height,
        depth,
    );

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

/// Pack one-byte palette indices into the sub-byte MSB-first layout the `png` crate
/// writes, padding each scanline to a byte.
fn pack_indices(indices: &[u8], width: u32, height: u32, depth: png::BitDepth) -> Vec<u8> {
    let bits = depth as usize;
    if bits == 8 {
        return indices.to_vec();
    }
    let mask = (1u8 << bits) - 1;
    let mut packed = BitVec::<u8, Msb0>::with_capacity(indices.len() * bits);

    for y in 0..height {
        for x in 0..width {
            let value = indices[(y * width + x) as usize] & mask;
            for shift in (0..bits).rev() {
                packed.push((value >> shift) & 1 == 1);
            }
        }
        // PNG pads every scanline to a byte boundary with zero bits.
        while !packed.len().is_multiple_of(8) {
            packed.push(false);
        }
    }

    packed.into_vec()
}

fn encode_gif(image: &DynamicImage) -> Result<Vec<u8>> {
    // `image`'s GIF encoder rejects L8, and GIF is palette-based anyway.
    let rgb = rgb_view(image);
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

fn encode_webp_lossy(image: &RgbImage, quality: Quality) -> Vec<u8> {
    let encoder = webp::Encoder::from_rgb(image.as_raw(), image.width(), image.height());
    encoder.encode(f32::from(quality.get())).to_vec()
}

fn encode_webp_lossless(image: &RgbImage) -> Vec<u8> {
    let encoder = webp::Encoder::from_rgb(image.as_raw(), image.width(), image.height());
    encoder.encode_lossless().to_vec()
}

/// Encode an image in a given media type at `quality` (used for the `--no-processing`
/// passthrough and light-novel resizes, which have no retained source bytes to copy).
pub(crate) fn encode_dynamic(
    image: &DynamicImage,
    media_type: MediaType,
    quality: Quality,
) -> Result<Vec<u8>> {
    match media_type {
        MediaType::Jpeg => encode_jpeg(image, quality),
        MediaType::Png => match image {
            DynamicImage::ImageLuma8(gray) => encode_png(
                gray.as_raw(),
                gray.width(),
                gray.height(),
                ExtendedColorType::L8,
            ),
            other => {
                let rgb = rgb_view(other);
                encode_png(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
            }
        },
        MediaType::Gif => encode_gif(image),
        MediaType::WebP => Ok(encode_webp_lossy(&rgb_view(image), quality)),
    }
}

/// A borrowed RGB view of `image`, converting only when the pixel type differs.
fn rgb_view(image: &DynamicImage) -> Cow<'_, RgbImage> {
    match image.as_rgb8() {
        Some(rgb) => Cow::Borrowed(rgb),
        None => Cow::Owned(image.to_rgb8()),
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

/// The output file name for a payload, keeping the source directory and adding
/// the `-kcc-<order>` suffix (see docs/architecture.md).
fn output_name(source_name: &str, order: OrderClass, media_type: MediaType) -> String {
    named_page(source_name, media_type, Some(order), None)
}

/// The output file name for a `--no-processing` page: the sanitized name alone.
fn unsuffixed_name(source_name: &str, media_type: MediaType) -> String {
    named_page(source_name, media_type, None, None)
}

/// The output file name for a Kindle Scribe split half (`-above`/`-below`) or an
/// unsplit `-whole` page: `kcc-0001-kcc-x-above.jpg` (KCC's `saveToDir`).
fn split_name(source_name: &str, order: OrderClass, part: &str, media_type: MediaType) -> String {
    named_page(source_name, media_type, Some(order), Some(part))
}

/// Build a page name from a source path, media type, optional order suffix and an
/// optional trailing part (`above`/`below`/`whole`).
fn named_page(
    source_name: &str,
    media_type: MediaType,
    order: Option<OrderClass>,
    part: Option<&str>,
) -> String {
    let (directory, file_name) = match source_name.rsplit_once('/') {
        Some((directory, file_name)) => (Some(directory), file_name),
        None => (None, source_name),
    };
    let stem = match file_name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file_name,
    };
    let mut name = stem.to_string();
    if let Some(order) = order {
        name.push_str(&format!("-kcc-{}", order.suffix()));
    }
    if let Some(part) = part {
        name.push('-');
        name.push_str(part);
    }
    name.push('.');
    name.push_str(media_type.extension());
    match directory {
        Some(directory) => format!("{directory}/{name}"),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::options::Options;
    use crate::units::Size;
    use clap::Parser;

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

    /// A page of a solid colour.
    fn page(width: u32, height: u32, color: [u8; 3]) -> Page {
        Page {
            source_name: "page.png".to_string(),
            rel_path: "page.png".to_string(),
            image: Some(DynamicImage::ImageRgb8(RgbImage::from_pixel(
                width,
                height,
                Rgb(color),
            ))),
            dimensions: Size::new(width, height),
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
    fn normal_page_is_passed_through() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(100, 150));
        let payloads = split_check(image, &options(&[])?, Size::new(1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::Normal]);
        Ok(())
    }

    #[test]
    fn wide_spread_below_bisect_is_split() -> Result<()> {
        // 1.5:1 landscape against a portrait profile sits between the split and
        // bisect thresholds, so it is bisected rather than rotated.
        let image = DynamicImage::ImageRgb8(RgbImage::new(300, 200));
        let payloads = split_check(image, &options(&[])?, Size::new(1072, 1448));
        assert_eq!(
            order_of(&payloads),
            vec![OrderClass::SplitLeft, OrderClass::SplitRight]
        );
        // Both halves are half as wide.
        assert_eq!(payloads[0].image.width(), 150);
        assert_eq!(payloads[1].image.width(), 150);
        Ok(())
    }

    #[test]
    fn wide_spread_at_bisect_is_rotated() -> Result<()> {
        // 2.5:1 exceeds BISECT_THRESHOLD, so the spread only rotates.
        let image = DynamicImage::ImageRgb8(RgbImage::new(500, 200));
        let payloads = split_check(image, &options(&[])?, Size::new(1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::RotateLast]);
        assert!(payloads[0].rotated);
        // Rotated 90°: dimensions swap.
        assert_eq!(payloads[0].image.dimensions(), (200, 500));
        Ok(())
    }

    #[test]
    fn rotate_first_puts_the_rotated_spread_first() -> Result<()> {
        // 2.5:1 exceeds the bisect threshold, so it only rotates.
        let image = DynamicImage::ImageRgb8(RgbImage::new(500, 200));
        let payloads = split_check(image, &options(&["--rotate-first"])?, Size::new(1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::RotateFirst]);
        Ok(())
    }

    #[test]
    fn no_rotate_keeps_the_spread_upright() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(500, 200));
        let payloads = split_check(image, &options(&["--no-rotate"])?, Size::new(1072, 1448));
        assert_eq!(order_of(&payloads), vec![OrderClass::RotateLast]);
        assert!(!payloads[0].rotated);
        assert_eq!(payloads[0].image.dimensions(), (500, 200));
        Ok(())
    }

    #[test]
    fn right_to_left_swaps_the_split_halves() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(300, 200, |x, _| {
            if x < 150 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 0, 0])
            }
        }));
        let ltr = split_check(image.clone(), &options(&[])?, Size::new(1072, 1448));
        let rtl = split_check(image, &options(&["--manga"])?, Size::new(1072, 1448));
        // Left-to-right reads left half first; right-to-left reads right half.
        assert_eq!(ltr[0].image.get_pixel(0, 0)[0], 0);
        assert_eq!(rtl[0].image.get_pixel(0, 0)[0], 255);
        Ok(())
    }

    #[test]
    fn maximize_strips_stacks_the_halves() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(400, 100));
        let payloads = split_check(
            image,
            &options(&["--maximize-strips"])?,
            Size::new(1072, 1448),
        );
        assert_eq!(order_of(&payloads), vec![OrderClass::Normal]);
        assert_eq!(payloads[0].image.dimensions(), (200, 200));
        Ok(())
    }

    #[test]
    fn contains_scales_down_to_fit() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(2000, 1000));
        let contained = contain(&image, Size::new(1000, 1000), Method::Lanczos)?;
        assert_eq!(contained.dimensions(), (1000, 500));
        Ok(())
    }

    #[test]
    fn pad_fills_to_the_exact_profile_size() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(2000, 1000));
        let padded = pad(
            &image,
            Size::new(1000, 1000),
            Method::Lanczos,
            Background::White,
        )?;
        assert_eq!(padded.dimensions(), (1000, 1000));
        // The letterbox is white.
        assert_eq!(padded.to_rgb8().get_pixel(500, 0)[0], 255);
        Ok(())
    }

    #[test]
    fn fit_crops_then_scales() -> Result<()> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
        let fitted = fit(&image, Size::new(500, 1000), Method::Lanczos)?;
        assert_eq!(fitted.dimensions(), (500, 1000));
        Ok(())
    }

    #[test]
    fn grayscale_pages_encode_as_jpeg_by_default() -> Result<()> {
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&[])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].media_type, MediaType::Jpeg);
        assert_eq!(encoded[0].name, "page-kcc-x.jpg");
        assert_eq!(encoded[0].size, Size::new(40, 40));
        Ok(())
    }

    #[test]
    fn force_png_emits_an_indexed_png() -> Result<()> {
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&["-p", "KoE", "--force-png"])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        assert_eq!(encoded[0].media_type, MediaType::Png);
        // Palette PNG signature + IHDR bit depth 4 (16-colour palette).
        assert_eq!(&encoded[0].bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(encoded[0].bytes[24], 4, "IHDR bit depth");
        Ok(())
    }

    #[test]
    fn png_legacy_emits_grayscale_png() -> Result<()> {
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&["-p", "KoE", "--force-png", "--png-legacy"])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        assert_eq!(encoded[0].media_type, MediaType::Png);
        // IHDR colour type 0 (grayscale).
        assert_eq!(encoded[0].bytes[25], 0, "IHDR colour type");
        Ok(())
    }

    #[test]
    fn force_png_on_kindle_emits_gif() -> Result<()> {
        // Kindle output replaces monochrome PNGs with GIFs (KCC's AZW3 path).
        let source = page(40, 40, [10, 10, 10]);
        let options = options(&["--force-png"])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        assert_eq!(encoded[0].media_type, MediaType::Gif);
        assert!(encoded[0].bytes.starts_with(b"GIF"));
        Ok(())
    }

    #[test]
    fn force_color_keeps_a_colour_page_as_jpeg() -> Result<()> {
        let source = page(40, 40, [255, 0, 0]);
        let options = options(&["--force-color", "--force-png"])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        // A colour page stays JPEG unless `--force-png-rgb` is given.
        assert_eq!(encoded[0].media_type, MediaType::Jpeg);
        Ok(())
    }

    #[test]
    fn force_png_rgb_keeps_a_colour_page_as_png() -> Result<()> {
        let source = page(40, 40, [255, 0, 0]);
        let options = options(&[
            "-p",
            "KoE",
            "--force-color",
            "--force-png",
            "--force-png-rgb",
        ])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        assert_eq!(encoded[0].media_type, MediaType::Png);
        Ok(())
    }

    #[test]
    fn no_processing_copies_the_source_bytes() -> Result<()> {
        let mut source = page(10, 10, [1, 2, 3]);
        source.raw = Some(vec![1, 2, 3, 4]);
        let options = options(&["--no-processing"])?;
        let encoded = process_page(&source, &options, options.profile_size())?;
        assert_eq!(encoded[0].bytes, vec![1, 2, 3, 4]);
        assert_eq!(encoded[0].media_type, MediaType::Png);
        assert_eq!(encoded[0].order_class, OrderClass::Normal);
        Ok(())
    }

    #[test]
    fn gamma_correction_darkens_midtones() -> Result<()> {
        let mut image = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 2, Rgb([128, 128, 128])));
        gamma_correct(&mut image, &options(&["--gamma", "2.0"])?, false);
        let value = image.to_rgb8().get_pixel(0, 0)[0];
        // 255 * (128/255)^2 ≈ 64 (KCC rounds to 64).
        assert_eq!(value, 64);
        Ok(())
    }

    #[test]
    fn autocontrast_stretches_a_full_range_image() -> Result<()> {
        // A high-contrast ramp (range 220 > 159) is stretched to [0, 255].
        let mut image = DynamicImage::ImageLuma8(GrayImage::from_fn(256, 1, |x, _| {
            Luma([(10 + x * 220 / 255) as u8])
        }));
        autocontrast_image(&mut image, &options(&[])?, false);
        let gray = to_luma601(&image);
        assert_eq!(gray.get_pixel(0, 0)[0], 0);
        assert_eq!(gray.get_pixel(255, 0)[0], 255);
        Ok(())
    }

    #[test]
    fn autocontrast_leaves_a_low_contrast_image_alone() -> Result<()> {
        // Range 100 < 159: probably intentional, so it is left untouched.
        let mut image = DynamicImage::ImageLuma8(GrayImage::from_fn(16, 1, |x, _| {
            Luma([(x as u8).saturating_add(100)])
        }));
        let before = image.clone();
        autocontrast_image(&mut image, &options(&[])?, false);
        assert_eq!(image.to_luma8(), before.to_luma8());
        Ok(())
    }

    #[test]
    fn autolevel_raises_the_black_point() -> Result<()> {
        // Most dark pixels sit at 30, so everything below 30 is clamped up.
        let mut values = vec![0u8; 40];
        values.extend(std::iter::repeat_n(30u8, 200));
        values.extend(std::iter::repeat_n(200u8, 16));
        let mut image =
            DynamicImage::ImageLuma8(GrayImage::from_raw(1, 256, values).context("1x256 buffer")?);
        autolevel_image(&mut image, false);
        let gray = image.to_luma8();
        assert!(gray.pixels().all(|pixel| pixel[0] >= 30));
        assert_eq!(gray.get_pixel(0, 0)[0], 30);
        Ok(())
    }

    #[test]
    fn indexed_packing_places_the_high_nibble_first() {
        // Two 4-bit indices per byte: 0x1 and 0x2 -> 0x12.
        let packed = pack_indices(&[1, 2], 2, 1, png::BitDepth::Four);
        assert_eq!(packed, vec![0x12]);

        // Three indices per row occupy 12 bits, so the row is padded to two bytes
        // with a zero low nibble and the next row starts on a fresh byte.
        let padded = pack_indices(&[0x1, 0x2, 0x3, 0x4, 0x5, 0x6], 3, 2, png::BitDepth::Four);
        assert_eq!(padded, vec![0x12, 0x30, 0x45, 0x60]);
    }

    #[test]
    fn indexed_packing_handles_one_two_and_eight_bit_depths() {
        // 1-bit: eight pixels per byte, most-significant bit first.
        assert_eq!(
            pack_indices(&[1, 0, 1, 1, 0, 0, 0, 1], 8, 1, png::BitDepth::One),
            vec![0b1011_0001]
        );
        // 2-bit: four pixels per byte, padded when the scanline is not a multiple of four.
        assert_eq!(
            pack_indices(&[0b01, 0b10, 0b11, 0b00], 4, 1, png::BitDepth::Two),
            vec![0b0110_1100]
        );
        assert_eq!(
            pack_indices(&[0b01, 0b10, 0b11], 3, 1, png::BitDepth::Two),
            vec![0b0110_1100]
        );
        // 8-bit indices pass through untouched.
        assert_eq!(
            pack_indices(&[0, 1, 2, 255], 4, 1, png::BitDepth::Eight),
            vec![0, 1, 2, 255]
        );
    }

    #[test]
    fn page_names_add_the_order_and_part_suffixes() {
        assert_eq!(
            output_name(
                "Chapter 1/kcc-0001.png",
                OrderClass::Normal,
                MediaType::Jpeg
            ),
            "Chapter 1/kcc-0001-kcc-x.jpg"
        );
        assert_eq!(
            split_name(
                "kcc-0001.png",
                OrderClass::RotateLast,
                "above",
                MediaType::Jpeg
            ),
            "kcc-0001-kcc-d-above.jpg"
        );
        assert_eq!(
            split_name("kcc-0002.png", OrderClass::Normal, "below", MediaType::Gif),
            "kcc-0002-kcc-x-below.gif"
        );
        assert_eq!(
            split_name("kcc-0003.png", OrderClass::Normal, "whole", MediaType::Png),
            "kcc-0003-kcc-x-whole.png"
        );
    }

    #[test]
    #[ignore = "slow: tall-page resize; run with --run-ignored"]
    fn a_tall_scribe_page_splits_at_1920_into_above_and_below() -> Result<()> {
        // A page larger than the KS profile: it is contain-resized to 2480 tall,
        // then split into a 1920-row top and a 560-row bottom.
        let tall = page(2000, 3000, [10, 10, 10]);
        let options = options(&["-f", "epub", "-p", "KS"])?;
        let size = options.profile_size();
        let encoded = process_page(&tall, &options, size)?;

        assert_eq!(encoded.len(), 2);
        assert_eq!(encoded[0].name, "page-kcc-x-above.jpg");
        assert_eq!(encoded[1].name, "page-kcc-x-below.jpg");
        assert_eq!(encoded[0].size, Size::new(1653, SCRIBE_MAX_DIMENSION));
        assert_eq!(encoded[1].size, Size::new(1653, 560));
        assert!(encoded[0].flags.above && !encoded[0].flags.below);
        assert!(!encoded[1].flags.above && encoded[1].flags.below);
        Ok(())
    }

    #[test]
    fn a_scribe_page_that_fits_is_named_whole() -> Result<()> {
        let small = page(100, 150, [10, 10, 10]);
        let options = options(&["-f", "epub", "-p", "KS"])?;
        let size = options.profile_size();
        let encoded = process_page(&small, &options, size)?;

        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0].name, "page-kcc-x-whole.jpg");
        assert!(!encoded[0].flags.above && !encoded[0].flags.below);
        Ok(())
    }
}
