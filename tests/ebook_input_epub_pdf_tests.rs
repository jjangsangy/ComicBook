//! Tests for the EPUB (spine-ordered) and PDF (embedded-image/rasterised) input
//! adapters.

use anyhow::{bail, Result};
use clap::Parser;
use comic_book::archive::{compress_archive, read_archive_entries, ArchiveKind, EntryContent};
use comic_book::cli::Cli;
use comic_book::ebook::convert_source;
use comic_book::ebook::input::load_tree;
use comic_book::ebook::options::Options;
use image::{DynamicImage, Rgb, RgbImage};
use std::fs;
use std::io::Write;
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
fn write_png(path: &Path, width: u32, height: u32, rgb: [u8; 3]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb(rgb))).save(path)?;
    Ok(())
}

/// Encode an in-memory RGB image as a JPEG.
fn jpeg_bytes(width: u32, height: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut out).encode_image(&RgbImage::from_pixel(
        width,
        height,
        Rgb([200, 100, 50]),
    ))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// EPUB input
// ---------------------------------------------------------------------------

/// Build a minimal fixed-layout EPUB folder with two spine pages.
///
/// Page 2 comes first in the spine and references two images (`small.png` and
/// `large.png`) so the "largest image per page" rule is exercised; page 1
/// references `a.png`.
fn build_epub(root: &Path) -> Result<std::path::PathBuf> {
    let epub = root.join("book.epub");
    let src = root.join("epub-src");
    fs::create_dir_all(src.join("META-INF"))?;
    fs::create_dir_all(src.join("OEBPS/Text"))?;
    fs::write(
        src.join("META-INF/container.xml"),
        br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
    )?;
    fs::write(
        src.join("OEBPS/content.opf"),
        br#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <manifest>
    <item id="p1" href="Text/page1.xhtml" media-type="application/xhtml+xml"/>
    <item id="p2" href="Text/page2.xhtml" media-type="application/xhtml+xml"/>
    <item id="a" href="Images/a.png" media-type="image/png"/>
    <item id="s" href="Images/small.png" media-type="image/png"/>
    <item id="l" href="Images/large.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="p2"/>
    <itemref idref="p1"/>
  </spine>
</package>"#,
    )?;
    fs::write(
        src.join("OEBPS/Text/page1.xhtml"),
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<img src="../Images/a.png"/></body></html>"#,
    )?;
    fs::write(
        src.join("OEBPS/Text/page2.xhtml"),
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<img src="../Images/small.png"/><img src="../Images/large.png"/></body></html>"#,
    )?;

    write_png(&src.join("OEBPS/Images/a.png"), 40, 50, [10, 20, 30])?;
    write_png(&src.join("OEBPS/Images/small.png"), 5, 5, [1, 1, 1])?;
    write_png(&src.join("OEBPS/Images/large.png"), 60, 70, [2, 2, 2])?;

    compress_archive(ArchiveKind::Cbz, &src, &epub)?;
    Ok(epub)
}

#[test]
fn epub_loads_spine_in_order_and_picks_the_largest_image() -> Result<()> {
    let tmp = tempdir()?;
    let epub = build_epub(tmp.path())?;

    let tree = load_tree(&epub, &options(&[])?)?;
    assert_eq!(tree.page_count(), 2);

    // Flat chapter, spine order: page2 (largest of small/large) then page1.
    assert_eq!(tree.chapters.len(), 1);
    assert_eq!(tree.chapters[0].name, "");
    let pages = &tree.chapters[0].pages;
    assert_eq!(pages[0].dimensions().to_dimensions(), (60, 70));
    assert_eq!(pages[0].rel_path, "0.png");
    assert_eq!(pages[1].dimensions().to_dimensions(), (40, 50));
    assert_eq!(pages[1].rel_path, "1.png");

    // The chosen image keeps its original bytes (see docs/porting.md).
    assert_eq!(
        pages[0].source_media_type,
        Some(comic_book::ebook::model::MediaType::Png)
    );
    assert!(pages[0].raw.is_some());
    Ok(())
}

#[test]
fn epub_without_a_resolvable_spine_falls_back_to_the_archive() -> Result<()> {
    // A container/OPF whose spine names no usable page: KCC's `if not
    // ordered_image_paths: return workdir` loads the raw extracted tree instead.
    let tmp = tempdir()?;
    let src = tmp.path().join("src");
    fs::create_dir_all(src.join("META-INF"))?;
    fs::create_dir_all(src.join("OEBPS"))?;
    fs::write(
        src.join("META-INF/container.xml"),
        br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
    )?;
    fs::write(
        src.join("OEBPS/content.opf"),
        br#"<package><manifest/><spine><itemref idref="missing"/></spine></package>"#,
    )?;
    write_png(&src.join("OEBPS/Images/page1.png"), 10, 10, [0, 0, 0])?;
    write_png(&src.join("OEBPS/Images/page2.png"), 20, 20, [0, 0, 0])?;

    let epub = tmp.path().join("plain.epub");
    compress_archive(ArchiveKind::Cbz, &src, &epub)?;

    let tree = load_tree(&epub, &options(&[])?)?;
    let names: Vec<&str> = tree.chapters[0]
        .pages
        .iter()
        .map(|page| page.rel_path.as_str())
        .collect();
    assert_eq!(names, vec!["page1.png", "page2.png"]);
    Ok(())
}

