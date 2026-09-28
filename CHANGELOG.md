# Changelog

All notable changes to `comic-book` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are
reconstructed from the repository's release tags (`v*`); each release links to the diff
since the previous tag. The release workflow dates the `Unreleased` section into a tagged
release when a version tag is pushed.

## [Unreleased]

### Changed

- Hardened the `archive` public API so that invalid archive states are unrepresentable (the
  archive/path step of the type-safety refactor tracked in `REFACTOR.md`):
  - `normalize_archive_path` returns `Option<NormalizedArchivePath>` instead of a `String` with an
    empty-string sentinel, and `parse_entry_info`/`list_archive_entry_names` return an
    `ArchiveEntry { name, kind }` whose `EntryKind` is `File` or `Directory` (no positional `bool`).
  - `read_archive_entries` and `ArchiveReader::read_entries` hand their callback an
    `EntryContent::{Directory, File(&[u8])}`, so a directory can no longer be spelled as an empty
    file; `ArchiveWriter::add_entry`/`add_entry_normalized` take the same `EntryContent`.
  - `convert_archive_ext` takes a `RootStripPolicy` (`Never`/`Always`/`IfMatchingDestination`)
    instead of a `strip_root: bool`.
  - `get_images_from_source` returns `Vec<DecodedImage>` (`name: BaseName`, `image`) instead of
    `Vec<(String, DynamicImage)>`.

### Fixed

- `scripts/set-version.sh` and `scripts/set-version.ps1` (with `--changelog` / `-Changelog`) now
  re-open an empty `## [Unreleased]` heading above the dated release, so the default branch keeps a
  section for the next release's entries instead of losing it on roll-over.

## [0.2.7] - 2026-09-27

### Added

- A second `Overall Progress` bar for multi-input `ebook` runs: it tracks how many of the input
  files have been converted and sits above each file's per-file page bar. Both bars share one
  `indicatif::MultiProgress` through the new `ebook::progress::Reporter`, and status lines
  (`Created …` and warnings) are drawn below the bars as trailing lines, so the bars hold their
  place while the text grows downward. A single-input run keeps just its page bar, since that
  already is its overall progress. Recorded as a conscious deviation from KCC in `docs/porting.md`.

### Docs

- Documented the `ebook` overall progress bar in `docs/cli.md` and `docs/porting.md`.
- Documented the requirement to record every user-visible change under `## [Unreleased]` in
  `AGENTS.md`, `CONTRIBUTING.md` and `docs/development.md`.

## [0.2.6] - 2026-09-27

### Changed

- The release workflow now fills each GitHub Release body with the changelog for the release's
  whole major.minor line (`scripts/changelog-notes.sh`), so a `0.2.6` release page also carries the
  0.2.5, 0.2.4 … entries as well as its own, followed by GitHub's generated notes.
- `scripts/set-version.sh` and `scripts/set-version.ps1` accept `--changelog` (`-Changelog`),
  which dates the changelog's top `## [Unreleased]` section as the released version and updates
  its compare link references. The release workflow passes it only in the job that commits the
  version to the default branch, so the per-target build jobs keep stamping a throwaway checkout.

### Docs

- The full `comic-book ebook --help` reference is now reproduced in `README.md`.

## [0.2.5] - 2026-09-26

### Changed

- Vectorised the per-pixel hotspots of the `ebook` image pipeline. The scalar, allocating helpers
  (`imageproc::map::map_pixels` allocated a `Vec` *per pixel*, and dominated the profile) are
  replaced by the `wide`-based SIMD kernels in `src/ebook/processing/kernels.rs`: luma min/max,
  planar inversion and thresholding, the 3-tap box blur, bounding boxes/rectangle counts, row and
  column emptiness and the autocontrast stretch LUT. Every kernel is bit-identical to the scalar
  code it replaces (integer arithmetic only).
