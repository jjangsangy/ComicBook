//! Archive and directory input via [`crate::archive`].
//!
//! Loads a `.cbz`/`.cbr`/`.cb7`/`.cbt` archive or an image folder into a
//! [`ComicTree`], covering the parts of KCC's `getWorkFolder` + `removeNonImages`
//! + `sanitizeTree` that concern *structure*:
//!
//! - non-image entries (and OS junk such as `._*` / `.DS_Store`) are dropped;
//! - a single redundant root directory wrapping an archive is stripped, matching
//!   KCC's "one top-level folder" flattening (image folders are already relative
//!   to their own root, so they are used as-is);
//! - pages and chapters are ordered naturally (case-insensitive alphanumeric),
//!   matching KCC's `walkSort`/`os_sorted`;
//! - pages are grouped into chapters by their containing directory;
//! - a top-level `ComicInfo.xml`, if present, is retained on the tree.
//!
//! Unlike KCC, nothing is written to a temp directory: entries are decoded
//! straight into the in-memory tree.

use anyhow::{bail, Context, Result};
use std::cmp::Ordering;
use std::path::Path;

use crate::archive::{is_os_metadata, open_reader, ArchiveKind};

use crate::ebook::model::{Chapter, ComicTree, CoverSource, MediaType, Page};

/// Image extensions accepted as comic pages.
///
/// This is KCC's `shared.IMAGE_TYPES` minus `.jp2` and `.avif`, which have no decoder in the
/// current (pure-Rust) dependency set and are therefore ignored like any other non-image
/// rather than aborting the conversion. Deliberately narrower than `image_ops::IMG_EXTENSIONS`
/// (see docs/dependencies.md): the clamp/convert commands use the wider set.
const EBOOK_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Load an archive or image folder into a [`ComicTree`].
///
/// `source` may be a supported archive file or a directory; the reader is chosen
/// from `kind` (normally produced by [`crate::archive::detect_archive_kind`]).
pub fn load(source: &Path, kind: ArchiveKind) -> Result<ComicTree> {
    let mut reader = open_reader(kind, source)
        .with_context(|| format!("Failed to open source file/directory: {}", source.display()))?;

    let mut scratch = Vec::new();
    let mut pages: Vec<LoadedPage> = Vec::new();
    let mut comicinfo: Option<Vec<u8>> = None;

    reader.read_entries(&mut scratch, &mut |name, is_dir, data| {
        if is_dir || is_os_metadata(name) {
            return Ok(());
        }
        if is_comicinfo(name) {
            // Prefer the images-root copy over any nested one. KCC only ever
            // reads `<Images>/ComicInfo.xml`.
            if comicinfo.is_none() || !name.contains('/') {
                comicinfo = Some(data.to_vec());
            }
            return Ok(());
        }
        if is_ebook_image(name) {
            pages.push(load_page(name, data)?);
        }
        Ok(())
    })?;

    if pages.is_empty() {
        bail!(
            "No images detected in '{}'.\n\n\
             Possible causes:\n\n\
             1) Incompatible image file extension like .jxl. Convert to .png first.\n\
             2) Nested archive: extract it first, or use --file-fusion.",
            source.display()
        );
    }

    // KCC flattens a single top-level folder when extracting an archive, but
    // copies a folder source verbatim. Mirror both.
    Ok(build_tree(pages, comicinfo, kind != ArchiveKind::Directory))
}

/// A page decoded from a source entry, before chapter grouping.
///
/// Shared by every input adapter (archive, EPUB, PDF) so they all feed the same
/// chapter-grouping/natural-ordering logic.
pub(crate) struct LoadedPage {
    /// Book-relative source path (before redundant-root stripping).
    pub(crate) name: String,
    pub(crate) media_type: Option<MediaType>,
    /// Encoded source bytes, retained for the lazy decode and `--no-processing`.
    pub(crate) raw: Vec<u8>,
    /// Dimensions read from the codec header, without a full decode.
    pub(crate) dimensions: (u32, u32),
}

/// Read one encoded image entry into a [`LoadedPage`].
///
/// Only the codec header is parsed here: the pixel decode is deferred to
/// processing (see [`crate::ebook::model::Page`]), so ingest does not allocate a
/// decoded image per page. The media type is inferred from the name's extension,
/// which is how the OPF manifest later learns the payload's type.
pub(crate) fn load_page(name: &str, data: &[u8]) -> Result<LoadedPage> {
    let dimensions = image_dimensions(data)
        .with_context(|| format!("Image file {name} could not be decoded"))?;
    Ok(LoadedPage {
        name: name.to_string(),
        media_type: image_extension(name).and_then(|ext| MediaType::from_extension(&ext)),
        raw: data.to_vec(),
        dimensions,
    })
}

