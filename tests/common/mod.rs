//! Shared helpers for the integration tests.
//!
//! `indicatif` progress bars draw straight to the stderr file descriptor,
//! bypassing the capture libtest installs around `println!`/`eprintln!`. Without
//! this, `cargo test` is littered with half-rendered bar frames interleaved with
//! the harness's own output.
//!
//! The wrappers below point fd 2 at `/dev/null` (once per test binary) before
//! calling into the library entry points that render progress. Normal
//! `println!`/`eprintln!` capture is unaffected, so a failing test still shows
//! whatever its code printed.

use std::path::{Path, PathBuf};

use anyhow::Result;
use comic_book::clamp::Approach;

/// Discards everything written directly to stderr for the rest of the process.
///
/// Idempotent and safe to call from every test; only the first call does work.
#[cfg(unix)]
fn silence_progress_bars() {
    use std::os::fd::AsRawFd;
    use std::sync::Once;

    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dev_null = std::fs::File::open("/dev/null").expect("failed to open /dev/null");
        // SAFETY: `dev_null` is a valid open fd, and `dup2` atomically makes fd 2
        // a duplicate of it. Closing `dev_null` afterwards leaves fd 2 open.
        let redirected = unsafe { libc::dup2(dev_null.as_raw_fd(), libc::STDERR_FILENO) };
        assert!(redirected >= 0, "failed to redirect stderr to /dev/null");
    });
}

/// Non-Unix builds keep their stderr; progress bars are only cosmetic in tests.
#[cfg(not(unix))]
fn silence_progress_bars() {}

/// Silences progress output, then converts archives. See module docs.
pub fn run_convert(paths: &[PathBuf], target_ext: &str) -> Result<()> {
    silence_progress_bars();
    comic_book::convert::run_convert(paths, target_ext)
}

/// Silences progress output, then clamps images. See module docs.
pub fn run_clamp(
    input_path: &Path,
    output_dir: &Path,
    size_threshold: u64,
    approach: Approach,
    num_workers: usize,
) -> Result<()> {
    silence_progress_bars();
    comic_book::clamp::run_clamp(
        input_path,
        output_dir,
        size_threshold,
        approach,
        num_workers,
    )
}