- Removed redundant image copies across the pipeline: grayscale planes are borrowed instead of
  cloned (`color::luma_view`), the resizer reads straight from the source buffer via
  `fast_image_resize::images::ImageRef`, contrast LUTs are applied in place, already-RGB buffers
  are borrowed for WebP/PNG/GIF encoding and `into_rgb8` replaces `to_rgb8` where ownership is
  available.
- Removed the remaining whole-buffer copies on the non-default paths: the moiré eraser borrows an
  existing RGB/L8 plane instead of converting it, a short webtoon strip is moved into its page, a
  light-novel page that already fits is moved into the archive instead of cloned, the cover
  thumbnail is moved when it already fits, and PDF verbatim JPEGs are borrowed through a `Cow`
  rather than copied per page.
- Threaded ownership through the crop preparation: the grayscale plane is inverted, autocontrasted
  and box-blurred **in place** (`autocontrast_cutoff_in_place`, `kernels::box_blur_1_in_place`)
  instead of allocating a fresh full-image copy at each pass, and the EPUB spine walk shares each
  chosen image with `Arc` rather than copying it out of the container map.
- Added the `wide` dependency (already transitively present via `quantette`) and the
  `examples/gen_bench.rs` input generator plus `scripts/bench.sh` / `scripts/flamegraph.py`
  profiling aids, and the `examples/alloc_count.rs` allocator counter with
  `scripts/memory_bench.sh` for baseline-vs-worktree memory comparisons.

### Docs

- Documented the SIMD kernels and the float-fidelity rule that keeps the Rec.601 grayscale and
  `colorCheck` weights scalar in `docs/processing.md`, the dependency in `docs/dependencies.md`,
  and the profiling workflow in `docs/development.md`.

## [0.2.4] - 2026-09-26

Release-automation release: the version now comes from the pushed tag instead of a
hand-bumped manifest.

### Added

