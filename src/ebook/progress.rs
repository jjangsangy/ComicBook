//! Progress reporting (indicatif), safe to run without an interactive terminal.
//!
//! Progress bars render straight to stderr. When stderr is not a terminal — the
//! normal case under `cargo nextest`, CI, or a pipe — every bar is hidden so it
//! emits nothing. Setting `COMIC_BOOK_QUIET` hides them unconditionally.

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Environment variable that forces progress output off.
pub const QUIET_ENV: &str = "COMIC_BOOK_QUIET";

/// Whether interactive progress output should be rendered.
pub fn progress_enabled() -> bool {
    use std::io::IsTerminal;
    std::env::var_os(QUIET_ENV).is_none() && std::io::stderr().is_terminal()
}

/// A primary progress bar with `len` steps, or a hidden bar when not interactive.
pub fn bar(len: u64, message: impl Into<String>) -> ProgressBar {
    if !progress_enabled() {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::new(len);
    bar.set_style(bar_style());
    bar.set_message(message.into());
    bar
}

/// The primary bar style. The template is a constant we know parses; on the
/// (impossible) template error, fall back to indicatif's default style rather
/// than panicking.
fn bar_style() -> ProgressStyle {
    match ProgressStyle::default_bar().template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.green/blue}] {pos}/{len} ({eta}) {msg}",
    ) {
        Ok(style) => style.progress_chars("#->"),
        Err(_) => ProgressStyle::default_bar(),
    }
}

/// A spinner (indeterminate) progress bar, or a hidden bar when not interactive.
pub fn spinner(message: impl Into<String>) -> ProgressBar {
    if !progress_enabled() {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::new_spinner();
    bar.set_style(spinner_style());
    bar.set_message(message.into());
    bar
}

/// The spinner style, with the same constant-template fallback as [`bar_style`].
fn spinner_style() -> ProgressStyle {
    match ProgressStyle::default_spinner().template("{spinner:.green} {msg}") {
        Ok(style) => style,
        Err(_) => ProgressStyle::default_spinner(),
    }
}

/// A [`ProgressBar`] that never draws, for headless callers.
pub fn hidden() -> ProgressBar {
    ProgressBar::with_draw_target(None, ProgressDrawTarget::hidden())
}

/// Print a warning to stderr, unless `COMIC_BOOK_QUIET` is set.
///
/// Warnings are diagnostic rather than progress, so they are not suppressed when
/// stderr is not a terminal (KCC prints its `detectSuboptimalProcessing` warnings
/// on every run); `COMIC_BOOK_QUIET` still silences them for scripted callers.
pub fn warn(message: &str) {
    if std::env::var_os(QUIET_ENV).is_some() {
        return;
    }
    eprintln!("{message}");
}
