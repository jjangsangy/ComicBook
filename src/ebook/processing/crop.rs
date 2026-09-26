//! Margin and page-number cropping (AGENTS.md §11.4, §8).
//!
//! Clean-room reimplementation of KCC's `cropMargin` / `cropPageNumber`
//! heuristics. A page is converted to grayscale (optionally inverted for a black
//! background), autocontrasted with a 1 % cutoff, box-blurred with radius 1, and
//! thresholded; the bounding box of the surviving dark pixels becomes the crop
//! rectangle. The page-number variant additionally inspects the bottom window for
//! a small candidate digit group and trims it off.
//!
//! The algorithms are pinned by the unit tests below and by
//! `tests/ebook_crop_tests.rs`, which compares the computed boxes against values
//! produced by KCC itself on committed fixtures.
//!
//! Three KCC quirks are reproduced deliberately because they change the result:
//!
//! * `ignore_pixels_near_edge` bails out when `int(0.02 * n) == int(0.025 * n)`.
//! * `group_close_values` drops the value that opens a new group.
//! * `empty_sections` compares a column section's bounds against the *height*,
//!   even in the vertical direction. That lives in [`super::interpanel`], but the
//!   same "reproduce the reference" rule applies.

use image::{DynamicImage, GrayImage, ImageBuffer, Luma, Pixel};
use imageproc::contrast::ThresholdType;

use crate::ebook::model::Background;
use crate::ebook::processing::color::to_luma601;
use crate::ebook::processing::fill::bounding_box;

/// Height of the window inspected for a page number, as a fraction of the page
/// height.
const WINDOW_H: f64 = 0.025;
/// Maximum accepted page-number width/height, as a fraction of the page size.
const MAX_SHAPE: (f64, f64) = (0.015 * 3.0, 0.02);
/// Minimum accepted page-number width/height, as a fraction of the page size.
const MIN_SHAPE: (f64, f64) = (0.003, 0.006);
/// Maximum tolerated gap between two digits, as a fraction of the page size.
const MAX_DIST: (f64, f64) = (0.01, 0.002);
/// Autocontrast cutoff applied before the threshold, in percent per end.
pub(crate) const CROP_CUTOFF: f64 = 1.0;
/// Crop power used by the inter-panel pass (KCC hard-codes `power = 1`).
pub(crate) const INTERPANEL_POWER: f64 = 1.0;

/// An inclusive index box `(x1, x2, y1, y2)`.
///
/// Unlike Pillow's `(left, upper, right, lower)`, this mirrors the order KCC's
/// page-number search uses internally, so the two dimensions can be compared
/// without transposing.
pub(crate) type IndexBox = (i64, i64, i64, i64);

/// KCC's `threshold_from_power`: the higher the power, the harder it crops.
pub fn threshold_from_power(power: f64) -> f64 {
    240.0 - power * 64.0
}

/// Group sorted, close values together (KCC's `group_close_values`).
///
/// Consecutive values whose distance from the current group's end is within
/// `max_dist` stay in the same group.
pub fn group_close_values(values: &[i64], max_dist: f64) -> Vec<(i64, i64)> {
    let mut groups = Vec::new();

    let mut group_start = -1i64;
    let mut group_end = 0i64;
    for &value in values {
        // The distance is unused while a group is being started, exactly as in
        // the reference (which computes it before branching too).
        let distance = value as f64 - group_end as f64;
        if group_start == -1 {
            group_start = value;
            group_end = value;
        } else if distance <= max_dist {
            group_end = value;
        } else {
            groups.push((group_start, group_end));
            group_start = -1;
            group_end = -1;
        }
    }

    if group_start != -1 {
        groups.push((group_start, group_end));
    }

    groups
}

/// Whether two inclusive boxes overlap once grown by `dx`/`dy` (KCC's
/// `box_intersect`).
fn box_intersect(a: IndexBox, b: IndexBox, dx: f64, dy: f64) -> bool {
    !((b.0 as f64 - dx) > a.1 as f64
        || (b.1 as f64 + dx) < a.0 as f64
        || (b.2 as f64 - dy) > a.3 as f64
        || (b.3 as f64 + dy) < a.2 as f64)
}

