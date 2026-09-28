//! Webtoon merge and panel splitting — KCC's `comic2panel` (see docs/processing.md).
//!
//! In `--webtoon` mode KCC collapses every chapter directory into a single tall
//! strip (at the chapter's most common width), then cuts that strip into "virtual
//! pages" at the device geometry by detecting the empty gutters between panels.
//! The per-page pipeline then runs on those virtual pages exactly as it does for a
//! normal book (webtoon mode makes [`super::page::split_check`] a passthrough, so it
//! never re-splits them).
//!
//! [`transform`] is the in-memory counterpart of KCC's `comic2panel.main(['-y', '-x',
//! '-i', '-m', …])` invocation: [`merge_chapter`] merges a chapter's pages and
//! [`split_chapter`] replaces them with the virtual pages. It runs *after*
//! `sanitize_tree` (as the reference does), so the merged strip inherits the first
//! page's sanitized `kcc-NNNN` name and the virtual pages are `kcc-NNNN-MMMM`.
//!
//! The edge filter is `imageproc`'s 3x3 convolution (see docs/dependencies.md); it reproduces
//! Pillow's `FIND_EDGES` except at the 1px border ring, which Pillow copies from the
//! source and we restore explicitly (the reference's `Image.filter` leaves borders
//! untouched). Everything else — the panel scan, the overlap split and the packing —
//! is KCC's own heuristic, reproduced from the documented behaviour.

use anyhow::{bail, Context, Result};
use image::{DynamicImage, GenericImageView, GrayImage, Luma, Rgb, RgbImage};
use imageproc::filter::filter;
use imageproc::kernel::Kernel;

use crate::ebook::model::{Background, ComicTree, MediaType, Page, PageData, RelPath, SourceName};
use crate::ebook::options::Options;
use crate::ebook::processing::color::to_luma601;
use crate::ebook::processing::kernels;
use crate::ebook::processing::page::{self, Method};
use crate::path_text;
use crate::units::{Pixels, Size};

/// The reference caps the virtual page width at 1072 px (`max_width`), regardless of
/// how wide the device is.
const MAX_VIRTUAL_WIDTH: u32 = 1072;
/// A panel up to `1.5 ×` the virtual height is kept whole; up to `2 ×` it is split
/// into two overlapping halves; beyond that it is split into as many parts as needed.
const PANEL_KEEP_RATIO: f64 = 1.5;
const PANEL_TWO_HALVES_RATIO: f64 = 2.0;
/// A virtual page shorter than this is dropped (KCC's `pageHeight > 15`).
const MIN_PAGE_HEIGHT: u32 = 15;
/// The reference refuses to split a strip narrower than 300 px.
const MIN_STRIP_WIDTH: u32 = 300;
/// A white/black run becomes an "edge" when the Laplacian exceeds this (KCC's `p > 6`).
const EDGE_THRESHOLD: u8 = 6;
/// The reference aborts when a merged strip would exceed `131072 * 4` pixels tall.
const MAX_MERGED_HEIGHT: Pixels = Pixels::new(131_072 * 4);

/// A detected panel `[top, bottom)` within the merged strip.
///
/// `height == bottom - top`; deriving it removes a third tuple field that could
/// drift from the stored edges.
#[derive(Clone, Copy)]
struct Panel {
    top: u32,
    bottom: u32,
}

impl Panel {
    /// The panel's height in pixels.
    ///
    /// Saturating so an inverted span yields `0` rather than panicking (see the
    /// refactor's panic-free rule).
    fn height(&self) -> u32 {
        self.bottom.saturating_sub(self.top)
    }
}

/// Merge every chapter into a strip and split it into virtual pages, in place.
pub fn transform(tree: &mut ComicTree, options: &Options) -> Result<()> {
    for chapter in &mut tree.chapters {
        if chapter.pages.is_empty() {
            continue;
        }
        let merged = merge_chapter(&mut chapter.pages)?;
        // The merged strip is written back under the first page's sanitized stem
        // (`os.path.splitext(first)[0]`), then saved as PNG; the virtual pages keep
        // that stem and get a `-NNNN` suffix.
        let stem = path_text::stem(chapter.pages[0].source_name.as_str()).to_string();
        chapter.pages = split_chapter(merged, &stem, options)?;
    }
    Ok(())
}

