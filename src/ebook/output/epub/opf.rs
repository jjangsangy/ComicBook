//! `content.opf` generation and the spread-property algorithm (see docs/output.md).
//!
//! The OPF is the device-sensitive heart of the EPUB: the Dublin Core block, the
//! Kindle fixed-layout metas, the manifest and — most delicately — the spine's
//! `page-spread-*` properties. That algorithm is ported verbatim: a forward pass
//! that alternates left/right (honouring the `-cb-a`…`-cb-d` spread specials)
//! followed by a backward fix-up pass that anchors the tail of the book, with
//! `--spread-shift`, `--one-page-landscape` and the PDF/EPUB source flip applied
//! as the reference does.
//!
//! The document skeleton lives in `templates/content.opf`, `templates/style.css`
//! and the `CONTAINER_XML` literal; this module computes the values they
//! interpolate (see docs/dependencies.md).

use std::path::Path;

use anyhow::Result;

use super::templates::{
    render_lf, Direction, Href, Idref, ManifestId, ManifestMediaType, Opf, OpfItem, PageSide,
    Series, SpineAttr, SpineItem, StyleCss, WritingMode,
};
use super::{html_escape, images_dir, text_dir, unique_id, FileName, PageRef};
use crate::ebook::metadata::BookMetadata;
use crate::ebook::model::OrderClass;
use crate::ebook::options::{Geometry, Options, OutputEncoding, ReaderFamily};

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
    let device = options.device_size();

    // The reading direction is the `(invert_direction, right_to_left)` XOR,
    // computed once (docs/refactor.md B8/G1) and shared by the writing mode, the spine
    // progression and the initial page side.
    let direction =
        Direction::from_flags(options.main.invert_direction, options.main.right_to_left());
    let writing_mode = WritingMode::resolve(options.main.vertical_4_panel, direction);

    let manifest = manifest_items(filelist);

    let initial_side = flip_for_source(direction.opening_side(), source, options);
    let order: Vec<OrderClass> = filelist.iter().map(|entry| entry.order_class).collect();
    let spread = spread_properties(&order, options.main.right_to_left(), initial_side);
    let spine: Vec<SpineItem> = filelist
        .iter()
        .zip(&spread)
        .map(|(entry, &side)| {
            let side = if options.output.one_page_landscape {
                PageSide::Center
            } else {
                side
            };
            SpineItem {
                idref: Idref::page(&unique_id(entry)),
                attr: SpineAttr::page_spread(side, options.device.reader),
            }
        })
        .collect();

    let title = html_escape(title);
    let description = (!metadata.summary.is_empty()).then(|| html_escape(&metadata.summary));
    let creators: Vec<String> = metadata
        .authors
        .iter()
        .map(|author| html_escape(author))
        .collect();

    // Series metadata is only meaningful for non-Kindle readers.
    let series_name = html_escape(&metadata.series);
    let series_group = group_position(metadata).map(|value| html_escape(&value));
    let series = if options.device.reader != ReaderFamily::Kindle && !metadata.series.is_empty() {
        Some(Series {
            name: &series_name,
            group: series_group.as_deref(),
        })
    } else {
        None
    };

    let view = Opf {
        title: &title,
        language,
        uuid,
        contributor: CONTRIBUTOR,
        description: description.as_deref(),
        creators: &creators,
        series,
        modified,
        has_cover,
        kindle_layout: options.device.reader == ReaderFamily::Kindle
            && !matches!(options.device.geometry, Geometry::Custom { .. }),
        device,
        writing_mode,
        region_mag: !matches!(options.output.encoding, OutputEncoding::Epub { kfx: true }),
        manifest: &manifest,
        direction,
        spine: &spine,
    };
    // askama drops a single trailing newline from every template; KCC's OPF is
    // newline terminated (see docs/architecture.md).
    let mut out = render_lf(&view)?;
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
            id: ManifestId::page(&id),
            href: Href::xhtml(&text_dir(entry.image_dir), entry.stem()),
            media_type: ManifestMediaType::Xhtml,
        });
        manifest.push(OpfItem {
            id: ManifestId::image(&id),
            href: Href::image(&images_dir(entry.image_dir), entry.file),
            media_type: ManifestMediaType::Image(entry.media_type),
        });
        if let Some(below) = entry.below {
            let below_file = below.name.as_relative().file_name().unwrap_or("");
            manifest.push(OpfItem {
                id: ManifestId::below_image(&id),
                href: Href::image(&images_dir(entry.image_dir), FileName::new(below_file)),
                media_type: ManifestMediaType::Image(below.media_type),
            });
        }
    }
    manifest
}

/// The shared `style.css`.
pub(crate) fn style_css(options: &Options) -> Result<String> {
    let view = StyleCss {
        scribe: options.processing.scribe,
        panel: options.panel_view_enabled(),
    };
    render_lf(&view)
}

