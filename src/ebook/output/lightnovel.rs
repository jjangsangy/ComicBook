//! Light-novel mode — KCC's `--lightnovel`.
//!
//! `--light-novel` all but skips the comic pipeline (see docs/output.md): only the
//! pages that exceed the profile are grayscaled and downscaled, everything else is
//! copied byte-for-byte, and the whole book is repackaged as a CBZ with its
//! **original** file structure. KCC runs this branch before
//! `getMetadata`/`sanitizeTree`/`imgDirectoryProcessing`, so pages keep their
//! source names (no chapter slugs and no `cb-NNNN` renumbering), no cover is
//! processed and `--no-processing` is irrelevant — the resize still happens.
//!
//! Two deliberate deviations, both because the in-memory [`ComicTree`] only carries
//! images (see docs/architecture.md): non-image entries other than `ComicInfo.xml` are not
//! reproduced, and KCC's `RGB → L` / `RGBA → LA` conversion becomes `RGB/RGBA → L`
//! (alpha is dropped; comic scans are effectively opaque).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::DynamicImage;
use rayon::prelude::*;

use crate::archive::{ArchiveKind, ArchiveWriter, EntryContent};
use crate::ebook::input;
use crate::ebook::model::{Page, SourceName};
use crate::ebook::options::{Options, ReaderFamily};
use crate::ebook::processing::color::to_luma601;
use crate::ebook::processing::page::{self, Method};
use crate::ebook::{naming, progress};
use crate::units::Size;

/// KCC caps both light-novel dimensions at 1920 for Kindle profiles.
const KINDLE_MAX_DIMENSION: u32 = 1920;

/// Convert `source` in light-novel mode, returning the output path(s).
///
/// This is the single-file entry point; it reports through a standalone
/// [`progress::Reporter`]. Batch callers use [`convert_with`].
pub fn convert(source: &Path, options: &Options) -> Result<Vec<PathBuf>> {
    convert_with(source, options, &progress::Reporter::standalone())
}

/// [`convert`] with progress reported through `reporter`.
pub fn convert_with(
    source: &Path,
    options: &Options,
    reporter: &progress::Reporter,
) -> Result<Vec<PathBuf>> {
    let mut tree = input::load_tree(source, options)?;
    let bounds = resize_bounds(options);

    // Parallelise within each chapter and collect in reading order, matching the
    // `os.walk` order KCC's `makeZIP` reproduces. Pages are mutated so each one's
    // pixels are released as soon as it has been resized.
    let bar = reporter.child(tree.page_count() as u64, "Resizing images");
    let mut entries: Vec<(SourceName, Vec<u8>)> = Vec::new();
    for chapter in &mut tree.chapters {
        let resized = chapter
            .pages
            .par_iter_mut()
            .map(|page| {
                let name = page.source_name.clone();
                let result = resize_page(page, bounds, options).map(|bytes| (name, bytes));
                bar.inc(1);
                result
            })
            .collect::<Result<Vec<_>>>()?;
        entries.extend(resized);
    }
    bar.finish_and_clear();

    // KCC's `if ext != '.epub'` guard only preserves `.epub` for an image-less
    // source; every source we can load has at least one page, so the output is
    // always a CBZ.
    let dest = naming::output_filename(
        source,
        options.output.destination.as_deref(),
        ".cbz",
        "",
        options,
    );
    write_cbz(&dest, &entries, tree.comicinfo.as_deref())?;
    Ok(vec![dest])
}

/// The profile geometry a light-novel page is bounded by, Kindle-capped as in KCC.
///
/// KCC reads the profile table here (which `checkOptions` replaces with the
/// `Custom` entry when `--custom-width`/`--custom-height` were given), so
/// `device.data` is the right source.
fn resize_bounds(options: &Options) -> Size {
    let mut size = options.device_size();
    if options.device.reader == ReaderFamily::Kindle {
        size.width = size.width.min(KINDLE_MAX_DIMENSION);
        size.height = size.height.min(KINDLE_MAX_DIMENSION);
    }
    size
}

/// Resize one page if it exceeds the bounds, returning the bytes to archive.
///
/// A page that already fits is emitted untouched (KCC only calls `img.save` inside
/// its size check, so the file on disk is left as it was); such a page is never
/// decoded.
fn resize_page(page: &mut Page, bounds: Size, options: &Options) -> Result<Vec<u8>> {
    let size = page.dimensions();
    if size.width <= bounds.width && size.height <= bounds.height {
        // The tree is dropped once every page has been archived, so the source
        // bytes can be *moved* into the output instead of cloned.
        if let Some(raw) = page.take_source() {
            return Ok(raw);
        }
    }

    page.ensure_decoded()?;
    // Captured before the pixels are taken: a pixel-only page carries its media
    // type in the payload, which `take_image` moves away.
    let media_type = page
        .media_type()
        .context("light-novel page has no media type")?;
    let mut image = page
        .take_image()
        .context("light-novel page has no decoded image")?;
    if !options.processing.color.force_color {
        image = grayscale(image);
    }
    let image = page::contain(&image, bounds, Method::Bicubic)?;
    page::encode_dynamic(&image, media_type, options.processing.jpeg_quality)
}

/// KCC's light-novel colour conversion (`RGB → L`, `RGBA → LA`; see module docs).
fn grayscale(image: DynamicImage) -> DynamicImage {
    match image {
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => {
            DynamicImage::ImageLuma8(to_luma601(&image))
        }
        other => other,
    }
}

/// Write the (mostly untouched) pages as a CBZ, preserving their source paths.
fn write_cbz(
    dest: &Path,
    entries: &[(SourceName, Vec<u8>)],
    comicinfo: Option<&[u8]>,
) -> Result<()> {
    let mut writer =
        ArchiveWriter::new(ArchiveKind::Cbz, dest).context("Failed to create the CBZ archive")?;

    // KCC's light-novel branch never calls `removeNonImages`, so a discovered
    // `ComicInfo.xml` survives into the output.
    if let Some(xml) = comicinfo {
        writer.add_entry("ComicInfo.xml", EntryContent::File(xml))?;
    }
    for (name, bytes) in entries {
        writer
            .add_entry(name.as_str(), EntryContent::File(bytes))
            .with_context(|| format!("Failed to add {name} to the CBZ"))?;
    }

    writer.finish()?;
    Ok(())
}