/// Merge a chapter's pages vertically into one RGB strip (KCC's `mergeDirectory`).
///
/// Every page is widened to the chapter's most common width with a bicubic
/// `ImageOps.fit`; the canvas is sized from the pages' *original* heights, so a page
/// widened by the fit is clipped at the bottom — a reference quirk that is
/// reproduced deliberately (see docs/architecture.md).
///
/// Each page's pixels are decoded on demand and released as soon as it has been
/// blitted, so only the source bytes plus the growing canvas are retained.
fn merge_chapter(pages: &mut [Page]) -> Result<DynamicImage> {
    let target_width = most_common_width(pages);
    let target_height: u32 = pages.iter().map(|page| page.dimensions().height).sum();
    if Pixels::new(u64::from(target_height)) > MAX_MERGED_HEIGHT {
        bail!(
            "Webtoon strip is too tall at {target_height} px ({target_width} px wide); \
             try separate chapter folders or --file-fusion"
        );
    }

    let mut canvas = RgbImage::from_pixel(target_width, target_height, Rgb([0, 0, 0]));
    let mut y: i64 = 0;
    for page in pages.iter_mut() {
        page.ensure_decoded()?;
        let image = page
            .take_image()
            .context("webtoon page has no decoded image")?;
        let rgb = match image {
            DynamicImage::ImageRgb8(buffer) => buffer,
            other => other.to_rgb8(),
        };
        let resized = if rgb.width() != target_width {
            let height = (f64::from(rgb.height())
                * (f64::from(target_width) / f64::from(rgb.width())))
                as u32;
            page::fit(
                &DynamicImage::ImageRgb8(rgb),
                Size::new(target_width, height),
                Method::Bicubic,
            )?
        } else {
            DynamicImage::ImageRgb8(rgb)
        };
        // `fit` preserves the RGB8 type, so this converts nothing.
        let resized = resized.into_rgb8();
        image::imageops::replace(&mut canvas, &resized, 0, y);
        y += i64::from(resized.height());
    }
    Ok(DynamicImage::ImageRgb8(canvas))
}

/// The chapter's most common page width (KCC's `max(set(sizes), key=sizes.count)`).
///
/// Ties are broken by the first page that reached the winning count, which is
/// deterministic; the reference's `set` iteration order there is implementation
/// defined (see docs/architecture.md).
fn most_common_width(pages: &[Page]) -> u32 {
    let mut best = 0;
    let mut best_count = 0;
    for (index, page) in pages.iter().enumerate() {
        let width = page.dimensions().width;
        if pages[..index]
            .iter()
            .any(|earlier| earlier.dimensions().width == width)
        {
            continue;
        }
        let count = pages
            .iter()
            .filter(|candidate| candidate.dimensions().width == width)
            .count();
        if count > best_count {
            best_count = count;
            best = width;
        }
    }
    best
}

/// Split a merged strip into virtual pages (KCC's `splitImage`).
///
/// Takes the strip by value so the short-strip path can move it into its single
/// page instead of cloning the whole (potentially huge) buffer.
fn split_chapter(merged: DynamicImage, stem: &str, options: &Options) -> Result<Vec<Page>> {
    let (width, height) = merged.dimensions();
    if height <= options.device.data.height {
        // Shorter than the device: the strip is used as a single page (`<stem>.png`).
        return Ok(vec![page_from(
            merged,
            SourceName::new(format!("{stem}.png")),
        )]);
    }
    let Some(strip_width) = StripWidth::new(width) else {
        bail!(
            "Webtoon strip is only {width} px wide (needs at least {MIN_STRIP_WIDTH} px); \
             try the legacy extract option"
        );
    };

    let panels = detect_panels(&merged, strip_width);
    let virtual_height = virtual_height(width, options);
    let split = split_panels(&panels, virtual_height);
    let units = pack_pages(&split, virtual_height);

    let mut pages = Vec::new();
    let mut number = 1;
    for unit in &units {
        let page_height: u32 = unit.iter().map(|&index| split[index].height()).sum();
        if page_height <= MIN_PAGE_HEIGHT {
            continue;
        }
        let mut canvas = RgbImage::new(width, page_height);
        let mut target_y = 0i64;
        for &index in unit {
            let panel = split[index];
            let crop = merged
                .crop_imm(0, panel.top, width, panel.height())
                .into_rgb8();
            image::imageops::replace(&mut canvas, &crop, 0, target_y);
            target_y += i64::from(panel.height());
        }
        let name = format!("{stem}-{number:04}.png");
        pages.push(page_from(
            DynamicImage::ImageRgb8(canvas),
            SourceName::new(name),
        ));
        number += 1;
    }
    Ok(pages)
}

