//! Tests for page/chapter naming, output filename resolution and the sibling
//! `Covers/` pick.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::archive::{compress_archive, ArchiveKind};
use comic_book::cli::Cli;
use comic_book::ebook::input::load_tree;
use comic_book::ebook::metadata::ComicInfo;
use comic_book::ebook::naming::{output_filename, sanitize_tree, select_cover};
use comic_book::ebook::options::Options;
use image::{DynamicImage, RgbImage};
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

/// Write a tiny PNG, creating parent directories.
fn write_png(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    DynamicImage::ImageRgb8(RgbImage::new(4, 4)).save(path)?;
    Ok(())
}

/// The `(chapter name, [page source names])` shape of a tree after sanitizing.
fn shape(tree: &comic_book::ebook::ComicTree) -> Vec<(String, Vec<String>)> {
    tree.chapters
        .iter()
        .map(|chapter| {
            (
                chapter.name.to_string(),
                chapter
                    .pages
                    .iter()
                    .map(|page| page.source_name.to_string())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn chapters_and_pages_are_renamed() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("page1.png"))?;
    write_png(&source.join("Chapter 1/page2.png"))?;
    write_png(&source.join("Chapter 1/page3.png"))?;

    let mut tree = load_tree(&source, &options(&[])?)?;
    let sanitized = sanitize_tree(&mut tree, &options(&["-f", "epub"])?);

    assert_eq!(
        shape(&tree),
        vec![
            (String::new(), vec!["kcc-0001.png".to_string()]),
            (
                "chapter-1".to_string(),
                vec![
                    "chapter-1/kcc-0002.png".to_string(),
                    "chapter-1/kcc-0003.png".to_string(),
                ],
            ),
        ]
    );
    assert_eq!(sanitized.cover_path.as_deref(), Some("kcc-0001.png"));
    assert_eq!(
        sanitized
            .chapter_titles
            .get("chapter-1")
            .map(String::as_str),
        Some("Chapter 1")
    );
    Ok(())
}

#[test]
fn page_numbering_is_global_and_lowercases_the_extension() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("A1.PNG"))?;
    write_png(&source.join("Chapter 1/B1.PNG"))?;

    let mut tree = load_tree(&source, &options(&[])?)?;
    sanitize_tree(&mut tree, &options(&["-f", "epub"])?);

    assert_eq!(
        shape(&tree),
        vec![
            (String::new(), vec!["kcc-0001.png".to_string()]),
            (
                "chapter-1".to_string(),
                vec!["chapter-1/kcc-0002.png".to_string()],
            ),
        ]
    );
    Ok(())
}

#[test]
fn unsorted_chapters_get_zero_padded_numbers() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("Chapter 1/a.png"))?;
    write_png(&source.join("Chapter 2/b.png"))?;
    write_png(&source.join("Chapter 10/c.png"))?;

    let mut tree = load_tree(&source, &options(&[])?)?;
    sanitize_tree(&mut tree, &options(&["-f", "epub"])?);

    let names: Vec<String> = tree.chapters.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, vec!["chapter-0001", "chapter-0002", "chapter-0010"]);
    Ok(())
}

#[test]
fn cbz_keeps_naturally_ordered_directory_names() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("Chapter 1/a.png"))?;
    write_png(&source.join("Chapter 2/b.png"))?;

    let mut tree = load_tree(&source, &options(&[])?)?;
    sanitize_tree(&mut tree, &options(&["-p", "KDX"])?);

    let names: Vec<String> = tree.chapters.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, vec!["Chapter 1", "Chapter 2"]);
    Ok(())
}

#[test]
fn cbz_still_pads_unsorted_directory_numbers() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("Chapter 1/a.png"))?;
    write_png(&source.join("Chapter 2/b.png"))?;
    write_png(&source.join("Chapter 10/c.png"))?;

    let mut tree = load_tree(&source, &options(&[])?)?;
    sanitize_tree(&mut tree, &options(&["-p", "KDX"])?);

    let names: Vec<String> = tree.chapters.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, vec!["Chapter 0001", "Chapter 0002", "Chapter 0010"]);
    Ok(())
}

#[test]
fn colliding_slugs_get_an_a_suffix() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    // Both slugify to `foo-bar`; only the second (whose raw name differs from its
    // slug) takes the `A` suffix, matching KCC's `sanitizeTree`.
    write_png(&source.join("foo-bar/a.png"))?;
    write_png(&source.join("foo_bar/b.png"))?;

    let mut tree = load_tree(&source, &options(&[])?)?;
    sanitize_tree(&mut tree, &options(&["-f", "epub"])?);

    let names: Vec<String> = tree.chapters.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(names, vec!["foo-bar", "foo-barA"]);
    Ok(())
}

