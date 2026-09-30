//! Portable SIMD kernels for the per-pixel hotspots of the image pipeline.
//!
//! `imageproc`'s map/stats/contrast helpers are scalar and, worse, allocate a
//! `Vec` per pixel while mapping (see docs/dependencies.md), which dominates a
//! conversion's profile. These kernels replace the pieces that matter with
//! `wide`-based SIMD: 16-byte min/max, threshold, inversion and row/column
//! reduction, plus an exact integer 3-tap box blur.
//!
//! Every kernel here performs integer arithmetic that is bit-identical to the
//! scalar reference it replaces; nothing here reassociates the pinned Rec.601
//! float weighting (see [`super::color::luma601`]), so the emitted pages do not
//! change. `wide` lowers to NEON on aarch64, SSE2 on x86_64 and a portable
//! fallback elsewhere, so the same source builds on every target.

use image::{DynamicImage, GrayImage};
use wide::{i16x8, u16x8, u8x16};

use crate::ebook::processing::color::luma601;
use crate::units::{BBox, Range};

/// A 16-byte SIMD block loaded from `src` at `at`.
#[inline(always)]
fn load16(src: &[u8], at: usize) -> u8x16 {
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&src[at..at + 16]);
    u8x16::from(bytes)
}

/// Store a 16-byte SIMD block into `dst` at `at`.
#[inline(always)]
fn store16(dst: &mut [u8], at: usize, value: u8x16) {
    dst[at..at + 16].copy_from_slice(&value.to_array());
}

// --- statistics -----------------------------------------------------------------

/// The minimum and maximum samples of a byte slice, or `None` when it is empty.
///
/// Replaces `imageproc::stats::min_max` for the single-channel case.
pub(crate) fn min_max(data: &[u8]) -> Option<Range> {
    if data.is_empty() {
        return None;
    }
    let mut min = u8x16::splat(u8::MAX);
    let mut max = u8x16::splat(0);
    let (chunks, tail) = data.as_chunks::<16>();
    for chunk in chunks {
        let block = u8x16::from(*chunk);
        min = min.min(block);
        max = max.max(block);
    }
    let mut lo = min.reduce_min();
    let mut hi = max.reduce_max();
    for &value in tail {
        lo = lo.min(value);
        hi = hi.max(value);
    }
    Some(Range::new(lo, hi))
}

/// The Rec.601 luma minimum and maximum of an image, without materialising a
/// grayscale copy when the source is already RGB or `L8`.
///
/// This is `min_max(to_luma601(image))` with the same per-pixel weighting, so
/// the detected contrast range is unchanged.
pub(crate) fn luma_min_max(image: &DynamicImage) -> Option<Range> {
    if let Some(gray) = image.as_luma8() {
        return min_max(gray.as_raw());
    }
    if let Some(rgb) = image.as_rgb8() {
        let raw = rgb.as_raw();
        if raw.is_empty() {
            return None;
        }
        let mut lo = u8::MAX;
        let mut hi = 0u8;
        for pixel in raw.as_chunks::<3>().0 {
            let value = luma601(pixel[0], pixel[1], pixel[2]);
            lo = lo.min(value);
            hi = hi.max(value);
        }
        return Some(Range::new(lo, hi));
    }
    luma_min_max(&DynamicImage::ImageRgb8(image.to_rgb8()))
}

// --- point operations -----------------------------------------------------------

/// `255 - value` for a whole buffer, in place.
pub(crate) fn invert_in_place(data: &mut [u8]) {
    let white = u8x16::splat(255);
    let full = data.len() / 16 * 16;
    let mut index = 0;
    while index < full {
        store16(data, index, white - load16(data, index));
        index += 16;
    }
    while index < data.len() {
        data[index] = 255 - data[index];
        index += 1;
    }
}

/// Which side of the threshold becomes white — the two modes of
/// `imageproc::contrast::threshold` (`Binary` / `BinaryInverted`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThresholdKind {
    /// `255` where `value > threshold` (was `inverted == false`).
    Above,
    /// `255` where `value <= threshold` (was `inverted == true`).
    Below,
}

