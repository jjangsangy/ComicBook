//! EPUB input: spine-ordered images.
//!
//! KCC's `getWorkFolder` extracts the EPUB like any archive, then reads its
//! `META-INF/container.xml`, resolves the OPF, walks the spine and collects the
//! *largest* image each XHTML page references, copying them into a fresh flat
//! `Images/` tree in spine order (see docs/architecture.md and docs/porting.md).
//! This port does the same
//! in memory: the spine images become a flat [`ComicTree`] whose pages keep their
//! original bytes.
//!
//! Two KCC behaviours are preserved:
//!
//! - `--legacy-extract` and `--light-novel` skip the spine walk and treat the
//!   EPUB as a plain archive (KCC returns the raw extracted tree before the EPUB
//!   branch), so every image in the container is loaded in natural order.
//! - if the spine walk finds no image at all, the plain-archive fallback is used
//!   too (KCC's `if not ordered_image_paths: return workdir`).

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::archive::{open_reader, ArchiveKind, EntryContent};
use crate::ebook::model::ComicTree;
use crate::ebook::options::{Layout, Options};
use crate::path_text;

use super::archive::{build_tree, load_page, LoadedPage, RootStrip};

/// The chosen image of each spine page, named `"<i><ext>"`, in spine order.
///
/// Shared by `Arc` so an image referenced by several spine pages is copied once.
type OrderedImages = Vec<(String, Arc<[u8]>)>;

/// Load an EPUB source into a [`ComicTree`] in spine order.
pub fn load(source: &Path, options: &Options) -> Result<ComicTree> {
    // KCC bails out to the plain extracted tree before the spine walk for these
    // modes, so the container's images load in natural order instead.
    if options.processing.source.legacy_extract || options.main.layout == Layout::LightNovel {
        return super::archive::load(source, ArchiveKind::Cbz);
    }

    let files = read_container(source)?;
    match spine_images(&files)? {
        Some(images) => Ok(build_tree(
            images
                .into_iter()
                .map(|(name, data)| load_page(&name, &data))
                .collect::<Result<Vec<LoadedPage>>>()?,
            None,
            RootStrip::Keep,
        )),
        // KCC falls back to the raw extracted tree when the spine walk fails.
        None => super::archive::load(source, ArchiveKind::Cbz),
    }
}

/// Read every file entry of the EPUB container into memory.
fn read_container(source: &Path) -> Result<HashMap<String, Arc<[u8]>>> {
    let mut reader = open_reader(ArchiveKind::Cbz, source)?;
    let mut scratch = Vec::new();
    let mut files = HashMap::new();
    reader.read_entries(&mut scratch, &mut |name, content| {
        if let EntryContent::File(data) = content {
            // One copy out of the reader's scratch buffer; the spine walk then
            // shares the chosen entries by `Arc` instead of copying them again.
            files.insert(name.as_str().to_string(), Arc::from(data));
        }
        Ok(())
    })?;
    Ok(files)
}

/// Walk the EPUB's OPF spine and return the chosen image of each page, named
/// `"<i><ext>"` in spine order (KCC's `f"{i}{ext}"`), or `None` when no page
/// yields an image.
fn spine_images(files: &HashMap<String, Arc<[u8]>>) -> Result<Option<OrderedImages>> {
    let container = files
        .get("META-INF/container.xml")
        .context("EPUB container is missing META-INF/container.xml")?;
    let rootfile = scan_elements(container)
        .into_iter()
        .find(|el| el.local == "rootfile")
        .and_then(|el| attr(&el, "full-path"))
        .context("EPUB container.xml does not name a rootfile")?;
    let opf_path = normalize(&rootfile);
    let opf_dir = dir_of(&opf_path);

    let opf = files
        .get(&opf_path)
        .with_context(|| format!("EPUB OPF '{opf_path}' is missing"))?;
    let elements = scan_elements(opf);

    // Manifest entries for the XHTML pages, by id.
    let mut xhtml_by_id: HashMap<String, String> = HashMap::new();
    for el in elements.iter().filter(|el| el.local == "item") {
        if attr(el, "media-type").as_deref() == Some("application/xhtml+xml") {
            if let (Some(id), Some(href)) = (attr(el, "id"), attr(el, "href")) {
                xhtml_by_id.insert(id, href);
            }
        }
    }
    let spine: Vec<String> = elements
        .iter()
        .filter(|el| el.local == "itemref")
        .filter_map(|el| attr(el, "idref"))
        .collect();

    let mut ordered: OrderedImages = Vec::new();
    for idref in &spine {
        let Some(href) = xhtml_by_id.get(idref) else {
            continue;
        };
        let page_path = resolve_relative(&opf_dir, href);
        let Some(page) = files.get(&page_path) else {
            continue;
        };
        let page_dir = dir_of(&page_path);

        // KCC keeps the largest referenced image per page.
        let mut largest = 0usize;
        let mut chosen: Option<String> = None;
        for el in scan_elements(page) {
            if el.local != "img" && el.local != "image" {
                continue;
            }
            for (key, value) in &el.attrs {
                if !key.contains("src") && !key.contains("href") {
                    continue;
                }
                let candidate = resolve_relative(&page_dir, value);
                if let Some(data) = files.get(&candidate) {
                    if data.len() > largest {
                        largest = data.len();
                        chosen = Some(candidate);
                    }
                }
            }
        }

        if let Some(image_path) = chosen {
            // KCC copies whatever the page references and only later drops
            // non-image extensions (`removeNonImages`); mirror that so an
            // unsupported reference leaves the page without an image.
            if super::archive::is_ebook_image(&image_path) {
                if let Some(data) = files.get(&image_path) {
                    let ext = extension_of(&image_path);
                    ordered.push((format!("{}{}", ordered.len(), ext), Arc::clone(data)));
                }
            }
        }
    }

    if ordered.is_empty() {
        return Ok(None);
    }
    Ok(Some(ordered))
}

