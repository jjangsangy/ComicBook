//! Colour vs. grayscale decision — `colorCheck` (see docs/processing.md).
//!
//! Clean-room reimplementation of KCC's chroma histogram heuristic: an image is
//! considered "colourful" when its Cb/Cr histograms spread far enough around the
//! neutral value (128). The decision matters twice: it is compared against
//! `--force-color` to decide whether a page keeps its colour, and it gates
//! `--color-autocontrast`.
//!
//! The JFIF YCbCr matrix below is the conversion Pillow applies in
//! `Image.convert("YCbCr")`. Kept bespoke (see docs/dependencies.md): the coefficients are
//! pinned to JFIF/Rec. 601 by the tests below.

use std::borrow::Cow;

use image::{DynamicImage, GrayImage, RgbImage};

use crate::ebook::options::Options;
use crate::ebook::processing::crop::trim_histogram_ends;
use crate::units::{Percent, Range};

/// `(cutoff percent, neutral diff threshold)` pairs, applied in order until one
/// decides (see docs/processing.md).
const CASCADE: [(Percent, i32); 3] = [
    (Percent::new(0.0), 22),
    (Percent::new(0.2), 10),
    (Percent::new(3.0), 4),
];

/// Below this chroma spread the page is treated as "not colourful" (KCC's bias
/// adjustment; do not lower it).
const SPREAD_THRESHOLD: u8 = 7;

/// Whether a page should be treated as colour.
///
/// `original_is_grayscale` is the source's colour mode: a page that was decoded
/// as `L`/`1` is never colour, matching KCC's `original_color_mode in ("L", "1")`
/// shortcut.
pub fn color_check(image: &RgbImage, original_is_grayscale: bool, options: &Options) -> bool {
    if original_is_grayscale {
        return false;
    }
    if options.main.webtoon {
        return true;
    }
    calculate_color(image, options.processing.color.force_color)
}

/// The histogram cascade, returning as soon as a step decides.
fn calculate_color(image: &RgbImage, force_color: bool) -> bool {
    let (cb_hist, cr_hist) = chroma_histograms(image);

    for (cutoff, diff_threshold) in CASCADE {
        if let Some(decision) =
            color_precision(&cb_hist, &cr_hist, cutoff, diff_threshold, force_color)
        {
            return decision;
        }
    }
    false
}

/// Cb and Cr histograms of an RGB image.
fn chroma_histograms(image: &RgbImage) -> ([u64; 256], [u64; 256]) {
    let mut cb_hist = [0u64; 256];
    let mut cr_hist = [0u64; 256];

    for pixel in image.pixels() {
        let (_, cb, cr) = rgb_to_ycbcr(pixel[0], pixel[1], pixel[2]);
        cb_hist[cb as usize] += 1;
        cr_hist[cr as usize] += 1;
    }

    (cb_hist, cr_hist)
}

/// One cascade step. `Some(decision)` when the step can decide, `None` to carry
/// on to the next cutoff.
fn color_precision(
    cb_hist: &[u64; 256],
    cr_hist: &[u64; 256],
    cutoff: Percent,
    diff_threshold: i32,
    force_color: bool,
) -> Option<bool> {
    let mut cb = *cb_hist;
    let mut cr = *cr_hist;
    histograms_cutoff(&mut cb, &mut cr, cutoff);

    let cb = nonzero_bounds(&cb)?;
    let cr = nonzero_bounds(&cr)?;

    if force_color {
        // With `--force-color` a biased histogram is enough to call it colour.
        if cb.min > 128 || cr.min > 128 || cb.max < 128 || cr.max < 128 {
            return Some(true);
        }
    } else if cb.spread() < SPREAD_THRESHOLD && cr.spread() < SPREAD_THRESHOLD {
        return Some(false);
    }

    let low = 128 - diff_threshold;
    let high = 128 + diff_threshold;
    if i32::from(cb.min) <= low
        || i32::from(cr.min) <= low
        || i32::from(cb.max) >= high
        || i32::from(cr.max) >= high
    {
        return Some(true);
    }

    None
}

/// Remove `cutoff` percent of samples from both ends of each histogram, which
/// discards JPEG ringing artefacts before the spread is measured.
fn histograms_cutoff(cb_hist: &mut [u64; 256], cr_hist: &mut [u64; 256], cutoff: Percent) {
    if cutoff.is_zero() {
        return;
    }
    for hist in [cb_hist, cr_hist] {
        let sample_count: u64 = hist.iter().sum();
        let cut = ((sample_count as f64 * cutoff.value()) / 100.0).floor() as u64;
        trim_histogram_ends(hist, cut);
    }
}

/// The first and last non-zero bins of a histogram, or `None` when it is empty.
fn nonzero_bounds(hist: &[u64; 256]) -> Option<Range> {
    let first = hist.iter().position(|&count| count != 0)?;
    let last = hist.iter().rposition(|&count| count != 0)?;
    Some(Range::new(first as u8, last as u8))
}

/// JFIF full-range RGB → YCbCr, the transform Pillow applies for `YCbCr`.
pub(crate) fn rgb_to_ycbcr(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
    let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
    (clamp_u8(y), clamp_u8(cb), clamp_u8(cr))
}

