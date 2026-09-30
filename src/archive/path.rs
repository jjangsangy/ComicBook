use std::fmt;
use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use relative_path::{Component, RelativePath, RelativePathBuf};

/// A validated, normalized archive entry path.
///
/// The only constructors are [`normalize_archive_path`] and [`parse_entry_info`], so any value
/// of this type is guaranteed non-empty, forward-slash separated, and free of traversal, `.`/`..`
/// and Windows-drive segments. Handing one to a writer therefore cannot silently rewrite the
/// entry name (the failure mode `add_entry_normalized` used to allow by taking a bare `&str`).
///
/// Backed by a [`RelativePathBuf`] (docs/refactor.md §8.1): the crate's model is exactly this type's
/// invariant — a relative, `/`-separated path — so its `file_name`/`parent`/`extension`/`strip_prefix`
/// operations replace the hand-rolled separator ladders. The sanitizer invariant on top (non-empty,
/// zip-slip-safe, no drive prefixes) is *not* something `RelativePathBuf` guarantees, which is why
/// the outer newtype stays and the crate type is never exposed directly.
///
/// `#[repr(transparent)]`: layout-identical to the owned `RelativePathBuf` produced by
/// normalization, with no allocation or copy of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct NormalizedArchivePath(RelativePathBuf);

impl NormalizedArchivePath {
    /// The normalized path as a borrowed string.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The normalized path as a borrowed, `/`-separated [`RelativePath`].
    ///
    /// This is the entry point for cross-platform path operations (see docs/refactor.md §8.1):
    /// callers use `as_relative().file_name()`/`parent()`/`extension()`/`components()`
    /// instead of splitting the string on separators themselves.
    pub fn as_relative(&self) -> &RelativePath {
        self.0.as_relative_path()
    }

    /// The remainder after removing `prefix`, which must end on a component boundary (e.g.
    /// `"Root/"`). The result is already normalized: it is a suffix of a normalized path.
    ///
    /// Returns `None` when `prefix` is not a whole-component prefix of `self`, and also when
    /// it consumes `self` entirely (`"Root"` against `"Root/"`): the empty remainder would
    /// break this type's non-empty invariant, and callers treat "nothing left" as no match.
    pub fn strip_prefix(&self, prefix: &str) -> Option<NormalizedArchivePath> {
        let rest = self.as_relative().strip_prefix(prefix).ok()?;
        if rest.as_str().is_empty() {
            return None;
        }
        Some(NormalizedArchivePath(rest.to_relative_path_buf()))
    }
}

impl Deref for NormalizedArchivePath {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for NormalizedArchivePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// Whether an archive entry is a file or a directory.
///
/// Replaces the `bool` that used to travel beside an entry name; the "a directory carries no
/// data" invariant is now expressed by [`crate::archive::reader::EntryContent`] rather than
/// re-asserted at every producer and consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
}

/// A listed archive entry: a normalized name plus its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub name: NormalizedArchivePath,
    pub kind: EntryKind,
}

/// Normalize an archive entry path to a clean, forward-slash-separated relative path.
///
/// Strips leading/trailing slashes, backslashes, Windows drive prefixes (e.g. `C:`),
/// redundant `./`, and `..` segments to prevent path traversal and ensure uniform cross-platform
/// compatibility across Windows, Linux, and macOS. Returns `None` when nothing is left, so empty
/// paths are unrepresentable rather than signalled by a `""` sentinel. Kept bespoke (see
/// docs/dependencies.md).
pub fn normalize_archive_path(raw: &str) -> Option<NormalizedArchivePath> {
    // Build the result in place (a single buffer) rather than collecting segments into an
    // intermediate `Vec` and joining them, which avoids an allocation for every entry.
    let mut out = String::with_capacity(raw.len());
    for part in raw.split(['/', '\\']) {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            continue;
        }
        // Remove Windows drive letters (e.g. "C:", "c:")
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            continue;
        }
        // Strip any trailing colon(s) and re-trim, then re-run the `.`/`..` guard: the
        // colon strip can re-expose whitespace (`"a :"` -> `"a"`) and, crucially, can
        // turn a segment into a traversal one (`"..:"` -> `".."`). Doing the dot check
        // *after* the strip is what makes normalization idempotent and closes the
        // `safe_join` escape; see the properties below.
        let cleaned = trimmed.trim_end_matches(':').trim();
        if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(cleaned);
    }
    if out.is_empty() {
        None
    } else {
        Some(NormalizedArchivePath(RelativePathBuf::from(out)))
    }
}

