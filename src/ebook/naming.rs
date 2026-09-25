//! Slugify, page naming and output filename resolution (Phase 4).
//!
//! Mirrors KCC's `sanitizeTree`/`slugify`/`getOutputFilename` (AGENTS.md §8),
//! including the `-kcc-x`/`-kcc-a`…`-kcc-d` page suffixes and the `_kcc<N>`
//! collision scheme: [`sanitize_tree`] rewrites a [`ComicTree`]'s chapter
//! directories and page names to the deterministic output layout the pipeline
//! expects, and [`output_filename`] resolves where the finished book is written.
//!
//! Slugification delegates the hard part (Unicode transliteration) to the `slug`
//! crate rather than re-implementing `python-slugify` (AGENTS.md §5.3/§13.8.1);
//! the KCC-specific zero-padding and CBZ pass-through rules are layered on top.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::ebook::model::{ComicTree, MediaType, Page};
use crate::ebook::options::{Format, Options};

/// KCC's deterministic page-name prefix (`kcc-0001`).
const PAGE_PREFIX: &str = "kcc";

/// Image extensions accepted in a sibling `Covers/` directory.
///
/// Deliberately narrower than `shared.IMAGE_TYPES` (`.jp2`/`.avif` included) for
/// the same reason as the page loader: no pure-Rust decoder is available, so
/// those files are ignored rather than aborting (AGENTS.md §13.4.6).
const COVER_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// The result of [`sanitize_tree`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sanitized {
    /// Slugified chapter basename → the pre-slugify basename (KCC's
    /// `chapterNames`). Duplicate basenames overwrite, as KCC's flat dict does.
    pub chapter_titles: HashMap<String, String>,
    /// The output path of the book's cover page: the first page in walk order,
    /// relative to the image root.
    pub cover_path: Option<String>,
}

/// KCC's `slugify` (AGENTS.md §8).
///
/// The format and `is_natural_sorted` arguments reproduce the two shortcuts in
/// the reference: a naturally ordered CBZ keeps its directory names verbatim,
/// and an already-naturally-ordered tree skips the number zero-padding.
pub fn slugify(value: &str, format: Format, is_natural_sorted: bool) -> String {
    if format == Format::Cbz && is_natural_sorted {
        return value.to_string();
    }
    let mut value = if format == Format::Cbz {
        value.to_string()
    } else {
        slug::slugify(value)
    };
    if !is_natural_sorted {
        value = pad_numbers(&value);
    }
    value
}

/// Zero-pad the first two numeric runs to four digits (KCC's non-natural-sort
/// rule). The reference applies `re.sub(r'([0-9]+)', r'0000\1', value, count=2)`
/// and then strips the surplus zeros with `re.sub(r'0*([0-9]{4,})', r'\1', …)`.
fn pad_numbers(value: &str) -> String {
    static INNER: OnceLock<Regex> = OnceLock::new();
    static OUTER: OnceLock<Regex> = OnceLock::new();
    let inner = INNER.get_or_init(|| Regex::new(r"([0-9]+)").expect("valid regex"));
    let outer = OUTER.get_or_init(|| Regex::new(r"0*([0-9]{4,})").expect("valid regex"));
    let padded = inner.replacen(value, 2, "0000${1}");
    outer.replace_all(&padded, "${1}").into_owned()
}

