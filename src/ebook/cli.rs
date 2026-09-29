//! Command-line surface for `comic-book ebook`, grouped like KCC's parser
//! (see docs/cli.md).
//!
//! The structs are pure argument containers; they are resolved into
//! [`Options`](super::options::Options) by [`Options::resolve`](super::options::Options::resolve).

use clap::{Args, ValueHint};
use std::path::PathBuf;

use super::options::{
    BatchSplit, BorderColor, Cropping, DocType, Format, InterPanelCrop, MetadataTitle, Splitter,
};
use super::profiles::Profile;

/// Convert comic archives and folders into e-book formats.
#[derive(Args, Debug, Clone)]
pub struct EbookArgs {
    /// Files, folders or archives to convert
    #[arg(required = true, value_name = "INPUT", value_hint = ValueHint::AnyPath)]
    pub input: Vec<PathBuf>,

    #[command(flatten)]
    pub device: DeviceArgs,

    #[command(flatten)]
    pub main: MainArgs,

    #[command(flatten)]
    pub processing: ProcessingArgs,

    #[command(flatten)]
    pub output: OutputArgs,

    #[command(flatten)]
    pub custom: CustomProfileArgs,
}

/// Device / profile selection.
#[derive(Args, Debug, Clone)]
pub struct DeviceArgs {
    /// Device profile
    #[arg(
        short = 'p',
        long = "profile",
        value_enum,
        ignore_case = true,
        default_value = "KV",
        value_name = "PROFILE"
    )]
    pub profile: Profile,
}

/// Main processing switches.
#[derive(Args, Debug, Clone)]
pub struct MainArgs {
    /// Manga style (right-to-left reading and splitting)
    #[arg(short = 'm', long = "manga")]
    pub manga: bool,

    /// Only resize images and preserve the original file structure
    #[arg(long = "light-novel")]
    pub light_novel: bool,

    /// Crop to fill the screen
    #[arg(long = "wallpaper")]
    pub wallpaper: bool,

    /// Invert page turn direction
    #[arg(long = "invert-direction")]
    pub invert_direction: bool,

    /// Try to increase the quality of magnification
    #[arg(short = 'q', long = "hq")]
    pub hq: bool,

    /// Display two, not four, panels in Panel View mode
    #[arg(short = '2', long = "two-panel")]
    pub two_panel: bool,

    /// Display side panels first in virtual panel view
    #[arg(long = "vertical-4-panel")]
    pub vertical_4_panel: bool,

    /// Use the legacy panel view method from KCC 6
    #[arg(long = "legacy-panel-view")]
    pub legacy_panel_view: bool,

    /// Webtoon processing mode
    #[arg(short = 'w', long = "webtoon")]
    pub webtoon: bool,

    /// Maximal size of the output file in MB
    #[arg(long = "target-size", value_name = "MB")]
    pub target_size: Option<u32>,

    /// Combine all input files into a single book
    #[arg(long = "file-fusion")]
    pub file_fusion: bool,
}

/// Image processing and enhancement options.
#[derive(Args, Debug, Clone)]
pub struct ProcessingArgs {
    /// Do not modify images and ignore any profile or processing option
    #[arg(short = 'n', long = "no-processing")]
    pub no_processing: bool,

