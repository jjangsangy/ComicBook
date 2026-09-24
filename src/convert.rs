use crate::archive::{detect_archive_kind, parse_target_extension, ArchiveKind};
use crate::image_ops::is_image_file;
use anyhow::{anyhow, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Copy, Clone)]
struct TargetFormat<'a> {
    kind: ArchiveKind,
    ext: &'a str,
}

struct ConvertTask {
    source_path: PathBuf,
    source_kind: ArchiveKind,
    dest_path: PathBuf,
}

fn contains_direct_image_files<P: AsRef<Path>>(dir: P) -> bool {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if (path.is_file() || path.is_symlink()) && is_image_file(&path) {
                return true;
            }
        }
    }
    false
}

fn contains_archive_files<P: AsRef<Path>>(dir: P) -> bool {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(kind) = detect_archive_kind(&path) {
                    if kind != ArchiveKind::Directory {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn contains_any_images<P: AsRef<Path>>(dir: P) -> bool {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if (path.is_file() || entry.file_type().is_symlink()) && is_image_file(path) {
            return true;
        }
    }
    false
}

fn count_comic_subdirs<P: AsRef<Path>>(dir: P) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() && contains_any_images(&path) {
                count += 1;
            }
        }
    }
    count
}

fn is_standalone_comic_dir<P: AsRef<Path>>(dir: P) -> bool {
    let dir = dir.as_ref();
    contains_direct_image_files(dir)
        || (contains_any_images(dir)
            && !contains_archive_files(dir)
            && count_comic_subdirs(dir) <= 1)
}

fn sorted_dir_entries(dir: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("Failed to read directory {}: {}", dir.display(), e);
            return Vec::new();
        }
    };

    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .collect();
    paths.sort_by(|a, b| {
        let name_a = a.file_name().unwrap_or_default().to_string_lossy();
        let name_b = b.file_name().unwrap_or_default().to_string_lossy();
        natord::compare(&name_a, &name_b)
    });
    paths
}

fn get_dest_path(
    source_path: &Path,
    source_kind: ArchiveKind,
    target: TargetFormat,
) -> Result<PathBuf> {
    let stem = match source_kind {
        ArchiveKind::Directory => source_path
            .file_name()
            .ok_or_else(|| anyhow!("Invalid directory name for {}", source_path.display()))?
            .to_string_lossy(),
        _ => source_path
            .file_stem()
            .ok_or_else(|| anyhow!("Invalid file stem for {}", source_path.display()))?
            .to_string_lossy(),
    };

    let target_filename = match target.kind {
        ArchiveKind::Directory => stem.to_string(),
        _ => format!("{}.{}", stem, target.ext),
    };

    let final_dest = match source_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(&target_filename),
        _ => PathBuf::from(&target_filename),
    };

    Ok(final_dest)
}

fn is_already_target_format(source_kind: ArchiveKind, target_kind: ArchiveKind) -> bool {
    match (source_kind, target_kind) {
        (ArchiveKind::Directory, ArchiveKind::Directory) => true,
        (ArchiveKind::Directory, _) | (_, ArchiveKind::Directory) => false,
        (src, tgt) => src == tgt,
    }
}

fn print_skip_already_target(path: &Path, target: TargetFormat) {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if target.kind == ArchiveKind::Directory {
        println!("Skipping {}: already a directory.", name);
    } else {
        println!("Skipping {}: already a .{} file.", name, target.ext);
    }
}

fn collect_archive_file_task(
    path: &Path,
    kind: ArchiveKind,
    target: TargetFormat,
    tasks: &mut Vec<ConvertTask>,
) -> Result<()> {
    if is_already_target_format(kind, target.kind) {
        print_skip_already_target(path, target);
        return Ok(());
    }

    let dest_path = get_dest_path(path, kind, target)?;
    tasks.push(ConvertTask {
        source_path: path.to_path_buf(),
        source_kind: kind,
        dest_path,
    });
    Ok(())
}

fn collect_tasks_for_dir_target(
    dir: &Path,
    target: TargetFormat,
    tasks: &mut Vec<ConvertTask>,
) -> Result<()> {
    if !contains_archive_files(dir) {
        print_skip_already_target(dir, target);
        return Ok(());
    }

    for entry_path in sorted_dir_entries(dir) {
        if entry_path.is_dir() {
            print_skip_already_target(&entry_path, target);
        } else if let Some(kind) = detect_archive_kind(&entry_path) {
            collect_archive_file_task(&entry_path, kind, target, tasks)?;
        }
    }

    Ok(())
}

