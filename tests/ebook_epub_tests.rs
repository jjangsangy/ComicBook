//! Phase 5 tests for the fixed-layout EPUB/KePub output (AGENTS.md §15, Phase 5
//! exit criterion): a fixture book converts to a structurally valid EPUB that can
//! be parsed back from container → OPF → spine → XHTML → image references.
//!
//! The output is asserted structurally rather than byte-for-byte: `dcterms:modified`
//! and the `dc:identifier` UUID are freshly generated on every run.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Parser;
use comic_book::cli::Cli;
use comic_book::ebook::options::Options;
use comic_book::ebook::{convert_source, progress};
use image::{DynamicImage, Rgb, RgbImage};
use tempfile::tempdir;
use zip::CompressionMethod;

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

/// A folder with a root page, a split spread and two chapter directories.
fn fixture(root: &Path) {
    write_png(&root.join("01-normal.png"), 100, 150, [10, 10, 10]);
    // 2.5:1 exceeds the bisect threshold, so it rotates to `-kcc-d` (a `center`
    // spread in the spine).
    write_png(&root.join("02-spread.png"), 500, 200, [255, 255, 255]);
    write_png(&root.join("Chapter 1/01.png"), 100, 150, [10, 10, 10]);
    write_png(&root.join("Chapter 1/02.png"), 100, 150, [200, 200, 200]);
    write_png(&root.join("Chapter 2/01.png"), 100, 150, [10, 10, 10]);
}

/// Run the ebook pipeline for a source and return the output paths.
fn convert(source: &Path, args: &[&str]) -> Vec<PathBuf> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let options = options(args);
    convert_source(source, &options).expect("conversion succeeds")
}

// --- a minimal EPUB reader -------------------------------------------------------

/// The entries of an EPUB, in archive order.
struct Epub {
    entries: Vec<(String, Vec<u8>, CompressionMethod)>,
}

impl Epub {
    fn open(path: &Path) -> Epub {
        let file = fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entries = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_string();
            let method = entry.compression();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            entries.push((name, data, method));
        }
        Epub { entries }
    }

    fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(entry, ..)| entry == name)
            .map(|(_, data, _)| data.as_slice())
    }

    fn text(&self, name: &str) -> String {
        String::from_utf8(
            self.bytes(name)
                .unwrap_or_else(|| panic!("missing {name}"))
                .to_vec(),
        )
        .unwrap()
    }

    fn has(&self, name: &str) -> bool {
        self.bytes(name).is_some()
    }
}

/// Extract a space-separated attribute value from an XML line.
fn attr(line: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=\"");
    let start = line.find(&key)? + key.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Resolve a relative reference against a directory inside the container.
