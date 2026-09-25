//! Input adapters: turn each supported source into a [`ComicTree`](super::model::ComicTree).
//!
//! Archives and folders arrive in Phase 1; EPUB and PDF inputs in Phase 11.

pub mod archive;
pub mod epub;
pub mod fusion;
pub mod pdf;
