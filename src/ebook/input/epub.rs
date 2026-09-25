//! EPUB input: spine-ordered images (Phase 11).

use anyhow::{bail, Result};
use std::path::Path;

use crate::ebook::model::ComicTree;

/// Load an EPUB source by walking its spine and collecting the largest image of
/// each page (KCC's `getWorkFolder` EPUB branch). Implemented in Phase 11.
pub fn load(source: &Path) -> Result<ComicTree> {
    bail!(
        "EPUB input is not implemented yet (Phase 11): {}",
        source.display()
    )
}