- `scripts/set-version.sh` and `scripts/set-version.ps1`, which stamp `Cargo.toml`'s `[package]
  version` from a release tag (validating it as SemVer first, and rewriting only the manifest's
  `[package]` entry).

### Build

- The release workflow validates the pushed tag as a SemVer version, stamps `Cargo.toml`'s
  package version — and the matching `Cargo.lock` entry — from it before building, and then
  commits the same stamp back to the default branch. Binaries attached to a release now report
  that release's version from `comic-book --version`, and `main` always declares the last
  released version, instead of both drifting from a hand-bumped manifest value.

### Fixed

- Re-synced the committed `Cargo.lock` with `Cargo.toml` (both now declare `0.2.3`), so a
  `--locked` build on `main` no longer fails.

### Docs

- Updated `CONTRIBUTING.md` and `docs/development.md` for tag-driven versioning.

## [0.2.3] - 2026-09-26

Version-metadata release. The `v0.2.3` tag points at the same commit as `v0.2.2`, and the
subsequent `main` commit bumps the crate version to `0.2.3`.

- Note: the `v0.2.1`, `v0.2.2` and `v0.2.3` tags all still declare `version = "0.2.0"` in
  `Cargo.toml`, so a binary built from those tags reports `0.2.0`. Only the follow-up
  commit on `main` sets the manifest to `0.2.3`.

## [0.2.2] - 2026-09-26

Reduce the `ebook` pipeline's peak memory use and make output naming deterministic.

### Changed

- `ebook` pages are now loaded lazily and borrowed rather than cloned throughout the
  pipeline (input adapters, model, processing, and the EPUB/Kindle/light-novel builders),
  so peak RSS is linear in the *encoded* book rather than the decoded one. Large-book tests
  act as memory-regression guards.
- Removed the `_kcc<N>` filename-collision counter. Output names are now deterministic and
  overwrite existing files instead of being suffixed (`book_kcc0.epub`, …).

### Docs

- Updated `docs/architecture.md`, `docs/development.md`, `docs/porting.md` and
  `docs/output.md` for the borrowing model and the deterministic-naming deviation.

### Tests

- Expanded robustness, processing, input, naming and Kindle test suites.

## [0.2.1] - 2026-09-26

### Added

- `--kepub-short-ext` to emit a shortened `.kepub` extension instead of `.kepub.epub`;
  rejected with a clear error unless KePub output is active.

### Changed

- Device-profile help output and generated shell completions now display the device name as
  well as the profile code.

### Docs

- Documented the shortened KePub extension in `docs/cli.md` and `docs/output.md`.

### Tests

- Added naming and CLI tests covering `--kepub-short-ext` and profile display.

## [0.2.0] - 2026-09-26

Feature-complete `ebook` release. The `ebook` subcommand (see `0.2.0-rc.1`) is considered
done; this tag adds an infrastructure update only.

### Build

- Updated GitHub Actions runners off deprecated versions in the CI and release workflows.

## [0.2.0-rc.1] - 2026-09-26

First pre-release of the `ebook` subcommand: an independent, pure-Rust reimplementation of
Kindle Comic Converter's `kcc-c2e` comic-to-ebook pipeline (see `docs/porting.md`).

### Added

- `ebook` subcommand with device profiles (Kindle, Kobo, reMarkable, generic) and an `auto`
  output format that maps MOBI to Kindle profiles, PDF to reMarkable, and EPUB otherwise.
- Output formats: fixed-layout **EPUB 3**, **KePub**, **AZW3**, dual **MOBI7 + KF8 MOBI**,
  **PDF**, repackaged **CBZ**, a light-novel layout, and a `kfx`-as-EPUB preset, plus
  size-capped `-200mb` presets.
- Input adapters for `.cbz`/`.zip`, `.cbr`/`.rar`, `.cb7`/`.7z`, `.cbt`/`.tar` archives and
  image folders, plus secondary **EPUB** (spine-ordered images) and **PDF** input (embedded
  image extraction and pure-Rust vector-page rasterisation).
- Image processing: colour handling (grayscale/palette quantisation and Floyd–Steinberg
  dithering), gamma/contrast/resize, fill, and page splitting.
- Cropping and enhancement: margins, page-number removal, inter-panel cropping and the
  `--erase-rainbow` moiré eraser (2D-FFT luminance filter).
- Panel View variants, Scribe strips, spread options, and webtoon (`comic2panel`) mode.
- `ComicInfo.xml` metadata parsing (lenient where KCC fails), KCC-compatible naming and
  slugification, `--file-fusion`, name-collision handling, size-based tome chunking, and
  `--delete`/`--temp-dir` handling.
- Cover generation with the KCC `N/M` tome label drawn via the bundled `font8x8` bitmap
  font.
- A full `docs/` reference: `README.md`, `architecture.md`, `clamp.md`, `cli.md`,
  `convert.md`, `dependencies.md`, `development.md`, `output.md`, `porting.md` and
  `processing.md`.

### Changed

- `ebook` is compiled entirely into the binary — archive extraction, image processing,
  EPUB/PDF/MOBI authoring and PDF rasterisation require no external programs.
- The generated EPUB/KePub documents (OPF/NCX/NAV/XHTML) are rendered from Askama templates
  compiled into Rust, replacing hand-rolled string assembly; a malformed template is now a
  compile error.
- Added `ebook` dependencies (`imageproc`, `quantette`, `png`, `rustfft`, `bitvec`,
  `quick-xml`, `slug`, `regex`, `uuid`, `time`, `askama`, `pdf-writer`, `flate2`,
  `kindling-mobi`, `font8x8`, `pdfboss-render`, `pdfboss-core`) and declared **MSRV 1.93**.
- Enabled stricter lints: `clippy::unwrap_used`, `clippy::expect_used` and `clippy::panic`
  are denied, with `rust::unused_must_use` denied.
- Rewrote `README.md` and `AGENTS.md` for the expanded command set and documentation layout.

### Changed — `clamp`

- Cleaned up the `clamp` implementation and its internal naming, with substantially more
  integration coverage.

### Tests

- Added `ebook` suites for EPUB, Kindle, chunking, cropping, EPUB/PDF input, naming, output,
  processing, robustness (malformed inputs must error, never panic) and webtoon, with golden
  OPF/NCX/NAV/XHTML fixtures.
- Adopted `cargo-nextest`; CI now skips slow tests and no longer fails fast across the
  matrix.

## [0.1.0] - 2026-09-24

First stable `convert`/`clamp` release.

### Changed

- Improved the installers: SHA-256 checksum verification, `PATH` hints, and `--version` /
  `--install-dir` options for `scripts/install.sh` and `scripts/install.ps1`.

## [0.1.0-rc.6] - 2026-09-24

### Fixed

- Further fixes to the Linux `aarch64` release build configuration.

## [0.1.0-rc.5] - 2026-09-24

### Fixed

- Fixed the Linux `aarch64` release build by adding `.cargo/config.toml` and the
  `CXXFLAGS_*` (`-DLITTLE_ENDIAN -include cstddef`) needed to compile `unrar`'s vendored C++
  on that target.

## [0.1.0-rc.4] - 2026-09-24

### Fixed

- Restored the package version to valid SemVer (`0.1.0`).

## [0.1.0-rc.3] - 2026-09-24

### Changed

- Simplified the release workflow and trimmed `CONTRIBUTING.md`.

## [0.1.0-rc.2] - 2026-09-24

### Changed

- Release-metadata adjustments to the package version.

## [0.1.0-rc.1] - 2026-09-24

First tagged pre-release of the initial `comic-book` CLI.

### Added

- `convert` — repackage comic archives and directory trees between `.cbz`/`.zip`,
  `.cbr`/`.rar`, `.cb7`/`.7z`, `.cbt`/`.tar` and plain directories, with directory expansion
  and in-memory streamed conversion.
- `clamp` — rewrite oversized pages under a size threshold using the `split`, `resize`
  (Lanczos3) or `max-width` approach, with WebP output, case-insensitive natural page
  ordering, and Rayon-backed parallel processing with `indicatif` progress bars.
- `completions` — shell completion generation for Bash, Zsh, Fish, PowerShell and Elvish.
- Pure-Rust archive backends (`zip`, `unrar`/`rars`, `sevenz-rust2`, `tar`) selected at
  runtime by content sniffing; no external programs are required.
- Project scaffolding: `README.md`, `CONTRIBUTING.md`, issue and pull-request templates,
  `LICENSE` and `.gitignore`.
- `scripts/install.sh` and `scripts/install.ps1` installers.
- CI plus a release workflow that builds and attaches prebuilt binaries for macOS
  (Apple Silicon and Intel), Linux (x86_64 and aarch64, static musl) and Windows (x86_64).

### Changed

- Reduced memory usage in archive handling and applied micro-optimisations across the
  readers, writers and conversion paths.

### Fixed

- Corrected a badge link typo in `README.md`.

<!-- Links -->

[Unreleased]: https://github.com/jjangsangy/ComicBook/compare/v0.2.7...HEAD
[0.2.7]: https://github.com/jjangsangy/ComicBook/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/jjangsangy/ComicBook/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/jjangsangy/ComicBook/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/jjangsangy/ComicBook/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/jjangsangy/ComicBook/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/jjangsangy/ComicBook/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/jjangsangy/ComicBook/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/jjangsangy/ComicBook/compare/v0.2.0-rc.1...v0.2.0
[0.2.0-rc.1]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0...v0.2.0-rc.1
[0.1.0]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0-rc.6...v0.1.0
[0.1.0-rc.6]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0-rc.5...v0.1.0-rc.6
[0.1.0-rc.5]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0-rc.4...v0.1.0-rc.5
[0.1.0-rc.4]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0-rc.3...v0.1.0-rc.4
[0.1.0-rc.3]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0-rc.2...v0.1.0-rc.3
[0.1.0-rc.2]: https://github.com/jjangsangy/ComicBook/compare/v0.1.0-rc.1...v0.1.0-rc.2
[0.1.0-rc.1]: https://github.com/jjangsangy/ComicBook/releases/tag/v0.1.0-rc.1
