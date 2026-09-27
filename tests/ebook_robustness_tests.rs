//! Hardening tests.
//!
//! The `ebook` pipeline reads untrusted, frequently-broken files: truncated
//! downloads, hand-edited archives, Windows-authored ZIPs and hostile entry
//! paths. Hardening means such input is **rejected with an error, never a
//! panic**, and that the same code scales to a large book and stays
//! platform-agnostic.
//!
//! Every test here wraps the library entry point in [`catch_unwind`] so a panic
//! is reported as a test failure with the offending input rather than as an
//! opaque abort, and the fuzzers drive deterministic pseudo-random bytes through
//! [`load_tree`]/[`convert_source`] so a regression is reproducible.

use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use clap::Parser;
use comic_book::archive::{compress_archive, ArchiveKind};
use comic_book::cli::Cli;
use comic_book::ebook::convert_source;
use comic_book::ebook::input::load_tree;
use comic_book::ebook::model::ComicTree;
use comic_book::ebook::options::Options;
use comic_book::ebook::progress;
use image::{DynamicImage, Rgb, RgbImage};
use tempfile::tempdir;

// --- helpers -----------------------------------------------------------------

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

/// Encode an in-memory RGB image as PNG bytes for raw archive entries.
fn png_bytes(width: u32, height: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([0, 0, 0])))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Run [`load_tree`], turning a panic into an `Err` so the test can report it.
fn load_catching(path: &Path, options: &Options) -> Result<Result<ComicTree>> {
    match catch_unwind(AssertUnwindSafe(|| load_tree(path, options))) {
        Ok(result) => Ok(result),
        Err(_) => bail!("load_tree panicked for {}", path.display()),
    }
}

/// Run the full conversion pipeline, turning a panic into an `Err`.
fn convert_catching(source: &Path, options: &Options) -> Result<Result<Vec<PathBuf>>> {
    match catch_unwind(AssertUnwindSafe(|| convert_source(source, options))) {
        Ok(result) => Ok(result),
        Err(_) => bail!("convert_source panicked for {}", source.display()),
    }
}

/// Write a raw ZIP with the given `(entry name, payload)` pairs, bypassing the
/// library's path normalization so the reader itself is exercised.
fn write_raw_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let file = fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    for (name, data) in entries {
        zip.start_file(*name, SimpleFileOptions::default())?;
        zip.write_all(data)?;
    }
    zip.finish()?;
    Ok(())
}

/// A tiny deterministic xorshift64* generator, so fuzz failures reproduce.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() >> 33) as u8).collect()
    }
}

// --- malformed archives ------------------------------------------------------

/// Build a valid book of `pages` solid pages of the given size and return its
/// source folder.
fn build_source(root: &Path, pages: u32, width: u32, height: u32) -> Result<PathBuf> {
    let source = root.join("source");
    for index in 0..pages {
        let shade = (index * 7 % 250) as u8;
        write_png(
            &source.join(format!("page{index:04}.png")),
            width,
            height,
            [shade, 128, 200 - shade / 2],
        )?;
    }
    Ok(source)
}

/// Every truncated prefix of a valid archive is rejected (or partially loaded),
/// never a panic. The truncation drops the format's trailing index/structure, so
/// most prefixes fail to open.
#[test]
fn truncated_archives_are_rejected_without_panicking() -> Result<()> {
    let tmp = tempdir()?;
    let source = build_source(tmp.path(), 3, 24, 32)?;

    for (extension, kind) in [
        ("cbz", ArchiveKind::Cbz),
        ("cbr", ArchiveKind::Cbr),
        ("cb7", ArchiveKind::Cb7),
        ("cbt", ArchiveKind::Cbt),
    ] {
        let archive = tmp.path().join(format!("book.{extension}"));
        compress_archive(kind, &source, &archive)?;
        let bytes = fs::read(&archive)?;

        let cuts = [
            1,
            bytes.len() / 4,
            bytes.len() / 2,
            bytes.len() * 3 / 4,
            bytes.len() - 1,
        ];
        for (index, cut) in cuts.into_iter().enumerate() {
            if cut == 0 || cut >= bytes.len() {
                continue;
            }
            let truncated = tmp.path().join(format!("trunc{index}.{extension}"));
            fs::write(&truncated, &bytes[..cut])?;

            let result = load_catching(&truncated, &options(&[])?)
                .map_err(|error| anyhow::anyhow!("{extension} @{cut}: {error}"))?;
            // Either rejected outright or partially loaded, but never an empty
            // tree and never a panic.
            if let Ok(tree) = result {
                assert!(
                    tree.page_count() > 0,
                    "{extension} truncated to {cut} loaded no pages"
                );
            }
        }
    }
    Ok(())
}

