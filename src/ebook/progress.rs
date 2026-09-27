//! Progress reporting (indicatif), safe to run without an interactive terminal.
//!
//! Progress bars render straight to stderr. When stderr is not a terminal — the
//! normal case under `cargo nextest`, CI, or a pipe — every bar is hidden so it
//! emits nothing. Setting `COMIC_BOOK_QUIET` hides them unconditionally.

use std::cell::RefCell;

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressFinish, ProgressStyle};

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

/// A bar for attaching to a [`MultiProgress`], built on a hidden draw target.
///
/// `set_message` draws immediately, so building on the default stderr target would paint
/// a frame before the bar is attached — a frame the multi does not track, leaving a stale
/// line behind and desynchronising the multi's cursor accounting. Building hidden
/// suppresses that draw; `MultiProgress::add`/`insert_after` re-target the bar once built.
fn bar_for_multi(len: u64, message: String) -> ProgressBar {
    let bar = ProgressBar::with_draw_target(Some(len), ProgressDrawTarget::hidden());
    bar.set_style(bar_style());
    bar.set_message(message);
    bar
}

/// A bare bar that renders one line of text, for a status line drawn *below* the
/// progress bars.
///
/// [`MultiProgress`] can only print free text *above* its bars
/// ([`MultiProgress::println`]); to place a line *below* the bars it has to be
/// attached as a bar. The `{msg}` style renders just the message, so the bar looks
/// like a plain line, and `AndLeave` keeps it on screen when the `MultiProgress` is
/// dropped (the default `AndClear` would wipe every status line at the end of the run).
///
/// The message is not set here: it must be set *after* the bar is attached, so the
/// draw it triggers reaches the [`MultiProgress`] (a bar that never draws contributes
/// no line). See [`Reporter::push_line`].
fn line_bar() -> ProgressBar {
    let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::hidden())
        .with_finish(ProgressFinish::AndLeave);
    bar.set_style(line_style());
    bar
}

/// The status-line style, with the same constant-template fallback as [`bar_style`].
fn line_style() -> ProgressStyle {
    match ProgressStyle::default_bar().template("{msg}") {
        Ok(style) => style,
        Err(_) => ProgressStyle::default_bar(),
    }
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

/// Progress for a batch of files: an overall bar plus the per-file bars below it.
///
/// The `ebook` pipeline reports two levels of progress. A single [`bar`] tracks the
/// pages of one file; a run over several inputs additionally shows how many files
/// are done. Parent and child bars must share one [`MultiProgress`] or their
/// redraws clobber each other, so callers pass a `Reporter` down the pipeline
/// rather than creating bars ad hoc — the same overall-plus-children layout
/// [`crate::clamp`] uses for its per-book bars.
///
/// A `Reporter` takes one of three shapes:
/// - [`Reporter::batch`] draws the overall bar and nests each [`Reporter::child`]
///   below it;
/// - [`Reporter::standalone`] draws no overall bar and returns top-level children
///   (the single-file behaviour);
/// - when output is not interactive ([`progress_enabled`] is false) every bar is
///   hidden.
///
/// Status lines ([`Reporter::println`] and [`Reporter::warn`]) are drawn *below* the
/// bars, so the bars hold their place while the text grows downward.
pub struct Reporter {
    multi: Option<MultiProgress>,
    overall: Option<ProgressBar>,
    /// Status lines attached below the bars, kept alive so the [`MultiProgress`]
    /// keeps drawing them. Empty outside batch mode.
    lines: RefCell<Vec<ProgressBar>>,
}

impl Reporter {
    /// A reporter for one file: [`Reporter::child`] returns a top-level [`bar`] and
    /// there is no overall bar.
    pub fn standalone() -> Self {
        Self {
            multi: None,
            overall: None,
            lines: RefCell::new(Vec::new()),
        }
    }

    /// A reporter for a batch of `total` files, with an overall bar labelled
    /// `message`.
    pub fn batch(total: u64, message: impl Into<String>) -> Self {
        if !progress_enabled() {
            return Self::standalone();
        }
        let multi = MultiProgress::new();
        let overall = multi.add(bar_for_multi(total, message.into()));
        Self {
            multi: Some(multi),
            overall: Some(overall),
            lines: RefCell::new(Vec::new()),
        }
    }

    /// A per-file bar: nested under the overall bar in batch mode, top-level
    /// otherwise.
    pub fn child(&self, len: u64, message: impl Into<String>) -> ProgressBar {
        match (&self.multi, &self.overall) {
            (Some(multi), Some(overall)) => {
                multi.insert_after(overall, bar_for_multi(len, message.into()))
            }
            _ => bar(len, message),
        }
    }

    /// Record one completed file on the overall bar (a no-op without one).
    pub fn inc(&self) {
        if let Some(overall) = &self.overall {
            overall.inc(1);
        }
    }

    /// Clear the overall bar and settle the status lines once the batch is finished
    /// (a no-op without an overall bar).
    pub fn finish(&self) {
        if let Some(overall) = &self.overall {
            overall.finish_and_clear();
        }
        for line in self.lines.borrow().iter() {
            line.finish();
        }
    }

    /// Print a status line *below* the bars, keeping the bars in place.
    ///
    /// A plain `println!` between two draws leaves the `MultiProgress` redraw one line
    /// too low and stacks stale frames, so status lines must go through the multi.
    /// [`MultiProgress::println`] would draw them *above* the bars, pushing the bars
    /// down on every line; [`Reporter::push_line`] attaches them below instead.
    ///
    /// Because multi bars render to the draw target (stderr), an interactive
    /// multi-file run prints result lines to stderr; the headless path (redirected
    /// output) has no bars and keeps using stdout.
    pub fn println(&self, message: impl std::fmt::Display) {
        match &self.multi {
            Some(multi) => self.push_line(multi, message.to_string()),
            None => println!("{message}"),
        }
    }

    /// Report a warning: below the bars in batch mode, on stderr otherwise.
    pub fn warn(&self, message: &str) {
        match &self.multi {
            Some(multi) => self.push_line(multi, message.to_string()),
            None => warn(message),
        }
    }

    /// Append `message` as a status line *below* the bars.
    ///
    /// [`MultiProgress::println`] draws a line *above* the bars, shoving the bars down
    /// the screen on every call. To keep the bars in place and let the text run
    /// downward, each line is attached as a trailing bar instead ([`line_bar`]). The
    /// message is set after attaching so the draw it triggers reaches the multi; the
    /// line is then retained so the multi keeps drawing it.
    fn push_line(&self, multi: &MultiProgress, message: String) {
        let line = multi.add(line_bar());
        line.set_message(message);
        self.lines.borrow_mut().push(line);
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
