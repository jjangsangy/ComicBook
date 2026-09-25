//! `comic-book ebook` — comic-to-ebook conversion.
//!
//! This is a clean-room Rust port of KCC's `kcc-c2e` pipeline (see `AGENTS.md`).
//! Phase 0 provides the command-line surface, the device-profile tables, option
//! resolution and progress scaffolding; Phase 1 adds the input adapters
//! ([`input`]) that decode archives/folders into a [`ComicTree`]; Phase 2 adds the
//! per-page image pipeline ([`processing`]) that turns a tree into encoded pages.
//! The output builders are added by the phases that follow.
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
pub use model::{Background, Chapter, ComicTree, CoverSource, OrderClass, Page, PageFlags};
pub use options::{BorderColor, DocType, Format, Options};
pub use profiles::{DeviceKind, Profile, ProfileData};

use anyhow::{bail, Result};

/// Run the `ebook` subcommand.
pub fn run_ebook(args: EbookArgs) -> Result<()> {
    let options = Options::resolve(&args)?;

    // Phases 0-2 are in place (CLI, option resolution, input adapters, image
    // processing); the output builders land in later phases (AGENTS.md §15).
    bail!(
        "`comic-book ebook` is not implemented yet: parsing, option resolution, source \
         loading and image processing are in place, but the output builders land in later \
         phases.\n\
         Parsed {} input(s); profile {} [{}x{}]; format {:?}.",
        options.inputs.len(),
        options.profile_data.name,
        options.profile_data.width,
        options.profile_data.height,
        options.format,
    )
}
