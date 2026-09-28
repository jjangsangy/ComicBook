# Porting notes

The `kcc-c2e` port is feature-complete (crate version `0.2.0`). This file records how it was
built and where it deliberately differs from KCC. The user-facing behaviour is specified in
[cli.md](cli.md), [processing.md](processing.md) and [output.md](output.md).

## Phase history

Each phase ended with `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings` and
`cargo nextest run` green on the CI matrix.

| Phase | Scope | Where it landed |
|:---|:---|:---|
| 0 | CLI surface, options resolution, profiles, progress scaffolding | `ebook/{cli,options,profiles,progress}.rs` |
| 1 | Input adapters + data model | `ebook/input/*`, `model.rs` |
| 2 | Core image pipeline (colour, fill, split, gamma/contrast/resize, encode) | `ebook/processing/{color,fill,page}.rs` |
| 3 | Cropping & enhancement (margin/page-number, inter-panel, moiré) | `ebook/processing/{crop,interpanel,rainbow}.rs` |
| R | Off-the-shelf refactor (side phase, below) | cross-cutting |
| 4 | Metadata parsing and naming/filename resolution | `metadata.rs`, `naming.rs` |
| 5 | EPUB/KePub output (first shippable format) | `output/epub/*`, `output/kepub.rs` |
| 6 | Cover, Panel View variants, Scribe strips, spread options | `processing/{cover,page}.rs`, `output/epub/*` |
| 7 | CBZ, PDF and light-novel output | `output/{cbz,pdf,lightnovel}.rs` |
| 8 | Kindle AZW3/MOBI via `kindling` | `output/kindle.rs` |
| 9 | Chunking, fusion, `--delete`/`--temp-dir` | `chunk.rs`, `input/fusion.rs` |
| 10 | Webtoon (`comic2panel`) | `processing/webtoon.rs` |
| 11 | PDF/EPUB input, processing warnings, polish | `input/{pdf,epub}.rs`, `processing/mod.rs` |
| 12 | Hardening (fuzz, memory, cross-platform) | `tests/ebook_robustness_tests.rs` |

## Decisions and deviations

Conscious differences from KCC, all pinned by tests.

### Architecture / performance

- **In-memory, streamed pipeline.** KCC extracts to a temp tree, copies again for fusion and
  pickles options per worker. This port builds one in-memory `ComicTree` and streams output, so
  at least one full copy of every page is removed. Consequently `--temp-dir` only relocates the
  Kindle builder's scratch directory; the EPUB/CBZ/PDF paths have no temp tree to move.
- **Two-level progress.** A run with several inputs draws an overall file-progress bar above
  each file's page bar (`progress::Reporter`, the same overall-plus-children layout `clamp`
  uses), with `Created …`/warning lines drawn below the bars so the bars stay put; KCC reports
  per-image progress only. A single input keeps just its page bar, since that already is its
  overall progress.
- **`--no-processing` is byte-for-byte.** `Page` retains the source's encoded bytes and media
  type, and pages are emitted with their source extension and `OrderClass::Normal`.
- **Windows path-length flattening is not reproduced.** KCC flattens a tree when a Windows path
  would exceed ~220 chars; nothing is materialised on disk here, so the heuristic has no
  equivalent (zip entry names are unaffected by OS path limits).

### Naming and input

- **Ordering is case-insensitive natural sort** (`natord`), reproducing KCC's pre-order walk.
- **Archive-vs-folder parity.** A single redundant archive root directory is stripped only for
  archive sources, and `Page::source_name` stores the post-strip book-relative path, so
  equivalent CBZ/folder inputs load into identical trees.
- **Image extensions** are KCC's set minus `.jp2`/`.avif` (`.png/.jpg/.jpeg/.gif/.webp`), so
  `.bmp`/`.tiff` pages are dropped exactly as KCC drops them; `.jp2`/`.avif` have no decoder in
  the current pure-Rust dependency set and are ignored like any other non-image.
- **Junk filtering**: `._*`, `.DS_Store`, `Thumbs.db` and `__macOSX/` entries are skipped
  before decoding.
