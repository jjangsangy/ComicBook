//! Fourier moiré eraser for colour e-ink screens (see docs/processing.md).
//!
//! Clean-room reimplementation of KCC's `rainbow_artifacts_eraser`: the luminance
//! plane is transformed with a 2D FFT and the diagonal frequency bands that beat
//! against a colour e-ink panel's filter array are attenuated. For a colour page
//! the luminance is filtered in YUV and the untouched chroma is recombined; for a
//! grayscale page the `L` plane is filtered directly.
//!
//! The reference uses `numpy.fft.rfft2`; we run a full complex 2D FFT because the
//! attenuation mask is symmetric under `f -> -f` (the four target angles are
//! pairwise 180° apart), so the two are equivalent. Exact bytes will still differ
//! by rounding from the last FFT ULP, which is why the tests assert attenuation
//! behaviour rather than byte equality.

use image::{DynamicImage, GrayImage, Luma, Rgb, RgbImage};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use crate::ebook::processing::color::{luma_view, OutputColor};

/// Frequencies at or above this many cycles/pixel are eligible (0.5 is Nyquist).
const FREQ_THRESHOLD: f64 = 0.30;
/// The primary artifact angle, in degrees.
const TARGET_ANGLE: f64 = 135.0;
/// Half-width of the attenuated angular band, in degrees.
const ANGLE_TOLERANCE: f64 = 10.0;
/// Multiplier applied to the affected frequencies (`0.10` = 90 % attenuation).
const ATTENUATION: f64 = 0.10;

/// Remove colour e-ink moiré from a page (KCC's `erase_rainbow_artifacts`).
///
/// `color_output` must be the page's *output* colour mode (KCC passes
/// `colorOutput`), which also selects the YUV or grayscale path.
pub fn erase_rainbow_artifacts(image: &DynamicImage, color_output: OutputColor) -> DynamicImage {
    if color_output.is_color() {
        // Borrow the plane when it is already RGB8; only other pixel types pay
        // for the conversion.
        match image.as_rgb8() {
            Some(rgb) => DynamicImage::ImageRgb8(erase_color(rgb)),
            None => DynamicImage::ImageRgb8(erase_color(&image.to_rgb8())),
        }
    } else {
        // `luma_view` borrows an existing `L8` plane instead of cloning it; the
        // FFT below reads it pixel by pixel and allocates its own buffer either way.
        DynamicImage::ImageLuma8(erase_gray(&luma_view(image)))
    }
}

/// The signed frequency (cycles/pixel) of FFT bin `index`, matching
/// `numpy.fft.fftfreq`.
fn frequency(index: usize, length: usize) -> f64 {
    let index = index as f64;
    let length = length as f64;
    if index <= length / 2.0 {
        index / length
    } else {
        (index - length) / length
    }
}

/// Whether `angle` (degrees, `[0, 360)`) lies in one of the four attenuated bands.
fn is_diagonal(angle: f64) -> bool {
    [
        TARGET_ANGLE,
        TARGET_ANGLE + 180.0,
        TARGET_ANGLE + 90.0,
        TARGET_ANGLE + 270.0,
    ]
    .iter()
    .any(|&target| {
        let min = (target - ANGLE_TOLERANCE).rem_euclid(360.0);
        let max = (target + ANGLE_TOLERANCE).rem_euclid(360.0);
        if min > max {
            angle >= min || angle <= max
        } else {
            angle >= min && angle <= max
        }
    })
}

/// In-place forward FFT over each row.
fn forward_rows(data: &mut [Complex<f64>], width: usize) {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(width);
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    for row in data.chunks_mut(width) {
        fft.process_with_scratch(row, &mut scratch);
    }
}

/// In-place forward FFT over each column.
fn forward_columns(data: &mut [Complex<f64>], width: usize, height: usize) {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(height);
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    let mut column = vec![Complex::new(0.0, 0.0); height];
    for x in 0..width {
        for y in 0..height {
            column[y] = data[y * width + x];
        }
        fft.process_with_scratch(&mut column, &mut scratch);
        for y in 0..height {
            data[y * width + x] = column[y];
        }
    }
}

/// The unnormalised 2D forward FFT of a row-major `width` x `height` buffer.
fn forward(data: &mut [Complex<f64>], width: usize, height: usize) {
    forward_rows(data, width);
    forward_columns(data, width, height);
}

