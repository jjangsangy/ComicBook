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
use relative_path::{Component, RelativePath, RelativePathBuf};
use std::cmp::Ordering;
use std::path::Path;

use crate::archive::{is_os_metadata, open_reader, ArchiveKind, EntryContent};

use crate::ebook::model::{
    Chapter, ChapterName, ComicTree, MediaType, Page, PageData, RelPath, Source, SourceName,
};
use crate::units::Size;

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

    reader.read_entries(&mut scratch, &mut |name, content| {
        let EntryContent::File(data) = content else {
            return Ok(());
        };
        let name = name.as_str();
        if is_os_metadata(name) {
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
    let strip = if kind == ArchiveKind::Directory {
        RootStrip::Keep
    } else {
        RootStrip::Strip
    };
    Ok(build_tree(pages, comicinfo, strip))
}

/// A page decoded from a source entry, before chapter grouping.
///
/// Shared by every input adapter (archive, EPUB, PDF) so they all feed the same
/// chapter-grouping/natural-ordering logic. It is [`Page`]'s book-relative name
/// plus the `PageData`/dimensions carrier, moved into a `Page` unchanged.
pub(crate) struct LoadedPage {
    /// Book-relative source path (before redundant-root stripping).
    pub(crate) name: SourceName,
    /// Encoded source bytes and media type, retained for the lazy decode and
    /// `--no-processing`.
    pub(crate) data: PageData,
    /// Dimensions read from the codec header, without a full decode.
    pub(crate) dimensions: Size,
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
    // Every caller gates on `is_ebook_image`, so the extension is known; the JPEG
    // default keeps the (unreachable) unknown-extension case byte-identical to the
    // old lazy `unwrap_or(MediaType::Jpeg)`.
    let media_type = image_extension(name)
        .and_then(|ext| MediaType::from_extension(&ext))
        .unwrap_or(MediaType::Jpeg);
    Ok(LoadedPage {
        name: SourceName::new(name),
        data: PageData::Encoded(Source::new(data.to_vec(), media_type)),
        dimensions,
    })
}

/// The `(width, height)` of an encoded image, read from its header.
fn image_dimensions(data: &[u8]) -> Result<Size> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .context("image format could not be detected")?;
    let dimensions = reader
        .into_dimensions()
        .context("image dimensions could not be read")?;
    Ok(Size::from_dimensions(dimensions))
}

/// Whether [`build_tree`] should collapse a single redundant top-level folder.
///
/// Mirrors [`crate::archive::RootStripPolicy`] for the in-memory tree build: an archive source
/// strips its wrapper folder, while a directory source (and the EPUB/PDF adapters that
/// synthesize a flat page list) keeps its paths as given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootStrip {
    Strip,
    Keep,
}

/// Group decoded pages into naturally ordered chapters and finish a [`ComicTree`].
///
/// `strip_root` mirrors KCC's archive-only flattening of a single redundant
/// top-level folder (see docs/porting.md); callers that synthesize a flat page list
/// (EPUB spine, PDF pages) pass [`RootStrip::Keep`] because there is nothing to strip.
pub(crate) fn build_tree(
    mut pages: Vec<LoadedPage>,
    comicinfo: Option<Vec<u8>>,
    strip_root: RootStrip,
) -> ComicTree {
    if strip_root == RootStrip::Strip {
        strip_common_root(&mut pages);
    }
    ComicTree {
        chapters: group_into_chapters(pages),
        comicinfo,
    }
}

/// The extension of `name` (after the final `.`), lower-cased, if any.
fn image_extension(name: &str) -> Option<String> {
    RelativePath::new(name)
        .extension()
        .map(str::to_ascii_lowercase)
}

/// Whether `name` looks like an image KCC would keep (`shared.IMAGE_TYPES`).
pub(crate) fn is_ebook_image(name: &str) -> bool {
    image_extension(name)
        .map(|ext| EBOOK_IMAGE_EXTENSIONS.contains(&ext.as_str()))
        .unwrap_or(false)
}

