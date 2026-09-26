//! `comic-book ebook` — comic-to-ebook conversion.
//!
//! This is a clean-room Rust port of KCC's `kcc-c2e` pipeline (see `AGENTS.md`).
//! Phase 0 provides the command-line surface, the device-profile tables, option
//! resolution and progress scaffolding; Phase 1 adds the input adapters
//! ([`input`]) that decode archives/folders into a [`ComicTree`]; Phase 2 adds the
//! per-page image pipeline ([`processing`]) that turns a tree into encoded pages;
//! Phase 3 adds cropping and enhancement; Phase 4 adds metadata resolution
//! ([`metadata`]) and page/chapter naming ([`naming`]); Phase 5 adds the output
//! builders ([`output`]) and makes `-f epub`/`-f kepub` shippable; Phase 6 completes
//! the cover pipeline, the Kindle Scribe `-above`/`-below` split and the panel-view/
//! spread variants; Phase 7 adds `-f cbz`, `-f pdf` and `--light-novel`; Phase 8
//! adds the Kindle output (`-f azw3`/`-f mobi` via `kindling`); Phase 9 adds
//! tome chunking ([`chunk`]), `--file-fusion` and `--delete`; Phase 10 adds
//! `--webtoon` ([`processing::webtoon`]); Phase 11 adds the EPUB (spine-ordered) and PDF
//! (embedded-image/rasterised) input adapters ([`input`]) and KCC's
//! `detectSuboptimalProcessing` warnings; Phase 12 hardens the pipeline against
//! malformed/truncated inputs and verifies it scales to a large book (the
//! `ebook_robustness_tests` suite).
//!
//! # Exit codes
//!
//! [`run_ebook`] returns an `anyhow::Result`. The binary maps any error to exit
//! code `1`; command-line usage errors are reported by `clap` with exit code `2`.

pub mod chunk;
pub mod cli;
pub mod input;
pub mod metadata;
pub mod model;
pub mod naming;
pub mod options;
pub mod output;
pub mod processing;
pub mod profiles;
pub mod progress;

pub use cli::EbookArgs;
pub use metadata::BookMetadata;
pub use model::{Background, Chapter, ComicTree, CoverSource, OrderClass, Page, PageFlags};
pub use naming::Sanitized;
pub use options::{BorderColor, DocType, Format, Options};
pub use profiles::{DeviceKind, Profile, ProfileData};

use anyhow::Result;
use std::path::{Path, PathBuf};

/// A source loaded, its metadata resolved and its names sanitized.
///
/// This is the Rust counterpart of the pre-processing half of KCC's `makeBook`:
/// [`input::load_tree`] decodes the source, [`metadata::resolve`] turns its
/// `ComicInfo.xml` and the CLI overrides into a [`BookMetadata`], and
/// [`naming::sanitize_tree`] renames every chapter directory and page to the
/// deterministic output layout (AGENTS.md §3, §15 Phase 4).
#[derive(Debug, Clone)]
pub struct PreparedBook {
    pub tree: ComicTree,
    pub metadata: BookMetadata,
    /// A sibling `Covers/` image to use instead of the first page, if any
    /// (KCC's `options.customcover`).
    pub cover_override: Option<PathBuf>,
    /// The slugified chapter/page names and the cover page path.
    pub sanitized: Sanitized,
}

/// Load a source, resolve its metadata and sanitize its chapter/page names.
pub fn prepare_book(source: &Path, options: &Options) -> Result<PreparedBook> {
    let tree = input::load_tree(source, options)?;
    let cover_override = naming::select_cover(source);
    Ok(assemble(tree, cover_override, source, options, None, false))
}

/// Run the `ebook` subcommand.
pub fn run_ebook(args: EbookArgs) -> Result<()> {
    let options = Options::resolve(&args)?;

    if options.file_fusion {
        return run_fusion(&options);
    }

    for source in options.inputs.clone() {
        let written = convert_source(&source, &options)?;
        for path in &written {
            println!("Created {}", path.display());
        }

        if options.delete {
            delete_source(&source)?;
        }
    }

    Ok(())
}