/// Deterministic pseudo-random bytes are rejected cleanly for every extension,
/// including when a valid magic prefix is planted to force the reader to be
/// reached.
#[test]
fn fuzzed_archive_and_document_bytes_never_panic() -> Result<()> {
    let tmp = tempdir()?;
    let options = options(&[])?;
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);

    // Magic prefixes that make `detect_source_kind` route into each reader.
    let cases: [(&str, &[u8]); 5] = [
        ("cbz", b"PK\x03\x04"),
        ("cbt", b""),
        ("cb7", b"7z\xbc\xaf\x27\x1c"),
        ("epub", b"PK\x03\x04"),
        ("pdf", b"%PDF-1.7"),
    ];

    for round in 0..64usize {
        for (extension, magic) in cases {
            let mut body = magic.to_vec();
            let len = 32 + (rng.next_u64() as usize % 2048);
            body.extend_from_slice(&rng.bytes(len));

            let path = tmp
                .path()
                .join(format!("fuzz{round}-{extension}.{extension}"));
            fs::write(&path, &body)?;

            // The only contract is "no panic"; a rejected input is the norm.
            let _ = load_catching(&path, &options)?;
        }
    }
    Ok(())
}

/// Hostile entry names (`..`, absolute paths, drive letters, empty names) are
/// neutralized by the reader: nothing escapes the image root and every loaded
/// page name is interior and forward-slashed.
#[test]
fn hostile_archive_entry_names_are_neutralized() -> Result<()> {
    let tmp = tempdir()?;
    let png = png_bytes(8, 8)?;
    let archive = tmp.path().join("hostile.cbz");

    write_raw_zip(
        &archive,
        &[
            ("../../escape.png", &png),
            ("/absolute.png", &png),
            ("C:\\windows\\evil.png", &png),
            ("..\\..\\evil2.png", &png),
            ("Chapter/../../../escape2.png", &png),
            ("", &png),
            ("Chapter/ok.png", &png),
        ],
    )?;

    let tree = load_catching(&archive, &options(&[])?)?;
    let tree = tree?;
    assert!(tree.page_count() >= 1, "at least the valid page must load");

    for chapter in &tree.chapters {
        assert!(
            !chapter.name.starts_with('/') && !chapter.name.contains(".."),
            "chapter escaped the root: {:?}",
            chapter.name
        );
        for page in &chapter.pages {
            assert!(
                !page.source_name.starts_with('/') && !page.source_name.contains(".."),
                "page escaped the root: {:?}",
                page.source_name
            );
            assert!(
                !page.source_name.contains('\\'),
                "page name kept a backslash: {:?}",
                page.source_name
            );
        }
    }
    Ok(())
}

/// Windows-authored archive entries (backslash separators) load as ordinary
/// nested chapters, so a CBZ behaves the same on every platform.
#[test]
fn windows_authored_archive_entries_load_as_nested_chapters() -> Result<()> {
    let tmp = tempdir()?;
    let png = png_bytes(10, 12)?;
    let archive = tmp.path().join("windows.cbz");
    write_raw_zip(
        &archive,
        &[
            ("book\\Chapter 1\\p01.png", &png),
            ("book/Chapter 2/p01.png", &png),
        ],
    )?;

    let tree = load_catching(&archive, &options(&[])?)?;
    let tree = tree?;
    // The single `book/` wrapper is flattened; both chapters remain.
    let names: Vec<&str> = tree
        .chapters
        .iter()
        .map(|chapter| chapter.name.as_str())
        .collect();
    assert_eq!(names, vec!["Chapter 1", "Chapter 2"]);
    Ok(())
}