/// One XML start tag: its local name and its attributes' local names/values.
struct XmlElement {
    local: String,
    attrs: Vec<(String, String)>,
}

/// The value of attribute `name` (compared by local name), if present.
fn attr(element: &XmlElement, name: &str) -> Option<String> {
    element
        .attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// Collect every start/empty tag in an XML document, namespace-insensitively.
///
/// A lenient scan (malformed XML just ends the scan) is enough here: KCC reads
/// these documents with `ElementTree` and matches elements by local name at any
/// depth (`{*}`), which this reproduces for the handful of tags the spine walk
/// cares about.
fn scan_elements(xml: &[u8]) -> Vec<XmlElement> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(xml);
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(start)) | Ok(Event::Empty(start)) => {
                let local = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                let mut attrs = Vec::new();
                for attribute in start.attributes().flatten() {
                    let key =
                        String::from_utf8_lossy(attribute.key.local_name().as_ref()).into_owned();
                    let value = attribute
                        .unescape_value()
                        .map(|value| value.into_owned())
                        .unwrap_or_default();
                    attrs.push((key, value));
                }
                out.push(XmlElement { local, attrs });
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// Normalize a container path: forward slashes, no leading `./` or `/`.
fn normalize(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

/// The directory part of a container path (`""` for a root-level file).
fn dir_of(path: &str) -> String {
    path_text::directory(path).to_string()
}

/// The extension of a path, including the leading dot (`""` when none).
fn extension_of(path: &str) -> String {
    match path_text::extension(path) {
        Some(ext) => format!(".{ext}"),
        None => String::new(),
    }
}

/// Resolve `relative` against `base_dir`, applying `.`/`..` the way
/// `os.path.join` + filesystem access would.
fn resolve_relative(base_dir: &str, relative: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    if !relative.starts_with('/') && !base_dir.is_empty() {
        segments.extend(base_dir.split('/').filter(|part| !part.is_empty()));
    }
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_resolution_applies_parent_segments() {
        assert_eq!(
            resolve_relative("OEBPS/Text", "../Images/cover.jpg"),
            "OEBPS/Images/cover.jpg"
        );
        assert_eq!(
            resolve_relative("OEBPS", "Text/page.xhtml"),
            "OEBPS/Text/page.xhtml"
        );
        assert_eq!(resolve_relative("", "Images/a.png"), "Images/a.png");
        assert_eq!(resolve_relative("OEBPS/Text", "a.png"), "OEBPS/Text/a.png");
    }

    #[test]
    fn element_scan_is_namespace_insensitive() -> Result<()> {
        let xml = br#"<package xmlns="http://x"><manifest><item id="a" href="p.xhtml" media-type="application/xhtml+xml"/></manifest></package>"#;
        let elements = scan_elements(xml);
        let item = elements
            .iter()
            .find(|el| el.local == "item")
            .context("manifest item present")?;
        assert_eq!(attr(item, "id").as_deref(), Some("a"));
        assert_eq!(
            attr(item, "media-type").as_deref(),
            Some("application/xhtml+xml")
        );
        Ok(())
    }

    #[test]
    fn extension_of_includes_the_dot() {
        assert_eq!(extension_of("OEBPS/Images/a.JPG"), ".JPG");
        assert_eq!(extension_of("a"), "");
        assert_eq!(extension_of(".hidden"), "");
    }
}