#[test]
fn epub_ignores_a_referenced_non_image() -> Result<()> {
    // KCC copies whatever the page references and only later drops non-image
    // extensions, so a page whose only image is unsupported yields nothing while
    // the rest of the spine still loads.
    let tmp = tempdir()?;
    let src = tmp.path().join("src");
    fs::create_dir_all(src.join("META-INF"))?;
    fs::create_dir_all(src.join("OEBPS/Text"))?;
    fs::write(
        src.join("META-INF/container.xml"),
        br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
    )?;
    fs::write(
        src.join("OEBPS/content.opf"),
        br#"<package><manifest>
<item id="p1" href="Text/p1.xhtml" media-type="application/xhtml+xml"/>
<item id="p2" href="Text/p2.xhtml" media-type="application/xhtml+xml"/>
</manifest><spine><itemref idref="p1"/><itemref idref="p2"/></spine></package>"#,
    )?;
    fs::write(
        src.join("OEBPS/Text/p1.xhtml"),
        br#"<html><body><img src="../Images/big.jxl"/></body></html>"#,
    )?;
    fs::write(
        src.join("OEBPS/Text/p2.xhtml"),
        br#"<html><body><img src="../Images/small.png"/></body></html>"#,
    )?;
    write_png(&src.join("OEBPS/Images/small.png"), 6, 6, [0, 0, 0])?;
    fs::write(src.join("OEBPS/Images/big.jxl"), vec![0u8; 5000])?;

    let epub = tmp.path().join("book.epub");
    compress_archive(ArchiveKind::Cbz, &src, &epub)?;

    let tree = load_tree(&epub, &options(&[])?)?;
    assert_eq!(tree.page_count(), 1);
    assert_eq!(
        tree.chapters[0].pages[0].dimensions().to_dimensions(),
        (6, 6)
    );
    Ok(())
}

#[test]
fn legacy_extract_loads_an_epub_as_a_plain_archive() -> Result<()> {
    let tmp = tempdir()?;
    let epub = build_epub(tmp.path())?;

    let tree = load_tree(&epub, &options(&["--legacy-extract"])?)?;
    // Every image in the container, in natural order, rather than spine order.
    assert_eq!(tree.page_count(), 3);
    let names: Vec<&str> = tree.chapters[0]
        .pages
        .iter()
        .map(|page| page.rel_path.as_str())
        .collect();
    assert!(names.contains(&"a.png"), "{names:?}");
    Ok(())
}

#[test]
fn epub_source_converts_end_to_end() -> Result<()> {
    let tmp = tempdir()?;
    let epub = build_epub(tmp.path())?;

    let written = convert_source(&epub, &options(&["-f", "cbz"])?)?;
    assert_eq!(written.len(), 1);

    let mut images = Vec::new();
    read_archive_entries(ArchiveKind::Cbz, &written[0], |name, content| {
        if let EntryContent::File(_) = content {
            images.push(name.to_string());
        }
        Ok(())
    })?;
    assert_eq!(images.len(), 2, "unexpected entries: {images:?}");
    Ok(())
}

// ---------------------------------------------------------------------------
// PDF input
// ---------------------------------------------------------------------------

/// A one-page PDF whose content fills the page with a solid black rectangle.
fn build_vector_pdf(path: &Path) -> Result<()> {
    use pdf_writer::{Content, Pdf, Rect, Ref};
    let mut pdf = Pdf::new();
    let (catalog, pages, page, content_id) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    pdf.catalog(catalog).pages(pages);

    let mut content = Content::new();
    content.set_fill_rgb(0.0, 0.0, 0.0);
    content.rect(0.0, 0.0, 100.0, 200.0);
    content.fill_nonzero();
    pdf.stream(content_id, &content.finish());

    {
        let mut page_writer = pdf.page(page);
        page_writer.parent(pages);
        page_writer.media_box(Rect::new(0.0, 0.0, 100.0, 200.0));
        page_writer.contents(content_id);
    }
    pdf.pages(pages).kids([page]).count(1);
    fs::write(path, pdf.finish())?;
    Ok(())
}

