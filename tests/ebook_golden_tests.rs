//! Snapshot tests for the generated EPUB documents (see docs/architecture.md and
//! docs/development.md).
//!
//! The OPF/NCX/NAV/XHTML layout is device-sensitive and must be reproduced
//! exactly, so the documents are pinned with [`insta`]. The only volatile fields
//! are the `dc:identifier`/`dtb:uid` UUID and the `dcterms:modified` timestamp;
//! `insta` filters replace both, so the committed snapshots are stable.
//!
//! `insta` folds CRLF to LF and trims one trailing newline before comparing
//! (<https://insta.rs/docs/snapshot-files/>), so a raw document would silently
//! stop catching two properties this suite pins (docs/output.md): the generated
//! documents are LF-only, and `nav.xhtml` ends without a newline. [`snapshot_payload`]
//! asserts the LF-only contract and records the exact tail, keeping both visible
//! as ordinary snapshot diffs.
//!
//! Regenerate the snapshots (only for an intentional format change):
//!
//! ```text
//! INSTA_UPDATE=always cargo nextest run --test ebook_golden_tests
//! ```
//!
//! `cargo insta review` (from `cargo install cargo-insta`) walks pending changes
//! interactively; `cargo insta test` runs the suite (through nextest, per
//! `.config/insta.yaml`) and collects them, and `cargo insta test --review` chains
//! the two.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;
use comic_book::cli::Cli;
use comic_book::ebook::options::Options;
use comic_book::ebook::{convert_source, progress};
use image::{DynamicImage, Rgb, RgbImage};
use tempfile::tempdir;

/// `insta` filters replace the two volatile fields in every `content.opf`/`toc.ncx`
/// so the committed snapshots are byte-stable.
///
/// The UUID filter matches `urn:uuid:<uuid>` up to the first non-UUID character,
/// mirroring the previous hand-rolled replacement. The timestamp filter rewrites
/// the text content of the `dcterms:modified` meta element (non-greedy, and
/// `.`-matches-newline so it survives any wrapping).
const FILTERS: &[(&str, &str)] = &[
    (r"urn:uuid:[0-9A-Fa-f-]+", "urn:uuid:UUID"),
    (
        r#"(?s)<meta property="dcterms:modified">.*?</meta>"#,
        r#"<meta property="dcterms:modified">TIMESTAMP</meta>"#,
    ),
];

/// Markers that record whether a document ended with a newline; see [`snapshot_payload`].
const EOF_NEWLINE: &str = "<EOF: newline>";
const EOF_NO_NEWLINE: &str = "<EOF: no newline>";