/// The inverse of [`forward`], including the `1 / (width * height)` scaling.
fn inverse(data: &mut [Complex<f64>], width: usize, height: usize) {
    // The inverse composes in the opposite order to the forward transform.
    let mut planner = FftPlanner::new();
    let column_fft = planner.plan_fft_inverse(height);
    let mut scratch = vec![Complex::new(0.0, 0.0); column_fft.get_inplace_scratch_len()];
    let mut column = vec![Complex::new(0.0, 0.0); height];
    for x in 0..width {
        for y in 0..height {
            column[y] = data[y * width + x];
        }
        column_fft.process_with_scratch(&mut column, &mut scratch);
        for y in 0..height {
            data[y * width + x] = column[y];
        }
    }

    let row_fft = planner.plan_fft_inverse(width);
    let mut scratch = vec![Complex::new(0.0, 0.0); row_fft.get_inplace_scratch_len()];
    for row in data.chunks_mut(width) {
        row_fft.process_with_scratch(row, &mut scratch);
    }

    let scale = 1.0 / (width as f64 * height as f64);
    for value in data.iter_mut() {
        *value *= scale;
    }
}

/// Multiply the diagonal high-frequency bins by [`ATTENUATION`].
fn attenuate(spectrum: &mut [Complex<f64>], width: usize, height: usize) {
    let threshold_squared = FREQ_THRESHOLD * FREQ_THRESHOLD;

    for y in 0..height {
        let freq_y = frequency(y, height);
        for x in 0..width {
            let freq_x = frequency(x, width);
            if freq_x * freq_x + freq_y * freq_y < threshold_squared {
                continue;
            }
            let angle = freq_y.atan2(freq_x).to_degrees().rem_euclid(360.0);
            if is_diagonal(angle) {
                spectrum[y * width + x] *= ATTENUATION;
            }
        }
    }
}

/// Filter a single grayscale plane.
fn erase_gray(image: &GrayImage) -> GrayImage {
    let (width, height) = image.dimensions();
    let (width, height) = (width as usize, height as usize);

    let mut data: Vec<Complex<f64>> = image
        .pixels()
        .map(|pixel| Complex::new(f64::from(pixel[0]), 0.0))
        .collect();
    forward(&mut data, width, height);
    attenuate(&mut data, width, height);
    inverse(&mut data, width, height);

    GrayImage::from_fn(width as u32, height as u32, |x, y| {
        let value = data[(y as usize) * width + x as usize].re;
        Luma([value.clamp(0.0, 255.0) as u8])
    })
}

/// Filter the luminance of a colour page, leaving its chroma intact.
fn erase_color(image: &RgbImage) -> RgbImage {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let count = width * height;

    let mut luma = vec![0.0f64; count];
    let mut chroma_u = vec![0.0f64; count];
    let mut chroma_v = vec![0.0f64; count];
    for (index, pixel) in image.pixels().enumerate() {
        let (y, u, v) = rgb_to_yuv(pixel[0], pixel[1], pixel[2]);
        luma[index] = y;
        chroma_u[index] = u;
        chroma_v[index] = v;
    }

    let mut spectrum: Vec<Complex<f64>> = luma.iter().map(|&y| Complex::new(y, 0.0)).collect();
    forward(&mut spectrum, width, height);
    attenuate(&mut spectrum, width, height);
    inverse(&mut spectrum, width, height);

    RgbImage::from_fn(width as u32, height as u32, |x, y| {
        let index = (y as usize) * width + x as usize;
        let cleaned = spectrum[index].re.clamp(0.0, 255.0);
        let (r, g, b) = yuv_to_rgb(cleaned, chroma_u[index], chroma_v[index]);
        Rgb([
            r.clamp(0.0, 255.0) as u8,
            g.clamp(0.0, 255.0) as u8,
            b.clamp(0.0, 255.0) as u8,
        ])
    })
}

/// BT.601-style RGB → YUV (luminance in `[0, 255]`, chroma centred on zero).
fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (f64::from(r), f64::from(g), f64::from(b));
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let u = -0.147_13 * r - 0.288_86 * g + 0.436 * b;
    let v = 0.615 * r - 0.514_99 * g - 0.100_01 * b;
    (y, u, v)
}

