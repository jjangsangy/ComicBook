use crate::archive::{detect_archive_kind, extract_archive, get_images_from_source, ArchiveKind};
use crate::image_ops::{
    resize_image_by_total_pixels, resize_image_by_width, save_image_as_webp, split_image_iterative,
};
use anyhow::{anyhow, Context, Result};
use clap::ValueEnum;
use image::DynamicImage;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Approach {
    Split,
    Resize,
    MaxWidth,
}

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

    match approach {
        Approach::Split | Approach::Resize => {
            if size_threshold <= 500_000 {
                return Err(anyhow!(
                    "For 'split' or 'resize' approach, size_threshold must be > 500,000 pixels"
                ));
            }
        }
        Approach::MaxWidth => {
            if size_threshold <= 400 {
                return Err(anyhow!(
                    "For 'max-width' approach, size_threshold must be > 400 pixels"
                ));
            }
        }
    }

    let chapters_to_process: Vec<(PathBuf, ArchiveKind)> = if input_path.is_file() {
        let kind = detect_archive_kind(input_path)
            .ok_or_else(|| anyhow!("Unsupported file type for {}", input_path.display()))?;
        vec![(input_path.to_path_buf(), kind)]
    } else if input_path.is_dir() {
        let mut entries = Vec::new();
        for entry in fs::read_dir(input_path)? {
            let entry = entry?;
            let path = entry.path();
            if same_file::is_same_file(&path, output_dir).unwrap_or(false) {
                continue;
            }
            if let Some(kind) = detect_archive_kind(&path) {
                entries.push((path, kind));
            }
        }
        entries.sort_by(|a, b| {
            let name_a = a.0.file_name().unwrap_or_default().to_string_lossy();
            let name_b = b.0.file_name().unwrap_or_default().to_string_lossy();
            natord::compare(&name_a, &name_b)
        });
        entries
    } else {
        return Err(anyhow!(
            "Input path '{}' does not exist",
            input_path.display()
        ));
    };

    if chapters_to_process.is_empty() {
        println!("No supported files or directories to process.");
        return Ok(());
    }

    let thread_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(num_workers)
        .build()
        .context("Failed to configure thread pool")?;

    let mp = MultiProgress::new();
    let overall_bar = mp.add(ProgressBar::new(chapters_to_process.len() as u64));
    overall_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.green/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("valid template")
            .progress_chars("#>-"),
    );
    overall_bar.set_message("Overall Progress");

    thread_pool.install(|| {
        chapters_to_process
            .par_iter()
            .for_each(|(chapter_path, kind)| {
                let chapter_name = chapter_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let result: Result<()> = (|| {
                    let original_images = get_images_from_source(*kind, chapter_path)?;
                    let chapter_stem = if chapter_path.is_dir() {
                        chapter_path
                            .file_name()
                            .unwrap_or_else(|| chapter_path.as_os_str())
                            .to_string_lossy()
                    } else {
                        chapter_path
                            .file_stem()
                            .unwrap_or_else(|| chapter_path.as_os_str())
                            .to_string_lossy()
                    };
                    let output_chapter_dir = output_dir.join(chapter_stem.as_ref());

                    if same_file::is_same_file(&output_chapter_dir, output_dir).unwrap_or(false) {
                        return Err(anyhow!(
                            "Output chapter directory resolves to output directory: {}",
                            output_dir.display()
                        ));
                    }

                    if output_chapter_dir.exists() {
                        remove_dir_all_force(&output_chapter_dir)?;
                    }

                    let max_dimension: u64 = match approach {
                        Approach::Split | Approach::Resize => original_images
                            .iter()
                            .map(|(_, img)| (img.width() as u64) * (img.height() as u64))
                            .max()
                            .unwrap_or(0),
                        Approach::MaxWidth => original_images
                            .iter()
                            .map(|(_, img)| img.width() as u64)
                            .max()
                            .unwrap_or(0),
                    };

                    if max_dimension < size_threshold {
                        // Extract directly without re-encoding
                        extract_archive(*kind, chapter_path, &output_chapter_dir)?;
                    } else {
                        fs::create_dir_all(&output_chapter_dir)?;

                        // Process images
                        let mut processed_images: Vec<DynamicImage> = Vec::new();
                        for (_name, img) in original_images {
                            match approach {
                                Approach::Split => {
                                    let parts = split_image_iterative(img, size_threshold);
                                    processed_images.extend(parts);
                                }
                                Approach::Resize => {
                                    let resized = resize_image_by_total_pixels(img, size_threshold);
                                    processed_images.push(resized);
                                }
                                Approach::MaxWidth => {
                                    let resized = resize_image_by_width(img, size_threshold as u32);
                                    processed_images.push(resized);
                                }
                            }
                        }

                        let sub_bar = mp.insert_after(
                            &overall_bar,
                            ProgressBar::new(processed_images.len() as u64),
                        );
                        sub_bar.set_style(
                            ProgressStyle::default_bar()
                                .template("  -> {msg} [{bar:30.cyan/blue}] {pos}/{len}")
                                .expect("valid template")
                                .progress_chars("=>-"),
                        );

                        for (idx, img) in processed_images.iter().enumerate() {
                            let filename = format!("{:03}.webp", idx + 1);
                            sub_bar.set_message(format!("{} - {}", chapter_name, filename));
                            let out_file = output_chapter_dir.join(&filename);
                            save_image_as_webp(img, &out_file, 90.0)?;
                            sub_bar.inc(1);
                        }

                        sub_bar.finish_and_clear();
                    }

                    Ok(())
                })();

                match result {
                    Ok(_) => {
                        overall_bar.inc(1);
                    }
                    Err(e) => {
                        overall_bar.inc(1);
                        mp.println(format!("Error processing {}: {}", chapter_name, e))
                            .ok();
                    }
                }
            });
    });

    overall_bar.finish_with_message("Clamping complete.");
    println!("Clamping complete.");
    Ok(())
}
