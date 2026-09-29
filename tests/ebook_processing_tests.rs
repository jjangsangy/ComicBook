//! Tests for the core image pipeline: a fixture book processes into the expected
//! encoded pages — order classes, media types, dimensions and flags.

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::archive::{compress_archive, ArchiveKind};
use comic_book::cli::Cli;
use comic_book::ebook::input::load_tree;
use comic_book::ebook::model::{ComicTree, EncodedPage, MediaType, OrderClass};
use comic_book::ebook::options::Options;
use comic_book::ebook::processing::{detect_suboptimal_processing, process_tree};
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use std::fs;
use std::path::Path;
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

/// Write a solid-colour PNG, creating parent directories.
fn write_png(path: &Path, width: u32, height: u32, color: [u8; 3]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb(color))).save(path)?;
    Ok(())
}

/// Reduce an encoded page to the tuple the assertions care about.
fn summary(page: &EncodedPage) -> (OrderClass, MediaType, u32, u32, bool) {
    (
        page.order_class,
        page.media_type,
        page.size.width,
        page.size.height,
        page.flags.background.is_black(),
    )
}

#[test]
fn fixture_book_snapshot() -> Result<()> {
    // Keep the progress bar quiet regardless of how `cargo nextest` was invoked.
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("01-normal.png"), 100, 150, [10, 10, 10])?;
    write_png(&source.join("02-split.png"), 300, 200, [255, 255, 255])?;
    write_png(&source.join("03-wide.png"), 500, 200, [255, 255, 255])?;
    write_png(&source.join("04-black.png"), 100, 150, [0, 0, 0])?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let mut tree = load_tree(&archive, &options(&[])?)?;
    // A non-Kindle profile keeps monochrome pages as JPEG rather than GIF.
    let options = options(&["-p", "KoE"])?;
    let book = process_tree(&mut tree, &options)?;

    assert_eq!(book.page_count, 5, "one page splits into two");

    let pages: Vec<&EncodedPage> = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter())
        .collect();
    assert_eq!(book.chapters.len(), 1);
    assert!(
        book.chapters[0].name.is_root(),
        "all pages are in the root chapter"
    );

    let expected = [
        // A page that already fits the profile is left untouched.
        (OrderClass::Normal, MediaType::Jpeg, 100, 150, true),
        // The 1.5:1 spread is bisected into two reading-order halves.
        (OrderClass::SplitLeft, MediaType::Jpeg, 150, 200, false),
        (OrderClass::SplitRight, MediaType::Jpeg, 150, 200, false),
        // The 2.5:1 spread exceeds the bisect threshold, so it only rotates.
        (OrderClass::RotateLast, MediaType::Jpeg, 200, 500, false),
        // A black page is flagged so the output can colour its margins.
        (OrderClass::Normal, MediaType::Jpeg, 100, 150, true),
    ];
    let actual: Vec<_> = pages.iter().map(|page| summary(page)).collect();
    assert_eq!(actual, expected);

    let names: Vec<&str> = pages.iter().map(|page| page.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "01-normal-cb-x.jpg",
            "02-split-cb-b.jpg",
            "02-split-cb-c.jpg",
            "03-wide-cb-d.jpg",
            "04-black-cb-x.jpg",
        ]
    );

    // Every payload is a decodable image of the advertised size.
    for page in &pages {
        let decoded = image::load_from_memory(&page.bytes)
            .with_context(|| format!("{} is not decodable", page.name))?;
        assert_eq!(decoded.dimensions(), (page.size.width, page.size.height));
    }
    Ok(())
}

/// Processing decodes each page in flight and releases its pixels afterwards, so
/// the source tree never retains a decoded book (see docs/architecture.md).
#[test]
fn processing_releases_decoded_pixels() -> Result<()> {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    for index in 0..6u32 {
        write_png(
            &source.join(format!("page{index:02}.png")),
            80,
            120,
            [20, 40, 60],
        )?;
    }
    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let mut tree = load_tree(&archive, &options(&[])?)?;
    for chapter in &tree.chapters {
        for page in &chapter.pages {
            assert!(page.decoded().is_none(), "ingest decoded a page");
        }
    }

    let book = process_tree(&mut tree, &options(&["-p", "KoE"])?)?;
    assert_eq!(book.page_count, 6);

    // The decoded pixels were released as each page was encoded.
    for chapter in &tree.chapters {
        for page in &chapter.pages {
            assert!(
                page.decoded().is_none(),
                "page {} retained its decoded pixels",
                page.rel_path
            );
        }
    }
    Ok(())
}