/// Merge overlapping boxes, restarting after each merge (KCC's `merge_boxes`).
///
/// The restart is load-bearing: a merge can create a box that overlaps an
/// earlier one, so the scan always resumes from the front.
pub fn merge_boxes(mut boxes: Vec<IndexBox>, dx: f64, dy: f64) -> Vec<IndexBox> {
    let mut j = 0usize;
    while j + 1 < boxes.len() {
        let first = boxes[j];
        let mut merged: Option<IndexBox> = None;
        let mut other: Vec<IndexBox> = Vec::new();

        for &candidate in &boxes[j + 1..] {
            if box_intersect(first, candidate, dx, dy) {
                merged = Some(match merged {
                    None => (
                        first.0.min(candidate.0),
                        first.1.max(candidate.1),
                        first.2.min(candidate.2),
                        first.3.max(candidate.3),
                    ),
                    Some(accumulated) => (
                        accumulated.0.min(candidate.0),
                        accumulated.1.max(candidate.1),
                        accumulated.2.min(candidate.2),
                        accumulated.3.max(candidate.3),
                    ),
                });
            } else {
                other.push(candidate);
            }
        }

        if let Some(merged) = merged {
            let mut rebuilt: Vec<IndexBox> = boxes[..j].to_vec();
            rebuilt.extend(other);
            rebuilt.push(merged);
            boxes = rebuilt;
            j = 0;
        } else {
            j += 1;
        }
    }

    boxes
}

// --- low-level image helpers ----------------------------------------------------

/// Any decoded image as an 8-bit grayscale image, matching Pillow's
/// `ImageOps.grayscale` (which is a no-op on an already-`L` image).
pub(crate) fn to_gray(image: &DynamicImage) -> GrayImage {
    match image {
        DynamicImage::ImageLuma8(gray) => gray.clone(),
        _ => to_luma601(image),
    }
}

/// `255 - value` for every pixel.
pub(crate) fn invert(image: &GrayImage) -> GrayImage {
    imageproc::map::map_pixels(image, |pixel| Luma([255 - pixel[0]]))
}

/// Drop `cut` samples from each end of a 256-bin histogram (Pillow's
/// `autocontrast` trim; shared with `color.rs`).
pub(crate) fn trim_histogram_ends(histogram: &mut [u64; 256], cut: u64) {
    let mut remaining = cut;
    for count in histogram.iter_mut() {
        if remaining > *count {
            remaining -= *count;
            *count = 0;
        } else {
            *count -= remaining;
            remaining = 0;
        }
        if remaining == 0 {
            break;
        }
    }

    let mut remaining = cut;
    for count in histogram.iter_mut().rev() {
        if remaining > *count {
            remaining -= *count;
            *count = 0;
        } else {
            *count -= remaining;
            remaining = 0;
        }
        if remaining == 0 {
            break;
        }
    }
}

/// Pillow's `ImageOps.autocontrast(image, cutoff)`: drop `cutoff` percent of the
/// samples from each end of the histogram, then stretch the remainder to
/// `[0, 255]` with a truncated (not rounded) linear map.
pub(crate) fn autocontrast_cutoff(image: &GrayImage, cutoff: f64) -> GrayImage {
    let mut histogram: [u64; 256] = imageproc::stats::histogram(image).channels[0].map(u64::from);

    if cutoff != 0.0 {
        let total: u64 = histogram.iter().sum();
        let cut = ((total as f64 * cutoff) / 100.0).floor() as u64;
        trim_histogram_ends(&mut histogram, cut);
    }

    let low = histogram.iter().position(|&count| count != 0);
    let high = histogram.iter().rposition(|&count| count != 0);

    let lut: [u8; 256] = match (low, high) {
        (Some(low), Some(high)) if high > low => {
            let scale = 255.0 / (high as f64 - low as f64);
            let offset = -(low as f64) * scale;
            std::array::from_fn(|index| {
                let value = (index as f64 * scale + offset) as i64;
                value.clamp(0, 255) as u8
            })
        }
        // A flat or empty histogram maps to itself.
        _ => std::array::from_fn(|index| index as u8),
    };

    imageproc::map::map_pixels(image, |pixel| Luma([lut[pixel[0] as usize]]))
}