/// Chapter/page names are slugified to ASCII, so an EPUB written on any platform
/// never embeds a Windows-reserved character (see docs/development.md).
#[test]
fn slugified_names_avoid_windows_reserved_characters() {
    use comic_book::ebook::naming::slugify;
    use comic_book::ebook::options::Format;

    let raw_names = [
        "a:b?c*d|e",
        "..\\..\\escape",
        "\\server\\share",
        "<chapter>",
        "trailing. ",
        "quote\"and'apostrophe",
        "\u{1F600} emoji",
    ];
    for raw in raw_names {
        let slug = slugify(raw, Format::Epub, false);
        for forbidden in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            assert!(
                !slug.contains(forbidden),
                "{raw:?} → {slug:?} kept the reserved character {forbidden:?}"
            );
        }
        assert!(
            !slug.ends_with('.') && !slug.ends_with(' '),
            "{raw:?} → {slug:?} ends with a character Windows trims"
        );
        assert!(slug.is_ascii(), "{raw:?} → {slug:?} is not ASCII");
    }
}

// --- malformed EPUB / PDF ----------------------------------------------------

/// A named EPUB layout: a case label plus the archive's `(entry name, payload)`
/// pairs.
type EpubCase<'a> = (&'a str, Vec<(&'a str, &'a [u8])>);

/// Assemble a folder, then wrap it as an `.epub` (a plain ZIP).
fn build_epub(root: &Path, layout: &[(&str, &[u8])]) -> Result<PathBuf> {
    let src = root.join("epub-src");
    fs::create_dir_all(&src)?;
    for (name, data) in layout {
        let path = src.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)?;
    }
    let epub = root.join("book.epub");
    compress_archive(ArchiveKind::Cbz, &src, &epub)?;
    Ok(epub)
}

/// A range of structurally broken EPUBs must fail with an error instead of
/// panicking: a missing/garbled container, a container that names an absent OPF,
/// and a spine that resolves to nothing.
#[test]
fn malformed_epub_containers_error_without_panicking() -> Result<()> {
    let tmp = tempdir()?;
    let png = png_bytes(6, 6)?;
    let options = options(&[])?;

    let cases: Vec<EpubCase> = vec![
        ("no-container", vec![("OEBPS/Images/page.png", &png)]),
        (
            "garbled-container",
            vec![
                ("META-INF/container.xml", b"<<<not xml>>>"),
                ("OEBPS/Images/page.png", &png),
            ],
        ),
        (
            "missing-opf",
            vec![
                (
                    "META-INF/container.xml",
                    br#"<container><rootfiles><rootfile full-path="OEBPS/missing.opf"/></rootfiles></container>"#,
                ),
                ("OEBPS/Images/page.png", &png),
            ],
        ),
        (
            "no-rootfile",
            vec![
                ("META-INF/container.xml", b"<container></container>"),
                ("OEBPS/Images/page.png", &png),
            ],
        ),
        (
            "escaping-rootfile",
            vec![
                (
                    "META-INF/container.xml",
                    br#"<container><rootfiles><rootfile full-path="../../../../etc/passwd"/></rootfiles></container>"#,
                ),
                ("OEBPS/Images/page.png", &png),
            ],
        ),
    ];

    for (label, layout) in cases {
        let dir = tmp.path().join(label);
        let epub = build_epub(&dir, &layout)?;
        let tree =
            load_catching(&epub, &options)?.map_err(|error| anyhow::anyhow!("{label}: {error}"));
        assert!(
            tree.is_err(),
            "{label}: a malformed EPUB should fail, not load"
        );
    }

    // A container that parses but whose spine yields no image falls back to the
    // plain archive; with no images at all that too must error.
    let dir = tmp.path().join("empty-spine");
    let epub = build_epub(
        &dir,
        &[
            (
                "META-INF/container.xml",
                br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#,
            ),
            (
                "OEBPS/content.opf",
                br#"<package><manifest/><spine><itemref idref="missing"/></spine></package>"#,
            ),
            ("OEBPS/notes.txt", b"not an image"),
        ],
    )?;
    assert!(load_catching(&epub, &options)?.is_err());
    Ok(())
}

