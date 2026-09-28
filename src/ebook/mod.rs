//! `comic-book ebook` — comic-to-ebook conversion.
//!
//! This is a clean-room Rust port of KCC's `kcc-c2e` pipeline; the design and the
//! behaviour it reproduces are documented under `docs/` (see `docs/architecture.md`).
//! Input adapters ([`input`]) decode archives, folders, EPUBs and PDFs into a
//! [`ComicTree`]; [`processing`] (including [`processing::webtoon`]) turns that
//! tree into encoded pages; [`metadata`] and [`naming`] resolve metadata and the
//! output layout; [`output`] writes EPUB/KePub, CBZ, PDF, light-novel or Kindle
//! files, splitting into tomes via [`chunk`].
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
pub use model::{
    Background, Chapter, ChapterName, ComicTree, OrderClass, Orientation, Page, PageData,
    PageFlags, PageName, RelPath, ScribeHalf, Source, SourceName,
};
pub use naming::Sanitized;
pub use options::{BorderColor, DocType, Format, Layout, Options};
pub use profiles::{DeviceKind, Profile, ProfileData};

use anyhow::Result;
use std::path::{Path, PathBuf};

/// A source loaded, its metadata resolved and its names sanitized.
///
/// This is the Rust counterpart of the pre-processing half of KCC's `makeBook`:
/// [`input::load_tree`] decodes the source, [`metadata::resolve`] turns its
/// `ComicInfo.xml` and the CLI overrides into a [`BookMetadata`], and
/// [`naming::sanitize_tree`] renames every chapter directory and page to the
/// deterministic output layout (see docs/architecture.md).
#[derive(Debug)]
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
    Ok(assemble(
        tree,
        cover_override,
        source,
        options,
        TitleOrigin::Derived,
        &progress::Reporter::standalone(),
    ))
}

/// Run the `ebook` subcommand.
pub fn run_ebook(args: EbookArgs) -> Result<()> {
    let options = Options::resolve(&args)?;

    if options.main.file_fusion {
        return run_fusion(&options);
    }

    // With several inputs, show an overall bar above the per-file bars. A single
    // input's per-file bar already is its overall progress, so it gets no extra bar.
    let reporter = if options.inputs.len() > 1 {
        progress::Reporter::batch(options.inputs.len() as u64, "Overall Progress")
    } else {
        progress::Reporter::standalone()
    };

    for source in options.inputs.clone() {
        let written = convert_source_with(&source, &options, &reporter)?;
        for path in &written {
            reporter.println(format!("Created {}", path.display()));
        }
        reporter.inc();

        if options.session.delete {
            delete_source(&source)?;
        }
    }
    reporter.finish();

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
    if fusion_options.output.destination.is_none() {
        fusion_options.output.destination = Some(fused.output_dir.clone());
    }

    // The synthetic source path KCC converts (`<first name> [fused]`); the title is
    // the single source of truth (see [`TitleOrigin::Fusion`]).
    let source = fused.output_dir.join(&fused.title);
    let reporter = progress::Reporter::standalone();
    let prepared = assemble(
        fused.tree,
        fused.cover,
        &source,
        &fusion_options,
        TitleOrigin::Fusion(fused.title.as_str()),
        &reporter,
    );
    let written = convert_prepared(prepared, &source, &fusion_options, &reporter)?;
    for path in &written {
        reporter.println(format!("Created {}", path.display()));
    }

    Ok(())
}

/// Run the full pipeline for one source, returning the output paths.
///
/// This is KCC's `makeBook` without the per-source loop: load, resolve metadata,
/// sanitize names, build the cover, process the images and write the output.
/// `--light-novel` diverges before any of that — KCC's light-novel branch runs
/// right after extraction and never touches metadata, naming or the cover
/// (see docs/output.md), so it is dispatched to [`output::lightnovel`] wholesale.
pub fn convert_source(source: &Path, options: &Options) -> Result<Vec<PathBuf>> {
    convert_source_with(source, options, &progress::Reporter::standalone())
}

/// [`convert_source`] with progress reported through `reporter`.
pub fn convert_source_with(
    source: &Path,
    options: &Options,
    reporter: &progress::Reporter,
) -> Result<Vec<PathBuf>> {
    if options.main.layout == Layout::LightNovel {
        return output::lightnovel::convert_with(source, options, reporter);
    }

    let tree = input::load_tree(source, options)?;
    let cover_override = naming::select_cover(source);
    let prepared = assemble(
        tree,
        cover_override,
        source,
        options,
        TitleOrigin::Derived,
        reporter,
    );
    convert_prepared(prepared, source, options, reporter)
}

/// Where [`assemble`] takes the book's default title from.
#[derive(Debug, Clone, Copy)]
enum TitleOrigin<'a> {
    /// Derive the title from the source path (KCC's usual rule).
    Derived,
    /// A `--file-fusion` run: the synthetic `<name> [fused]` title, whose
    /// `fusion_NNNN_` ordering prefix is stripped from the navigation titles.
    Fusion(&'a str),
}

impl<'a> TitleOrigin<'a> {
    /// The title override passed to metadata resolution.
    fn default_title(self) -> Option<&'a str> {
        match self {
            TitleOrigin::Derived => None,
            TitleOrigin::Fusion(title) => Some(title),
        }
    }

    /// Whether the `fusion_NNNN_` prefix must be stripped from chapter titles.
    fn is_fusion(self) -> bool {
        matches!(self, TitleOrigin::Fusion(_))
    }
}

/// Resolve a tree's metadata, sanitize its names and build the [`PreparedBook`].
///
/// `title_origin` supplies the default title (fusion overrides the title derived
/// from `source`, where the source is a synthetic `<name> [fused]` directory) and
/// whether to strip the `fusion_NNNN_` ordering prefix from navigation titles.
fn assemble(
    mut tree: ComicTree,
    cover_override: Option<PathBuf>,
    source: &Path,
    options: &Options,
    title_origin: TitleOrigin<'_>,
    reporter: &progress::Reporter,
) -> PreparedBook {
    let metadata =
        metadata::resolve_with(&tree, source, &options.output, title_origin.default_title());
    // KCC warns about a likely-degraded conversion after the tree is extracted but
    // before it is renamed (`detectSuboptimalProcessing`).
    for warning in processing::detect_suboptimal_processing(&tree, options) {
        reporter.warn(&warning);
    }
    let mut sanitized = naming::sanitize_tree(&mut tree, options);
    if title_origin.is_fusion() {
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
    reporter: &progress::Reporter,
) -> Result<Vec<PathBuf>> {
    let cover = if options.main.webtoon && prepared.cover_override.is_none() {
        None
    } else {
        processing::cover::process(&prepared.tree, prepared.cover_override.as_deref(), options)?
    };
    if options.main.webtoon {
        processing::webtoon::transform(&mut prepared.tree, options)?;
    }
    let mut processed = processing::process_tree_with(&mut prepared.tree, options, reporter)?;
    processed.cover = cover;
    // The output builders read only the *encoded* book, so the decoded-source tree
    // (source bytes and any residual pixels) can be released before packaging.
    prepared.tree = ComicTree::new();
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
