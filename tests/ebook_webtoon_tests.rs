//! Phase 10 tests for `--webtoon` (AGENTS.md §15, Phase 10 exit criterion): a
//! webtoon source merges each chapter into a strip and splits it into virtual pages
//! at the device geometry, and the cover is omitted unless a custom cover is set.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::archive::{compress_archive, ArchiveKind};
use comic_book::cli::Cli;
use comic_book::ebook::options::Options;
use comic_book::ebook::{convert_source, progress};
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use tempfile::tempdir;

/// Resolve options from a `comic-book ebook` command line.
fn options(args: &[&str]) -> Result<Options> {
    let mut full = vec!["comic-book", "ebook", "book.cbz"];
    full.extend_from_slice(args);
    let cli = Cli::try_parse_from(full)?;
    match cli.command {
        comic_book::cli::Commands::Ebook(args) => Options::resolve(&args),
        _ => bail!("expected the ebook subcommand"),
    }
}

/// A checkerboard-banded webtoon strip, matching KCC's `comic2panel` fixture.
fn checker_strip(width: u32, segments: &[(u32, u32)]) -> DynamicImage {
    let total: u32 = segments.iter().map(|(height, _)| height).sum();
    let mut image = RgbImage::from_pixel(width, total, Rgb([255, 255, 255]));
    let mut y = 0;
    for &(height, content) in segments {
        if content > 0 {
            let top = y + (height - content) / 2;
            for py in top..top + content {
                for px in 40..width.saturating_sub(40) {
                    let dark = ((px / 6) + (py / 6)) % 2 == 0;
                    image.put_pixel(
                        px,
                        py,
                        if dark {
                            Rgb([0, 0, 0])
                        } else {
                            Rgb([255, 255, 255])
                        },
                    );
                }
            }
        }
        y += height;
    }
    DynamicImage::ImageRgb8(image)
}

/// The three-panel fixture: it packs into two virtual pages, 780 and 525 px tall.
const SEGMENTS: &[(u32, u32)] = &[
    (100, 0),
    (500, 400),
    (200, 0),
    (450, 350),
    (200, 0),
    (600, 500),
    (100, 0),
];

/// Write a single-strip source folder.
fn write_source(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    checker_strip(800, SEGMENTS).save(root.join("01.png"))?;
    Ok(())
}

/// Run the ebook pipeline for a source and return the output paths.
fn convert(source: &Path, args: &[&str]) -> Result<Vec<PathBuf>> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let options = options(args)?;
    convert_source(source, &options)
}

/// The entry names of a zip archive.
fn entry_names(path: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    (0..archive.len())
        .map(|index| Ok(archive.by_index(index)?.name().to_string()))
        .collect()
}

/// Read one entry's bytes from a zip archive.
fn entry_bytes(path: &Path, name: &str) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entry = archive
        .by_name(name)
        .with_context(|| format!("{name} present"))?;
    let mut data = Vec::new();
    entry.read_to_end(&mut data)?;
    Ok(data)
}

#[test]
fn webtoon_splits_a_strip_and_omits_the_cover() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_source(&source)?;

    let written = convert(&source, &["-f", "epub", "-p", "KV", "-w"])?;
    let names = entry_names(&written[0])?;

    // The strip becomes two virtual pages, named after the first page's stem.
    assert!(
        names.contains(&"OEBPS/Images/kcc-0001-0001-kcc-x.jpg".to_string()),
        "missing page 1: {names:?}"
    );
    assert!(
        names.contains(&"OEBPS/Images/kcc-0001-0002-kcc-x.jpg".to_string()),
        "missing page 2: {names:?}"
    );
    // No cover is built when webtoon mode has no custom cover (KCC's `makeBook`).
    assert!(
        !names.iter().any(|name| name.ends_with("cover.jpg")),
        "{names:?}"
    );

    // The virtual pages already fit within the KV profile, and webtoon mode disables
    // upscaling, so they are emitted at the strip width untouched.
    let one = image::load_from_memory(&entry_bytes(
        &written[0],
        "OEBPS/Images/kcc-0001-0001-kcc-x.jpg",
    )?)?;
    let two = image::load_from_memory(&entry_bytes(
        &written[0],
        "OEBPS/Images/kcc-0001-0002-kcc-x.jpg",
    )?)?;
    assert_eq!(one.dimensions(), (800, 780));
    assert_eq!(two.dimensions(), (800, 525));
    Ok(())
}

#[test]
fn no_processing_emits_the_merged_strip_pngs() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_source(&source)?;

    let written = convert(&source, &["-f", "epub", "-p", "KV", "-w", "-n"])?;
    let names = entry_names(&written[0])?;

    // `imgDirectoryProcessing` is skipped under `-n`, so the split PNGs are packaged.
    assert!(
        names.contains(&"OEBPS/Images/kcc-0001-0001.png".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"OEBPS/Images/kcc-0001-0002.png".to_string()),
        "{names:?}"
    );

    let one =
        image::load_from_memory(&entry_bytes(&written[0], "OEBPS/Images/kcc-0001-0001.png")?)?;
    let two =
        image::load_from_memory(&entry_bytes(&written[0], "OEBPS/Images/kcc-0001-0002.png")?)?;
    assert_eq!(one.dimensions(), (800, 780));
    assert_eq!(two.dimensions(), (800, 525));
    Ok(())
}

#[test]
fn a_custom_cover_is_kept_in_webtoon_mode() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_source(&source)?;
    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    // A sibling `Covers/` image is selected as the custom cover.
    fs::create_dir_all(tmp.path().join("Covers"))?;
    DynamicImage::ImageRgb8(RgbImage::from_pixel(400, 600, Rgb([30, 30, 200])))
        .save(tmp.path().join("Covers/cover.png"))?;

    let written = convert(&archive, &["-f", "epub", "-p", "KV", "-w"])?;
    let names = entry_names(&written[0])?;
    assert!(
        names.contains(&"OEBPS/Images/cover.jpg".to_string()),
        "custom cover missing: {names:?}"
    );
    // The pages are still the split webtoon virtual pages.
    assert!(names.contains(&"OEBPS/Images/kcc-0001-0001-kcc-x.jpg".to_string()));
    Ok(())
}
