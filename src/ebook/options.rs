//! Resolved run configuration.
//!
//! [`Options::resolve`] is the Rust equivalent of KCC's `checkOptions()`: it
//! takes the raw CLI arguments and derives the concrete device, format and
//! processing flags the rest of the pipeline consumes (see docs/architecture.md).
//!
//! The resolved surface is grouped by concern — [`DeviceOptions`],
//! [`MainOptions`], [`ProcessingOptions`], [`OutputOptions`] and
//! [`SessionOptions`] — so each stage can take the smallest group it reads
//! instead of the whole run. [`Options`] itself is a thin aggregate.

use anyhow::{bail, Result};
use clap::ValueEnum;
use std::path::PathBuf;

use super::cli::EbookArgs;
use super::profiles::{Profile, ProfileData, PALETTE16};
use crate::units::{Fraction, Megabytes, Percent, Quality, Size};

/// User-selectable output format (see docs/cli.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Pick by profile: MOBI for Kindle, PDF for reMarkable, otherwise EPUB.
    Auto,
    /// Fixed-layout EPUB 3.
    Epub,
    /// Kobo KePub (`.kepub.epub`).
    Kepub,
    /// KF8-only Kindle file.
    Azw3,
    /// Dual MOBI7 + KF8 `.mobi`.
    Mobi,
    /// Keep the intermediate EPUB alongside the MOBI.
    #[value(name = "mobi+epub")]
    MobiEpub,
    /// Repackage the processed images as a comic archive.
    Cbz,
    /// PDF.
    Pdf,
    /// EPUB preset for Calibre's KFX Output plugin.
    Kfx,
    /// EPUB preset capped at ~200 MB.
    #[value(name = "epub-200mb")]
    Epub200mb,
    /// PDF preset capped at ~200 MB.
    #[value(name = "pdf-200mb")]
    Pdf200mb,
    /// MOBI + EPUB preset capped at ~200 MB.
    #[value(name = "mobi+epub-200mb")]
    MobiEpub200mb,
}

impl Format {
    /// True when this is a Kindle MOBI-family format.
    fn is_mobi_family(self) -> bool {
        matches!(
            self,
            Format::Mobi | Format::MobiEpub | Format::Azw3 | Format::MobiEpub200mb
        )
    }
}

/// `--doc-type` for Kindle output (see docs/output.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum DocType {
    /// Leave the doc-type untouched (avoids the firmware "back-to-library" issue).
    #[default]
    None,
    /// Force the EBOK tag.
    Ebok,
    /// Force the PDOC tag.
    Pdoc,
}

/// Page background override from `--borders` (also the legacy
/// `--black-borders` / `--white-borders`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BorderColor {
    /// Force white borders.
    White,
    /// Force black borders.
    Black,
}

/// Double-page parsing mode (`-r, --splitter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Splitter {
    /// Always split a spread into two pages (KCC's `0`).
    #[default]
    #[value(alias = "0")]
    Split,
    /// Always rotate a spread upright (KCC's `1`).
    #[value(alias = "1")]
    Rotate,
    /// Split narrow spreads and rotate wide ones (KCC's `2`).
    #[value(alias = "2")]
    Both,
}

/// Cropping mode (`-c, --cropping`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Cropping {
    /// Cropping disabled (KCC's `0`).
    #[value(alias = "0")]
    Off,
    /// Crop the page margins (KCC's `1`).
    #[value(alias = "1")]
    Margins,
    /// Crop margins and page numbers (KCC's `2`, the default).
    #[default]
    #[value(name = "pages", alias = "2")]
    PageNumbers,
}

/// Empty-section cropping (`--inter-panel-crop`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum InterPanelCrop {
    /// Disabled (KCC's `0`, the default).
    #[default]
    #[value(alias = "0")]
    Off,
    /// Crop horizontal (column) gutters (KCC's `1`).
    #[value(alias = "1")]
    Horizontal,
    /// Crop both horizontal and vertical gutters (KCC's `2`).
    #[value(alias = "2")]
    Both,
}

/// How a resolved title uses embedded ComicInfo metadata (`--metadata-title`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum MetadataTitle {
    /// Use the default title schema only (KCC's `0`, the default).
    #[default]
    #[value(alias = "0")]
    Default,
    /// Append the embedded title to the default schema (KCC's `1`).
    #[value(alias = "1")]
    Combine,
    /// Use the embedded title only (KCC's `2`).
    #[value(alias = "2")]
    Only,
}

