# Property-based tests

How `comic-book` uses [`proptest`](https://proptest-rs.github.io/proptest/), what is already
pinned, and the candidate properties that have been specified but not yet implemented.

## Why and where

A property-based test states a *law* that must hold for every input in some generated space, then
**shrinks** any failing input to a minimal counterexample. That complements the point-example unit
tests, the `insta` golden documents and the committed KCC reference fixtures
(see [development.md](development.md) → Testing & validation): the golden tests pin *this exact*
output, the properties pin *the shape of correctness* across the input space.

Conventions:

- `proptest = "1"` is a **dev-only** dependency (`[dev-dependencies]`), so it never ships.
- Each property lives in a nested `mod properties { use super::*; use proptest::prelude::*; … }`
  inside its module's existing `#[cfg(test)] mod tests`, so it can reach private helpers.
- Bodies assert with `prop_assert!`/`prop_assert_eq!`. The crate denies
  `clippy::unwrap_used`/`expect_used`/`panic` for `--all-targets`, so no `unwrap`/`expect`/`panic!`
  appears in a test; fallible setup may use `?` (proptest implements
  `From<E: std::error::Error> for TestCaseError`, and any `anyhow::Error` can be bridged with
  `.map_err(|e| TestCaseError::fail(e.to_string()))?`).
- Generated images and payloads stay **tiny** and case counts modest
  (`ProptestConfig::with_cases(64)`), so the properties add well under a second to the suite.
- They **never** assert golden bytes — device- and codec-sensitive output stays with the `insta` /
  fixture tests.
- A failure writes a replayable seed under `proptest-regressions/`.

## Covered today

| Area | Module | Properties |
|:---|:---|:---|
| Geometry units | `src/units.rs` | `IndexBox::union` semilattice laws + encloses both; `intersects` symmetry & margin-monotonicity; `BBox<u32>::area` exact `u64` product (no overflow); `Quality::new` totality over `0..=255` |
| Naming | `ebook/naming.rs` | `slugify` idempotence (both styles, both sort modes); `Slug` output is a safe lower-case path component |
| Metadata | `ebook/metadata.rs` | `ComicInfo::parse` canonicalises each people field (de-duplicated, sorted, empty-free) |
| Cropping | `ebook/processing/crop.rs` | `merge_boxes` pairwise-disjoint output, union conservation, idempotence, no growth; `clamp_bbox` contains its input and the 10 % band |
| Fill / colour | `ebook/processing/{fill,color}.rs` | uniform page follows the 128 threshold; neutral (grey) images are Gray |
| Page size | `ebook/processing/page.rs` | `contain`/`thumbnail` fit the target (thumbnail never upscales); `fit`/`pad` produce the exact target size |
| Webtoon | `ebook/processing/webtoon.rs` | ordered partition of panels into virtual pages; split parts preserve the panel's top/bottom |
| Chunking | `ebook/chunk.rs` | `pack_units` conserves whole units under the target (order, no drop/duplicate, oversized-unit exception); Scribe `above` pairs only with the immediately following `below` |
| EPUB output | `ebook/output/epub/{opf,mod,templates}.rs` | `spread_properties` side rules (Center iff rotated, split halves pin their side, adjacent plain pages alternate); `html_escape` round-trips; `normalize_lf` is CR-free and idempotent |
| Profiles | `ebook/profiles.rs` | every table row is internally consistent (geometry vs `Profile::Other`, grayscale palettes, brand predicates vs code prefixes, `from_code(code()) == profile`) |
| Archive paths | `archive/path.rs` | `normalize_archive_path` clean/dot-free/idempotent; `safe_join` containment (and no `..`) |
| Resize / split | `image_ops.rs` | `resize_image_by_total_pixels` only shrinks; `split_image_iterative` preserves pixels and order |
| Ingest | `ebook/input/archive.rs` | `build_tree` (Keep) preserves the page multiset, orders chapters/pages, and reconstructs `chapter.name.join(rel_path) == source_name` |

## Candidate properties (specified, not yet implemented)

Each row is a concrete, ready-to-implement property. When one lands it should join the table above.
“Needs” records the setup that made it more than a trivial addition.

### Geometry, naming & metadata

| Target | Property | Oracle / generation note |
|:---|:---|:---|
| `IndexBox::union` | least-upper-bound completeness (beyond the landed “encloses both”): for any common upper bound `c`, `union ⊆ c` | Generate `c` by jittering the union’s edges outward, else the hypothesis is unsatisfiable (vacuous) |
| `ComicInfo::parse` | round-trip a canonical `ComicInfo` through a reference serializer | Needs a test-local `to_comicinfo_xml` (the crate has no writer); exclude `\r` and XML-illegal C0 controls; author people already sorted/de-duplicated |
| `resolve_with` | `authors` is the sorted, unique union of `writers ∪ pencillers ∪ inkers ∪ colorists`, else `["cb"]`; an explicit `-a` wins | Needs a `ComicTree` + `Options` fixture (`Options::resolve`); catches a dropped `.chain` arm |
| `resolve_with` | title-schema composition (`Only` verbatim, `-t` verbatim, `Default`/`Combine` concatenation with `Vol.`/`#` padding) | Oracle is the explicit concatenation; cross the mode × empty-field product |
| `resolve_with` | `comicinfo_xml` retained iff `keep_comicinfo ∧ CBZ-output ∧ parseable` | Covers the “flag set but non-CBZ” and “flag set but malformed” gaps |
| `zfill` | `len == max(len, w)`; no-op when long enough; idempotent; sign preserved | Private → in-module; draw `w` relative to `v.len()` or most cases hit only the no-op branch |
| `slugify` | the first two numeric runs pad to width 4 (and none otherwise); `Slug,true` equals `slug::slugify` | Oracle is a small model of `pad_numbers`; `slug` is a dependency, so the bypass clause is checkable directly |
| `sanitize_tree` | global `cb-NNNN` numbering, page conservation, extension lower-casing, `cover_path`, chapter-title map | Needs a `ComicTree` + `Options` fixture; snapshot the originals *before* the in-place rename |

### Image processing

| Target | Property | Oracle / generation note |
|:---|:---|:---|
| `crop` autocontrast | range `[0,255]` after a non-flat stretch, idempotence, monotone map | Random images are essentially never flat, so the stretch branch fires |
| `crop::group_close_values` | groups sorted with gaps `> max_dist`; endpoints drawn from the input; `len == 0 ⇔ empty` | Random gaps straddle `max_dist` |
| `crop::trim_histogram_ends` | surviving bins contiguous; total drops by at most `2·cut`; `cut == 0` identity | Sparse histograms |
| `interpanel` crop | only removes lines of the cropped axis; border gutters untouched; solid page unchanged | Place the gutter at a random interior band |
| `page::split_check` | payload counts at the `1.16`/`1.8` ratio boundaries; rotated payloads swap dimensions | Sample ratios straddling both thresholds |
| `page::bisect`, `maximize_strips` | the halves/stack reconstruct the source pixels; `right_to_left` swaps them | Compare against `crop_imm`; odd/even widths |
| `page::pack_indices` | sub-byte (1/2/4/8-bit) packing round-trips with per-scanline padding | Widths where `w·bits` is not a multiple of 8 |
| `page` encoders | PNG round-trips pixel-exactly; JPEG/GIF/WebP decode to the same dimensions | Lossy codecs → dims only, never bytes |
| `cover` | `crop_main_cover` only shrinks width, within the frame; the tome label clips on tiny covers | Ratio sweep `1.0/1.34/1.7/1.83/2.0`; cover `1..64²` |
| `webtoon::detect_panels` | ordered, disjoint panels within `[0, height]` | Banded strips (solid strips yield none — vacuous) |
| `webtoon::merge_chapter` | merged strip is `(width, Σ heights)` and the exact row concatenation | Common-width pages; marked rows |

### Chunking, model & output

| Target | Property | Oracle / generation note |
|:---|:---|:---|
| `chunk::split` | whole-book conservation across every mode (flat / two-level / per-subdirectory) | Distinct payload per page; cap ≈ total/2 so it is not a single tome |
| `chunk::{flatten, image_level, per_top_level}` | flatten roots every page and is idempotent; image level reports mixed depths; per-top-level groups *consecutive* equal keys | `A, B, A` distinguishes run-grouping from global grouping |
| `chunk::assemble` | one cover per tome, `page_count` agrees, single-tome cover byte-identical | Needs `total > 1` with a cover |
| OPF `manifest_items` / `build_opf` | one manifest id per image (plus a below-image), ids unique, one spine entry per page in order | Global `cb-NNNN`-shaped names make ids unique |
| `epub::{unique_id, stem_of}`, `build_entries` | `unique_id` has no `/`; `stem_of` matches `splitext`; no duplicate archive paths; a `-below` page never enters the spine | Multi-dot / leading-dot names; a chapter with no pages |
| `xhtml::PanelGrid` | `order()` is a permutation of the regions; `PanelId` injective | All four grids × orientation × direction |
| `templates::{NavTitle, NavId, ManifestId, Idref}` | `NavId::folded` has no `/`; `NavTitle` delegates to `html_escape`; spine/manifest id spelling agrees | uids both containing and not containing `"above"` |
| `model` `Page`/`PageData` | the state machine never loses a payload; a second `take_*` returns `None` and does not panic | Sequences over the four start states |

### Archive, ingest & options

| Target | Property | Oracle / generation note |
|:---|:---|:---|
| `ArchiveWriter` → `open_reader` | write `{name → bytes}` then read back equal, for all five kinds | `tempfile` per case, low case count; RAR folds `/`↔`\`; include nested names |
| `list_entries` ≡ `read_entries` | the two reader paths agree on `(name, kind)` | Independent code paths (`by_index` vs streaming) |
| zip writer | explicit file order is preserved through write→read | Exclude the natural-sorting directory backend |
| `ops` root-strip | collapses exactly one wrapper level iff every entry shares one top-level dir | Multi-root variant must strip nothing |
| `input/archive` | `strip_common_root` strips one level only; `is_ebook_image` extension set; `load_page` header dims + media type | Real tiny encodes; **do not** assert `strip_common_root` idempotence |
| `input/epub::resolve_relative` | `resolve_relative(dir, "../"+rel) == resolve_relative(parent(dir), rel)`; absolute `rel` drops the base | Needs `..` to be non-vacuous |
| `input/epub::spine_images` | `Some` iff a spine page references an existing image; each entry is `(i + ext, bytes)` picking the **largest** acceptable image | Include a longer non-`src`/`href` attribute as a decoy |
| `input/pdf` | `render_target`/`FitPreference`/`render_zoom` arithmetic; `find`/`find_in_range` vs a naive scan | Vary the crop mode so a swapped factor shows |
| `image_ops` | `resize_image_by_width` scales only the width axis, never upscales; `resize_lanczos3` returns exactly the requested `Size` | Random pixel types |
| `options::resolve` | cross-field derived-value invariants (geometry tracks `data`, webtoon/kfx/kepub/scribe interactions) and the error conditions | Heavy: structured argv over `ALL_PROFILES`; skip `Profile::Other` without custom sizes |

## Findings surfaced by the property tests

- **`normalize_archive_path` traversal + idempotence (fixed).** The `.`/`..` guard ran before the
  trailing-colon strip, so `"..:"` was cleaned to `".."` and survived — `safe_join(base, "..:")`
  returned `base/..` (a zip-slip) — and `"a :"` normalised to `"a "` then `"a"`. The strip now runs
  first and re-trims; both are pinned by tests in `archive/path.rs`.
- **Open: non-title metadata under non-default title modes.** `resolve_with` assigns `volume` and
  `number` only inside the `book_default_title` branch (default title, `Default`/`Combine`). Under
  `--metadata-title 2` or an explicit `-t`, `series`/`summary`/`bookmarks` are still lifted but
  `volume`/`number` stay empty, and the OPF `group-position` then drops them — contradicting the
  module doc and [output.md](output.md). A property pinning either behaviour must wait on a
  docs-vs-code decision; not yet pinned.