/// Pillow's `ImageFilter.BoxBlur(1)`: a horizontal then vertical 3-tap box
/// average, each rounded, with edge pixels replicated.
///
/// The two passes are *not* equivalent to a single 3x3 average because each pass
/// rounds; the reference rounds per pass, so we do too. Kept bespoke (§5.3).
pub(crate) fn box_blur_1(image: &GrayImage) -> GrayImage {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return image.clone();
    }

    let mut horizontal = GrayImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let left = x.saturating_sub(1);
            let right = (x + 1).min(width - 1);
            let sum = u32::from(image.get_pixel(left, y)[0])
                + u32::from(image.get_pixel(x, y)[0])
                + u32::from(image.get_pixel(right, y)[0]);
            horizontal.put_pixel(x, y, Luma([((sum + 1) / 3) as u8]));
        }
    }

    let mut vertical = GrayImage::new(width, height);
    for y in 0..height {
        let top = y.saturating_sub(1);
        let bottom = (y + 1).min(height - 1);
        for x in 0..width {
            let sum = u32::from(horizontal.get_pixel(x, top)[0])
                + u32::from(horizontal.get_pixel(x, y)[0])
                + u32::from(horizontal.get_pixel(x, bottom)[0]);
            vertical.put_pixel(x, y, Luma([((sum + 1) / 3) as u8]));
        }
    }

    vertical
}

/// `255` where `value <= threshold`, else `0` (KCC's `point` threshold).
pub(crate) fn binarize(image: &GrayImage, threshold: f64) -> GrayImage {
    // A negative (or NaN) threshold matches no pixel; the `u8` cast would otherwise
    // saturate a large `--cropping-power` threshold to 0 and match every black pixel.
    if threshold.is_nan() || threshold < 0.0 {
        return GrayImage::new(image.width(), image.height());
    }
    imageproc::contrast::threshold(
        image,
        threshold.min(255.0) as u8,
        ThresholdType::BinaryInverted,
    )
}

/// The bounding box of the non-zero pixels, as Pillow's `getbbox` returns it
/// (`(left, upper, right, lower)`, `right`/`lower` exclusive), or `None` when the
/// image is entirely zero.
fn bbox_nonzero(image: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    bounding_box(image, |value| value != 0)
}

/// Pillow's `Image.crop`: the requested rectangle, with out-of-bounds pixels
/// filled with zero.
///
/// The copy runs on the typed buffer (via [`image::imageops::replace`], whose
/// negative-offset clipping matches Pillow's) so grayscale pages keep their pixel
/// type rather than being re-lumaed through `DynamicImage`'s `Rgba<u8>` view.
fn crop_padded_buf<P>(
    source: &ImageBuffer<P, Vec<P::Subpixel>>,
    left: i64,
    upper: i64,
    right: i64,
    lower: i64,
) -> ImageBuffer<P, Vec<P::Subpixel>>
where
    P: Pixel + 'static,
{
    let width = (right - left).max(0) as u32;
    let height = (lower - upper).max(0) as u32;
    let mut cropped = ImageBuffer::new(width, height);
    image::imageops::replace(&mut cropped, source, -left, -upper);
    cropped
}

/// [`crop_padded_buf`] for a concrete [`DynamicImage`], preserving its pixel type.
fn crop_padded(
    image: &DynamicImage,
    left: i64,
    upper: i64,
    right: i64,
    lower: i64,
) -> DynamicImage {
    macro_rules! arm {
        ($variant:ident, $buffer:expr) => {
            DynamicImage::$variant(crop_padded_buf($buffer, left, upper, right, lower))
        };
    }

    match image {
        DynamicImage::ImageLuma8(buffer) => arm!(ImageLuma8, buffer),
        DynamicImage::ImageLumaA8(buffer) => arm!(ImageLumaA8, buffer),
        DynamicImage::ImageRgb8(buffer) => arm!(ImageRgb8, buffer),
        DynamicImage::ImageRgba8(buffer) => arm!(ImageRgba8, buffer),
        DynamicImage::ImageLuma16(buffer) => arm!(ImageLuma16, buffer),
        DynamicImage::ImageLumaA16(buffer) => arm!(ImageLumaA16, buffer),
        DynamicImage::ImageRgb16(buffer) => arm!(ImageRgb16, buffer),
        DynamicImage::ImageRgba16(buffer) => arm!(ImageRgba16, buffer),
        other => {
            DynamicImage::ImageRgb8(crop_padded_buf(&other.to_rgb8(), left, upper, right, lower))
        }
    }
}

/// The non-zero pixel count and total area of `[left, right) x [upper, lower)`,
/// clipped to the image.
fn count_nonzero(image: &GrayImage, left: i64, upper: i64, right: i64, lower: i64) -> (u64, u64) {
    let width = image.width() as i64;
    let height = image.height() as i64;
    let left = left.max(0);
    let upper = upper.max(0);
    let right = right.min(width);
    let lower = lower.min(height);

    let mut count = 0u64;
    let mut area = 0u64;
    for y in upper..lower {
        for x in left..right {
            area += 1;
            if image.get_pixel(x as u32, y as u32)[0] != 0 {
                count += 1;
            }
        }
    }

    (count, area)
}

