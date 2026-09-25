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

/// Per-approach clamping policy, expressed as methods rather than a `match` at each call
/// site. Callers invoke `measure`/`clamp` without ever asking which approach is in play,
/// so the processing pipeline carries no branch-per-image logic.
///
/// `split` and `resize` enforce an identical policy, so their cases collapse into a single
/// `Split | Resize` arm. That states each shared fact — the floor, the metric, the label —
/// exactly once instead of duplicating it per approach, where the copies could drift.
impl Approach {
    /// Smallest `size_threshold` this approach can make progress with.
    fn min_threshold(self) -> u64 {
        match self {
            Approach::Split | Approach::Resize => 500_000,
            Approach::MaxWidth => 400,
        }
    }

    /// How the rule is named in error messages. A single rule can cover several
    /// approaches, so the label names every approach it applies to.
    fn rule_label(self) -> &'static str {
        match self {
            Approach::Split | Approach::Resize => "'split' or 'resize'",
            Approach::MaxWidth => "'max-width'",
        }
    }

    /// How large an image is under this approach: total pixels, or just the width.
    fn measure(self, img: &DynamicImage) -> u64 {
        match self {
            Approach::Split | Approach::Resize => total_pixels(img),
            Approach::MaxWidth => u64::from(img.width()),
        }
    }

    /// Rewrite an image that exceeds the threshold into one or more replacements.
    fn clamp(self, img: DynamicImage, threshold: u64) -> Vec<DynamicImage> {
        match self {
            Approach::Split => split_image_iterative(img, threshold),
            Approach::Resize => vec![resize_image_by_total_pixels(img, threshold)],
            Approach::MaxWidth => vec![resize_image_by_width(img, threshold as u32)],
        }
    }

    /// Reject thresholds too small for this approach to make progress with.
    fn validate_threshold(self, size_threshold: u64) -> Result<()> {
        if size_threshold <= self.min_threshold() {
            return Err(anyhow!(
                "For {} approach, size_threshold must be > {} pixels",
                self.rule_label(),
                group_thousands(self.min_threshold())
            ));
        }
        Ok(())
    }
}

/// Render an integer with thousands separators for user-facing messages (e.g. `500,000`).
/// Kept bespoke (AGENTS.md §5.3, "genuinely trivial").
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.char_indices() {
        if idx > 0 && (digits.len() - idx).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
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

/// Preserve the previous behaviour of tolerating read-only files on Windows when deleting
/// (kept bespoke, §5.3).
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
    approach: Approach,
    images: &[(String, DynamicImage)],
    threshold: u64,
) -> bool {
    images
        .iter()
        .all(|(_, img)| approach.measure(img) < threshold)
}

/// Apply the approach to every image, concatenating the results in reading order.
fn clamp_images(
    approach: Approach,
    images: Vec<(String, DynamicImage)>,
    threshold: u64,
) -> Vec<DynamicImage> {
    images
        .into_iter()
        .flat_map(|(_, img)| approach.clamp(img, threshold))
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
    approach: Approach,
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

    if is_within_threshold(approach, &images, threshold) {
        // Already small enough: extract without re-encoding.
        return extract_archive(chapter.kind, &chapter.path, &output_chapter_dir);
    }

    fs::create_dir_all(&output_chapter_dir)?;
    let clamped = clamp_images(approach, images, threshold);
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
                approach,
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
