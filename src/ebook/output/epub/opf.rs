//! `content.opf` generation and the spread-property algorithm (AGENTS.md §12.2).
//!
//! The OPF is the device-sensitive heart of the EPUB: the Dublin Core block, the
//! Kindle fixed-layout metas, the manifest and — most delicately — the spine's
//! `page-spread-*` properties. That algorithm is ported verbatim: a forward pass
//! that alternates left/right (honouring the `-kcc-a`…`-kcc-d` spread specials)
//! followed by a backward fix-up pass that anchors the tail of the book, with
//! `--spread-shift`, `--one-page-landscape` and the PDF/EPUB source flip applied
//! as the reference does.

use std::path::Path;

use super::{html_escape, images_dir, text_dir, unique_id, PageRef};
use crate::ebook::metadata::BookMetadata;
use crate::ebook::options::Options;

/// KCC's `KindleComicConverter-<version>` contributor string.
const CONTRIBUTOR: &str = "KindleComicConverter-11.3.2";

/// Build `OEBPS/content.opf`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_opf(
    title: &str,
    filelist: &[PageRef<'_>],
    has_cover: bool,
    source: &Path,
    metadata: &BookMetadata,
    language: &str,
    uuid: &str,
    modified: &str,
    options: &Options,
) -> String {
    let device = (options.profile_data.width, options.profile_data.height);

    // `--vertical-4-panel` writes top-to-bottom; `--invert-direction` swaps the
    // two suffixes relative to the normal rule.
    let mut writing_mode = if options.vertical_4_panel {
        "vertical"
    } else {
        "horizontal"
    }
    .to_string();
    writing_mode.push_str(match (options.invert_direction, options.right_to_left) {
        (true, true) | (false, false) => "-lr",
        _ => "-rl",
    });

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<package version=\"3.0\" unique-identifier=\"BookID\" xmlns=\"http://www.idpf.org/2007/opf\">\n",
    );
    out.push_str(
        "<metadata xmlns:opf=\"http://www.idpf.org/2007/opf\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n",
    );
    out.push_str(&format!("<dc:title>{}</dc:title>\n", html_escape(title)));
    out.push_str(&format!("<dc:language>{language}</dc:language>\n"));
    out.push_str(&format!(
        "<dc:identifier id=\"BookID\">urn:uuid:{uuid}</dc:identifier>\n"
    ));
    out.push_str(&format!(
        "<dc:contributor id=\"contributor\">{CONTRIBUTOR}</dc:contributor>\n"
    ));
    if !metadata.summary.is_empty() {
        out.push_str(&format!(
            "<dc:description>{}</dc:description>\n",
            html_escape(&metadata.summary)
        ));
    }
    for author in &metadata.authors {
        out.push_str(&format!(
            "<dc:creator>{}</dc:creator>\n",
            html_escape(author)
        ));
    }

    // Series metadata is only meaningful for non-Kindle readers.
    if !options.is_kindle && !metadata.series.is_empty() {
        out.push_str(&format!(
            "<meta property=\"belongs-to-collection\" id=\"c02\">{}</meta>\n",
            html_escape(&metadata.series)
        ));
        out.push_str("<meta refines=\"#c02\" property=\"collection-type\">series</meta>\n");
        let group = if !metadata.volume.is_empty() && !metadata.number.is_empty() {
            Some(format!("{}.{}", metadata.volume, metadata.number))
        } else if !metadata.volume.is_empty() {
            Some(metadata.volume.clone())
        } else if !metadata.number.is_empty() {
            Some(metadata.number.clone())
        } else {
            None
        };
        if let Some(group) = group {
            out.push_str(&format!(
                "<meta refines=\"#c02\" property=\"group-position\">{}</meta>\n",
                html_escape(&group)
            ));
        }
    }

    out.push_str(&format!(
        "<meta property=\"dcterms:modified\">{modified}</meta>\n"
    ));
    if has_cover {
        out.push_str("<meta name=\"cover\" content=\"cover\"/>\n");
    }

    if options.is_kindle && !options.custom_profile {
        out.push_str("<meta name=\"fixed-layout\" content=\"true\"/>\n");
        out.push_str(&format!(
            "<meta name=\"original-resolution\" content=\"{}x{}\"/>\n",
            device.0, device.1
        ));
        out.push_str("<meta name=\"book-type\" content=\"comic\"/>\n");
        out.push_str(&format!(
            "<meta name=\"primary-writing-mode\" content=\"{writing_mode}\"/>\n"
        ));
        out.push_str("<meta name=\"zero-gutter\" content=\"true\"/>\n");
        out.push_str("<meta name=\"zero-margin\" content=\"true\"/>\n");
        out.push_str("<meta name=\"ke-border-color\" content=\"#FFFFFF\"/>\n");
        out.push_str("<meta name=\"ke-border-width\" content=\"0\"/>\n");
        out.push_str("<meta name=\"orientation-lock\" content=\"none\"/>\n");
        out.push_str(&format!(
            "<meta name=\"region-mag\" content=\"{}\"/>\n",
            if options.kfx { "false" } else { "true" }
        ));
    }

    out.push_str("<meta property=\"rendition:spread\">landscape</meta>\n");
    out.push_str("<meta property=\"rendition:layout\">pre-paginated</meta>\n");
    out.push_str("</metadata>\n<manifest>\n");
    out.push_str("<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");
    out.push_str(
        "<item id=\"nav\" href=\"nav.xhtml\" properties=\"nav\" media-type=\"application/xhtml+xml\"/>\n",
    );
    if has_cover {
        out.push_str(
            "<item id=\"cover\" href=\"Images/cover.jpg\" media-type=\"image/jpeg\" properties=\"cover-image\"/>\n",
        );
    }

    for entry in filelist {
        let id = unique_id(entry);
        out.push_str(&format!(
            "<item id=\"page_{id}\" href=\"{}/{}.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
            text_dir(entry.image_dir),
            entry.stem
        ));
        out.push_str(&format!(
            "<item id=\"img_{id}\" href=\"{}/{}\" media-type=\"{}\"/>\n",
            images_dir(entry.image_dir),
            entry.file,
            entry.media_type.mime()
        ));
        // A Scribe `-above` page has a matching `-below` image in the same chapter.
        if entry.file.contains("above") {
            let below_id = id.replace("above", "below");
            out.push_str(&format!(
                "<item id=\"img_{below_id}\" href=\"{}/{}\" media-type=\"{}\"/>\n",
                images_dir(entry.image_dir),
                entry.file.replace("above", "below"),
                entry.media_type.mime()
            ));
        }
    }
    out.push_str("<item id=\"css\" href=\"Text/style.css\" media-type=\"text/css\"/>\n");
    out.push_str("</manifest>\n");

    let reflist: Vec<String> = filelist.iter().map(unique_id).collect();
    let (direction, initial_side) = match (options.invert_direction, options.right_to_left) {
        (true, false) | (false, true) => ("rtl", "right"),
        _ => ("ltr", "left"),
    };
    let initial_side = flip_for_source(initial_side, source, options);
    let spread = spread_properties(&reflist, options.right_to_left, initial_side);

    out.push_str(&format!(
        "<spine page-progression-direction=\"{direction}\" toc=\"ncx\">\n"
    ));
    for (entry, property) in reflist.iter().zip(&spread) {
        let property = if options.one_page_landscape {
            "center"
        } else {
            property
        };
        out.push_str(&format!(
            "<itemref idref=\"page_{entry}\" {}/>\n",
            page_spread_property(property, options)
        ));
    }
    out.push_str("</spine>\n</package>\n");
    out
}

