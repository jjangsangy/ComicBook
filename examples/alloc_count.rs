//! Development aid: count heap allocations for one `ebook` conversion.
//!
//! Installs a counting global allocator, parses a minimal `ebook` command line
//! from its arguments, and reports the allocation calls, the total bytes
//! requested and the peak live heap over the conversion. Used to check that a
//! change does not raise memory churn (not just peak RSS).
//!
//! Any arguments after the output directory are forwarded to the `ebook`
//! subcommand, so a scenario (e.g. `--force-png`, `--webtoon`, `-f pdf`) can be
//! measured by comparing the baseline and current binaries with the same flags.
//!
//! ```sh
//! cargo build --release --example alloc_count
//! ./target/release/examples/alloc_count target/bench/bench.cbz target/bench/out_alloc
//! ./target/release/examples/alloc_count target/bench/bench.cbz target/bench/out_png --force-png
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::Parser;

/// A `System` allocator that records every request.
struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK_LIVE: AtomicUsize = AtomicUsize::new(0);

fn record(size: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(size, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK_LIVE.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every method forwards to `System` with the same layout it was given,
// after updating the counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: alloc_count <input> <out-dir> [ebook args...]")?;
    let outdir = args
        .next()
        .ok_or("usage: alloc_count <input> <out-dir> [ebook args...]")?;
    let extra: Vec<String> = args.collect();

    let mut argv: Vec<String> = vec![
        "comic-book".into(),
        "ebook".into(),
        input,
        "-o".into(),
        outdir,
    ];
    // Default to EPUB unless the forwarded flags pick a format themselves.
    if !extra.iter().any(|arg| arg == "-f" || arg == "--format") {
        argv.push("-f".into());
        argv.push("epub".into());
    }
    argv.extend(extra);
    let cli = comic_book::cli::Cli::try_parse_from(argv)?;
    let ebook = match cli.command {
        comic_book::cli::Commands::Ebook(args) => args,
        _ => return Err("expected the ebook subcommand".into()),
    };

    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    let baseline_live = LIVE.load(Ordering::Relaxed);

    comic_book::ebook::run_ebook(ebook)?;

    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocations;
    let bytes = BYTES.load(Ordering::Relaxed) - bytes;
    let peak_live = PEAK_LIVE.load(Ordering::Relaxed);
    println!("allocations: {allocations}");
    println!("allocated MiB: {:.1}", bytes as f64 / (1024.0 * 1024.0));
    println!("peak live MiB: {:.1}", peak_live as f64 / (1024.0 * 1024.0));
    println!("baseline live KiB: {:.0}", baseline_live as f64 / 1024.0);
    Ok(())
}
