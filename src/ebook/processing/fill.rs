//! Page background detection — `fillCheck` (see docs/processing.md).
//!
//! Clean-room reimplementation of KCC's fill heuristic: threshold the page to a
//! black/white mask, compare the bounding boxes of the two colours, and fall back
//! to sampling the page border in 5-pixel strips when the areas are close.
//!
//! The result decides the padding colour for `pad` resizes and whether a page is
//! flagged `BlackBackground` (see docs/architecture.md).

use image::{DynamicImage, GrayImage};

use crate::ebook::model::Background;
use crate::ebook::processing::color::luma_view;
use crate::ebook::processing::kernels;

/// Threshold between black and white; values `>= 128` are white.
const THRESHOLD: u8 = 128;

/// Detect the dominant background colour of a page.
pub fn fill_check(image: &DynamicImage) -> Background {
    let mask = luma_view(image);

    let white_box = kernels::bbox_ge(&mask, THRESHOLD);
    let black_box = kernels::bbox_lt(&mask, THRESHOLD);

    let white_area = white_box.map(box_area).unwrap_or(0);
    let black_area = black_box.map(box_area).unwrap_or(0);

    let diff = if white_box.is_some() && black_box.is_some() {
        let largest = white_area.max(black_area);
        let smallest = white_area.min(black_area);
        (largest - smallest) as f64 / smallest as f64 * 100.0
    } else {
        0.0
    };

    if diff > 0.5 {
        // Whichever colour occupies the larger bounding box wins.
        return if black_area < white_area {
            Background::White
        } else {
            Background::Black
        };
    }

    if border_fill(&mask) > 0 {
        Background::Black
    } else {
        Background::White
    }
}

/// The area of an inclusive pixel bounding box (`(left, top, right, bottom)` with
/// `right`/`bottom` exclusive, as Pillow's `getbbox` returns them).
fn box_area((left, top, right, bottom): (u32, u32, u32, u32)) -> u64 {
    u64::from(right - left) * u64::from(bottom - top)
}

/// Sum the border-strip histogram votes (KCC's tie-breaker).
///
/// A strip with no black pixels votes `-1`, one with no white pixels votes `+1`,
/// and a mixed strip votes `0`. A positive total means the border is mostly
/// black.
fn border_fill(mask: &GrayImage) -> i64 {
    let (width, height) = mask.dimensions();
    let mut total = 0i64;

    let mut start_y = 0;
    while start_y < height {
        let top = if start_y + 5 > height {
            height.saturating_sub(5)
        } else {
            start_y
        };
        total += strip_vote(mask, 0, top, width, (top + 5).min(height));
        start_y += 5;
    }

    let mut start_x = 0;
    while start_x < width {
        let left = if start_x + 5 > width {
            width.saturating_sub(5)
        } else {
            start_x
        };
        total += strip_vote(mask, left, 0, (left + 5).min(width), height);
        start_x += 5;
    }

    total
}

/// The histogram vote for the rectangle `[left, right) x [top, bottom)`.
fn strip_vote(mask: &GrayImage, left: u32, top: u32, right: u32, bottom: u32) -> i64 {
    let black = kernels::count_lt(
        mask,
        i64::from(left),
        i64::from(top),
        i64::from(right),
        i64::from(bottom),
        THRESHOLD,
    );
    let width = i64::from(right) - i64::from(left);
    let height = i64::from(bottom) - i64::from(top);
    let area = (width.max(0) * height.max(0)) as u64;
    let white = area - black;

    match (black, white) {
        (0, _) => -1,
        (_, 0) => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    /// Build an RGB image from a per-pixel colour function.
    fn build(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| Rgb(f(x, y))))
    }

    #[test]
    fn solid_white_is_white() {
        let image = build(20, 20, |_, _| [255, 255, 255]);
        assert_eq!(fill_check(&image), Background::White);
    }

    #[test]
    fn solid_black_is_black() {
        let image = build(20, 20, |_, _| [0, 0, 0]);
        assert_eq!(fill_check(&image), Background::Black);
    }

    #[test]
    fn white_page_with_small_dark_content_is_white() {
        // A big white field with a small black square in the middle.
        let image = build(40, 40, |x, y| {
            if (18..22).contains(&x) && (18..22).contains(&y) {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        assert_eq!(fill_check(&image), Background::White);
    }

    #[test]
    fn black_page_with_small_light_content_is_black() {
        let image = build(40, 40, |x, y| {
            if (18..22).contains(&x) && (18..22).contains(&y) {
                [255, 255, 255]
            } else {
                [0, 0, 0]
            }
        });
        assert_eq!(fill_check(&image), Background::Black);
    }

    #[test]
    fn equal_bounding_boxes_fall_back_to_border_sampling() {
        // Left half white, right half black: both bounding boxes span the whole
        // image, so the strip sampler decides. The border is mixed on all sides,
        // so the vote is zero and the page reads as white.
        let image = build(
            20,
            20,
            |x, _| {
                if x < 10 {
                    [255, 255, 255]
                } else {
                    [0, 0, 0]
                }
            },
        );
        assert_eq!(fill_check(&image), Background::White);
    }
}
