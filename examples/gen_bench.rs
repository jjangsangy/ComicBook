//! Benchmark-input generator (development aid).
//!
//! Writes a synthetic manga-style `.cbz` so the conversion pipeline can be timed
//! and profiled on realistic, full-resolution pages without committing large
//! fixtures. Not part of the shipped binary.
//!
//! ```sh
//! cargo run --release --example gen_bench -- target/bench/bench.cbz 40 1600 2400
//! ```

use std::env;
use std::fs::File;
use std::io::BufWriter;

use image::{DynamicImage, GrayImage, Luma, Rgb, RgbImage};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

/// A deterministic xorshift so repeated runs produce identical inputs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: u32) -> u32 {
        (self.next() % u64::from(bound)) as u32
    }
}

/// A grayscale line-art page: ink blobs, panel borders and a soft gradient.
fn gray_page(width: u32, height: u32, seed: u64) -> GrayImage {
    let mut rng = Rng(seed | 1);
    let mut image = GrayImage::from_fn(width, height, |x, y| {
        // Diagonal gradient plus mild dither noise, so the page is not flat.
        let base = 210 + (x * 20 / width.max(1)) as i32 - (y * 30 / height.max(1)) as i32;
        let noise = (rng.next() % 25) as i32 - 12;
        Luma([(base + noise).clamp(0, 255) as u8])
    });

    // Panel frames.
    for _ in 0..8 {
        let x0 = rng.below(width / 3);
        let y0 = rng.below(height / 3);
        let w = width / 4 + rng.below(width / 3);
        let h = height / 5 + rng.below(height / 3);
        for x in x0..(x0 + w).min(width) {
            image.put_pixel(x, y0.min(height - 1), Luma([0]));
            image.put_pixel(x, (y0 + h).min(height - 1), Luma([0]));
        }
        for y in y0..(y0 + h).min(height) {
            image.put_pixel(x0.min(width - 1), y, Luma([0]));
            image.put_pixel((x0 + w).min(width - 1), y, Luma([0]));
        }
    }

    // Filled ink blobs.
    for _ in 0..60 {
        let cx = rng.below(width);
        let cy = rng.below(height);
        let r = 4 + rng.below(40);
        for y in cy.saturating_sub(r)..(cy + r).min(height) {
            for x in cx.saturating_sub(r)..(cx + r).min(width) {
                let dx = x as i64 - cx as i64;
                let dy = y as i64 - cy as i64;
                if dx * dx + dy * dy <= (r * r) as i64 {
                    image.put_pixel(x, y, Luma([0]));
                }
            }
        }
    }

    image
}

/// A colour splash page (exercises `colorCheck` and the colour encode path).
fn color_page(width: u32, height: u32, seed: u64) -> RgbImage {
    let mut rng = Rng(seed | 1);
    RgbImage::from_fn(width, height, |x, y| {
        let r = (x * 255 / width.max(1)) as u8;
        let g = (y * 255 / height.max(1)) as u8;
        let b = ((x + y) * 255 / (width + height).max(1)) as u8;
        let noise = (rng.next() % 60) as u8;
        Rgb([r.saturating_add(noise), g, b.saturating_sub(noise / 2)])
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "target/bench/bench.cbz".to_string());
    let pages: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    let width: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1600);
    let height: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(2400);

    if let Some(parent) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = File::create(&out)?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    for index in 0..pages {
        let name = format!("page-{index:04}.jpg");
        let bytes = if index % 10 == 3 {
            let image = DynamicImage::ImageRgb8(color_page(width, height, u64::from(index) + 7));
            let mut buf = Vec::new();
            image.write_to(
                &mut std::io::Cursor::new(&mut buf),
                image::ImageFormat::Jpeg,
            )?;
            buf
        } else {
            let image = DynamicImage::ImageLuma8(gray_page(width, height, u64::from(index) + 7));
            let mut buf = Vec::new();
            image.write_to(
                &mut std::io::Cursor::new(&mut buf),
                image::ImageFormat::Jpeg,
            )?;
            buf
        };
        zip.start_file(&name, options)?;
        std::io::Write::write_all(&mut zip, &bytes)?;
    }

    zip.finish()?;
    println!("wrote {out}: {pages} pages at {width}x{height}");
    Ok(())
}