#[test]
fn sanitize_keeps_an_archive_and_a_folder_in_step() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("cover.png"))?;
    write_png(&source.join("Chapter 1/page.png"))?;

    let archive = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let mut from_folder = load_tree(&source, &options(&[])?)?;
    let mut from_archive = load_tree(&archive, &options(&[])?)?;
    let opts = options(&["-f", "epub"])?;
    sanitize_tree(&mut from_folder, &opts);
    sanitize_tree(&mut from_archive, &opts);

    assert_eq!(shape(&from_folder), shape(&from_archive));
    Ok(())
}

#[test]
fn output_filenames_cover_every_format() -> Result<()> {
    let tmp = tempdir()?;
    // A `.cbr` source keeps the CBZ output name (`book.cbz`) distinct from itself.
    let source = tmp.path().join("book.cbr");
    fs::write(&source, b"x")?;

    let epub = options(&["-f", "epub"])?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &epub),
        tmp.path().join("book.epub")
    );

    let kepub = options(&["-p", "KoE", "-f", "epub"])?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &kepub),
        tmp.path().join("book.kepub.epub")
    );

    let short_ext = options(&["-p", "KoE", "-f", "epub", "--kepub-short-ext"])?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &short_ext),
        tmp.path().join("book.kepub")
    );

    let plain = options(&["-p", "KoE", "-f", "epub", "--no-kepub"])?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &plain),
        tmp.path().join("book.epub")
    );

    let cbz = options(&["-p", "KDX"])?;
    assert_eq!(
        output_filename(&source, None, ".cbz", "", &cbz),
        tmp.path().join("book.cbz")
    );

    let pdf = options(&["-p", "Rmk2"])?;
    assert_eq!(
        output_filename(&source, None, ".pdf", "", &pdf),
        tmp.path().join("book.pdf")
    );
    Ok(())
}

#[test]
fn kepub_short_ext_requires_kepub_output() -> Result<()> {
    // A plain EPUB on a non-Kobo profile is not a KePub.
    let err = options(&["-f", "epub", "--kepub-short-ext"])
        .err()
        .context("the flag must be rejected without KePub output")?;
    assert!(err.to_string().contains("--kepub-short-ext"));

    // Non-EPUB formats are rejected too.
    assert!(options(&["-f", "cbz", "--kepub-short-ext"]).is_err());

    // `--no-kepub` forces plain EPUB even on a Kobo profile.
    assert!(options(&["-p", "KoE", "-f", "epub", "--no-kepub", "--kepub-short-ext"]).is_err());

    // A Kobo profile (or explicit `-f kepub`) is accepted.
    assert!(options(&["-p", "KoE", "-f", "epub", "--kepub-short-ext"]).is_ok());
    assert!(options(&["-f", "kepub", "--kepub-short-ext"]).is_ok());
    Ok(())
}

#[test]
fn tome_number_is_appended() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;

    let epub = options(&["-f", "epub"])?;
    assert_eq!(
        output_filename(&source, None, ".epub", " 2", &epub),
        tmp.path().join("book 2.epub")
    );
    Ok(())
}

#[test]
fn output_directory_and_explicit_file_are_honoured() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;
    let epub = options(&["-f", "epub"])?;

    let explicit = tmp.path().join("out.epub");
    assert_eq!(
        output_filename(&source, Some(&explicit), ".epub", "", &epub),
        explicit
    );

    let directory = tmp.path().join("outdir");
    let resolved = output_filename(&source, Some(&directory), ".epub", "", &epub);
    assert!(directory.is_dir(), "the output directory is created");
    assert_eq!(resolved, directory.join("book.epub"));
    Ok(())
}

#[test]
fn an_existing_output_is_overwritten_not_renamed() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;
    let epub = options(&["-f", "epub"])?;

    // The resolved name is deterministic and never gains a `_kcc<N>` suffix, so an
    // existing file is simply replaced on write.
    fs::write(tmp.path().join("book.epub"), b"x")?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &epub),
        tmp.path().join("book.epub")
    );
    Ok(())
}

#[test]
fn kepub_output_keeps_the_whole_extension() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;

    // The name must be exactly `book.kepub.epub` — no `.kepub_kccN` fragment.
    let kepub = options(&["-p", "KoE", "-f", "epub"])?;
    fs::write(tmp.path().join("book.kepub.epub"), b"x")?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &kepub),
        tmp.path().join("book.kepub.epub")
    );

    let short_ext = options(&["-p", "KoE", "-f", "epub", "--kepub-short-ext"])?;
    fs::write(tmp.path().join("book.kepub"), b"x")?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &short_ext),
        tmp.path().join("book.kepub")
    );
    Ok(())
}

#[test]
fn kept_intermediate_epub_uses_its_own_name() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;
    let mobi = options(&["-f", "mobi+epub"])?;

    // The intermediate EPUB keeps its deterministic name; `.epub` and `.mobi`
    // never collide, so no counter is needed.
    fs::write(tmp.path().join("book.mobi"), b"x")?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &mobi),
        tmp.path().join("book.epub")
    );
    Ok(())
}

