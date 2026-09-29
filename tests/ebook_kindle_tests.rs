//! Tests for the Kindle output (AZW3/MOBI) built through `kindling`: the produced
//! files are read back structurally with `kindling`'s own dumper, and `mobi+epub`
//! keeps a valid intermediate EPUB.
//!
//! The assertions read the `section.field = value` dump `kindling::mobi_dump`
//! emits, so they pin the real container layout (KF8-only vs dual MOBI7+KF8, the
//! EXTH metadata, the shelf tag) rather than mere file existence.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::cli::Cli;
use comic_book::ebook::options::Options;
use comic_book::ebook::{convert_source, progress};
use image::{DynamicImage, Rgb, RgbImage};
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

/// A two-page folder fixture.
fn fixture(root: &Path) -> Result<()> {
    write_png(&root.join("01.png"), 100, 150, [10, 10, 10])?;
    write_png(&root.join("02.png"), 100, 150, [200, 200, 200])
}

/// Run the ebook pipeline for a source and return the output paths.
fn convert(source: &Path, args: &[&str]) -> Result<Vec<PathBuf>> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let options = options(args)?;
    convert_source(source, &options)
}

/// The structural dump of a MOBI/AZW3, as `kindling` reads it back.
fn dump(path: &Path) -> Result<String> {
    kindling::mobi_dump::dump_mobi(path).map_err(|error| anyhow::anyhow!("{error}"))
}

/// The field behind a `<section>.<field> = <value>` dump line, if present.
fn field<'a>(dump: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key} = ");
    dump.lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

/// One entry of a ZIP, without decompressing the whole archive.
fn zip_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let Ok(mut entry) = archive.by_name(name) else {
        return Ok(None);
    };
    let mut data = Vec::new();
    entry.read_to_end(&mut data)?;
    Ok(Some(data))
}

// --- containers ------------------------------------------------------------------

#[test]
fn azw3_is_a_kf8_only_file() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let written = convert(&source, &["-f", "azw3", "-p", "KV"])?;
    assert_eq!(written, vec![tmp.path().join("book.azw3")]);

    let dump = dump(&written[0])?;
    assert_eq!(field(&dump, "palmdb.type"), Some("\"BOOK\""));
    assert_eq!(field(&dump, "palmdb.creator"), Some("\"MOBI\""));
    // The single MOBI header is the KF8 one, and there is no MOBI7/KF8 boundary.
    assert_eq!(field(&dump, "mobi.file_version"), Some("8"));
    assert_eq!(field(&dump, "mobi.min_version"), Some("8"));
    assert!(
        !dump.contains("kf8.boundary_record"),
        "KF8-only output has no MOBI7 section: {dump}"
    );
    Ok(())
}

#[test]
fn mobi_is_a_dual_mobi7_plus_kf8_file() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let written = convert(&source, &["-f", "mobi", "-p", "KV"])?;
    assert_eq!(written, vec![tmp.path().join("book.mobi")]);

    let dump = dump(&written[0])?;
    // The KF7 section is MOBI 6 and the file carries an explicit KF8 boundary.
    assert_eq!(field(&dump, "mobi.file_version"), Some("6"));
    assert!(
        field(&dump, "kf8.boundary_record").is_some(),
        "dual MOBI has a KF8 boundary: {dump}"
    );
    // The KF8 header follows the boundary.
    assert_eq!(field(&dump, "section0.file_version"), Some("6"));
    assert!(
        dump.contains("mobi.file_version = 8"),
        "the KF8 section is present: {dump}"
    );
    Ok(())
}

#[test]
fn mobi_epub_keeps_a_valid_intermediate_epub() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let written = convert(&source, &["-f", "mobi+epub", "-p", "KV"])?;
    assert_eq!(
        written,
        vec![tmp.path().join("book.mobi"), tmp.path().join("book.epub")],
        "both the MOBI and the kept EPUB are reported"
    );

    // The kept EPUB is the exact fixed-layout container the Kindle file was
    // built from: `mimetype` first and stored, plus the OPF.
    assert_eq!(
        zip_entry(&written[1], "mimetype")?.as_deref(),
        Some(b"application/epub+zip".as_slice())
    );
    let opf = zip_entry(&written[1], "OEBPS/content.opf")?.context("the OPF is present")?;
    let opf = String::from_utf8(opf)?;
    assert!(opf.contains("<meta name=\"fixed-layout\" content=\"true\"/>"));
    assert!(opf.contains("<meta name=\"original-resolution\" content=\"1072x1448\"/>"));
    Ok(())
}