/// Set every pixel of `[left, right) x [upper, lower)` to `value`, clipped to the
/// image.
fn fill_rect(image: &mut GrayImage, left: i64, upper: i64, right: i64, lower: i64, value: u8) {
    let width = image.width() as i64;
    let height = image.height() as i64;
    let left = left.clamp(0, width);
    let upper = upper.clamp(0, height);
    let right = right.clamp(0, width);
    let lower = lower.clamp(0, height);
    if left >= right || upper >= lower {
        return;
    }

    let rect = imageproc::rect::Rect::at(left as i32, upper as i32)
        .of_size((right - left) as u32, (lower - upper) as u32);
    imageproc::drawing::draw_filled_rect_mut(image, rect, Luma([value]));
}

/// Suppress near-edge noise so scan artefacts cannot anchor the crop box.
///
/// For each of the four sides, the thin inner strip just inside the border is
/// examined: if it is *nearly* empty, it is cleared, and if the outer edge strip
/// then contains any ink it is cleared too.
fn ignore_pixels_near_edge(bw: &mut GrayImage) {
    let width = bw.width() as i64;
    let height = bw.height() as i64;

    // Too small for the strips to differ: nothing to do.
    if (0.02 * height as f64) as i64 == (0.025 * height as f64) as i64 {
        return;
    }
    if (0.02 * width as f64) as i64 == (0.025 * width as f64) as i64 {
        return;
    }

    let edge_boxes = [
        (0, 0, width, (0.02 * height as f64) as i64),
        (0, (0.98 * height as f64) as i64, width, height),
        (0, 0, (0.02 * width as f64) as i64, height),
        ((0.98 * width as f64) as i64, 0, width, height),
    ];
    let inner_boxes = [
        (
            (0.02 * width as f64) as i64,
            (0.02 * height as f64) as i64,
            (0.98 * width as f64) as i64,
            (0.025 * height as f64) as i64,
        ),
        (
            (0.02 * width as f64) as i64,
            (0.975 * height as f64) as i64,
            (0.98 * width as f64) as i64,
            (0.98 * height as f64) as i64,
        ),
        (
            (0.02 * width as f64) as i64,
            (0.02 * height as f64) as i64,
            (0.025 * width as f64) as i64,
            (0.98 * height as f64) as i64,
        ),
        (
            (0.975 * width as f64) as i64,
            (0.02 * height as f64) as i64,
            (0.98 * width as f64) as i64,
            (0.98 * height as f64) as i64,
        ),
    ];

    for (edge_box, inner_box) in edge_boxes.iter().zip(inner_boxes.iter()) {
        let (inner_left, inner_upper, inner_right, inner_lower) = *inner_box;
        let (count, area) = count_nonzero(bw, inner_left, inner_upper, inner_right, inner_lower);
        let imperfections = if area > 0 {
            count as f64 / area as f64
        } else {
            0.0
        };

        if imperfections > 0.0 && imperfections < 0.001 {
            fill_rect(bw, inner_left, inner_upper, inner_right, inner_lower, 0);
        }
        if imperfections < 0.001 {
            let (edge_left, edge_upper, edge_right, edge_lower) = *edge_box;
            if count_nonzero(bw, edge_left, edge_upper, edge_right, edge_lower).0 != 0 {
                fill_rect(bw, edge_left, edge_upper, edge_right, edge_lower, 0);
            }
        }
    }
}

/// Pillow's `Image.crop` with float coordinates, which rounds (half-to-even)
/// before cropping.
///
/// `f64::round_ties_even` is exactly Pillow's `round` (banker's rounding); see
/// `pillow_round_is_half_to_even`.
pub(crate) fn crop_rounded(
    image: &DynamicImage,
    left: f64,
    upper: f64,
    right: f64,
    lower: f64,
) -> DynamicImage {
    crop_padded(
        image,
        left.round_ties_even() as i64,
        upper.round_ties_even() as i64,
        right.round_ties_even() as i64,
        lower.round_ties_even() as i64,
    )
}

// --- cropping ------------------------------------------------------------------

/// The grayscale preparation shared by every crop variant.
fn prepared_gray(image: &DynamicImage, background: Background) -> GrayImage {
    let gray = to_gray(image);
    let gray = if background == Background::White {
        gray
    } else {
        invert(&gray)
    };
    box_blur_1(&autocontrast_cutoff(&gray, CROP_CUTOFF))
}

