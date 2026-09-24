# AGENTS.md

Guidance for coding agents working in this repository. It captures the architecture of the
`convert` path, its measured performance/memory characteristics, and where the real wins are.

## Build, test, lint

```bash
cargo build --release          # binary at target/release/comic-book
cargo test                     # unit + integration tests
cargo clippy --all-targets --all-features
cargo fmt --check
```

All four must stay green. There are ~49 integration tests covering every conversion direction,
root-dir stripping, and extraction; add cases when you change conversion behaviour.

## Scope convention

- **Focus on the `convert` subcommand** when optimizing. The `clamp` subcommand is explicitly
  out of scope for these performance notes (it is CPU-bound and legitimately uses Rayon).
- **`convert` is I/O-bound. Do not parallelize it.** Rayon here would not raise throughput and
  would multiply peak memory (one scratch buffer per worker). Keep the conversion loop serial.

## Convert path architecture

```
cli::run → convert::run_convert
  collect_*_tasks()                 # directory scan, produces Vec<ConvertTask>
  execute_conversion_tasks()        # serial loop, ONE reused `scratch: Vec<u8>`
    archive::ops::convert_archive_ext_with_scratch()
      open_reader(src_kind, src)    # opened ONCE, reused for list + read
      reader.list_entries()         # only when target == Directory (root detection)
      reader.read_entries(scratch, cb)   # streams each entry into `scratch`
      writer.add_entry_normalized(...)   # name already normalized by the reader
```

Key invariants to preserve:

- **One reusable `scratch: Vec<u8>`** lives for the whole batch (in
  `convert.rs::execute_conversion_tasks`) and is threaded through
  `convert_archive_ext_with_scratch`. Peak memory is thus bounded by the **largest single
  entry**, not the archive or the batch. Do not allocate a fresh buffer per entry.
- Reader callbacks hand out **already-normalized** entry names. Use
  `ArchiveWriter::add_entry_normalized` inside the pipeline; `add_entry` (which normalizes) is
  only for external callers.
- The source reader is opened **once per conversion**; `list_entries` and `read_entries` share
  the same instance.
- Writers store, never recompress: ZIP uses `CompressionMethod::Stored`, 7z uses
  `EncoderMethod::COPY`, RAR uses `MemberCoding::Stored`. Comic images are already compressed.

## Measured characteristics

Indicative numbers from an 8-core / 8 GB Apple Silicon machine. Treat as *orders of magnitude*,
not precise benchmarks.

| Conversion | Input | Peak RSS |
|:---|:---|:---|
| `dir → cbz` | 12 MB | ~6 MB |
| `cbz → dir`, `cbz → cbt` | 77 MB | ~4 MB |
| `dir → cbr` | 12 MB | ~21 MB |
| `cbz → cbr` | 77 MB | **~88 MB** |

- **Peak RSS is bounded by the largest entry for every target except CBR/RAR**, which scales
  with the *total* archive size (see below).
- Per-entry overhead is ~5 µs and is dominated by `open`/`read`/tar-header syscalls, not
  allocations. Micro-optimizing per-entry allocations is measurable only in pathological cases.
- Wall-clock timing on a shared desktop is very noisy (10× run-to-run swings from page-cache and
  write effects). Do not claim a timing win without an interleaved A/B run and many repetitions.

## The one real memory problem: the CBR/RAR writer

`src/archive/formats/rar.rs::RarArchiveWriter` pushes
`EntrySource::from_bytes(Arc::<[u8]>::from(data))` for **every** member and only calls
`write_streaming_archive_to` at `finish()`. So it holds the entire uncompressed archive in RAM
(O(total size)) — the single place the convert path violates a memory-constrained environment
(1 GB comic ≈ 1 GB RSS). The `rars` streaming API was chosen specifically to avoid the old
high-level `Builder` that materialized the archive *twice*; this holds it once.

Fix options, best first:

1. **Directory sources → `EntrySource::from_path(source_file)`.** Zero-copy, no temp file, flat
   RAM. Requires plumbing an optional source path through the reader callback (`&[u8]` today).
   Best fix for the common `dir → cbr`.
2. **Spool entries to one temp file** and use `EntrySource::from_path`. Bounds RAM for all
   sources but reintroduces temp-file I/O, which the README explicitly avoids.
3. Document and accept.

## Changelog: most recent optimization pass

Low-risk hygiene changes only; **no measurable wall-clock or peak-memory delta** (verified by an
interleaved A/B against the prior commit, including a 6,000-entry corpus). They remove redundant
work and allocation churn:

- Normalize each entry path **once** (`ArchiveWriter::add_entry_normalized`); the reader already
  normalized it. Previously normalized twice per entry.
- Readers no longer allocate a throwaway `String` per entry (`file_entry.name().to_string()`,
  `entry.path()?.to_string_lossy().to_string()` were dropped).
- `convert_archive_ext_with_scratch` opens the source **once** and reuses the reader for both
  `list_entries` and `read_entries` (previously two opens).
- `SevenZipReader` keeps its opened `ArchiveReader` → one header parse instead of three.
- `TarReader` reads buffered and uses `entries_with_seek()` for name listing, so enumeration
  seeks over entry bodies instead of reading the whole tar.
- `find_single_root_dir` and the ZIP writer no longer collect a `Vec<&str>` per entry.
- `detect_archive_kind` is called once per top-level path.

## Prioritized backlog (larger impact)

1. **CBR/RAR writer memory** — implement the directory-source zero-copy path (see above).
2. **Release profile** — `Cargo.toml` has no `[profile.release]`. Adding `lto = "thin"` and
   `codegen-units = 1` typically gives a few-to-low-teens % globally. Needs a quieter benchmark
   than a shared desktop to substantiate.
3. **RAR *source* double-read** — for `cbr → dir`, `list_entries` (`header.skip()`) reads the
   archive body and `read_entries` reads it again. unrar has no seek-skip; this needs the
   root-detection strategy reworked, not a micro-fix.
4. **Task-collection walks** — `convert.rs` calls `contains_any_images` /
   `count_comic_subdirs`, which re-walk the same subtrees. Irrelevant to conversion speed but can
   cost seconds on large library trees; memoize if it matters.
5. **Deflate inflate for third-party CBZs** — CPU-bound and serial. Parallel *decompression* is
   the only place concurrency could help, but it fights the memory bound; use a bounded buffer
   pool if ever attempted.

## Benchmarking notes

- Build the baseline from `HEAD` read-only without disturbing `.git`:
  `mkdir -p target/bench/baseline && git archive HEAD | tar -x -C target/bench/baseline`, then
  build it with a separate `CARGO_TARGET_DIR`.
- Measure with `/usr/bin/time -l` (macOS) and read `maximum resident set size`.
- Interleave A/B runs to cancel drift; take the best of many; be suspicious of any single-run
  comparison. `target/` is gitignored, so benchmark scratch data there is fine to leave or delete.
