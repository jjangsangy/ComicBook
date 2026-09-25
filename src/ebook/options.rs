//! Resolved run configuration.
//!
//! [`Options::resolve`] is the Rust equivalent of KCC's `checkOptions()`: it
//! takes the raw CLI arguments and derives the concrete device, format and
//! processing flags the rest of the pipeline consumes (AGENTS.md §8).

use anyhow::{bail, Result};
use clap::ValueEnum;
use std::path::PathBuf;

use super::cli::EbookArgs;
use super::profiles::{Profile, ProfileData, PALETTE16};

/// User-selectable output format (AGENTS.md §4.3).
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

/// `--doc-type` for Kindle output (AGENTS.md §9).
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

/// Page background override from `--black-borders` / `--white-borders`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderColor {
    White,
    Black,
}

/// A fully resolved `comic-book ebook` run.
#[derive(Debug, Clone)]
pub struct Options {
    // Inputs
    pub inputs: Vec<PathBuf>,

    // Device / profile
    pub profile: Profile,
    pub profile_data: ProfileData,
    pub device_kind: super::profiles::DeviceKind,
    pub is_kindle: bool,
    /// KCC's `isKobo`, which really means "not Kindle" (Kobo, reMarkable, Other).
    pub is_kobo: bool,
    /// True when `--custom-width`/`--custom-height` replaced the profile geometry.
    pub custom_profile: bool,

    // Main
    pub manga: bool,
    pub light_novel: bool,
    pub wallpaper: bool,
    pub invert_direction: bool,
    pub hq: bool,
    pub two_panel: bool,
    pub vertical_4_panel: bool,
    pub legacy_panel_view: bool,
    pub webtoon: bool,
    pub target_size: Option<u32>,
    pub file_fusion: bool,
    pub panel_view: bool,

    // Processing
    pub no_processing: bool,
    pub splitter: u8,
    pub gamma: f32,
    pub auto_level: bool,
    pub no_auto_contrast: bool,
    pub color_auto_contrast: bool,
    pub cropping: u8,
    pub cropping_power: f32,
    pub cropping_minimum: f32,
    pub preserve_margin: u32,
    pub inter_panel_crop: u8,
    pub borders_color: Option<BorderColor>,
    pub force_color: bool,
    pub erase_rainbow: bool,
    pub force_png: bool,
    pub force_png_rgb: bool,
    pub webp: bool,
    pub png_legacy: bool,
    pub no_quantize: bool,
    pub jpeg_quality: u8,
    pub maximize_strips: bool,
    pub upscale: bool,
    pub stretch: bool,
    pub no_rotate: bool,
    pub rotate_right: bool,
    pub rotate_first: bool,
    pub smart_cover_crop: bool,
    pub cover_fill: bool,
    pub legacy_extract: bool,
    pub pdf_width: bool,
    pub delete: bool,
    pub temp_dir: bool,

    // Output
    pub output: Option<PathBuf>,
    pub title: Option<String>,
    pub metadata_title: u8,
    pub keep_comicinfo: bool,
    pub author: Option<String>,
    pub language: String,
    pub format: Format,
    pub doc_type: DocType,
    pub batch_split: u8,
    pub spread_shift: bool,
    pub one_page_landscape: bool,
    pub no_kepub: bool,
    pub right_to_left: bool,

    // Derived output flags
    pub kfx: bool,
    pub kepub: bool,
    pub keep_epub: bool,
    pub kindle_azw3: bool,
    pub kindle_scribe_azw3: bool,
    pub webp_output: bool,
}