fn resolve(base_dir: &str, reference: &str) -> String {
    let mut parts: Vec<&str> = base_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for segment in reference.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The `itemref` targets of the OPF spine, in order.
fn spine(opf: &str) -> Vec<String> {
    opf.lines()
        .filter(|line| line.starts_with("<itemref "))
        .map(|line| attr(line, "idref").expect("idref"))
        .collect()
}

/// The manifest items as `(id, href, media-type)`.
fn manifest(opf: &str) -> Vec<(String, String, String)> {
    opf.lines()
        .filter(|line| line.starts_with("<item "))
        .map(|line| {
            (
                attr(line, "id").expect("id"),
                attr(line, "href").expect("href"),
                attr(line, "media-type").expect("media-type"),
            )
        })
        .collect()
}

// --- tests -----------------------------------------------------------------------

#[test]
fn epub_is_structurally_valid() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fixture(&source);

    let outputs = convert(&source, &["-f", "epub", "-p", "KoE", "--no-kepub"]);
    assert_eq!(outputs.len(), 1);
    let epub = Epub::open(&outputs[0]);

    // `mimetype` first, stored and uncompressed, as the EPUB specification requires.
    assert_eq!(epub.entries[0].0, "mimetype");
    assert_eq!(epub.entries[0].2, CompressionMethod::Stored);
    assert_eq!(epub.text("mimetype"), "application/epub+zip");

    // container → OPF.
    let container = epub.text("META-INF/container.xml");
    assert!(container.contains("full-path=\"OEBPS/content.opf\""));
    let opf = epub.text("OEBPS/content.opf");
    assert!(opf.contains("<package version=\"3.0\""));
    assert!(opf.contains("<dc:title>book</dc:title>"));
    assert!(opf.contains("urn:uuid:"));

    // Every manifest href resolves to a real entry.
    let items = manifest(&opf);
    for (id, href, _) in &items {
        assert!(
            epub.has(&format!("OEBPS/{href}")),
            "manifest item {id} points at missing OEBPS/{href}"
        );
    }

    // The spine references manifest page items, one per page, in reading order.
    let reference: Vec<String> = items
        .iter()
        .filter(|(id, ..)| id.starts_with("page_"))
        .map(|(id, ..)| id.clone())
        .collect();
    let spine = spine(&opf);
    assert_eq!(spine, reference);
    assert_eq!(spine.len(), 5, "four pages plus the rotated spread page");

    // Every page's XHTML references an image that exists in the container.
    for id in &spine {
        let (_, href, _) = items
            .iter()
            .find(|(item, ..)| item == id)
            .expect("page item");
        let xhtml = epub.text(&format!("OEBPS/{href}"));
        assert!(xhtml.contains("<!DOCTYPE html>"));
        assert!(xhtml.contains("<meta name=\"viewport\""));
        let image = attr(
            xhtml
                .lines()
                .find(|line| line.contains("<img "))
                .expect("img"),
            "src",
        )
        .expect("src");
        let directory = href.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        assert!(
            epub.has(&format!("OEBPS/{}", resolve(directory, &image))),
            "page {href} references missing image {image}"
        );
    }

    // The navigation documents list every chapter directory.
    let nav = epub.text("OEBPS/nav.xhtml");
    assert!(nav.contains("epub:type=\"toc\""));
    assert!(nav.contains("epub:type=\"page-list\""));
    for title in ["chapter-1", "chapter-2"] {
        assert!(nav.contains(title), "nav is missing {title}");
    }
    let ncx = epub.text("OEBPS/toc.ncx");
    assert!(ncx.contains("<navPoint"));
    assert!(ncx.contains("Text/chapter-1/"));

    assert!(epub.has("OEBPS/Text/style.css"));
    assert!(epub.has("OEBPS/Images/cover.jpg"));
}

#[test]
fn kindle_epub_has_fixed_layout_and_panel_view() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    // A page larger than the K57 screen so all four Panel View quadrants appear.
    write_png(&source.join("01.png"), 800, 1200, [10, 10, 10]);
    write_png(&source.join("02-spread.png"), 500, 200, [255, 255, 255]);

    let outputs = convert(
        &source,
        &["-f", "epub", "-p", "K57", "--hq", "-a", "Jane Doe"],
    );
    let epub = Epub::open(&outputs[0]);
    let opf = epub.text("OEBPS/content.opf");

    assert!(opf.contains("<meta name=\"fixed-layout\" content=\"true\"/>"));
    assert!(opf.contains("<meta name=\"book-type\" content=\"comic\"/>"));
    assert!(opf.contains("<meta name=\"original-resolution\" content=\"600x800\"/>"));
    assert!(opf.contains("<meta name=\"orientation-lock\" content=\"none\"/>"));
    assert!(opf.contains("<dc:creator>Jane Doe</dc:creator>"));

    // Kindle spreads use the `linear`/`page-spread-*` spelling.
    assert!(opf.contains("linear=\"yes\" properties=\"page-spread-"));
    // The 2.5:1 page rotates, so it is a centred spread.
    assert!(opf.contains("properties=\"page-spread-center\""));

    // Panel View markup is present in every page.
    let xhtml = epub.text("OEBPS/Text/kcc-0001-kcc-x.xhtml");
    assert!(xhtml.contains("<div id=\"PV\">"));
    assert!(xhtml.contains("class=\"app-amzn-magnify\""));
    assert!(xhtml.contains("<div class=\"PV-P\""));
    assert!(
        epub.text("OEBPS/Text/style.css").contains("#PV {"),
        "panel-view CSS is missing"
    );
    assert!(
        xhtml.contains("<div style=\"display:none;\">.</div>"),
        "the Kindle panel-mode spacer div is missing"
    );
}

#[test]
fn kepub_has_the_kobo_extension_and_spread_properties() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fixture(&source);

    let output_dir = tmp.path().join("out");
    let outputs = convert(
        &source,
        &[
            "-f",
            "epub",
            "-p",
            "KoE",
            "-o",
            output_dir.to_str().unwrap(),
        ],
    );
    assert_eq!(
        outputs[0].file_name().and_then(|name| name.to_str()),
        Some("book.kepub.epub"),
        "Kobo output uses the .kepub.epub extension"
    );

    let epub = Epub::open(&outputs[0]);
    let opf = epub.text("OEBPS/content.opf");
    assert!(opf.contains("properties=\"rendition:page-spread-"));
    assert!(!opf.contains("linear=\"yes\""));
    assert!(!opf.contains("fixed-layout"));
}

