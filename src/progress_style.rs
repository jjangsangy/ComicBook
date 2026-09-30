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

#[cfg(test)]
mod tests {
    use super::*;

    // `ProgressStyle` exposes neither a template accessor nor `PartialEq`, so once the
    // parser's verdict is pinned the helpers can only be observed as "does not panic".

    #[test]
    fn constant_templates_are_accepted() {
        // Representative of the constant templates the CLI passes. Pinning that the
        // parser accepts them shows the helpers take their `Ok` arm, so calling them
        // is not merely a smoke test.
        for template in ["", "{msg}", "{msg} {pos}/{len}"] {
            assert!(ProgressStyle::default_bar().template(template).is_ok());
            assert!(ProgressStyle::default_spinner().template(template).is_ok());
        }
        bar("{msg}");
        bar_with_chars("{msg}", "#->");
        spinner("{msg}");
    }

    #[test]
    fn an_invalid_template_falls_back_without_panicking() {
        // A lone `}` puts indicatif's parser into its double-close state; a following
        // non-`}` character is rejected, so this exercises the `Err` fallback arms
        // (unlike a bare `{`, which ends in the parser's `MaybeOpen` state and is Ok).
        let template = "}x";
        assert!(ProgressStyle::default_bar().template(template).is_err());
        assert!(ProgressStyle::default_spinner().template(template).is_err());
        bar(template);
        bar_with_chars(template, "#->");
        spinner(template);
    }
}
