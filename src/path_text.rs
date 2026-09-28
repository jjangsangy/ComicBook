//! Cross-platform text plumbing for the `/`-separated, archive-relative paths the
//! pipeline carries (Phase 8 of the type-safety refactor, findings E5 and G6).
//!
//! Every name that reaches these helpers has already been normalized to forward
//! slashes ([`crate::archive::normalize_archive_path`]), so one `/`-aware
//! implementation is exact where the old `.rsplit(['/', '\\']).next().unwrap_or(name)`
//! ladders were redundant: `rsplit` always yields at least one item, so each
//! `unwrap_or` fallback was dead and hid the invariant. The single genuinely
//! backslash-aware caller — [`crate::archive::is_os_metadata`], which must also
//! classify raw host paths — keeps its own local split and says so at its site.

/// The final `/`-separated segment (Python's `os.path.basename`).
pub(crate) fn file_name(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    }
}

/// Everything before the final `/` (Python's `os.path.dirname`); `""` at the root.
pub(crate) fn directory(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[..index],
        None => "",
    }
}

/// Split a path into its `(directory, file_name)`; the root directory is `""`.
pub(crate) fn split_dir_file(path: &str) -> (&str, &str) {
    (directory(path), file_name(path))
}

/// `path` with its final extension removed (Python's `os.path.splitext(path)[0]`).
///
/// Operates on the whole path, so any directory survives; compose with
/// [`file_name`] to drop the directory as well. A leading dot is not an extension
/// (`.hidden` is returned unchanged) and a name with no dot is returned as-is.
pub(crate) fn stem(path: &str) -> &str {
    match path.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => path,
    }
}

/// The extension of the final segment, without the dot, when a non-empty stem
/// precedes it. `None` for `page`, `cover.` and `.hidden`.
pub(crate) fn extension(path: &str) -> Option<&str> {
    match file_name(path).rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => Some(ext),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_is_the_last_segment() {
        assert_eq!(file_name("a/b/c.jpg"), "c.jpg");
        assert_eq!(file_name("c.jpg"), "c.jpg");
        assert_eq!(file_name(""), "");
    }

    #[test]
    fn directory_is_everything_before_the_last_segment() {
        assert_eq!(directory("a/b/c.jpg"), "a/b");
        assert_eq!(directory("c.jpg"), "");
        assert_eq!(split_dir_file("a/b"), ("a", "b"));
        assert_eq!(split_dir_file("b"), ("", "b"));
    }

    #[test]
    fn stem_drops_only_the_final_extension() {
        assert_eq!(stem("a/b.c"), "a/b");
        assert_eq!(stem("a.b.c"), "a.b");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(stem("a"), "a");
        assert_eq!(stem("a."), "a");
    }

    #[test]
    fn extension_ignores_a_leading_dot_and_empty_stems() {
        assert_eq!(extension("a/b.JPG"), Some("JPG"));
        assert_eq!(extension("a."), Some(""));
        assert_eq!(extension("a"), None);
        assert_eq!(extension(".hidden"), None);
        assert_eq!(extension("dir/.hidden"), None);
    }
}