/// Rewrite a tree's chapter directories and page names in place.
///
/// This is the in-memory equivalent of KCC's `sanitizeTree`: every chapter
/// directory path is slugified component by component (with the `A`-suffix
/// collision rule), every page is renamed to `kcc-NNNN.<ext>` with the extension
/// lower-cased, and the first page in walk order is reported as the cover.
pub fn sanitize_tree(tree: &mut ComicTree, options: &Options) -> Sanitized {
    let (slug_map, chapter_titles) = slugify_directories(tree, options);

    let mut page_number = 1u32;
    let mut cover_path = None;
    for chapter in &mut tree.chapters {
        if let Some(slug) = slug_map.get(&chapter.name) {
            chapter.name = slug.clone();
        }
        let directory = chapter.name.clone();
        for page in &mut chapter.pages {
            let stem = format!("{PAGE_PREFIX}-{page_number:04}");
            page_number += 1;
            let file = format!("{stem}.{}", page_extension(page));
            page.rel_path = file.clone();
            page.source_name = if directory.is_empty() {
                file
            } else {
                format!("{directory}/{file}")
            };
            if cover_path.is_none() {
                cover_path = Some(page.source_name.clone());
            }
        }
    }

    Sanitized {
        chapter_titles,
        cover_path,
    }
}

/// Slugify every directory path in the tree.
///
/// Returns the raw-path → slug-path map and the slug-basename → original-basename
/// map. Siblings are processed in KCC's natural order and natural-sortedness is
/// evaluated per parent directory, exactly as `sanitizeTree` does for each
/// `os.walk` root.
fn slugify_directories(
    tree: &ComicTree,
    options: &Options,
) -> (HashMap<String, String>, HashMap<String, String>) {
    // Every directory path (all prefixes of the chapter paths).
    let mut directories: BTreeSet<String> = BTreeSet::new();
    for chapter in &tree.chapters {
        let name = chapter.name.trim_matches('/');
        if name.is_empty() {
            continue;
        }
        let mut prefix = String::new();
        for segment in name.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            directories.insert(prefix.clone());
        }
    }

    // Parent path → immediate child basenames.
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for directory in &directories {
        let (parent, base) = split_dir_file(directory);
        children
            .entry(parent.to_string())
            .or_default()
            .push(base.to_string());
    }

    let mut slug_map: HashMap<String, String> = HashMap::new();
    let mut titles: HashMap<String, String> = HashMap::new();

    // Breadth-first so a parent is always resolved before its children.
    let mut queue: Vec<(String, String)> = vec![(String::new(), String::new())];
    while let Some((raw_parent, slug_parent)) = queue.pop() {
        let Some(siblings) = children.get(&raw_parent) else {
            continue;
        };
        let natural_sorted = is_natural_sorted(siblings);
        let mut order = siblings.clone();
        order.sort_by(|a, b| natord::compare_ignore_case(a, b));

        let mut used: HashSet<String> = HashSet::new();
        for sibling in order {
            let mut slug = slugify(&sibling, options.format, natural_sorted);
            while used.contains(&slug) && sibling.to_uppercase() != slug.to_uppercase() {
                slug.push('A');
            }
            used.insert(slug.clone());

            let raw = join(&raw_parent, &sibling);
            let slugged = join(&slug_parent, &slug);
            titles.insert(slug.clone(), sibling.clone());
            slug_map.insert(raw.clone(), slugged.clone());
            queue.push((raw, slugged));
        }
    }

    (slug_map, titles)
}

/// Whether a sibling set is already in KCC's natural order
/// (`os_sorted(dirs) == sorted(dirs)`).
fn is_natural_sorted(names: &[String]) -> bool {
    let mut natural = names.to_vec();
    natural.sort_by(|a, b| natord::compare_ignore_case(a, b));
    let mut plain = names.to_vec();
    plain.sort();
    natural == plain
}

/// KCC's `getOutputFilename` (AGENTS.md §8).
///
/// `ext` includes the leading dot (`".epub"`); `tome_number` is KCC's
/// `" N"`/`""` suffix. The returned path is made absolute, as the reference
/// calls `os.path.abspath`.
pub fn output_filename(
    source: &Path,
    wanted: Option<&Path>,
    ext: &str,
    tome_number: &str,
    options: &Options,
) -> PathBuf {
    // KCC's `folder_output` (`-f folder`) is not ported; output is always a file.
    let ext = if options.format == Format::Epub && options.kepub {
        ".kepub.epub".to_string()
    } else {
        ext.to_string()
    };

    let filename = match wanted {
        Some(wanted) => wanted_filename(source, wanted, &ext, tome_number),
        None if source.is_dir() => append_str(source, &format!("{tome_number}{ext}")),
        None if options.format == Format::Epub && options.kepub => {
            let base = if source.is_file() {
                source.file_stem()
            } else {
                source.file_name()
            };
            let name = format!(
                "{}{tome_number}{ext}",
                kobo_name(&base.unwrap_or_default().to_string_lossy())
            );
            source.with_file_name(name)
        }
        None => append_str(&strip_extension(source), &format!("{tome_number}{ext}")),
    };

    resolve_collision(filename, &ext, options)
}

