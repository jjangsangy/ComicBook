//! Per-page XHTML generation — `buildHTML` (see docs/output.md).
//!
//! One fixed-layout XHTML file is written per encoded page. The image is
//! referenced through the `Images/` tree with one `../` per chapter level plus the
//! implicit `Text/` level, and the `viewport`/`img` sizes come from the processed
//! page's dimensions, halved-and-a-bit in `--hq` mode exactly as the reference
//! does. A Kindle Scribe tall-page split adds a second `<img>` at `top: 1920px`, and
//! the `viewport` spans both images. Kindle Panel View markup is emitted when the
//! profile and options enable it.
//!
//! The document skeleton lives in `templates/page.xhtml`; this module computes the
//! values it interpolates (see docs/dependencies.md).

use anyhow::Result;
use relative_path::RelativePath;

use super::html_escape;
use super::templates::{render_lf, BelowImage, PageXhtml, PanelBox, PanelId};
use super::PageRef;
use crate::ebook::model::{Orientation, PageFlags};
use crate::ebook::options::{Options, PanelView, ReaderFamily};
use crate::units::Size;

/// Build one page's XHTML (`buildHTML`).
///
/// `page` carries the image's chapter directory (relative to `OEBPS/Images`), its
/// file name and its extension-less stem (the `<title>` and the XHTML file name
/// come from the stem), plus the optional second image of a Kindle Scribe tall-page
/// split, laid out under the first at `top: 1920px`.
pub(crate) fn build_xhtml(page: &PageRef<'_>, options: &Options) -> Result<Vec<u8>> {
    let PageRef {
        image_dir,
        file,
        size,
        flags,
        below,
        ..
    } = *page;
    let stem = page.stem();
    let depth = RelativePath::new(image_dir.as_str()).components().count();
    // KCC walks `dirpath` up to the `Images` component, counting one `../` per
    // level and one for the `Text/` level itself.
    let backref = depth + 1;
    let postfix = if depth == 0 {
        String::new()
    } else {
        format!("{image_dir}/")
    };
    let style_href = format!("{}style.css", "../".repeat(backref - 1));
    let image_src = format!("{}Images/{postfix}{file}", "../".repeat(backref));

    // The viewport spans the stacked page (KCC's `imgsizeframe`), but each `<img>`
    // keeps its own size.
    let frame_height = size.height + below.map_or(0, |image| image.size.height);
    let viewport = if options.main.hq {
        Size::new(
            (f64::from(size.width) / 1.5).floor() as u32,
            (f64::from(frame_height) / 1.5).floor() as u32,
        )
    } else {
        Size::new(size.width, frame_height)
    };

    let body_style = if flags.background.is_black() {
        "background-color:#000000;"
    } else {
        ""
    };
    let title = html_escape(stem.as_str());

    // The Scribe `-below` reference: `format!` allocates only when a companion
    // exists; the absent case is an empty, never-referenced `String`.
    let below_src = match below {
        Some(image) => {
            let file = image.name.as_relative().file_name().unwrap_or("");
            format!("{}Images/{postfix}{file}", "../".repeat(backref))
        }
        None => String::new(),
    };
    let below_image = below.map(|image| BelowImage {
        src: below_src.as_str(),
        size: image.size,
    });

    let panel = options.panel_view_enabled();
    let (boxes, panel_size) = if panel {
        panel_layout(size, flags, options)
    } else {
        (Vec::new(), size)
    };

    let view = PageXhtml {
        title: &title,
        style_href: &style_href,
        viewport,
        body_style,
        kindle_spacer: options.device.reader == ReaderFamily::Kindle,
        img_size: size,
        image_src: &image_src,
        below: below_image,
        panel,
        boxes: &boxes,
        panel_size,
    };
    // askama drops a single trailing newline from every template; KCC's page
    // XHTML is newline terminated (see docs/architecture.md).
    let mut out = render_lf(&view)?;
    out.push('\n');
    Ok(out.into_bytes())
}

/// Panel View regions for the 2x2 grid, in KCC's `boxes` order.
const PANELS_2X2: [PanelId; 4] = [PanelId::Tl, PanelId::Tr, PanelId::Bl, PanelId::Br];
/// Panel View regions when the page is narrower than the screen (`PV-T`/`PV-B`).
const PANELS_STACKED: [PanelId; 2] = [PanelId::T, PanelId::B];
/// Panel View regions when the page is shorter than the screen (`PV-L`/`PV-R`).
const PANELS_SIDE_BY_SIDE: [PanelId; 2] = [PanelId::L, PanelId::R];