/// The tightest margin crop box, or `None` when the page has no detectable ink.
pub fn margin_bbox(
    image: &DynamicImage,
    power: f64,
    background: Background,
) -> Option<(u32, u32, u32, u32)> {
    let gray = prepared_gray(image, background);
    let mut bw = binarize(&gray, threshold_from_power(power));
    ignore_pixels_near_edge(&mut bw);
    bbox_nonzero(&bw)
}

/// The crop box that additionally trims a detected bottom page number.
pub fn page_number_bbox(
    image: &DynamicImage,
    power: f64,
    background: Background,
) -> Option<(u32, u32, u32, u32)> {
    let gray = prepared_gray(image, background);
    let (image_width, image_height) = gray.dimensions();
    let threshold = threshold_from_power(power);
    let mut bw = binarize(&gray, threshold);
    ignore_pixels_near_edge(&mut bw);

    let (left, _top, right, bottom) = bbox_nonzero(&bw)?;

    let window_height = (f64::from(image_height) * WINDOW_H) as i64;
    let part_width = right as i64 - left as i64;
    if part_width <= 0 || window_height <= 0 {
        return None;
    }

    // The band of rows just above the last inked row is where a page number
    // would sit.
    let part_top = bottom as i64 - window_height;
    let part = crop_padded_buf(&gray, left as i64, part_top, right as i64, bottom as i64);
    let (part_width, part_height) = part.dimensions();

    let max_dist_x = f64::from(image_width) * MAX_DIST.0;
    let mut groups: Vec<IndexBox> = Vec::new();
    for y in 0..part_height {
        let indices: Vec<i64> = (0..part_width)
            .filter(|&x| f64::from(part.get_pixel(x, y)[0]) <= threshold)
            .map(i64::from)
            .collect();
        for (start, end) in group_close_values(&indices, max_dist_x) {
            groups.push((start, end, i64::from(y), i64::from(y)));
        }
    }

    let boxes = merge_boxes(groups, max_dist_x, f64::from(image_height) * MAX_DIST.1);
    let min_width = f64::from(image_width) * MIN_SHAPE.0;
    let min_height = f64::from(image_height) * MIN_SHAPE.1;
    let boxes: Vec<IndexBox> = boxes
        .into_iter()
        .filter(|b| (b.1 - b.0) as f64 >= min_width && (b.3 - b.2) as f64 >= min_height)
        .collect();

    // The lowest detected object anchors the search; anything sharing its band
    // suggests the "page number" is really artwork and must not be cropped.
    let lowest_start = boxes
        .iter()
        .filter(|b| b.3 == window_height - 1)
        .map(|b| b.2)
        .min()
        .unwrap_or(0);
    let in_band: Vec<IndexBox> = boxes
        .iter()
        .copied()
        .filter(|b| b.3 >= lowest_start)
        .collect();

    let max_shape = (
        f64::from(image_width) * MAX_SHAPE.0,
        (f64::from(image_height) * MAX_SHAPE.1).max(3.0),
    );
    let should_force_crop = in_band.len() == 1
        && (in_band[0].1 - in_band[0].0) as f64 <= max_shape.0
        && (in_band[0].3 - in_band[0].2) as f64 <= max_shape.1;

    let bottom = if should_force_crop {
        bottom as i64 - (window_height - in_band[0].2 + 1)
    } else {
        image_height as i64
    };

    // Cropping to the full height is a no-op, so skip the copy it would cost.
    if bottom >= image_height as i64 {
        bbox_nonzero(&bw)
    } else {
        bbox_nonzero(&crop_padded_buf(&bw, 0, 0, image_width as i64, bottom))
    }
}

/// Cap a crop box at 10 % per side (KCC's `cropMargin`/`cropPageNumber`).
pub fn clamp_bbox(bbox: (u32, u32, u32, u32), width: u32, height: u32) -> (f64, f64, f64, f64) {
    let (width, height) = (f64::from(width), f64::from(height));
    let (left, upper, right, lower) = bbox;
    (
        (0.1 * width).min(f64::from(left)),
        (0.1 * height).min(f64::from(upper)),
        (0.9 * width).max(f64::from(right)),
        (0.9 * height).max(f64::from(lower)),
    )
}

