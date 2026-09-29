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
//!
//! The small enums and newtypes below give the formatted values a compiler-checked
//! identity: a page side, a writing mode, a `<manifest>` id or a Panel View region
//! can no longer be confused with a neighbouring `&'static str` (see
//! docs/refactor.md B7/B8/B9/B10/D13). They are all `Copy`/`#[repr(transparent)]` and compiler-erased,
//! and they render through [`fmt::Display`] exactly as the strings they replaced.

use std::borrow::Cow;
use std::fmt;

use anyhow::Result;
use askama::Template;

use super::{FileName, Stem};
use crate::ebook::model::MediaType;
use crate::ebook::options::ReaderFamily;
use crate::units::Size;

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

/// A page's physical side in the `page-spread-*` algorithm (KCC's `pageside`).
///
/// `Center` is a distinct state (a `-cb-a`/`-cb-d` spread special or a
/// `--one-page-landscape` page), not an "unset" value, so the three-way choice is
/// exhaustive and `other()` has no impossible input (docs/refactor.md B7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageSide {
    Left,
    Right,
    Center,
}

impl PageSide {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PageSide::Left => "left",
            PageSide::Right => "right",
            PageSide::Center => "center",
        }
    }

    /// The opposite side. `Center` is never alternated in the algorithm; mapping
    /// it to itself keeps the function total without inventing a dummy side.
    pub(crate) fn other(self) -> PageSide {
        match self {
            PageSide::Left => PageSide::Right,
            PageSide::Right => PageSide::Left,
            PageSide::Center => PageSide::Center,
        }
    }
}

impl fmt::Display for PageSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The spine's `page-progression-direction` (KCC's `(invert_direction,
/// right_to_left)` XOR, computed once — docs/refactor.md B8/G1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Ltr,
    Rtl,
}

impl Direction {
    pub(crate) fn from_flags(invert_direction: bool, right_to_left: bool) -> Self {
        if invert_direction ^ right_to_left {
            Direction::Rtl
        } else {
            Direction::Ltr
        }
    }

    /// The opening page side before `flip_for_source` (KCC's `initial_side`).
    pub(crate) fn opening_side(self) -> PageSide {
        match self {
            Direction::Ltr => PageSide::Left,
            Direction::Rtl => PageSide::Right,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Direction::Ltr => "ltr",
            Direction::Rtl => "rtl",
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `primary-writing-mode` meta: KCC's four exact spellings of the
/// (orientation × direction) pair (docs/refactor.md B8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WritingMode {
    HorizontalLr,
    HorizontalRl,
    VerticalLr,
    VerticalRl,
}

impl WritingMode {
    /// Whether the page is written top-to-bottom (`--vertical-4-panel`).
    pub(crate) fn resolve(vertical: bool, direction: Direction) -> Self {
        match (vertical, direction) {
            (false, Direction::Ltr) => WritingMode::HorizontalLr,
            (false, Direction::Rtl) => WritingMode::HorizontalRl,
            (true, Direction::Ltr) => WritingMode::VerticalLr,
            (true, Direction::Rtl) => WritingMode::VerticalRl,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WritingMode::HorizontalLr => "horizontal-lr",
            WritingMode::HorizontalRl => "horizontal-rl",
            WritingMode::VerticalLr => "vertical-lr",
            WritingMode::VerticalRl => "vertical-rl",
        }
    }
}

impl fmt::Display for WritingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A manifest `<item media-type>`.
///
/// The `ncx`/`nav`/`cover`/`css` items are static in `templates/content.opf`, so
/// only the two variants an [`OpfItem`] actually holds are represented
/// (docs/refactor.md B10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestMediaType {
    Xhtml,
    Image(MediaType),
}

impl ManifestMediaType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ManifestMediaType::Xhtml => "application/xhtml+xml",
            ManifestMediaType::Image(media_type) => media_type.mime(),
        }
    }
}

