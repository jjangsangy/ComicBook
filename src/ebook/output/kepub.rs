//! KePub variant (`.kepub.epub` + Kobo spread properties).
//!
//! KePub is not a separate package: it is the same fixed-layout EPUB built by
//! [`super::epub::build_epub`], with two differences that `Options::resolve` and
//! the builders already apply — the `.kepub.epub` output extension
//! ([`crate::ebook::naming::output_filename`]) and the `rendition:page-spread-*`
//! spine properties the Kobo reader understands (see docs/output.md).

// No code lives here: see the module documentation above.