#[test]
fn manga_sets_right_to_left_progression() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    // A spread-free book: with a rotated spread present, KCC's backward fix-up
    // pass re-anchors every earlier page, so keep this fixture simple.
    write_png(&source.join("01.png"), 100, 150, [10, 10, 10]);
    write_png(&source.join("02.png"), 100, 150, [200, 200, 200]);

    let outputs = convert(&source, &["-f", "epub", "-p", "KoE", "--no-kepub", "-m"]);
    let epub = Epub::open(&outputs[0]);
    let opf = epub.text("OEBPS/content.opf");
    assert!(opf.contains("<spine page-progression-direction=\"rtl\""));
    let spine = spine(&opf);
    assert!(
        opf.contains(&format!(
            "<itemref idref=\"{}\" properties=\"rendition:page-spread-right\"/>",
            spine[0]
        )),
        "a right-to-left book starts on the right"
    );
    assert!(
        opf.contains(&format!(
            "<itemref idref=\"{}\" properties=\"rendition:page-spread-left\"/>",
            spine[1]
        )),
        "the second page turns to the left"
    );
}

#[test]
fn no_processing_copies_images_verbatim() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    write_png(&source.join("01.png"), 100, 150, [10, 10, 10]);
    let pristine = fs::read(source.join("01.png")).unwrap();

    let outputs = convert(&source, &["-f", "epub", "-p", "KoE", "--no-kepub", "-n"]);
    let epub = Epub::open(&outputs[0]);

    assert_eq!(
        epub.bytes("OEBPS/Images/kcc-0001.png").unwrap(),
        pristine.as_slice(),
        "--no-processing must emit the source bytes untouched"
    );
}

#[test]
fn comicinfo_bookmarks_name_the_navigation_entries() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fixture(&source);
    fs::write(
        source.join("ComicInfo.xml"),
        br#"<ComicInfo><Series>Berserk</Series><Page Image="1" Bookmark="The Black Swordsman"/></ComicInfo>"#,
    )
    .unwrap();

    let outputs = convert(&source, &["-f", "epub", "-p", "KoE", "--no-kepub"]);
    let epub = Epub::open(&outputs[0]);

    let nav = epub.text("OEBPS/nav.xhtml");
    assert!(nav.contains("The Black Swordsman"));
    assert!(epub.text("OEBPS/toc.ncx").contains("The Black Swordsman"));
    // Bookmark chapters suppress the directory-based chapter list.
    assert!(!nav.contains("chapter-1"));
    // The series is recorded for non-Kindle readers.
    assert!(epub
        .text("OEBPS/content.opf")
        .contains("belongs-to-collection"));
}

#[test]
fn supported_formats_are_reported_when_unimplemented() {
    // CBZ/PDF/Kindle land in later phases; the error must be clear, not silent.
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fixture(&source);

    let options = options(&["-f", "pdf", "-p", "Rmk1"]);
    let error = convert_source(&source, &options).unwrap_err().to_string();
    assert!(
        error.contains("not implemented"),
        "unexpected error: {error}"
    );
}

/// A book with no spread specials, so the spread algorithm is easy to read.
fn simple_fixture(root: &Path) {
    write_png(&root.join("01.png"), 100, 150, [10, 10, 10]);
    write_png(&root.join("02.png"), 100, 150, [200, 200, 200]);
}

