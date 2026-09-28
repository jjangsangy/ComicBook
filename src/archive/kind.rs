use clap::ValueEnum;
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

/// Target format for `comic-book convert --to` (see docs/convert.md).
///
/// A plain archive and its comic-book spelling (e.g. `zip`/`cbz`) share an
/// [`ArchiveKind`] but differ in the output extension, so both live here as
/// variants. `ValueEnum` keeps the accepted spellings in sync with the help text
/// and rejects anything else before the pipeline runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
pub enum ArchiveFormat {
    /// Comic-book ZIP (`.cbz`).
    Cbz,
    /// Plain ZIP (`.zip`).
    Zip,
    /// Comic-book RAR (`.cbr`).
    Cbr,
    /// Plain RAR (`.rar`).
    Rar,
    /// Comic-book 7-Zip (`.cb7`).
    Cb7,
    /// Plain 7-Zip (`.7z`).
    #[value(name = "7z")]
    SevenZ,
    /// Comic-book tar (`.cbt`).
    Cbt,
    /// Plain tar (`.tar`).
    Tar,
    /// Uncompressed directory of images.
    #[value(name = "dir")]
    Dir,
}

impl ArchiveFormat {
    /// The canonical output extension (`dir` is the accepted spelling for a
    /// directory target, whose output name carries no extension).
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveFormat::Cbz => "cbz",
            ArchiveFormat::Zip => "zip",
            ArchiveFormat::Cbr => "cbr",
            ArchiveFormat::Rar => "rar",
            ArchiveFormat::Cb7 => "cb7",
            ArchiveFormat::SevenZ => "7z",
            ArchiveFormat::Cbt => "cbt",
            ArchiveFormat::Tar => "tar",
            ArchiveFormat::Dir => "dir",
        }
    }

    /// The archive container this format writes.
    pub fn kind(self) -> ArchiveKind {
        match self {
            ArchiveFormat::Cbz | ArchiveFormat::Zip => ArchiveKind::Cbz,
            ArchiveFormat::Cbr | ArchiveFormat::Rar => ArchiveKind::Cbr,
            ArchiveFormat::Cb7 | ArchiveFormat::SevenZ => ArchiveKind::Cb7,
            ArchiveFormat::Cbt | ArchiveFormat::Tar => ArchiveKind::Cbt,
            ArchiveFormat::Dir => ArchiveKind::Directory,
        }
    }
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
    let format = ArchiveFormat::from_str(&clean, true).ok()?;
    Some((format.extension(), format.kind()))
}