/// KCC's `maybeCrop`: apply `--preserve-margin`, reject crops below
/// `--cropping-minimum`, then crop.
fn maybe_crop(
    image: &mut DynamicImage,
    bbox: (f64, f64, f64, f64),
    minimum: f64,
    preserve_margin: u32,
) {
    let width = f64::from(image.width());
    let height = f64::from(image.height());
    let (mut left, mut upper, mut right, mut lower) = bbox;

    if preserve_margin != 0 {
        let ratio = 1.0 - f64::from(preserve_margin) / 100.0;
        left *= ratio;
        upper *= ratio;
        right += (width - right) * (1.0 - ratio);
        lower += (height - lower) * (1.0 - ratio);
    }

    let box_area = (right - left) * (lower - upper);
    let image_area = width * height;
    if image_area <= 0.0 || box_area / image_area < minimum {
        return;
    }

    *image = crop_rounded(image, left, upper, right, lower);
}

/// Trim the page margins (KCC's `cropMargin`, `--cropping 1`).
pub fn crop_margin(
    image: &mut DynamicImage,
    power: f64,
    minimum: f64,
    preserve_margin: u32,
    background: Background,
) {
    if let Some(bbox) = margin_bbox(image, power, background) {
        maybe_crop(
            image,
            clamp_bbox(bbox, image.width(), image.height()),
            minimum,
            preserve_margin,
        );
    }
}

/// Trim the margins and the bottom page number (KCC's `cropPageNumber`,
/// `--cropping 2`).
pub fn crop_page_number(
    image: &mut DynamicImage,
    power: f64,
    minimum: f64,
    preserve_margin: u32,
    background: Background,
) {
    if let Some(bbox) = page_number_bbox(image, power, background) {
        maybe_crop(
            image,
            clamp_bbox(bbox, image.width(), image.height()),
            minimum,
            preserve_margin,
        );
    }
}

