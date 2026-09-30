# Type-safety refactor: make impossible states unrepresentable

`comic-book` was ported from Python (KCC's `kcc-c2e`) and kept its dynamic idioms: boolean-blind
APIs, stringly-typed modes, magic numbers, `Option` pairs encoding one state, and runtime guards for
states the compiler could rule out. This records the refactor that replaced each with a
compiler-checked enum, newtype or type state. Behaviour and emitted bytes are unchanged; the
deliberate exceptions are in [§5](#5-ordered-refactor-plan-completed).

> **Status: complete.** Phases 1–8 ([§5](#5-ordered-refactor-plan-completed)) and the optional
> path layer ([§8.1](#81-back-the-path-newtypes-with-relative-path)) have landed; Phase 9 is this
> document. The original line-numbered audit and the per-phase working notes have been distilled
> away; file and function names are the durable anchors.

---

## 1. Constraints and decision rules

Every change satisfies these; a proposal that violates one belongs in [§4](#4-non-goals-memory--simd).

1. **Zero-cost only.** Newtypes are `#[repr(transparent)]`; enums are fieldless/`Copy`; state
   transitions are moves. No new allocation, `Clone`, `Box` or dynamic dispatch on a hot path.
2. **No extra memory.** Peak RSS stays linear in the *encoded* book, not the decoded one
   ([architecture.md](architecture.md)); never retain decoded pixels longer or duplicate an encoded
   page.
3. **Do not defeat SIMD.** The `kernels.rs` (16-lane `wide`), `rainbow.rs` FFT and `image_ops.rs`
   resize loops may be parameterised by enum/`const` (the branch is loop-invariant), never by a
   `dyn` predicate or a new bounds check inside the pixel loop.
4. **Behaviour and output are frozen.** The EPUB/OPF/NCX/NAV/XHTML/MOBI and CBZ/PDF layouts are
   device-sensitive ([output.md](output.md)); askama changed only where it renders identical bytes.
5. **Every change is pinned** by the existing tests plus a `CHANGELOG.md` `[Unreleased]` entry.
6. **Panic-free.** `Cargo.toml` denies clippy `unwrap_used`/`expect_used`/`panic` and the tree
   contains none ([§7.5](#75-the-lint-contract)). An "impossible" state is made unrepresentable or
   returned as a `Result`, never `expect`-ed or `unreachable!`-ed.

Cost legend: **✅** compiler-erased · **➕** less work/memory · **⛔** rejected.

## 2. The toolkit

Five zero-cost mechanisms are reused throughout.

### 2.1 Enums instead of booleans ("boolean blindness")

A `bool` parameter, or a pair of `bool`s that must agree, encodes a state the compiler cannot check.
Replace it with a fieldless enum and an exhaustive `match` (e.g. `ScribeHalf { NotSplit, Above,
Below }` instead of `above`/`below` bools).

### 2.2 Newtypes for confusable primitives

`String`/`&str`/`u32` values that mean different things are trivially swappable, so wrap them
transparently (`#[repr(transparent)]`, e.g. `NormalizedArchivePath`). No allocation, no copy.

### 2.3 Type states / sum types for mutually exclusive fields

`Option` pairs or `bool`+payload pairs that are only ever set together are an anonymous sum type;
model them as an enum whose transitions are moves. The canonical one is the page payload, which
deliberately preserves the bytes+pixels pairing ([§4](#4-non-goals-memory--simd)):

```rust
enum PageData {
    Encoded(Source),                       // bytes only, not yet decoded
    EncodedDecoded(Source, DynamicImage),  // cached decode over the retained bytes
    Pixels(MediaType, DynamicImage),       // webtoon strip: pixel-only
    Consumed,                              // bytes/pixels moved out
}
```

### 2.4 Borrowed tag unions instead of `(bool, &[u8])`

A "directory" is signalled by `is_dir == true` *and* an empty slice; make the kind carry the
payload: `pub enum EntryContent<'a> { Directory, File(&'a [u8]) }`. No allocation, still borrows the
scratch buffer.

### 2.5 Kill wildcards and impossible-state guards

Replace `match x { A => .., B => .., _ => .. }` on a `u8`/`&str`/bool-pair with an exhaustive
`match` on an enum, so adding a variant is a compile error and the silent `_` arm disappears.

## 3. Findings catalogue

The audit catalogued **95 findings** in seven categories (A 21 · B 16 · C 12 · D 18 · E 16 · G 8 ·
F 4). The subsections below record what each finding became; the ids are still cited from code
comments.

### 3.1 Boolean blindness (A1–A21)

Directory-ness → `EntryContent` (A1, A2); root-strip bools → `RootStripPolicy`/`RootStrip` (A3,
A4); fit strategy → `FitPreference` (A5); `above`/`below` → `ScribeHalf` (A6); the duplicate/derived
background → the resolved fill (A7); detected/retained colour → `Detected`/`OutputColor` (A8);
threshold polarity → `ThresholdKind` (A9); axis bools → `Axis` (A10); band result → `Band` (A11);
the reader triple → `ReaderFamily` (A12); output flags → `OutputEncoding`/`PanelView`/`PngOptions`
(A13); fusion+title → `TitleOrigin` (A14); MOBI bools → `MobiFlags` (A15); PDF gray → `ColorSpace`
(A16); `drop_bookmarks` → `Tomes` (A17); `keep_epub` → `OutputEncoding::Mobi` (A18); the re-derived
panel/style bools → `Options::panel_view_enabled` (A19); border flags → `--borders`/`BorderColor`
(A20); custom profile → `Geometry` (A21).

### 3.2 Stringly-typed values and magic numbers (B1–B16)

The five `u8` CLI modes → `Splitter`/`Cropping`/`InterPanelCrop`/`MetadataTitle`/`BatchSplit`,
matched exhaustively by every consumer (B1, B2); resolved format → `OutputEncoding` (B5); `--to` →
`ArchiveFormat` (B6); page sides → `PageSide` (B7); writing mode / direction →
`WritingMode`/`Direction` (B8); panel ids → `PanelId` (B9); manifest media types →
`ManifestMediaType` (B10); string `region_mag` → `bool` (B11); naming → `NameStyle` (B12); webtoon
panel tuple → `Panel` (B13); fill tri-state → `StripVote` (B14); profile brand/scribe prefixes →
`DeviceKind` (B15); the nine `ComicInfo` names → `Field` (B16). (B3/B4, the chunk split-mode depth
and the `image_level` tuple, were left as-is.)

### 3.3 Type states and invalid states (C1–C12)

`Page`'s parallel `Option`s → `PageData` (C1); write-only `ComicTree.cover`/`CoverSource` deleted
(C2); cover flag pair → `Option<Cover>` (C3); `Payload.rotated` → `Orientation` (C4); `Reporter`
`Option`s → `Mode` (C6); dead `OpfItem` trio deleted (C7); `has_below` → `Option<BelowImage>` (C8);
description/series/group pairs → `Option`s (C9); the OCF list → move-only `EpubEntries` (C10); the
panel `bool`+`u32` → `Option<u32>` + validated `StripWidth` (C11); `Fused.source` derived (C12).

### 3.4 Newtypes (D1–D18)

The name identities `SourceName`/`RelPath`/`PageName`/`ChapterName` (D1, D14); `Size` (D2);
`IndexBox`/`BBox` (D3); `Range` (D4); `Percent`/`Fraction` (D5); `Pixels`/`Bytes`/`Megabytes` (D6);
`NormalizedArchivePath` (D7, D8); `BaseName`/`DecodedImage` (D9); naming slugs / `cover_path` (D10);
`ZipPath`/`ZipEntry` (D11); `ImageDir`/`FileName`/`Stem` (D12); the output ids
`ManifestId`/`Idref`/`Href`/`SpineAttr`/`NavId`/`NavTitle` (D13); `Quality` (D16);
`Palette`/`PaletteIndices` (D17); `DocTitle`/`Language`/`Uuid` (D18). Cross-cutting types live in
[`src/units.rs`](../src/units.rs).

### 3.5 Guards, wildcards and impossible-state checks (E1–E16)

One `EntryContent` match per reader (E1, E2); the two root `Option`s → `RootStrip` (E3); typed
`Option`s replace lossy `unwrap_or("")`/`unwrap_or(false)` (E4); the `rsplit(..).next()` ladder →
`relative-path` (E5, [§8.1](#81-back-the-path-newtypes-with-relative-path)); the unreachable spread
`else` (E6) and `panel_style` wildcard (E7) → exhaustive matches; `write_tome`'s `bail!` gone (E8);
`put_pixel`'s wildcard → `CoverPixels` (E9); blank-image fallback → infallible `rgb_to_luma` (E10);
`Profile::entry` O(1) with no fallback row (E11); epoch fallback → `Result` (E12); `then(..).flatten()`
→ `and_then` (E13); lossy UTF-8 element name → fallible (E14); `resize_lanczos3` → `Result` (E15);
`is_image_file` → one `path_extension` (E16).

### 3.6 Duplicated conditions and derivations (G1–G8)

The spread XOR computed once (G1); `panel_view_enabled()` (G2); the dead `OpfItem` trio deleted
(G3); Kepub keyed on `OutputEncoding` (G4); the `progress_style` helper (G5); the crate-backed path
layer retires the stem/extension helpers (G6, [§8.1](#81-back-the-path-newtypes-with-relative-path));
`BatchSplit::ensure_splitting` (G7); `DeviceKind` for Kobo-family behaviour (G8).

### 3.7 Memory-safety footguns (F1–F4)

`Page`/`Chapter`/`ComicTree`/`PreparedBook`/`ProcessedBook`/`ProcessedChapter` no longer derive
`Clone`, so a payload clone is a compile error (F1); `Page::to_decoded`'s one-off copy is documented
(F2); `EncodedPage: Clone` is intentionally kept (cover-sized) (F3); the iterate-only
`options.inputs.clone()` was dropped (F4).

## 4. Non-goals (memory & SIMD)

The idiomatic refactor must **not** be applied here:

1. `PageData` keeps the bytes+pixels pairing (C1) — never collapse it to `Encoded → Pixels`, which
   would drop the bytes a decoded page legitimately keeps.
2. Never `Clone` the encoded book (C10, D11, F1): `ZipEntry`/`EpubEntries` are move-only, borrowing
   `Cow::Borrowed(page.bytes)`.
3. Never box or `dyn`-dispatch the pixel kernels (A9, G5); keep shared helpers `impl Fn`/`impl FnMut`.
4. Do not add per-pixel work in `crop.rs`/`rainbow.rs`/`webtoon.rs`/`page.rs` (A10, C11).
5. Newtypes stay transparent: no `Deref`-through-`Box`, owned conversion or allocating constructor.
6. Keep `EncodedPage: Clone` (F3); do not "fix" it into an `Arc` without a measured need.
7. Preserve output bytes; template guards became `{% if let Some(..) %}`.

## 5. Ordered refactor plan (completed)

Each phase landed green (`fmt`, `clippy -D warnings`, `nextest`) before the next began.

| Phase | Findings | Landed |
|:--|:--|:--|
| 1 Archive tag union & path identity | A1–A4, E1, E2, D7, D8, D9, E4 | `EntryKind`, `EntryContent`, `ArchiveEntry`, `NormalizedArchivePath`, `RootStripPolicy`/`RootStrip` |
| 2 Typed CLI values | B1, B2, B6, A20 | the five mode `ValueEnum`s + `ArchiveFormat`, `--borders <white\|black>`, numeric aliases |
| 3 Resolved configuration sum types | A12–A14, A18, A19, A21, B5, B15; G2, G4, G8 | `ReaderFamily`, `Geometry`, `PanelView`, `OutputEncoding`, `TitleOrigin`; `Options` split into groups ([§6.2](#62-resolved-options)) |
| 4 Geometry and unit newtypes | D2–D6, D16, D17 | the `units` module: `Size`, `BBox`, `Range`, `Percent`, `Fraction`, `Pixels`, `Bytes`, `Megabytes`, `Quality` |
| 5 Processing enums | A5–A11, C4, B13, B14 | `Detected`/`OutputColor`, `ThresholdKind`, `Axis`, `Band`, `ScribeHalf`, `Orientation`, `StripVote`, `Panel`, `FitPreference` |
| 6 Page state machine & name identity | C1–C3, D1, D14, D15, F1, F2, C12 | `PageData`, the four name newtypes, `ProcessedBook.cover: Option<Cover>`, `Clone` removal |
| 7 Output type states & stringly values | B7–B11, C7–C10, D11–D13, A15–A17, E6–E8 | `PageSide`, `WritingMode`, `PanelId`, `ManifestMediaType`, `ColorSpace`, `MobiFlags`, `Tomes`, move-only entry list |
| 8 Sweep (guards, wildcards, duplication) | E3, E9–E16, F3, F4, C6, C11, B16, A21; G1, G3, G5–G7 | `RootStrip`, `CoverPixels`, `Profile::entry`, `Reporter::Mode`, `Field`, shared helpers |
| 9 Close-out | — | this document; the `architecture.md` data-model block was updated |

**Post-review follow-ups.** Phase 1's directory classification is pinned in the `archive` tests: a
raw name ending in a backslash counts as a directory, matching how the zip/tar/7z/rar backends read
`parse_entry_info`. Phase 5 was hardened after review: `OutputColor` is an opaque newtype built only
by `OutputColor::from_detection`, the resolved `--borders` fill is the distinct `ResolvedFill`
newtype (so it cannot be swapped with the detected `Page::background`), and the
`Detected`/`ScribeHalf`/`Orientation` dispatches are exhaustive `match`es.

**Behaviour-visible deviations** (everything else is byte-identical; the goldens pass unchanged):

- **A20 — the only accepted-input change.** `--borders <white|black>` replaces
  `--black-borders`/`--white-borders`; the old two are hidden legacy flags that now *conflict*, so
  passing both is a `clap` error rather than silently letting white win.
- **A7 — the background flag is the resolved fill,** fed from `--borders` (`page_fill`), not the
  detected `Page::background`; its default is `Background::White`.
- **E15/E16 — two observable fixes.** `resize_lanczos3` returns `Result` instead of cloning on an
  impossible buffer mismatch; `is_image_file` no longer treats `.png`/non-UTF-8 names as images.
- **Sketch refinements:** `PanelView` is `{ Off, Hq, Two, Legacy }` with `--vertical-4-panel` kept a
  separate flag; `OutputEncoding::Azw3` is a unit variant (the Scribe split is
  `ProcessingOptions::scribe`); A9 is a `const`-generic `threshold_mono::<INVERTED>`; A11 is a
  `Band` struct, not a sum type.

## 6. Completed state

### 6.1 Command-line surface

Only the *values* of the mode flags changed (names stay stable for KCC compatibility). The five
integer modes (`--splitter`, `--cropping`, `--inter-panel-crop`, `--metadata-title`, `--batch-split`)
accept named values with their `0`/`1`/`2` spellings as aliases; `--gamma 0` = `auto`;
`--target-size` is `Option<Megabytes>`; `--cropping-minimum` is a `Fraction`; `--preserve-margin` is
`Option<Percent>` (`0` = off); `--jpeg-quality` is `Option<Quality>`; `--language` is `Language`;
`--borders <white|black>` replaces the two border flags; `convert --to` is a validated
`ArchiveFormat`. Everything else (`--manga`, `--hq`, the PNG/rotate/cover/strip/sizing bools, …)
keeps its `bool` surface, resolving into the enums below.

### 6.2 Resolved options

`Options` is a thin aggregate of five cohesive groups mirroring the CLI `*Args` groups, so each
stage takes only the group it reads:

```rust
pub struct Options {
    pub inputs: Vec<PathBuf>,
    pub device: DeviceOptions,         // profile, data, reader: ReaderFamily, geometry: Geometry
    pub main: MainOptions,             // manga, hq, layout, panel_view, vertical_4_panel, webtoon,
                                       // invert_direction, file_fusion, target_size
    pub processing: ProcessingOptions, // splitter, gamma, autocontrast, color, cropping, borders,
                                       // png, webp_output, jpeg_quality, strips, sizing, rotation,
                                       // cover, source, scribe, no_processing, erase_rainbow, …
    pub output: OutputOptions,         // destination, title, author, language, metadata_title,
                                       // keep_comicinfo, doc_type, encoding, batch_split, …
    pub session: SessionOptions,       // delete, temp_dir
}

impl MainOptions { pub fn right_to_left(&self) -> bool { self.manga && !self.webtoon } }
```

Derived facts are methods, computed once in `resolve`: `MainOptions::right_to_left()`,
`Options::profile_size()`/`device_size()`, `kindle_azw3()`, `panel_view_enabled()`.

### 6.3 Enums introduced

```rust
enum ReaderFamily   { Kindle, Kobo }                  // Kobo == KCC's isKobo == !isKindle
enum Geometry       { Profile(Profile), Custom { width: u32, height: u32 } }
enum Layout         { Regular, LightNovel, Wallpaper }
enum PanelView      { Off, Hq, Two, Legacy }
enum Splitter       { Split, Rotate, Both }
enum Gamma          { Auto, Linear(f32) }
enum Autocontrast   { Contrast, Off, Level }
enum Cropping       { Off, Margins, PageNumbers }
enum InterPanelCrop { Off, Horizontal, Both }
enum MetadataTitle  { Default, Combine, Only }
enum BatchSplit     { None, Auto, PerSubdirectory }
enum OutputEncoding { Epub { kfx: bool }, Kepub { short_ext: bool }, Mobi { keep_epub: bool }, Azw3, Cbz, Pdf }
```

plus the process-level enums: `EntryKind`, `EntryContent`, `RootStripPolicy`, `Size`, `IndexBox`,
`BBox`, `Range`, `Detected`, `OutputColor`, `ThresholdKind`, `Axis`, `Band`, `ScribeHalf`,
`Orientation`, `StripVote`, `Panel`, `FitPreference`, `PageSide`, `WritingMode`, `PanelId`,
`ManifestMediaType`, `ColorSpace`.

### 6.4 Booleans that stay booleans

Genuinely independent flags stay `bool`, grouped into small `Copy`+`Default` clusters
(`ColorTuning`, `PngOptions`, `Strips`, `Sizing`, `RotationOptions`, `CoverOptions`,
`SourceOptions`): `manga`, `hq`, `webtoon`, `invert_direction`, `file_fusion`, `no_processing`,
`erase_rainbow`, `keep_comicinfo`, `spread_shift`, `one_page_landscape`, `delete`, `temp_dir`, …

## 7. Testing and process

### 7.1 Quality gates

After every phase, before the next begins:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run 2>&1 | tail -n 20
```

Always pipe `nextest` through `tail` (`2>&1` is required — it writes to stderr).

### 7.5 The lint contract

```toml
[lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
```

The tree contains no `.unwrap()`, `.expect()`, `panic!`, `unreachable!` or `debug_assert!`. An
"impossible" state is therefore made unrepresentable or returned as `Result`/`Option`; `unwrap_or*`
is allowed, but a new `expect` would both fail clippy and risk the `ebook_robustness_tests`
"error, never panic" contract.

### 7.6 Per-phase test gates

Each phase ran its default suites plus the `#[ignore]`d tests for its area — e.g. Phase 3 the
format/preset → `OutputEncoding` resolution table, Phase 6 the memory-ceiling tests
(`ingest_and_repack_stay_far_below_the_decoded_book_size`,
`a_large_book_converts_under_a_memory_ceiling`), Phase 7 the byte-exact EPUB goldens.

### 7.7 Regression guardrails

- **Golden references are frozen** — never regenerate the EPUB snapshots to make a refactor pass.
- **Memory ceilings are frozen** — `ebook_robustness_tests` asserts peak RSS is linear in the encoded
  book.
- **Performance is measured, not assumed** — `scripts/bench.sh`, `examples/alloc_count` and
  `scripts/memory_bench.sh` against a baseline worktree; Phase 4 confirmed the newtypes were neutral
  (allocations `1 222 972 → 1 222 949`, identical bytes requested, peak live `401.7 → 399.3 MiB`).

## 8. Side quests

### 8.1 Back the path newtypes with `relative-path`

**Done.** The path newtypes are backed by `RelativePathBuf`, the hand-rolled separator helpers are
deleted, `safe_join` reuses the sanitizer via `to_path`, and hostile-name neutralisation is pinned
by `integration_tests::sanitizer_neutralizes_hostile_names` (see [porting.md](porting.md),
[dependencies.md](dependencies.md)). The outer newtypes stay, because the invariant they encode
(sanitized, non-empty, zip-slip-safe) is not one `RelativePath` guarantees. Two things stay bespoke:
the KCC-derived **sanitizer rules** (`RelativePath::normalize()` keeps leading `..`) and
**`safe_join`'s drop-not-pop `..` semantics** (deliberately not `to_logical_path`, pinned by
`test_safe_join`).