    /// Double page parsing mode: split, rotate, or both
    #[arg(
        short = 'r',
        long = "splitter",
        value_enum,
        default_value = "0",
        value_name = "split|rotate|both"
    )]
    pub splitter: Splitter,

    /// Apply gamma correction to linearize the image (auto when 0)
    #[arg(
        short = 'g',
        long = "gamma",
        default_value = "0.0",
        value_name = "FLOAT"
    )]
    pub gamma: f32,

    /// Cropping mode: off, margins, or margins + page numbers
    #[arg(
        short = 'c',
        long = "cropping",
        value_enum,
        default_value = "2",
        value_name = "off|margins|pages"
    )]
    pub cropping: Cropping,

    /// Cropping power
    #[arg(long = "cropping-power", default_value = "1.0", value_name = "FLOAT")]
    pub cropping_power: f32,

    /// Cropping minimum area ratio
    #[arg(long = "cropping-minimum", default_value = "0.0", value_name = "FLOAT")]
    pub cropping_minimum: f32,

    /// After calculating the crop, back up the specified percentage
    #[arg(long = "preserve-margin", default_value = "0", value_name = "PERCENT")]
    pub preserve_margin: u32,

    /// Crop empty sections: off, horizontal, or both
    #[arg(
        long = "inter-panel-crop",
        value_enum,
        default_value = "0",
        value_name = "off|horizontal|both"
    )]
    pub inter_panel_crop: InterPanelCrop,

    /// Force the page border colour instead of autodetecting it
    #[arg(
        long = "borders",
        value_enum,
        value_name = "white|black",
        conflicts_with_all = ["black_borders", "white_borders"]
    )]
    pub borders: Option<BorderColor>,

    /// Deprecated alias for `--borders black`
    #[arg(long = "black-borders", hide = true, conflicts_with = "white_borders")]
    pub black_borders: bool,

    /// Deprecated alias for `--borders white`
    #[arg(long = "white-borders", hide = true, conflicts_with = "black_borders")]
    pub white_borders: bool,

    /// Don't convert images to grayscale
    #[arg(long = "force-color")]
    pub force_color: bool,

    /// Create PNG files instead of JPEG for black and white images
    #[arg(long = "force-png")]
    pub force_png: bool,

    /// Force colour images to be saved as PNG
    #[arg(long = "force-png-rgb")]
    pub force_png_rgb: bool,

    /// Replace JPEG with lossy WebP and PNG with lossless WebP
    #[arg(long = "webp")]
    pub webp: bool,

    /// Use a more compatible 8-bit PNG instead of 4-bit
    #[arg(long = "png-legacy")]
    pub png_legacy: bool,

    /// Don't quantize to a 16-colour PNG
    #[arg(long = "no-quantize")]
    pub no_quantize: bool,

    /// The JPEG quality, on a scale from 0 (worst) to 95 (best)
    #[arg(
        long = "jpeg-quality",
        value_parser = clap::value_parser!(u8).range(0..=95),
        value_name = "0-95"
    )]
    pub jpeg_quality: Option<u8>,

    /// Turn 1x4 strips into 2x2 strips
    #[arg(long = "maximize-strips")]
    pub maximize_strips: bool,

    /// Set the most common dark pixel value as the black point for leveling
    #[arg(long = "auto-level")]
    pub auto_level: bool,

    /// Disable autocontrast
    #[arg(long = "no-autocontrast")]
    pub no_auto_contrast: bool,

    /// Autocontrast colour pages too
    #[arg(long = "color-autocontrast")]
    pub color_auto_contrast: bool,

    /// Erase the rainbow effect on colour e-ink screens
    #[arg(long = "erase-rainbow")]
    pub erase_rainbow: bool,

    /// Attempt to crop the main cover from a wide image
    #[arg(long = "smart-cover-crop")]
    pub smart_cover_crop: bool,

    /// Crop the cover to fill the screen
    #[arg(long = "cover-fill")]
    pub cover_fill: bool,

    /// Resize images smaller than the device's resolution
    #[arg(short = 'u', long = "upscale")]
    pub upscale: bool,

    /// Stretch images to the device's resolution
    #[arg(short = 's', long = "stretch")]
    pub stretch: bool,

    /// Do not rotate double-page spreads in the spread splitter
    #[arg(long = "no-rotate")]
    pub no_rotate: bool,

    /// Rotate double-page spreads in the opposite direction
    #[arg(long = "rotate-right")]
    pub rotate_right: bool,

    /// Put the rotated 2-page spread first in the spread splitter
    #[arg(long = "rotate-first")]
    pub rotate_first: bool,

    /// Use the legacy PDF/EPUB image extraction method from older KCC versions
    #[arg(long = "legacy-extract")]
    pub legacy_extract: bool,

    /// Render vector PDFs to device width instead of height
    #[arg(long = "pdf-width")]
    pub pdf_width: bool,

    /// Delete source files or directories after a successful conversion
    #[arg(short = 'd', long = "delete")]
    pub delete: bool,

    /// Create temporary files on the source file's drive
    #[arg(long = "temp-dir")]
    pub temp_dir: bool,

    /// Create JPEG files using mozjpeg (unsupported; use --jpeg-quality)
    #[arg(long = "mozjpeg")]
    pub mozjpeg: bool,
}

/// Output selection and metadata.
#[derive(Args, Debug, Clone)]
pub struct OutputArgs {
    /// Output directory or file
    #[arg(short = 'o', long = "output", value_hint = ValueHint::AnyPath, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Comic title (default: filename or directory name)
    #[arg(short = 't', long = "title", value_name = "TITLE")]
    pub title: Option<String>,

    /// Write title using embedded metadata: combine with the default schema, or use it only
    #[arg(
        long = "metadata-title",
        value_enum,
        default_value = "0",
        value_name = "default|combine|only"
    )]
    pub metadata_title: MetadataTitle,

    /// Keep any original ComicInfo.xml files
    #[arg(long = "keep-comicinfo")]
    pub keep_comicinfo: bool,

    /// Author name (default: cb)
    #[arg(short = 'a', long = "author", value_name = "AUTHOR")]
    pub author: Option<String>,

    /// EPUB language
    #[arg(long = "language", default_value = "en-US", value_name = "BCP47")]
    pub language: String,

    /// Output format
    #[arg(
        short = 'f',
        long = "format",
        value_enum,
        default_value = "auto",
        value_name = "FORMAT"
    )]
    pub format: Format,

    /// Output EPUB with a `.epub` extension rather than `.kepub.epub`
    #[arg(long = "no-kepub")]
    pub no_kepub: bool,

    /// Output KePub with a single `.kepub` extension rather than `.kepub.epub`
    #[arg(long = "kepub-short-ext")]
    pub kepub_short_ext: bool,

    /// Split output into multiple files: none, automatic, or per subdirectory
    #[arg(
        short = 'b',
        long = "batch-split",
        value_enum,
        default_value = "0",
        value_name = "none|auto|per-subdir"
    )]
    pub batch_split: BatchSplit,

    /// Shift the first page to the opposite side in landscape for spread alignment
    #[arg(long = "spread-shift")]
    pub spread_shift: bool,

    /// Show a single centred page in landscape
    #[arg(long = "one-page-landscape")]
    pub one_page_landscape: bool,

    /// Kindle doc-type tag
    #[arg(
        long = "doc-type",
        value_enum,
        default_value = "none",
        value_name = "ebok|pdoc|none"
    )]
    pub doc_type: DocType,
}

/// Custom screen geometry overriding the selected profile.
#[derive(Args, Debug, Clone)]
pub struct CustomProfileArgs {
    /// Replace the screen width provided by the device profile
    #[arg(long = "custom-width", default_value = "0", value_name = "PX")]
    pub custom_width: u32,

    /// Replace the screen height provided by the device profile
    #[arg(long = "custom-height", default_value = "0", value_name = "PX")]
    pub custom_height: u32,
}
