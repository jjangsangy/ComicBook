//! Metadata: `ComicInfo.xml` parsing and title/author/series resolution.
//!
//! This is the Rust counterpart of KCC's `metadata.MetadataParser` plus
//! `comic2ebook.getMetadata`: the raw `ComicInfo.xml` captured by the input
//! adapters ([`ComicTree::comicinfo`]) is parsed into [`ComicInfo`], and
//! [`resolve`] folds it together with the CLI overrides into the [`BookMetadata`]
//! the output builders consume (see docs/architecture.md).
//!
//! The XML is parsed with `quick-xml` (see docs/dependencies.md); nothing is written
//! back, because `--keep-comicinfo` retains the original document bytes verbatim.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::ebook::model::ComicTree;
use crate::ebook::options::{MetadataTitle, OutputEncoding, OutputOptions};

/// The ComicInfo.xml elements KCC reads (its `MetadataParser.data` keys).
const SINGLE_FIELDS: [&str; 9] = [
    "Series",
    "Volume",
    "Number",
    "Summary",
    "Title",
    "Writer",
    "Penciller",
    "Inker",
    "Colorist",
];

/// The metadata KCC's `MetadataParser` extracts from a `ComicInfo.xml`.
///
/// People fields (`writers`, …) are already de-duplicated and sorted, matching
/// KCC's `list(set(...)); .sort()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComicInfo {
    pub series: String,
    pub volume: String,
    pub number: String,
    pub writers: Vec<String>,
    pub pencillers: Vec<String>,
    pub inkers: Vec<String>,
    pub colorists: Vec<String>,
    pub summary: String,
    pub title: String,
    /// `<Page Bookmark="…" Image="…"/>` entries, as `(image index, bookmark)`.
    pub bookmarks: Vec<(usize, String)>,
}