impl fmt::Display for ManifestMediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An OPF manifest `<item id>`; centralises the `page_`/`img_`/`-below`
/// conventions so they cannot drift between the manifest and the spine
/// (docs/refactor.md D13).
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct ManifestId(String);

impl ManifestId {
    pub(crate) fn page(unique_id: &str) -> Self {
        ManifestId(format!("page_{unique_id}"))
    }

    pub(crate) fn image(unique_id: &str) -> Self {
        ManifestId(format!("img_{unique_id}"))
    }

    /// The Scribe `-below` companion image id (KCC's `id.replace("above", "below")`,
    /// a global replace kept bug-compatible). Derived from [`ManifestId::image`] so
    /// the `img_` convention has a single definition.
    pub(crate) fn below_image(above_unique_id: &str) -> Self {
        ManifestId::image(&above_unique_id.replace("above", "below"))
    }
}

impl fmt::Display for ManifestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A spine `<itemref idref>`.
///
/// Holds the [`ManifestId`] it references rather than a bare string, so a spine
/// reference and its manifest item share one id value and cannot spell different
/// id conventions.
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct Idref(ManifestId);

impl Idref {
    /// The spine reference for the manifest item of the same page id.
    pub(crate) fn page(unique_id: &str) -> Self {
        Idref(ManifestId::page(unique_id))
    }
}

impl fmt::Display for Idref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A path relative to `OEBPS` (`<item href>`, `<navPoint><content src>`, `<a href>`).
///
/// The two kinds are distinguished so a page reference and an image reference
/// cannot be substituted for one another at a construction site.
#[derive(Debug)]
pub(crate) enum Href {
    /// A page's XHTML document.
    Xhtml(String),
    /// An image (a manifest `<item href>`).
    Image(String),
}

impl Href {
    /// A page XHTML path from its text directory and stem.
    pub(crate) fn xhtml(text_dir: &str, stem: Stem<'_>) -> Self {
        Href::Xhtml(format!("{text_dir}/{stem}.xhtml"))
    }

    /// An image path from its images directory and file name.
    pub(crate) fn image(images_dir: &str, file: FileName<'_>) -> Self {
        Href::Image(format!("{images_dir}/{file}"))
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Href::Xhtml(href) | Href::Image(href) => href,
        }
    }
}

impl fmt::Display for Href {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A spine `<itemref>`'s spread-attribute fragment (`linear="…" properties="…"`).
///
/// Constructed only by [`SpineAttr::page_spread`], so the two reader-family
/// spellings have a single definition and there is no `new(any String)` escape
/// hatch.
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct SpineAttr(String);

impl SpineAttr {
    /// The `page-spread-*` attribute for `side`, spelled per reader family (KCC's
    /// `pageSpreadProperty`).
    pub(crate) fn page_spread(side: PageSide, reader: ReaderFamily) -> Self {
        let property = side.as_str();
        match reader {
            ReaderFamily::Kindle => SpineAttr(format!(
                "linear=\"yes\" properties=\"page-spread-{property}\""
            )),
            ReaderFamily::Kobo => {
                SpineAttr(format!("properties=\"rendition:page-spread-{property}\""))
            }
        }
    }
}

impl fmt::Display for SpineAttr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A `navPoint`/`li` anchor id: a path with `/` folded to `_`.
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct NavId(String);

impl NavId {
    pub(crate) fn folded(value: &str) -> Self {
        NavId(value.replace('/', "_"))
    }
}

impl fmt::Display for NavId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A navigation title, HTML-escaped on construction.
///
/// Escaping happens here, not at the call site, so a `NavTitle` is always
/// HTML-safe and an unescaped title is unrepresentable.
#[repr(transparent)]
#[derive(Debug)]
pub(crate) struct NavTitle(String);

impl NavTitle {
    pub(crate) fn new(value: &str) -> Self {
        NavTitle(super::html_escape(value))
    }
}

impl fmt::Display for NavTitle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The non-Kindle `belongs-to-collection` metadata: a series name and its
/// optional `group-position` (docs/refactor.md C9). `group` is only meaningful when the
/// series is present, which the nested `Option` models directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Series<'a> {
    pub name: &'a str,
    pub group: Option<&'a str>,
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
    pub viewport: Size,
    /// The `body` style: empty, or the black-background colour.
    pub body_style: &'a str,
    /// The Kindle `display:none` spacer div.
    pub kindle_spacer: bool,
    /// The page image's own `<img>` size.
    pub img_size: Size,
    /// The page image reference (`../Images/<chapter>/<file>`).
    pub image_src: &'a str,
    /// The Kindle Scribe `-below` second image, if this is an `-above` page.
    pub below: Option<BelowImage<'a>>,
    /// Whether the Kindle virtual Panel View block is emitted.
    pub panel: bool,
    pub boxes: &'a [PanelBox],
    /// The Panel View region size.
    pub panel_size: Size,
}

