//! CBZ repackaging output.
//!
//! `-f cbz` (and the Kindle DX `Auto` default) repackages the processed pages as
//! a ZIP comic without any EPUB scaffolding, mirroring KCC's `makeZIP` over its
//! `OEBPS/Images` tree (see docs/output.md). The processed pages already carry their
//! sanitized `cb-NNNN-cb-<order>` names in their chapter directories, so the
//! archive entries are exactly what KCC's `ComicPage.saveToDir` left on disk.

use std::path::Path;

use anyhow::{Context, Result};

use crate::archive::{ArchiveKind, ArchiveWriter, EntryContent};
use crate::ebook::processing::ProcessedBook;
use crate::ebook::PreparedBook;

/// The cover entry KCC writes at the archive root when the cover is not simply the
/// first page (`##cover.jpg`).
const COVER_NAME: &str = "##cover.jpg";
/// The `ComicInfo.xml` entry KCC round-trips under `--keep-comicinfo`.
const COMICINFO_NAME: &str = "ComicInfo.xml";

/// Repackage a processed book as a CBZ and write it to `dest`.
///
/// The cover is added only when it is not the first page already — KCC gates the
/// `##cover.jpg` write on `cover.smartcover or options.customcover` (see
/// docs/output.md), which the pipeline tracks as the cover's smart-crop flag and
/// a sibling `Covers/` override. `ComicInfo.xml` is added only when
/// `--keep-comicinfo` retained it (KCC's `options.comicinfo_xml`, populated for CBZ
/// only).
pub fn build_cbz(dest: &Path, book: &ProcessedBook, prepared: &PreparedBook) -> Result<()> {
    let mut writer =
        ArchiveWriter::new(ArchiveKind::Cbz, dest).context("Failed to create the CBZ archive")?;

    if let Some(cover) = &book.cover {
        if cover.smart_cropped || prepared.cover_override.is_some() {
            writer.add_entry(COVER_NAME, EntryContent::File(&cover.page.bytes))?;
        }
    }

    if let Some(xml) = &prepared.metadata.comicinfo_xml {
        writer.add_entry(COMICINFO_NAME, EntryContent::File(xml))?;
    }

    for chapter in &book.chapters {
        for page in &chapter.pages {
            writer
                .add_entry(page.name.as_str(), EntryContent::File(&page.bytes))
                .with_context(|| format!("Failed to add {} to the CBZ", page.name))?;
        }
    }

    writer.finish()?;
    Ok(())
}
