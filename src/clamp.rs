use crate::archive::{detect_archive_kind, extract_archive, get_images_from_source, ArchiveKind};
use crate::image_ops::{
    resize_image_by_total_pixels, resize_image_by_width, save_image_as_webp, split_image_iterative,
};
use anyhow::{anyhow, Context, Result};
use clap::ValueEnum;
use image::DynamicImage;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use walkdir::WalkDir;

/// WebP quality used when re-encoding clamped images.
const WEBP_QUALITY: f32 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Approach {
    Split,
    Resize,
    MaxWidth,
}

/// Everything that differs between clamp [`Approach`]es, expressed as data.
///
/// Each approach is a policy: a validation floor, labels for user-facing messages, the
/// metric that decides whether an image is oversized, and the transformation that brings
/// it back under the threshold. Bundling that policy into one value keeps the processing
/// pipeline free of `match approach` branches — callers just invoke the strategy.
#[derive(Clone, Copy)]
struct ClampStrategy {
    /// Smallest `size_threshold` this approach can make progress with.
    min_threshold: u64,
    /// Grouped rendering of `min_threshold` for error messages (e.g. `500,000`).
    min_threshold_label: &'static str,
    /// Approach label as it appears in error messages.
    name: &'static str,
    /// How large an image is under this approach: total pixels, or just the width.
    measure: fn(&DynamicImage) -> u64,
    /// Rewrite an image that exceeds the threshold into one or more replacements.
    clamp: fn(DynamicImage, u64) -> Vec<DynamicImage>,
}

impl Approach {
    /// Project this approach onto its executable policy.
    fn strategy(self) -> ClampStrategy {
        match self {
            Approach::Split => ClampStrategy {
                min_threshold: 500_000,
                min_threshold_label: "500,000",
                name: "'split' or 'resize'",
                measure: total_pixels,
                clamp: split_image_iterative,
            },
            Approach::Resize => ClampStrategy {
                min_threshold: 500_000,
                min_threshold_label: "500,000",
                name: "'split' or 'resize'",
                measure: total_pixels,
                clamp: |img, threshold| vec![resize_image_by_total_pixels(img, threshold)],
            },
            Approach::MaxWidth => ClampStrategy {
                min_threshold: 400,
                min_threshold_label: "400",
                name: "'max-width'",
                measure: |img| u64::from(img.width()),
                clamp: |img, threshold| vec![resize_image_by_width(img, threshold as u32)],
            },
        }
    }

    /// Reject thresholds too small for this approach to make progress with.
    fn validate_threshold(self, size_threshold: u64) -> Result<()> {
        let strategy = self.strategy();
        if size_threshold <= strategy.min_threshold {
            return Err(anyhow!(
                "For {} approach, size_threshold must be > {} pixels",
                strategy.name,
                strategy.min_threshold_label
            ));
        }
        Ok(())
    }
}

/// Total pixel count of an image.
fn total_pixels(img: &DynamicImage) -> u64 {
    u64::from(img.width()) * u64::from(img.height())
}

/// A single comic to clamp, together with the archive kind used to read it.
struct Chapter {
    path: PathBuf,
    kind: ArchiveKind,
}

impl Chapter {
    /// Name shown in progress and error messages.
    fn display_name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }

    /// Directory name the clamped result is written to: the stem for files, the name for
    /// directories.
    fn output_name(&self) -> &OsStr {
        let stem = if self.path.is_dir() {
            self.path.file_name()
        } else {
            self.path.file_stem()
        };
        stem.unwrap_or_else(|| self.path.as_os_str())
    }
}

/// Preserve the previous behaviour of tolerating read-only files on Windows when deleting.
pub fn remove_dir_all_force<P: AsRef<Path>>(path: P) -> io::Result<()> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(());
    }
    match fs::remove_dir_all(path) {
        Ok(_) => Ok(()),
        Err(e) => {
            #[cfg(windows)]
            {
                for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                    if let Ok(metadata) = entry.metadata() {
                        let mut permissions = metadata.permissions();
                        if permissions.readonly() {
                            permissions.set_readonly(false);
                            let _ = fs::set_permissions(entry.path(), permissions);
                        }
                    }
                }
                fs::remove_dir_all(path)
            }
            #[cfg(not(windows))]
            Err(e)
        }
    }
}