/// Output splitting mode (`-b, --batch-split`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum BatchSplit {
    /// Keep everything in one output file (KCC's `0`, the default).
    #[default]
    #[value(alias = "0")]
    None,
    /// Split automatically to respect the size cap (KCC's `1`).
    #[value(alias = "1")]
    Auto,
    /// Give each top-level directory its own output file (KCC's `2`).
    #[value(name = "per-subdir", alias = "2")]
    PerSubdirectory,
}

impl BatchSplit {
    /// Force automatic splitting unless the user already asked for per-directory splitting
    /// (KCC's preset expansion).
    pub fn ensure_splitting(&mut self) {
        if *self != BatchSplit::PerSubdirectory {
            *self = BatchSplit::Auto;
        }
    }
}

/// The reader family a resolved run targets (KCC's `iskindle` / `isKobo`).
///
/// `Kobo` is KCC's `isKobo`, which really means *not* Kindle: it also covers
/// reMarkable and `Other`, matching the reference's `rendition:` spread spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderFamily {
    Kindle,
    Kobo,
}

/// Where the device geometry came from (`--custom-width`/`--custom-height`).
///
/// Replaces the old `custom_profile` flag paired with a `"Custom"` name sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometry {
    /// The named profile's own screen geometry.
    Profile(Profile),
    /// An explicit resolution overriding the profile's.
    Custom { width: u32, height: u32 },
}

/// Reading layout (`--light-novel` / `--wallpaper`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Regular,
    LightNovel,
    Wallpaper,
}

/// Kindle Panel View mode (`-q/--hq`, `-2/--two-panel`, `--legacy-panel-view`).
///
/// Panel View markup is only *emitted* for Kindle readers; the variant selects
/// the panel-grid scaling, exactly as KCC's `buildHTML` does. `--vertical-4-panel`
/// is an independent axis (it only affects the OPF writing mode) and stays a
/// separate [`MainOptions::vertical_4_panel`] flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelView {
    /// Panel View markup disabled (KCC's `panelview == False`).
    #[default]
    Off,
    /// `-q/--hq`: the grid uses the page's own size (KCC's `hq`).
    Hq,
    /// `-2/--two-panel`: the grid scales to the device width (KCC's `autoscale`).
    Two,
    /// `--legacy-panel-view`: the grid magnifies by 1.5x (KCC's `legacypanelview`).
    Legacy,
}

/// Gamma correction (`-g/--gamma`).
///
/// A value below `0.1` means "use the profile's gamma" (KCC's fallback), so the
/// knob is really a choice between the automatic profile gamma and an explicit one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Gamma {
    /// Use the profile's gamma (KCC's `gamma < 0.1` fallback).
    #[default]
    Auto,
    /// An explicit gamma value.
    Linear(f32),
}

/// Autocontrast behaviour (`--no-autocontrast` / `--auto-level`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Autocontrast {
    /// The ordinary contrast stretch (KCC's default).
    #[default]
    Contrast,
    /// Disabled (`--no-autocontrast`).
    Off,
    /// Contrast stretch preceded by an autolevel pass (`--auto-level`).
    Level,
}

/// The resolved output encoding: the request [`Format`] with its preset
/// expansions and KePub/KFX/MOBI sub-flags folded in.
///
/// Replaces the old `format` field plus the `kfx` / `kepub` / `keep_epub` /
/// `kindle_azw3` / `kindle_scribe_azw3` derived booleans, so `write_tome` can
/// dispatch with an exhaustive `match` (no `bail!`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputEncoding {
    /// Fixed-layout EPUB; `kfx` selects the Calibre KFX preset.
    Epub { kfx: bool },
    /// Kobo KePub; `short_ext` selects the single `.kepub` extension.
    Kepub { short_ext: bool },
    /// Dual MOBI7 + KF8; `keep_epub` retains the intermediate EPUB.
    ///
    /// (`kindle_scribe_azw3` lives on [`ProcessingOptions::scribe`] because it also
    /// applies to EPUB/MOBI output on a Scribe, not just AZW3.)
    Mobi { keep_epub: bool },
    /// KF8-only Kindle file.
    Azw3,
    /// A comic archive.
    Cbz,
    /// A PDF.
    Pdf,
}