/// The `(width, height)` of an encoded image, read from its header.
fn image_dimensions(data: &[u8]) -> Result<(u32, u32)> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .context("image format could not be detected")?;
    reader
        .into_dimensions()
        .context("image dimensions could not be read")
}

/// Group decoded pages into naturally ordered chapters and finish a [`ComicTree`].
///
/// `strip_root` mirrors KCC's archive-only flattening of a single redundant
/// top-level folder (see docs/porting.md); callers that synthesize a flat page list
/// (EPUB spine, PDF pages) pass `false` because there is nothing to strip.
pub(crate) fn build_tree(
    mut pages: Vec<LoadedPage>,
    comicinfo: Option<Vec<u8>>,
    strip_root: bool,
) -> ComicTree {
    if strip_root {
        strip_common_root(&mut pages);
    }
    ComicTree {
        chapters: group_into_chapters(pages),
        cover: Some(CoverSource::FirstPage),
        comicinfo,
    }
}

/// The extension of `name` (after the final `.`), lower-cased, if any.
fn image_extension(name: &str) -> Option<String> {
    let file = name.rsplit('/').next().unwrap_or(name);
    let (stem, ext) = file.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Whether `name` looks like an image KCC would keep (`shared.IMAGE_TYPES`).
pub(crate) fn is_ebook_image(name: &str) -> bool {
    image_extension(name)
        .map(|ext| EBOOK_IMAGE_EXTENSIONS.contains(&ext.as_str()))
        .unwrap_or(false)
}

/// Whether `name` is a `ComicInfo.xml` (at any depth).
fn is_comicinfo(name: &str) -> bool {
    name.rsplit('/').next() == Some("ComicInfo.xml")
}

/// Strip a single common top-level directory from every page path.
///
/// Mirrors KCC's `getWorkFolder` behaviour: when an archive extracts to exactly
/// one top-level folder, that folder's contents are hoisted to the image root.
/// Paths already at the root (or spanning more than one top-level entry) are left
/// untouched. Kept bespoke (see docs/dependencies.md) — the rule is KCC-specific
/// (see docs/porting.md).
fn strip_common_root(pages: &mut [LoadedPage]) {
    let mut root: Option<&str> = None;
    for page in pages.iter() {
        let mut components = page.name.split('/');
        let first = components.next().unwrap_or("");
        if first.is_empty() || components.next().is_none() {
            // A file sitting directly at the root, so there is no wrapper folder.
            return;
        }
        match root {
            None => root = Some(first),
            Some(existing) if existing == first => {}
            Some(_) => return,
        }
    }

    if let Some(root) = root {
        // Safe to slice: `root` ends on a char boundary and is followed by '/'.
        let prefix_len = root.len() + 1;
        for page in pages.iter_mut() {
            page.name = page.name[prefix_len..].to_string();
        }
    }
}

/// Group pages (already book-relative) into naturally ordered chapters.
fn group_into_chapters(mut pages: Vec<LoadedPage>) -> Vec<Chapter> {
    pages.sort_by(|a, b| {
        let (dir_a, file_a) = split_dir_file(&a.name);
        let (dir_b, file_b) = split_dir_file(&b.name);
        compare_dir_paths(dir_a, dir_b).then_with(|| natord::compare_ignore_case(file_a, file_b))
    });

    let mut chapters: Vec<Chapter> = Vec::new();
    for loaded in pages {
        let (dir, file) = split_dir_file(&loaded.name);
        let (dir_name, file_name) = (dir.to_string(), file.to_string());
        let page = Page {
            source_name: loaded.name,
            rel_path: file_name,
            image: None,
            dimensions: loaded.dimensions,
            background: Default::default(),
            flags: Default::default(),
            raw: Some(loaded.raw),
            source_media_type: loaded.media_type,
        };
        match chapters.last_mut() {
            Some(chapter) if chapter.name == dir_name => chapter.pages.push(page),
            _ => chapters.push(Chapter {
                name: dir_name,
                pages: vec![page],
            }),
        }
    }
    chapters
}

/// Split a book-relative path into `(directory, file_name)`; the root directory is `""`.
fn split_dir_file(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(index) => (&path[..index], &path[index + 1..]),
        None => ("", path),
    }
}

/// Compare two directory paths in KCC's pre-order walk order.
///
/// `os.walk` visits the root before its children and siblings before deeper
/// nests; comparing component-wise (naturally, case-insensitively) and treating
/// a prefix as "less" reproduces that ordering: `"" < "A" < "A/B" < "B"`.
fn compare_dir_paths(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    if a.is_empty() {
        return Ordering::Less;
    }
    if b.is_empty() {
        return Ordering::Greater;
    }
    let mut left = a.split('/');
    let mut right = b.split('/');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match natord::compare_ignore_case(x, y) {
                Ordering::Equal => {}
                other => return other,
            },
        }
    }
}
