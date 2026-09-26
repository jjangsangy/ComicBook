//! Tests for CBZ/PDF output and light-novel mode: a CBZ repackage loads back into
//! an equivalent tree, a PDF has one page per image at the image's own pixel size,
//! and `--light-novel` preserves the source structure while only resizing the
//! oversized pages.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::cli::Cli;
use comic_book::ebook::input::load_tree;
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

/// Write a solid-colour PNG, creating parent directories.
fn write_png(path: &Path, width: u32, height: u32, color: [u8; 3]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb(color))).save(path)?;
    Ok(())
}

/// Run the ebook pipeline for a source and return the output paths.
fn convert(source: &Path, args: &[&str]) -> Result<Vec<PathBuf>> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let options = options(args)?;
    convert_source(source, &options)
}

/// The entry names of a CBZ (or any ZIP), in archive order.
fn zip_entries(path: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(path)?;
    let archive = zip::ZipArchive::new(file)?;
    Ok(archive.file_names().map(str::to_string).collect())
}

/// The bytes of one entry inside a ZIP.
fn zip_entry(path: &Path, name: &str) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let Ok(mut entry) = archive.by_name(name) else {
        return Ok(None);
    };
    let mut data = Vec::new();
    entry.read_to_end(&mut data)?;
    Ok(Some(data))
}

/// The `(chapter, [page names])` shape of a tree.
fn shape(tree: &comic_book::ebook::ComicTree) -> Vec<(String, Vec<String>)> {
    tree.chapters
        .iter()
        .map(|chapter| {
            (
                chapter.name.clone(),
                chapter
                    .pages
                    .iter()
                    .map(|page| page.source_name.clone())
                    .collect(),
            )
        })
        .collect()
}

// --- CBZ -------------------------------------------------------------------------

#[test]
fn cbz_repackage_loads_back_into_the_processed_tree() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("01-normal.png"), 100, 150, [10, 10, 10])?;
    // 2.5:1 exceeds the bisect threshold, so it rotates to `-kcc-d`.
    write_png(&source.join("02-spread.png"), 500, 200, [255, 255, 255])?;
    write_png(&source.join("Chapter 1/01.png"), 100, 150, [10, 10, 10])?;

    let written = convert(&source, &["-f", "cbz", "-p", "KV", "-c", "0"])?;
    assert_eq!(written.len(), 1);
    assert_eq!(written[0], tmp.path().join("source.cbz"));

    let entries = zip_entries(&written[0])?;
    // The processed pages keep their sanitized `kcc-NNNN-kcc-<order>` names and
    // their chapter directory (naturally ordered CBZ names are kept verbatim).
    assert!(entries.contains(&"kcc-0001-kcc-x.jpg".to_string()));
    assert!(entries.contains(&"kcc-0002-kcc-d.jpg".to_string()));
    assert!(entries.contains(&"Chapter 1/kcc-0003-kcc-x.jpg".to_string()));
    // No cover override and no `--keep-comicinfo`, so neither is written.
    assert!(!entries.iter().any(|name| name == "##cover.jpg"));
    assert!(!entries.iter().any(|name| name == "ComicInfo.xml"));

    // The repackaged archive loads back into the same chapters and pages.
    let tree = load_tree(&written[0], &options(&[])?)?;
    assert_eq!(
        shape(&tree),
        vec![
            (
                String::new(),
                vec![
                    "kcc-0001-kcc-x.jpg".to_string(),
                    "kcc-0002-kcc-d.jpg".to_string(),
                ],
            ),
            (
                "Chapter 1".to_string(),
                vec!["Chapter 1/kcc-0003-kcc-x.jpg".to_string()],
            ),
        ]
    );
    Ok(())
}

#[test]
fn cbz_writes_the_cover_and_comicinfo_when_asked() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    // A 5:2 cover: `--smart-cover-crop` crops one side out of the spread.
    write_png(&source.join("01-cover.png"), 1000, 400, [200, 30, 30])?;
    write_png(&source.join("02.png"), 100, 150, [10, 10, 10])?;
    let comicinfo = br#"<ComicInfo><Series>Berserk</Series></ComicInfo>"#;
    fs::write(source.join("ComicInfo.xml"), comicinfo)?;

    let written = convert(
        &source,
        &[
            "-f",
            "cbz",
            "-p",
            "KV",
            "-c",
            "0",
            "--smart-cover-crop",
            "--keep-comicinfo",
        ],
    )?;

    let entries = zip_entries(&written[0])?;
    assert!(
        entries.iter().any(|name| name == "##cover.jpg"),
        "the smart-cropped cover is written: {entries:?}"
    );
    assert_eq!(
        zip_entry(&written[0], "ComicInfo.xml")?.as_deref(),
        Some(comicinfo.as_slice()),
        "`--keep-comicinfo` round-trips the document verbatim"
    );
    Ok(())
}

// --- PDF -------------------------------------------------------------------------

/// A page of the generated PDF as the assertions care about it.
struct PdfPage {
    width: f64,
    height: f64,
    image_filter: Option<String>,
    image_color_space: Option<String>,
    image_width: i64,
    image_height: i64,
}