/// The `page-spread-*` property for each spine item (KCC's two-pass algorithm).
///
/// The forward pass alternates sides, letting a rotated spread special
/// ([`OrderClass::RotateFirst`]/[`OrderClass::RotateLast`]) reset the direction and
/// a split half ([`OrderClass::SplitLeft`]/[`OrderClass::SplitRight`]) pin its
/// physical side. The backward pass then walks from the end, anchoring the tail so
/// the last pages line up with the book's opening side. The pass is driven by the
/// [`OrderClass`] variants rather than by re-parsing the `-cb-*` name suffix, so it
/// is exhaustive over the classes and a new one is a compile error (docs/refactor.md B7).
fn spread_properties(
    order: &[OrderClass],
    right_to_left: bool,
    mut pageside: PageSide,
) -> Vec<PageSide> {
    let mut sides: Vec<PageSide> = Vec::with_capacity(order.len());

    for &order in order {
        match (right_to_left, order) {
            // A rotated spread special centres the page and re-anchors the side.
            (_, OrderClass::RotateFirst | OrderClass::RotateLast) => {
                sides.push(PageSide::Center);
                pageside = if right_to_left {
                    PageSide::Right
                } else {
                    PageSide::Left
                };
            }
            // A split half pins its physical side.
            (true, OrderClass::SplitLeft) => {
                sides.push(PageSide::Right);
                pageside = PageSide::Right;
            }
            (true, OrderClass::SplitRight) => {
                sides.push(PageSide::Left);
                pageside = PageSide::Right;
            }
            (false, OrderClass::SplitLeft) => {
                sides.push(PageSide::Left);
                pageside = PageSide::Left;
            }
            (false, OrderClass::SplitRight) => {
                sides.push(PageSide::Right);
                pageside = PageSide::Left;
            }
            // An ordinary page alternates from the running side.
            (_, OrderClass::Normal) => {
                sides.push(pageside);
                pageside = pageside.other();
            }
        }
    }

    // Backward fix-up: every page from the first spread special onward keeps the
    // side set by that special; earlier ordinary pages are re-alternated.
    let mut spread_seen = false;
    for index in (0..order.len()).rev() {
        if order[index] == OrderClass::Normal {
            if spread_seen {
                sides[index] = pageside;
                pageside = pageside.other();
            }
        } else {
            spread_seen = true;
            pageside = if right_to_left {
                PageSide::Left
            } else {
                PageSide::Right
            };
        }
    }

    sides
}

/// KCC's `group-position`: `<volume>.<number>`, or whichever part is present.
fn group_position(metadata: &BookMetadata) -> Option<String> {
    if !metadata.volume.is_empty() && !metadata.number.is_empty() {
        Some(format!("{}.{}", metadata.volume, metadata.number))
    } else if !metadata.volume.is_empty() {
        Some(metadata.volume.clone())
    } else if !metadata.number.is_empty() {
        Some(metadata.number.clone())
    } else {
        None
    }
}

/// Flip the opening side for a PDF/EPUB source, then for `--spread-shift`, exactly
/// as `buildOPF` does before its forward pass.
fn flip_for_source(mut side: PageSide, source: &Path, options: &Options) -> PageSide {
    let name = source.to_string_lossy().to_lowercase();
    if name.ends_with(".pdf") || name.ends_with(".epub") {
        side = side.other();
    }
    if options.output.spread_shift {
        side = side.other();
    }
    side
}