/// Apply a binary threshold in place.
///
/// The polarity is hoisted out of the loops by monomorphising on `INVERTED`, so
/// each instantiation is the hand-written loop for its mode and nothing indirect
/// enters the 16-lane body (see docs/architecture.md).
pub(crate) fn threshold_in_place(data: &mut [u8], threshold: u8, kind: ThresholdKind) {
    match kind {
        ThresholdKind::Above => threshold_mono::<false>(data, threshold),
        ThresholdKind::Below => threshold_mono::<true>(data, threshold),
    }
}

/// The `const`-generic threshold core: `255` where `value <= threshold` when
/// `INVERTED`, else `255` where `value > threshold`.
fn threshold_mono<const INVERTED: bool>(data: &mut [u8], threshold: u8) {
    let t = u8x16::splat(threshold);
    let full = data.len() / 16 * 16;
    let mut index = 0;
    while index < full {
        let block = load16(data, index);
        let mask = if INVERTED {
            block.simd_le(t)
        } else {
            block.simd_gt(t)
        };
        store16(data, index, mask);
        index += 16;
    }
    while index < data.len() {
        let hot = if INVERTED {
            data[index] <= threshold
        } else {
            data[index] > threshold
        };
        data[index] = if hot { 255 } else { 0 };
        index += 1;
    }
}

/// Pillow's `ImageOps.autocontrast` stretch LUT for `output_min = 0`,
/// `output_max = 255` (the mapping `imageproc::contrast::stretch_contrast`
/// computes per pixel, precomputed once).
///
/// `min < max` is required by the reference; callers guard it.
pub(crate) fn stretch_contrast_lut(range: Range) -> [u8; 256] {
    let input_width = u16::from(range.max) - u16::from(range.min);
    std::array::from_fn(|index| {
        let value = index as u16;
        if value <= u16::from(range.min) {
            0
        } else if value >= u16::from(range.max) {
            255
        } else {
            (((value - u16::from(range.min)) * 255) / input_width) as u8
        }
    })
}

/// Map every sample through a 256-entry lookup table.
pub(crate) fn apply_lut_in_place(data: &mut [u8], lut: &[u8; 256]) {
    for value in data {
        *value = lut[*value as usize];
    }
}

// --- 3-tap box blur -------------------------------------------------------------

/// `(a + b + c + 1) / 3` for one scalar triple.
#[inline(always)]
fn blur_scalar(a: u32, b: u32, c: u32) -> u8 {
    ((a + b + c + 1) / 3) as u8
}

/// The three-tap sum (plus the reference's `+1` rounding term) of two 16-lane
/// byte vectors, widened to `u16`.
#[inline(always)]
fn sum3(left: u8x16, mid: u8x16, right: u8x16) -> (u16x8, u16x8) {
    let one = u16x8::splat(1);
    let low = u16x8::from_u8x16_low(left)
        + u16x8::from_u8x16_low(mid)
        + u16x8::from_u8x16_low(right)
        + one;
    let high = u16x8::from_u8x16_high(left)
        + u16x8::from_u8x16_high(mid)
        + u16x8::from_u8x16_high(right)
        + one;
    (low, high)
}

/// `sum / 3` for every lane; `sum` never exceeds `766`, so the `1 / 3` magic
/// multiply is exact.
#[inline(always)]
fn div3(sum: u16x8) -> i16x8 {
    sum.mul_keep_high(u16x8::splat(21846)).cast_signed()
}

/// [`sum3`] followed by the exact `1 / 3`.
#[inline(always)]
fn blur_block(left: u8x16, mid: u8x16, right: u8x16) -> u8x16 {
    let (low, high) = sum3(left, mid, right);
    u8x16::narrow_i16x8(div3(low), div3(high))
}

/// One horizontal pass of [`box_blur_1_in_place`] over a single row.
fn blur_row(row: &[u8], out: &mut [u8]) {
    let width = row.len();
    if width == 1 {
        out[0] = row[0];
        return;
    }
    out[0] = blur_scalar(u32::from(row[0]), u32::from(row[0]), u32::from(row[1]));
    out[width - 1] = blur_scalar(
        u32::from(row[width - 2]),
        u32::from(row[width - 1]),
        u32::from(row[width - 1]),
    );

    let mut x = 1;
    while x + 16 < width {
        let block = blur_block(load16(row, x - 1), load16(row, x), load16(row, x + 1));
        store16(out, x, block);
        x += 16;
    }
    while x < width - 1 {
        out[x] = blur_scalar(
            u32::from(row[x - 1]),
            u32::from(row[x]),
            u32::from(row[x + 1]),
        );
        x += 1;
    }
}

