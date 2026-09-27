//! Tests for the `ebook` input adapters: a fixture CBZ/CBR/CB7/CBT or folder must
//! load into an identical, naturally ordered [`ComicTree`].

use anyhow::{bail, Result};
use clap::Parser;
use comic_book::archive::{compress_archive, ArchiveKind};
use comic_book::cli::Cli;
use comic_book::ebook::input::{detect_source_kind, load_tree, SourceKind};
use comic_book::ebook::model::{ComicTree, CoverSource};
use comic_book::ebook::options::Options;
use image::{DynamicImage, GenericImageView, RgbImage};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

/// Resolve the default option set for a `comic-book ebook` run.
fn options() -> Result<Options> {
    let cli = Cli::try_parse_from(["comic-book", "ebook", "book.cbz"])?;
    match cli.command {
        comic_book::cli::Commands::Ebook(args) => Options::resolve(&args),
        _ => bail!("expected the ebook subcommand"),
    }
}

/// A chapter's name plus its pages' `(file name, width, height)`.
type ChapterShape = (String, Vec<(String, u32, u32)>);

/// Reduce a tree to the structure the tests care about.
fn tree_shape(tree: &ComicTree) -> Vec<ChapterShape> {
    tree.chapters
        .iter()
        .map(|chapter| {
            let pages = chapter
                .pages
                .iter()
                .map(|page| {
                    let (width, height) = page.image.dimensions();
                    (page.rel_path.clone(), width, height)
                })
                .collect();
            (chapter.name.clone(), pages)
        })
        .collect()
}

/// Write a solid-colour PNG of the given size, creating parent directories.
fn write_png(path: &Path, width: u32, height: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    DynamicImage::ImageRgb8(RgbImage::new(width, height)).save(path)?;
    Ok(())
}

/// Build `layout` (relative path, width, height) into a folder, then wrap it in
/// every archive format. Returns `(kind, path)` pairs, including the directory
/// source itself.
fn build_variants(root: &Path, layout: &[(&str, u32, u32)]) -> Result<Vec<(ArchiveKind, PathBuf)>> {
    let source = root.join("source");
    for (rel, width, height) in layout {
        write_png(&source.join(rel), *width, *height)?;
    }

    let mut variants = Vec::new();
    for (name, kind) in [
        ("book.cbz", ArchiveKind::Cbz),
        ("book.cbr", ArchiveKind::Cbr),
        ("book.cb7", ArchiveKind::Cb7),
        ("book.cbt", ArchiveKind::Cbt),
    ] {
        let dest = root.join(name);
        compress_archive(kind, &source, &dest)?;
        variants.push((kind, dest));
    }
    variants.push((ArchiveKind::Directory, source));
    Ok(variants)
}

/// Assert every variant loads into exactly `expected`.
fn assert_variants_match(
    variants: &[(ArchiveKind, PathBuf)],
    expected: &[ChapterShape],
) -> Result<()> {
    let options = options()?;
    for (kind, path) in variants {
        let tree = load_tree(path, &options).map_err(|err| anyhow::anyhow!("{kind:?}: {err}"))?;
        assert_eq!(&tree_shape(&tree), expected, "shape mismatch for {kind:?}");
        let pages: usize = tree.chapters.iter().map(|c| c.pages.len()).sum();
        assert_eq!(pages, tree.page_count(), "page_count mismatch for {kind:?}");
        assert!(tree.cover.is_some(), "cover missing for {kind:?}");
    }
    Ok(())
}

#[test]
fn flat_pages_load_identically_across_formats() -> Result<()> {
    let tmp = tempdir()?;
    let variants = build_variants(
        tmp.path(),
        &[
            ("page1.png", 10, 11),
            ("page2.png", 20, 21),
            ("page10.png", 30, 31),
        ],
    )?;

    let expected: Vec<ChapterShape> = vec![(
        String::new(),
        vec![
            ("page1.png".to_string(), 10, 11),
            ("page2.png".to_string(), 20, 21),
            ("page10.png".to_string(), 30, 31),
        ],
    )];
    assert_variants_match(&variants, &expected)
}

