//! Phase 9 tests for tome chunking, `--file-fusion` and `--delete`
//! (AGENTS.md §15, Phase 9 exit criterion): a source splits at the size boundary
//! (or per subdirectory) into several titled files, fusion merges inputs into one
//! book, and `--delete` removes the source after a successful conversion.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::cli::{Cli, Commands};
use comic_book::ebook::options::Options;
use comic_book::ebook::{convert_source, progress, run_ebook, EbookArgs};
use image::{DynamicImage, Rgb, RgbImage};
use tempfile::tempdir;

/// Resolve options from a `comic-book ebook` command line.
fn resolve(args: &[&str]) -> Result<Options> {
    let mut full = vec!["comic-book", "ebook", "book.cbz"];
    full.extend_from_slice(args);
    let cli = Cli::try_parse_from(full)?;
    match cli.command {
        Commands::Ebook(args) => Options::resolve(&args),
        _ => bail!("expected the ebook subcommand"),
    }
}

/// Parse a full `comic-book ebook` command line into its arguments.
fn args(args: &[&str]) -> Result<EbookArgs> {
    let mut full = vec!["comic-book", "ebook"];
    full.extend_from_slice(args);
    let cli = Cli::try_parse_from(full)?;
    match cli.command {
        Commands::Ebook(args) => Ok(args),
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

/// Write an incompressible PNG so its file size is predictable (for size caps).
fn write_noise_png(path: &Path, width: u32, height: u32, seed: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut state = seed;
    let image = RgbImage::from_fn(width, height, |_, _| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let byte = (state >> 24) as u8;
        Rgb([byte, byte ^ 0x5a, byte.wrapping_add(0x11)])
    });
    image.save(path)?;
    Ok(())
}

/// Run the ebook pipeline for one source and return the output paths.
fn convert(source: &Path, options: &Options) -> Result<Vec<PathBuf>> {
    std::env::set_var(progress::QUIET_ENV, "1");
    convert_source(source, options)
}

/// Read every entry of a ZIP into a path → bytes map.
fn zip_entries(path: &Path) -> Result<HashMap<String, Vec<u8>>> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entries = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        entries.insert(name, data);
    }
    Ok(entries)
}

/// The text of one ZIP entry.
fn zip_text(path: &Path, name: &str) -> Result<String> {
    let bytes = zip_entries(path)?
        .remove(name)
        .with_context(|| format!("{name} present"))?;
    Ok(String::from_utf8(bytes)?)
}

/// A two-chapter fixture with no root pages, so `--batch-split 2` splits it.
fn two_chapter_fixture(root: &Path) -> Result<()> {
    write_png(&root.join("Chapter 1/01.png"), 100, 150, [30, 30, 30])?;
    write_png(&root.join("Chapter 2/01.png"), 100, 150, [200, 200, 200])
}

// --- batch splitting -------------------------------------------------------------

#[test]
fn batch_split_two_gives_each_subdirectory_its_own_tome() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    two_chapter_fixture(&source)?;

    let written = convert(
        &source,
        &resolve(&["-f", "epub", "-p", "KoE", "--no-kepub", "-b", "2"])?,
    )?;
    assert_eq!(
        written,
        vec![
            tmp.path().join("book 1.epub"),
            tmp.path().join("book 2.epub")
        ]
    );

    let first = zip_entries(&written[0])?;
    let second = zip_entries(&written[1])?;
    assert!(first.contains_key("OEBPS/Images/chapter-1/kcc-0001-kcc-x.jpg"));
    assert!(second.contains_key("OEBPS/Images/chapter-2/kcc-0002-kcc-x.jpg"));

    // Each tome carries its own `base [i/n]` title.
    let first_opf = String::from_utf8(first["OEBPS/content.opf"].clone())?;
    let second_opf = String::from_utf8(second["OEBPS/content.opf"].clone())?;
    assert!(first_opf.contains("book [1/2]"), "{first_opf}");
    assert!(second_opf.contains("book [2/2]"), "{second_opf}");
    Ok(())
}