/// Resolve the `--output` (`wantedname`) branch of `getOutputFilename`.
fn wanted_filename(source: &Path, wanted: &Path, ext: &str, tome_number: &str) -> PathBuf {
    let wanted_string = wanted.to_string_lossy();
    if wanted_string.ends_with(ext) {
        return absolute(wanted);
    }

    let wanted_root = strip_extension(wanted);
    if extension_with_dot(wanted) == ".mobi" && ext == ".epub" {
        return absolute(&append_str(&wanted_root, ext));
    }

    let directory = absolute(wanted);
    if !directory.exists() {
        let _ = std::fs::create_dir(&directory);
    }
    let base = if source.is_file() {
        source.file_stem()
    } else {
        source.file_name()
    };
    let base = base.unwrap_or_default().to_string_lossy();
    directory.join(format!("{base}{tome_number}{ext}"))
}

/// Append KCC's `_kcc<N>` collision counter, or its MOBI/EPUB equivalent.
fn resolve_collision(filename: PathBuf, ext: &str, options: &Options) -> PathBuf {
    if filename.exists() {
        let basename = strip_extension(&filename);
        let mut counter = 0;
        loop {
            let candidate = append_str(&basename, &format!("_kcc{counter}{ext}"));
            if !candidate.exists() {
                return candidate;
            }
            counter += 1;
        }
    }
    // A kept intermediate EPUB must not clobber the MOBI built from it.
    if options.format == Format::Mobi && ext == ".epub" {
        let basename = strip_extension(&filename);
        if !append_str(&basename, ".mobi").is_file() {
            return filename;
        }
        let mut counter = 0;
        while append_str(&basename, &format!("_kcc{counter}.mobi")).is_file() {
            counter += 1;
        }
        return append_str(&basename, &format!("_kcc{counter}{ext}"));
    }
    filename
}

/// KCC's sibling `Covers/` overrides (AGENTS.md §8).
///
/// When `<source-parent>/Covers/` exists, the source's index among the
/// same-extension sibling files selects the matching cover image. Returns `None`
/// when there is no `Covers/` directory, when the source is not in the series, or
/// when there are fewer covers than siblings.
pub fn select_cover(source: &Path) -> Option<PathBuf> {
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let covers_dir = parent.join("Covers");
    if !covers_dir.is_dir() {
        return None;
    }

    let source_name = source.file_name()?.to_string_lossy().into_owned();
    // KCC derives the series extension from the source (empty for directories).
    let extension = extension_with_dot(source);
    let listing_dir = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let mut series: Vec<String> = read_names(listing_dir)
        .into_iter()
        .filter(|name| name.ends_with(&extension) && !name.contains("_kcc"))
        .collect();
    series.sort_by(|a, b| natord::compare_ignore_case(a, b));
    let index = series.iter().position(|name| name == &source_name)?;

    let mut covers: Vec<String> = read_names(&covers_dir)
        .into_iter()
        .filter(|name| is_cover_image(name))
        .collect();
    covers.sort_by(|a, b| natord::compare_ignore_case(a, b));
    covers.get(index).map(|name| covers_dir.join(name))
}

/// The file names in a directory (empty on error).
fn read_names(directory: &Path) -> Vec<String> {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `name` has one of the cover image extensions.
fn is_cover_image(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => {
            COVER_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        }
        _ => false,
    }
}

