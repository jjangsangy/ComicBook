//! Comic book archive extraction, conversion, creation, and inspection.
//!
//! Provides support for:
//! - CBZ (ZIP)
//! - CBR (RAR 4.0)
//! - CB7 (7-Zip)
//! - CBT (TAR)
//! - Plain folders/directories

pub mod formats;
pub mod kind;
pub mod ops;
pub mod path;
pub mod reader;
pub mod writer;

// Re-export core types and functions for ergonomic, backwards-compatible usage
pub use formats::{
    DirectoryArchiveWriter, DirectoryReader, RarArchiveWriter, RarReader, SevenZipArchiveWriter,
    SevenZipReader, TarArchiveWriter, TarReader, ZipArchiveWriter, ZipReader,
};
pub use kind::{
    detect_archive_kind, detect_archive_kind_from_bytes, detect_archive_kind_from_type,
    parse_target_extension, ArchiveKind,
};
pub use ops::{
    compress_archive, convert_archive, convert_archive_ext, extract_archive,
    get_images_from_source, list_archive_entry_names, read_archive_entries,
};
pub use path::{
    copy_dir_all, find_single_root_dir, is_matching_root, normalize_archive_path, safe_join,
};
pub use reader::{open_reader, ArchiveReader, EntryCallback};
pub use writer::ArchiveWriter;
