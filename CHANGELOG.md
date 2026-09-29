# Changelog

All notable changes to `comic-book` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are
reconstructed from the repository's release tags (`v*`); each release links to the diff
since the previous tag. The release workflow dates the `Unreleased` section into a tagged
release when a version tag is pushed.

## [Unreleased]

### Added

- Added a `Coverage` workflow (`.github/workflows/coverage.yml`) that runs the test suite under
  `cargo llvm-cov nextest`, uploads the `lcov` report to Codecov — which serves the new coverage
  badge in the README — and enforces a 95% line-coverage floor (`--fail-under-lines`) as a
  `Coverage` status check. The per-file table is written to the run's job summary, a same-repo pull
  request gets a sticky coverage comment, and the `lcov` data, an HTML report and the summary
  markdown are uploaded as the `coverage` artifact. The Codecov upload authenticates with a
  `CODECOV_TOKEN` repository secret. Coverage is split out of `ci.yml` so a failing gate does not
  redden the main CI badge.

### Changed

- Rebranded the `ebook` output from the old `kcc` identifiers to `cb`: when no `-a/--author` is
  given the author is now `cb`, and sanitized page files are named `cb-NNNN-cb-<order>` (e.g.
  `cb-0001-cb-x.xhtml`, `cb-0002-cb-d-above.jpg`) instead of `kcc-NNNN-kcc-<order>`. The page-name
  prefix now lives in one place (`ebook::naming::PAGE_PREFIX`), and the "already processed" warning
  refers to `cb`. Both the default author and the page filenames are user-visible, so the emitted
  documents change; the golden fixtures were regenerated for the new names.

## [0.2.8] - 2026-09-28

### Changed

- Hardened the `archive` public API so that invalid archive states are unrepresentable (the
  archive/path step of the type-safety refactor tracked in `docs/refactor.md`):
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
- Typed the `comic-book` command-line values so invalid modes are rejected by `clap` instead of
  being re-interpreted from bare integers (the CLI step of the type-safety refactor tracked in
  `docs/refactor.md`):
  - `ebook --splitter`, `--cropping`, `--inter-panel-crop`, `--metadata-title` and `--batch-split`
    now accept named values (`--splitter split`, `--cropping pages`, `--inter-panel-crop both`,
    `--metadata-title combine`, `--batch-split per-subdir`) as well as their previous `0`/`1`/`2`
    spellings, which are kept as aliases.
  - `ebook --borders <white|black>` replaces `--black-borders`/`--white-borders`; the two old flags
    are hidden aliases for `--borders white`/`--borders black` and conflict with each other (passing
    both is now a `clap` error rather than silently letting white win).
  - `convert --to` is now a validated `ValueEnum` (`cbz`, `zip`, `cbr`, `rar`, `cb7`, `7z`, `cbt`,
    `tar`, `dir`), so an unsupported target is rejected while parsing.
- Split the resolved `ebook` run configuration into cohesive groups and replaced the derived
  boolean flags with enums (the configuration step of the type-safety refactor tracked in
  `docs/refactor.md`):
  - `ebook::options::Options` is now a thin aggregate of `DeviceOptions`/`MainOptions`/
    `ProcessingOptions`/`OutputOptions`/`SessionOptions` (plus `inputs`). `metadata::resolve_with`
    takes `&OutputOptions`, and the `chunk`/`kindle` helpers take the single group they read.
  - The `is_kindle`/`is_kobo`/`device_kind` triple is a `ReaderFamily`; `custom_profile` plus the
    `"Custom"` name sentinel is a `Geometry`; `panel_view`/`two_panel`/`legacy_panel_view` is a
    `PanelView` (with `--vertical-4-panel` kept as an independent flag, since it only affects the
    OPF writing mode); and `format` plus the `kfx`/`kepub`/`keep_epub`/`kindle_azw3` flags is an
    `OutputEncoding` (`write_tome` matches it exhaustively, and `Options::resolve` derives a concrete
    `ResolvedFormat` so its encoding match is exhaustive too — no `bail!` remains).
    `kindle_scribe_azw3` becomes `ProcessingOptions::scribe`, because it also applies to EPUB/MOBI
    output on a Scribe. `right_to_left` is now `MainOptions::right_to_left()`, and the Kindle
    Panel View predicate is `Options::panel_view_enabled()`.
  - `Profile::is_kobo_brand()`/`is_scribe()` no longer test the human-facing profile code string;
    they read `DeviceKind` and an explicit Scribe set respectively.
  - `assemble` now takes a `TitleOrigin` (`Derived`/`Fusion`) instead of the covarying
    `default_title` + `fusion` pair, and `naming::slugify` takes a `NameStyle` (`Slug`/`Cbz`)
    rather than the whole request `Format`.
