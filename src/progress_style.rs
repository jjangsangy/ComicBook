//! Shared `indicatif` bar/spinner styles (Phase 8 of the type-safety refactor,
//! finding G5; see `docs/refactor.md`).
//!
//! `ProgressStyle::default_bar().template(..)` with a constant-template fallback was
//! copied across `convert`, `clamp` and `ebook`. The templates themselves differ per
//! site (colours, spinner vs bar), so only the fallback idiom is shared; each caller
//! still passes its own template and progress characters.

use indicatif::ProgressStyle;

/// A bar style from the constant `template`, falling back to indicatif's default bar
/// style on the (impossible) template error rather than panicking.
pub(crate) fn bar(template: &str) -> ProgressStyle {
    match ProgressStyle::default_bar().template(template) {
        Ok(style) => style,
        Err(_) => ProgressStyle::default_bar(),
    }
}

/// A bar style from the constant `template` with `chars` as its progress characters.
pub(crate) fn bar_with_chars(template: &str, chars: &str) -> ProgressStyle {
    match ProgressStyle::default_bar().template(template) {
        Ok(style) => style.progress_chars(chars),
        Err(_) => ProgressStyle::default_bar(),
    }
}

/// A spinner style from the constant `template`, with the same fallback.
pub(crate) fn spinner(template: &str) -> ProgressStyle {
    match ProgressStyle::default_spinner().template(template) {
        Ok(style) => style,
        Err(_) => ProgressStyle::default_spinner(),
    }
}