/// The Kindle Scribe `-below` companion image of an `-above` page: its reference
/// and size (`top: 1920px` under the first image). Replaces the old
/// `has_below`/`below_image_src`/`below_img_width`/`below_img_height` group, whose
/// members had to agree by hand (docs/refactor.md C8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BelowImage<'a> {
    pub src: &'a str,
    pub size: Size,
}

/// A Kindle Panel View region: the closed set of KCC `PV-*` ids.
///
/// Fieldless, so the grid tables, the element id and the `style` can never
/// disagree and adding a region is a compile error rather than a silently-empty
/// `style` (docs/refactor.md B9/E7). The ids match `templates/style.css`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PanelId {
    /// Top-left quadrant.
    Tl,
    /// Top-right quadrant.
    Tr,
    /// Bottom-left quadrant.
    Bl,
    /// Bottom-right quadrant.
    Br,
    /// Top half (page narrower than the screen).
    T,
    /// Bottom half (page narrower than the screen).
    B,
    /// Left half (page shorter than the screen).
    L,
    /// Right half (page shorter than the screen).
    R,
}

impl PanelId {
    /// KCC's `PV-*` element id.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PanelId::Tl => "PV-TL",
            PanelId::Tr => "PV-TR",
            PanelId::Bl => "PV-BL",
            PanelId::Br => "PV-BR",
            PanelId::T => "PV-T",
            PanelId::B => "PV-B",
            PanelId::L => "PV-L",
            PanelId::R => "PV-R",
        }
    }

    /// The region's magnified `<img>` `style`; `x`/`y` are the centred percent
    /// offsets of the split axes. The quadrant styles are borrowed (no
    /// allocation); only the split axes, whose offsets are runtime values, own.
    pub(crate) fn style(self, x: i64, y: i64) -> Cow<'static, str> {
        match self {
            PanelId::Tl => Cow::Borrowed("position:absolute;left:0;top:0;"),
            PanelId::Tr => Cow::Borrowed("position:absolute;right:0;top:0;"),
            PanelId::Bl => Cow::Borrowed("position:absolute;left:0;bottom:0;"),
            PanelId::Br => Cow::Borrowed("position:absolute;right:0;bottom:0;"),
            PanelId::T => Cow::Owned(format!("position:absolute;top:0;left:{x}%;")),
            PanelId::B => Cow::Owned(format!("position:absolute;bottom:0;left:{x}%;")),
            PanelId::L => Cow::Owned(format!("position:absolute;left:0;top:{y}%;")),
            PanelId::R => Cow::Owned(format!("position:absolute;right:0;top:{y}%;")),
        }
    }
}

impl fmt::Display for PanelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One Panel View region, its magnify ordinal and `img` style.
pub(crate) struct PanelBox {
    pub id: PanelId,
    pub ordinal: u32,
    pub style: Cow<'static, str>,
}