/// The Panel View grid shape for a page (KCC's `buildHTML` axis test).
///
/// Modelling the shape as an enum (rather than a `(no_horizontal, no_vertical)`
/// `bool` pair) makes the four reachable shapes exhaustive, and keys both the
/// region list and its magnification order off one value, so the two can never
/// disagree (docs/refactor.md §2.1/§2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelGrid {
    /// The page fills the screen in both axes: no panels.
    None,
    /// The page is smaller than the screen in both axes: the 2x2 quadrants.
    Quadrants,
    /// The page overflows one axis only: the top/bottom halves.
    Stacked,
    /// The page overflows the other axis only: the left/right halves.
    SideBySide,
}

impl PanelGrid {
    /// Classify from the two axis predicates: `no_horizontal` is the scaled width
    /// not exceeding the screen, `no_vertical` the same for the height.
    fn classify(no_horizontal: bool, no_vertical: bool) -> Self {
        match (no_horizontal, no_vertical) {
            (true, true) => PanelGrid::None,
            (false, false) => PanelGrid::Quadrants,
            (true, false) => PanelGrid::Stacked,
            (false, true) => PanelGrid::SideBySide,
        }
    }

    /// The regions in KCC's `boxes` order.
    fn regions(self) -> &'static [PanelId] {
        match self {
            PanelGrid::None => &[],
            PanelGrid::Quadrants => &PANELS_2X2,
            PanelGrid::Stacked => &PANELS_STACKED,
            PanelGrid::SideBySide => &PANELS_SIDE_BY_SIDE,
        }
    }

    /// The magnification ordinals, keyed by orientation and reading direction.
    fn order(self, orientation: Orientation, right_to_left: bool) -> &'static [u32] {
        match self {
            PanelGrid::None => &[],
            PanelGrid::Quadrants => match (orientation, right_to_left) {
                (Orientation::Rotated, true) => &[1, 3, 2, 4],
                (Orientation::Rotated, false) => &[2, 4, 1, 3],
                (Orientation::Upright, true) => &[2, 1, 4, 3],
                (Orientation::Upright, false) => &[1, 2, 3, 4],
            },
            PanelGrid::Stacked => match (orientation, right_to_left) {
                (Orientation::Rotated, true) => &[1, 2],
                (Orientation::Rotated, false) => &[2, 1],
                (Orientation::Upright, _) => &[1, 2],
            },
            PanelGrid::SideBySide => match (orientation, right_to_left) {
                (Orientation::Rotated, _) => &[1, 2],
                (Orientation::Upright, true) => &[2, 1],
                (Orientation::Upright, false) => &[1, 2],
            },
        }
    }
}

/// The Kindle virtual Panel View grid (`buildHTML`'s `PV-*` block).
///
/// The panel grid depends on how the page's scaled size compares with the device
/// screen: a page far smaller than the screen in one axis drops those panels.
fn panel_layout(size: Size, flags: PageFlags, options: &Options) -> (Vec<PanelBox>, Size) {
    let device = options.device_size();

    // `--two-panel` scales the page to the device width; `--hq` magnifies by 1.5x.
    let scaled = match options.main.panel_view {
        PanelView::Two => {
            let scale = f64::from(device.width) / f64::from(size.width);
            Size::new(device.width, (scale * f64::from(size.height)) as u32)
        }
        PanelView::Hq => size,
        PanelView::Legacy | PanelView::Off => Size::new(
            (f64::from(size.width) * 1.5) as u32,
            (f64::from(size.height) * 1.5) as u32,
        ),
    };

    let no_horizontal =
        f64::from(scaled.width) - f64::from(device.width) < f64::from(device.width) * 0.01;
    let no_vertical =
        f64::from(scaled.height) - f64::from(device.height) < f64::from(device.height) * 0.01;

    let x = panel_offset(device.width, scaled.width);
    let y = panel_offset(device.height, scaled.height);

    let right_to_left = options.main.right_to_left();
    let grid = PanelGrid::classify(no_horizontal, no_vertical);

    let boxes = grid
        .regions()
        .iter()
        .zip(grid.order(flags.orientation, right_to_left))
        .map(|(&id, &ordinal)| PanelBox {
            id,
            ordinal,
            style: id.style(x, y),
        })
        .collect();

    (boxes, scaled)
}

