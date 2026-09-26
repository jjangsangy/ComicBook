//! Inter-panel gutter cropping (see docs/processing.md).
//!
//! Clean-room reimplementation of KCC's `crop_empty_inter_panel`: the page is
//! thresholded like the margin cutter, empty rows (or columns) away from the
//! borders are found, and all but a small fraction of each gutter is deleted.
//!
//! KCC compares a column section's bounds against the page *height* even when it
//! is looking for empty columns; that off-by-design quirk changes which gutters
//! are eligible, so it is reproduced here.

use std::collections::BTreeSet;

use image::{DynamicImage, GrayImage, ImageBuffer, Pixel, Primitive};

use crate::ebook::model::Background;
use crate::ebook::processing::crop::{
    autocontrast_cutoff, binarize, box_blur_1, group_close_values, invert, threshold_from_power,
    to_gray, CROP_CUTOFF, INTERPANEL_POWER,
};

/// Which gutters to collapse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Drop empty rows, keeping the panels stacked.
    Horizontal,
    /// Drop empty columns, keeping the panels side by side.
    Vertical,
    /// Drop both.
    Both,
}

/// Split index `value` into `(first, last)` keeping `keep` of the span as margin.
fn kept_span(start: i64, end: i64, keep: f64) -> (i64, i64) {
    let span = (end - start) as f64;
    let margin = keep / 2.0 * span;
    ((start as f64 + margin) as i64, (end as f64 - margin) as i64)
}

/// The indices of the empty rows/columns that should be removed.
fn empty_sections(bw: &GrayImage, keep: f64, horizontal: bool) -> BTreeSet<usize> {
    let (width, height) = bw.dimensions();

    let mut empties: Vec<i64> = Vec::new();
    if horizontal {
        for y in 0..height {
            if (0..width).all(|x| bw.get_pixel(x, y)[0] == 0) {
                empties.push(i64::from(y));
            }
        }
    } else {
        for x in 0..width {
            if (0..height).all(|y| bw.get_pixel(x, y)[0] == 0) {
                empties.push(i64::from(x));
            }
        }
    }

    // The reference uses `img.size[1]` (the height) for the border test in both
    // directions; see the module docs.
    let reference = f64::from(height);

    let mut remove = BTreeSet::new();
    for (start, end) in group_close_values(&empties, 1.0) {
        // Ignore gutters that touch the page border.
        if (end as f64) < reference * 0.99 && (start as f64) > reference * 0.01 {
            let (first, last) = kept_span(start, end, keep);
            for index in first..last {
                if index >= 0 {
                    remove.insert(index as usize);
                }
            }
        }
    }

    remove
}

/// Copy `source` without the lines listed in `remove`.
///
/// Rows/columns are moved through the typed buffer, so grayscale pages keep their
/// pixel type (kept bespoke; see docs/dependencies.md).
fn keep_lines<P>(
    source: &ImageBuffer<P, Vec<P::Subpixel>>,
    remove: &BTreeSet<usize>,
    remove_rows: bool,
) -> ImageBuffer<P, Vec<P::Subpixel>>
where
    P: Pixel + 'static,
{
    let width = source.width() as usize;
    let height = source.height() as usize;
    let channels = P::CHANNEL_COUNT as usize;
    let raw = source.as_raw();

    if remove_rows {
        let kept: Vec<usize> = (0..height).filter(|y| !remove.contains(y)).collect();
        let stride = width * channels;
        let mut out = Vec::with_capacity(kept.len() * stride);
        for &y in &kept {
            out.extend_from_slice(&raw[y * stride..(y + 1) * stride]);
        }
        ImageBuffer::from_raw(width as u32, kept.len() as u32, out)
            .unwrap_or_else(|| source.clone())
    } else {
        let kept: Vec<usize> = (0..width).filter(|x| !remove.contains(x)).collect();
        let out_width = kept.len();
        let mut out = vec![P::Subpixel::DEFAULT_MIN_VALUE; out_width * height * channels];
        for y in 0..height {
            for (target_x, &x) in kept.iter().enumerate() {
                let from = (y * width + x) * channels;
                let to = (y * out_width + target_x) * channels;
                out[to..to + channels].copy_from_slice(&raw[from..from + channels]);
            }
        }
        ImageBuffer::from_raw(out_width as u32, height as u32, out)
            .unwrap_or_else(|| source.clone())
    }
}

