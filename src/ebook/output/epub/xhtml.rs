//! Per-page XHTML generation — `buildHTML` (AGENTS.md §12.2).
//!
//! One fixed-layout XHTML file is written per encoded page. The image is
//! referenced through the `Images/` tree with one `../` per chapter level plus the
//! implicit `Text/` level, and the `viewport`/`img` sizes come from the processed
//! page's dimensions, halved-and-a-bit in `--hq` mode exactly as the reference
//! does. Kindle Panel View markup is emitted when the profile and options enable
//! it.

use super::html_escape;
use crate::ebook::model::PageFlags;
use crate::ebook::options::Options;

/// Build one page's XHTML (`buildHTML`).
///
/// `image_dir` is the chapter directory relative to `OEBPS/Images` (`""` at the
/// root), `file` the image file name and `stem` its extension-less form (both
/// used as the reference does: the `<title>` and the XHTML file name come from the
/// stem).
pub(crate) fn build_xhtml(
    image_dir: &str,
    file: &str,
    stem: &str,
    width: u32,
    height: u32,
    flags: PageFlags,
    options: &Options,
) -> Vec<u8> {
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
    let style_prefix = "../".repeat(backref - 1);
    let image_prefix = "../".repeat(backref);

    let frame = if options.hq {
        (
            (f64::from(width) / 1.5).floor() as u32,
            (f64::from(height) / 1.5).floor() as u32,
        )
    } else {
        (width, height)
    };

    let additional_style = if flags.black_background {
        "background-color:#000000;"
    } else {
        ""
    };

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<!DOCTYPE html>\n");
    out.push_str(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n",
    );
    out.push_str("<head>\n");
    out.push_str(&format!("<title>{}</title>\n", html_escape(stem)));
    out.push_str(&format!(
        "<link href=\"{style_prefix}style.css\" type=\"text/css\" rel=\"stylesheet\"/>\n"
    ));
    out.push_str(&format!(
        "<meta name=\"viewport\" content=\"width={}, height={}\"/>\n",
        frame.0, frame.1
    ));
    out.push_str("</head>\n");
    out.push_str(&format!("<body style=\"{additional_style}\">\n"));
    out.push_str("<div style=\"text-align:center;\">\n");
    if options.is_kindle {
        // This `display:none` div fixes formatting issues with virtual panel mode.
        out.push_str("<div style=\"display:none;\">.</div>\n");
    }
    out.push_str(&format!(
        "<img width=\"{width}\" height=\"{height}\" src=\"{image_prefix}Images/{postfix}{file}\"/>\n"
    ));
    out.push_str("</div>\n");

    if options.is_kindle && options.panel_view {
        panel_view(
            &mut out,
            file,
            width,
            height,
            flags,
            additional_style,
            &image_prefix,
            &postfix,
            options,
        );
    }

    out.push_str("</body>\n");
    out.push_str("</html>\n");
    out.into_bytes()
}

/// Append the Kindle virtual Panel View markup (`buildHTML`'s `PV-*` block).
///
/// The panel grid depends on how the page's scaled size compares with the device
/// screen: a page far smaller than the screen in one axis drops those panels.
#[allow(clippy::too_many_arguments)]
fn panel_view(
    out: &mut String,
    file: &str,
    width: u32,
    height: u32,
    flags: PageFlags,
    additional_style: &str,
    image_prefix: &str,
    postfix: &str,
    options: &Options,
) {
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

    let style = |name: &str| -> String {
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
    };

    // The panel order and grid follow `buildHTML`: a rotated page reorders the
    // quadrants, and right-to-left reading mirrors them.
    let (boxes, order): (&[&str], &[u32]) = if !no_horizontal && !no_vertical {
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

    out.push_str("<div id=\"PV\">\n");
    for (index, box_name) in boxes.iter().enumerate() {
        out.push_str(&format!("<div id=\"{box_name}\">\n"));
        out.push_str(&format!(
            "<a style=\"display:inline-block;width:100%;height:100%;\" class=\"app-amzn-magnify\" \
             data-app-amzn-magnify='{{\"targetId\":\"{box_name}-P\", \"ordinal\":{}}}'></a>\n",
            order[index]
        ));
        out.push_str("</div>\n");
    }
    out.push_str("</div>\n");
    for box_name in boxes {
        out.push_str(&format!(
            "<div class=\"PV-P\" id=\"{box_name}-P\" style=\"{additional_style}\">\n"
        ));
        out.push_str(&format!(
            "<img style=\"{}\" src=\"{image_prefix}Images/{postfix}{file}\" width=\"{}\" height=\"{}\"/>\n",
            style(box_name),
            size.0,
            size.1
        ));
        out.push_str("</div>\n");
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
