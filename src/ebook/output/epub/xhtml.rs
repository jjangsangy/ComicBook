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

use askama::Template;

use anyhow::Result;

use super::html_escape;
use super::templates::{PageXhtml, PanelBox};
use super::PageRef;
use crate::ebook::model::PageFlags;
use crate::ebook::options::Options;

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
        stem,
        width,
        height,
        flags,
        below,
        ..
    } = *page;
    let depth = image_dir
        .split('/')
        .filter(|segment| !segment.is_empty())
        .count();
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
    let frame_height = height + below.map_or(0, |image| image.height);
    let (viewport_width, viewport_height) = if options.hq {
        (
            (f64::from(width) / 1.5).floor() as u32,
            (f64::from(frame_height) / 1.5).floor() as u32,
        )
    } else {
        (width, frame_height)
    };

    let body_style = if flags.black_background {
        "background-color:#000000;"
    } else {
        ""
    };
    let title = html_escape(stem);

    let (below_src, below_width, below_height) = match below {
        Some(image) => {
            let file = image.name.rsplit('/').next().unwrap_or(image.name.as_str());
            (
                format!("{}Images/{postfix}{file}", "../".repeat(backref)),
                image.width,
                image.height,
            )
        }
        None => (String::new(), 0, 0),
    };

    let panel = options.is_kindle && options.panel_view;
    let (boxes, panel_width, panel_height) = if panel {
        panel_layout(width, height, flags, options)
    } else {
        (Vec::new(), width, height)
    };

    let view = PageXhtml {
        title: &title,
        style_href: &style_href,
        viewport_width,
        viewport_height,
        body_style,
        kindle_spacer: options.is_kindle,
        img_width: width,
        img_height: height,
        image_src: &image_src,
        has_below: below.is_some(),
        below_image_src: &below_src,
        below_img_width: below_width,
        below_img_height: below_height,
        panel,
        boxes: &boxes,
        panel_width,
        panel_height,
    };
    // askama drops a single trailing newline from every template; KCC's page
    // XHTML is newline terminated (see docs/architecture.md).
    let mut out = view.render()?;
    out.push('\n');
    Ok(out.into_bytes())
}

/// The Kindle virtual Panel View grid (`buildHTML`'s `PV-*` block).
///
/// The panel grid depends on how the page's scaled size compares with the device
/// screen: a page far smaller than the screen in one axis drops those panels.
fn panel_layout(
    width: u32,
    height: u32,
    flags: PageFlags,
    options: &Options,
) -> (Vec<PanelBox>, u32, u32) {
    let device = (options.profile_data.width, options.profile_data.height);

    // `--two-panel` scales the page to the device width; `--hq` magnifies by 1.5x.
    let size = if options.two_panel {
        let scale = f64::from(device.0) / f64::from(width);
        (device.0, (scale * f64::from(height)) as u32)
    } else if options.hq {
        (width, height)
    } else {
        (
            (f64::from(width) * 1.5) as u32,
            (f64::from(height) * 1.5) as u32,
        )
    };

    let no_horizontal = f64::from(size.0) - f64::from(device.0) < f64::from(device.0) * 0.01;
    let no_vertical = f64::from(size.1) - f64::from(device.1) < f64::from(device.1) * 0.01;

    let x = panel_offset(device.0, size.0);
    let y = panel_offset(device.1, size.1);

    // The panel order and grid follow `buildHTML`: a rotated page reorders the
    // quadrants, and right-to-left reading mirrors them.
    let (names, order): (&[&'static str], &[u32]) = if !no_horizontal && !no_vertical {
        if flags.rotated {
            if options.right_to_left {
                (&["PV-TL", "PV-TR", "PV-BL", "PV-BR"], &[1, 3, 2, 4])
            } else {
                (&["PV-TL", "PV-TR", "PV-BL", "PV-BR"], &[2, 4, 1, 3])
            }
        } else if options.right_to_left {
            (&["PV-TL", "PV-TR", "PV-BL", "PV-BR"], &[2, 1, 4, 3])
        } else {
            (&["PV-TL", "PV-TR", "PV-BL", "PV-BR"], &[1, 2, 3, 4])
        }
    } else if no_horizontal && !no_vertical {
        if flags.rotated && !options.right_to_left {
            (&["PV-T", "PV-B"], &[2, 1])
        } else {
            (&["PV-T", "PV-B"], &[1, 2])
        }
    } else if !no_horizontal && no_vertical {
        if flags.rotated || !options.right_to_left {
            (&["PV-L", "PV-R"], &[1, 2])
        } else {
            (&["PV-L", "PV-R"], &[2, 1])
        }
    } else {
        (&[], &[])
    };

    let boxes = names
        .iter()
        .zip(order)
        .map(|(&name, &ordinal)| PanelBox {
            id: name,
            ordinal,
            style: panel_style(name, x, y),
        })
        .collect();

    (boxes, size.0, size.1)
}

/// The `style` attribute of a Panel View region.
fn panel_style(name: &str, x: i64, y: i64) -> String {
    match name {
        "PV-TL" => "position:absolute;left:0;top:0;".to_string(),
        "PV-TR" => "position:absolute;right:0;top:0;".to_string(),
        "PV-BL" => "position:absolute;left:0;bottom:0;".to_string(),
        "PV-BR" => "position:absolute;right:0;bottom:0;".to_string(),
        "PV-T" => format!("position:absolute;top:0;left:{x}%;"),
        "PV-B" => format!("position:absolute;bottom:0;left:{x}%;"),
        "PV-L" => format!("position:absolute;left:0;top:{y}%;"),
        "PV-R" => format!("position:absolute;right:0;top:{y}%;"),
        _ => String::new(),
    }
}

/// KCC's `getPanelViewSize`: the percentage offset of a centred panel.
fn panel_offset(device: u32, size: u32) -> i64 {
    let inner = (f64::from(device) / 2.0 - f64::from(size) / 2.0).trunc();
    (inner / f64::from(device) * 100.0).trunc() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_offset_centres_the_panel() {
        // A 642px-wide panel on a 1072px screen sits ~20% in from the edge.
        assert_eq!(panel_offset(1072, 642), 20);
        // A panel wider than the screen overhangs both edges (negative offset).
        assert_eq!(panel_offset(600, 800), -16);
    }
}