- **Slugification uses the `slug` crate** — the one intentional naming deviation. It collapses
  every non-alphanumeric run, so `_`/`.` become `-`, whereas KCC's `python-slugify` config
  preserves them. No maintained crate reproduces `python-slugify`'s option set, and the
  affected characters are not device-sensitive; the pinned properties (ASCII output, zero-padded
  numbers, the `-kcc-x` suffixes) are all preserved. The KCC-specific zero-padding and the CBZ
  pass-through are layered on top with `regex`.
- **No `_kcc<N>` collision suffix.** KCC renames an output that would clobber an existing
  file (`book_kcc0.epub`, …). This port keeps the output name deterministic and overwrites
  instead, so the filename never carries extra counter information; the behaviour is pinned by
  `ebook_naming_tests` and `ebook_kindle_tests`.
- **`ComicInfo.xml` is parsed leniently where KCC crashes.** KCC discards *all* metadata when an
  element has no text node (e.g. an empty `<Series/>`); this port treats a missing text node as
  an empty string. A genuinely malformed document is still ignored wholesale, matching KCC's
  exception path.
- **`--no-processing` names have no `-kcc-x` order suffix** (`kcc-NNNN.ext`), because KCC never
  constructs a `ComicPage` under `-n`.

### Processing fidelity

See [processing.md](processing.md) for the live list. The main deliberately-preserved quirks:
exact Pillow primitives, `group_close_values`' dropped value, the inter-panel finder's
height/width mix-up, the full-complex-spectrum moiré FFT, `--wallpaper` implementing the
documented (unreachable-in-KCC) fit, and the webtoon merge canvas quirk plus Pillow border ring.

### Output and cover

- **CBZ/PDF/Package keep KCC's shape**; the PDF path additionally embeds compatible JPEGs
  verbatim via `DCTDecode` (header-probed, no re-decode) and `FlateDecode`s everything else.
- **Cover** is KCC's `Cover.process` (autocontrast, optional grayscale, smart crop, fit/thumbnail)
  plus the `N/M` tome label drawn with `font8x8`. No pure-Rust crate ships KCC's Pillow face, so
  the label keeps KCC's position/size/colours and differs only in glyph shapes; only a split
  book's covers are labelled.
- **Kindle output pins the container, not the bytes.** `kindling` chooses its own compression
  seeds/UIDs, so tests read the file back structurally (`palmdb` type/creator, `file_version` 8
  vs 6 + `kf8.boundary_record`, the EXTH metadata, the kept EPUB). A byte diff against
  `kindlegen` is neither possible nor permitted.
- **KFX resolution override is deferred.** `-f kfx` produces the EPUB preset with
  `region-mag=false` at the profile resolution; resizing to the inputs' most common resolution
  is not implemented.

### Chunking, fusion, webtoon, input

- **No empty leading tome.** KCC creates an empty first tome when the very first unit already
  exceeds the cap; this port never emits an empty tome (one fewer output file in that edge case).
- **Fusion's `Covers/` pick is deterministic** (natural-sorted) rather than filesystem order;
  `--delete` leaves fusion sources alone, as KCC only removes its own scratch tree.
- **Webtoon** reuses `imageproc` for the edge convolution and restores Pillow's unchanged border
  ring; the merge/panel heuristics are ported from behaviour.
- **PDF input** uses the pure-Rust `pdfboss` rasterizer; `--legacy-extract` is the raw byte scan.
  Two documented deviations: KCC's extra text/CCITT render triggers are not reproduced, and an
  extracted image is re-encoded as PNG rather than copied with its original extension.
- **EPUB input** walks the OPF spine in memory; a spine that yields no image falls back to plain
  archive extraction (as do `--legacy-extract`/`--light-novel`).
- **`detectSuboptimalProcessing`** is a pure function returning warning strings, printed through
  `progress::warn` before `sanitize_tree`.

### Hardening

- **Fuzz/robustness is a test suite, not new guards.** `tests/ebook_robustness_tests.rs` drives
  malformed inputs through `catch_unwind`; the contract is *error, never panic*.
- `--splitter`/`--cropping`/`--inter-panel-crop` are constrained to `0..=2` (a clap usage error
  otherwise), matching KCC's `choices`.