- Replaced the geometry and measurement primitives with named, compiler-checked newtypes (the
  geometry/unit step of the type-safety refactor tracked in `docs/refactor.md`), collected in the new
  `units` module:
  - `Size { width, height }` replaces the bare `(u32, u32)`/adjacent `u32` size pairs: a page's
    header `dimensions`, an `EncodedPage`'s `width`/`height`, the profile/cover/light-novel page
    size, and the `fit`/`contain`/`thumbnail`/`pad`/`resize` targets.
  - `BBox<T>` (Pillow's `left`/`upper`/`right`/`lower` order) and `IndexBox` (`x1`/`x2`/`y1`/`y2`,
    KCC's axis-grouped inclusive order) replace the interchangeable positional 4-tuples in the
    crop/kernel bounding-box code, and `Range { min, max }` replaces the `(u8, u8)` luma/chroma
    pairs.
  - `Percent`/`Fraction` replace bare `f64` percentages and fractions (`--preserve-margin` is now
    `Option<Percent>`, `--cropping-minimum` a `Fraction`); `Pixels`/`Bytes`/`Megabytes` replace the
    `u64`/`u32` pixel counts, byte caps and MB options (a pixel threshold can no longer be compared
    against an encoded-size cap); the resolved `--jpeg-quality` is a validated `Quality`; and the
    quantiser's palette and index plane are `Palette`/`PaletteIndices`.
  - The public helpers `clamp::run_clamp`, `image_ops::split_image_iterative`,
    `image_ops::resize_image_by_total_pixels` and `image_ops::resize_image_by_width` now take the
    typed `Pixels` (and `resize_lanczos3` a `Size`) instead of bare `u64`/`u32` — a signature-level
    change for library callers. The `BBox`/`Range`/`IndexBox` span accessors saturate rather than
    subtracting unchecked, so a transposed box can no longer panic a debug build.
- Updated the `docs/cli.md` and `docs/processing.md` reference tables to describe the typed mode
  values and the consolidated `--borders` flag introduced by the CLI step above.
- Replaced the boolean-blind flags, magic tri-states and stringly page parts in the
  image-processing pipeline with fieldless enums (the processing step of the type-safety refactor
  tracked in `docs/refactor.md`). Behaviour and emitted bytes are unchanged:
  - `PageFlags` carries `orientation: Orientation` (`Upright`/`Rotated`), `background: ResolvedFill`
    (the resolved `--borders` fill, distinct from the detected `Page::background`) and `half:
    ScribeHalf` (`NotSplit`/`Above`/`Below`) instead of the `rotated`/`black_background` bools and the
    `above`/`below` bool pair — so a page can no longer be both halves of a Kindle Scribe split.
    `chunk` and `output/epub` now read `flags.half`.
  - The colour decision is `color::Detected` plus `OutputColor::from_detection(Detected,
    force_color)`, replacing the two same-typed `color`/`color_output` bools; the dead
    `color_check(.., original_is_grayscale)` parameter is removed (the grayscale-source guard lives
    at the call sites, which still skip the RGB round-trip).
  - `kernels::threshold_in_place` takes a `ThresholdKind` (`Above`/`Below`) and monomorphises the
    polarity through a `const`-generic core, so the 16-lane loop stays branch-free; `band_white_black`
    returns a named `Band { has_white, has_black }` instead of a transposable `(bool, bool)`.
  - The inter-panel helpers take an `Axis` (`Rows`/`Columns`) instead of the two inverted
    `horizontal`/`remove_rows` boolean spellings; `fill::strip_vote` returns a `StripVote`
    (`Black`/`Mixed`/`White`) instead of the `-1`/`0`/`+1` magic tri-state; the webtoon `Panel` is a
    `{ top, bottom }` struct with a derived `height()` instead of a `(u32, u32, u32)` tuple; the PDF
    fit choice is a `FitPreference` (`Height`/`WidthForPortrait`); and the Scribe page-part suffix is
    a `PagePart` (`Above`/`Below`/`Whole`) rather than a bare `"above"`/`"below"`/`"whole"` literal.
- Hardened the processing-step types so their invariants are compiler-checked rather than merely
  documented (the `docs/refactor.md` Phase 5 follow-up). `OutputColor` is now an opaque newtype built only
  by `OutputColor::from_detection`, with `is_color`/`is_gray` accessors in place of `==` against its
  variants; the resolved `--borders` fill is the distinct `ResolvedFill` newtype rather than a bare
  `Background`, so it cannot be confused with the detected `Page::background`; and the `Detected`
  (colour mode) and `ScribeHalf`/`Orientation` (page flags) dispatches in the processing and output
  pipeline use exhaustive `match`es instead of `if … == variant` comparisons or a `bool` re-collapse.
- Made the page payload a move-only state machine and gave the page/chapter names distinct
  compiler-checked identities (the page-state step of the type-safety refactor tracked in
  `docs/refactor.md`). Emitted bytes are unchanged:
  - `Page` replaces its `image: Option<DynamicImage>`/`raw: Option<Vec<u8>>`/`source_media_type:
    Option<MediaType>` trio with one `PageData` (`Encoded(Source)`/`EncodedDecoded(Source,
    DynamicImage)`/`Pixels(MediaType, DynamicImage)`/`Consumed`). The `(None, None)` page that four
    call sites had to guard is no longer representable, a decode keeps the encoded bytes
    (`EncodedDecoded`) so `--no-processing` still emits them without decoding, and every state
    transition matches the enum exhaustively so a new state is a compile error.
  - `Page` no longer derives `Clone` (nor do `Chapter`/`ComicTree`/`PreparedBook`/`ProcessedBook`/
    `ProcessedChapter`), so a stray clone cannot duplicate a decoded frame plus the encoded book.
  - `SourceName` (book-relative source path), `RelPath` (chapter-relative file name) and `PageName`
    (`EncodedPage::name`) are `#[repr(transparent)]` newtypes, and `ChapterName` is a `Root`/`Dir`
    enum rather than an empty-string sentinel — so the names cannot be swapped and the root chapter
    cannot be misspelled as an empty directory.
  - The unread `ComicTree::cover`/`CoverSource` are removed (cover selection already flowed through
    `PreparedBook::cover_override`/`processing::cover`); `ProcessedBook` carries the cover as one
    `Option<Cover>` (page plus its smart-crop flag) instead of the split `cover`/`cover_smart_crop`
    pair; `input::archive::LoadedPage` folds into the `PageData` carrier; and `input::fusion::Fused`
    derives its synthetic source path from `output_dir.join(&title)` instead of storing it.
  - Removed the vestigial `Page::flags`/`PageFlags::order_class` (never read; `EncodedPage::
    order_class` is the single owner), and replaced the silent `media_type().unwrap_or(Jpeg)` and
    the `--splitter` equality checks with explicit, exhaustive handling.
- Gave the EPUB output pipeline compiler-checked identities instead of bare strings and booleans
  (the output step of the type-safety refactor tracked in `docs/refactor.md`). Emitted bytes are
  unchanged:
  - `PageRef`'s three confusable `&str` fields are `ImageDir`/`FileName`/`Stem` newtypes, the
    manifest/spine/navigation values are `ManifestId`/`Idref`/`Href`/`SpineAttr`/`NavId`/`NavTitle`
    with the `page_`/`img_`/`-below` id conventions centralised, and the document modes are enums:
    `PageSide`/`Direction` (with the `(invert_direction, right_to_left)` XOR computed once),
    `WritingMode`, `ManifestMediaType` and `PanelId` (replacing the stringly Panel View id and its
    wildcard `style` match).
  - `Opf`'s `has_description`/`has_series`/`has_group` bool+payload pairs are
    `Option<&str>`/`Option<Series>`; the never-set `OpfItem` `properties`/`has_properties_before`/
    `has_properties_after` trio is deleted; `region_mag` is a `bool`; and `PageXhtml`'s
    `has_below`/`below_image_src`/`below_img_width`/`below_img_height` group is one
    `Option<BelowImage>`.
  - The OEBPS entry list is a move-only `EpubEntries` of `ZipEntry`/`ZipPath` whose only constructor
    prepends the stored `mimetype` entry, replacing the anonymous `(String, Cow<[u8]>)` vector whose
    ordering rule lived only in a doc comment (nothing clones the borrowed book).
  - The fifteen-positional-bool `kindling` call is a named `MobiFlags` struct; `PdfImage`'s
    `gray: bool` and `jpeg_components() -> Option<u8>` are a `ColorSpace` enum; and the threaded
    `drop_bookmarks: bool` is a `Tomes` (`Single`/`Split`) enum.
- Removed the remaining runtime guards, `_` wildcards and duplicated helpers across the codebase
  (the sweep step of the type-safety refactor tracked in `docs/refactor.md`). Emitted bytes are
  unchanged:
  - `archive::ops` collapses a wrapper folder through one `RootStrip { name, prefix }` value (its
    `RootStripPolicy` match is exhaustive again, with no guarded `_` arm), and the repeated
    basename/stem plumbing is now delegated to `relative-path`'s `RelativePath` methods used by the
    archive, naming, chunk, webtoon, page and EPUB paths (the dead `rsplit(..).next().unwrap_or(..)`
    fallbacks are gone, including the cover-name classifier and the wrapper-root component scan).
  - `metadata::ComicInfo` keys its nine single-value fields on a `Field` enum (one spelling of
    each name, with `Field::from_name` as its inverse) and drains them through one exhaustive
    `match`, replacing three separate string lists; a malformed (non-UTF-8) element name now
    discards the document through the existing rule rather than silently failing every comparison.
  - `ebook::progress::Reporter` holds one `Mode` (`Batch { .. }`/`Standalone`) instead of parallel
    `Option<MultiProgress>`/`Option<ProgressBar>` fields, so a half-set reporter is unrepresentable;
    the shared `progress_style::{bar, bar_with_chars, spinner}` helper replaces the copied
    `ProgressStyle::default_bar().template(..)` idiom in `convert`/`clamp`/`ebook`.
  - `processing::cover` draws the tome label through a `CoverPixels` (`Luma`/`Rgb`) enum, so the
    pixel-writing helper no longer has a `_ => {}` wildcard that silently drops other pixel types;
    `color::rgb_to_luma` builds its buffer infallibly (no `unwrap_or_else` blank-image fallback);
    and `webtoon::detect_panels` tracks the open panel as a single `Option<u32>` instead of a
    `bool`/`u32` pair mutated in lockstep, taking a validated `StripWidth` so the scan step can
    never be zero.
  - `Profile::entry` resolves its table row in O(1) with no fallback: the profile rows are one
    `const` that both the `--profile` list and a compile-time discriminant check are derived from,
    so there is no hand-kept variant list and no per-variant match; `BatchSplit::ensure_splitting`
    replaces the free `force_split`; the EPUB `dcterms:modified` formatting returns `Result`
    instead of an epoch-string `unwrap_or_else`; `output::kindle`'s `--temp-dir` lookup is an
    `and_then` rather than a `then(..).flatten()`; and the `ebook` run no longer clones the input
    `Vec` to iterate it.
  - `image_ops::resize_lanczos3` (and the `resize_image_by_*` wrappers) return `Result` instead of
    silently returning a full copy of the original image on an impossible buffer mismatch, removing
    three hidden `DynamicImage::clone`s. `image_ops::is_image_file` is now built on a single
    `image_ops::path_extension(&Path)` helper (no `to_string_lossy` ladder); as a consequence a
    leading-dot name (`.png`) and a non-UTF-8 file name are no longer treated as images by
    `convert`/`clamp`.
- Backed the archive-relative path newtypes (`NormalizedArchivePath`, `SourceName`, `RelPath`,
  `PageName`, `ChapterName`) with the `relative-path` crate's `RelativePathBuf` instead of `String`,
  and removed the private hand-rolled separator helpers in favour of `RelativePath`'s
  `file_name`/`parent`/`file_stem`/`extension`/`components`/`strip_prefix` operations (the optional
  path-layer side quest in `docs/refactor.md`). The newtypes gained an `as_relative()` accessor; their
  `as_str()`/`Deref`/`Display`/comparison surface and every emitted archive/EPUB byte is unchanged.
  `std::path` remains the host-filesystem type (`safe_join`'s `PathBuf` and
  `image_ops::path_extension`), and `is_os_metadata` keeps its backslash-aware split because it also
  classifies raw host paths.

### Fixed

- `clamp`'s `remove_dir_all_force` no longer trips the `unused_variables` lint on Windows: the
  outer `Err(e)` binding is used only by the non-Windows arm, so it is now `Err(_e)` for both.
- `scripts/set-version.sh` and `scripts/set-version.ps1` (with `--changelog` / `-Changelog`) now
  re-open an empty `## [Unreleased]` heading above the dated release, so the default branch keeps a
  section for the next release's entries instead of losing it on roll-over.

### Docs

- Moved the type-safety refactor record to [`docs/refactor.md`](docs/refactor.md), distilled to the
  binding rules, the completed phases and their behaviour-visible deviations, the resulting option
  and enum shapes, and the test/lint guardrails. The former root-level `REFACTOR.md` is removed and
  every reference repointed.
- Corrected the lint command in `AGENTS.md`, `docs/refactor.md`, `docs/development.md` and
  `docs/porting.md` to `cargo clippy --all-targets --all-features -- -D warnings` (the missing `--`
  made cargo reject `-D warnings` as an unexpected argument).
- Required piping `cargo nextest` through `tail` (`cargo nextest run 2>&1 | tail -n 20`) in
  `AGENTS.md`, `docs/refactor.md`, `docs/development.md`, `CONTRIBUTING.md`, `README.md` and the pull
  request template: only the pass/fail summary and the names of the failing cases matter, so the
  full run output is not echoed. CI keeps the unfiltered `cargo nextest run --no-fail-fast`.
- Noted in `AGENTS.md` that the `cargo nextest`-through-`tail` rule overrides the built-in
  `terminal` tool's default guidance against piping to `head`/`tail`, and dropped the rule's
  trailing cross-reference.

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

[Unreleased]: https://github.com/jjangsangy/ComicBook/compare/v0.2.8...HEAD
[0.2.8]: https://github.com/jjangsangy/ComicBook/compare/v0.2.7...v0.2.8
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