/// Convert all inputs as one fused book (`--file-fusion`).
///
/// KCC merges the inputs into a temp directory and converts that once; the port
/// merges them into a [`ComicTree`] ([`input::fusion`]) and runs the same shared
/// pipeline. `--delete` is not honoured here, matching the reference, whose fused
/// run deletes only its own scratch tree, never the user's sources.
fn run_fusion(options: &Options) -> Result<()> {
    let fused = input::fusion::build(&options.inputs, options)?;

    // KCC defaults a fused run's output directory to the first source's directory
    // (`options.output = fusion_source_parent`).
    let mut fusion_options = options.clone();
    if fusion_options.output.is_none() {
        fusion_options.output = Some(fused.output_dir.clone());
    }

    let prepared = assemble(
        fused.tree,
        fused.cover,
        &fused.source,
        &fusion_options,
        Some(&fused.title),
        true,
    );
    let written = convert_prepared(prepared, &fused.source, &fusion_options)?;
    for path in &written {
        println!("Created {}", path.display());
    }

    Ok(())
}

/// Run the full pipeline for one source, returning the output paths.
///
/// This is KCC's `makeBook` without the per-source loop: load, resolve metadata,
/// sanitize names, build the cover, process the images and write the output.
/// `--light-novel` diverges before any of that — KCC's light-novel branch runs
/// right after extraction and never touches metadata, naming or the cover
/// (AGENTS.md §12.3), so it is dispatched to [`output::lightnovel`] wholesale.
pub fn convert_source(source: &Path, options: &Options) -> Result<Vec<PathBuf>> {
    if options.light_novel {
        return output::lightnovel::convert(source, options);
    }

    let tree = input::load_tree(source, options)?;
    let cover_override = naming::select_cover(source);
    let prepared = assemble(tree, cover_override, source, options, None, false);
    convert_prepared(prepared, source, options)
}

/// Resolve a tree's metadata, sanitize its names and build the [`PreparedBook`].
///
/// `default_title` overrides the title derived from `source` (used by fusion,
/// where the source is a synthetic `<name> [fused]` directory); `fusion` strips
/// the `fusion_NNNN_` ordering prefix from the navigation titles.
fn assemble(
    mut tree: ComicTree,
    cover_override: Option<PathBuf>,
    source: &Path,
    options: &Options,
    default_title: Option<&str>,
    fusion: bool,
) -> PreparedBook {
    let metadata = metadata::resolve_with(&tree, source, options, default_title);
    // KCC warns about a likely-degraded conversion after the tree is extracted but
    // before it is renamed (`detectSuboptimalProcessing`).
    for warning in processing::detect_suboptimal_processing(&tree, options) {
        progress::warn(&warning);
    }
    let mut sanitized = naming::sanitize_tree(&mut tree, options);
    if fusion {
        for title in sanitized.chapter_titles.values_mut() {
            *title = naming::strip_fusion_prefix(title);
        }
    }
    PreparedBook {
        tree,
        metadata,
        cover_override,
        sanitized,
    }
}

/// Process a prepared book and write its output(s).
///
/// The cover is processed from the source before the per-page pass mutates the
/// first page (cropping), exactly as `makeBook` builds the `Cover` first. In
/// webtoon mode without a custom cover KCC builds no cover at all, so the cover
/// step is skipped; the tree is then merged and panel-split before processing.
fn convert_prepared(
    mut prepared: PreparedBook,
    source: &Path,
    options: &Options,
) -> Result<Vec<PathBuf>> {
    let cover = if options.webtoon && prepared.cover_override.is_none() {
        None
    } else {
        processing::cover::process(&prepared.tree, prepared.cover_override.as_deref(), options)?
    };
    if options.webtoon {
        processing::webtoon::transform(&mut prepared.tree, options)?;
    }
    let mut processed = processing::process_tree(&mut prepared.tree, options)?;
    if let Some(cover) = cover {
        processed.cover = Some(cover.page);
        processed.cover_smart_crop = cover.smart_cropped;
    }
    output::write_book(processed, &prepared, source, options)
}

/// Remove a source after a successful conversion (`-d/--delete`).
fn delete_source(source: &Path) -> Result<()> {
    if source.is_dir() {
        crate::clamp::remove_dir_all_force(source)?;
    } else if source.is_file() {
        std::fs::remove_file(source)?;
    }
    Ok(())
}