/// One vertical pass of [`box_blur_1_in_place`] over three (already horizontal) rows.
///
/// The side neighbours are already replicated in the horizontal pass, so every
/// column is treated identically.
fn blur_col(top: &[u8], mid: &[u8], bot: &[u8], out: &mut [u8]) {
    let width = mid.len();
    let mut x = 0;
    while x + 16 <= width {
        let block = blur_block(load16(top, x), load16(mid, x), load16(bot, x));
        store16(out, x, block);
        x += 16;
    }
    while x < width {
        out[x] = blur_scalar(u32::from(top[x]), u32::from(mid[x]), u32::from(bot[x]));
        x += 1;
    }
}

/// Pillow's `ImageFilter.BoxBlur(1)`, in place: a horizontal then vertical 3-tap
/// average, each rounded, with edge pixels replicated.
///
/// Bit-identical to the scalar version it replaces (see docs/processing.md); only
/// the inner loops are vectorised. The horizontal pass reads the source into a
/// scratch buffer, so the vertical pass can overwrite the source (which is no
/// longer needed), keeping the blur to one scratch allocation instead of a second
/// full image.
pub(crate) fn box_blur_1_in_place(image: &mut GrayImage) {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return;
    }
    let (wu, hu) = (width as usize, height as usize);

    let mut horizontal = vec![0u8; wu * hu];
    {
        let src = image.as_raw();
        for y in 0..hu {
            blur_row(
                &src[y * wu..(y + 1) * wu],
                &mut horizontal[y * wu..(y + 1) * wu],
            );
        }
    }

    let pixels: &mut [u8] = &mut *image;
    for y in 0..hu {
        let top = &horizontal[y.saturating_sub(1) * wu..][..wu];
        let mid = &horizontal[y * wu..(y + 1) * wu];
        let bot = &horizontal[(y + 1).min(hu - 1) * wu..][..wu];
        blur_col(top, mid, bot, &mut pixels[y * wu..(y + 1) * wu]);
    }
}

// --- rectangle reductions -------------------------------------------------------

/// A clipped `(left, top, right, bottom)` rectangle within an image.
fn clip(image: &GrayImage, left: i64, top: i64, right: i64, bottom: i64) -> Option<BBox<usize>> {
    let width = i64::from(image.width());
    let height = i64::from(image.height());
    let left = left.max(0);
    let top = top.max(0);
    let right = right.min(width);
    let bottom = bottom.min(height);
    if left >= right || top >= bottom {
        return None;
    }
    Some(BBox::new(
        left as usize,
        top as usize,
        right as usize,
        bottom as usize,
    ))
}

/// Count the samples of `[left, right) x [top, bottom)` for which `vector` (or
/// `scalar` on the tail) is true, clipped to the image.
fn count_where(
    image: &GrayImage,
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
    vector: impl Fn(u8x16) -> u8x16,
    scalar: impl Fn(u8) -> bool,
) -> u64 {
    let Some(rect) = clip(image, left, top, right, bottom) else {
        return 0;
    };
    let (left, top, right, bottom) = (rect.left, rect.upper, rect.right, rect.lower);
    let stride = image.width() as usize;
    let raw = image.as_raw();
    let mut count = 0u64;
    for y in top..bottom {
        let row = &raw[y * stride..(y + 1) * stride];
        let mut x = left;
        while x + 16 <= right {
            count += u64::from(vector(load16(row, x)).to_bitmask().count_ones());
            x += 16;
        }
        while x < right {
            if scalar(row[x]) {
                count += 1;
            }
            x += 1;
        }
    }
    count
}

/// The number of samples `< threshold` in the clipped rectangle.
pub(crate) fn count_lt(
    image: &GrayImage,
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
    threshold: u8,
) -> u64 {
    count_where(
        image,
        left,
        top,
        right,
        bottom,
        |block| block.simd_lt(u8x16::splat(threshold)),
        |value| value < threshold,
    )
}

/// The number of non-zero samples in the clipped rectangle.
pub(crate) fn count_nonzero(
    image: &GrayImage,
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
) -> u64 {
    count_where(
        image,
        left,
        top,
        right,
        bottom,
        |block| block.simd_ne(u8x16::splat(0)),
        |value| value != 0,
    )
}

