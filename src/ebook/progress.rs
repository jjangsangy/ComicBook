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
    bar.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.green/blue}] {pos}/{len} ({eta}) {msg}",
            )
            .expect("valid template")
            .progress_chars("#>-"),
    );
    bar.set_message(message.into());
    bar
}

/// A spinner (indeterminate) progress bar, or a hidden bar when not interactive.
pub fn spinner(message: impl Into<String>) -> ProgressBar {
    if !progress_enabled() {
        return ProgressBar::hidden();
    }
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid template"),
    );
    bar.set_message(message.into());
    bar
}

/// A [`ProgressBar`] that never draws, for headless callers.
pub fn hidden() -> ProgressBar {
    ProgressBar::with_draw_target(None, ProgressDrawTarget::hidden())
}