fn collect_tasks_for_archive_target(
    dir: &Path,
    target: TargetFormat,
    tasks: &mut Vec<ConvertTask>,
) -> Result<()> {
    if is_standalone_comic_dir(dir) {
        let dest_path = get_dest_path(dir, ArchiveKind::Directory, target)?;
        tasks.push(ConvertTask {
            source_path: dir.to_path_buf(),
            source_kind: ArchiveKind::Directory,
            dest_path,
        });
        return Ok(());
    }

    for entry_path in sorted_dir_entries(dir) {
        if entry_path.is_file() {
            if let Some(kind) = detect_archive_kind(&entry_path) {
                collect_archive_file_task(&entry_path, kind, target, tasks)?;
            }
        } else if entry_path.is_dir() && contains_any_images(&entry_path) {
            let dest_path = get_dest_path(&entry_path, ArchiveKind::Directory, target)?;
            tasks.push(ConvertTask {
                source_path: entry_path,
                source_kind: ArchiveKind::Directory,
                dest_path,
            });
        }
    }

    Ok(())
}

fn collect_dir_tasks(dir: &Path, target: TargetFormat, tasks: &mut Vec<ConvertTask>) -> Result<()> {
    match target.kind {
        ArchiveKind::Directory => collect_tasks_for_dir_target(dir, target, tasks),
        _ => collect_tasks_for_archive_target(dir, target, tasks),
    }
}

fn collect_path_tasks(
    path: &Path,
    target: TargetFormat,
    tasks: &mut Vec<ConvertTask>,
) -> Result<()> {
    if !path.exists() {
        eprintln!("Warning: '{}' does not exist, skipping.", path.display());
        return Ok(());
    }

    if path.is_file() {
        // Detect once and reuse the kind instead of re-reading magic bytes downstream.
        match detect_archive_kind(path) {
            Some(kind) => collect_archive_file_task(path, kind, target, tasks)?,
            None => {
                eprintln!(
                    "Warning: '{}' is not a supported comic archive format, skipping.",
                    path.display()
                );
            }
        }
    } else if path.is_dir() {
        collect_dir_tasks(path, target, tasks)?;
    }

    Ok(())
}

fn execute_conversion_tasks(tasks: &[ConvertTask], target_kind: ArchiveKind) {
    let pb = ProgressBar::new(tasks.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .expect("valid template")
            .progress_chars("#>-"),
    );

    let should_strip = target_kind == ArchiveKind::Directory;
    // One reusable buffer for the whole batch: each archive's entry data is streamed through this
    // same allocation, so converting a directory of files never grows the footprint beyond the
    // largest single entry.
    let mut scratch = Vec::new();
    for task in tasks {
        let file_name = task
            .source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        pb.set_message(format!("Converting {}", file_name));

        if let Err(e) = crate::archive::ops::convert_archive_ext_with_scratch(
            task.source_kind,
            &task.source_path,
            target_kind,
            &task.dest_path,
            should_strip,
            &mut scratch,
        ) {
            pb.println(format!("Error processing {}: {}", file_name, e));
        }

        pb.inc(1);
    }

    pb.finish_with_message("Conversion complete.");
    println!("Conversion complete.");
}

pub fn run_convert(paths: &[PathBuf], target_ext_raw: &str) -> Result<()> {
    if paths.is_empty() {
        return Err(anyhow!("No files or directories specified for conversion."));
    }

    let (target_ext_clean, target_kind) = parse_target_extension(target_ext_raw).ok_or_else(|| {
        anyhow!(
            "Target extension '{}' is not supported. Supported: cbz, zip, cbr, rar, cb7, 7z, cbt, tar, dir",
            target_ext_raw
        )
    })?;

    let target = TargetFormat {
        kind: target_kind,
        ext: target_ext_clean,
    };

    let mut tasks = Vec::new();
    for p in paths {
        collect_path_tasks(p, target, &mut tasks)?;
    }

    if tasks.is_empty() {
        println!("No files to convert.");
        return Ok(());
    }

    execute_conversion_tasks(&tasks, target.kind);
    Ok(())
}