/// The `page-spread-*` property for each spine item (KCC's two-pass algorithm).
///
/// The forward pass alternates sides, letting the `-kcc-a`/`-kcc-d` spread
/// specials reset direction and `-kcc-b`/`-kcc-c` pin a split half. The backward
/// pass then walks from the end, anchoring the tail so the last pages line up
/// with the book's opening side.
fn spread_properties(
    reflist: &[String],
    right_to_left: bool,
    mut pageside: &'static str,
) -> Vec<&'static str> {
    let mut sides: Vec<&'static str> = Vec::with_capacity(reflist.len());

    for entry in reflist {
        let center = entry.contains("-kcc-a") || entry.contains("-kcc-d");
        if right_to_left {
            if center {
                sides.push("center");
                pageside = "right";
            } else if entry.contains("-kcc-b") {
                sides.push("right");
                pageside = "right";
            } else if entry.contains("-kcc-c") {
                sides.push("left");
                pageside = "right";
            } else {
                sides.push(pageside);
                pageside = other(pageside);
            }
        } else if center {
            sides.push("center");
            pageside = "left";
        } else if entry.contains("-kcc-b") {
            sides.push("left");
            pageside = "left";
        } else if entry.contains("-kcc-c") {
            sides.push("right");
            pageside = "left";
        } else {
            sides.push(pageside);
            pageside = other(pageside);
        }
    }

    // Backward fix-up: every page from the first spread special onward keeps the
    // side set by that special; earlier `-kcc-x` pages are re-alternated.
    let mut spread_seen = false;
    for index in (0..reflist.len()).rev() {
        let entry = &reflist[index];
        if !entry.contains("-kcc-x") {
            spread_seen = true;
            pageside = if right_to_left { "left" } else { "right" };
        } else if spread_seen {
            sides[index] = pageside;
            pageside = other(pageside);
        }
    }

    sides
}