#[test]
fn chapters_are_naturally_ordered_across_formats() -> Result<()> {
    let tmp = tempdir()?;
    let variants = build_variants(
        tmp.path(),
        &[
            ("Chapter 10/z.png", 10, 10),
            ("Chapter 2/b.png", 20, 20),
            ("Chapter 1/a.png", 30, 30),
        ],
    )?;

    let expected: Vec<ChapterShape> = vec![
        ("Chapter 1".to_string(), vec![("a.png".to_string(), 30, 30)]),
        ("Chapter 2".to_string(), vec![("b.png".to_string(), 20, 20)]),
        (
            "Chapter 10".to_string(),
            vec![("z.png".to_string(), 10, 10)],
        ),
    ];
    assert_variants_match(&variants, &expected)
}

#[test]
fn root_pages_form_the_first_chapter() -> Result<()> {
    let tmp = tempdir()?;
    let variants = build_variants(
        tmp.path(),
        &[
            ("cover.png", 40, 40),
            ("Chapter 1/page1.png", 50, 50),
            ("Chapter 1/Sub/page2.png", 60, 60),
        ],
    )?;

    // Pre-order: root, then `Chapter 1`, then its nested `Sub` directory.
    let expected: Vec<ChapterShape> = vec![
        (String::new(), vec![("cover.png".to_string(), 40, 40)]),
        (
            "Chapter 1".to_string(),
            vec![("page1.png".to_string(), 50, 50)],
        ),
        (
            "Chapter 1/Sub".to_string(),
            vec![("page2.png".to_string(), 60, 60)],
        ),
    ];
    assert_variants_match(&variants, &expected)
}

#[test]
fn archive_wrapper_directory_is_flattened() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("wrap");
    write_png(&source.join("book/page1.png"), 10, 10)?;
    write_png(&source.join("book/page2.png"), 20, 20)?;

    let archive = tmp.path().join("wrap.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let tree = load_tree(&archive, &options()?)?;
    assert_eq!(
        tree_shape(&tree),
        vec![(
            String::new(),
            vec![
                ("page1.png".to_string(), 10, 10),
                ("page2.png".to_string(), 20, 20),
            ],
        )]
    );
    Ok(())
}

#[test]
fn image_folder_keeps_a_single_subdirectory_as_a_chapter() -> Result<()> {
    // KCC copies a folder source verbatim (no flattening), unlike an archive.
    let tmp = tempdir()?;
    let source = tmp.path().join("manga");
    write_png(&source.join("book/page1.png"), 10, 10)?;

    let tree = load_tree(&source, &options()?)?;
    assert_eq!(
        tree_shape(&tree),
        vec![("book".to_string(), vec![("page1.png".to_string(), 10, 10)])]
    );
    Ok(())
}

#[test]
fn non_images_and_os_junk_are_dropped() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("src");
    write_png(&source.join("page1.png"), 10, 10)?;
    fs::write(source.join("notes.txt"), b"ignore me")?;
    fs::write(source.join("._page1.png"), b"apple double")?;
    fs::create_dir_all(source.join("__MACOSX"))?;
    fs::write(source.join("__MACOSX/junk.png"), b"not an image")?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let tree = load_tree(&archive, &options()?)?;
    assert_eq!(
        tree_shape(&tree),
        vec![(String::new(), vec![("page1.png".to_string(), 10, 10)])]
    );
    Ok(())
}

#[test]
fn comicinfo_is_captured_and_not_a_page() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("src");
    write_png(&source.join("book/page1.png"), 10, 10)?;
    fs::write(source.join("book/ComicInfo.xml"), b"<ComicInfo/>")?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let tree = load_tree(&archive, &options()?)?;
    assert_eq!(
        tree_shape(&tree),
        vec![(String::new(), vec![("page1.png".to_string(), 10, 10)])]
    );
    assert_eq!(tree.comicinfo.as_deref(), Some(b"<ComicInfo/>".as_slice()));
    Ok(())
}