#[test]
fn scribe_profile_splits_a_tall_page_into_above_and_below() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    // Larger than the KS profile, so it is contain-resized to 1653x2480 and then
    // split at 1920 (cropping disabled to keep the geometry predictable).
    write_png(&source.join("01.png"), 2000, 3000, [10, 10, 10]);

    let outputs = convert(&source, &["-f", "epub", "-p", "KS", "-c", "0"]);
    let epub = Epub::open(&outputs[0]);

    // The tall page becomes two images; the unsplit name is never written.
    assert!(epub.has("OEBPS/Images/kcc-0001-kcc-x-above.jpg"));
    assert!(epub.has("OEBPS/Images/kcc-0001-kcc-x-below.jpg"));
    assert!(!epub.has("OEBPS/Images/kcc-0001-kcc-x.jpg"));

    // The XHTML stacks both images and the viewport spans their combined height.
    let xhtml = epub.text("OEBPS/Text/kcc-0001-kcc-x-above.xhtml");
    assert!(xhtml.contains(
        "<img width=\"1653\" height=\"1920\" src=\"../Images/kcc-0001-kcc-x-above.jpg\"/>"
    ));
    assert!(xhtml.contains(
        "<img style=\"top: 1920px\" width=\"1653\" height=\"560\" src=\"../Images/kcc-0001-kcc-x-below.jpg\"/>"
    ));
    assert!(xhtml.contains("content=\"width=1653, height=2480\""));

    let opf = epub.text("OEBPS/content.opf");
    // The `-below` image is in the manifest but never a spine item of its own.
    assert!(opf.contains("href=\"Images/kcc-0001-kcc-x-below.jpg\""));
    let spine = spine(&opf);
    assert_eq!(spine.len(), 1);
    assert!(spine[0].contains("above"));
    // Every manifest href still resolves (the below image included).
    for (id, href, _) in manifest(&opf) {
        assert!(
            epub.has(&format!("OEBPS/{href}")),
            "manifest item {id} points at missing OEBPS/{href}"
        );
    }
}

#[test]
fn scribe_profile_names_a_short_page_whole() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    write_png(&source.join("01.png"), 100, 150, [10, 10, 10]);

    let outputs = convert(&source, &["-f", "epub", "-p", "KS", "-c", "0"]);
    let epub = Epub::open(&outputs[0]);
    assert!(epub.has("OEBPS/Images/kcc-0001-kcc-x-whole.jpg"));
    assert!(!epub.has("OEBPS/Images/kcc-0001-kcc-x.jpg"));
}

#[test]
fn one_page_landscape_centres_every_spine_item() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    simple_fixture(&source);

    let outputs = convert(
        &source,
        &["-f", "epub", "-p", "K57", "--one-page-landscape"],
    );
    let opf = Epub::open(&outputs[0]).text("OEBPS/content.opf");
    let itemrefs: Vec<&str> = opf
        .lines()
        .filter(|line| line.starts_with("<itemref "))
        .collect();
    assert_eq!(itemrefs.len(), 2);
    assert!(itemrefs
        .iter()
        .all(|line| line.contains("properties=\"page-spread-center\"")));
}

#[test]
fn spread_shift_flips_the_opening_side() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    simple_fixture(&source);

    let plain_dir = tmp.path().join("plain");
    let plain = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "KoE",
                "--no-kepub",
                "-o",
                plain_dir.to_str().unwrap(),
            ],
        )[0],
    );
    let shifted_dir = tmp.path().join("shifted");
    let shifted = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "KoE",
                "--no-kepub",
                "--spread-shift",
                "-o",
                shifted_dir.to_str().unwrap(),
            ],
        )[0],
    );

    let plain_spine = spine(&plain.text("OEBPS/content.opf"));
    let plain_opf = plain.text("OEBPS/content.opf");
    assert!(plain_opf.contains(&format!(
        "<itemref idref=\"{}\" properties=\"rendition:page-spread-left\"/>",
        plain_spine[0]
    )));

    let shift_opf = shifted.text("OEBPS/content.opf");
    let shift_spine = spine(&shift_opf);
    assert!(shift_opf.contains(&format!(
        "<itemref idref=\"{}\" properties=\"rendition:page-spread-right\"/>",
        shift_spine[0]
    )));
}

#[test]
fn invert_direction_reverses_progression_and_writing_mode() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    simple_fixture(&source);

    let normal_dir = tmp.path().join("normal");
    let normal = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "K57",
                "-o",
                normal_dir.to_str().unwrap(),
            ],
        )[0],
    );
    let normal_opf = normal.text("OEBPS/content.opf");
    assert!(normal_opf.contains("<spine page-progression-direction=\"ltr\""));
    assert!(normal_opf.contains("primary-writing-mode\" content=\"horizontal-lr\""));

    let inverted_dir = tmp.path().join("inverted");
    let inverted = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "K57",
                "--invert-direction",
                "-o",
                inverted_dir.to_str().unwrap(),
            ],
        )[0],
    );
    let inverted_opf = inverted.text("OEBPS/content.opf");
    assert!(inverted_opf.contains("<spine page-progression-direction=\"rtl\""));
    assert!(inverted_opf.contains("primary-writing-mode\" content=\"horizontal-rl\""));
}