impl OutputEncoding {
    /// True for the Kindle image path (KCC's `kindle_azw3`): a Kindle reader
    /// writing an EPUB, MOBI or AZW3.
    pub fn is_kindle_azw3(self) -> bool {
        matches!(
            self,
            OutputEncoding::Epub { .. } | OutputEncoding::Mobi { .. } | OutputEncoding::Azw3
        )
    }
}

/// The concrete encoding a request [`Format`] resolves to once preset expansion and the
/// `auto` profile rule have stripped away every preset variant (docs/refactor.md B5).
///
/// [`Options::resolve`] derives this once, so building the [`OutputEncoding`] is an
/// exhaustive `match` and no "unresolved format" guard can exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedFormat {
    Epub,
    Mobi,
    Azw3,
    Cbz,
    Pdf,
}

/// Colour handling (`--force-color` / `--color-autocontrast`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorTuning {
    pub force_color: bool,
    pub autocontrast_color: bool,
}

/// PNG encoding switches (`--force-png` / `--force-png-rgb` / `--png-legacy` /
/// `--no-quantize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PngOptions {
    pub force: bool,
    pub force_rgb: bool,
    pub legacy: bool,
    pub no_quantize: bool,
}

/// Webtoon strip handling (`--maximize-strips`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Strips {
    pub maximize: bool,
}

/// Resize switches (`--upscale` / `--stretch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sizing {
    pub upscale: bool,
    pub stretch: bool,
}

/// Spread rotation switches (`--no-rotate` / `--rotate-right` / `--rotate-first`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RotationOptions {
    pub no_rotate: bool,
    pub right: bool,
    pub first: bool,
}

/// Cover switches (`--smart-cover-crop` / `--cover-fill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoverOptions {
    pub smart_crop: bool,
    pub fill: bool,
}

/// Source-extraction switches (`--legacy-extract` / `--pdf-width`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceOptions {
    pub legacy_extract: bool,
    pub pdf_width: bool,
}

/// Device profile and screen geometry (`DeviceArgs` + `CustomProfileArgs`).
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    pub profile: Profile,
    /// The resolved screen geometry (after custom overrides and device caps).
    pub data: ProfileData,
    pub reader: ReaderFamily,
    pub geometry: Geometry,
}

/// Reading direction and layout switches (`MainArgs`).
#[derive(Debug, Clone)]
pub struct MainOptions {
    pub manga: bool,
    pub hq: bool,
    pub layout: Layout,
    pub panel_view: PanelView,
    pub webtoon: bool,
    pub invert_direction: bool,
    /// `--vertical-4-panel`: an independent axis driving the OPF writing mode.
    pub vertical_4_panel: bool,
    pub file_fusion: bool,
    pub target_size: Option<Megabytes>,
}

impl MainOptions {
    /// Derived once (KCC's `rightToLeft`); cleared by webtoon mode.
    pub fn right_to_left(&self) -> bool {
        self.manga && !self.webtoon
    }
}

/// Image-processing switches (`ProcessingArgs`).
#[derive(Debug, Clone)]
pub struct ProcessingOptions {
    pub no_processing: bool,
    pub splitter: Splitter,
    pub gamma: Gamma,
    pub autocontrast: Autocontrast,
    pub color: ColorTuning,
    pub cropping: Cropping,
    pub cropping_power: f32,
    pub cropping_minimum: Fraction,
    pub preserve_margin: Option<Percent>,
    pub inter_panel_crop: InterPanelCrop,
    pub borders: Option<BorderColor>,
    pub png: PngOptions,
    /// Derived: `--webp` and an output that keeps WebP (see `Options::resolve`).
    pub webp_output: bool,
    pub jpeg_quality: Quality,
    pub strips: Strips,
    pub sizing: Sizing,
    pub rotation: RotationOptions,
    pub cover: CoverOptions,
    pub erase_rainbow: bool,
    pub source: SourceOptions,
    /// Derived (KCC's `kindle_scribe_azw3`): drives the tall-page split.
    pub scribe: bool,
}

/// Output selection and metadata (`OutputArgs`).
#[derive(Debug, Clone)]
pub struct OutputOptions {
    /// `-o/--output`.
    pub destination: Option<PathBuf>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: String,
    pub metadata_title: MetadataTitle,
    pub keep_comicinfo: bool,
    pub doc_type: DocType,
    pub encoding: OutputEncoding,
    pub batch_split: BatchSplit,
    pub spread_shift: bool,
    pub one_page_landscape: bool,
}