/// Whether `name` is a `ComicInfo.xml` (at any depth).
fn is_comicinfo(name: &str) -> bool {
    RelativePath::new(name).file_name() == Some("ComicInfo.xml")
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
        let mut components = page.name.as_relative().components();
        // A path with no second component (or none at all) sits at the root, so
        // there is no wrapper folder to strip.
        let Some(Component::Normal(first)) = components.next() else {
            return;
        };
        if components.next().is_none() {
            return;
        }
        match root {
            None => root = Some(first),
            Some(existing) if existing == first => {}
            Some(_) => return,
        }
    }

    // The scan above returns early unless every page shares `root` as its first
    // component, so the strip is exactly "drop that first component"; the remainder of a
    // normalized path is itself normalized and non-empty. Dropping by component avoids a
    // fallible `strip_prefix` whose impossible failure the old unconditional slice masked.
    if root.is_some() {
        for page in pages.iter_mut() {
            let mut rest = RelativePathBuf::new();
            for component in page.name.as_relative().components().skip(1) {
                rest.push(component);
            }
            page.name = SourceName::new(rest);
        }
    }
}

/// Group pages (already book-relative) into naturally ordered chapters.
fn group_into_chapters(mut pages: Vec<LoadedPage>) -> Vec<Chapter> {
    pages.sort_by(|a, b| {
        let (dir_a, file_a) = split(a.name.as_relative());
        let (dir_b, file_b) = split(b.name.as_relative());
        compare_dir_paths(dir_a, dir_b).then_with(|| natord::compare_ignore_case(file_a, file_b))
    });

    let mut chapters: Vec<Chapter> = Vec::new();
    for loaded in pages {
        let (dir, file) = split(loaded.name.as_relative());
        let chapter_name = ChapterName::new(dir);
        let rel_path = RelPath::new(file);
        let page = Page {
            source_name: loaded.name,
            rel_path,
            data: loaded.data,
            dimensions: loaded.dimensions,
            background: Default::default(),
        };
        match chapters.last_mut() {
            Some(chapter) if chapter.name == chapter_name => chapter.pages.push(page),
            _ => chapters.push(Chapter {
                name: chapter_name,
                pages: vec![page],
            }),
        }
    }
    chapters
}

/// Split a page path into `(directory, file_name)`; the root directory is `""`.
fn split(path: &RelativePath) -> (&RelativePath, &str) {
    (
        path.parent().unwrap_or(RelativePath::new("")),
        path.file_name().unwrap_or(""),
    )
}

