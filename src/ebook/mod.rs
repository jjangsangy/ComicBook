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
//! spread variants; Phase 7 adds `-f cbz`, `-f pdf` and `--light-novel`. The Kindle
//! output (Phase 8) and chunking/fusion/webtoon (Phases 9–10) follow.
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

use anyhow::{bail, Result};
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
    let mut tree = input::load_tree(source)?;
    let metadata = metadata::resolve(&tree, source, options);
    let sanitized = naming::sanitize_tree(&mut tree, options);
    let cover_override = naming::select_cover(source);
    Ok(PreparedBook {
        tree,
        metadata,
        cover_override,
        sanitized,
    })
}

/// Run the `ebook` subcommand.
pub fn run_ebook(args: EbookArgs) -> Result<()> {
    let options = Options::resolve(&args)?;

    // Features that change the pipeline wholesale and land in later phases
    // (AGENTS.md §15). Bailing beats silently ignoring the flag.
    if options.file_fusion {
        bail!("--file-fusion is not implemented yet (AGENTS.md §15, Phase 9)");
    }
    if options.webtoon {
        bail!("--webtoon is not implemented yet (AGENTS.md §15, Phase 10)");
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

    let mut prepared = prepare_book(source, options)?;
    // The cover is processed from the source before the per-page pass mutates the
    // first page (cropping), exactly as `makeBook` builds the `Cover` first.
    let cover =
        processing::cover::process(&prepared.tree, prepared.cover_override.as_deref(), options)?;
    let mut processed = processing::process_tree(&mut prepared.tree, options)?;
    if let Some(cover) = cover {
        processed.cover = Some(cover.page);
        processed.cover_smart_crop = cover.smart_cropped;
    }
    output::write_book(&processed, &prepared, source, options)
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