/// A PDF that cannot be parsed (a stub, random bytes, a truncated header) errors
/// cleanly on both the rasterise and the `--legacy-extract` paths.
#[test]
fn malformed_pdf_sources_error_without_panicking() -> Result<()> {
    let tmp = tempdir()?;
    let mut rng = Rng(0xDEAD_BEEF_CAFE_1234);

    let bodies: [Vec<u8>; 4] = [
        b"stub".to_vec(),
        b"%PDF-1.7\n".to_vec(),
        rng.bytes(512),
        b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer<<>>\n%%EOF".to_vec(),
    ];

    for (index, body) in bodies.iter().enumerate() {
        for args in [Vec::new(), vec!["--legacy-extract"]] {
            let path = tmp.path().join(format!("broken{index}.pdf"));
            fs::write(&path, body)?;
            let tree = load_catching(&path, &options(&args)?)?
                .map_err(|error| anyhow::anyhow!("broken{index} {args:?}: {error}"));
            assert!(
                tree.is_err(),
                "broken{index} {args:?}: a malformed PDF should fail, not load"
            );
        }
    }
    Ok(())
}

/// The full pipeline (not just the loader) rejects corrupt sources with an error.
#[test]
fn corrupt_sources_fail_the_full_pipeline_without_panicking() -> Result<()> {
    let tmp = tempdir()?;

    // A `.cbz` that is not a ZIP at all.
    let junk = tmp.path().join("junk.cbz");
    fs::write(&junk, b"this is definitely not a zip archive")?;
    assert!(convert_catching(&junk, &options(&["-f", "cbz"])?)?.is_err());

    // A valid ZIP whose only entry decodes to nothing usable.
    let empty = tmp.path().join("empty.cbz");
    write_raw_zip(&empty, &[("page.png", b"not a real png")])?;
    assert!(convert_catching(&empty, &options(&["-f", "cbz"])?)?.is_err());
    Ok(())
}

// --- CLI validation ----------------------------------------------------------

/// The enumerated processing modes mirror KCC's argparse `choices`: anything
/// outside `0..=2` is rejected before the pipeline runs.
#[test]
fn out_of_range_processing_modes_are_rejected() {
    for flag in ["--splitter", "--cropping", "--inter-panel-crop"] {
        for value in ["0", "1", "2"] {
            assert!(
                Cli::try_parse_from(["comic-book", "ebook", "book.cbz", flag, value]).is_ok(),
                "{flag} {value} should parse"
            );
        }
        for value in ["3", "255", "-1"] {
            assert!(
                Cli::try_parse_from(["comic-book", "ebook", "book.cbz", flag, value]).is_err(),
                "{flag} {value} should be rejected"
            );
        }
    }
}

// --- scale & memory ----------------------------------------------------------

/// Peak resident set size of this process in bytes, when the platform exposes it.
///
/// `cargo nextest` runs each test in its own process, so this reflects only the
/// conversion under test. Windows has no `getrusage`; callers skip the assertion
/// there.
#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    use std::mem::MaybeUninit;

    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `usage` points at valid, writable memory for a `rusage`, which
    // `getrusage` initializes.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: `getrusage` returned success, so the struct is initialized.
    let usage = unsafe { usage.assume_init() };
    let raw = usage.ru_maxrss as u64;
    // Linux reports kilobytes, macOS bytes.
    Some(if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    })
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

/// Count the `.xhtml` pages of an EPUB without pulling in an EPUB reader.
fn count_epub_pages(path: &Path) -> Result<usize> {
    let file = fs::File::open(path)?;
    let archive = zip::ZipArchive::new(file)?;
    Ok(archive
        .file_names()
        .filter(|name| name.starts_with("OEBPS/Text/") && name.ends_with(".xhtml"))
        .count())
}

