//! Phase 2 tests for the core image pipeline: a fixture book processes into the
//! expected encoded pages — order classes, media types, dimensions and flags
//! (AGENTS.md §15, Phase 2 exit criterion).

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
fn options(args: &[&str]) -> Options {
    let mut full = vec!["comic-book", "ebook", "book.cbz"];
    full.extend_from_slice(args);
    let cli = Cli::try_parse_from(full).expect("CLI parses");
    match cli.command {
        comic_book::cli::Commands::Ebook(args) => Options::resolve(&args).expect("resolves"),
        _ => unreachable!(),
    }
}

/// Write a solid-colour PNG, creating parent directories.
fn write_png(path: &Path, width: u32, height: u32, color: [u8; 3]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb(color)))
        .save(path)
        .unwrap();
}

/// Reduce an encoded page to the tuple the assertions care about.
fn summary(page: &EncodedPage) -> (OrderClass, MediaType, u32, u32, bool) {
    (
        page.order_class,
        page.media_type,
        page.width,
        page.height,
        page.flags.black_background,
    )
}

#[test]
fn fixture_book_snapshot() {
    // Keep the progress bar quiet regardless of how `cargo nextest` was invoked.
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir().unwrap();
    let source = tmp.path().join("source");
    write_png(&source.join("01-normal.png"), 100, 150, [10, 10, 10]);
    write_png(&source.join("02-split.png"), 300, 200, [255, 255, 255]);
    write_png(&source.join("03-wide.png"), 500, 200, [255, 255, 255]);
    write_png(&source.join("04-black.png"), 100, 150, [0, 0, 0]);

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive).unwrap();

    let mut tree = load_tree(&archive, &options(&[])).unwrap();
    // A non-Kindle profile keeps monochrome pages as JPEG rather than GIF.
    let options = options(&["-p", "KoE"]);
    let book = process_tree(&mut tree, &options).unwrap();

    assert_eq!(book.page_count, 5, "one page splits into two");

    let pages: Vec<&EncodedPage> = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter())
        .collect();
    assert_eq!(book.chapters.len(), 1);
    assert_eq!(
        book.chapters[0].name, "",
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
            "01-normal-kcc-x.jpg",
            "02-split-kcc-b.jpg",
            "02-split-kcc-c.jpg",
            "03-wide-kcc-d.jpg",
            "04-black-kcc-x.jpg",
        ]
    );

    // Every payload is a decodable image of the advertised size.
    for page in &pages {
        let decoded = image::load_from_memory(&page.bytes)
            .unwrap_or_else(|err| panic!("{} is not decodable: {err}", page.name));
        assert_eq!(decoded.dimensions(), (page.width, page.height));
    }
}

#[test]
fn no_processing_copies_source_bytes_verbatim() {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir().unwrap();
    let source = tmp.path().join("source");
    write_png(&source.join("01-page.png"), 100, 150, [10, 10, 10]);

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive).unwrap();

    let mut tree = load_tree(&archive, &options(&[])).unwrap();
    let pristine = tree.chapters[0].pages[0].raw.clone().unwrap();

    let options = options(&["-p", "KoE", "--no-processing"]);
    let book = process_tree(&mut tree, &options).unwrap();

    let pages: Vec<&EncodedPage> = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter())
        .collect();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].media_type, MediaType::Png);
    assert_eq!(pages[0].order_class, OrderClass::Normal);
    assert_eq!(pages[0].bytes, pristine, "the source bytes are untouched");
}

#[test]
fn no_processing_keeps_the_sanitized_name() {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir().unwrap();
    let source = tmp.path().join("source");
    write_png(&source.join("01-page.png"), 100, 150, [10, 10, 10]);

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive).unwrap();

    let options = options(&["-p", "KoE", "--no-processing"]);
    let mut tree = load_tree(&archive, &options).unwrap();
    comic_book::ebook::naming::sanitize_tree(&mut tree, &options);
    let book = process_tree(&mut tree, &options).unwrap();

    let names: Vec<&str> = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter().map(|page| page.name.as_str()))
        .collect();
    // `--no-processing` emits the sanitized name without an order suffix.
    assert_eq!(names, vec!["kcc-0001.png"]);
}

/// Load a folder of solid PNGs as a source tree (page names drive the warnings).
fn tree_of(root: &Path, pages: &[(&str, u32, u32)]) -> ComicTree {
    let source = root.join("source");
    for (name, width, height) in pages {
        write_png(&source.join(name), *width, *height, [128, 128, 128]);
    }
    load_tree(&source, &options(&[])).unwrap()
}

#[test]
fn kcc_made_sources_warn_about_quality_loss() {
    let tmp = tempdir().unwrap();
    let tree = tree_of(tmp.path(), &[("page-kcc-x.png", 2000, 3000)]);

    let warnings = detect_suboptimal_processing(&tree, &options(&["--stretch"]));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("created by KCC"), "{warnings:?}");
}

#[test]
fn small_images_warn_unless_upscaled_or_scribe() {
    let tmp = tempdir().unwrap();
    let tree = tree_of(tmp.path(), &[("page1.png", 10, 10), ("page2.png", 20, 20)]);

    let warnings = detect_suboptimal_processing(&tree, &options(&[]));
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].contains("smaller than target device resolution"),
        "{warnings:?}"
    );

    assert!(detect_suboptimal_processing(&tree, &options(&["--upscale"])).is_empty());
    assert!(detect_suboptimal_processing(&tree, &options(&["-p", "KS"])).is_empty());
}

#[test]
fn pages_larger_than_the_device_do_not_warn() {
    let tmp = tempdir().unwrap();
    // KV is 1072x1448; both pages exceed it.
    let tree = tree_of(
        tmp.path(),
        &[("page1.png", 1200, 1500), ("page2.png", 2000, 3000)],
    );
    assert!(detect_suboptimal_processing(&tree, &options(&[])).is_empty());
}

#[test]
fn a_page_smaller_in_only_one_dimension_does_not_count() {
    let tmp = tempdir().unwrap();
    // 2000x10 is wider than KV but shorter, so it is not "smaller".
    let tree = tree_of(tmp.path(), &[("page1.png", 2000, 10)]);
    assert!(detect_suboptimal_processing(&tree, &options(&[])).is_empty());
}
