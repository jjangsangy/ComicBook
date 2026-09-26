//! `content.opf` generation and the spread-property algorithm (AGENTS.md §12.2).
//!
//! The OPF is the device-sensitive heart of the EPUB: the Dublin Core block, the
//! Kindle fixed-layout metas, the manifest and — most delicately — the spine's
//! `page-spread-*` properties. That algorithm is ported verbatim: a forward pass
//! that alternates left/right (honouring the `-kcc-a`…`-kcc-d` spread specials)
//! followed by a backward fix-up pass that anchors the tail of the book, with
//! `--spread-shift`, `--one-page-landscape` and the PDF/EPUB source flip applied
//! as the reference does.
//!
//! The document skeleton lives in `templates/content.opf`, `templates/style.css`
//! and the `CONTAINER_XML` literal; this module computes the values they
//! interpolate (AGENTS.md §5.3).

use std::path::Path;

use anyhow::Result;
use askama::Template;

use super::templates::{Opf, OpfItem, SpineItem, StyleCss};
use super::{html_escape, images_dir, text_dir, unique_id, PageRef};
use crate::ebook::metadata::BookMetadata;
use crate::ebook::options::Options;

/// KCC's `KindleComicConverter-<version>` contributor string.
const CONTRIBUTOR: &str = "KindleComicConverter-11.3.2";

/// `META-INF/container.xml`; fully static, so it is a literal rather than a
/// template.
pub(crate) const CONTAINER_XML: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles>
<rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
</rootfiles>
</container>"#;

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
) -> Result<String> {
    let device = (options.profile_data.width, options.profile_data.height);

    // `--vertical-4-panel` writes top-to-bottom; `--invert-direction` swaps the
    // two suffixes relative to the normal rule.
    let writing_mode = format!(
        "{}{}",
        if options.vertical_4_panel {
            "vertical"
        } else {
            "horizontal"
        },
        match (options.invert_direction, options.right_to_left) {
            (true, true) | (false, false) => "-lr",
            _ => "-rl",
        }
    );

    let manifest = manifest_items(filelist);

    let (direction, initial_side) = match (options.invert_direction, options.right_to_left) {
        (true, false) | (false, true) => ("rtl", "right"),
        _ => ("ltr", "left"),
    };
    let initial_side = flip_for_source(initial_side, source, options);
    let reflist: Vec<String> = filelist.iter().map(unique_id).collect();
    let spread = spread_properties(&reflist, options.right_to_left, initial_side);
    let spine: Vec<SpineItem> = reflist
        .iter()
        .zip(&spread)
        .map(|(entry, property)| {
            let property = if options.one_page_landscape {
                "center"
            } else {
                property
            };
            SpineItem {
                idref: format!("page_{entry}"),
                attr: page_spread_property(property, options),
            }
        })
        .collect();

    let title = html_escape(title);
    let has_description = !metadata.summary.is_empty();
    let description = html_escape(&metadata.summary);
    let creators: Vec<String> = metadata
        .authors
        .iter()
        .map(|author| html_escape(author))
        .collect();

    // Series metadata is only meaningful for non-Kindle readers.
    let has_series = !options.is_kindle && !metadata.series.is_empty();
    let series = html_escape(&metadata.series);
    let group = if !metadata.volume.is_empty() && !metadata.number.is_empty() {
        Some(format!("{}.{}", metadata.volume, metadata.number))
    } else if !metadata.volume.is_empty() {
        Some(metadata.volume.clone())
    } else if !metadata.number.is_empty() {
        Some(metadata.number.clone())
    } else {
        None
    };
    let has_group = has_series && group.is_some();
    let group = html_escape(group.as_deref().unwrap_or(""));

    let view = Opf {
        title: &title,
        language,
        uuid,
        contributor: CONTRIBUTOR,
        has_description,
        description: &description,
        creators: &creators,
        has_series,
        series: &series,
        has_group,
        group: &group,
        modified,
        has_cover,
        kindle_layout: options.is_kindle && !options.custom_profile,
        device_width: device.0,
        device_height: device.1,
        writing_mode: &writing_mode,
        region_mag: if options.kfx { "false" } else { "true" },
        manifest: &manifest,
        direction,
        spine: &spine,
    };
    // askama drops a single trailing newline from every template; KCC's OPF is
    // newline terminated (AGENTS.md §5.2).
    let mut out = view.render()?;
    out.push('\n');
    Ok(out)
}

/// The OPF manifest: a page item and an image item per page, plus the `-below`
/// image of a Scribe `-above` tall-page split (KCC's `buildOPF`).
fn manifest_items(filelist: &[PageRef<'_>]) -> Vec<OpfItem> {
    let mut manifest = Vec::with_capacity(filelist.len() * 2);
    for entry in filelist {
        let id = unique_id(entry);
        manifest.push(OpfItem {
            id: format!("page_{id}"),
            href: format!("{}/{}.xhtml", text_dir(entry.image_dir), entry.stem),
            media_type: "application/xhtml+xml",
            properties: String::new(),
            has_properties_before: false,
            has_properties_after: false,
        });
        manifest.push(OpfItem {
            id: format!("img_{id}"),
            href: format!("{}/{}", images_dir(entry.image_dir), entry.file),
            media_type: entry.media_type.mime(),
            properties: String::new(),
            has_properties_before: false,
            has_properties_after: false,
        });
        if let Some(below) = entry.below {
            let below_id = id.replace("above", "below");
            let below_file = below.name.rsplit('/').next().unwrap_or(below.name.as_str());
            manifest.push(OpfItem {
                id: format!("img_{below_id}"),
                href: format!("{}/{}", images_dir(entry.image_dir), below_file),
                media_type: below.media_type.mime(),
                properties: String::new(),
                has_properties_before: false,
                has_properties_after: false,
            });
        }
    }
    manifest
}

/// The shared `style.css`.
pub(crate) fn style_css(options: &Options) -> Result<String> {
    let view = StyleCss {
        scribe: options.kindle_scribe_azw3,
        panel: options.is_kindle && options.panel_view,
    };
    Ok(view.render()?)
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
    fn a_scribe_above_page_adds_its_below_image_to_the_manifest() {
        use crate::ebook::model::{EncodedPage, MediaType, OrderClass, PageFlags};

        let below = EncodedPage {
            name: "kcc-0001-kcc-x-below.jpg".to_string(),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            bytes: Vec::new(),
            width: 100,
            height: 50,
            flags: PageFlags {
                order_class: OrderClass::Normal,
                rotated: false,
                black_background: false,
                above: false,
                below: true,
            },
        };
        let entry = PageRef {
            image_dir: "Chapter 1",
            file: "kcc-0001-kcc-x-above.jpg",
            stem: "kcc-0001-kcc-x-above",
            width: 100,
            height: 150,
            flags: PageFlags {
                order_class: OrderClass::Normal,
                rotated: false,
                black_background: false,
                above: true,
                below: false,
            },
            media_type: MediaType::Jpeg,
            below: Some(&below),
        };

        let items = manifest_items(&[entry]);
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "page_Images_Chapter 1_kcc-0001-kcc-x-above",
                "img_Images_Chapter 1_kcc-0001-kcc-x-above",
                "img_Images_Chapter 1_kcc-0001-kcc-x-below",
            ]
        );
        assert_eq!(items[2].href, "Images/Chapter 1/kcc-0001-kcc-x-below.jpg");
    }

    #[test]
    fn html_escape_matches_python() {
        assert_eq!(
            html_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#x27;f"
        );
    }
}
