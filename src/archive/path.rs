use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Normalize an archive entry path to a clean, forward-slash-separated relative path.
///
/// Strips leading/trailing slashes, backslashes, Windows drive prefixes (e.g. `C:`),
/// redundant `./`, and `..` segments to prevent path traversal and ensure uniform cross-platform
/// compatibility across Windows, Linux, and macOS.
pub fn normalize_archive_path(raw: &str) -> String {
    let mut parts = Vec::new();
    for part in raw.split(['/', '\\']) {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            continue;
        }
        // Remove Windows drive letters (e.g. "C:", "c:")
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            continue;
        }
        let cleaned = trimmed.trim_end_matches(':');
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }
    }
    parts.join("/")
}

/// Safely join an archive entry path onto a destination directory using the host platform's
/// native path separators, while preventing path traversal or escaping the target directory.
pub fn safe_join<P: AsRef<Path>>(base: P, relative: &str) -> PathBuf {
    let mut target = base.as_ref().to_path_buf();
    for part in relative.split(['/', '\\']) {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            continue;
        }
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            continue;
        }
        let cleaned = trimmed.trim_end_matches(':');
        if !cleaned.is_empty() {
            target.push(cleaned);
        }
    }
    target
}

/// Recursively copy an entire directory tree from `src` to `dst`.
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

/// Helper to determine whether an entry is OS-generated junk/metadata.
pub fn is_os_metadata(name: &str) -> bool {
    name.starts_with("__MACOSX")
        || name.ends_with(".DS_Store")
        || name.ends_with("Thumbs.db")
        || name.starts_with("._")
}

/// Helper to extract normalized entry path and directory flag from raw archive metadata.
///
/// Returns `None` if the normalized path is empty.
pub fn parse_entry_info(raw_name: &str, format_is_dir: bool) -> Option<(String, bool)> {
    let clean = normalize_archive_path(raw_name);
    if clean.is_empty() {
        return None;
    }
    let is_dir = format_is_dir || raw_name.ends_with('/') || raw_name.ends_with('\\');
    Some((clean, is_dir))
}

/// Check if all entries in an archive belong to a single top-level directory.
///
/// If every non-metadata file and directory resides inside one top-level folder,
/// returns `Some(root_folder_name)`. Otherwise, returns `None`.
pub fn find_single_root_dir(entries: &[(String, bool)]) -> Option<String> {
    let mut candidate_root: Option<&str> = None;

    for (name, is_dir) in entries {
        // Ignore metadata or OS junk files when determining if there is a common root
        if is_os_metadata(name) {
            continue;
        }

        let parts: Vec<&str> = name.split('/').collect();
        if parts.is_empty() {
            continue;
        }

        let first = parts[0];

        // If this entry is a file at the root level (no '/' in path),
        // then the archive has files at root, so there is no single root folder!
        if parts.len() == 1 && !*is_dir {
            return None;
        }

        match candidate_root {
            None => candidate_root = Some(first),
            Some(curr) if curr == first => {}
            Some(_) => return None, // Multiple different top-level items
        }
    }

    candidate_root.map(|s| s.to_string())
}

/// Check whether an archive's root folder matches the destination folder or source file stem.
pub fn is_matching_root(root: &str, dest_name: &str, src_stem: &str) -> bool {
    fn normalize(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    }

    let norm_root = normalize(root);
    let norm_dest = normalize(dest_name);
    let norm_stem = normalize(src_stem);

    if norm_root.is_empty() {
        return false;
    }

    norm_root == norm_dest
        || norm_root == norm_stem
        || (!norm_dest.is_empty()
            && (norm_dest.contains(&norm_root) || norm_root.contains(&norm_dest)))
        || (!norm_stem.is_empty()
            && (norm_stem.contains(&norm_root) || norm_root.contains(&norm_stem)))
}