/// Compare two directory paths in KCC's pre-order walk order.
///
/// `os.walk` visits the root before its children and siblings before deeper
/// nests; comparing component-wise (naturally, case-insensitively) and treating
/// a prefix as "less" reproduces that ordering: `"" < "A" < "A/B" < "B"`.
fn compare_dir_paths(a: &RelativePath, b: &RelativePath) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    if a.as_str().is_empty() {
        return Ordering::Less;
    }
    if b.as_str().is_empty() {
        return Ordering::Greater;
    }
    let mut left = a.iter();
    let mut right = b.iter();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(name: &str) -> LoadedPage {
        LoadedPage {
            name: SourceName::new(name),
            data: PageData::Encoded(Source::new(Vec::new(), MediaType::Png)),
            dimensions: Size::new(1, 1),
        }
    }

    #[test]
    fn directory_paths_order_by_walk_pre_order() {
        let path = RelativePath::new;
        assert_eq!(compare_dir_paths(path("A"), path("A")), Ordering::Equal);
        assert_eq!(compare_dir_paths(path(""), path("A")), Ordering::Less);
        assert_eq!(compare_dir_paths(path("A"), path("")), Ordering::Greater);
        // A nested path sorts after its parent (the path is longer but shares a prefix).
        assert_eq!(compare_dir_paths(path("A/B"), path("A")), Ordering::Greater);
        assert_eq!(compare_dir_paths(path("A"), path("A/B")), Ordering::Less);
        // Siblings order naturally and case-insensitively.
        assert_eq!(compare_dir_paths(path("A"), path("b")), Ordering::Less);
    }

    #[test]
    fn strip_common_root_leaves_a_non_normal_first_component_alone() {
        // A `..`-rooted name has no `Normal` first component, so nothing is stripped.
        let mut pages = vec![loaded("..")];
        strip_common_root(&mut pages);
        assert_eq!(pages[0].name.as_str(), "..");
    }

    #[test]
    fn strip_common_root_drops_a_shared_wrapper_directory() {
        // Every page shares `Chapter`, so the wrapper is dropped and `a.png` is kept.
        let mut pages = vec![loaded("Chapter/a.png"), loaded("Chapter/b.png")];
        strip_common_root(&mut pages);
        assert_eq!(pages[0].name.as_str(), "a.png");
        assert_eq!(pages[1].name.as_str(), "b.png");

        // A flat page has no wrapper, so nothing changes.
        let mut flat = vec![loaded("a.png")];
        strip_common_root(&mut flat);
        assert_eq!(flat[0].name.as_str(), "a.png");
    }

    // Property-based checks for the chapter grouping/ordering under `RootStrip::Keep`
    // (docs/development.md): the tree builder must lose no page, reassemble every page's
    // path from its chapter + relative name, and impose exactly the two orderings the
    // walk/natural comparators define.
    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// The path segments a synthetic book draws from: case variants and duplicates so
        /// the grouping and comparator rules are actually exercised.
        const SEGMENTS: &[&str] = &[
            "A", "a", "B", "b", "Chapter", "chapter", "page.png", "Page.PNG", "0", "1",
        ];

        /// A page name of 0..=3 `/`-separated segments (a canonical relative path).
        fn page_name() -> impl Strategy<Value = String> {
            prop::collection::vec(prop::sample::select(SEGMENTS), 0..=3)
                .prop_map(|parts| parts.join("/"))
        }

        /// A random 0..=8 page list, free to contain duplicates and case variants.
        fn page_names() -> impl Strategy<Value = Vec<String>> {
            prop::collection::vec(page_name(), 0..=8)
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(64))]

            /// `build_tree` under `RootStrip::Keep` preserves every page and orders the
            /// chapter tree exactly as the grouping comparators require.
            #[test]
            fn build_tree_keep_preserves_pages_and_orders_the_tree(names in page_names()) {
                let loaded_pages: Vec<LoadedPage> =
                    names.iter().map(|name| loaded(name)).collect();
                let tree = build_tree(loaded_pages, None, RootStrip::Keep);

                // (1) No page is dropped or duplicated.
                prop_assert_eq!(tree.page_count(), names.len());

                // (2) The multiset of source names is preserved.
                let mut expected = names.clone();
                expected.sort();
                let mut actual: Vec<String> = tree
                    .chapters
                    .iter()
                    .flat_map(|chapter| chapter.pages.iter())
                    .map(|page| page.source_name.as_str().to_string())
                    .collect();
                actual.sort();
                prop_assert_eq!(actual, expected);

                // (3) Chapters are non-decreasing in the directory walk order.
                let dirs: Vec<&RelativePath> = tree
                    .chapters
                    .iter()
                    .map(|chapter| chapter.name.as_relative())
                    .collect();
                for (a, b) in dirs.iter().copied().zip(dirs.iter().copied().skip(1)) {
                    prop_assert_ne!(compare_dir_paths(a, b), Ordering::Greater);
                }

                // (4) Pages within a chapter are non-decreasing in natural order.
                for chapter in &tree.chapters {
                    let files: Vec<&str> = chapter
                        .pages
                        .iter()
                        .map(|page| page.rel_path.as_str())
                        .collect();
                    for (a, b) in files.iter().copied().zip(files.iter().copied().skip(1)) {
                        prop_assert_ne!(natord::compare_ignore_case(a, b), Ordering::Greater);
                    }
                }

                // (5) Chapter name joined with the relative path reassembles the source.
                for chapter in &tree.chapters {
                    for page in &chapter.pages {
                        let joined = chapter
                            .name
                            .as_relative()
                            .join(page.rel_path.as_relative());
                        prop_assert_eq!(joined.as_str(), page.source_name.as_str());
                    }
                }
            }
        }
    }
}