- **The large-book tests are memory regression guards**: pages are decoded lazily and released
  as they are encoded, so peak RSS is linear in the *encoded* book, not the decoded one. The
  ceilings guard against accidentally retaining the decoded book (or duplicating the archive)
  rather than against a small constant.
- **Undecodable images are rejected during processing, not ingest.** Ingest reads the codec
  header for dimensions and defers the full decode, so a page whose header parses but whose
  pixels do not is reported by the processing pass; `--no-processing` never decodes and so, like
  KCC's `removeNonImages`, only checks the extension.
- **MSRV is 1.93**, the highest `rust-version` in the resolved dependency graph.

## Off-the-shelf refactor (Phase R)

Applied retroactively per the policy in [dependencies.md](dependencies.md); each change is a
behaviour-preserving commit backed by the Phase 1–3 tests.

**Swapped to crates/std:** `blank`→`ImageBuffer::new`, `blit`→`imageops::replace`,
`round_half_even`→`f64::round_ties_even`, `binarize`→`imageproc::contrast::threshold`,
`fill_rect`→`draw_filled_rect_mut`, `dynamic_color_type`→`ExtendedColorType::from`,
`pack_indices`→`bitvec`; plus consolidations (one `encode_png`, one `resize_buffer`, a shared
`fill::bounding_box`, `trim_histogram_ends`, and a unified `is_os_metadata`).

**Kept bespoke** (each with a policy comment at its site): `remove_dir_all_force`,
`copy_dir_all`, `group_thousands`, `normalize_archive_path`/`safe_join`/`is_matching_root`,
`strip_common_root`, the `image_ops`/`EBOOK_IMAGE_EXTENSIONS` split, `fit`/`contain_size`/`pad`,
`box_blur_1`, `keep_lines`, the JFIF/Rec. 601 colour matrices, the full-spectrum FFT, and the
clamp/convert `ProgressStyle`s. The only candidates that would have added a brand-new
dependency (`remove_dir_all`/`dircpy`/`thousands`) were rejected because they pull far more
transitive crates than they shrink our source; `bitvec` was already transitive, so promoting it
adds no weight. Irreproducible KCC/Pillow behaviour (crop grouping/merging, spread/rotate
decisions, archive-format wrappers) stays hand-rolled by design.

## Crate-backed path layer (refactor.md §8.1)

The archive/page name newtypes (`NormalizedArchivePath`, `SourceName`, `RelPath`, `PageName`,
`ChapterName`) are now backed by `relative-path`'s `RelativePathBuf` instead of `String`, and the
private `path_text` separator helpers are deleted; call sites use
`RelativePath::{file_name, parent, file_stem, extension, components, strip_prefix}`. The crate was
chosen over `camino`/`typed-path` (host-sensitive or heavier separator models) and `path-clean`/
`normpath` (they keep the leading `..` that is precisely the zip-slip case to remove): it is the
only crate whose model is "a relative, `/`-separated path". `std::path` remains the OS-boundary type
(real files, `safe_join`'s `PathBuf`); the two models meet only at conversion points.

**Kept bespoke:** the sanitizer rules in `normalize_archive_path` (traversal, drive prefixes,
per-component trimming, empty → `None`) — `RelativePath::normalize` keeps leading `..` and does not
drop drive letters or trim, so it cannot stand in; `safe_join`'s drop-`..` semantics (sanitize, then
`to_path`, *not* `to_logical_path`); and `is_os_metadata`'s backslash-aware split, because it also
classifies raw host paths, which the `/`-only `RelativePath` model never sees.
`image_ops::path_extension` now uses `std::path::Path::extension`, since it operates on a host
path rather than an archive name. The newtypes gained `as_relative()`; their `as_str()`/`Deref`/
`Display`/comparison surface is unchanged and emitted bytes are byte-identical. The gate is the
full suite — in particular `integration_tests::test_normalize_archive_path`, `test_safe_join`,
`test_cross_platform_nested_directory_extraction` and the new `sanitizer_neutralizes_hostile_names`
(hostile names cross-checked against `zip::enclosed_name`), plus `ebook_input_tests` and
`ebook_robustness_tests`.