impl ComicInfo {
    /// Parse a `ComicInfo.xml` document.
    ///
    /// Elements are matched by their local name at any depth, mirroring
    /// `getElementsByTagName`, and only the first occurrence of each field is
    /// used. A non-integer `Page/Image` attribute is an error, which
    /// [`resolve`] treats as "no usable ComicInfo" exactly as KCC discards the
    /// whole file when `int()` raises (see docs/porting.md).
    pub fn parse(xml: &[u8]) -> Result<ComicInfo> {
        let mut reader = Reader::from_reader(xml);
        let mut info = ComicInfo::default();
        let mut found: HashMap<&'static str, String> = HashMap::new();
        let mut capture: Option<&'static str> = None;
        let mut text = String::new();

        loop {
            match reader.read_event().context("Invalid ComicInfo.xml")? {
                Event::Start(event) => {
                    let qname = event.name();
                    let name = local_name(qname.as_ref());
                    if let Some(field) = single_field(name) {
                        if !found.contains_key(field) {
                            capture = Some(field);
                            text.clear();
                        }
                    } else if name == "Page" {
                        parse_page(&event, &mut info)?;
                    }
                }
                Event::Empty(event) => {
                    let qname = event.name();
                    let name = local_name(qname.as_ref());
                    if let Some(field) = single_field(name) {
                        found.entry(field).or_default();
                    } else if name == "Page" {
                        parse_page(&event, &mut info)?;
                    }
                }
                Event::Text(value) => {
                    if capture.is_some() {
                        text.push_str(&value.unescape().context("Invalid ComicInfo.xml text")?);
                    }
                }
                Event::End(event) => {
                    let qname = event.name();
                    if let Some(field) = capture {
                        if local_name(qname.as_ref()) == field {
                            found.insert(field, std::mem::take(&mut text));
                            capture = None;
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }

        info.series = found.remove("Series").unwrap_or_default();
        info.volume = found.remove("Volume").unwrap_or_default();
        info.number = found.remove("Number").unwrap_or_default();
        info.summary = found.remove("Summary").unwrap_or_default();
        info.title = found.remove("Title").unwrap_or_default();
        for (element, target) in [
            ("Writer", &mut info.writers),
            ("Penciller", &mut info.pencillers),
            ("Inker", &mut info.inkers),
            ("Colorist", &mut info.colorists),
        ] {
            *target = split_people(found.remove(element).unwrap_or_default());
        }

        Ok(info)
    }
}

/// The metadata KCC's `getMetadata` resolves for one book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookMetadata {
    pub title: String,
    pub authors: Vec<String>,
    pub series: String,
    pub volume: String,
    pub number: String,
    pub summary: String,
    /// Non-empty only when the ComicInfo had `<Page Bookmark=…/>` entries.
    pub bookmarks: Vec<(usize, String)>,
    /// The original `ComicInfo.xml` bytes, retained only for `--keep-comicinfo`
    /// on CBZ output (KCC's `options.comicinfo_xml`).
    pub comicinfo_xml: Option<Vec<u8>>,
}

/// Resolve a book's metadata from its tree, source path and CLI options.
///
/// This mirrors KCC's `getMetadata` (see docs/architecture.md): the default title
/// comes from
/// the source name, `--metadata-title` selects how the ComicInfo title is used,
/// the author falls back to the first listed people (or `KCC`), and the
/// series/volume/number/summary/bookmarks are lifted from the ComicInfo
/// regardless of the other flags.
pub fn resolve(tree: &ComicTree, source: &Path, output: &OutputOptions) -> BookMetadata {
    resolve_with(tree, source, output, None)
}

/// Resolve a book's metadata, overriding the title derived from `source`.
///
/// `--file-fusion` converts a synthetic `<name> [fused]` directory whose default
/// title cannot be derived from the path with the usual file/directory rules, so
/// the caller supplies it (see docs/porting.md).
pub fn resolve_with(
    tree: &ComicTree,
    source: &Path,
    output: &OutputOptions,
    default_title: Option<&str>,
) -> BookMetadata {
    // A malformed ComicInfo is ignored entirely, matching KCC's
    // `except Exception: os.remove(xmlPath); return`.
    let comicinfo = tree
        .comicinfo
        .as_deref()
        .and_then(|xml| ComicInfo::parse(xml).ok());

    let book_default_title = output.title.is_none();
    let default_author = output.author.is_none();

    let mut title = match &output.title {
        Some(title) => title.clone(),
        None => default_title
            .map(str::to_string)
            .unwrap_or_else(|| default_title_from(source)),
    };
    let mut authors = match &output.author {
        Some(author) => vec![author.clone()],
        None => vec!["KCC".to_string()],
    };
    let mut series = String::new();
    let mut volume = String::new();
    let mut number = String::new();
    let mut summary = String::new();
    let mut bookmarks = Vec::new();
    let mut comicinfo_xml = None;

    if let Some(info) = &comicinfo {
        // `Only` takes the embedded title verbatim; `Default`/`Combine` fold the
        // embedded series/volume/number into the default schema, but only when the
        // user did not pass an explicit title.
        match output.metadata_title {
            MetadataTitle::Only => title = info.title.clone(),
            mode @ (MetadataTitle::Default | MetadataTitle::Combine) => {
                if book_default_title {
                    if !info.series.is_empty() {
                        title = info.series.clone();
                    }
                    if !info.volume.is_empty() {
                        title.push_str(" Vol. ");
                        title.push_str(&zfill(&info.volume, 2));
                        volume = info.volume.clone();
                    }
                    if !info.number.is_empty() {
                        title.push_str(" #");
                        title.push_str(&zfill(&info.number, 3));
                        number = info.number.clone();
                    }
                    if matches!(mode, MetadataTitle::Combine) && !info.title.is_empty() {
                        title.push_str(": ");
                        title.push_str(&info.title);
                    }
                }
            }
        }

        if default_author {
            let mut people = BTreeSet::new();
            for person in info
                .writers
                .iter()
                .chain(&info.pencillers)
                .chain(&info.inkers)
                .chain(&info.colorists)
            {
                people.insert(person.clone());
            }
            authors = if people.is_empty() {
                vec!["KCC".to_string()]
            } else {
                people.into_iter().collect()
            };
        }

        if !info.bookmarks.is_empty() {
            bookmarks = info.bookmarks.clone();
        }
        if !info.summary.is_empty() {
            summary = info.summary.clone();
        }
        if !info.series.is_empty() {
            series = info.series.clone();
        }
        if output.keep_comicinfo && matches!(output.encoding, OutputEncoding::Cbz) {
            comicinfo_xml = tree.comicinfo.clone();
        }
    }

    BookMetadata {
        title,
        authors,
        series,
        volume,
        number,
        summary,
        bookmarks,
        comicinfo_xml,
    }
}

/// The default title for a source (KCC's `os.path.basename`/`splitext`).
fn default_title_from(source: &Path) -> String {
    if source.is_dir() {
        source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        source
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Read the `Bookmark`/`Image` attributes of a `<Page>` element.
fn parse_page(event: &BytesStart<'_>, info: &mut ComicInfo) -> Result<()> {
    let mut image = None;
    let mut bookmark = None;
    for attribute in event.attributes() {
        let attribute = attribute.context("Invalid ComicInfo.xml attribute")?;
        let value = attribute
            .unescape_value()
            .context("Invalid ComicInfo.xml attribute value")?;
        match local_name(attribute.key.as_ref()) {
            "Image" => {
                image = Some(
                    value
                        .parse::<usize>()
                        .with_context(|| format!("Page Image '{}' is not a number", value))?,
                );
            }
            "Bookmark" => bookmark = Some(value.into_owned()),
            _ => {}
        }
    }
    if let (Some(image), Some(bookmark)) = (image, bookmark) {
        info.bookmarks.push((image, bookmark));
    }
    Ok(())
}

/// Split KCC's `', '`-separated people list, de-duplicate and sort.
fn split_people(value: String) -> Vec<String> {
    let mut people: BTreeSet<String> = value.split(", ").map(|person| person.to_string()).collect();
    people.remove("");
    people.into_iter().collect()
}

/// The element's local name (the part after any namespace prefix).
fn local_name(name: &[u8]) -> &str {
    let name = match name.iter().rposition(|byte| *byte == b':') {
        Some(index) => &name[index + 1..],
        None => name,
    };
    std::str::from_utf8(name).unwrap_or("")
}

/// The static field name matching `name`, if any.
fn single_field(name: &str) -> Option<&'static str> {
    SINGLE_FIELDS.iter().copied().find(|field| *field == name)
}

/// Python's `str.zfill`: pad with leading zeros, keeping a leading sign.
fn zfill(value: &str, width: usize) -> String {
    let (sign, digits) = match value.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => match value.strip_prefix('+') {
            Some(rest) => ("+", rest),
            None => ("", value),
        },
    };
    let mut out = String::with_capacity(value.len().max(width));
    out.push_str(sign);
    for _ in (sign.len() + digits.len())..width {
        out.push('0');
    }
    out.push_str(digits);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use clap::Parser;

    use crate::cli::{Cli, Commands};
    use crate::ebook::options::Options;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Berserk</Series>
  <Volume>3</Volume>
  <Number>7</Number>
  <Title>The Golden Age</Title>
  <Summary>A summary &amp; more</Summary>
  <Writer>Kentaro Miura, Someone Else</Writer>
  <Penciller>Kentaro Miura</Penciller>
  <Page Image="0" Bookmark="Chapter 1"/>
  <Page Image="5" Bookmark="Chapter 2"/>
</ComicInfo>"#;

    fn options(args: &[&str]) -> Result<Options> {
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = Cli::try_parse_from(full)?;
        match cli.command {
            Commands::Ebook(args) => Options::resolve(&args),
            _ => bail!("expected the ebook subcommand"),
        }
    }

    /// Resolve from the whole run, for test convenience.
    fn resolve(tree: &ComicTree, source: &Path, options: &Options) -> BookMetadata {
        super::resolve(tree, source, &options.output)
    }

    fn tree(xml: &str) -> ComicTree {
        let mut tree = ComicTree::new();
        tree.comicinfo = Some(xml.as_bytes().to_vec());
        tree
    }

    #[test]
    fn parses_every_field() -> Result<()> {
        let info = ComicInfo::parse(SAMPLE.as_bytes())?;
        assert_eq!(info.series, "Berserk");
        assert_eq!(info.volume, "3");
        assert_eq!(info.number, "7");
        assert_eq!(info.title, "The Golden Age");
        assert_eq!(info.summary, "A summary & more");
        assert_eq!(info.writers, vec!["Kentaro Miura", "Someone Else"]);
        assert_eq!(info.pencillers, vec!["Kentaro Miura"]);
        assert_eq!(
            info.bookmarks,
            vec![(0, "Chapter 1".to_string()), (5, "Chapter 2".to_string())]
        );
        Ok(())
    }

    #[test]
    fn people_are_deduplicated_and_sorted() -> Result<()> {
        let xml = "<ComicInfo><Writer>B, A, B</Writer></ComicInfo>";
        let info = ComicInfo::parse(xml.as_bytes())?;
        assert_eq!(info.writers, vec!["A", "B"]);
        Ok(())
    }

    #[test]
    fn self_closing_fields_are_treated_as_empty() -> Result<()> {
        let info = ComicInfo::parse(b"<ComicInfo><Series/></ComicInfo>")?;
        assert_eq!(info.series, "");
        Ok(())
    }

    #[test]
    fn a_non_numeric_page_image_is_an_error() {
        let xml = r#"<ComicInfo><Page Image="x" Bookmark="c"/></ComicInfo>"#;
        assert!(ComicInfo::parse(xml.as_bytes()).is_err());
    }

    #[test]
    fn malformed_xml_is_ignored_by_resolve() -> Result<()> {
        let options = options(&[])?;
        let broken = r#"<ComicInfo><Page Image="x" Bookmark="c"/></ComicInfo>"#;
        let metadata = resolve(&tree(broken), Path::new("/tmp/Book.cbz"), &options);
        assert_eq!(metadata.title, "Book");
        assert_eq!(metadata.authors, vec!["KCC"]);
        assert!(metadata.bookmarks.is_empty());
        Ok(())
    }

    #[test]
    fn default_title_combines_series_volume_and_number() -> Result<()> {
        let options = options(&[])?;
        let metadata = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &options);
        assert_eq!(metadata.title, "Berserk Vol. 03 #007");
        assert_eq!(metadata.volume, "3");
        assert_eq!(metadata.number, "7");
        assert_eq!(metadata.series, "Berserk");
        assert_eq!(metadata.summary, "A summary & more");
        assert_eq!(metadata.bookmarks.len(), 2);
        Ok(())
    }

    #[test]
    fn metadata_title_one_appends_the_title() -> Result<()> {
        let options = options(&["--metadata-title", "1"])?;
        let metadata = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &options);
        assert_eq!(metadata.title, "Berserk Vol. 03 #007: The Golden Age");
        Ok(())
    }

    #[test]
    fn metadata_title_two_uses_the_title_only() -> Result<()> {
        let options = options(&["--metadata-title", "2"])?;
        let metadata = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &options);
        assert_eq!(metadata.title, "The Golden Age");
        Ok(())
    }

