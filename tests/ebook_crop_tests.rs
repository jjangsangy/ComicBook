//! Tests for the cropping and enhancement algorithms: the boxes the Rust port
//! computes must match what KCC itself returned on the committed fixtures.
//!
//! `tests/fixtures/crop/README.md` records how the fixtures and the reference
//! values were produced. The fixtures are pure black/white, so the preprocessing
//! chain (grayscale, autocontrast, box blur, threshold) is bit-exact between
//! Pillow and this implementation and the boxes can be compared exactly.

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::cli::Cli;
use comic_book::ebook::input::load_tree;
use comic_book::ebook::model::Background;
use comic_book::ebook::options::Options;
use comic_book::ebook::processing::crop;
use comic_book::ebook::processing::interpanel::{self, Direction};
use comic_book::ebook::processing::process_tree;
use comic_book::units::{BBox, Fraction};
use image::{DynamicImage, GenericImageView};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

/// Load a committed crop fixture.
fn fixture(name: &str) -> Result<DynamicImage> {
    let path = fixture_path(name);
    image::open(&path).with_context(|| format!("cannot open {}", path.display()))
}

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

#[test]
fn margin_boxes_match_kcc() -> Result<()> {
    let page = fixture("margin-white.png")?;

    // Power 0 and 1 catch the 1px blur halo; higher power only keeps the core.
    assert_eq!(
        crop::margin_bbox(&page, 0.0, Background::White),
        Some(BBox::new(19, 29, 181, 271))
    );
    assert_eq!(
        crop::margin_bbox(&page, 1.0, Background::White),
        Some(BBox::new(19, 29, 181, 271))
    );
    assert_eq!(
        crop::margin_bbox(&page, 2.0, Background::White),
        Some(BBox::new(20, 30, 180, 270))
    );
    assert_eq!(
        crop::margin_bbox(&page, 3.0, Background::White),
        Some(BBox::new(21, 31, 179, 269))
    );

    // Treating the page as black effectively inverts it, leaving ink everywhere.
    assert_eq!(
        crop::margin_bbox(&page, 1.0, Background::Black),
        Some(BBox::new(0, 0, 200, 300))
    );
    Ok(())
}

#[test]
fn margin_crop_matches_kcc() -> Result<()> {
    let mut page = fixture("margin-white.png")?;
    crop::crop_margin(&mut page, 1.0, Fraction::new(0.0), None, Background::White);
    // Box (19, 29, 181, 271).
    assert_eq!(page.dimensions(), (162, 242));
    Ok(())
}

#[test]
fn page_number_boxes_match_kcc() -> Result<()> {
    let page = fixture("pagenum-white.png")?;

    assert_eq!(
        crop::margin_bbox(&page, 1.0, Background::White),
        Some(BBox::new(79, 99, 721, 1171))
    );
    assert_eq!(
        crop::page_number_bbox(&page, 1.0, Background::White),
        Some(BBox::new(79, 99, 721, 1101))
    );
    assert_eq!(
        crop::margin_bbox(&page, 2.0, Background::White),
        Some(BBox::new(80, 100, 720, 1170))
    );
    assert_eq!(
        crop::page_number_bbox(&page, 2.0, Background::White),
        Some(BBox::new(80, 100, 720, 1100))
    );
    Ok(())
}

#[test]
fn page_number_crop_matches_kcc() -> Result<()> {
    let mut page = fixture("pagenum-white.png")?;
    crop::crop_page_number(&mut page, 1.0, Fraction::new(0.0), None, Background::White);
    assert_eq!(page.dimensions(), (642, 1002));

    // The margin-only mode keeps the page number: the box is 30 rows taller.
    let mut margins = fixture("pagenum-white.png")?;
    crop::crop_margin(
        &mut margins,
        1.0,
        Fraction::new(0.0),
        None,
        Background::White,
    );
    assert_eq!(margins.dimensions(), (642, 1072));
    Ok(())
}

#[test]
fn black_background_page_number_boxes_match_kcc() -> Result<()> {
    let page = fixture("pagenum-black.png")?;

    assert_eq!(
        crop::margin_bbox(&page, 1.0, Background::Black),
        Some(BBox::new(79, 99, 721, 1171))
    );
    assert_eq!(
        crop::page_number_bbox(&page, 1.0, Background::Black),
        Some(BBox::new(79, 99, 721, 1101))
    );

    let mut cropped = fixture("pagenum-black.png")?;
    crop::crop_page_number(
        &mut cropped,
        1.0,
        Fraction::new(0.0),
        None,
        Background::Black,
    );
    assert_eq!(cropped.dimensions(), (642, 1002));
    Ok(())
}

#[test]
fn inter_panel_crop_matches_kcc() -> Result<()> {
    let page = fixture("interpanel-white.png")?;

    let horizontal = interpanel::crop_empty_inter_panel(
        &page,
        Direction::Horizontal,
        Fraction::new(0.04),
        Background::White,
    );
    assert_eq!(horizontal.dimensions(), (200, 285));

    let vertical = interpanel::crop_empty_inter_panel(
        &page,
        Direction::Vertical,
        Fraction::new(0.04),
        Background::White,
    );
    assert_eq!(vertical.dimensions(), (184, 300));

    let both = interpanel::crop_empty_inter_panel(
        &page,
        Direction::Both,
        Fraction::new(0.04),
        Background::White,
    );
    assert_eq!(both.dimensions(), (184, 285));
    Ok(())
}

#[test]
fn process_tree_crops_a_page_number_page() -> Result<()> {
    // A non-Kindle profile leaves the cropped page smaller than the screen, so
    // it is not rescaled and the crop dimensions pass straight through.
    assert_eq!(
        process_fixture("pagenum-white.png", &[])?,
        vec![(642, 1002)]
    );
    Ok(())
}

/// The path of a committed crop fixture.
fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/crop")
        .join(name)
}

/// Process one committed fixture as a source folder and return the encoded pages.
fn process_fixture(name: &str, args: &[&str]) -> Result<Vec<(u32, u32)>> {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");

    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    fs::create_dir_all(&source)?;
    fs::copy(fixture_path(name), source.join("page.png"))?;

    let mut full = vec!["-p", "KoE"];
    full.extend_from_slice(args);
    let options = options(&full)?;
    let mut tree = load_tree(&source, &options)?;
    let book = process_tree(&mut tree, &options)?;

    assert_eq!(book.page_count, 1);
    Ok(book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.pages.iter())
        .map(|page| (page.size.width, page.size.height))
        .collect())
}

#[test]
fn process_tree_collapses_inter_panel_gutters() -> Result<()> {
    // Cropping is disabled so only the inter-panel pass changes the geometry.
    let pages = process_fixture(
        "interpanel-white.png",
        &["--cropping", "0", "--inter-panel-crop", "2"],
    )?;
    assert_eq!(pages, vec![(184, 285)]);
    Ok(())
}

#[test]
fn process_tree_runs_the_moire_eraser() -> Result<()> {
    // A colour page under `--erase-rainbow` still encodes to the same size.
    let pages = process_fixture("pagenum-white.png", &["--cropping", "0", "--erase-rainbow"])?;
    assert_eq!(pages, vec![(800, 1200)]);
    Ok(())
}