/// [`keep_lines`] for a concrete [`DynamicImage`], preserving its pixel type.
fn remove_lines(image: &DynamicImage, remove: &BTreeSet<usize>, remove_rows: bool) -> DynamicImage {
    macro_rules! arm {
        ($variant:ident, $buffer:expr) => {
            DynamicImage::$variant(keep_lines($buffer, remove, remove_rows))
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
        DynamicImage::ImageRgb32F(buffer) => arm!(ImageRgb32F, buffer),
        DynamicImage::ImageRgba32F(buffer) => arm!(ImageRgba32F, buffer),
        other => DynamicImage::ImageRgb8(keep_lines(&other.to_rgb8(), remove, remove_rows)),
    }
}

/// Collapse the empty gutters of a page (KCC's `crop_empty_inter_panel`).
///
/// `keep` is the fraction of each gutter retained after cropping.
pub fn crop_empty_inter_panel(
    image: &DynamicImage,
    direction: Direction,
    keep: f64,
    background: Background,
) -> DynamicImage {
    let gray = to_gray(image);
    let gray = if background == Background::White {
        gray
    } else {
        invert(&gray)
    };
    let gray = box_blur_1(&autocontrast_cutoff(&gray, CROP_CUTOFF));
    let bw = binarize(&gray, threshold_from_power(INTERPANEL_POWER));

    let mut result = image.clone();
    if matches!(direction, Direction::Horizontal | Direction::Both) {
        let remove = empty_sections(&bw, keep, true);
        result = remove_lines(&result, &remove, true);
    }
    if matches!(direction, Direction::Vertical | Direction::Both) {
        let remove = empty_sections(&bw, keep, false);
        result = remove_lines(&result, &remove, false);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgb, RgbImage};

    /// Two panels stacked vertically with a white gutter between them, matching
    /// the committed `interpanel-white.png` fixture.
    fn two_panels(width: u32, height: u32, gutter: (u32, u32)) -> DynamicImage {
        let (gutter_top, gutter_bottom) = gutter;
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            let panel = (20..180).contains(&x)
                && (20..280).contains(&y)
                && !(gutter_top..gutter_bottom).contains(&y);
            if panel {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        }))
    }

    #[test]
    fn horizontal_crop_collapses_the_gutter() {
        let image = two_panels(200, 300, (140, 160));
        let cropped =
            crop_empty_inter_panel(&image, Direction::Horizontal, 0.04, Background::White);
        assert_eq!(cropped.dimensions(), (200, 285));
    }

    #[test]
    fn vertical_crop_collapses_the_side_margins() {
        let image = two_panels(200, 300, (140, 160));
        let cropped = crop_empty_inter_panel(&image, Direction::Vertical, 0.04, Background::White);
        assert_eq!(cropped.dimensions(), (184, 300));
    }

    #[test]
    fn both_directions_compose() {
        let image = two_panels(200, 300, (140, 160));
        let cropped = crop_empty_inter_panel(&image, Direction::Both, 0.04, Background::White);
        assert_eq!(cropped.dimensions(), (184, 285));
    }

    #[test]
    fn a_solid_page_has_no_gutters_to_crop() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 300, Rgb([0, 0, 0])));
        let cropped = crop_empty_inter_panel(&image, Direction::Both, 0.04, Background::White);
        assert_eq!(cropped.dimensions(), (200, 300));
    }

    #[test]
    fn grayscale_pages_keep_their_pixel_type() {
        let gray = DynamicImage::ImageLuma8(GrayImage::from_fn(200, 300, |x, y| {
            let panel =
                (20..180).contains(&x) && (20..280).contains(&y) && !(140..160).contains(&y);
            image::Luma([if panel { 0 } else { 255 }])
        }));
        let cropped = crop_empty_inter_panel(&gray, Direction::Horizontal, 0.04, Background::White);
        assert!(matches!(cropped, DynamicImage::ImageLuma8(_)));
        assert_eq!(cropped.dimensions(), (200, 285));
    }

    #[test]
    fn kept_span_retains_half_the_margin_at_each_end() {
        // A 16-pixel gutter keeps 2 % at each end, so 15 rows are dropped.
        assert_eq!(kept_span(142, 158, 0.04), (142, 157));
    }
}