/// Bounding box of the samples matching `vector`/`scalar`, as Pillow's
/// `getbbox` returns it (`(left, upper, right, lower)` with right/lower
/// exclusive), or `None` when nothing matches.
fn bbox_where(
    image: &GrayImage,
    vector: impl Fn(u8x16) -> u8x16,
    scalar: impl Fn(u8) -> bool,
) -> Option<BBox<u32>> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let raw = image.as_raw();
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0u32;
    let mut max_y = 0u32;

    for y in 0..height {
        let row = &raw[y * width..(y + 1) * width];
        let mut row_first = u32::MAX;
        let mut row_last = 0u32;
        let mut x = 0;
        while x + 16 <= width {
            let bits = vector(load16(row, x)).to_bitmask();
            if bits != 0 {
                // The bitmask spans 16 lanes; the highest set bit is `31 - lz`.
                row_first = row_first.min(x as u32 + bits.trailing_zeros());
                row_last = row_last.max(x as u32 + 31 - bits.leading_zeros());
            }
            x += 16;
        }
        while x < width {
            if scalar(row[x]) {
                let x = x as u32;
                row_first = row_first.min(x);
                row_last = row_last.max(x);
            }
            x += 1;
        }
        if row_first != u32::MAX {
            let y = y as u32;
            min_x = min_x.min(row_first);
            max_x = max_x.max(row_last);
            min_y = min_y.min(y);
            max_y = y;
        }
    }

    (min_x != u32::MAX).then_some(BBox::new(min_x, min_y, max_x + 1, max_y + 1))
}

/// Bounding box of samples `>= threshold`.
pub(crate) fn bbox_ge(image: &GrayImage, threshold: u8) -> Option<BBox<u32>> {
    bbox_where(
        image,
        |block| block.simd_ge(u8x16::splat(threshold)),
        |value| value >= threshold,
    )
}

/// Bounding box of samples `< threshold`.
pub(crate) fn bbox_lt(image: &GrayImage, threshold: u8) -> Option<BBox<u32>> {
    bbox_where(
        image,
        |block| block.simd_lt(u8x16::splat(threshold)),
        |value| value < threshold,
    )
}

/// Bounding box of non-zero samples.
pub(crate) fn bbox_nonzero(image: &GrayImage) -> Option<BBox<u32>> {
    bbox_where(
        image,
        |block| block.simd_ne(u8x16::splat(0)),
        |value| value != 0,
    )
}

/// Whether a band contained any non-zero (white) and any zero (black) sample.
///
/// Both facts are independent (an empty band is neither), so this is a struct,
/// not a sum type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Band {
    pub(crate) has_white: bool,
    pub(crate) has_black: bool,
}

/// Whether the horizontal band `[x0, x1) x [y0, y1)` contains any non-zero
/// (white) and any zero (black) sample; `has_black` starts `true` when the band
/// overhangs `height`, matching Pillow's zero-fill below the strip.
pub(crate) fn band_white_black(
    image: &GrayImage,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    height: u32,
) -> Band {
    let width = image.width() as usize;
    let stride = width;
    let raw = image.as_raw();
    let mut has_white = false;
    let mut has_black = y1 > height;
    for y in y0..y1.min(height) {
        let row = &raw[y as usize * stride..(y as usize + 1) * stride];
        let mut x = x0 as usize;
        let end = (x1 as usize).min(width);
        while x + 16 <= end {
            let bits = load16(row, x).simd_ne(u8x16::splat(0)).to_bitmask();
            has_white |= bits != 0;
            has_black |= bits != 0xFFFF_u32;
            x += 16;
        }
        while x < end {
            if row[x] != 0 {
                has_white = true;
            } else {
                has_black = true;
            }
            x += 1;
        }
    }
    Band {
        has_white,
        has_black,
    }
}

/// The zero-based indices of the rows that are entirely zero.
pub(crate) fn empty_rows(image: &GrayImage) -> Vec<usize> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let raw = image.as_raw();
    let zero = u8x16::splat(0);
    let mut rows = Vec::new();
    for y in 0..height {
        let row = &raw[y * width..(y + 1) * width];
        let mut any = false;
        let mut x = 0;
        while x + 16 <= width {
            any |= load16(row, x).simd_ne(zero).any();
            if any {
                break;
            }
            x += 16;
        }
        if !any {
            while x < width {
                if row[x] != 0 {
                    any = true;
                    break;
                }
                x += 1;
            }
        }
        if !any {
            rows.push(y);
        }
    }
    rows
}

