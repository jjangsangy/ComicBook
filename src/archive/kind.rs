use std::path::Path;

/// Supported comic book archive formats and directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveKind {
    Cbz,
    Cbr,
    Cb7,
    Cbt,
    Directory,
}

impl ArchiveKind {
    /// Returns the default file extension associated with this archive kind.
    #[allow(dead_code)]
    pub fn default_extension(&self) -> &'static str {
        match self {
            ArchiveKind::Cbz => "cbz",
            ArchiveKind::Cbr => "cbr",
            ArchiveKind::Cb7 => "cb7",
            ArchiveKind::Cbt => "cbt",
            ArchiveKind::Directory => "",
        }
    }

    /// Returns `true` if this kind represents a compressed archive format rather than a directory.
    #[allow(dead_code)]
    pub fn is_archive(&self) -> bool {
        !matches!(self, ArchiveKind::Directory)
    }
}

/// Detect the archive kind of a given path.
///
/// If `path` is a directory, returns `Some(ArchiveKind::Directory)`.
/// For files, magic bytes are inspected using the `infer` library.
pub fn detect_archive_kind<P: AsRef<Path>>(path: P) -> Option<ArchiveKind> {
    let p = path.as_ref();
    if p.is_dir() {
        return Some(ArchiveKind::Directory);
    }
    let kind = infer::get_from_path(p).ok().flatten()?;
    detect_archive_kind_from_type(&kind)
}

/// Detect the archive kind from an `infer::Type`.
pub fn detect_archive_kind_from_type(kind: &infer::Type) -> Option<ArchiveKind> {
    match kind.extension() {
        "zip" => Some(ArchiveKind::Cbz),
        "rar" => Some(ArchiveKind::Cbr),
        "7z" => Some(ArchiveKind::Cb7),
        "tar" => Some(ArchiveKind::Cbt),
        _ => match kind.mime_type() {
            "application/zip" => Some(ArchiveKind::Cbz),
            "application/vnd.rar" | "application/x-rar-compressed" => Some(ArchiveKind::Cbr),
            "application/x-7z-compressed" => Some(ArchiveKind::Cb7),
            "application/x-tar" => Some(ArchiveKind::Cbt),
            _ => None,
        },
    }
}

/// Detect the archive kind from raw bytes using magic bytes matching.
pub fn detect_archive_kind_from_bytes(bytes: &[u8]) -> Option<ArchiveKind> {
    let kind = infer::get(bytes)?;
    detect_archive_kind_from_type(&kind)
}

/// Parse a target extension or format name into its canonical extension string and `ArchiveKind`.
pub fn parse_target_extension(ext: &str) -> Option<(&'static str, ArchiveKind)> {
    let clean = ext.trim_start_matches('.').to_ascii_lowercase();
    match clean.as_str() {
        "cbz" => Some(("cbz", ArchiveKind::Cbz)),
        "zip" => Some(("zip", ArchiveKind::Cbz)),
        "cbr" => Some(("cbr", ArchiveKind::Cbr)),
        "rar" => Some(("rar", ArchiveKind::Cbr)),
        "cb7" => Some(("cb7", ArchiveKind::Cb7)),
        "7z" => Some(("7z", ArchiveKind::Cb7)),
        "cbt" => Some(("cbt", ArchiveKind::Cbt)),
        "tar" => Some(("tar", ArchiveKind::Cbt)),
        "dir" => Some(("dir", ArchiveKind::Directory)),
        _ => None,
    }
}