#[test]
fn two_panel_and_vertical_4_panel_reshape_the_panel_view() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    // Larger than the K57 screen so Panel View has content to magnify.
    write_png(&source.join("01.png"), 800, 1200, [10, 10, 10]);

    // `--hq` alone lays out four quadrants.
    let quad_dir = tmp.path().join("quad");
    let quad = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "K57",
                "--hq",
                "-c",
                "0",
                "-o",
                quad_dir.to_str().unwrap(),
            ],
        )[0],
    );
    let quad_xhtml = quad.text("OEBPS/Text/kcc-0001-kcc-x.xhtml");
    assert!(quad_xhtml.contains("<div id=\"PV-TL\">"));
    assert!(quad_xhtml.contains("<div id=\"PV-BR\">"));

    // `-2/--two-panel` scales the page to the device width, leaving only the
    // vertical pair of panels.
    let two_dir = tmp.path().join("two");
    let two = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "K57",
                "--hq",
                "--two-panel",
                "-c",
                "0",
                "-o",
                two_dir.to_str().unwrap(),
            ],
        )[0],
    );
    let two_xhtml = two.text("OEBPS/Text/kcc-0001-kcc-x.xhtml");
    assert!(two_xhtml.contains("<div id=\"PV-T\">"));
    assert!(two_xhtml.contains("<div id=\"PV-B\">"));
    assert!(!two_xhtml.contains("PV-TL"));

    // `--vertical-4-panel` writes a vertical reading mode.
    let vertical_dir = tmp.path().join("vertical");
    let vertical = Epub::open(
        &convert(
            &source,
            &[
                "-f",
                "epub",
                "-p",
                "K57",
                "--hq",
                "--vertical-4-panel",
                "-o",
                vertical_dir.to_str().unwrap(),
            ],
        )[0],
    );
    assert!(vertical
        .text("OEBPS/content.opf")
        .contains("primary-writing-mode\" content=\"vertical-lr\""));
}

#[test]
fn smart_cover_crop_takes_a_single_side_and_the_cover_is_fitted() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    // A 2:1 spread: the smart crop keeps the right (non-manga) half, so the cover
    // ends up portrait once thumbnailed.
    write_png(&source.join("01.png"), 2000, 1000, [255, 255, 255]);

    let outputs = convert(
        &source,
        &[
            "-f",
            "epub",
            "-p",
            "KoE",
            "--no-kepub",
            "--smart-cover-crop",
        ],
    );
    let epub = Epub::open(&outputs[0]);
    let cover = image::load_from_memory(epub.bytes("OEBPS/Images/cover.jpg").unwrap()).unwrap();
    assert!(
        cover.width() < cover.height(),
        "the wide spread was not cropped"
    );
}

#[test]
fn cover_is_taken_from_the_uncropped_first_page() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fs::create_dir_all(&source).unwrap();
    // A white page with a small dark block: `-c 1` crops the page down to the
    // block, but KCC builds the cover before processing, so it keeps 400x600.
    let mut page = RgbImage::from_pixel(400, 600, Rgb([255, 255, 255]));
    for y in 250..350 {
        for x in 150..250 {
            page.put_pixel(x, y, Rgb([0, 0, 0]));
        }
    }
    DynamicImage::ImageRgb8(page)
        .save(source.join("01.png"))
        .unwrap();

    let outputs = convert(
        &source,
        &["-f", "epub", "-p", "KoE", "--no-kepub", "-c", "1"],
    );
    let epub = Epub::open(&outputs[0]);
    let cover = image::load_from_memory(epub.bytes("OEBPS/Images/cover.jpg").unwrap()).unwrap();
    assert_eq!((cover.width(), cover.height()), (400, 600));

    // The processed page itself was margin-cropped.
    let processed =
        image::load_from_memory(epub.bytes("OEBPS/Images/kcc-0001-kcc-x.jpg").unwrap()).unwrap();
    assert!(processed.width() < 400);
}

/// Opt-in EPUB conformance check (AGENTS.md §16); run with
/// `cargo test -- --ignored epubcheck` when `epubcheck` is on `PATH`.
#[test]
#[ignore = "requires the epubcheck command on PATH"]
fn epubcheck_accepts_a_generated_epub() {
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fixture(&source);
    let outputs = convert(&source, &["-f", "epub", "-p", "KoE", "--no-kepub"]);

    let status = std::process::Command::new("epubcheck")
        .arg(&outputs[0])
        .status();
    let Ok(status) = status else {
        eprintln!("epubcheck is not installed; skipping the conformance check");
        return;
    };
    assert!(
        status.success(),
        "epubcheck rejected {}",
        outputs[0].display()
    );
}