/// A [`Page`] wrapping a webtoon strip or virtual page.
///
/// The strip is RGB and reported as PNG so `--no-processing` emits the merged/split
/// PNGs the reference would have packaged (`imgDirectoryProcessing` is skipped there);
/// the normal path re-encodes the pixels through the per-page pipeline.
fn page_from(image: DynamicImage, source_name: SourceName) -> Page {
    let rel_path = RelPath::new(path_text::file_name(source_name.as_str()));
    let dimensions = Size::from_dimensions(image.dimensions());
    Page {
        source_name,
        rel_path,
        data: PageData::Pixels(MediaType::Png, image),
        dimensions,
        background: Background::White,
    }
}

/// The effective virtual page height for a strip `strip_width` px wide.
///
/// KCC clamps the target width to [`MAX_VIRTUAL_WIDTH`] and scales the device height
/// by the *virtual* width; when the device is wider than the cap it divides by the cap
/// instead of the device width.
fn virtual_height(strip_width: u32, options: &Options) -> u32 {
    let device = &options.device.data;
    let virtual_width = MAX_VIRTUAL_WIDTH.min(device.width).min(strip_width);
    let base = if device.width > MAX_VIRTUAL_WIDTH {
        f64::from(device.height) / f64::from(MAX_VIRTUAL_WIDTH)
    } else {
        f64::from(device.height) / f64::from(device.width)
    };
    (base * f64::from(virtual_width)) as u32
}

/// A merged strip's width, validated wide enough for the panel scan.
///
/// The scan advances [`StripWidth::step`] rows per iteration; a strip narrow enough
/// for that step to round to zero would spin forever. Building the width once, through
/// [`StripWidth::new`], makes that state unrepresentable instead of re-guarding it at
/// the loop (REFACTOR.md C11).
#[derive(Clone, Copy)]
struct StripWidth(u32);

impl StripWidth {
    /// The only constructor: `None` when `width` is too narrow to scan safely
    /// (narrower than [`MIN_STRIP_WIDTH`]).
    fn new(width: u32) -> Option<Self> {
        (width >= MIN_STRIP_WIDTH).then_some(Self(width))
    }

    /// The strip width in pixels.
    fn get(self) -> u32 {
        self.0
    }

    /// `width / 80`, rounded up to even: the band height the scan samples.
    fn v_pad(self) -> u32 {
        let pad = self.0 / 80;
        if pad % 2 == 1 {
            pad + 1
        } else {
            pad
        }
    }

    /// `width / 20`: the horizontal padding excluded from each band.
    fn h_pad(self) -> u32 {
        self.0 / 20
    }

    /// The rows the scan advances per iteration. Non-zero by construction: `new`
    /// requires at least [`MIN_STRIP_WIDTH`], whose `v_pad` rounds up to `4`.
    fn step(self) -> u32 {
        self.v_pad() / 2
    }
}

/// Find the panels in a merged strip (KCC's panel scan).
///
/// The strip is thresholded into an edge map, then scanned top to bottom in
/// `v_pad / 2` steps; a band that contains no edge is "solid". A panel is a maximal
/// run of non-solid bands, and a short panel that starts in the first `2 * v_pad`
/// rows is discarded. The width comes from a [`StripWidth`], so the scan's step is
/// known non-zero without a guard.
fn detect_panels(image: &DynamicImage, strip_width: StripWidth) -> Vec<Panel> {
    let width = strip_width.get();
    let (_, height) = image.dimensions();
    let h_pad = strip_width.h_pad();
    let v_pad = strip_width.v_pad();
    let mask = edge_mask(image);

    let mut panels = Vec::new();
    let mut y_work = 0u32;
    // The open panel's top, or `None` between panels. One `Option` replaces the old
    // `panel_detected`/`panel_top` pair, which had to be mutated in lockstep and could
    // drift (REFACTOR.md C11).
    let mut open_panel: Option<u32> = None;
    let step = strip_width.step();

    while y_work < height {
        let solid = band_is_solid(&mask, h_pad, y_work, width - h_pad, y_work + v_pad, height);

        if !solid && open_panel.is_none() {
            open_panel = Some(y_work);
        }

        // The bottom edge: close a panel that runs off the end of the strip.
        if height - y_work <= v_pad / 2 && !solid {
            if let Some(top) = open_panel.take() {
                panels.push(Panel {
                    top,
                    bottom: height,
                });
            }
        }

        if solid {
            if let Some(top) = open_panel.take() {
                let panel = Panel {
                    top,
                    bottom: y_work,
                };
                // Skip a short panel hugging the top of the strip.
                if panel.top < v_pad * 2 && panel.height() < v_pad * 2 {
                    // dropped
                } else {
                    panels.push(panel);
                }
            }
        }

        y_work += step;
    }

    panels
}