/// Cross-cutting side effects (`-d/--delete`, `--temp-dir`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionOptions {
    pub delete: bool,
    pub temp_dir: bool,
}

/// A fully resolved `comic-book ebook` run: one small group per concern.
#[derive(Debug, Clone)]
pub struct Options {
    pub inputs: Vec<PathBuf>,
    pub device: DeviceOptions,
    pub main: MainOptions,
    pub processing: ProcessingOptions,
    pub output: OutputOptions,
    pub session: SessionOptions,
}

impl Options {
    /// The resolved page size: the profile geometry enlarged by 1.5x under `--hq`
    /// panel-view mode (KCC's `imgsizeframe`).
    pub fn profile_size(&self) -> Size {
        let size = self.device_size();
        if self.main.hq {
            Size::new(
                (f64::from(size.width) * 1.5) as u32,
                (f64::from(size.height) * 1.5) as u32,
            )
        } else {
            size
        }
    }

    /// The raw profile geometry (before the `--hq` enlargement), shared by the
    /// cover, light-novel and OPF/XHTML builders.
    pub fn device_size(&self) -> Size {
        Size::new(self.device.data.width, self.device.data.height)
    }

    /// KCC's `kindle_azw3`: a Kindle reader writing an EPUB, MOBI or AZW3.
    pub fn kindle_azw3(&self) -> bool {
        matches!(self.device.reader, ReaderFamily::Kindle) && self.output.encoding.is_kindle_azw3()
    }

    /// Whether Kindle Panel View markup is emitted (KCC's `panelview`): a Kindle reader with
    /// a panel mode selected. Derived once so the OPF and page builders stay in sync
    /// (docs/refactor.md G2).
    pub fn panel_view_enabled(&self) -> bool {
        matches!(self.device.reader, ReaderFamily::Kindle) && self.main.panel_view != PanelView::Off
    }