/// KCC's `getPanelViewSize`: the percentage offset of a centred panel.
fn panel_offset(device: u32, size: u32) -> i64 {
    let inner = (f64::from(device) / 2.0 - f64::from(size) / 2.0).trunc();
    (inner / f64::from(device) * 100.0).trunc() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::model::{EncodedPage, MediaType, OrderClass, PageFlags, PageName};
    use crate::ebook::output::epub::{FileName, ImageDir};
    use clap::Parser;

    /// Resolve options from a `comic-book ebook` command line.
    fn options(args: &[&str]) -> Result<Options> {
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = crate::cli::Cli::try_parse_from(full)?;
        match cli.command {
            crate::cli::Commands::Ebook(args) => Options::resolve(&args),
            _ => anyhow::bail!("expected the ebook subcommand"),
        }
    }

    #[test]
    fn panel_offset_centres_the_panel() {
        // A 642px-wide panel on a 1072px screen sits ~20% in from the edge.
        assert_eq!(panel_offset(1072, 642), 20);
        // A panel wider than the screen overhangs both edges (negative offset).
        assert_eq!(panel_offset(600, 800), -16);
    }

    #[test]
    fn panel_grids_are_classified_by_which_axis_the_page_fills() {
        assert_eq!(PanelGrid::classify(true, true), PanelGrid::None);
        assert_eq!(PanelGrid::classify(false, false), PanelGrid::Quadrants);
        assert_eq!(PanelGrid::classify(true, false), PanelGrid::Stacked);
        assert_eq!(PanelGrid::classify(false, true), PanelGrid::SideBySide);
    }

    #[test]
    fn panel_grids_map_to_their_regions() {
        assert!(PanelGrid::None.regions().is_empty());
        assert_eq!(PanelGrid::Quadrants.regions(), &PANELS_2X2);
        assert_eq!(PanelGrid::Stacked.regions(), &PANELS_STACKED);
        assert_eq!(PanelGrid::SideBySide.regions(), &PANELS_SIDE_BY_SIDE);
    }

    #[test]
    fn panel_orders_key_off_orientation_and_reading_direction() {
        assert!(PanelGrid::None
            .order(Orientation::Upright, false)
            .is_empty());

        assert_eq!(
            PanelGrid::Quadrants.order(Orientation::Rotated, true),
            &[1u32, 3, 2, 4]
        );
        assert_eq!(
            PanelGrid::Quadrants.order(Orientation::Rotated, false),
            &[2u32, 4, 1, 3]
        );
        assert_eq!(
            PanelGrid::Quadrants.order(Orientation::Upright, true),
            &[2u32, 1, 4, 3]
        );
        assert_eq!(
            PanelGrid::Quadrants.order(Orientation::Upright, false),
            &[1u32, 2, 3, 4]
        );

        assert_eq!(
            PanelGrid::Stacked.order(Orientation::Rotated, true),
            &[1u32, 2]
        );
        assert_eq!(
            PanelGrid::Stacked.order(Orientation::Rotated, false),
            &[2u32, 1]
        );
        assert_eq!(
            PanelGrid::Stacked.order(Orientation::Upright, true),
            &[1u32, 2]
        );

        assert_eq!(
            PanelGrid::SideBySide.order(Orientation::Rotated, false),
            &[1u32, 2]
        );
        assert_eq!(
            PanelGrid::SideBySide.order(Orientation::Upright, true),
            &[2u32, 1]
        );
        assert_eq!(
            PanelGrid::SideBySide.order(Orientation::Upright, false),
            &[1u32, 2]
        );
    }

    #[test]
    fn two_panel_scales_the_page_to_the_device_width() -> Result<()> {
        // A tall page under `--two-panel`: the grid scales it to the device width and
        // emits the stacked (top/bottom) regions.
        let options = options(&["-p", "KV", "-2"])?;
        let (boxes, scaled) = panel_layout(Size::new(400, 600), PageFlags::default(), &options);
        assert_eq!(scaled.width, options.device_size().width);
        assert_eq!(
            boxes.iter().map(|panel| panel.id).collect::<Vec<_>>(),
            PANELS_STACKED
        );
        Ok(())
    }

    #[test]
    fn a_scribe_below_image_is_referenced_from_the_above_page() -> Result<()> {
        // The `-above` page carries its `-below` companion as a second stacked image;
        // the viewport spans both while each `<img>` keeps its own size.
        let below = EncodedPage {
            name: PageName::new("cb-0001-cb-x-below.jpg"),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            bytes: Vec::new(),
            size: Size::new(1653, 560),
            flags: PageFlags::default(),
        };
        let page = PageRef {
            image_dir: ImageDir::new(""),
            file: FileName::new("cb-0001-cb-x-above.jpg"),
            size: Size::new(1653, 1920),
            flags: PageFlags::default(),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            below: Some(&below),
        };
        let xhtml = String::from_utf8(build_xhtml(&page, &options(&["-p", "KS"])?)?)?;
        assert!(
            xhtml.contains("src=\"../Images/cb-0001-cb-x-below.jpg\""),
            "{xhtml}"
        );
        assert!(xhtml.contains("top: 1920px"), "{xhtml}");
        // The viewport spans the stacked pair (1920 + 560).
        assert!(xhtml.contains("height=2480"), "{xhtml}");
        Ok(())
    }
}