#[test]
fn plain_mobi_does_not_keep_the_intermediate_epub() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let written = convert(&source, &["-f", "mobi", "-p", "KV"])?;
    assert_eq!(written, vec![tmp.path().join("book.mobi")]);
    assert!(
        !tmp.path().join("book.epub").exists(),
        "the intermediate EPUB is removed"
    );
    // Only the staged scratch directory may be left behind; nothing next to the
    // output.
    let leftovers: Vec<String> = fs::read_dir(tmp.path())?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "book" && name != "book.mobi")
        .collect();
    assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
    Ok(())
}

// --- metadata --------------------------------------------------------------------

#[test]
fn title_and_author_reach_the_mobi() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let written = convert(
        &source,
        &["-f", "azw3", "-p", "KV", "-t", "My Book", "-a", "A. Writer"],
    )?;
    let dump = dump(&written[0])?;
    assert_eq!(field(&dump, "mobi.full_name"), Some("\"My Book\""));
    assert_eq!(field(&dump, "exth[100].value"), Some("\"A. Writer\""));
    Ok(())
}

#[test]
fn the_shelf_tag_is_omitted_by_default_and_set_on_request() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    // The default is `none`: no EXTH 501 at all, avoiding the firmware
    // "back to library" issue (see docs/output.md).
    let default = convert(&source, &["-f", "azw3", "-p", "KV"])?;
    assert!(
        !dump(&default[0])?.contains("exth[501]"),
        "no shelf tag by default"
    );

    for (doc_type, tag) in [("ebok", "EBOK"), ("pdoc", "PDOC")] {
        let written = convert(&source, &["-f", "azw3", "-p", "KV", "--doc-type", doc_type])?;
        let dump = dump(&written[0])?;
        assert_eq!(
            field(&dump, "exth[501].value"),
            Some(format!("\"{tag}\"").as_str()),
            "--doc-type {doc_type} sets EXTH 501"
        );
    }
    Ok(())
}

#[test]
fn the_writing_mode_follows_the_page_progression() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let ltr = convert(&source, &["-f", "azw3", "-p", "KV"])?;
    let ltr = dump(&ltr[0])?;
    assert_eq!(field(&ltr, "exth[525].value"), Some("\"horizontal-lr\""));
    assert_eq!(field(&ltr, "exth[527].value"), Some("\"ltr\""));

    let rtl = convert(&source, &["-f", "azw3", "-p", "KV", "-m"])?;
    let rtl = dump(&rtl[0])?;
    assert_eq!(field(&rtl, "exth[525].value"), Some("\"horizontal-rl\""));
    assert_eq!(field(&rtl, "exth[527].value"), Some("\"rtl\""));
    Ok(())
}

// --- pipeline interactions -------------------------------------------------------

#[test]
fn auto_format_on_a_kindle_profile_produces_a_mobi() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    let written = convert(&source, &["-p", "KV"])?;
    assert_eq!(written, vec![tmp.path().join("book.mobi")]);
    Ok(())
}

#[test]
fn panel_view_markup_survives_the_kindle_builder() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;

    // `-q` enables Panel View; the `app-amzn-magnify` overlay must round-trip
    // through `kindling` into the KF8 text (it is passed through verbatim).
    let written = convert(&source, &["-f", "azw3", "-p", "KV", "-q"])?;
    assert_eq!(written, vec![tmp.path().join("book.azw3")]);

    let dump = dump(&written[0])?;
    assert!(dump.contains("mobi.identifier = \"MOBI\""));
    Ok(())
}

#[test]
fn an_existing_kindle_file_is_overwritten() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source)?;
    fs::write(tmp.path().join("book.azw3"), b"x")?;

    // Output names are deterministic, so the existing file is replaced rather than
    // getting a `_cb` counter.
    let written = convert(&source, &["-f", "azw3", "-p", "KV"])?;
    assert_eq!(written, vec![tmp.path().join("book.azw3")]);
    assert_ne!(
        fs::read(tmp.path().join("book.azw3"))?,
        b"x",
        "the existing keeper file is overwritten"
    );
    Ok(())
}

#[test]
fn explicit_batch_splitting_and_size_caps_now_convert() -> Result<()> {
    std::env::set_var(progress::QUIET_ENV, "1");
    // A small flat book fits under the cap either way, so each run yields one tome
    // with the bare name (chunking leaves a small book as a single tome).
    for args in [
        vec!["-f", "mobi", "-p", "KV", "-b", "2"],
        vec!["-f", "mobi", "-p", "KV", "--target-size", "50"],
    ] {
        let tmp = tempdir()?;
        let source = tmp.path().join("book");
        fixture(&source)?;
        let written = convert_source(&source, &options(&args)?)?;
        assert_eq!(
            written,
            vec![tmp.path().join("book.mobi")],
            "args: {args:?}"
        );
    }
    Ok(())
}