/// The inverse of [`rgb_to_yuv`].
fn yuv_to_rgb(y: f64, u: f64, v: f64) -> (f64, f64, f64) {
    let r = y + 1.139_83 * v;
    let g = y - 0.394_65 * u - 0.580_60 * v;
    let b = y + 2.032_11 * u;
    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::processing::color::Detected;
    use image::GenericImageView;

    /// Standard deviation of a grayscale image's samples.
    fn deviation(image: &GrayImage) -> f64 {
        let values: Vec<f64> = image.pixels().map(|pixel| f64::from(pixel[0])).collect();
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
    }

    /// A plane wave at `angle` degrees and `freq` cycles/pixel.
    fn plane_wave(size: u32, freq: f64, angle_degrees: f64) -> GrayImage {
        let angle = angle_degrees.to_radians();
        let (dx, dy) = (angle.cos(), angle.sin());
        GrayImage::from_fn(size, size, |x, y| {
            let phase = 2.0 * std::f64::consts::PI * freq * (f64::from(x) * dx + f64::from(y) * dy);
            Luma([(128.0 + 100.0 * phase.cos()).clamp(0.0, 255.0) as u8])
        })
    }

    #[test]
    fn frequency_matches_fftfreq() {
        assert_eq!(frequency(0, 8), 0.0);
        assert_eq!(frequency(1, 8), 0.125);
        assert_eq!(frequency(4, 8), 0.5);
        assert_eq!(frequency(5, 8), -0.375);
        assert_eq!(frequency(7, 8), -0.125);
    }

    #[test]
    fn diagonal_bands_are_masked_including_the_perpendicular_ones() {
        for angle in [135.0, 315.0, 225.0, 45.0, 140.0, 130.0] {
            assert!(is_diagonal(angle), "{angle} should be attenuated");
        }
        for angle in [0.0, 90.0, 180.0, 270.0, 100.0, 124.0] {
            assert!(!is_diagonal(angle), "{angle} should be preserved");
        }
    }

    #[test]
    fn forward_and_inverse_round_trip() {
        let mut data: Vec<Complex<f64>> = (0..12 * 7)
            .map(|i| Complex::new(((i * 37) % 251) as f64 - 125.0, 0.0))
            .collect();
        let original = data.clone();
        forward(&mut data, 12, 7);
        inverse(&mut data, 12, 7);
        for (got, want) in data.iter().zip(original.iter()) {
            assert!(
                (got.re - want.re).abs() < 1e-9,
                "round trip drifted: {} vs {}",
                got.re,
                want.re
            );
        }
    }

    #[test]
    fn a_diagonal_high_frequency_wave_is_attenuated() {
        let image = plane_wave(64, 0.36, 135.0);
        let filtered = erase_gray(&image);
        assert!(
            deviation(&filtered) < 0.4 * deviation(&image),
            "expected strong attenuation: {} -> {}",
            deviation(&image),
            deviation(&filtered)
        );
    }

    #[test]
    fn an_axis_aligned_wave_is_preserved() {
        let image = plane_wave(64, 0.36, 0.0);
        let filtered = erase_gray(&image);
        assert!(
            deviation(&filtered) > 0.9 * deviation(&image),
            "expected the wave to survive: {} -> {}",
            deviation(&image),
            deviation(&filtered)
        );
    }

    #[test]
    fn a_low_frequency_diagonal_wave_is_preserved() {
        // Below the 0.30 cycles/pixel threshold, so it is not touched.
        let image = plane_wave(64, 0.12, 135.0);
        let filtered = erase_gray(&image);
        assert!(deviation(&filtered) > 0.9 * deviation(&image));
    }

    #[test]
    fn a_flat_image_survives_unchanged() {
        let image = GrayImage::from_pixel(32, 32, Luma([90]));
        let filtered = erase_gray(&image);
        assert!(filtered.pixels().all(|pixel| pixel[0].abs_diff(90) <= 1));
    }

    #[test]
    fn the_colour_path_keeps_the_size_and_chroma_shape() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(48, 32, |x, y| {
            Rgb([(x * 5) as u8, (y * 7) as u8, 128])
        }));
        let filtered =
            erase_rainbow_artifacts(&image, OutputColor::from_detection(Detected::Color, true));
        assert_eq!(filtered.dimensions(), (48, 32));
    }

    #[test]
    fn the_grayscale_path_returns_grayscale() {
        let image =
            DynamicImage::ImageRgb8(RgbImage::from_fn(16, 16, |x, _| Rgb([(x * 16) as u8; 3])));
        let filtered =
            erase_rainbow_artifacts(&image, OutputColor::from_detection(Detected::Gray, false));
        assert!(matches!(filtered, DynamicImage::ImageLuma8(_)));
    }

    #[test]
    fn the_colour_path_accepts_a_non_rgb_source() {
        // A non-RGB8 image forces the `to_rgb8()` conversion branch of the colour
        // path before filtering; a flat source is all-DC, so it survives unattenuated.
        let image = DynamicImage::ImageLuma8(GrayImage::from_pixel(16, 16, Luma([120])));
        let filtered =
            erase_rainbow_artifacts(&image, OutputColor::from_detection(Detected::Color, true));
        assert_eq!(filtered.dimensions(), (16, 16));
        let rgb = filtered.to_rgb8();
        for pixel in rgb.pixels() {
            assert!(
                pixel[0].abs_diff(120) <= 2
                    && pixel[1].abs_diff(120) <= 2
                    && pixel[2].abs_diff(120) <= 2,
                "a flat source is preserved, got {pixel:?}"
            );
        }
    }
}