/// Resolve the input path into the list of comics to clamp, sorted in natural order.
fn collect_chapters(input_path: &Path, output_dir: &Path) -> Result<Vec<Chapter>> {
    if input_path.is_file() {
        let kind = detect_archive_kind(input_path)
            .ok_or_else(|| anyhow!("Unsupported file type for {}", input_path.display()))?;
        return Ok(vec![Chapter {
            path: input_path.to_path_buf(),
            kind,
        }]);
    }

    if input_path.is_dir() {
        let mut chapters = Vec::new();
        for entry in fs::read_dir(input_path)? {
            let path = entry?.path();
            if same_file::is_same_file(&path, output_dir).unwrap_or(false) {
                continue;
            }
            if let Some(kind) = detect_archive_kind(&path) {
                chapters.push(Chapter { path, kind });
            }
        }
        chapters.sort_by(|a, b| {
            natord::compare(
                &a.path.file_name().unwrap_or_default().to_string_lossy(),
                &b.path.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        return Ok(chapters);
    }

    Err(anyhow!(
        "Input path '{}' does not exist",
        input_path.display()
    ))
}

/// True when no image exceeds the threshold, so the source can be copied verbatim.
fn is_within_threshold(
    strategy: ClampStrategy,
    images: &[(String, DynamicImage)],
    threshold: u64,
) -> bool {
    images
        .iter()
        .all(|(_, img)| (strategy.measure)(img) < threshold)
}

/// Apply the strategy to every image, concatenating the results in reading order.
fn clamp_images(
    strategy: ClampStrategy,
    images: Vec<(String, DynamicImage)>,
    threshold: u64,
) -> Vec<DynamicImage> {
    images
        .into_iter()
        .flat_map(|(_, img)| (strategy.clamp)(img, threshold))
        .collect()
}

/// Encode each clamped image as a numbered WebP inside `output_chapter_dir`.
fn write_clamped_images(
    images: &[DynamicImage],
    output_chapter_dir: &Path,
    chapter_name: &str,
    bar: &ProgressBar,
) -> Result<()> {
    for (index, image) in images.iter().enumerate() {
        let filename = format!("{:03}.webp", index + 1);
        bar.set_message(format!("{} - {}", chapter_name, filename));
        save_image_as_webp(image, output_chapter_dir.join(&filename), WEBP_QUALITY)?;
        bar.inc(1);
    }
    bar.finish_and_clear();
    Ok(())
}

/// Clamp one comic, either copying it verbatim or re-encoding its images.
fn process_chapter(
    chapter: &Chapter,
    output_dir: &Path,
    strategy: ClampStrategy,
    threshold: u64,
    progress: &MultiProgress,
    overall_bar: &ProgressBar,
) -> Result<()> {
    let images = get_images_from_source(chapter.kind, &chapter.path)?;
    let chapter_name = chapter.display_name();
    let output_chapter_dir = output_dir.join(chapter.output_name());

    if same_file::is_same_file(&output_chapter_dir, output_dir).unwrap_or(false) {
        return Err(anyhow!(
            "Output chapter directory resolves to output directory: {}",
            output_dir.display()
        ));
    }

    if output_chapter_dir.exists() {
        remove_dir_all_force(&output_chapter_dir)?;
    }

    if is_within_threshold(strategy, &images, threshold) {
        // Already small enough: extract without re-encoding.
        return extract_archive(chapter.kind, &chapter.path, &output_chapter_dir);
    }

    fs::create_dir_all(&output_chapter_dir)?;
    let clamped = clamp_images(strategy, images, threshold);
    let bar = progress.insert_after(overall_bar, chapter_progress_bar(clamped.len() as u64));
    write_clamped_images(&clamped, &output_chapter_dir, &chapter_name, &bar)
}

fn overall_progress_bar(total: u64) -> ProgressBar {
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.green/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("valid template")
            .progress_chars("#>-"),
    );
    bar.set_message("Overall Progress");
    bar
}

fn chapter_progress_bar(total: u64) -> ProgressBar {
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("  -> {msg} [{bar:30.cyan/blue}] {pos}/{len}")
            .expect("valid template")
            .progress_chars("=>-"),
    );
    bar
}

pub fn run_clamp(
    input_path: &Path,
    output_dir: &Path,
    size_threshold: u64,
    approach: Approach,
    num_workers: usize,
) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create output directory {}", output_dir.display()))?;

    // Prevent saving into the same directory
    if same_file::is_same_file(input_path, output_dir).unwrap_or(false) {
        return Err(anyhow!(
            "Cannot save into the same directory you're reading from"
        ));
    }

    approach.validate_threshold(size_threshold)?;

    let chapters = collect_chapters(input_path, output_dir)?;
    if chapters.is_empty() {
        println!("No supported files or directories to process.");
        return Ok(());
    }

    let strategy = approach.strategy();
    let thread_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_workers)
        .build()
        .context("Failed to configure thread pool")?;

    let mp = MultiProgress::new();
    let overall_bar = mp.add(overall_progress_bar(chapters.len() as u64));

    thread_pool.install(|| {
        chapters.par_iter().for_each(|chapter| {
            let chapter_name = chapter.display_name();

            if let Err(err) = process_chapter(
                chapter,
                output_dir,
                strategy,
                size_threshold,
                &mp,
                &overall_bar,
            ) {
                mp.println(format!("Error processing {}: {}", chapter_name, err))
                    .ok();
            }

            overall_bar.inc(1);
        });
    });

    overall_bar.finish_with_message("Clamping complete.");
    println!("Clamping complete.");
    Ok(())
}
