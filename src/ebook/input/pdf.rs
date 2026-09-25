//! PDF input: embedded-image extraction and optional rasterisation (Phase 11).

use anyhow::{bail, Result};
use std::path::Path;

use crate::ebook::model::ComicTree;

/// Load a PDF source, either by extracting embedded images or by rendering pages
/// (KCC's `getWorkFolder` PDF branch). Implemented in Phase 11.
pub fn load(source: &Path) -> Result<ComicTree> {
    bail!(
        "PDF input is not implemented yet (Phase 11): {}",
        source.display()
    )
}