    /// Resolve raw CLI arguments into a run configuration (KCC's `checkOptions`).
    pub fn resolve(args: &EbookArgs) -> Result<Self> {
        let profile = args.device.profile;
        let reader = if profile.is_kindle() {
            ReaderFamily::Kindle
        } else {
            // KCC's `isKobo` really means "not Kindle" (Kobo, reMarkable, Other).
            ReaderFamily::Kobo
        };

        if args.processing.mozjpeg {
            bail!("--mozjpeg is not supported; use --jpeg-quality to control JPEG output");
        }

        let mut no_kepub = args.output.no_kepub;
        if args.main.light_novel {
            no_kepub = true;
        }

        // MOBI/Send-to-Kindle output is Kindle-only (KCC raises the same error).
        let request = args.output.format;
        if reader == ReaderFamily::Kobo
            && (request.is_mobi_family() || matches!(request, Format::Kfx | Format::Epub200mb))
        {
            bail!("MOBI/Send to Kindle output is not supported for non-Kindle profiles");
        }

        // The request `Format` is consumed here: preset expansion and the `auto` profile
        // rule turn it into one of the five concrete encodings plus their side effects, so
        // no later stage can observe a preset variant (docs/refactor.md B5).
        let mut target_size = args.main.target_size;
        let mut batch_split = args.output.batch_split;
        let mut keep_epub = false;
        let mut kfx = false;
        let mut kepub_requested = false;
        let resolved = match request {
            // `auto` picks by profile: CBZ for the DX, MOBI for Kindle (which always
            // splits), PDF for reMarkable, else EPUB.
            Format::Auto if profile == Profile::Kdx => ResolvedFormat::Cbz,
            Format::Auto if profile.is_kindle() => {
                batch_split.ensure_splitting();
                ResolvedFormat::Mobi
            }
            Format::Auto if profile.is_remarkable() => ResolvedFormat::Pdf,
            Format::Auto => ResolvedFormat::Epub,
            Format::Epub => ResolvedFormat::Epub,
            Format::Kepub => {
                kepub_requested = true;
                ResolvedFormat::Epub
            }
            Format::Azw3 => ResolvedFormat::Azw3,
            Format::Mobi => {
                batch_split.ensure_splitting();
                ResolvedFormat::Mobi
            }
            // `mobi+epub` keeps the intermediate EPUB.
            Format::MobiEpub => {
                keep_epub = true;
                batch_split.ensure_splitting();
                ResolvedFormat::Mobi
            }
            Format::Cbz => ResolvedFormat::Cbz,
            Format::Pdf => ResolvedFormat::Pdf,
            // `kfx` is an EPUB preset aimed at Calibre's KFX Output plugin.
            Format::Kfx => {
                target_size = Some(195);
                kfx = true;
                batch_split.ensure_splitting();
                ResolvedFormat::Epub
            }
            // Size-capped presets expand to their base format and force splitting.
            Format::Epub200mb => {
                target_size = Some(195);
                batch_split.ensure_splitting();
                ResolvedFormat::Epub
            }
            Format::Pdf200mb => {
                target_size = Some(195);
                batch_split.ensure_splitting();
                ResolvedFormat::Pdf
            }
            Format::MobiEpub200mb => {
                keep_epub = true;
                target_size = Some(195);
                batch_split.ensure_splitting();
                ResolvedFormat::Mobi
            }
        };

        if target_size.is_none() && profile.is_remarkable() {
            target_size = Some(95);
        }

        let mut borders = args
            .processing
            .borders
            .or_else(|| args.processing.white_borders.then_some(BorderColor::White))
            .or_else(|| args.processing.black_borders.then_some(BorderColor::Black));

        let mut hq = args.main.hq;
        let mut panel_view = true;
        // Older Kindle models don't support Panel View.
        if matches!(
            profile,
            Profile::K1 | Profile::K2 | Profile::K34 | Profile::Kdx
        ) {
            panel_view = false;
            hq = false;
        }
        if !hq && !args.main.two_panel && !args.main.legacy_panel_view {
            panel_view = false;
        }

        // Webtoon mode mandates its own option set.
        let mut upscale = args.processing.upscale;
        if args.main.webtoon {
            panel_view = false;
            upscale = false;
            hq = false;
            borders = Some(BorderColor::White);
        }

        // Disable all Kindle features for other e-readers.
        if profile == Profile::Other || profile.is_kobo_brand() {
            panel_view = false;
            hq = false;
        }

        // KFX output disables Panel View (see docs/cli.md).
        if kfx {
            panel_view = false;
        }

        // Custom screen geometry replaces the profile's.
        let mut data = profile.data();
        let custom_width = args.custom.custom_width;
        let custom_height = args.custom.custom_height;
        let custom = custom_width != 0 || custom_height != 0;
        if custom {
            if custom_width != 0 {
                data.width = custom_width;
            }
            if custom_height != 0 {
                data.height = custom_height;
            }
            data.palette = &PALETTE16;
            data.gamma = 1.0;
        }

        // The `OTHER` profile carries no screen geometry of its own (see docs/cli.md),
        // so resizing against it produces nothing; it is only meaningful with an
        // explicit custom resolution. KCC divides by the zero width and crashes here.
        if !custom && (data.width == 0 || data.height == 0) {
            bail!(
                "profile {} has no screen size; set --custom-width and --custom-height",
                profile.code()
            );
        }

        let jpeg_quality = Quality::new(args.processing.jpeg_quality.unwrap_or_else(|| {
            if profile.is_scribe() || profile == Profile::Kcs {
                90
            } else {
                85
            }
        }))?;

        let kindle_azw3 = reader == ReaderFamily::Kindle
            && matches!(
                resolved,
                ResolvedFormat::Mobi | ResolvedFormat::Epub | ResolvedFormat::Azw3
            );
        let scribe = profile.is_scribe() && kindle_azw3;
        let webp_output = resolved != ResolvedFormat::Pdf && !kindle_azw3 && args.processing.webp;

        // CBZ on the Kindle DX/DXG supports a taller image (see docs/cli.md).
        if profile == Profile::Kdx && resolved == ResolvedFormat::Cbz {
            data.height = 1200;
        }
        // Scribe KF8 output caps the width at 1920 (see docs/cli.md).
        if scribe {
            data.width = data.width.min(1920);
        }

        // KePub is an EPUB variant for Kobo devices (either requested outright or implied
        // by a Kobo-brand profile writing an EPUB).
        let kepub = kepub_requested
            || (profile.is_kobo_brand() && resolved == ResolvedFormat::Epub && !no_kepub);
        // The shortened extension only makes sense for KePub output.
        if args.output.kepub_short_ext && !kepub {
            bail!("--kepub-short-ext requires KePub output; use `-f kepub` or a Kobo profile");
        }

        let encoding = if kepub {
            OutputEncoding::Kepub {
                short_ext: args.output.kepub_short_ext,
            }
        } else {
            match resolved {
                ResolvedFormat::Epub => OutputEncoding::Epub { kfx },
                ResolvedFormat::Mobi => OutputEncoding::Mobi { keep_epub },
                ResolvedFormat::Azw3 => OutputEncoding::Azw3,
                ResolvedFormat::Cbz => OutputEncoding::Cbz,
                ResolvedFormat::Pdf => OutputEncoding::Pdf,
            }
        };

        let panel_view = if !panel_view {
            PanelView::Off
        } else if args.main.two_panel {
            PanelView::Two
        } else if hq {
            PanelView::Hq
        } else {
            PanelView::Legacy
        };

        // `--light-novel` shadows `--wallpaper` only on the path where it actually
        // short-circuits the page pipeline (the non-fusion path); under `--file-fusion`
        // the page pipeline runs, so wallpaper's fit branch must still be selected.
        let layout = if args.main.light_novel && !args.main.file_fusion {
            Layout::LightNovel
        } else if args.main.wallpaper {
            Layout::Wallpaper
        } else if args.main.light_novel {
            Layout::LightNovel
        } else {
            Layout::Regular
        };

        let gamma = if args.processing.gamma < 0.1 {
            Gamma::Auto
        } else {
            Gamma::Linear(args.processing.gamma)
        };

        let autocontrast = if args.processing.no_auto_contrast {
            Autocontrast::Off
        } else if args.processing.auto_level {
            Autocontrast::Level
        } else {
            Autocontrast::Contrast
        };

        let geometry = if custom {
            Geometry::Custom {
                width: data.width,
                height: data.height,
            }
        } else {
            Geometry::Profile(profile)
        };

        Ok(Options {
            inputs: args.input.clone(),
            device: DeviceOptions {
                profile,
                data,
                reader,
                geometry,
            },
            main: MainOptions {
                manga: args.main.manga,
                hq,
                layout,
                panel_view,
                webtoon: args.main.webtoon,
                invert_direction: args.main.invert_direction,
                vertical_4_panel: args.main.vertical_4_panel,
                file_fusion: args.main.file_fusion,
                target_size: target_size.map(Megabytes::new),
            },
            processing: ProcessingOptions {
                no_processing: args.processing.no_processing,
                splitter: args.processing.splitter,
                gamma,
                autocontrast,
                color: ColorTuning {
                    force_color: args.processing.force_color,
                    autocontrast_color: args.processing.color_auto_contrast,
                },
                cropping: args.processing.cropping,
                cropping_power: args.processing.cropping_power,
                cropping_minimum: Fraction::new(f64::from(args.processing.cropping_minimum)),
                preserve_margin: (args.processing.preserve_margin != 0)
                    .then(|| Percent::new(f64::from(args.processing.preserve_margin))),
                inter_panel_crop: args.processing.inter_panel_crop,
                borders,
                png: PngOptions {
                    force: args.processing.force_png,
                    force_rgb: args.processing.force_png_rgb,
                    legacy: args.processing.png_legacy,
                    no_quantize: args.processing.no_quantize,
                },
                webp_output,
                jpeg_quality,
                strips: Strips {
                    maximize: args.processing.maximize_strips,
                },
                sizing: Sizing {
                    upscale,
                    stretch: args.processing.stretch,
                },
                rotation: RotationOptions {
                    no_rotate: args.processing.no_rotate,
                    right: args.processing.rotate_right,
                    first: args.processing.rotate_first,
                },
                cover: CoverOptions {
                    smart_crop: args.processing.smart_cover_crop,
                    fill: args.processing.cover_fill,
                },
                erase_rainbow: args.processing.erase_rainbow,
                source: SourceOptions {
                    legacy_extract: args.processing.legacy_extract,
                    pdf_width: args.processing.pdf_width,
                },
                scribe,
            },
            output: OutputOptions {
                destination: args.output.output.clone(),
                title: args.output.title.clone(),
                author: args.output.author.clone(),
                language: args.output.language.clone(),
                metadata_title: args.output.metadata_title,
                keep_comicinfo: args.output.keep_comicinfo,
                doc_type: args.output.doc_type,
                encoding,
                batch_split,
                spread_shift: args.output.spread_shift,
                one_page_landscape: args.output.one_page_landscape,
            },
            session: SessionOptions {
                delete: args.processing.delete,
                temp_dir: args.processing.temp_dir,
            },
        })
    }
}