// Tests for the cropping primitives and the KCC-parity boxes are compared
// against committed fixtures in `tests/ebook_crop_tests.rs`.
#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, GrayImage, Luma, Rgb, RgbImage};

    fn gray(width: u32, height: u32, value: u8) -> GrayImage {
        GrayImage::from_pixel(width, height, Luma([value]))
    }

    /// A white page with a black rectangle.
    fn framed(width: u32, height: u32, rect: (u32, u32, u32, u32)) -> DynamicImage {
        let (rx0, ry0, rx1, ry1) = rect;
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            if (rx0..rx1).contains(&x) && (ry0..ry1).contains(&y) {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        }))
    }

    #[test]
    fn threshold_scales_with_power() {
        assert_eq!(threshold_from_power(0.0), 240.0);
        assert_eq!(threshold_from_power(1.0), 176.0);
        assert_eq!(threshold_from_power(2.0), 112.0);
        assert_eq!(threshold_from_power(3.0), 48.0);
    }

    #[test]
    fn grouping_splits_on_distance() {
        assert_eq!(group_close_values(&[], 1.0), vec![]);
        assert_eq!(group_close_values(&[5], 1.0), vec![(5, 5)]);
        // The value that opens a new group is dropped, not carried over: the
        // reference resets the group bounds to -1 and never re-adds the value.
        assert_eq!(
            group_close_values(&[1, 2, 3, 10, 11], 1.0),
            vec![(1, 3), (11, 11)]
        );
        assert_eq!(group_close_values(&[1, 2, 8, 9], 1.0), vec![(1, 2), (9, 9)]);
    }

    #[test]
    fn merging_joins_touching_boxes() {
        // Two unit boxes on consecutive rows, within the y tolerance.
        let merged = merge_boxes(vec![(0, 4, 0, 0), (0, 4, 1, 1)], 0.0, 1.0);
        assert_eq!(merged, vec![(0, 4, 0, 1)]);

        // A y gap larger than the tolerance keeps them apart.
        let separate = merge_boxes(vec![(0, 4, 0, 0), (0, 4, 5, 5)], 0.0, 1.0);
        assert_eq!(separate, vec![(0, 4, 0, 0), (0, 4, 5, 5)]);
    }

    #[test]
    fn blur_spreads_an_impulse_with_rounding() {
        // Pillow's BoxBlur(1) turns a lone 255 into a 3x3 block of 28s.
        let mut image = gray(7, 7, 0);
        image.put_pixel(3, 3, Luma([255]));
        let blurred = box_blur_1(&image);
        for y in 2..=4 {
            for x in 2..=4 {
                assert_eq!(blurred.get_pixel(x, y)[0], 28, "at {x},{y}");
            }
        }
        assert_eq!(blurred.get_pixel(0, 0)[0], 0);
    }

    #[test]
    fn blur_replicates_the_edges() {
        // A full-height left edge column blurs to 170/85, not 85/85.
        let mut image = gray(5, 5, 0);
        for y in 0..5 {
            image.put_pixel(0, y, Luma([255]));
        }
        let blurred = box_blur_1(&image);
        for y in 0..5 {
            assert_eq!(blurred.get_pixel(0, y)[0], 170);
            assert_eq!(blurred.get_pixel(1, y)[0], 85);
            assert_eq!(blurred.get_pixel(2, y)[0], 0);
        }
    }

    #[test]
    fn autocontrast_with_cutoff_stretches_the_surviving_range() {
        // The lone 1 is dropped by the 1 % cutoff, leaving 200..201 to stretch.
        let mut values = vec![1u8];
        values.extend(std::iter::repeat_n(200u8, 1000));
        values.extend(std::iter::repeat_n(201u8, 1000));
        let image = GrayImage::from_raw(1, values.len() as u32, values).unwrap();

        let stretched = autocontrast_cutoff(&image, 1.0);
        assert_eq!(stretched.get_pixel(0, 0)[0], 0, "the 1 is clamped to black");
        assert_eq!(stretched.get_pixel(0, 1)[0], 0, "200 becomes black");
        assert_eq!(
            stretched.get_pixel(0, image.height() - 1)[0],
            255,
            "201 becomes white"
        );
    }

    #[test]
    fn autocontrast_leaves_a_flat_image_alone() {
        let image = gray(4, 4, 123);
        let stretched = autocontrast_cutoff(&image, 1.0);
        assert!(stretched.pixels().all(|pixel| pixel[0] == 123));
    }

    #[test]
    fn trim_histogram_ends_drops_both_ends_and_stops_when_empty() {
        // 100 samples: one at each extreme, the rest in the middle. A cut of 1
        // clears both outlier bins and leaves the interior alone.
        let mut histogram = [0u64; 256];
        histogram[0] = 1;
        histogram[10] = 98;
        histogram[255] = 1;
        trim_histogram_ends(&mut histogram, 1);
        assert_eq!(histogram[0], 0);
        assert_eq!(histogram[255], 0);
        assert_eq!(histogram[10], 98, "interior bins are untouched");

        // A cut larger than the total simply empties the histogram.
        let mut small = [0u64; 256];
        small[100] = 5;
        trim_histogram_ends(&mut small, 999);
        assert!(small.iter().all(|&count| count == 0));
    }

    #[test]
    fn margin_bbox_traces_the_ink() {
        let page = framed(200, 300, (20, 30, 180, 270));
        // The 1px blur widens the box by one pixel on each side.
        assert_eq!(
            margin_bbox(&page, 1.0, Background::White),
            Some((19, 29, 181, 271))
        );
    }

    #[test]
    fn margin_crop_removes_the_border() {
        let mut page = framed(200, 300, (20, 30, 180, 270));
        crop_margin(&mut page, 1.0, 0.0, 0, Background::White);
        assert_eq!(page.dimensions(), (162, 242));
    }

    #[test]
    fn a_blank_page_is_not_cropped() {
        let mut page = DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 300, Rgb([255; 3])));
        crop_margin(&mut page, 1.0, 0.0, 0, Background::White);
        assert_eq!(page.dimensions(), (200, 300));
    }

    #[test]
    fn minimum_area_ratio_vetoes_the_crop() {
        let mut page = framed(200, 300, (20, 30, 180, 270));
        // The crop keeps ~87 % of the page, so a 95 % minimum rejects it.
        crop_margin(&mut page, 1.0, 0.95, 0, Background::White);
        assert_eq!(page.dimensions(), (200, 300));
    }

    #[test]
    fn page_number_crop_trims_the_digit() {
        // A tall page so the search window is wide enough to merge rows.
        let mut page = DynamicImage::ImageRgb8(RgbImage::from_fn(800, 1200, |x, y| {
            let artwork = (80..720).contains(&x) && (100..1100).contains(&y);
            let number = (380..404).contains(&x) && (1150..1170).contains(&y);
            if artwork || number {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        }));

        // Margin cropping would keep the number; page-number cropping drops it.
        assert_eq!(
            margin_bbox(&page, 1.0, Background::White),
            Some((79, 99, 721, 1171))
        );
        assert_eq!(
            page_number_bbox(&page, 1.0, Background::White),
            Some((79, 99, 721, 1101))
        );

        crop_page_number(&mut page, 1.0, 0.0, 0, Background::White);
        assert_eq!(page.dimensions(), (642, 1002));
    }

    #[test]
    fn page_number_crop_works_on_a_black_background() {
        let mut page = DynamicImage::ImageRgb8(RgbImage::from_fn(800, 1200, |x, y| {
            let artwork = (80..720).contains(&x) && (100..1100).contains(&y);
            let number = (380..404).contains(&x) && (1150..1170).contains(&y);
            if artwork || number {
                Rgb([255, 255, 255])
            } else {
                Rgb([0, 0, 0])
            }
        }));

        assert_eq!(
            page_number_bbox(&page, 1.0, Background::Black),
            Some((79, 99, 721, 1101))
        );
        crop_page_number(&mut page, 1.0, 0.0, 0, Background::Black);
        assert_eq!(page.dimensions(), (642, 1002));
    }

    #[test]
    fn crop_is_capped_at_ten_percent_per_side() {
        // Ink only in the very centre: the clamp keeps 10 % borders.
        let page = framed(200, 300, (90, 140, 110, 160));
        let bbox = margin_bbox(&page, 1.0, Background::White).unwrap();
        assert_eq!(clamp_bbox(bbox, 200, 300), (20.0, 30.0, 180.0, 270.0));
    }

    #[test]
    fn preserve_margin_pulls_the_box_back_out() {
        let mut page = framed(200, 300, (20, 30, 180, 270));
        crop_margin(&mut page, 1.0, 0.0, 0, Background::White);
        assert_eq!(page.dimensions(), (162, 242));

        let mut preserved = framed(200, 300, (20, 30, 180, 270));
        // 10 % preserve: the box grows back towards the original edges.
        crop_margin(&mut preserved, 1.0, 0.0, 10, Background::White);
        assert!(preserved.width() > 162 && preserved.width() < 200);
        assert!(preserved.height() > 242 && preserved.height() < 300);
    }

    #[test]
    fn pillow_round_is_half_to_even() {
        // Pillow's `Image.crop` rounds float coordinates with `round`, which is
        // banker's rounding; `f64::round_ties_even` is the same function.
        let round = |value: f64| value.round_ties_even() as i64;
        assert_eq!(round(0.5), 0);
        assert_eq!(round(1.5), 2);
        assert_eq!(round(2.5), 2);
        assert_eq!(round(3.5), 4);
        assert_eq!(round(-0.5), 0);
        assert_eq!(round(2.4), 2);
        assert_eq!(round(2.6), 3);
    }

    #[test]
    fn binarize_is_white_at_or_below_the_threshold() {
        let mut image = gray(4, 1, 100);
        for (x, value) in [0u8, 128, 129, 255].into_iter().enumerate() {
            image.put_pixel(x as u32, 0, Luma([value]));
        }
        let bw = binarize(&image, 128.0);
        let output: Vec<u8> = (0..4).map(|x| bw.get_pixel(x, 0)[0]).collect();
        assert_eq!(output, vec![255, 255, 0, 0]);

        // A negative (or NaN) threshold matches nothing; above 255 matches everything.
        let blank = gray(4, 4, 0);
        assert!(binarize(&blank, -400.0).pixels().all(|pixel| pixel[0] == 0));
        assert!(binarize(&blank, 300.0)
            .pixels()
            .all(|pixel| pixel[0] == 255));
    }

    #[test]
    fn fill_rect_clips_to_the_image_and_ignores_empty_rects() {
        let mut image = gray(4, 4, 0);
        fill_rect(&mut image, 1, 1, 3, 3, 200);
        assert_eq!(image.get_pixel(2, 2)[0], 200);
        assert_eq!(image.get_pixel(0, 0)[0], 0);

        // Out-of-bounds coordinates are clamped, not wrapped; an inverted rect is a no-op.
        fill_rect(&mut image, -2, -2, 2, 2, 100);
        fill_rect(&mut image, 3, 3, 1, 1, 7);
        assert_eq!(image.get_pixel(0, 0)[0], 100);
        assert_eq!(image.get_pixel(2, 2)[0], 200);
        assert_eq!(image.get_pixel(3, 3)[0], 0);
    }

    #[test]
    fn out_of_bounds_crops_are_zero_filled() {
        let image = DynamicImage::ImageLuma8(GrayImage::from_raw(2, 2, vec![1, 2, 3, 4]).unwrap());
        let cropped = crop_padded(&image, 0, -1, 2, 1);
        assert_eq!(cropped.dimensions(), (2, 2));
        let gray = cropped.to_luma8();
        assert_eq!(gray.get_pixel(0, 0)[0], 0);
        assert_eq!(gray.get_pixel(0, 1)[0], 1);
    }
}