    #[test]
    fn explicit_title_is_not_overridden() -> Result<()> {
        let options = options(&["-t", "Custom"])?;
        let metadata = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &options);
        assert_eq!(metadata.title, "Custom");
        assert_eq!(metadata.series, "Berserk", "series is still lifted");
        Ok(())
    }

    #[test]
    fn comicinfo_authors_replace_the_default_author() -> Result<()> {
        let defaults = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &options(&[])?);
        assert_eq!(defaults.authors, vec!["Kentaro Miura", "Someone Else"]);

        let explicit = resolve(
            &tree(SAMPLE),
            Path::new("/tmp/Book.cbz"),
            &options(&["-a", "Me"])?,
        );
        assert_eq!(explicit.authors, vec!["Me"]);
        Ok(())
    }

    #[test]
    fn keep_comicinfo_retains_the_document_for_cbz() -> Result<()> {
        let cbz = options(&["-p", "KDX", "--keep-comicinfo"])?;
        assert!(matches!(cbz.output.encoding, OutputEncoding::Cbz));
        let kept = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &cbz);
        assert_eq!(kept.comicinfo_xml.as_deref(), Some(SAMPLE.as_bytes()));

        // Without the flag, the document is not retained.
        let without = resolve(&tree(SAMPLE), Path::new("/tmp/Book.cbz"), &options(&[])?);
        assert_eq!(without.comicinfo_xml, None);
        Ok(())
    }

    #[test]
    fn zfill_matches_python() {
        assert_eq!(zfill("5", 2), "05");
        assert_eq!(zfill("12", 2), "12");
        assert_eq!(zfill("123", 3), "123");
        assert_eq!(zfill("-5", 3), "-05");
    }
}