/// `content.opf` (`buildOPF`).
#[derive(Template)]
#[template(path = "content.opf", escape = "none")]
pub(crate) struct Opf<'a> {
    pub title: &'a str,
    pub language: &'a str,
    pub uuid: &'a str,
    pub contributor: &'a str,
    /// The Dublin Core description, present only when the metadata has a summary.
    pub description: Option<&'a str>,
    pub creators: &'a [String],
    /// The `belongs-to-collection` metadata, present only for non-Kindle readers.
    pub series: Option<Series<'a>>,
    pub modified: &'a str,
    pub has_cover: bool,
    /// The Kindle fixed-layout metas (a Kindle reader without custom geometry).
    pub kindle_layout: bool,
    /// The device screen size (`original-resolution`).
    pub device: Size,
    pub writing_mode: WritingMode,
    pub region_mag: bool,
    pub manifest: &'a [OpfItem],
    pub direction: Direction,
    pub spine: &'a [SpineItem],
}

/// One `<item>` in the OPF manifest.
pub(crate) struct OpfItem {
    pub id: ManifestId,
    pub href: Href,
    pub media_type: ManifestMediaType,
}

/// One `<itemref>` in the OPF spine.
pub(crate) struct SpineItem {
    pub idref: Idref,
    pub attr: SpineAttr,
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
    pub id: NavId,
    pub title: NavTitle,
    pub source: Href,
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
    use super::{
        normalize_lf, render_lf, Direction, FileName, Href, NavTitle, Opf, PageSide, Series,
        SpineAttr, Stem, WritingMode,
    };
    use crate::units::Size;

    #[test]
    fn spine_attrs_use_the_per_reader_spelling() {
        use crate::ebook::options::ReaderFamily;
        assert_eq!(
            SpineAttr::page_spread(PageSide::Left, ReaderFamily::Kindle).to_string(),
            "linear=\"yes\" properties=\"page-spread-left\""
        );
        assert_eq!(
            SpineAttr::page_spread(PageSide::Right, ReaderFamily::Kobo).to_string(),
            "properties=\"rendition:page-spread-right\""
        );
    }

    #[test]
    fn nav_titles_are_escaped_on_construction() {
        assert_eq!(
            NavTitle::new("a&b<c>d\"e'f").to_string(),
            "a&amp;b&lt;c&gt;d&quot;e&#x27;f"
        );
    }

    #[test]
    fn hrefs_distinguish_pages_from_images() {
        assert_eq!(
            Href::xhtml("Text", Stem::new("cb-0001-cb-x")).to_string(),
            "Text/cb-0001-cb-x.xhtml"
        );
        assert_eq!(
            Href::image("Images/Chapter 1", FileName::new("cb-0001-cb-x.jpg")).to_string(),
            "Images/Chapter 1/cb-0001-cb-x.jpg"
        );
    }

    #[test]
    fn a_series_with_a_group_renders_both_meta_lines() -> anyhow::Result<()> {
        // The `belongs-to-collection`/`group-position` path is not reached by any
        // golden (they are Kindle or series-less), so pin it here (docs/refactor.md C9).
        let view = Opf {
            title: "Title",
            language: "en",
            uuid: "UUID",
            contributor: "C",
            description: None,
            creators: &[],
            series: Some(Series {
                name: "Berserk",
                group: Some("3.7"),
            }),
            modified: "2020-01-01T00:00:00Z",
            has_cover: false,
            kindle_layout: false,
            device: Size::new(0, 0),
            writing_mode: WritingMode::HorizontalLr,
            region_mag: true,
            manifest: &[],
            direction: Direction::Ltr,
            spine: &[],
        };
        let out = render_lf(&view)?;
        assert!(out.contains("<meta property=\"belongs-to-collection\" id=\"c02\">Berserk</meta>"));
        assert!(out.contains("<meta refines=\"#c02\" property=\"group-position\">3.7</meta>"));
        Ok(())
    }

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