/// Resolve options from a `comic-book ebook` command line.
fn options(args: &[&str]) -> Result<Options> {
    let mut full = vec!["comic-book", "ebook", "book"];
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

/// A folder with a root page, a rotated spread and two chapter directories.
///
/// The first page is larger than the Kindle screen so Panel View has real
/// quadrants to lay out (see the `kindle_panel` scenario).
fn fixture(root: &Path, comicinfo: bool) -> Result<()> {
    write_png(&root.join("01-normal.png"), 800, 1200, [10, 10, 10])?;
    // 2.5:1 exceeds the bisect threshold, so it rotates to `-cb-d` (`center` spread).
    write_png(&root.join("02-spread.png"), 500, 200, [255, 255, 255])?;
    write_png(&root.join("Chapter 1/01.png"), 100, 150, [10, 10, 10])?;
    write_png(&root.join("Chapter 1/02.png"), 100, 150, [200, 200, 200])?;
    write_png(&root.join("Chapter 2/01.png"), 100, 150, [10, 10, 10])?;
    if comicinfo {
        fs::write(
            root.join("ComicInfo.xml"),
            br#"<ComicInfo><Series>Berserk</Series><Volume>3</Volume><Number>7</Number><Summary>A &amp; B</Summary><Page Image="1" Bookmark="The Black Swordsman"/></ComicInfo>"#,
        )?;
    }
    Ok(())
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
fn run(scenario: &Scenario) -> Result<BTreeMap<String, Vec<u8>>> {
    std::env::set_var(progress::QUIET_ENV, "1");
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    fixture(&source, scenario.comicinfo)?;
    let out_dir = tmp.path().join("out");

    let mut argv: Vec<&str> = vec!["-o", out_dir.to_str().context("utf8 path")?];
    argv.extend_from_slice(scenario.args);
    let options = options(&argv)?;
    let outputs = convert_source(&source, &options)?;
    assert_eq!(outputs.len(), 1, "expected a single output file");

    read_entries(&outputs[0])
}

/// Read all entries of an EPUB into a map.
fn read_entries(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let file = fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        entries.insert(name, data);
    }
    Ok(entries)
}

/// The snapshot payload for one generated document.
///
/// `insta` normalises both the line endings and one trailing newline, so a raw
/// document would hide the two properties this suite pins. This asserts the
/// LF-only contract directly and appends a marker recording the exact tail, so a
/// CR or a missing/added final newline stays an ordinary, visible diff.
fn snapshot_payload(name: &str, text: &str) -> String {
    assert!(!text.contains('\r'), "{name} is not LF-only");
    assert!(
        !text.contains("<EOF:"),
        "{name} collides with the EOF marker"
    );
    let mut payload = text.to_string();
    payload.push_str(if text.ends_with('\n') {
        EOF_NEWLINE
    } else {
        EOF_NO_NEWLINE
    });
    payload
}

fn check_scenario(scenario: &Scenario) -> Result<()> {
    let entries = run(scenario)?;

    // The fixed documents plus every generated page XHTML, in archive order.
    let mut docs: Vec<String> = FIXED_DOCS.iter().map(|s| s.to_string()).collect();
    docs.extend(
        entries
            .keys()
            .filter(|name| name.starts_with("OEBPS/Text/") && name.ends_with(".xhtml"))
            .cloned(),
    );

    // Resolve every document to its snapshot payload before asserting: the
    // `with_settings!` block is a closure, so `?` cannot be used inside it.
    let mut snapshots: Vec<(String, String)> = Vec::new();
    for name in &docs {
        let bytes = entries
            .get(name)
            .with_context(|| format!("missing generated document {name}"))?;
        let text =
            String::from_utf8(bytes.clone()).with_context(|| format!("{name} is not UTF-8"))?;
        // The suffix selects the snapshot file: the scenario name (the snapshot
        // name itself defaults to the enclosing `check_scenario` function, so
        // without it the scenarios would collide) plus the archive path, with
        // `/` flattened because it is not filename-safe.
        let suffix = format!("{}__{}", scenario.name, name.replace('/', "__"));
        snapshots.push((suffix, snapshot_payload(name, &text)));
    }

    insta::with_settings!({ filters => FILTERS.to_vec() }, {
        for (suffix, payload) in snapshots {
            insta::with_settings!({ snapshot_suffix => suffix }, {
                insta::assert_snapshot!(payload);
            });
        }
    });

    Ok(())
}

/// `--hq` viewport halving, Kindle fixed-layout metas and bookmark navigation.
#[test]
fn kindle_hq_documents_match_snapshot() -> Result<()> {
    check_scenario(&Scenario {
        name: "kindle_hq",
        args: &["-f", "epub", "-p", "K57", "--hq", "-a", "Jane Doe"],
        comicinfo: true,
    })
}

/// A page larger than the screen: the four-quadrant Panel View markup
/// (`--legacy-panel-view` enables Panel View without `--hq`, so the grid is not
/// suppressed).
#[test]
fn kindle_panel_documents_match_snapshot() -> Result<()> {
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
    })
}

/// KePub spread properties (`rendition:page-spread-*`) and right-to-left reading.
#[test]
fn kobo_documents_match_snapshot() -> Result<()> {
    check_scenario(&Scenario {
        name: "kobo",
        args: &["-f", "epub", "-p", "KoE", "--no-kepub", "-m"],
        comicinfo: false,
    })
}

/// `insta` folds CRLF to LF before comparing, so [`snapshot_payload`]'s refusal
/// to accept a `\r` is the only thing that keeps a CRLF regression catchable
/// (docs/output.md). Pin that guard so it is not silently dropped.
#[test]
#[should_panic(expected = "is not LF-only")]
fn snapshot_payload_rejects_carriage_returns() {
    let _ = snapshot_payload("content.opf", "a\r\nb\n");
}