/// Whether the horizontal band `[x0, x1) × [y0, y1)` of the edge map is uniform.
///
/// Pillow's `Image.crop` zero-fills below the strip, and `detectSolid` reads a
/// uniformly-black band as solid, so a band that overhangs the bottom is black-padded.
fn band_is_solid(mask: &GrayImage, x0: u32, y0: u32, x1: u32, y1: u32, height: u32) -> bool {
    let band = kernels::band_white_black(mask, x0, y0, x1, y1, height);
    !band.has_white || !band.has_black
}

/// The thresholded edge map of a strip (KCC's `FIND_EDGES` + `point(p > 6)`).
fn edge_mask(image: &DynamicImage) -> GrayImage {
    let gray = to_luma601(image);
    let (width, height) = gray.dimensions();

    // Pillow's `FIND_EDGES` is a 3x3 Laplacian; `imageproc` supplies the convolution
    // (see docs/dependencies.md), and the result is clamped to `[0, 255]` as Pillow does.
    let kernel = Kernel::new(&[-1i32, -1, -1, -1, 8, -1, -1, -1, -1], 3, 3);
    let filtered: image::ImageBuffer<Luma<i16>, Vec<i16>> =
        filter(&gray, kernel, |value| value.clamp(0, 255) as i16);
    let mut luma = GrayImage::from_fn(width, height, |x, y| {
        Luma([filtered.get_pixel(x, y)[0].clamp(0, 255) as u8])
    });

    // Pillow's convolution leaves the 1px border ring unchanged; `imageproc` pads by
    // continuity, so restore the source values there for parity.
    if width > 0 && height > 0 {
        for x in 0..width {
            luma.put_pixel(x, 0, *gray.get_pixel(x, 0));
            luma.put_pixel(x, height - 1, *gray.get_pixel(x, height - 1));
        }
        for y in 0..height {
            luma.put_pixel(0, y, *gray.get_pixel(0, y));
            luma.put_pixel(width - 1, y, *gray.get_pixel(width - 1, y));
        }
    }

    // Threshold in place: `imageproc::contrast::threshold` would clone the map.
    kernels::threshold_in_place(&mut luma, EDGE_THRESHOLD, kernels::ThresholdKind::Above);
    luma
}

/// Split over-long panels, with overlap, until each is at most `virtual_height`.
fn split_panels(panels: &[Panel], virtual_height: u32) -> Vec<Panel> {
    let vh = f64::from(virtual_height);
    let mut out = Vec::new();
    for &panel in panels {
        let (top, bottom) = (panel.top, panel.bottom);
        let h = f64::from(panel.height());
        if h <= vh * PANEL_KEEP_RATIO {
            out.push(panel);
        } else if h <= vh * PANEL_TWO_HALVES_RATIO {
            let diff = panel.height() - virtual_height;
            out.push(Panel {
                top,
                bottom: bottom - diff,
            });
            out.push(Panel {
                top: bottom - virtual_height,
                bottom,
            });
        } else {
            let parts = (h / vh).ceil() as u32;
            let diff = panel.height() / parts;
            out.push(Panel {
                top,
                bottom: top + virtual_height,
            });
            for part in 1..parts.saturating_sub(1) {
                let start = top + part * diff;
                out.push(Panel {
                    top: start,
                    bottom: start + virtual_height,
                });
            }
            out.push(Panel {
                top: bottom - virtual_height,
                bottom,
            });
        }
    }
    out
}