/// JFIF full-range YCbCr → RGB, the inverse of [`rgb_to_ycbcr`].
pub(crate) fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    let (y, cb, cr) = (f64::from(y), f64::from(cb) - 128.0, f64::from(cr) - 128.0);
    let r = y + 1.402 * cr;
    let g = y - 0.344_136 * cb - 0.714_136 * cr;
    let b = y + 1.772 * cb;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

/// Rec. 601 luma, the weighting Pillow uses for `convert("L")`.
pub(crate) fn luma601(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b))
}

/// Convert any image to an 8-bit grayscale image using the Rec. 601 weights.
///
/// This is the Rust equivalent of Pillow's `convert("L")`, which KCC relies on
/// for its grayscale output, autocontrast range and fill detection.
///
/// An already-8-bit source is mapped directly rather than routed through a cloned
/// RGB buffer, which keeps the conversion to a single extra allocation (a full
/// image copy otherwise dominates a page's peak working set). Read-only callers
/// should prefer [`luma_view`], which borrows an `L8` plane outright.
pub(crate) fn to_luma601(image: &DynamicImage) -> GrayImage {
    match image {
        DynamicImage::ImageLuma8(gray) => gray.clone(),
        DynamicImage::ImageRgb8(rgb) => rgb_to_luma(rgb),
        other => rgb_to_luma(&other.to_rgb8()),
    }
}

/// A borrowed or freshly converted Rec. 601 grayscale view of `image`.
///
/// Avoids the full-image `L8` copy [`to_luma601`] would make when the caller
/// only needs to read the plane (fill detection, contrast range, crop prep).
pub(crate) fn luma_view(image: &DynamicImage) -> Cow<'_, GrayImage> {
    match image.as_luma8() {
        Some(gray) => Cow::Borrowed(gray),
        None => Cow::Owned(to_luma601(image)),
    }
}

/// Rec. 601 RGB → `L8`, written into an exactly-sized output buffer (no
/// per-pixel allocation and no `Vec` growth, unlike `imageproc::map::map_pixels`).
fn rgb_to_luma(rgb: &RgbImage) -> GrayImage {
    let (width, height) = rgb.dimensions();
    let raw = rgb.as_raw();
    let mut out = Vec::with_capacity(raw.len() / 3);
    for pixel in rgb.as_raw().as_chunks::<3>().0 {
        out.push(luma601(pixel[0], pixel[1], pixel[2]));
    }
    GrayImage::from_raw(width, height, out).unwrap_or_else(|| GrayImage::new(width, height))
}

fn clamp_u8(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::options::Options;
    use anyhow::{bail, Result};
    use clap::Parser;
    use image::Rgb;

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

    /// A solid-colour image.
    fn solid(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
        RgbImage::from_fn(width, height, |_, _| Rgb(color))
    }

    /// A page split into a black and a white half.
    fn half_black(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                Rgb([0, 0, 0])
            } else {
                Rgb([255, 255, 255])
            }
        })
    }

    #[test]
    fn grayscale_sources_are_never_colour() -> Result<()> {
        let image = solid(8, 8, [0, 128, 255]);
        assert!(!color_check(&image, true, &options(&[])?));
        assert!(!color_check(&image, true, &options(&["--force-color"])?));
        Ok(())
    }

    #[test]
    fn grayscale_pixels_are_not_colour() -> Result<()> {
        // Cb == Cr == 128 everywhere, so the spread test bails out.
        let image = half_black(16, 16);
        assert!(!color_check(&image, false, &options(&[])?));
        Ok(())
    }

    #[test]
    fn saturated_colours_are_detected() -> Result<()> {
        // Two very different hues: the chroma histograms spread far past the
        // neutral band, which is what KCC measures.
        let image = RgbImage::from_fn(16, 16, |x, _| {
            if x < 8 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        });
        assert!(color_check(&image, false, &options(&[])?));
        Ok(())
    }

    #[test]
    fn a_solid_colour_has_no_chroma_spread() -> Result<()> {
        // A single flat colour is treated as a (coloured) background, not as a
        // colourful page: KCC requires chroma variation.
        let image = solid(8, 8, [255, 0, 0]);
        assert!(!color_check(&image, false, &options(&[])?));
        Ok(())
    }

    #[test]
    fn webtoon_forces_colour() -> Result<()> {
        let image = half_black(16, 16);
        assert!(color_check(&image, false, &options(&["--webtoon"])?));
        Ok(())
    }

    #[test]
    fn ycbcr_round_trips_within_a_level() {
        for &(r, g, b) in &[
            (0u8, 0u8, 0u8),
            (255, 255, 255),
            (12, 200, 57),
            (200, 30, 90),
        ] {
            let (y, cb, cr) = rgb_to_ycbcr(r, g, b);
            let (rr, gg, bb) = ycbcr_to_rgb(y, cb, cr);
            // Chroma subsampling-free JFIF conversion is lossy but close.
            let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
            assert!(
                close(rr, r) && close(gg, g) && close(bb, b),
                "{r},{g},{b} -> {rr},{gg},{bb}"
            );
        }
    }

    #[test]
    fn luma_matches_rec601_reference() {
        assert_eq!(luma601(255, 255, 255), 255);
        assert_eq!(luma601(0, 0, 0), 0);
        assert_eq!(luma601(255, 0, 0), 76); // 0.299 * 255
        assert_eq!(luma601(0, 255, 0), 150); // 0.587 * 255
        assert_eq!(luma601(0, 0, 255), 29); // 0.114 * 255
    }
}