/// Parse a PDF back into one [`PdfPage`] per page, in page order.
fn read_pdf(path: &Path) -> Result<Vec<PdfPage>> {
    let bytes = fs::read(path)?;
    let doc = lopdf::Document::load_mem(&bytes)?;

    let mut pages = Vec::new();
    for (_number, id) in doc.get_pages() {
        let dict = doc.get_object(id)?.as_dict()?;

        let media_box = dict.get(b"MediaBox")?.as_array()?;
        let width = pdf_number(&media_box[2])?;
        let height = pdf_number(&media_box[3])?;

        let resources = resolve(&doc, dict.get(b"Resources")?)?.as_dict()?;
        let xobjects = resolve(&doc, resources.get(b"XObject")?)?.as_dict()?;
        let (_, object) = xobjects.iter().next().context("one image per page")?;
        let stream = resolve(&doc, object)?.as_stream()?;

        pages.push(PdfPage {
            width,
            height,
            image_filter: stream.dict.get(b"Filter").ok().and_then(name_of),
            image_color_space: stream.dict.get(b"ColorSpace").ok().and_then(name_of),
            image_width: stream.dict.get(b"Width")?.as_i64()?,
            image_height: stream.dict.get(b"Height")?.as_i64()?,
        });
    }
    Ok(pages)
}

/// Follow an indirect reference to its object.
fn resolve<'a>(doc: &'a lopdf::Document, object: &'a lopdf::Object) -> Result<&'a lopdf::Object> {
    match object {
        lopdf::Object::Reference(id) => Ok(doc.get_object(*id)?),
        other => Ok(other),
    }
}

/// A PDF number as `f64` (lopdf models integers and reals separately).
fn pdf_number(object: &lopdf::Object) -> Result<f64> {
    match object {
        lopdf::Object::Integer(value) => Ok(*value as f64),
        lopdf::Object::Real(value) => Ok(f64::from(*value)),
        other => bail!("not a number: {other:?}"),
    }
}

/// The name behind an object (`/DeviceGray` → `DeviceGray`).
fn name_of(object: &lopdf::Object) -> Option<String> {
    object.as_name().ok().map(|name| {
        String::from_utf8_lossy(name)
            .trim_start_matches('/')
            .to_string()
    })
}

#[test]
fn pdf_has_one_page_per_image_at_its_own_size() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("01.png"), 100, 150, [10, 10, 10])?;
    write_png(&source.join("02.png"), 120, 180, [200, 200, 200])?;
    write_png(&source.join("03.png"), 90, 140, [0, 0, 0])?;

    // All pages are portrait and inside the profile, so no spread is split and the
    // `-c 0` crop leaves the sizes untouched.
    let written = convert(&source, &["-f", "pdf", "-p", "KV", "-c", "0"])?;
    assert_eq!(written[0], tmp.path().join("source.pdf"));

    let pages = read_pdf(&written[0])?;
    assert_eq!(pages.len(), 3);
    let sizes: Vec<(f64, f64)> = pages.iter().map(|page| (page.width, page.height)).collect();
    assert_eq!(sizes, vec![(100.0, 150.0), (120.0, 180.0), (90.0, 140.0)]);

    // Each monochrome page is embedded as a grayscale JPEG XObject at full size.
    for page in &pages {
        assert_eq!(page.image_filter.as_deref(), Some("DCTDecode"));
        assert_eq!(page.image_color_space.as_deref(), Some("DeviceGray"));
        assert_eq!(page.image_width as f64, page.width);
        assert_eq!(page.image_height as f64, page.height);
    }
    Ok(())
}

#[test]
fn pdf_streams_png_pages_through_flate() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("01.png"), 100, 150, [10, 10, 10])?;

    // `--no-processing` keeps the PNG source, which the PDF embeds deflated.
    let written = convert(&source, &["-f", "pdf", "-p", "KV", "--no-processing"])?;
    let pages = read_pdf(&written[0])?;
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].image_filter.as_deref(), Some("FlateDecode"));
    assert_eq!((pages[0].width, pages[0].height), (100.0, 150.0));
    assert_eq!((pages[0].image_width, pages[0].image_height), (100, 150));
    Ok(())
}

// --- light novel -----------------------------------------------------------------

#[test]
fn light_novel_preserves_structure_and_only_resizes_oversized_pages() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("source");
    write_png(&source.join("a.png"), 20, 30, [10, 10, 10])?;
    write_png(&source.join("Chapter 1/b.png"), 20, 30, [10, 10, 10])?;
    // Larger than the KV profile (1072x1448), so it is grayscaled and contained.
    write_png(&source.join("Chapter 1/big.png"), 3000, 4000, [200, 30, 30])?;
    fs::write(
        source.join("ComicInfo.xml"),
        br#"<ComicInfo><Series>Berserk</Series></ComicInfo>"#,
    )?;

    let written = convert(&source, &["--light-novel", "-p", "KV"])?;
    assert_eq!(written[0], tmp.path().join("source.cbz"));

    let entries = zip_entries(&written[0])?;
    // The source structure survives: names, chapter directories and ComicInfo.xml.
    assert!(entries.contains(&"a.png".to_string()));
    assert!(entries.contains(&"Chapter 1/b.png".to_string()));
    assert!(entries.contains(&"Chapter 1/big.png".to_string()));
    assert!(entries.contains(&"ComicInfo.xml".to_string()));

    // A page that already fits is copied byte-for-byte.
    assert_eq!(
        zip_entry(&written[0], "a.png")?,
        fs::read(source.join("a.png")).ok()
    );

    // The oversized page is grayscaled and scaled to fit (3000x4000 → 1072x1429).
    let big =
        zip_entry(&written[0], "Chapter 1/big.png")?.context("the resized page is written")?;
    let decoded = image::load_from_memory(&big)?;
    assert_eq!(decoded.dimensions(), (1072, 1429));
    assert!(matches!(decoded, DynamicImage::ImageLuma8(_)));
    Ok(())
}

#[test]
fn light_novel_reports_a_folder_source_beside_it() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("manga");
    write_png(&source.join("page.png"), 20, 30, [10, 10, 10])?;
    let written = convert(&source, &["--light-novel", "-p", "KV"])?;
    assert_eq!(written, vec![tmp.path().join("manga.cbz")]);
    Ok(())
}
