//! Per-page image processing pipeline (AGENTS.md §11).
//!
//! This module orchestrates the parallel pass over a [`ComicTree`](super::model::ComicTree);
//! the individual algorithms live in the sibling modules.

pub mod color;
pub mod cover;
pub mod crop;
pub mod fill;
pub mod interpanel;
pub mod page;
pub mod rainbow;
pub mod webtoon;