#[test]
fn every_tome_of_a_split_book_gets_a_labelled_cover() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    two_chapter_fixture(&source)?;

    let written = convert(
        &source,
        &resolve(&["-f", "epub", "-p", "KoE", "--no-kepub", "-b", "2"])?,
    )?;
    let first = zip_entries(&written[0])?
        .remove("OEBPS/Images/cover.jpg")
        .context("the first cover is present")?;
    let second = zip_entries(&written[1])?
        .remove("OEBPS/Images/cover.jpg")
        .context("the second cover is present")?;
    assert_ne!(
        first, second,
        "each tome's cover gets its own N/M number (AGENTS.md §13.11.4)"
    );

    // A single-tome book keeps the unlabelled cover.
    let plain = zip_entries(&written[1])?;
    let single = convert(
        &source,
        &resolve(&["-f", "epub", "-p", "KoE", "--no-kepub"])?,
    )?;
    let single_cover = zip_entries(&single[0])?
        .remove("OEBPS/Images/cover.jpg")
        .context("the single-tome cover is present")?;
    assert_ne!(plain["OEBPS/Images/cover.jpg"], single_cover);
    Ok(())
}

#[test]
fn a_chunked_book_drops_its_comicinfo_bookmarks() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    two_chapter_fixture(&source)?;
    fs::write(
        source.join("ComicInfo.xml"),
        br#"<ComicInfo><Series>Berserk</Series><Page Image="0" Bookmark="The Black Swordsman"/></ComicInfo>"#,
    )?;

    let written = convert(
        &source,
        &resolve(&["-f", "epub", "-p", "KoE", "--no-kepub", "-b", "2"])?,
    )?;
    let nav = zip_text(&written[0], "OEBPS/nav.xhtml")?;
    assert!(
        !nav.contains("The Black Swordsman"),
        "chunked books reset the bookmark list (KCC's `ischunked`): {nav}"
    );
    // The directory-based chapter list is used instead.
    assert!(nav.contains("chapter-1"));
    Ok(())
}

// --- size caps -------------------------------------------------------------------

#[test]
fn target_size_splits_a_flat_book_at_the_size_boundary() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    // 400x400 noise compresses to ~0.5 MB, so six pages exceed a 1 MB cap and pack
    // two per tome. `--no-processing` keeps the PNG sizes predictable.
    for (index, seed) in (1..=6).enumerate() {
        write_noise_png(
            &source.join(format!("{index:02}.png")),
            400,
            400,
            seed as u32 + 1,
        )?;
    }

    let written = convert(
        &source,
        &resolve(&[
            "-f",
            "epub",
            "-p",
            "KoE",
            "--no-kepub",
            "--no-processing",
            "--target-size",
            "1",
        ])?,
    )?;
    let names: Vec<PathBuf> = (1..=written.len())
        .map(|number| tmp.path().join(format!("book {number}.epub")))
        .collect();
    assert_eq!(written, names, "the filenames carry the tome number");
    assert_eq!(written.len(), 3);

    // Every tome is titled and every page appears exactly once across the tomes.
    let mut pages = 0;
    for (index, path) in written.iter().enumerate() {
        let entries = zip_entries(path)?;
        let opf = String::from_utf8(entries["OEBPS/content.opf"].clone())?;
        assert!(opf.contains(&format!("book [{}/3]", index + 1)), "{opf}");
        pages += entries
            .keys()
            .filter(|name| name.starts_with("OEBPS/Images/") && name.ends_with(".png"))
            .count();
    }
    assert_eq!(pages, 6);
    Ok(())
}

// --- fusion ----------------------------------------------------------------------