/// The zero-based indices of the columns that are entirely zero.
pub(crate) fn empty_columns(image: &GrayImage) -> Vec<usize> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let raw = image.as_raw();
    let zero = u8x16::splat(0);
    let full = width / 16 * 16;
    let blocks = full.div_ceil(16);
    let mut accumulators = vec![u8x16::splat(0); blocks];
    let mut tail = vec![false; width - full];

    for y in 0..height {
        let row = &raw[y * width..(y + 1) * width];
        for (block, accumulator) in accumulators.iter_mut().enumerate() {
            let at = block * 16;
            *accumulator |= load16(row, at).simd_ne(zero);
        }
        for (index, value) in row[full..].iter().enumerate() {
            tail[index] |= *value != 0;
        }
    }

    let mut columns = Vec::new();
    for x in 0..full {
        if accumulators[x / 16].to_bitmask() >> (x % 16) & 1 == 0 {
            columns.push(x);
        }
    }
    for (index, filled) in tail.iter().enumerate() {
        if !*filled {
            columns.push(full + index);
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, RgbImage};

    /// A scalar reference for the box blur, used to pin the SIMD path.
    fn blur_reference(image: &GrayImage) -> GrayImage {
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

    fn pseudo_random(width: u32, height: u32, seed: u64) -> GrayImage {
        let mut state = seed | 1;
        GrayImage::from_fn(width, height, |_, _| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            Luma([(state % 256) as u8])
        })
    }

    #[test]
    fn box_blur_matches_the_scalar_reference() {
        // Several widths exercise the 16-lane body, its scalar head/tail and the
        // one-pixel edge replication.
        for width in [1u32, 2, 15, 16, 17, 31, 33, 64, 65] {
            let image = pseudo_random(width, 9, u64::from(width) + 3);
            let mut blurred = image.clone();
            box_blur_1_in_place(&mut blurred);
            assert_eq!(blurred, blur_reference(&image), "width {width}");
        }
    }

    #[test]
    fn min_max_matches_the_scalar_extremes() {
        assert_eq!(min_max(&[]), None);
        let data: Vec<u8> = (0..=255).collect();
        assert_eq!(min_max(&data), Some(Range::new(0, 255)));
        assert_eq!(min_max(&[7]), Some(Range::new(7, 7)));
        assert_eq!(min_max(&[200, 1, 3, 99, 42]), Some(Range::new(1, 200)));
    }

    #[test]
    fn threshold_matches_the_scalar_predicate() {
        let mut below: Vec<u8> = (0..=255).collect();
        threshold_in_place(&mut below, 128, ThresholdKind::Below);
        assert_eq!(below[128], 255);
        assert_eq!(below[129], 0);
        let mut above: Vec<u8> = (0..=255).collect();
        threshold_in_place(&mut above, 128, ThresholdKind::Above);
        assert_eq!(above[128], 0);
        assert_eq!(above[129], 255);
    }

    #[test]
    fn threshold_applies_the_scalar_tail_for_both_polarities() {
        // A length that is not a multiple of 16 leaves a scalar tail after the
        // 16-lane body; index 0 is processed by the body, index 19 by the tail.
        let mut above = vec![128u8; 20];
        above[0] = 1;
        above[19] = 200;
        threshold_in_place(&mut above, 128, ThresholdKind::Above);
        assert_eq!(above[0], 0);
        assert_eq!(above[19], 255);

        let mut below = vec![128u8; 20];
        below[0] = 1;
        below[19] = 200;
        threshold_in_place(&mut below, 128, ThresholdKind::Below);
        assert_eq!(below[0], 255);
        assert_eq!(below[19], 0);
    }

    #[test]
    fn luma_min_max_handles_pixel_only_and_alpha_sources() {
        // An empty RGB buffer has no samples to reduce.
        assert_eq!(
            luma_min_max(&DynamicImage::ImageRgb8(RgbImage::new(0, 0))),
            None
        );
        // A pixel type without a direct luma/RGB view is converted first; all four
        // pixels of (10, 20, 30) reduce to the same Rec.601 luma.
        let rgba = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([10, 20, 30, 255]),
        ));
        assert_eq!(luma_min_max(&rgba), Some(Range::new(18, 18)));
    }

    #[test]
    fn a_degenerate_box_blur_is_a_no_op() {
        // A zero-width row would panic in `blur_row` (it indexes the ends), so the guard
        // is load-bearing there; a zero-height image (or an empty one) has nothing to blur.
        for image in [
            GrayImage::new(0, 5),
            GrayImage::new(5, 0),
            GrayImage::new(0, 0),
        ] {
            let mut blurred = image.clone();
            box_blur_1_in_place(&mut blurred);
            assert_eq!(blurred, image);
        }
    }

    #[test]
    fn an_empty_bounding_box_search_finds_nothing() {
        // A defensive guard: with no pixels the scan would also find nothing.
        assert_eq!(bbox_ge(&GrayImage::new(0, 0), 100), None);
    }

    #[test]
    fn a_band_with_a_scalar_tail_reads_the_remaining_columns() {
        let mut image = GrayImage::new(32, 8);
        for x in 0..16 {
            image.put_pixel(x, 0, Luma([255]));
        }
        // The band ends at x = 20, so the final four columns go through the scalar
        // tail rather than the 16-lane body.
        assert_eq!(
            band_white_black(&image, 0, 0, 20, 1, 8),
            Band {
                has_white: true,
                has_black: true,
            }
        );
    }

    #[test]
    fn empty_columns_are_empty_for_a_zero_height_image() {
        // A zero-height image would otherwise report every column as empty; the guard
        // returns nothing instead.
        assert!(empty_columns(&GrayImage::new(5, 0)).is_empty());
        assert!(empty_columns(&GrayImage::new(0, 0)).is_empty());
    }

    #[test]
    fn invert_matches_the_scalar_subtraction() {
        let data: Vec<u8> = (0..=255).collect();
        let mut out = data.clone();
        invert_in_place(&mut out);
        assert!(out
            .iter()
            .zip(&data)
            .all(|(inverted, original)| *inverted == 255 - *original));
    }

    #[test]
    fn bounding_boxes_match_a_scalar_scan() {
        let image = GrayImage::from_fn(40, 23, |x, y| {
            if (10..20).contains(&x) && (5..15).contains(&y) {
                Luma([200])
            } else {
                Luma([10])
            }
        });
        assert_eq!(bbox_ge(&image, 100), Some(BBox::new(10, 5, 20, 15)));
        assert_eq!(bbox_lt(&image, 100), Some(BBox::new(0, 0, 40, 23)));
        assert_eq!(bbox_nonzero(&image), Some(BBox::new(0, 0, 40, 23)));
        let blank = GrayImage::new(8, 8);
        assert_eq!(bbox_nonzero(&blank), None);
    }

    #[test]
    fn empty_axes_agree_with_a_scalar_scan() {
        let mut image = GrayImage::new(35, 7);
        image.put_pixel(34, 3, Luma([255]));
        image.put_pixel(17, 0, Luma([255]));
        assert_eq!(empty_rows(&image), vec![1, 2, 4, 5, 6]);
        assert_eq!(
            empty_columns(&image),
            (0..35).filter(|x| *x != 17 && *x != 34).collect::<Vec<_>>()
        );
    }

    #[test]
    fn band_reports_white_and_black() {
        let mut image = GrayImage::new(32, 8);
        for x in 0..16 {
            image.put_pixel(x, 0, Luma([255]));
        }
        // Row 0: mixed over x in [0, 32) -> both.
        assert_eq!(
            band_white_black(&image, 0, 0, 32, 1, 8),
            Band {
                has_white: true,
                has_black: true
            }
        );
        // Row 1 is all black.
        assert_eq!(
            band_white_black(&image, 0, 1, 32, 2, 8),
            Band {
                has_white: false,
                has_black: true
            }
        );
        // Row 0, left half only: all white, no black.
        assert_eq!(
            band_white_black(&image, 0, 0, 16, 1, 8),
            Band {
                has_white: true,
                has_black: false
            }
        );
        // A band overhanging the bottom is black-padded.
        assert_eq!(
            band_white_black(&image, 0, 0, 16, 20, 8),
            Band {
                has_white: true,
                has_black: true
            }
        );
    }
}