/// Pack split panels into virtual pages, filling each up to `virtual_height`.
fn pack_pages(panels: &[Panel], virtual_height: u32) -> Vec<Vec<usize>> {
    let mut pages = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut page_left = i64::from(virtual_height);
    for (number, panel) in panels.iter().enumerate() {
        let height = i64::from(panel.height());
        if page_left - height > 0 {
            page_left -= height;
            current.push(number);
        } else {
            if !current.is_empty() {
                pages.push(std::mem::take(&mut current));
            }
            page_left = i64::from(virtual_height) - height;
            current.push(number);
        }
    }
    if !current.is_empty() {
        pages.push(current);
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use image::RgbImage;

    /// Resolve options from a `comic-book ebook` command line.
    fn options(args: &[&str]) -> Result<Options> {
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = crate::cli::Cli::try_parse_from(full)?;
        match cli.command {
            crate::cli::Commands::Ebook(args) => Options::resolve(&args),
            _ => bail!("expected the ebook subcommand"),
        }
    }

    /// A checkerboard-banded strip, matching the reference fixture generator.
    fn checker_strip(width: u32, segments: &[(u32, u32)]) -> DynamicImage {
        let total: u32 = segments.iter().map(|(height, _)| height).sum();
        let mut image = RgbImage::from_pixel(width, total, Rgb([255, 255, 255]));
        let mut y = 0;
        for &(height, content) in segments {
            if content > 0 {
                let top = y + (height - content) / 2;
                for py in top..top + content {
                    for px in 40..width.saturating_sub(40) {
                        let dark = ((px / 6) + (py / 6)) % 2 == 0;
                        image.put_pixel(
                            px,
                            py,
                            if dark {
                                Rgb([0, 0, 0])
                            } else {
                                Rgb([255, 255, 255])
                            },
                        );
                    }
                }
            }
            y += height;
        }
        DynamicImage::ImageRgb8(image)
    }

    fn strip_page(name: &str, image: DynamicImage) -> Page {
        let dimensions = Size::from_dimensions(image.dimensions());
        Page {
            source_name: SourceName::new(name),
            rel_path: RelPath::new(name),
            data: PageData::Pixels(MediaType::Png, image),
            dimensions,
            background: Background::White,
        }
    }

    fn split_sizes(pages: &[Page]) -> Vec<(u32, u32)> {
        pages
            .iter()
            .map(|page| page.dimensions().to_dimensions())
            .collect()
    }

    /// The three-panel fixture: pages pack into two virtual pages (KCC: 780 + 525).
    const SEGMENTS_A: &[(u32, u32)] = &[
        (100, 0),
        (500, 400),
        (200, 0),
        (450, 350),
        (200, 0),
        (600, 500),
        (100, 0),
    ];

    #[test]
    fn short_strip_is_one_page() -> Result<()> {
        let page = strip_page(
            "kcc-0001.png",
            checker_strip(800, &[(100, 0), (300, 200), (100, 0)]),
        );
        let merged = merge_chapter(&mut [page])?;
        let pages = split_chapter(merged, "kcc-0001", &options(&["-p", "KV"])?)?;
        assert_eq!(split_sizes(&pages), vec![(800, 500)]);
        assert_eq!(pages[0].source_name, "kcc-0001.png");
        Ok(())
    }

    #[test]
    fn three_panels_pack_into_two_virtual_pages() -> Result<()> {
        let page = strip_page("kcc-0001.png", checker_strip(800, SEGMENTS_A));
        let merged = merge_chapter(&mut [page])?;
        assert_eq!(merged.dimensions(), (800, 2150));
        let pages = split_chapter(merged, "kcc-0001", &options(&["-p", "KV"])?)?;
        assert_eq!(split_sizes(&pages), vec![(800, 780), (800, 525)]);
        assert_eq!(pages[0].source_name, "kcc-0001-0001.png");
        assert_eq!(pages[1].source_name, "kcc-0001-0002.png");
        Ok(())
    }

    #[test]
    #[ignore = "slow: long webtoon panel; run with --run-ignored"]
    fn a_super_long_panel_splits_with_overlap() -> Result<()> {
        let page = strip_page(
            "kcc-0001.png",
            checker_strip(800, &[(100, 0), (2600, 2500), (100, 0)]),
        );
        let merged = merge_chapter(&mut [page])?;
        let pages = split_chapter(merged, "kcc-0001", &options(&["-p", "KV"])?)?;
        // The KV profile (1072x1448) gives a virtual height of 1080, so the 2500px
        // panel becomes three 1080px parts.
        assert_eq!(
            split_sizes(&pages),
            vec![(800, 1080), (800, 1080), (800, 1080)]
        );
        assert_eq!(pages.len(), 3);
        Ok(())
    }

    #[test]
    #[ignore = "slow: webtoon virtual-page packing; run with --run-ignored"]
    fn wider_devices_use_the_1072_cap_for_the_virtual_height() -> Result<()> {
        let page = strip_page(
            "kcc-0001.png",
            checker_strip(800, &[(100, 0), (2600, 2500), (100, 0)]),
        );
        let merged = merge_chapter(&mut [page])?;
        // KO is 1264px wide, so the virtual height is 1680 / 1072 * 800 = 1253.
        let pages = split_chapter(merged, "kcc-0001", &options(&["-p", "KO"])?)?;
        assert_eq!(
            split_sizes(&pages),
            vec![(800, 1253), (800, 1253), (800, 1253)]
        );

        let ko = options(&["-p", "KO"])?;
        assert_eq!(virtual_height(800, &ko), 1253);
        let kv = options(&["-p", "KV"])?;
        assert_eq!(virtual_height(800, &kv), 1080);
        Ok(())
    }

    #[test]
    fn mixed_widths_are_fitted_to_the_most_common_width() -> Result<()> {
        // 800, 600, 800: the middle page is widened to 800, but the canvas is sized
        // from the original heights, so the last page is clipped (KCC's `mergeDirectory`).
        let mut pages = [
            strip_page(
                "kcc-0001.png",
                checker_strip(800, &[(100, 0), (300, 200), (100, 0)]),
            ),
            strip_page(
                "kcc-0002.png",
                checker_strip(600, &[(100, 0), (300, 200), (100, 0)]),
            ),
            strip_page(
                "kcc-0003.png",
                checker_strip(800, &[(100, 0), (300, 200), (100, 0)]),
            ),
        ];
        let merged = merge_chapter(&mut pages)?;
        assert_eq!(merged.dimensions(), (800, 1500));
        Ok(())
    }

    #[test]
    fn transform_replaces_each_chapter_with_merged_pages() -> Result<()> {
        use crate::ebook::model::{Chapter, ChapterName};

        let tree = ComicTree {
            chapters: vec![
                Chapter {
                    name: ChapterName::root(),
                    pages: Vec::new(),
                },
                Chapter {
                    name: ChapterName::new("Chapter 1"),
                    pages: vec![
                        strip_page("Chapter 1/kcc-0001.png", checker_strip(800, SEGMENTS_A)),
                        strip_page(
                            "Chapter 1/kcc-0002.png",
                            checker_strip(800, &[(200, 0), (100, 50), (200, 0)]),
                        ),
                    ],
                },
            ],
            comicinfo: None,
        };

        let mut tree = tree;
        transform(&mut tree, &options(&["-p", "KV"])?)?;
        assert!(tree.chapters[0].pages.is_empty());
        assert_eq!(tree.chapters[1].name.as_str(), "Chapter 1");
        // The merge joins both pages (2150 + 500 = 2650px tall), then the panels are
        // re-split; names keep the chapter directory and the first page's stem.
        assert_eq!(
            split_sizes(&tree.chapters[1].pages),
            vec![(800, 780), (800, 590)]
        );
        let names: Vec<&str> = tree.chapters[1]
            .pages
            .iter()
            .map(|page| page.source_name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["Chapter 1/kcc-0001-0001.png", "Chapter 1/kcc-0001-0002.png",]
        );
        Ok(())
    }

    #[test]
    fn strip_width_rejects_too_narrow_strips_and_keeps_the_step_non_zero() {
        // `new` is the gate: below `MIN_STRIP_WIDTH` it yields `None`, and at the
        // boundary the scan step is guaranteed non-zero so the loop always advances
        // (REFACTOR.md C11).
        assert!(StripWidth::new(MIN_STRIP_WIDTH - 1).is_none());
        assert!(StripWidth::new(MIN_STRIP_WIDTH).is_some_and(|width| width.step() > 0));
    }
}