/// KCC's `group-position`: `<volume>.<number>`, or whichever part is present.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Size;

    /// Run the spread algorithm over page order classes.
    fn spread(entries: &[OrderClass], right_to_left: bool, initial: PageSide) -> Vec<PageSide> {
        spread_properties(entries, right_to_left, initial)
    }

    #[test]
    fn plain_pages_alternate_from_the_initial_side() {
        use OrderClass::Normal;
        assert_eq!(
            spread(&[Normal, Normal, Normal], false, PageSide::Left),
            vec![PageSide::Left, PageSide::Right, PageSide::Left]
        );
        assert_eq!(
            spread(&[Normal, Normal, Normal], true, PageSide::Right),
            vec![PageSide::Right, PageSide::Left, PageSide::Right]
        );
    }

    #[test]
    fn split_halves_pin_their_sides() {
        use OrderClass::{SplitLeft, SplitRight};
        // Left-to-right: `SplitLeft` is the left half, `SplitRight` the right half.
        assert_eq!(
            spread(&[SplitLeft, SplitRight], false, PageSide::Left),
            vec![PageSide::Left, PageSide::Right]
        );
        // Right-to-left: the split halves keep their physical sides.
        assert_eq!(
            spread(&[SplitLeft, SplitRight], true, PageSide::Right),
            vec![PageSide::Right, PageSide::Left]
        );
    }

    #[test]
    fn a_rotated_spread_is_centred_and_re_anchors_the_pages_before_it() {
        // The backward fix-up pass re-anchors the page before a spread special so
        // the ends of the book meet.
        use OrderClass::{Normal, RotateFirst, RotateLast};
        assert_eq!(
            spread(&[Normal, RotateLast, Normal], false, PageSide::Left),
            vec![PageSide::Right, PageSide::Center, PageSide::Left]
        );
        assert_eq!(
            spread(&[Normal, RotateFirst, Normal], true, PageSide::Right),
            vec![PageSide::Left, PageSide::Center, PageSide::Right]
        );
    }

    #[test]
    fn shifting_the_initial_side_flips_every_plain_page() {
        use OrderClass::Normal;
        assert_eq!(
            spread(&[Normal, Normal], false, PageSide::Right),
            vec![PageSide::Right, PageSide::Left]
        );
    }

    #[test]
    fn a_scribe_above_page_adds_its_below_image_to_the_manifest() {
        use crate::ebook::model::{
            Background, EncodedPage, MediaType, OrderClass, Orientation, PageFlags, PageName,
            ResolvedFill, ScribeHalf,
        };
        use crate::ebook::output::epub::ImageDir;

        let below = EncodedPage {
            name: PageName::new("cb-0001-cb-x-below.jpg"),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            bytes: Vec::new(),
            size: Size::new(100, 50),
            flags: PageFlags {
                orientation: Orientation::Upright,
                background: ResolvedFill::new(Background::White),
                half: ScribeHalf::Below,
            },
        };
        let entry = PageRef {
            image_dir: ImageDir::new("Chapter 1"),
            file: FileName::new("cb-0001-cb-x-above.jpg"),
            size: Size::new(100, 150),
            flags: PageFlags {
                orientation: Orientation::Upright,
                background: ResolvedFill::new(Background::White),
                half: ScribeHalf::Above,
            },
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            below: Some(&below),
        };

        let items = manifest_items(&[entry]);
        let ids: Vec<String> = items.iter().map(|item| item.id.to_string()).collect();
        assert_eq!(
            ids,
            [
                "page_Images_Chapter 1_cb-0001-cb-x-above",
                "img_Images_Chapter 1_cb-0001-cb-x-above",
                "img_Images_Chapter 1_cb-0001-cb-x-below",
            ]
        );
        assert_eq!(
            items[2].href.as_str(),
            "Images/Chapter 1/cb-0001-cb-x-below.jpg"
        );
    }

    #[test]
    fn html_escape_matches_python() {
        assert_eq!(
            html_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&#x27;f"
        );
    }

    /// Resolve options from a `comic-book ebook` command line.
    fn options(args: &[&str]) -> Result<Options> {
        use clap::Parser;
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = crate::cli::Cli::try_parse_from(full)?;
        match cli.command {
            crate::cli::Commands::Ebook(args) => Options::resolve(&args),
            _ => anyhow::bail!("expected the ebook subcommand"),
        }
    }

    #[test]
    fn group_position_uses_whichever_parts_are_present() {
        let metadata = |volume: &str, number: &str| BookMetadata {
            title: String::new(),
            authors: Vec::new(),
            series: String::new(),
            volume: volume.to_string(),
            number: number.to_string(),
            summary: String::new(),
            bookmarks: Vec::new(),
            comicinfo_xml: None,
        };
        assert_eq!(group_position(&metadata("3", "7")).as_deref(), Some("3.7"));
        assert_eq!(group_position(&metadata("3", "")).as_deref(), Some("3"));
        assert_eq!(group_position(&metadata("", "7")).as_deref(), Some("7"));
        assert_eq!(group_position(&metadata("", "")), None);
    }

    #[test]
    fn a_pdf_or_epub_source_flips_the_opening_side() -> Result<()> {
        let plain = options(&[])?;
        assert_eq!(
            flip_for_source(PageSide::Left, Path::new("book.pdf"), &plain),
            PageSide::Right
        );
        assert_eq!(
            flip_for_source(PageSide::Left, Path::new("BOOK.EPUB"), &plain),
            PageSide::Right
        );
        assert_eq!(
            flip_for_source(PageSide::Left, Path::new("book.cbz"), &plain),
            PageSide::Left
        );
        Ok(())
    }

    #[test]
    fn spread_shift_flips_the_opening_side_and_composes_with_the_source() -> Result<()> {
        let shifted = options(&["--spread-shift"])?;
        assert_eq!(
            flip_for_source(PageSide::Left, Path::new("book.cbz"), &shifted),
            PageSide::Right
        );
        // Both flips compose: a PDF plus `--spread-shift` returns to the start.
        assert_eq!(
            flip_for_source(PageSide::Left, Path::new("book.pdf"), &shifted),
            PageSide::Left
        );
        Ok(())
    }
}
