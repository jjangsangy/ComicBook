//! `comic-book ebook` — comic-to-ebook conversion.
//!
//! This is a clean-room Rust port of KCC's `kcc-c2e` pipeline (see `AGENTS.md`).
//! Phase 0 provides the command-line surface, the device-profile tables, option
//! resolution and progress scaffolding; the conversion stages are added by the
//! phases that follow.
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

    // Phase 0 scaffolding: argument parsing and option resolution are complete,
    // but the processing/output pipeline lands in later phases (AGENTS.md §15).
    bail!(
        "`comic-book ebook` is not implemented yet (Phase 0 scaffolding only).\n\
         Parsed {} input(s); profile {} [{}x{}]; format {:?}.",
        options.inputs.len(),
        options.profile_data.name,
        options.profile_data.width,
        options.profile_data.height,
        options.format,
    )
}