/// Safely join an archive entry path onto a destination directory using the host platform's
/// native path separators, while preventing path traversal or escaping the target directory.
///
/// This is deliberately "sanitize, then [`RelativePath::to_path`]": the same traversal/`.`/`..`
/// and drive-prefix rules as [`normalize_archive_path`] are applied, so `..` is *dropped* rather
/// than *popped* (it is not `to_logical_path`); see `integration_tests::test_safe_join`. Reusing
/// the sanitizer removes the previous second copy of the separator ladder (docs/refactor.md §3.6).
pub fn safe_join<P: AsRef<Path>>(base: P, relative: &str) -> PathBuf {
    match normalize_archive_path(relative) {
        Some(normalized) => normalized.as_relative().to_path(base),
        None => base.as_ref().to_path_buf(),
    }
}

/// Recursively copy an entire directory tree from `src` to `dst` (kept bespoke; see
/// docs/dependencies.md).
pub fn copy_dir_all<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    let dst = dst.as_ref();
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_all(entry.path(), target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Whether an entry is OS-generated junk/metadata (KCC's `dot_clean`).
///
/// `__MACOSX` is matched per path component and the dot-files by base name, so a
/// nested `Chapter/._page.jpg` is caught as well as a root-level one.
///
/// This deliberately keeps its own backslash-aware split rather than using
/// [`RelativePath`]: it also classifies raw host paths (e.g. a `\`-separated Windows
/// name), which the normalised, forward-slash-only `RelativePath` model never sees
/// (docs/refactor.md E5 / §8.1).
pub fn is_os_metadata(name: &str) -> bool {
    if name.split(['/', '\\']).any(|part| part == "__MACOSX") {
        return true;
    }
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.starts_with("._")
        || base.eq_ignore_ascii_case(".DS_Store")
        || base.eq_ignore_ascii_case("Thumbs.db")
}

/// Helper to extract a normalized entry (name and kind) from raw archive metadata.
///
/// Returns `None` if the normalized path is empty.
pub fn parse_entry_info(raw_name: &str, format_is_dir: bool) -> Option<ArchiveEntry> {
    let name = normalize_archive_path(raw_name)?;
    let kind = if format_is_dir || raw_name.ends_with('/') || raw_name.ends_with('\\') {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    Some(ArchiveEntry { name, kind })
}

/// Check if all entries in an archive belong to a single top-level directory.
///
/// If every non-metadata file and directory resides inside one top-level folder,
/// returns `Some(root_folder_name)`. Otherwise, returns `None`.
pub fn find_single_root_dir(entries: &[ArchiveEntry]) -> Option<String> {
    let mut candidate_root: Option<&str> = None;

    for entry in entries {
        let name = entry.name.as_str();

        // Ignore metadata or OS junk files when determining if there is a common root
        if is_os_metadata(name) {
            continue;
        }

        // Normalized names are `/`-separated and traversal-free, so the component
        // iterator yields the first segment without allocating (docs/refactor.md §8.1).
        let mut components = entry.name.as_relative().components();
        let first = match components.next() {
            Some(Component::Normal(segment)) => segment,
            _ => continue,
        };

        // If this entry is a file at the root level (a single component), then the
        // archive has files at root, so there is no single root folder!
        if entry.kind == EntryKind::File && components.next().is_none() {
            return None;
        }

        match candidate_root {
            None => candidate_root = Some(first),
            Some(curr) if curr == first => {}
            Some(_) => return None, // Multiple different top-level items
        }
    }

    candidate_root.map(str::to_string)
}

/// Check whether an archive's root folder matches the destination folder or source file
/// stem (kept bespoke; see docs/dependencies.md).
///
/// An absent destination name or source stem is passed as `None` and simply cannot match,
/// rather than being collapsed into an empty string that is indistinguishable from a real
/// empty component.
pub fn is_matching_root(root: &str, dest_name: Option<&str>, src_stem: Option<&str>) -> bool {
    fn normalize(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    }

    let norm_root = normalize(root);
    if norm_root.is_empty() {
        return false;
    }

    [dest_name, src_stem]
        .into_iter()
        .flatten()
        .any(|candidate| {
            let norm = normalize(candidate);
            !norm.is_empty() && (norm.contains(&norm_root) || norm_root.contains(&norm))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_paths_are_rejected() {
        // No `""` sentinel: a path that normalizes to nothing is `None`.
        assert_eq!(normalize_archive_path(""), None);
        assert_eq!(normalize_archive_path("///"), None);
        assert_eq!(normalize_archive_path("./.\\.."), None);
        assert_eq!(parse_entry_info("", false), None);
    }

    #[test]
    fn parse_entry_info_tags_files_and_directories() {
        // A trailing separator or the format's own directory flag marks a directory; anything
        // else is a file. The normalized name travels with the kind.
        assert_eq!(
            parse_entry_info("Chapter/page.jpg", false).map(|entry| entry.kind),
            Some(EntryKind::File)
        );
        assert_eq!(
            parse_entry_info("Chapter/", false).map(|entry| entry.kind),
            Some(EntryKind::Directory)
        );
        // A raw name ending in a backslash is a directory too, matching how the zip/tar/7z/rar
        // backends read `parse_entry_info` (see the Phase 1 note in docs/refactor.md).
        assert_eq!(
            parse_entry_info("weird\\", false).map(|entry| entry.kind),
            Some(EntryKind::Directory)
        );
        assert_eq!(
            parse_entry_info("Chapter", true).map(|entry| entry.kind),
            Some(EntryKind::Directory)
        );
        assert_eq!(
            parse_entry_info("./Chapter\\page.jpg", false)
                .map(|entry| entry.name.as_str().to_string()),
            Some("Chapter/page.jpg".to_string())
        );
    }

    #[test]
    fn find_single_root_dir_reads_entry_kinds() {
        let entries = |paths: &[(&str, EntryKind)]| -> Vec<ArchiveEntry> {
            paths
                .iter()
                .filter_map(|(name, kind)| {
                    normalize_archive_path(name).map(|name| ArchiveEntry { name, kind: *kind })
                })
                .collect()
        };

        // Everything shares one top-level folder, so it is the wrapper to collapse.
        assert_eq!(
            find_single_root_dir(&entries(&[
                ("Root/a.jpg", EntryKind::File),
                ("Root/sub", EntryKind::Directory),
                ("Root/sub/b.jpg", EntryKind::File),
            ])),
            Some("Root".to_string())
        );
        // A file at the root means there is no wrapper.
        assert_eq!(
            find_single_root_dir(&entries(&[
                ("a.jpg", EntryKind::File),
                ("Root/b.jpg", EntryKind::File),
            ])),
            None
        );
        // Two different top-level items mean there is no wrapper.
        assert_eq!(
            find_single_root_dir(&entries(&[
                ("A/1.jpg", EntryKind::File),
                ("B/2.jpg", EntryKind::File),
            ])),
            None
        );
    }

    #[test]
    fn strip_prefix_is_component_wise_and_never_empty() {
        assert_eq!(
            normalize_archive_path("Root/Chapter/page.jpg")
                .and_then(|path| path.strip_prefix("Root/"))
                .map(|path| path.as_str().to_string()),
            Some("Chapter/page.jpg".to_string())
        );
        // A byte prefix that is not a whole component does not match.
        assert_eq!(
            normalize_archive_path("RootX/page.jpg").and_then(|path| path.strip_prefix("Root/")),
            None
        );
        // Consuming the whole path leaves no empty remainder.
        assert_eq!(
            normalize_archive_path("Root").and_then(|path| path.strip_prefix("Root/")),
            None
        );
    }

    #[test]
    fn is_os_metadata_classifies_raw_host_paths() {
        // The classifier keeps its own split because it also sees raw host paths, where
        // the separator may be a backslash (docs/refactor.md E5 / §8.1).
        assert!(is_os_metadata("__MACOSX\\Chapter\\page.jpg"));
        assert!(is_os_metadata("Chapter\\._page.jpg"));
        assert!(is_os_metadata("Chapter\\Thumbs.db"));
        assert!(!is_os_metadata("Chapter\\page.jpg"));
    }

    #[test]
    fn matching_root_treats_absent_names_as_non_matching() {
        assert!(is_matching_root("Issue 01", Some("Issue_01"), None));
        assert!(is_matching_root("Issue 01", None, Some("issue-01")));
        // An absent destination name or source stem cannot match a real root.
        assert!(!is_matching_root("Issue 01", None, None));
        assert!(!is_matching_root("Issue 01", Some(""), Some("")));
        // A root with no alphanumerics normalizes to nothing and never matches.
        assert!(!is_matching_root("---", Some("x"), None));
        assert!(!is_matching_root("!!!", Some("!!!"), None));
    }

    #[test]
    fn colon_only_segments_are_dropped() {
        // A segment of only colons (longer than the two-character drive-letter form)
        // normalizes away instead of leaving an empty component.
        assert_eq!(normalize_archive_path(":"), None);
        assert_eq!(
            normalize_archive_path("a/:::/b").map(|p| p.as_str().to_string()),
            Some("a/b".to_string())
        );
    }

    #[test]
    fn find_single_root_dir_ignores_os_metadata() {
        let entries = vec![
            parse_entry_info("__MACOSX/._page.jpg", false),
            parse_entry_info("._page.jpg", false),
            parse_entry_info("Root/page.jpg", false),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        assert_eq!(find_single_root_dir(&entries), Some("Root".to_string()));
    }

    #[test]
    fn copy_dir_all_recurses_into_subdirectories() -> std::io::Result<()> {
        let tmp = tempfile::tempdir()?;
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("nested"))?;
        std::fs::write(src.join("top.txt"), b"top")?;
        std::fs::write(src.join("nested/deep.txt"), b"deep")?;

        let dst = tmp.path().join("dst");
        copy_dir_all(&src, &dst)?;
        assert!(dst.join("top.txt").is_file());
        assert_eq!(std::fs::read(dst.join("nested/deep.txt"))?, b"deep");
        Ok(())
    }

    // Property-based checks for the sanitizer and `safe_join` (docs/development.md). The
    // alphabet deliberately includes the separators, dots, colons and whitespace the
    // sanitizer has special rules for, so the drive-letter/traversal shapes are exercised
    // rather than a vacuous alphanumerics-only generator.
    mod properties {
        use super::*;
        use proptest::prelude::*;

        /// The characters archive entry names are built from: alphanumerics plus the
        /// separators, dots, colons and whitespace the sanitizer distinguishes.
        const PATH_ALPHABET: &[char] = &['a', 'Z', '7', '/', '\\', '.', ':', ' ', '\t', '_', '-'];

        /// Hostile hand-picked names plus a mixed random name over [`PATH_ALPHABET`].
        /// `"..:"` and `"a :"` are the counterexamples that pinned the
        /// dot-segment/idempotence invariants below (a leaked `..`/re-exposed space),
        /// kept explicit so those properties keep exercising the fixed path.
        fn mixed_archive_name() -> impl Strategy<Value = String> {
            prop_oneof![
                Just("../".to_string()),
                Just("C:\\".to_string()),
                Just("a :".to_string()),
                Just("a/./b".to_string()),
                Just("..:".to_string()),
                prop::collection::vec(prop::sample::select(PATH_ALPHABET), 0..=24)
                    .prop_map(|chars| chars.into_iter().collect()),
            ]
        }

        /// The raw `safe_join` seeds plus an arbitrary (possibly non-ASCII) string.
        /// `"..:"` is the base-escape counterexample that pinned the containment
        /// property below.
        fn raw_archive_name() -> impl Strategy<Value = String> {
            prop_oneof![
                Just("../../etc/passwd".to_string()),
                Just("..\\..\\x".to_string()),
                Just("/abs/x".to_string()),
                Just("C:\\x".to_string()),
                Just("a/./b".to_string()),
                Just(String::new()),
                Just("///".to_string()),
                Just("..:".to_string()),
                any::<String>(),
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(64))]

            /// A successful normalization is a clean, forward-slash, relative path: no
            /// leading/trailing separator, no backslash, no empty component, and no
            /// component ending in `:` (the drive-letter strip is total).
            #[test]
            fn normalize_archive_path_emits_a_clean_relative_path(raw in mixed_archive_name()) {
                if let Some(normalized) = normalize_archive_path(&raw) {
                    let s = normalized.as_str();
                    prop_assert!(!s.is_empty());
                    prop_assert!(!s.starts_with('/'));
                    prop_assert!(!s.ends_with('/'));
                    prop_assert!(!s.contains('\\'));
                    for component in s.split('/') {
                        prop_assert!(!component.is_empty());
                        prop_assert!(!component.ends_with(':'));
                    }
                }
            }

            /// The sanitizer must never emit a `.` or `..` segment: it is the traversal
            /// guard the whole path-safety story rests on (the `"..:"` case is the one
            /// that pinned it).
            #[test]
            fn normalize_archive_path_never_emits_dot_segments(raw in mixed_archive_name()) {
                if let Some(normalized) = normalize_archive_path(&raw) {
                    for component in normalized.as_str().split('/') {
                        prop_assert_ne!(component, ".");
                        prop_assert_ne!(component, "..");
                    }
                }
            }

            /// Re-normalizing a normalized path is a no-op (the `"a :"` case is the one
            /// that pinned it).
            #[test]
            fn normalize_archive_path_is_idempotent(raw in mixed_archive_name()) {
                if let Some(normalized) = normalize_archive_path(&raw) {
                    let again = normalize_archive_path(normalized.as_str());
                    prop_assert_eq!(
                        again.as_ref().map(NormalizedArchivePath::as_str),
                        Some(normalized.as_str()),
                    );
                }
            }

            /// `safe_join` always lands inside `base`, and a relative path that
            /// normalizes away leaves `base` untouched.
            #[test]
            fn safe_join_stays_under_base(raw in raw_archive_name()) {
                let dir = tempfile::tempdir()?;
                let base = dir.path();
                let out = safe_join(base, &raw);
                prop_assert!(out.starts_with(base), "{raw:?} escaped {base:?}: {out:?}");
                if normalize_archive_path(&raw).is_none() {
                    prop_assert_eq!(out, base.to_path_buf());
                }
            }

            /// A joined path never contains a `..` (`ParentDir`) component (the `"..:"`
            /// case, which normalization previously leaked, is the one that pinned it).
            #[test]
            fn safe_join_never_escapes_base(raw in raw_archive_name()) {
                let dir = tempfile::tempdir()?;
                let base = dir.path();
                let out = safe_join(base, &raw);
                for component in out.components() {
                    prop_assert!(!matches!(component, std::path::Component::ParentDir));
                }
            }
        }
    }
}
