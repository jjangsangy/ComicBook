//! Askama view structs for the generated EPUB documents (AGENTS.md §5.3).
//!
//! Each struct is a thin view over pre-computed values. All of the KCC parity
//! work — escaping, the spread algorithm, the Panel View grid, the manifest and
//! spine ordering — stays in Rust; these structs only carry the data the
//! document skeleton needs. The templates live in `templates/` and are compiled
//! into the binary by the derive, so a malformed template is a compile error and
//! there is no runtime template parsing (AGENTS.md §14).
//!
//! `escape = "none"` is deliberate: `.xml`/`.xhtml` would otherwise select
//! askama's HTML escaper, but the interpolated strings are already escaped (or
//! intentionally left raw) exactly where the reference does, so the templates
//! must not escape them again.
//!
//! askama drops a single trailing newline from each template (Jinja's
//! `keep_trailing_newline = false`). KCC's OPF and page XHTML are newline
//! terminated, so their wrappers restore it; the NCX and NAV are not.

use askama::Template;

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