#[test]
fn fusion_merges_the_sources_into_one_book() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_png(&a.join("01.png"), 100, 150, [30, 30, 30])?;
    write_png(&b.join("01.png"), 100, 150, [200, 200, 200])?;

    std::env::set_var(progress::QUIET_ENV, "1");
    let cli = args(&[
        a.to_str().context("utf8 path")?,
        b.to_str().context("utf8 path")?,
        "--file-fusion",
        "-f",
        "epub",
        "-p",
        "KoE",
        "--no-kepub",
    ])?;
    run_ebook(cli)?;

    // The fused file is named after the first source and sits in its directory.
    let output = tmp.path().join("a [fused].epub");
    assert!(output.is_file(), "{output:?}");

    let entries = zip_entries(&output)?;
    assert!(entries.contains_key("OEBPS/Images/a/kcc-0001-kcc-x.jpg"));
    assert!(entries.contains_key("OEBPS/Images/b/kcc-0002-kcc-x.jpg"));
    let opf = String::from_utf8(entries["OEBPS/content.opf"].clone())?;
    assert!(opf.contains("a [fused]"), "{opf}");
    Ok(())
}

#[test]
fn fusion_keeps_the_user_order_when_it_differs_from_natural_sort() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_png(&a.join("01.png"), 100, 150, [30, 30, 30])?;
    write_png(&b.join("01.png"), 100, 150, [200, 200, 200])?;

    std::env::set_var(progress::QUIET_ENV, "1");
    // b first: the inputs are not in natural order, so `makeFusion` prefixes the
    // chapter directories so b's pages stay first.
    let cli = args(&[
        b.to_str().context("utf8 path")?,
        a.to_str().context("utf8 path")?,
        "--file-fusion",
        "-f",
        "epub",
        "-p",
        "KoE",
        "--no-kepub",
    ])?;
    run_ebook(cli)?;

    let entries = zip_entries(&tmp.path().join("b [fused].epub"))?;
    assert!(entries.contains_key("OEBPS/Images/fusion-0001-b/kcc-0001-kcc-x.jpg"));
    assert!(entries.contains_key("OEBPS/Images/fusion-0002-a/kcc-0002-kcc-x.jpg"));
    Ok(())
}

#[test]
fn fusion_uses_a_shared_covers_image() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    write_png(&a.join("01.png"), 100, 150, [30, 30, 30])?;
    write_png(&b.join("01.png"), 100, 150, [200, 200, 200])?;
    write_png(&tmp.path().join("Covers/cover.png"), 120, 180, [90, 40, 40])?;

    std::env::set_var(progress::QUIET_ENV, "1");
    // A shared `Covers/` image is a custom cover, which makes any output write it.
    let cli = args(&[
        a.to_str().context("utf8 path")?,
        b.to_str().context("utf8 path")?,
        "--file-fusion",
        "-f",
        "cbz",
        "-p",
        "KoE",
    ])?;
    run_ebook(cli)?;

    let entries = zip_entries(&tmp.path().join("a [fused].cbz"))?;
    assert!(
        entries.contains_key("##cover.jpg"),
        "the fused `Covers/` image becomes `##cover.jpg`: {:?}",
        entries.keys().collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn fusion_requires_at_least_two_sources() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a");
    write_png(&a.join("01.png"), 100, 150, [30, 30, 30])?;

    std::env::set_var(progress::QUIET_ENV, "1");
    let cli = args(&[
        a.to_str().context("utf8 path")?,
        "--file-fusion",
        "-f",
        "epub",
        "-p",
        "KoE",
    ])?;
    let error = match run_ebook(cli) {
        Ok(_) => bail!("fusion accepted a single source"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("at least 2"), "unexpected error: {error}");
    Ok(())
}

// --- delete ----------------------------------------------------------------------

#[test]
fn delete_removes_the_source_after_a_successful_conversion() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("01.png"), 100, 150, [30, 30, 30])?;

    std::env::set_var(progress::QUIET_ENV, "1");
    let cli = args(&[
        source.to_str().context("utf8 path")?,
        "-f",
        "epub",
        "-p",
        "KoE",
        "--no-kepub",
        "--delete",
    ])?;
    run_ebook(cli)?;

    assert!(!source.exists(), "the source directory is removed");
    assert!(tmp.path().join("book.epub").is_file());
    Ok(())
}