/// The lower-cased image extension of a page's source name.
fn page_extension(page: &Page) -> String {
    let file = page
        .source_name
        .rsplit('/')
        .next()
        .unwrap_or(&page.source_name);
    match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => page
            .source_media_type
            .unwrap_or(MediaType::Jpeg)
            .extension()
            .to_string(),
    }
}

/// KCC's `re.sub(r'\W+', '_', name)` for Kobo-brand output names.
fn kobo_name(name: &str) -> String {
    static NON_WORD: OnceLock<Regex> = OnceLock::new();
    let pattern = NON_WORD.get_or_init(|| Regex::new(r"\W+").expect("valid regex"));
    pattern.replace_all(name, "_").into_owned()
}

/// Split a book-relative path into `(directory, base)`; the root is `""`.
fn split_dir_file(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(index) => (&path[..index], &path[index + 1..]),
        None => ("", path),
    }
}

/// Join a directory and a name, skipping the separator at the root.
fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_string()
    } else {
        format!("{directory}/{name}")
    }
}

/// Python's `os.path.splitext(...)[0]`: drop the last extension of the basename.
fn strip_extension(path: &Path) -> PathBuf {
    match path.file_stem() {
        Some(stem) => path.with_file_name(stem),
        None => path.to_path_buf(),
    }
}

/// Python's `os.path.splitext(...)[1]`: the extension including its dot.
fn extension_with_dot(path: &Path) -> String {
    match path.extension() {
        Some(extension) => format!(".{}", extension.to_string_lossy()),
        None => String::new(),
    }
}

/// Append a literal suffix to a path's final component.
fn append_str(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Make a path absolute (KCC's `os.path.abspath`).
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_transliterates_and_collapses() {
        assert_eq!(slugify("Chapter 1", Format::Epub, true), "chapter-1");
        assert_eq!(slugify("Über Stück", Format::Epub, true), "uber-stuck");
        assert_eq!(slugify("A   B", Format::Epub, true), "a-b");
    }

    #[test]
    fn slugify_collapses_underscores_and_dots() {
        // Deliberate deviation from python-slugify, whose custom pattern preserves
        // `_`/`.` — see AGENTS.md §13.8.1.
        assert_eq!(slugify("a_b.c", Format::Epub, true), "a-b-c");
    }

    #[test]
    fn slugify_pads_numbers_unless_naturally_sorted() {
        // "chapter-10" is not naturally sorted, so the first two runs are padded.
        assert_eq!(slugify("Chapter 10", Format::Epub, false), "chapter-0010");
        assert_eq!(slugify("Chapter 10", Format::Epub, true), "chapter-10");
        assert_eq!(slugify("2 Vol 3", Format::Epub, false), "0002-vol-0003");
    }

    #[test]
    fn slugify_keeps_cbz_names_when_naturally_sorted() {
        assert_eq!(slugify("Chapter 1", Format::Cbz, true), "Chapter 1");
        assert_eq!(slugify("Chapter 1", Format::Cbz, false), "Chapter 0001");
    }

    #[test]
    fn natural_order_detection() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Already in plain order, so no padding is needed.
        assert!(is_natural_sorted(&names(&["Bar", "Foo"])));
        assert!(is_natural_sorted(&names(&["Chapter 1", "Chapter 2"])));
        // Plain order disagrees with natural order, so numbers must be padded.
        assert!(!is_natural_sorted(&names(&[
            "Chapter 1",
            "Chapter 10",
            "Chapter 2"
        ])));
        assert!(!is_natural_sorted(&names(&[
            "Chapter 1",
            "Chapter 2",
            "Chapter 10"
        ])));
    }

    #[test]
    fn kobo_name_replaces_non_word_runs() {
        assert_eq!(kobo_name("My Book! (1)"), "My_Book_1_");
        assert_eq!(kobo_name("café.1"), "café_1");
    }
}