/// A one-page PDF whose only content is a single embedded RGB image.
fn build_image_pdf(path: &Path, width: u32, height: u32) -> Result<()> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};

    let raw = vec![200u8; (width * height * 3) as usize];
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    let data = encoder.finish()?;

    let mut pdf = Pdf::new();
    let (catalog, pages, page, content_id, image_id) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    pdf.catalog(catalog).pages(pages);
    {
        let mut xobject = pdf.image_xobject(image_id, &data);
        xobject.filter(Filter::FlateDecode);
        xobject.width(width as i32);
        xobject.height(height as i32);
        xobject.color_space().device_rgb();
        xobject.bits_per_component(8);
    }

    let name = Name(b"Im1");
    let mut content = Content::new();
    content.save_state();
    content.transform([width as f32, 0.0, 0.0, height as f32, 0.0, 0.0]);
    content.x_object(name);
    content.restore_state();
    pdf.stream(content_id, &content.finish());

    {
        let mut page_writer = pdf.page(page);
        page_writer.parent(pages);
        page_writer.media_box(Rect::new(0.0, 0.0, width as f32, height as f32));
        page_writer.resources().x_objects().pair(name, image_id);
        page_writer.contents(content_id);
    }
    pdf.pages(pages).kids([page]).count(1);
    fs::write(path, pdf.finish())?;
    Ok(())
}

#[test]
fn pdf_with_one_image_extracts_it_at_native_size() -> Result<()> {
    let tmp = tempdir()?;
    let pdf = tmp.path().join("book.pdf");
    build_image_pdf(&pdf, 40, 30)?;

    let tree = load_tree(&pdf, &options(&[])?)?;
    assert_eq!(tree.page_count(), 1);
    let page = &tree.chapters[0].pages[0];
    assert_eq!(page.dimensions().to_dimensions(), (40, 30));
    assert_eq!(page.rel_path, "p-0.png");
    assert_eq!(
        page.source_media_type,
        Some(comic_book::ebook::model::MediaType::Png)
    );
    Ok(())
}

#[test]
fn vector_pdf_pages_are_rasterised_to_the_device_target() -> Result<()> {
    let tmp = tempdir()?;
    let pdf = tmp.path().join("book.pdf");
    build_vector_pdf(&pdf)?;

    // Default KV profile, `--cropping 2`: target 1072*1.25 x 1448*1.25.
    let tree = load_tree(&pdf, &options(&[])?)?;
    assert_eq!(tree.page_count(), 1);
    let page = &tree.chapters[0].pages[0];
    let (width, height) = page.dimensions().to_dimensions();
    assert_eq!(height, 1810, "target height");

    // The rendered page carries the drawn black fill, not a blank white page.
    let center = page
        .to_decoded()?
        .to_rgb8()
        .get_pixel(width / 2, height / 2)
        .0;
    assert!(center[0] < 10, "center pixel was {center:?}");
    Ok(())
}

#[test]
fn legacy_extract_scans_embedded_jpegs() -> Result<()> {
    let tmp = tempdir()?;
    let pdf = tmp.path().join("book.pdf");

    // The legacy scanner never parses the PDF, so a minimal wrapper around a JPEG
    // stream is enough (KCC's `pdfjpgextract`).
    let jpeg = jpeg_bytes(200, 150)?;
    let mut blob = Vec::new();
    blob.extend_from_slice(b"%PDF-1.4\n1 0 obj\n<<>>\nstream\n");
    blob.extend_from_slice(&jpeg);
    blob.extend_from_slice(b"\nendstream\nendobj\n%%EOF");
    fs::write(&pdf, blob)?;

    let tree = load_tree(&pdf, &options(&["--legacy-extract"])?)?;
    assert_eq!(tree.page_count(), 1);
    assert_eq!(
        tree.chapters[0].pages[0].dimensions().to_dimensions(),
        (200, 150)
    );

    // Without the flag the stub is not a parseable PDF.
    assert!(load_tree(&pdf, &options(&[])?).is_err());
    Ok(())
}

#[test]
fn pdf_source_converts_end_to_end() -> Result<()> {
    let tmp = tempdir()?;
    let pdf = tmp.path().join("book.pdf");
    // Portrait so the spread splitter leaves it whole.
    build_image_pdf(&pdf, 30, 40)?;

    let written = convert_source(&pdf, &options(&["-f", "cbz"])?)?;
    assert_eq!(written.len(), 1);

    let mut images = 0;
    read_archive_entries(ArchiveKind::Cbz, &written[0], |_, content| {
        if let EntryContent::File(_) = content {
            images += 1;
        }
        Ok(())
    })?;
    assert_eq!(images, 1);
    Ok(())
}