/// The opposite page side.
fn other(side: &'static str) -> &'static str {
    if side == "right" {
        "left"
    } else {
        "right"
    }
}

/// Flip the opening side for a PDF/EPUB source, then for `--spread-shift`, exactly
/// as `buildOPF` does before its forward pass.
fn flip_for_source(mut side: &'static str, source: &Path, options: &Options) -> &'static str {
    let name = source.to_string_lossy().to_lowercase();
    if name.ends_with(".pdf") || name.ends_with(".epub") {
        side = other(side);
    }
    if options.spread_shift {
        side = other(side);
    }
    side
}

/// KCC's `pageSpreadProperty`: a different attribute spelling per reader family.
fn page_spread_property(property: &str, options: &Options) -> String {
    if options.is_kindle {
        format!("linear=\"yes\" properties=\"page-spread-{property}\"")
    } else if options.is_kobo {
        format!("properties=\"rendition:page-spread-{property}\"")
    } else {
        String::new()
    }
}

/// `META-INF/container.xml` for the OEBPS layout.
pub(crate) fn container_xml() -> String {
    [
        "<?xml version=\"1.0\"?>\n",
        "<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n",
        "<rootfiles>\n",
        "<rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n",
        "</rootfiles>\n",
        "</container>",
    ]
    .concat()
}

/// The shared `style.css`.
pub(crate) fn style_css(options: &Options) -> String {
    let mut css = String::from(
        "@page {\nmargin: 0;\n}\nbody {\ndisplay: block;\nmargin: 0;\npadding: 0;\n}\n",
    );
    if options.kindle_scribe_azw3 {
        css.push_str("img {\ndisplay: block;\n}\n");
    }
    if options.is_kindle && options.panel_view {
        css.push_str(
            "#PV {\nposition: absolute;\nwidth: 100%;\nheight: 100%;\ntop: 0;\nleft: 0;\n}\n\
             #PV-T {\ntop: 0;\nwidth: 100%;\nheight: 50%;\n}\n\
             #PV-B {\nbottom: 0;\nwidth: 100%;\nheight: 50%;\n}\n\
             #PV-L {\nleft: 0;\nwidth: 49.5%;\nheight: 100%;\nfloat: left;\n}\n\
             #PV-R {\nright: 0;\nwidth: 49.5%;\nheight: 100%;\nfloat: right;\n}\n\
             #PV-TL {\ntop: 0;\nleft: 0;\nwidth: 49.5%;\nheight: 50%;\nfloat: left;\n}\n\
             #PV-TR {\ntop: 0;\nright: 0;\nwidth: 49.5%;\nheight: 50%;\nfloat: right;\n}\n\
             #PV-BL {\nbottom: 0;\nleft: 0;\nwidth: 49.5%;\nheight: 50%;\nfloat: left;\n}\n\
             #PV-BR {\nbottom: 0;\nright: 0;\nwidth: 49.5%;\nheight: 50%;\nfloat: right;\n}\n\
             .PV-P {\nwidth: 100%;\nheight: 100%;\ntop: 0;\nposition: absolute;\ndisplay: none;\n}\n",
        );
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the spread algorithm over file stems.
    fn spread(entries: &[&str], right_to_left: bool, initial: &'static str) -> Vec<&'static str> {
        let reflist: Vec<String> = entries.iter().map(|entry| entry.to_string()).collect();
        spread_properties(&reflist, right_to_left, initial)
    }

    #[test]
    fn plain_pages_alternate_from_the_initial_side() {
        assert_eq!(
            spread(&["a-kcc-x", "b-kcc-x", "c-kcc-x"], false, "left"),
            vec!["left", "right", "left"]
        );
        assert_eq!(
            spread(&["a-kcc-x", "b-kcc-x", "c-kcc-x"], true, "right"),
            vec!["right", "left", "right"]
        );
    }

    #[test]
    fn split_halves_pin_their_sides() {
        // Left-to-right: `-kcc-b` is the left half, `-kcc-c` the right half.
        assert_eq!(
            spread(&["a-kcc-b", "b-kcc-c"], false, "left"),
            vec!["left", "right"]
        );
        // Right-to-left: the split halves keep their physical sides.
        assert_eq!(
            spread(&["a-kcc-b", "b-kcc-c"], true, "right"),
            vec!["right", "left"]
        );
    }

    #[test]
    fn a_rotated_spread_is_centred_and_re_anchors_the_pages_before_it() {
        // The backward fix-up pass re-anchors the page before a spread special so
        // the ends of the book meet.
        assert_eq!(
            spread(&["a-kcc-x", "b-kcc-d", "c-kcc-x"], false, "left"),
            vec!["right", "center", "left"]
        );
        assert_eq!(
            spread(&["a-kcc-x", "b-kcc-a", "c-kcc-x"], true, "right"),
            vec!["left", "center", "right"]
        );
    }

    #[test]
    fn shifting_the_initial_side_flips_every_plain_page() {
        assert_eq!(
            spread(&["a-kcc-x", "b-kcc-x"], false, "right"),
            vec!["right", "left"]
        );
    }

    #[test]
    fn html_escape_matches_python() {
        assert_eq!(
            html_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#x27;f"
        );
    }
}