#[test]
fn source_names_are_book_relative() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("src");
    write_png(&source.join("Chapter 1/page.png"), 5, 5)?;

    let tree = load_tree(&source, &options()?)?;
    let page = &tree.chapters[0].pages[0];
    assert_eq!(page.source_name, "Chapter 1/page.png");
    assert_eq!(page.rel_path, "page.png");
    assert_eq!(tree.cover, Some(CoverSource::FirstPage));
    Ok(())
}

#[test]
fn detect_source_kind_classifies_inputs() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("src");
    write_png(&source.join("page.png"), 5, 5)?;
    assert_eq!(
        detect_source_kind(&source),
        Some(SourceKind::Archive(ArchiveKind::Directory))
    );

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;
    assert_eq!(
        detect_source_kind(&archive),
        Some(SourceKind::Archive(ArchiveKind::Cbz))
    );

    // A cbz-named file with no archive payload is unrecognised.
    let not_an_archive = tmp.path().join("nope.cbz");
    fs::write(&not_an_archive, b"just text")?;
    assert_eq!(detect_source_kind(&not_an_archive), None);
    Ok(())
}

#[test]
fn missing_and_unsupported_sources_error() -> Result<()> {
    let tmp = tempdir()?;

    let missing = match load_tree(&tmp.path().join("nope.cbz"), &options()?) {
        Ok(_) => bail!("a missing source should fail"),
        Err(error) => error,
    };
    assert!(missing.to_string().contains("Failed to open source"));

    let text = tmp.path().join("notes.txt");
    fs::write(&text, b"hello")?;
    let unsupported = match load_tree(&text, &options()?) {
        Ok(_) => bail!("an unsupported source should fail"),
        Err(error) => error,
    };
    assert!(unsupported.to_string().contains("Unsupported input"));
    Ok(())
}

#[test]
fn empty_folder_reports_no_images() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("empty");
    fs::create_dir_all(&source)?;

    let err = match load_tree(&source, &options()?) {
        Ok(_) => bail!("an empty folder should fail"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("No images detected"));
    Ok(())
}

/// A malformed EPUB/PDF stub still errors, with an input-specific message rather
/// than a "not implemented" stub error.
#[test]
fn malformed_epub_and_pdf_stubs_error() -> Result<()> {
    let tmp = tempdir()?;
    let options = options()?;

    let epub = tmp.path().join("book.epub");
    fs::write(&epub, b"stub")?;
    assert_eq!(detect_source_kind(&epub), Some(SourceKind::Epub));
    let err = match load_tree(&epub, &options) {
        Ok(_) => bail!("a malformed EPUB should fail"),
        Err(error) => error,
    };
    assert!(
        !err.to_string().to_lowercase().contains("not implemented"),
        "{err}"
    );

    let pdf = tmp.path().join("book.pdf");
    fs::write(&pdf, b"stub")?;
    assert_eq!(detect_source_kind(&pdf), Some(SourceKind::Pdf));
    let err = match load_tree(&pdf, &options) {
        Ok(_) => bail!("a malformed PDF should fail"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("PDF"), "{err}");
    Ok(())
}

#[test]
fn jp2_and_avif_entries_are_ignored() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("src");
    write_png(&source.join("page1.png"), 10, 10)?;
    fs::write(source.join("scan.jp2"), b"undecodable")?;
    fs::write(source.join("scan.avif"), b"undecodable")?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;
    let tree = load_tree(&archive, &options()?)?;
    assert_eq!(
        tree_shape(&tree),
        vec![(String::new(), vec![("page1.png".to_string(), 10, 10)])]
    );

    // A source holding only unsupported formats reports no images.
    let only = tmp.path().join("only");
    fs::create_dir_all(&only)?;
    fs::write(only.join("scan.jp2"), b"undecodable")?;
    let only_archive = tmp.path().join("only.cbz");
    compress_archive(ArchiveKind::Cbz, &only, &only_archive)?;
    let err = match load_tree(&only_archive, &options()?) {
        Ok(_) => bail!("a source with no images should fail"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("No images detected"));
    Ok(())
}
