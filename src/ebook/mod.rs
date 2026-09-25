//! `comic-book ebook` — comic-to-ebook conversion.
//!
//! This is a clean-room Rust port of KCC's `kcc-c2e` pipeline (see `AGENTS.md`).
//! Phase 0 provides the command-line surface, the device-profile tables, option
//! resolution and progress scaffolding; Phase 1 adds the input adapters
//! ([`input`]) that decode archives/folders into a [`ComicTree`]; Phase 2 adds the
//! per-page image pipeline ([`processing`]) that turns a tree into encoded pages;
//! Phase 4 adds metadata resolution ([`metadata`]) and page/chapter naming
//! ([`naming`]). The output builders are added by the phases that follow.
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

    // Phases 0-4 are in place (CLI, option resolution, input adapters, image
    // processing, metadata and naming); the output builders land in later phases
    // (AGENTS.md §15).
    bail!(
        "`comic-book ebook` is not implemented yet: parsing, option resolution, source \
         loading, image processing, metadata and page naming are in place, but the output \
         builders land in later phases.\n\
         Parsed {} input(s); profile {} [{}x{}]; format {:?}.",
        options.inputs.len(),
        options.profile_data.name,
        options.profile_data.width,
        options.profile_data.height,
        options.format,
    )
}