#[test]
fn no_processing_copies_source_bytes_verbatim() -> Result<()> {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("01-page.png"), 100, 150, [10, 10, 10])?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let mut tree = load_tree(&archive, &options(&[])?)?;
    let pristine = tree.chapters[0].pages[0]
        .source_bytes()
        .context("the source page keeps its bytes")?
        .to_vec();

    let options = options(&["-p", "KoE", "--no-processing"])?;
    let book = process_tree(&mut tree, &options)?;

    let pages: Vec<&EncodedPage> = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter())
        .collect();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_type, MediaType::Png);
    assert_eq!(pages[0].order_class, OrderClass::Normal);
    assert_eq!(pages[0].bytes, pristine, "the source bytes are untouched");
    Ok(())
}

#[test]
fn no_processing_keeps_the_sanitized_name() -> Result<()> {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("01-page.png"), 100, 150, [10, 10, 10])?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let options = options(&["-p", "KoE", "--no-processing"])?;
    let mut tree = load_tree(&archive, &options)?;
    comic_book::ebook::naming::sanitize_tree(&mut tree, &options);
    let book = process_tree(&mut tree, &options)?;

    let names: Vec<&str> = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter().map(|page| page.name.as_str()))
        .collect();
    // `--no-processing` emits the sanitized name without an order suffix.
    assert_eq!(names, vec!["cb-0001.png"]);
    Ok(())
}

/// Load a folder of solid PNGs as a source tree (page names drive the warnings).
fn tree_of(root: &Path, pages: &[(&str, u32, u32)]) -> Result<ComicTree> {
    let source = root.join("source");
    for (name, width, height) in pages {
        write_png(&source.join(name), *width, *height, [128, 128, 128])?;
    }
    load_tree(&source, &options(&[])?)
}

#[test]
fn cb_made_sources_warn_about_quality_loss() -> Result<()> {
    let tmp = tempdir()?;
    let tree = tree_of(tmp.path(), &[("page-cb-x.png", 2000, 3000)])?;

    let warnings = detect_suboptimal_processing(&tree, &options(&["--stretch"])?);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("created by cb"), "{warnings:?}");
    Ok(())
}

#[test]
fn small_images_warn_unless_upscaled_or_scribe() -> Result<()> {
    let tmp = tempdir()?;
    let tree = tree_of(tmp.path(), &[("page1.png", 10, 10), ("page2.png", 20, 20)])?;

    let warnings = detect_suboptimal_processing(&tree, &options(&[])?);
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("smaller than target device resolution"),
        "{warnings:?}"
    );

    assert!(detect_suboptimal_processing(&tree, &options(&["--upscale"])?).is_empty());
    assert!(detect_suboptimal_processing(&tree, &options(&["-p", "KS"])?).is_empty());
    Ok(())
}

#[test]
fn pages_larger_than_the_device_do_not_warn() -> Result<()> {
    let tmp = tempdir()?;
    // KV is 1072x1448; both pages exceed it.
    let tree = tree_of(
        tmp.path(),
        &[("page1.png", 1200, 1500), ("page2.png", 2000, 3000)],
    )?;
    assert!(detect_suboptimal_processing(&tree, &options(&[])?).is_empty());
    Ok(())
}

#[test]
fn a_page_smaller_in_only_one_dimension_does_not_count() -> Result<()> {
    let tmp = tempdir()?;
    // 2000x10 is wider than KV but shorter, so it is not "smaller".
    let tree = tree_of(tmp.path(), &[("page1.png", 2000, 10)])?;
    assert!(detect_suboptimal_processing(&tree, &options(&[])?).is_empty());
    Ok(())
}
