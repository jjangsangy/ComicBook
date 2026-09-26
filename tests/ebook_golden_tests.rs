//! Byte-level golden tests for the generated EPUB documents (AGENTS.md §5.2).
//!
//! The OPF/NCX/NAV/XHTML layout is device-sensitive and must be reproduced exactly.
//! These tests pin the current output byte-for-byte against reference copies in
//! `tests/fixtures/epub_golden/`, so the templating refactor can be proven
//! behaviour-preserving.
//!
//! The only volatile fields are the `dc:identifier`/`dtb:uid` UUID and the
//! `dcterms:modified` timestamp; both are normalised before comparison.
//!
//! Regenerate the references (only when an intentional format change is made):
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo nextest run --test ebook_golden_tests
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Parser;
use comic_book::cli::Cli;
use comic_book::ebook::options::Options;
use comic_book::ebook::{convert_source, progress};
use image::{DynamicImage, Rgb, RgbImage};
use tempfile::tempdir;

/// Resolve options from a `comic-book ebook` command line.
fn options(args: &[&str]) -> Options {
    let mut full = vec!["comic-book", "ebook", "book"];
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

/// A folder with a root page, a rotated spread and two chapter directories.
///
/// The first page is larger than the Kindle screen so Panel View has real
/// quadrants to lay out (see the `kindle_panel` scenario).
fn fixture(root: &Path, comicinfo: bool) {
    write_png(&root.join("01-normal.png"), 800, 1200, [10, 10, 10]);
    // 2.5:1 exceeds the bisect threshold, so it rotates to `-kcc-d` (`center` spread).
    write_png(&root.join("02-spread.png"), 500, 200, [255, 255, 255]);
    write_png(&root.join("Chapter 1/01.png"), 100, 150, [10, 10, 10]);
    write_png(&root.join("Chapter 1/02.png"), 100, 150, [200, 200, 200]);
    write_png(&root.join("Chapter 2/01.png"), 100, 150, [10, 10, 10]);
    if comicinfo {
        fs::write(
            root.join("ComicInfo.xml"),
            br#"<ComicInfo><Series>Berserk</Series><Volume>3</Volume><Number>7</Number><Summary>A &amp; B</Summary><Page Image="1" Bookmark="The Black Swordsman"/></ComicInfo>"#,
        )
        .unwrap();
    }
}

/// One golden scenario: a fixture + command line.
struct Scenario {
    name: &'static str,
    args: &'static [&'static str],
    comicinfo: bool,
}

/// The documents pinned for every scenario, besides the page XHTML.
const FIXED_DOCS: &[&str] = &[
    "META-INF/container.xml",
    "OEBPS/Text/style.css",
    "OEBPS/content.opf",
    "OEBPS/toc.ncx",
    "OEBPS/nav.xhtml",
];

/// Run the pipeline and return every EPUB entry as `name -> bytes`.
fn run(scenario: &Scenario) -> BTreeMap<String, Vec<u8>> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let tmp = tempdir().unwrap();
    let source = tmp.path().join("book");
    fixture(&source, scenario.comicinfo);
    let out_dir = tmp.path().join("out");

    let mut argv: Vec<&str> = vec!["-o", out_dir.to_str().unwrap()];
    argv.extend_from_slice(scenario.args);
    let options = options(&argv);
    let outputs = convert_source(&source, &options).expect("conversion succeeds");
    assert_eq!(outputs.len(), 1, "expected a single output file");

    read_entries(&outputs[0])
}

/// Read all entries of an EPUB into a map.
fn read_entries(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let file = fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        entries.insert(name, data);
    }
    entries
}

/// Replace the volatile UUID and timestamp so a document is comparable.
fn normalize(name: &str, text: String) -> String {
    if !(name.ends_with("content.opf") || name.ends_with("toc.ncx")) {
        return text;
    }
    let text = replace_uuid(&text);
    replace_timestamp(&text)
}

/// Replace every `urn:uuid:<uuid>` with a placeholder.
fn replace_uuid(text: &str) -> String {
    const MARKER: &str = "urn:uuid:";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(MARKER) {
        out.push_str(&rest[..pos + MARKER.len()]);
        let after = &rest[pos + MARKER.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
            .unwrap_or(after.len());
        out.push_str("UUID");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Replace the text content of the `dcterms:modified` meta element.
fn replace_timestamp(text: &str) -> String {
    const OPEN: &str = "<meta property=\"dcterms:modified\">";
    let Some(start) = text.find(OPEN) else {
        return text.to_string();
    };
    let content = start + OPEN.len();
    let Some(rel) = text[content..].find("</meta>") else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..content]);
    out.push_str("TIMESTAMP");
    out.push_str(&text[content + rel..]);
    out
}

/// Sanitize a document name into a golden filename.
fn golden_file(dir: &Path, name: &str) -> PathBuf {
    dir.join(name.replace('/', "__"))
}

fn check_scenario(scenario: &Scenario) {
    let entries = run(scenario);
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/epub_golden")
        .join(scenario.name);
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();

    // The fixed documents plus every generated page XHTML, in archive order.
    let mut docs: Vec<String> = FIXED_DOCS.iter().map(|s| s.to_string()).collect();
    docs.extend(
        entries
            .keys()
            .filter(|name| name.starts_with("OEBPS/Text/") && name.ends_with(".xhtml"))
            .cloned(),
    );

    for name in &docs {
        let bytes = entries
            .get(name)
            .unwrap_or_else(|| panic!("missing generated document {name}"));
        let text =
            String::from_utf8(bytes.clone()).unwrap_or_else(|_| panic!("{name} is not UTF-8"));
        let normalized = normalize(name, text);
        let path = golden_file(&golden_dir, name);

        if update {
            fs::create_dir_all(&golden_dir).unwrap();
            fs::write(&path, normalized.as_bytes()).unwrap();
            continue;
        }

        let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing golden {} — run UPDATE_GOLDEN=1 to capture it",
                path.display()
            )
        });
        assert_eq!(
            normalized, expected,
            "generated {name} differs from the golden reference"
        );
    }
}

/// `--hq` viewport halving, Kindle fixed-layout metas and bookmark navigation.
#[test]
fn kindle_hq_documents_match_golden() {
    check_scenario(&Scenario {
        name: "kindle_hq",
        args: &["-f", "epub", "-p", "K57", "--hq", "-a", "Jane Doe"],
        comicinfo: true,
    });
}

/// A page larger than the screen: the four-quadrant Panel View markup
/// (`--legacy-panel-view` enables Panel View without `--hq`, so the grid is not
/// suppressed).
#[test]
fn kindle_panel_documents_match_golden() {
    check_scenario(&Scenario {
        name: "kindle_panel",
        args: &[
            "-f",
            "epub",
            "-p",
            "K57",
            "--legacy-panel-view",
            "-a",
            "Jane Doe",
        ],
        comicinfo: false,
    });
}

/// KePub spread properties (`rendition:page-spread-*`) and right-to-left reading.
#[test]
fn kobo_documents_match_golden() {
    check_scenario(&Scenario {
        name: "kobo",
        args: &["-f", "epub", "-p", "KoE", "--no-kepub", "-m"],
        comicinfo: false,
    });
}