impl Options {
    /// Resolve raw CLI arguments into a run configuration (KCC's `checkOptions`).
    pub fn resolve(args: &EbookArgs) -> Result<Self> {
        let profile = args.device.profile;
        let is_kindle = profile.is_kindle();
        let is_kobo = !is_kindle;

        let mut format = args.output.format;
        let mut target_size = args.main.target_size;
        let mut batch_split = args.output.batch_split;
        let mut keep_epub = false;
        let mut kfx = false;
        let mut no_kepub = args.output.no_kepub;
        let mut panel_view = true;
        let mut hq = args.main.hq;
        let mut right_to_left = args.main.manga;
        let mut upscale = args.processing.upscale;

        if args.processing.mozjpeg {
            bail!("--mozjpeg is not supported; use --jpeg-quality to control JPEG output");
        }

        if args.main.light_novel {
            no_kepub = true;
        }

        // MOBI/Send-to-Kindle output is Kindle-only (KCC raises the same error).
        if !is_kindle
            && (format.is_mobi_family() || matches!(format, Format::Kfx | Format::Epub200mb))
        {
            bail!("MOBI/Send to Kindle output is not supported for non-Kindle profiles");
        }

        // Size-capped presets expand to their base format and force splitting.
        match format {
            Format::Pdf200mb => {
                target_size = Some(195);
                format = Format::Pdf;
                if batch_split != 2 {
                    batch_split = 1;
                }
            }
            Format::Epub200mb => {
                target_size = Some(195);
                format = Format::Epub;
                if batch_split != 2 {
                    batch_split = 1;
                }
            }
            Format::MobiEpub200mb => {
                keep_epub = true;
                target_size = Some(195);
                format = Format::Mobi;
                if batch_split != 2 {
                    batch_split = 1;
                }
            }
            _ => {}
        }

        if target_size.is_none() && profile.is_remarkable() {
            target_size = Some(95);
        }

        if format == Format::MobiEpub {
            keep_epub = true;
            format = Format::Mobi;
        }

        // `Auto` resolves by profile.
        if format == Format::Auto {
            format = if profile == Profile::Kdx {
                Format::Cbz
            } else if profile.is_kindle() {
                Format::Mobi
            } else if profile.is_remarkable() {
                Format::Pdf
            } else {
                Format::Epub
            };
        }

        let borders_color = if args.processing.white_borders {
            Some(BorderColor::White)
        } else if args.processing.black_borders {
            Some(BorderColor::Black)
        } else {
            None
        };

        // Splitting MOBI is not optional.
        if matches!(format, Format::Mobi | Format::Kfx) && batch_split != 2 {
            batch_split = 1;
        }

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
        let mut borders_color = borders_color;
        if args.main.webtoon {
            panel_view = false;
            right_to_left = false;
            upscale = false;
            hq = false;
            borders_color = Some(BorderColor::White);
        }

        // Disable all Kindle features for other e-readers.
        if profile == Profile::Other || profile.is_kobo_brand() {
            panel_view = false;
            hq = false;
        }

        // KFX output is an EPUB aimed at Calibre's KFX Output plugin.
        if format == Format::Kfx {
            target_size = Some(195);
            format = Format::Epub;
            kfx = true;
            panel_view = false;
        }

        // Custom screen geometry replaces the profile's.
        let mut profile_data = profile.data();
        let custom_profile = args.custom.custom_width != 0 || args.custom.custom_height != 0;
        if custom_profile {
            if args.custom.custom_width != 0 {
                profile_data.width = args.custom.custom_width;
            }
            if args.custom.custom_height != 0 {
                profile_data.height = args.custom.custom_height;
            }
            profile_data.name = "Custom".to_string();
            profile_data.palette = &PALETTE16;
            profile_data.gamma = 1.0;
        }

        let jpeg_quality = args.processing.jpeg_quality.unwrap_or_else(|| {
            if profile.is_scribe() || profile == Profile::Kcs {
                90
            } else {
                85
            }
        });

        let kindle_azw3 = is_kindle && matches!(format, Format::Mobi | Format::Epub | Format::Azw3);
        let kindle_scribe_azw3 = profile.is_scribe() && kindle_azw3;
        let webp_output = format != Format::Pdf && !kindle_azw3 && args.processing.webp;

        // CBZ on the Kindle DX/DXG supports a taller image (§12.1).
        if profile == Profile::Kdx && format == Format::Cbz {
            profile_data.height = 1200;
        }
        // Scribe KF8 output caps the width at 1920 (§12.1).
        if kindle_scribe_azw3 {
            profile_data.width = profile_data.width.min(1920);
        }

        // KePub is an EPUB variant for Kobo devices.
        let mut kepub = false;
        if format == Format::Kepub {
            format = Format::Epub;
            kepub = true;
        }
        if profile.is_kobo_brand() && format == Format::Epub && !no_kepub {
            kepub = true;
        }

        Ok(Options {
            inputs: args.input.clone(),

            profile,
            profile_data,
            device_kind: profile.device_kind(),
            is_kindle,
            is_kobo,
            custom_profile,

            manga: args.main.manga,
            light_novel: args.main.light_novel,
            wallpaper: args.main.wallpaper,
            invert_direction: args.main.invert_direction,
            hq,
            two_panel: args.main.two_panel,
            vertical_4_panel: args.main.vertical_4_panel,
            legacy_panel_view: args.main.legacy_panel_view,
            webtoon: args.main.webtoon,
            target_size,
            file_fusion: args.main.file_fusion,
            panel_view,

            no_processing: args.processing.no_processing,
            splitter: args.processing.splitter,
            gamma: args.processing.gamma,
            auto_level: args.processing.auto_level,
            no_auto_contrast: args.processing.no_auto_contrast,
            color_auto_contrast: args.processing.color_auto_contrast,
            cropping: args.processing.cropping,
            cropping_power: args.processing.cropping_power,
            cropping_minimum: args.processing.cropping_minimum,
            preserve_margin: args.processing.preserve_margin,
            inter_panel_crop: args.processing.inter_panel_crop,
            borders_color,
            force_color: args.processing.force_color,
            erase_rainbow: args.processing.erase_rainbow,
            force_png: args.processing.force_png,
            force_png_rgb: args.processing.force_png_rgb,
            webp: args.processing.webp,
            png_legacy: args.processing.png_legacy,
            no_quantize: args.processing.no_quantize,
            jpeg_quality,
            maximize_strips: args.processing.maximize_strips,
            upscale,
            stretch: args.processing.stretch,
            no_rotate: args.processing.no_rotate,
            rotate_right: args.processing.rotate_right,
            rotate_first: args.processing.rotate_first,
            smart_cover_crop: args.processing.smart_cover_crop,
            cover_fill: args.processing.cover_fill,
            legacy_extract: args.processing.legacy_extract,
            pdf_width: args.processing.pdf_width,
            delete: args.processing.delete,
            temp_dir: args.processing.temp_dir,

            output: args.output.output.clone(),
            title: args.output.title.clone(),
            metadata_title: args.output.metadata_title,
            keep_comicinfo: args.output.keep_comicinfo,
            author: args.output.author.clone(),
            language: args.output.language.clone(),
            format,
            doc_type: args.output.doc_type,
            batch_split,
            spread_shift: args.output.spread_shift,
            one_page_landscape: args.output.one_page_landscape,
            no_kepub,
            right_to_left,

            kfx,
            kepub,
            keep_epub,
            kindle_azw3,
            kindle_scribe_azw3,
            webp_output,
        })
    }
}