/// A 128-page book converts to a complete EPUB, and peak memory stays under a
/// generous ceiling. The ceiling is a regression guard against an accidental
/// whole-book duplication (see docs/architecture.md), not a tight budget.
#[test]
#[ignore = "slow: 128-page book; run with --run-ignored"]
fn a_large_book_converts_under_a_memory_ceiling() -> Result<()> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let tmp = tempdir()?;
    let source = build_source(tmp.path(), 128, 256, 384)?;

    let written = convert_catching(&source, &options(&["-f", "epub"])?)??;
    assert_eq!(written.len(), 1);
    assert_eq!(count_epub_pages(&written[0])?, 128);

    if let Some(peak) = peak_rss_bytes() {
        eprintln!("peak RSS for 128 pages: {} MiB", peak / (1024 * 1024));
        assert!(
            peak < 512 * 1024 * 1024,
            "peak RSS {peak} bytes exceeds the 512 MiB hardening ceiling"
        );
    }
    Ok(())
}

/// A deliberately oversized book for manual memory checks:
/// `cargo nextest run --run-ignored ignored-only ebook_robustness`.
#[test]
#[ignore = "stress test; run manually to profile a large book"]
fn huge_book_stress() -> Result<()> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let tmp = tempdir()?;
    let source = build_source(tmp.path(), 600, 256, 384)?;

    let written = convert_catching(&source, &options(&["-f", "epub"])?)??;
    assert_eq!(written.len(), 1);
    assert_eq!(count_epub_pages(&written[0])?, 600);

    if let Some(peak) = peak_rss_bytes() {
        eprintln!("peak RSS for 600 pages: {} MiB", peak / (1024 * 1024));
    }
    Ok(())
}

/// The decoded-bytes size of a `width`x`height` RGB page.
///
/// This is the amount the pre-lazy pipeline retained for *every* source page (an
/// `ImageRgb8` buffer), so it is the unit the streaming test below measures peak
/// memory against.
fn decoded_rgb_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 3
}

/// Build a `pages`-page source whose pages are all the same encoded image.
///
/// The image is encoded once and the bytes copied per file, so building the input
/// costs one PNG encode rather than `pages` of them.
fn build_uniform_source(root: &Path, pages: u32, width: u32, height: u32) -> Result<PathBuf> {
    let source = root.join("source");
    fs::create_dir_all(&source)?;
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([12, 128, 200]))).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )?;
    for index in 0..pages {
        fs::write(source.join(format!("page{index:04}.png")), &bytes)?;
    }
    Ok(source)
}

/// A large book is ingested and repackaged without ever holding its decoded
/// pixels, so peak memory tracks the archive size rather than the decoded book.
///
/// Ingest reads each page's header for its dimensions and keeps only the encoded
/// bytes ([`Page`](comic_book::ebook::model::Page) decodes lazily), and
/// `--no-processing` copies those bytes straight through. Peak memory here is
/// therefore a small fraction of the decoded book: decoding every page up front —
/// as the pipeline did before lazy ingest — could not pass this ceiling. The test
/// is deliberately `--no-processing` so it pins the *ingest* cost without paying
/// the (debug-slow) resampler. `--run-ignored` only.
#[test]
#[ignore = "slow: 256 large pages; run with --run-ignored"]
fn ingest_and_repack_stay_far_below_the_decoded_book_size() -> Result<()> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let tmp = tempdir()?;
    let (width, height, pages) = (1024, 1400, 256);
    let source = build_uniform_source(tmp.path(), pages, width, height)?;

    // Loading alone must not decode: the tree carries the bytes and dimensions.
    let tree = load_tree(&source, &options(&[])?)?;
    assert_eq!(tree.page_count(), pages as usize);
    for chapter in &tree.chapters {
        for page in &chapter.pages {
            assert!(
                page.decoded().is_none(),
                "ingest decoded {}",
                page.source_name
            );
        }
    }

    let written = convert_catching(&source, &options(&["-f", "epub", "--no-processing"])?)??;
    assert_eq!(written.len(), 1);
    assert_eq!(count_epub_pages(&written[0])?, pages as usize);

    let decoded = decoded_rgb_bytes(width, height) * u64::from(pages);
    if let Some(peak) = peak_rss_bytes() {
        eprintln!(
            "peak RSS for {pages} x {width}x{height} (--no-processing): {} MiB \
             (decoded book {} MiB)",
            peak >> 20,
            decoded >> 20
        );
        assert!(
            peak < decoded / 10,
            "peak RSS {peak} bytes is not clearly below the {decoded}-byte decoded book"
        );
    }
    Ok(())
}