#[test]
fn azw3_output_uses_the_deterministic_epub_name() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;
    let azw3 = options(&["-f", "azw3"])?;

    // The Kindle name is derived from the EPUB path; an existing AZW3 does not
    // rename the EPUB.
    fs::write(tmp.path().join("book.azw3"), b"x")?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &azw3),
        tmp.path().join("book.epub")
    );
    Ok(())
}

#[test]
fn an_explicit_kindle_wanted_file_resolves_to_its_epub() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("book.cbz");
    fs::write(&source, b"x")?;

    // `-o out.mobi` / `-o out.azw3` name the Kindle file, so the intermediate
    // EPUB resolves to `out.epub` (its extension is replaced back again).
    let mobi = options(&["-f", "mobi"])?;
    assert_eq!(
        output_filename(
            &source,
            Some(&tmp.path().join("out.mobi")),
            ".epub",
            "",
            &mobi
        ),
        tmp.path().join("out.epub")
    );
    let azw3 = options(&["-f", "azw3"])?;
    assert_eq!(
        output_filename(
            &source,
            Some(&tmp.path().join("out.azw3")),
            ".epub",
            "",
            &azw3
        ),
        tmp.path().join("out.epub")
    );
    Ok(())
}

#[test]
fn directory_source_output_sits_beside_the_directory() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("manga");
    write_png(&source.join("page.png"))?;

    assert_eq!(
        output_filename(&source, None, ".cbz", "", &options(&["-p", "KDX"])?),
        tmp.path().join("manga.cbz")
    );
    Ok(())
}

#[test]
fn kobo_sanitizes_the_source_name_when_the_extension_mismatches() -> Result<()> {
    let tmp = tempdir()?;
    let source = tmp.path().join("My Book.cbz");
    fs::write(&source, b"x")?;

    let kepub = options(&["-p", "KoE", "-f", "epub"])?;
    assert_eq!(
        output_filename(&source, None, ".epub", "", &kepub),
        tmp.path().join("My_Book.kepub.epub")
    );
    Ok(())
}

#[test]
fn covers_directory_selects_the_matching_index() -> Result<()> {
    let tmp = tempdir()?;
    let book1 = tmp.path().join("Book 1.cbz");
    let book2 = tmp.path().join("Book 2.cbz");
    fs::write(&book1, b"x")?;
    fs::write(&book2, b"x")?;
    let covers = tmp.path().join("Covers");
    fs::create_dir_all(&covers)?;
    fs::write(covers.join("c1.jpg"), b"x")?;
    fs::write(covers.join("c2.jpg"), b"x")?;

    assert_eq!(select_cover(&book2), Some(covers.join("c2.jpg")));
    assert_eq!(select_cover(&book1), Some(covers.join("c1.jpg")));
    Ok(())
}

#[test]
fn missing_or_short_covers_are_ignored() -> Result<()> {
    let tmp = tempdir()?;
    let book = tmp.path().join("Book 1.cbz");
    fs::write(&book, b"x")?;
    assert_eq!(select_cover(&book), None, "no Covers/ directory");

    let covers = tmp.path().join("Covers");
    fs::create_dir_all(&covers)?;
    fs::write(covers.join("only.jpg"), b"x")?;
    // Two siblings but only one cover: the second source has no match.
    let book2 = tmp.path().join("Book 2.cbz");
    fs::write(&book2, b"x")?;
    assert_eq!(select_cover(&book2), None);
    assert_eq!(select_cover(&book), Some(covers.join("only.jpg")));
    Ok(())
}

#[test]
fn comicinfo_bookmarks_are_parsed_through_prepare_book() -> Result<()> {
    // A light end-to-end check that loading, metadata and naming compose.
    let tmp = tempdir()?;
    let source = tmp.path().join("src");
    write_png(&source.join("cover.png"))?;
    write_png(&source.join("Chapter 1/page.png"))?;
    fs::write(
        source.join("ComicInfo.xml"),
        br#"<ComicInfo><Series>Berserk</Series><Page Image="1" Bookmark="Ch. 1"/></ComicInfo>"#,
    )?;

    let archive: PathBuf = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &source, &archive)?;

    let prepared = comic_book::ebook::prepare_book(&archive, &options(&["-f", "epub"])?)?;
    assert_eq!(prepared.metadata.title, "Berserk");
    assert_eq!(prepared.metadata.bookmarks, vec![(1, "Ch. 1".to_string())]);
    assert_eq!(
        prepared.sanitized.cover_path.as_deref(),
        Some("kcc-0001.png")
    );
    assert_eq!(prepared.tree.chapters[1].name.as_str(), "chapter-1");
    let comicinfo = prepared
        .tree
        .comicinfo
        .as_deref()
        .context("ComicInfo is retained")?;
    assert_eq!(ComicInfo::parse(comicinfo)?.series, "Berserk");
    Ok(())
}
