//! Askama view structs for the generated EPUB documents (see docs/dependencies.md).
//!
//! Each struct is a thin view over pre-computed values. All of the KCC parity
//! work — escaping, the spread algorithm, the Panel View grid, the manifest and
//! spine ordering — stays in Rust; these structs only carry the data the
//! document skeleton needs. The templates live in `templates/` and are compiled
//! into the binary by the derive, so a malformed template is a compile error and
//! there is no runtime template parsing (see docs/architecture.md).
//!
//! `escape = "none"` is deliberate: `.xml`/`.xhtml` would otherwise select
//! askama's HTML escaper, but the interpolated strings are already escaped (or
//! intentionally left raw) exactly where the reference does, so the templates
//! must not escape them again.
//!
//! askama drops a single trailing newline from each template (Jinja's
//! `keep_trailing_newline = false`). KCC's OPF and page XHTML are newline
//! terminated, so their wrappers restore it; the NCX and NAV are not.

use anyhow::Result;
use askama::Template;

/// Render an askama view with the template file's line endings boiled down to LF.
///
/// askama embeds each template file verbatim, so a CRLF checkout (Git for
/// Windows' `core.autocrlf`, or a Windows text-mode editor) would otherwise leak
/// `\r\n` into the emitted documents and make the EPUB differ from other
/// platforms. The documents are device-sensitive, so their bytes are pinned to LF
/// here instead of depending on how `templates/` happened to be checked out (see
/// docs/output.md).
pub(crate) fn render_lf<T: Template>(view: &T) -> Result<String> {
    Ok(normalize_lf(view.render()?))
}

/// Fold a rendered document's line endings down to LF.
fn normalize_lf(mut out: String) -> String {
    if !out.contains('\r') {
        return out;
    }
    // askama's `keep_trailing_newline = false` drops the `\n` of a trailing CRLF
    // pair but leaves the `\r`; remove it so a CRLF template renders exactly like
    // an LF one before the remaining line endings are normalised.
    if out.ends_with('\r') {
        out.pop();
    }
    out.replace("\r\n", "\n").replace('\r', "\n")
}

/// One page's XHTML (`buildHTML`).
#[derive(Template)]
#[template(path = "page.xhtml", escape = "none")]
pub(crate) struct PageXhtml<'a> {
    /// Already HTML-escaped: the page stem, as `<title>`.
    pub title: &'a str,
    /// The `style.css` back-reference for this page's depth.
    pub style_href: &'a str,
    /// The `viewport` meta size (the page size, or `--hq`'s halved frame).
    pub viewport_width: u32,
    pub viewport_height: u32,
    /// The `body` style: empty, or the black-background colour.
    pub body_style: &'a str,
    /// The Kindle `display:none` spacer div.
    pub kindle_spacer: bool,
    pub img_width: u32,
    pub img_height: u32,
    /// The page image reference (`../Images/<chapter>/<file>`).
    pub image_src: &'a str,
    /// Whether the Kindle Scribe `-below` second image is emitted.
    pub has_below: bool,
    /// The `-below` image reference and size (`top: 1920px` under the first).
    pub below_image_src: &'a str,
    pub below_img_width: u32,
    pub below_img_height: u32,
    /// Whether the Kindle virtual Panel View block is emitted.
    pub panel: bool,
    pub boxes: &'a [PanelBox],
    pub panel_width: u32,
    pub panel_height: u32,
}

/// One Panel View region (`PV-TL`, `PV-T`, …) and its magnify ordinal.
pub(crate) struct PanelBox {
    pub id: &'static str,
    pub ordinal: u32,
    pub style: String,
}

/// `content.opf` (`buildOPF`).
#[derive(Template)]
#[template(path = "content.opf", escape = "none")]
pub(crate) struct Opf<'a> {
    pub title: &'a str,
    pub language: &'a str,
    pub uuid: &'a str,
    pub contributor: &'a str,
    pub has_description: bool,
    pub description: &'a str,
    pub creators: &'a [String],
    pub has_series: bool,
    pub series: &'a str,
    pub has_group: bool,
    pub group: &'a str,
    pub modified: &'a str,
    pub has_cover: bool,
    /// The Kindle fixed-layout metas (`is_kindle && !custom_profile`).
    pub kindle_layout: bool,
    pub device_width: u32,
    pub device_height: u32,
    pub writing_mode: &'a str,
    pub region_mag: &'a str,
    pub manifest: &'a [OpfItem],
    pub direction: &'a str,
    pub spine: &'a [SpineItem],
}

/// One `<item>` in the OPF manifest. The reference writes `properties` before
/// `media-type` for the nav item and after it for the cover, so the position is
/// carried explicitly.
pub(crate) struct OpfItem {
    pub id: String,
    pub href: String,
    pub media_type: &'static str,
    pub properties: String,
    pub has_properties_before: bool,
    pub has_properties_after: bool,
}

/// One `<itemref>` in the OPF spine; `attr` is the spread property (or empty).
pub(crate) struct SpineItem {
    pub idref: String,
    pub attr: String,
}

/// `toc.ncx` (`buildNCX`).
#[derive(Template)]
#[template(path = "toc.ncx", escape = "none")]
pub(crate) struct Ncx<'a> {
    pub title: &'a str,
    pub language: &'a str,
    pub uuid: &'a str,
    pub navpoints: &'a [NavEntry],
}

/// `nav.xhtml` (`buildNAV`).
#[derive(Template)]
#[template(path = "nav.xhtml", escape = "none")]
pub(crate) struct Nav<'a> {
    pub title: &'a str,
    pub entries: &'a [NavEntry],
}

/// One navigation target, shared by the NCX and NAV documents.
pub(crate) struct NavEntry {
    pub id: String,
    pub title: String,
    pub source: String,
}

/// The shared `style.css`.
#[derive(Template)]
#[template(path = "style.css", escape = "none")]
pub(crate) struct StyleCss {
    /// The Scribe `img { display: block; }` block.
    pub scribe: bool,
    /// The Panel View CSS block.
    pub panel: bool,
}

#[cfg(test)]
mod tests {
    use super::normalize_lf;

    #[test]
    fn lf_documents_are_left_untouched() {
        let text = "a\nb\n".to_string();
        assert_eq!(normalize_lf(text.clone()), text);
    }

    #[test]
    fn crlf_documents_are_folded_to_lf() {
        assert_eq!(normalize_lf("a\r\nb\r\n".to_string()), "a\nb\n");
    }

    #[test]
    fn the_carriage_return_askama_leaves_at_the_end_is_dropped() {
        // `keep_trailing_newline = false` strips the `\n` of a trailing CRLF but
        // leaves the `\r`, so the CRLF render must not gain a stray final newline.
        assert_eq!(normalize_lf("a\r\nb\r".to_string()), "a\nb");
        assert_eq!(normalize_lf("a\r\n\r".to_string()), "a\n");
    }
}
