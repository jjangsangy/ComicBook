# Refactor: make impossible states unrepresentable

An audit of `comic-book`, which was ported from a Python codebase (KCC's `kcc-c2e`) and
inherited its dynamic idioms. This document catalogues every place where the Rust type
system is being under-used — boolean-blind APIs, stringly-typed modes, magic numbers,
`Option` pairs that encode one state, and runtime guards that defend states the compiler
could rule out — and describes how to replace each with a compiler-checked form.

Line numbers are as of the revision this audit was taken. They will drift; the file and
function names are the durable anchors.

---

## 1. Constraints and decision rules

Every proposal below must satisfy these rules. A proposal that violates one is listed in
[§4 Non-goals](#4-non-goals-memory--simd) instead of the catalogue.

1. **Zero-cost only.** Newtypes are `#[repr(transparent)]` and compiler-erased; enums are
   fieldless/`Copy` and passed by value; type-state transitions are moves, not
   allocations. A refactor that adds a heap allocation, a `Clone`, a `Box`, or dynamic
   dispatch on a hot path is rejected.
2. **No massive copying or memory use.** The pipeline is explicitly engineered so peak RSS
   is linear in the *encoded* book, not the decoded one (see `docs/architecture.md`). A
   refactor must never retain decoded pixels longer, duplicate an encoded page, or
   force a `to_vec()` of image bytes. Where a proposed type touches `Page`/`EncodedPage`
   this rule dominates.
3. **Do not defeat SIMD.** `processing/kernels.rs` (16-lane `wide`), the FFT in
   `processing/rainbow.rs`, and the resize in `image_ops.rs` are the hot loops. An
   abstraction is acceptable only if it is monomorphised or hoisted out of the per-pixel
   loop. No `dyn Fn` predicates inside pixel loops; no bounds-check changes inside them.
4. **Behaviour and output are frozen.** The emitted EPUB/OPF/NCX/NAV/XHTML/MOBI and the
   CBZ/PDF layouts are device-sensitive (`docs/output.md`). Refactors change Rust types,
   not bytes. Askama bindings may be edited only to render byte-identical output.
5. **Every change is pinned.** Per `AGENTS.md`: a behaviour-preserving commit, covered by
   the existing tests, plus a `CHANGELOG.md` entry under `## [Unreleased]`.
6. **Panic-free — including in refactor code.** `Cargo.toml` denies the clippy
   `unwrap_used`, `expect_used` and `panic` lints, and the tree contains no `.unwrap()`,
   `.expect()`, `panic!`, `unreachable!` or `debug_assert!` (see §7.5). A refactor that
   guards an "impossible" state must make it *unrepresentable in the type* or return a
   `Result`; it must not introduce a panic, an `expect`, or an `unreachable!`.

Cost legend: **✅** compiler-erased / zero runtime cost · **➕** strictly less work or
memory than today · **⛔** rejected (would copy, allocate or de-vectorise).

---

## 2. The toolkit

Four mechanisms cover almost every finding. All are zero-cost.

### 2.1 Enums instead of booleans ("boolean blindness")

A `bool` parameter or a pair of `bool`s that must agree encodes a state in a way the
compiler cannot check. Replace with a fieldless enum and an exhaustive `match`.

```rust
// before: (above, below) admits an impossible (true, true)
let flags = |above: bool, below: bool| PageFlags { above, below, .. };

// after
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ScribeHalf { #[default] NotSplit, Above, Below }
```

Cost ✅: an enum over `bool` is the same size or smaller and still `Copy`.

### 2.2 Newtypes for confusable primitives

Several `String`/`&str`/`u32` values mean different things and are trivially swappable at
call sites (a swapped pair still compiles). Wrap them — transparently.

```rust
#[repr(transparent)]
pub struct NormalizedArchivePath(String);
impl NormalizedArchivePath { pub fn as_str(&self) -> &str { &self.0 } }
```

Cost ✅: `#[repr(transparent)]` is layout-identical; no allocation, no copy. Do **not**
add `Deref`-through-`Box`, `Clone`, or conversion-by-clone on image-carrying types.

### 2.3 Type states / sum types for mutually exclusive fields

`Option` pairs or `bool`+payload pairs that are only ever set together are an anonymous
sum type. Model them as an enum so the invalid combination is unrepresentable, and make
the state transition a move.

```rust
/// Encoded source bytes plus the media type they encode to.
pub struct Source { raw: Vec<u8>, media_type: MediaType }

enum PageData {
    Encoded(Source),                       // bytes only, not yet decoded
    EncodedDecoded(Source, DynamicImage),  // cached decode over the retained bytes
    Pixels(MediaType, DynamicImage),       // webtoon strip: pixel-only
    Consumed,                              // bytes/pixels moved out
}
```

Cost ✅ **iff** the transition moves (never clones) and the "bytes + pixels" pairing is
preserved: `EncodedDecoded` deliberately keeps the encoded `Source` alongside the pixels,
and `take_image` moves `EncodedDecoded → Encoded` (this is exactly today's `image.take()`).
Do not box the image, and do not drop the bytes on decode. See
[§4](#4-non-goals-memory--simd) for the trap.

### 2.4 Borrowed tag unions instead of `(bool, &[u8])`

A "directory" is signalled today by `is_dir == true` *and* an empty `&[u8]`. Make the
kind carry the payload:

```rust
pub enum EntryContent<'a> { Directory, File(&'a [u8]) }
```

Cost ✅: a stack tagged union holding a borrow; no allocation, still borrows the scratch
buffer.

### 2.5 Kill wildcards and impossible-state guards

Replace `match x { A => .., B => .., _ => .. }` on a `u8`/`&str`/bool-pair with an
exhaustive `match` on an enum, so adding a variant is a compile error and the `_` arm
(which today silently absorbs out-of-range values) disappears. Replace
`if let Some(x) = f()` where `x` is a function of a neighbouring value with a single
sum type.

---

## 3. Catalogue

### 3.1 Boolean blindness

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| A1 | `archive/reader.rs:7,11`; `archive/ops.rs:21,123`; `archive/formats/{zip,tar,sevenz,directory,rar}.rs`; `archive/writer.rs:49,61,73` | `EntryCallback = FnMut(&str, bool, &[u8])` encodes directory-ness as a `bool` plus an empty-slice sentinel; the "directories carry no data" invariant is prose only and is re-asserted at every producer/consumer. | `FnMut(&str, EntryContent<'_>)` ([§2.4](#24-borrowed-tag-unions-instead-of-bool-u8)). `ArchiveWriter::add_entry`/`add_entry_normalized`/`dispatch` take `EntryContent`. | ✅ |
| A2 | `archive/reader.rs:21`; `archive/path.rs:90,103`; `archive/ops.rs:31` | `Vec<(String, bool)>` and `parse_entry_info(..) -> Option<(String, bool)>` put a positional `bool` in a public tuple. | `struct ArchiveEntry { name, kind: EntryKind }` (`EntryKind::File\|Directory`). `find_single_root_dir(&[ArchiveEntry])`. | ✅ |
| A3 | `archive/ops.rs:44,53,75` | `strip_common_root: bool` — call sites read `convert_archive_ext(.., false)`. | `enum RootStripPolicy { Never, Always, IfMatchingDestination }`. | ✅ |
| A4 | `ebook/input/archive.rs:126-133` (calls `:77`, `input/epub.rs:50`, `input/pdf.rs:77,162`) | `build_tree(.., strip_root: bool)`; call sites pass `kind != ArchiveKind::Directory`, `false`, `false`. | `enum RootStrip { Strip, Keep }` (or pass the source layout). | ✅ |
| A5 | `ebook/input/pdf.rs:85-108` | `render_zoom(pdf_width: bool, ..)` selects a fit strategy; `render_target` matches `cropping` as `1`/`2`/`_`. | `enum FitPreference { Height, WidthForPortrait }`; `Cropping` enum (B1). | ✅ |
| A6 | `model.rs:103-107`; `processing/page.rs:325-331,353,362,375,389`; `chunk.rs:174`; `output/epub/mod.rs:98-106`; `output/epub/opf.rs:167` | `PageFlags.above`/`.below` are a 3-state value represented as two `bool`s; `(true,true)` is representable. Consumers re-derive the pairing three ways. | `enum ScribeHalf { NotSplit, Above, Below }`; `flags(half)`. | ✅ |
| A7 | `model.rs:104,137`; `processing/page.rs:328`; `output/epub/xhtml.rs:64` | `PageFlags.black_background: bool` duplicates `Page::background: Background`. | Store `Background` in `PageFlags` (or drop the flag); match `Background::Black`. | ✅ |
| A8 | `processing/page.rs:313,321-322,394-402,403,409,414`; `processing/rainbow.rs:34`; `processing/mod.rs:185-186`; `processing/color.rs:33` | `color` (detected) and `color_output` (retained) are threaded as two same-typed `bool`s; `color_output ⟹ color` is enforced only at the computation site. `color_check(.., original_is_grayscale: bool)` is passed literal `false` by every production caller (only tests pass `true`). | `enum Detected { Gray, Color }` + `enum OutputColor { Gray, Color }` with a single `OutputColor::from_detection(Detected, force_color)` constructor; drop the dead `original_is_grayscale` parameter. | ✅ |
| A9 | `processing/kernels.rs:107` (calls `processing/crop.rs:235`, `processing/webtoon.rs:325`) | `threshold_in_place(data, threshold, inverted: bool)`; the doc comment already names the two modes. | `enum ThresholdKind { Above, Below }`; the branch is loop-invariant and hoistable (e.g. `<const INVERTED: bool>`). **Do not** use a `dyn` predicate. | ✅ |
| A10 | `processing/interpanel.rs:42,81,123` | `empty_sections(.., horizontal: bool)`, `keep_lines(.., remove_rows: bool)`, `remove_lines(.., remove_rows: bool)` are two inverted spellings of one axis. | `enum Axis { Rows, Columns }`. | ✅ |
| A11 | `processing/kernels.rs:457` (uses `interpanel.rs:293`) | `band_white_black(..) -> (bool, bool)` — a transposed destructure flips the detector. | `struct Band { has_white: bool, has_black: bool }` or `enum BandKind`. | ✅ |
| A12 | `options.rs:86-88,170-171`; `output/epub/opf.rs:129,187,277-283`; `output/epub/xhtml.rs:85`; `output/lightnovel.rs` | `is_kindle`/`is_kobo`/`device_kind` triplicate one fact; `is_kobo = !is_kindle` so `(false,false)`/`(true,true)` are reachable-but-wrong. The final `else` in `page_spread_property` is therefore dead. | Keep `device_kind`; add `DeviceKind::is_kindle()`/`is_kobo_like()`. Or `enum ReaderFamily { Kindle, Kobo }`. | ✅ |
| A13 | `options.rs:98-104,119-125,157-164` | "Derived output flags" (`kfx`/`kepub`/`keep_epub`/`kindle_azw3`/`kindle_scribe_azw3`/`webp_output`) and the panel/PNG clusters are mutually-constrained bools computed piecewise. | `enum OutputEncoding { Epub, Kepub{..}, Kfx, Mobi{..}, Azw3{scribe}, Cbz, Pdf }` + orthogonal `webp`; `enum PanelView { Off, Two, Vertical4, Legacy }`; `enum PngPolicy`. | ✅ |
| A14 | `ebook/mod.rs:169-177` (calls `:60-68`, `:120-128`) | `assemble(.., default_title: Option<&str>, fusion: bool, ..)` — `fusion` is always `true` exactly when `default_title.is_some()`. | `enum TitleOrigin<'a> { Derived, Fusion(&'a str) }`. | ✅ |
| A15 | `output/kindle.rs:87-104` | `build_mobi_from_extracted` is called with fifteen positional `bool`s plus `None`; comments are the only guard against transposition. | A named `struct MobiFlags { .. }` built once from `options`, applied in order. (`kindling`'s positional API is fixed.) | ✅ |
| A16 | `output/pdf.rs:120-127,185-195` | `PdfImage.gray: bool` and `jpeg_components(..) -> Option<u8>` where only `== 1`/`== 3` are produced. | `enum ColorSpace { Gray, Rgb }`; `jpeg_color_space(..) -> Option<ColorSpace>`. | ✅ |
| A17 | `output/mod.rs:42,60,87`; `output/kindle.rs:39` | `drop_bookmarks: bool` is a trailing unlabelled parameter threaded through three layers. | Pass `enum Tomes { Single, Split }` derived from `total > 1`. | ✅ |
| A18 | `output/mod.rs:151`; `output/kindle.rs:43` | `keep_epub` is the residue of `mobi+epub` flattened in `Options::resolve`. | Fold into `OutputEncoding::Mobi { keep_epub }` (A13). | ✅ |
| A19 | `output/epub/templates.rs:51-78,159-166`; `output/epub/opf.rs:185-188`; `output/epub/xhtml.rs:98` | `StyleCss{scribe,panel}` and `PageXhtml.kindle_spacer` re-derive `kindle_scribe_azw3`, `is_kindle && panel_view`, `is_kindle` at each site (two modules can drift). | One resolved value/predicate (e.g. `Options::panel_view_enabled()`), or carry the computed flags. | ✅ |
| A20 | `ebook/cli.rs:156-162`; `ebook/options.rs:248-254` | `--black-borders` and `--white-borders` are separate `bool`s; both may be passed and white silently wins. | `#[arg(long, value_enum)] borders: Option<BorderColor>` (or an `ArgGroup`). **Behaviour caveat:** rejects the previously-accepted "both" input. | ✅ |
| A21 | `ebook/options.rs:90,299-310`; `ebook/profiles.rs:539` | `custom_profile: bool` plus the literal name `"Custom"` both encode "geometry came from `--custom-*`". | `enum Geometry { Profile(Profile), Custom { width, height } }`. | ✅ |

### 3.2 Stringly-typed values and magic numbers

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| B1 | `ebook/options.rs:108,113,117,143,149`; `ebook/cli.rs:107-114,126-133,148-154,277-283,315-323`; consumers below | Five `u8` fields each constrained by clap to `0..=2` and then re-interpreted by integer literals with `_` arms: `splitter`, `cropping`, `inter_panel_crop`, `metadata_title`, `batch_split`. | `ValueEnum`s: `Splitter { Split, Rotate, Both }`, `Cropping { Off, Margins, PageNumbers }`, `InterPanelCrop { Off, Horizontal, Both }`, `MetadataTitle { Default, Combine, Only }`, `BatchSplit { None, Auto, PerSubdirectory }`. | ✅ |
| B2 | `processing/page.rs:181,188,201`; `output/epub/mod.rs:238-241`; `input/pdf.rs:85-89`; `metadata.rs:196,212`; `chunk.rs:39,56,59` | The integer modes above are matched with `==1`/`!=1`/`>0`/`_ => 0` in seven places. | The enums from B1 make each `match` exhaustive. | ✅ |
| B3 | `chunk.rs:48-72` | `mode` starts as a page-path *depth*, is mutated into a *split unit*, then tested with `mode >= 3`. | `enum SplitUnit { Page, Chapter, TopLevel }`. | ✅ |
| B4 | `chunk.rs:82-99` | `image_level(..) -> (usize, bool)`; the `bool` is "mixed", the seed `level = 1` is dead when pages exist. | `enum Level { Uniform(usize), Mixed }`. | ✅ |
| B5 | `ebook/options.rs:15-45,200-295,330`; `output/mod.rs:89-158` | Resolved `Options.format` is still the request-level `Format`, so after `resolve` six preset variants are unreachable but still matched; `write_tome` ends in `other => bail!(..)`. | `enum ResolvedFormat { Epub, Mobi, Azw3, Cbz, Pdf }` for `Options.format`; preset expansion on the request `Format`. | ✅ |
| B6 | `src/cli.rs:31-33`; `src/convert.rs:296-311` | `--to` is a `String` re-parsed by `parse_target_extension`, with a second copy of the accepted list in the error string. | `#[derive(ValueEnum)] enum ArchiveFormat`; reuse `ArchiveKind`. | ✅ |
| B7 | `output/epub/opf.rs:198-260,264-273,276-284` | Page sides `"left"`/`"right"`/`"center"` flow as `&'static str` through a push loop, a comparison-based `other()`, and `format!`. A typo is not a compile error. | `enum PageSide { Left, Right, Center }` with `other()`/`as_str()`; `enum Direction { Ltr, Rtl }`. | ✅ |
| B8 | `output/epub/opf.rs:53-71`; `templates.rs:108,111` | `writing_mode`/`direction` are `format!`-assembled from `horizontal|vertical`, `-lr|-rl`, `ltr|rtl`; the `(invert_direction, right_to_left)` XOR match is written twice. | `enum WritingMode { .. }`; compute `let rtl = invert_direction ^ right_to_left` once. | ✅ |
| B9 | `output/epub/templates.rs:81-85`; `output/epub/xhtml.rs:151-205` | `PanelBox.id` is a string keyed against parallel `&[&'static str]`/`&[u32]` arrays, then re-matched in `panel_style` with a `_ => String::new()` wildcard. | `enum PanelId { Tl, Tr, Bl, Br, T, B, L, R }` with `style()`; no wildcard. | ✅ |
| B10 | `output/epub/templates.rs:121`; `output/epub/opf.rs:154,162,173` | `OpfItem.media_type: &'static str` mixes the literal `"application/xhtml+xml"` with `MediaType::mime()`. | `enum ManifestMediaType { Xhtml, Ncx, Css, Image(MediaType) }`. | ✅ |
| B11 | `output/epub/opf.rs:133`; `templates.rs:109`; `templates/content.opf:24` | `region_mag` is stored as the strings `"true"`/`"false"`. | `bool` field; askama renders `true`/`false` identically. | ✅ |
| B12 | `naming.rs:51-59` | `slugify(value, format: Format, ..)` only ever asks "is this CBZ?", coupling naming to the whole request enum. | `enum NameStyle { Cbz, Slug }`. | ✅ |
| B13 | `processing/webtoon.rs:50` | `type Panel = (u32, u32, u32)` with `height == bottom - top` recomputed everywhere. | `struct Panel { top, bottom }` with `fn height()`. | ✅ |
| B14 | `processing/fill.rs:108-112` | `border_fill`/`strip_vote` returns `-1/0/+1` magic tri-state. | `enum StripVote { Black, Mixed, White }`. | ✅ |
| B15 | `ebook/profiles.rs:584-592` | `is_kobo_brand()`/`is_scribe()` are string-prefix tests on the human-facing profile code (`"Ko"`, `"KS"`). | Add a `scribe: bool` (or kind) to `ProfileEntry`; delegate `is_kobo_brand` to `DeviceKind`. | ✅ |
| B16 | `ebook/metadata.rs:23-33,74-122` | The same nine `ComicInfo` field names appear as string literals in three separate blocks (list, capture, removal); a typo yields silently-empty metadata. | `enum Field { Series, Volume, .. }` + `Field::from_name` + `HashMap<Field, String>`. | ✅ |

### 3.3 Type states and invalid states

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| C1 | `model.rs:128-148,167-178,183-194,197-199`; `processing/mod.rs:127-128,141-144`; `processing/page.rs:99-129` | `Page` holds `image: Option<DynamicImage>` **and** `raw: Option<Vec<u8>>` **and** `source_media_type: Option<MediaType>`. Four combinations are representable, three are used, and `(None,None)` is defended by `.context("page holds neither …")` at four sites. | `enum PageData { Encoded(Source), EncodedDecoded(Source, DynamicImage), Pixels(MediaType, DynamicImage), Consumed }` ([§2.3](#23-type-states--sum-types-for-mutually-exclusive-fields)). `ensure_decoded`/`take_image`/`passthrough_in_place` become matches. **Must keep the bytes+pixels pairing** (see §4). | ✅ |
| C2 | `model.rs:231-239,245`; `input/archive.rs:136`; `input/fusion.rs:82-86` | `ComicTree.cover: Option<CoverSource>` is never read; only `FirstPage`/`None` are ever constructed, `Sibling`/`Fused` have no constructors. | Delete the field and `CoverSource` (cover selection flows through `PreparedBook.cover_override` / `processing::cover`). | ➕ |
| C3 | `processing/mod.rs:41-51,109-114,219-222`; `processing/cover.rs:29-32` | `ProcessedBook.cover: Option<EncodedPage>` + `cover_smart_crop: bool` is the same fact as `cover::Cover { page, smart_cropped }`, split apart and re-joined. | `cover: Option<cover::Cover>`; drop `cover_smart_crop`. | ✅ |
| C4 | `processing/page.rs:57-61,175,190,195,207,256-259` | `Payload.rotated: bool` is only meaningful for the rotate orders, but `--no-rotate` can leave `RotateLast` tagged but upright, so it is not derivable from `OrderClass` alone. | `enum Orientation { Upright, Rotated }` beside `OrderClass` (two honest axes). | ✅ |
| C5 | `archive/reader.rs:18-21`; `archive/ops.rs:111,123`; `archive/formats/sevenz.rs:11-27` | The trait exposes `list_entries(&mut self)` and `read_entries(&mut self, ..)`; whether a reader is still streamable after listing is a doc comment, not a type. | Make streaming consuming (`into_entries(Box<Self>, ..)`) if the list-then-stream reuse is not required; otherwise document the state. Low priority. | ✅ |
| C6 | `ebook/progress.rs:102-108,138-145` | `Reporter` has `multi: Option<MultiProgress>` and `overall: Option<ProgressBar>` that are only ever both-`Some` (batch) or both-`None` (standalone); `child`'s `_` arm silently treats a half-set reporter as standalone. | Private `enum Mode { Batch { multi, overall, lines }, Standalone }`. | ✅ |
| C7 | `output/epub/templates.rs:118-125`; `output/epub/opf.rs:147-180` | `OpfItem.properties`/`has_properties_before`/`has_properties_after` are set to empty/false in all three construction sites; the real properties live in the template. Nine fields of dead state, repeated. | Delete the three fields (output is byte-identical) or model `enum PropertyPos { None, Before(String), After(String) }`. | ➕ |
| C8 | `output/epub/templates.rs:67-72`; `output/epub/xhtml.rs:73-105` | `PageXhtml.has_below` must agree with `below_image_src`/`below_img_width`/`below_img_height`; the `None` case is spelled `"",0,0` by hand. | `below: Option<BelowImage<'a>>`. | ✅ |
| C9 | `output/epub/templates.rs:95-103`; `output/epub/opf.rs:92-113` | `has_description`/`description`, `has_series`/`series`/`has_group`/`group` are bool+payload pairs; `has_group` re-derives `has_series && group.is_some()`, and group is nested inside series. | `description: Option<&str>`; `series: Option<Series<'a>>` with `group: Option<&str>`. | ✅ |
| C10 | `output/epub/package.rs:21-25`; `output/epub/mod.rs:75-82,141-219` | The OCF "`mimetype` first, stored" rule is enforced only by convention in a doc comment over a bare `Vec<(String, Cow<[u8]>)>`. | `struct EpubEntries<'a>(Vec<ZipEntry<'a>>)` built only by `build_entries`; `write_epub` writes `mimetype` first. **Do not derive `Clone`.** | ✅ |
| C11 | `processing/webtoon.rs:247,252-283` | `detect_panels` keeps `panel_detected: bool` and `panel_top: u32` in lockstep by hand; `step == 0` is prevented only by the caller's width guard. | `let mut open_panel: Option<u32>`; a validated `StripWidth` newtype. | ✅ |
| C12 | `input/fusion.rs:26-33,77-78` | `Fused.source` is `parent.join(&title)` — the title is redundant with the source's file name and both are threaded around. | Keep one and derive the other. | ➕ |

### 3.4 Newtypes

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| D1 | `model.rs:125,127`; `input/archive.rs:86`; `model.rs:224` | `Page.source_name` (book-relative) and `Page.rel_path` (chapter-relative) are both `String`; `LoadedPage.name` and `EncodedPage.name` are the same ambiguity. A swap compiles. | `#[repr(transparent)] SourceName(String)` / `RelPath(String)` / `PageName(String)`. | ✅ |
| D2 | `model.rs:136`; `input/archive.rs:91,112`; `input/pdf.rs:114`; `processing/page.rs:47,750,773,788`; `processing/cover.rs:97`; `webtoon.rs:145`; `output/epub/opf.rs:49,130-131`; `output/lightnovel.rs:83-90` | `(u32, u32)` and adjacent `u32` width/height pairs are everywhere; a transposition is silent and some are device-sensitive. | `#[derive(Clone, Copy)] struct Size { width: u32, height: u32 }`; `resize_to(image, size, ..)`. | ✅ |
| D3 | `processing/crop.rs:47,532`; `processing/kernels.rs:288,381`; `processing/fill.rs:55` | At least four meanings share positional 4-tuples: inclusive index box `(x1,x2,y1,y2)`, Pillow bbox `(l,u,r,b)`, clipped `usize` rect, float rect. | `struct IndexBox { x1, x2, y1, y2 }` and `struct BBox { left, upper, right, lower }` with explicit `to_pillow()`. | ✅ |
| D4 | `processing/kernels.rs:39,65`; `processing/page.rs:497`; `processing/color.rs:126` | `(u8, u8)` min/max; a swapped pair silently changes contrast guards. | `struct Range { min: u8, max: u8 }` with `is_degenerate()`. | ✅ |
| D5 | `processing/crop.rs:545,555`; `interpanel.rs:35`; `color.rs:114` | Percent (`preserve_margin`, `CROP_CUTOFF`) and fraction (`INTER_PANEL_KEEP`) floats cross the same boundaries as bare numbers, with `0` as "off". | `struct Percent(f64)` / `struct Fraction(f64)`; `Option<Percent>` for `preserve_margin`. | ✅ |
| D6 | `src/cli.rs:47-48`; `ebook/chunk.rs:122-128,191-198`; `src/clamp.rs:196-204` | Pixels, bytes and megabytes are all `u64`/`u32`; nothing stops a pixels value reaching a byte comparison. | `struct Pixels(u64)`, `struct Bytes(u64)`, `struct Megabytes(u32)`. | ✅ |
| D7 | `archive/path.rs:10`; `archive/writer.rs:49,61` | Raw and normalized archive paths are both `&str`; `add_entry_normalized` silently produces a wrong archive if handed a raw name. | `#[repr(transparent)] NormalizedArchivePath(String)`; typed `add_entry_normalized`. | ✅ |
| D8 | `archive/path.rs:10-32,92`; `archive/writer.rs:51,67` | `normalize_archive_path` returns `String` with `""` as the invalid sentinel; every caller re-checks `is_empty()`. | Return `Option<NormalizedArchivePath>`. | ✅ |
| D9 | `archive/ops.rs:175,183` | `get_images_from_source(..) -> Vec<(String, DynamicImage)>` where the `String` is a *basename* (elsewhere a full path). | `struct DecodedImage { name: BaseName, image: DynamicImage }`. | ✅ |
| D10 | `ebook/naming.rs:36-44` | `Sanitized.chapter_titles: HashMap<String, String>` (slug → raw basename, opposite meanings) and `cover_path: Option<String>` (a path held as `String`). | `Slug`/`RawName` newtypes; `cover_path: Option<RelPath>`. | ✅ |
| D11 | `output/epub/mod.rs:82,141-219`; `output/epub/package.rs:25`; `output/kindle.rs:131` | The zip entry `(String, Cow<'a, [u8]>)` is anonymous; `write_tree` re-derives paths from the same shape. | `struct ZipEntry<'a> { path: ZipPath, data: Cow<'a, [u8]> }`. **Do not derive `Clone`/`to_vec` — it holds borrowed image bytes.** | ✅ |
| D12 | `output/epub/mod.rs:30-45` | `PageRef` carries three confusable `&str`: `image_dir`, `file`, `stem`. | `ImageDir<'a>`/`FileName<'a>`/`Stem<'a>` newtypes; `PageRef` stays `Copy`. | ✅ |
| D13 | `output/epub/templates.rs:118-131,152-156` | `OpfItem{id,href}`, `SpineItem{idref,attr}`, `NavEntry{id,title,source}` are adjacent same-typed `String`s. | `ManifestId`, `Href`, `NavTitle` newtypes with the `page_`/`img_`/`-below` conventions in one place. | ✅ |
| D14 | `model.rs:207`; `chunk.rs:151-154,237-246` | `Chapter.name: String` uses `""` as the root-chapter sentinel, special-cased in consumers. | `ChapterName` newtype or `enum ChapterPath { Root, Dir(String) }`. | ✅ |
| D15 | `input/archive.rs:84-92` | `LoadedPage` duplicates `Page`'s carrier fields (`name`/`media_type`/`raw`/`dimensions`) as a staging row. | Once C1 lands, hold the `PageData` carrier plus the name. | ✅ |
| D16 | `processing/page.rs:1006,1103`; `processing/cover.rs:119` | `quality: u8` is JPEG quality, WebP quality and PNG depth interchangeably. | `struct Quality(u8)` validated once in `Options::resolve`. | ✅ |
| D17 | `processing/page.rs:890-895,964,1032` | A palette is `&[u8]` flat triples in, `Vec<[u8;3]>` in the result, re-flattened; `indices: Vec<u8>` is palette indices, not bytes. | `struct Palette(Vec<[u8; 3]>)`, `struct PaletteIndices(Vec<u8>)` (wrap by move only). | ✅ |
| D18 | `output/epub/nav.rs:79-97`; `output/epub/opf.rs:38-48` | `title`, `language`, `uuid` are three interchangeable `&str` parameters. | `DocTitle<'a>`/`Language<'a>`/`Uuid<'a>` newtypes. | ✅ |

### 3.5 Guards, wildcards and impossible-state checks

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| E1 | `archive/formats/zip.rs:37-44`, `tar.rs:52-60`, `sevenz.rs:35-47`, `directory.rs:46-58`; `rar.rs:61-84` | Each reader hand-writes `if is_dir { on_entry(name, true, &[]) } else { .. }`; the RAR reader matches the `bool` as literal `true`/`false` patterns. | `EntryContent` ([§2.4](#24-borrowed-tag-unions-instead-of-bool-u8)); one `match` per backend. | ✅ |
| E2 | `archive/formats/directory.rs:46-50` | The computed `is_dir` from `parse_entry_info` is discarded (`_`) and recomputed from `file_type()` two lines later. | Return/consume an `EntryKind`; branch on it. | ✅ |
| E3 | `archive/ops.rs:108-137` | `root_to_strip: Option<String>` and the derived `root_prefix: Option<String>` are two `Option`s for one value; the inner `if let Some(prefix)` can never be `None` when entered. | `struct RootStrip { name, prefix }` in one `Option`. | ✅ |
| E4 | `archive/ops.rs:100-101,81,112` | `dest.file_name()...unwrap_or("")`/`src.file_stem()...unwrap_or("")` collapse "absent" and "empty"; `same_file(...).unwrap_or(false)` and `.ok()` turn IO failures into silently-different conversion paths. | Keep `Option<&str>` (typed `DestName`/`SourceStem`) and let `is_matching_root` take `Option`s; make degraded paths explicit. | ✅ |
| E5 | `archive/path.rs:81`; `archive/ops.rs:183`; `input/archive.rs:143`; `input/epub.rs:230`; `naming.rs:360`; `chunk.rs:288`; `output/epub/mod.rs:109,158`; `xhtml.rs:75`; `opf.rs:169` | `.rsplit(['/','\\']).next().unwrap_or(name)` is repeated everywhere; `rsplit` always yields ≥1 item, so the fallback is dead and hides the invariant. | One `fn file_name(&str) -> &str` helper (or a `PageName` method); ideally derive once at `PageRef` construction (D12). | ✅ |
| E6 | `output/epub/opf.rs:276-284` | `page_spread_property`'s `else { String::new() }` is unreachable because `is_kobo == !is_kindle`. | Exhaustive `match options.reader` (A12) over `PageSide` (B7). | ✅ |
| E7 | `output/epub/xhtml.rs:193-205` | `panel_style`'s `_ => String::new()` hides typos and future ids. | `match PanelId { .. }` (B9), no wildcard. | ✅ |
| E8 | `output/mod.rs:157` | `write_tome`'s `other => bail!("internal error: unresolved output format")` defends an impossible state. | `ResolvedFormat` (B5) makes the match exhaustive. | ✅ |
| E9 | `processing/cover.rs:218-224` | `put_pixel`'s `_ => {}` silently drops any pixel type other than L8/Rgb8. | Narrow to a `CoverPixels` enum produced once by `draw_label`, so the arm cannot exist. No `debug_assert!`/`unreachable!` (see §1 rule 6). | ✅ |
| E10 | `processing/color.rs:188-193` | `GrayImage::from_raw(..).unwrap_or_else(|| GrayImage::new(..))` builds a blank image on a bug (the length is `width*height` by construction). | Build the buffer with `ImageBuffer::from_fn` (infallible) or return `Result`; **not** `expect` — the crate denies `clippy::expect_used`. | ✅ |
| E11 | `ebook/profiles.rs:552-557` | `Profile::entry` linearly searches a parallel array and falls back to row 0 on a miss; a table typo silently maps a profile to the wrong row. | Exhaustive `match self { Profile::K1 => &PROFILE_TABLE[0], .. }` (keep `ALL_PROFILES` only for clap); no fallback, O(1). | ✅ |
| E12 | `output/epub/mod.rs:222-228` | `OffsetDateTime::...format(format).unwrap_or_else(..)` on a compile-time constant format; the epoch fallback allocates. | Thread `Result` upward (the error is unreachable for a constant format); keep the epoch branch only as a documented last resort — no `expect` (denied). | ✅ |
| E13 | `output/kindle.rs:123-126` | `options.temp_dir.then(\|\| source.parent()).flatten()` obscures an `and_then`. | `source.parent().filter(\|_\| options.temp_dir)` in one `match`. | ✅ |
| E14 | `ebook/metadata.rs:312-318` | `std::str::from_utf8(name).unwrap_or("")` makes a malformed element name silently fail all comparisons. | Return `Result`/`Option`; let `parse` apply the "discard malformed ComicInfo" rule. | ✅ |
| E15 | `src/image_ops.rs:53-70` | `resize_lanczos3` has three `img.clone()` fallbacks for conditions the doc calls impossible; each is a full pixel-buffer copy. | Return `Result` so the impossible state is loud rather than a silent full-image clone. | ➕ (removes a hidden clone) |
| E16 | `src/image_ops.rs:18-34` | `is_image_file` re-derives the extension by lossy string surgery after `Path::extension()` returned nothing. | One `fn image_extension(&Path) -> Option<&str>`; no `to_string_lossy` ladder. | ✅ |

### 3.6 Duplicated conditions and derivations

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| G1 | `output/epub/opf.rs:60-63,68-71` | The `(invert_direction, right_to_left)` XOR match is written twice with `_` arms. | `let rtl = invert_direction ^ right_to_left;` once (B8). | ✅ |
| G2 | `output/epub/xhtml.rs:85`; `output/epub/opf.rs:187` | `options.is_kindle && options.panel_view` re-derived in two modules that must stay in sync. | One `Options::panel_view_enabled()` (A19). | ✅ |
| G3 | `output/epub/opf.rs:154-157,163-166,174-177` | The empty `OpfItem` property trio is repeated verbatim. | Delete (C7). | ➕ |
| G4 | `ebook/naming.rs:212,227` | `options.format == Format::Epub && options.kepub` re-tested in one function. | Resolved `OutputEncoding::Kepub` (A13). | ✅ |
| G5 | `src/convert.rs:260-263`; `src/clamp.rs:291-296`; `ebook/progress.rs:64-69,74-81,216-221` | The same `ProgressStyle::default_bar().template(..).unwrap_or(default)` idiom is copied 4+ times. | One `fn style(..)` (optionally `OnceLock`). | ✅ |
| G6 | `ebook/mod.rs:254-260`; `processing/webtoon.rs:147-152`; `processing/page.rs:1195-1198`; `input/archive.rs:142` | Three subtly different path-stem/extension helpers. | One `fn stem(&str) -> &str` + typed names (D1). | ✅ |
| G7 | `ebook/options.rs:204-206,212-214,219-221,257-259` | `if batch_split != 2 { batch_split = 1 }` repeated across preset arms. | `BatchSplit::ensure_splitting()` (B1). | ✅ |
| G8 | `ebook/options.rs:284-287`; `output/epub/opf.rs:101` | "Kobo family" behaviour is keyed off `is_kobo_brand()`/`is_kindle` derived bools in several places. | `DeviceKind` methods (A12/B15). | ✅ |

### 3.7 Memory-safety footguns (derive hygiene)

| # | Location | Issue | Fix | Cost |
|:--|:--|:--|:--|:--|
| F1 | `model.rs:118` | `Page` derives `Clone`, so a future `page.clone()` silently duplicates a full decoded frame plus encoded bytes. No in-tree caller does today. | Drop `Clone` from `Page`/`Chapter`/`ComicTree` (and never add it to `PageData`); make any genuine owned copy a named method. | ✅ |
| F2 | `model.rs:183-188` | `Page::to_decoded` clones an already-decoded `DynamicImage` for one-off passes. The clone is deliberate but is the one place a fix *reduces* cost. | A borrow- or move-returning variant (behaviour-adjacent; keep the owned API where required). | ➕ |
| F3 | `model.rs:216`; `chunk.rs:266-275`; `processing/cover.rs` | `EncodedPage` derives `Clone` and the per-tome labelled cover deep-copies its bytes. Acceptable (cover-sized), but the type does not signal cheap sharing. | Leave as-is; if a cover ever grows, use `Arc<[u8]>` as `input/epub.rs:33` already does. | ✅ (no change) |
| F4 | `ebook/mod.rs:87` | `for source in options.inputs.clone()` clones the whole input `Vec<PathBuf>` just to iterate. | `for source in &options.inputs`; the body already takes `&source`. | ➕ |

---

## 4. Non-goals (memory & SIMD)

These are the cases where the *apparently* idiomatic refactor must **not** be applied, or
must be applied in a specific restricted way, to honour the constraints in
[§1](#1-constraints-and-decision-rules).

1. **`PageData` must preserve the bytes+pixels pairing (C1).** Today a decoded page
   legitimately holds both its encoded `raw` and its `DynamicImage`, and `--no-processing`
   moves `raw` out *without* decoding. Do not "simplify" the state machine into
   `Encoded → Pixels` (which would drop the bytes on decode) or into a representation that
   re-decodes per consumer: that breaks the "peak RSS is linear in the encoded book"
   guarantee pinned by the memory tests (`docs/architecture.md`, `docs/porting.md`).
   The enum is only acceptable if every transition is a `mem::replace`/move and
   `EncodedDecoded` keeps both.
2. **Never `Clone` the encoded book (C10, D11, F1).** `build_entries` deliberately builds
   a `Vec` of entries that borrow the encoded pages (`Cow::Borrowed(page.bytes)`), so
   packaging does not duplicate the book. Any new wrapper (`ZipEntry`, `EpubEntries`) must
   be move-only; do not derive `Clone`, do not add `to_vec`, do not change `Cow::Borrowed`
   into an owned slice.
3. **Do not box or dynamically dispatch the pixel kernels (A9, G5).** `kernels.rs`'s
   `min_max`/`invert_in_place`/`threshold_in_place` are 16-lane `wide` loops. Enum- or
   `const`-generic parameterisation is zero-cost because the branch is loop-invariant.
   Replacing a `bool` with an `impl Fn`/`&dyn Fn` predicate, or boxing a LUT closure, would
   put an indirect call inside the per-pixel loop and is rejected. Keep every shared
   helper (`map_luma_or_rgb_in_place`, `count_where`, `bbox_where`) generic
   (`impl Fn`/`impl FnMut`, monomorphised) — never `dyn`.
4. **Do not add work to the crop/FFT loops (A10, C11).** The `Axis`/`ScribeHalf` enums and
   `Option<u32>` panel state are matched once per row/band, never per pixel. Do not
   introduce per-pixel bounds checks or index arithmetic changes in `crop.rs`,
   `rainbow.rs`, `webtoon.rs` or `page.rs`.
5. **Newtypes stay transparent.** `#[repr(transparent)]` over the existing `String`/`&str`/
   `u32`/`Cow`. No `Deref`-through-`Box`, no owned conversion, no `Clone`, no allocating
   constructor on a hot path.
6. **Keep `EncodedPage: Clone` (F3).** The cover clone is bounded and intentional. Do not
   "fix" it into an `Arc` unless a cover actually grows; switching carriers now would add
   indirection on the output path for no measured gain.
7. **Preserve output bytes.** Every askama field-type change (C8, C9, B10, B11) must render
   identically; the template guards become `{% if let Some(..) %}`. Anything that would
   alter the OPF/NCX/NAV/XHTML/MOBI bytes is out of scope regardless of how much cleaner
   the type is.

---

## 5. Ordered refactor plan

The phases run strictly top to bottom. Each is a self-contained, behaviour-preserving unit
that lands green (`cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo nextest run`) before the next begins. A phase assumes every earlier phase has
landed; explicit dependencies are called out. The finished shape is described in
[§6](#6-completed-state-flags-and-options).

### Phase 1 — Archive tag union and path identity

**Status:** ✅ **Complete** — landed on `rusty-refactor` in `3bf0919` ("phase 1 complete").

- **Findings:** A1, A2, A3, A4, E1, E2, D7, D8, D9, E4.
- **Scope:** `src/archive/**` plus the call sites `ebook/input/archive.rs`,
  `ebook/output/cbz.rs`, `ebook/output/lightnovel.rs`.
- **Deliverable:** `EntryKind { File, Directory }`, `EntryContent<'a> { Directory, File(&[u8]) }`,
  `ArchiveEntry { name, kind }`; `NormalizedArchivePath` with
  `normalize_archive_path -> Option<NormalizedArchivePath>`; a `RootStripPolicy`/`RootStrip`
  replacing the two `strip_root` bools; typed `add_entry`.
- **Gate:** archive + `ebook_input`/`ebook_robustness` tests, plus new unit tests for the tag
  union and empty-path rejection.
- **Why first:** module-local, no config coupling; it establishes the enum/newtype
  conventions every later phase reuses.

**Completed notes.**

- **Delivered as planned:** `EntryKind`, `EntryContent<'a>`, `ArchiveEntry`,
  `NormalizedArchivePath` (`Option` return), `RootStripPolicy` (`convert_archive_ext`) and
  `RootStrip` (`build_tree`), and a typed `ArchiveWriter::add_entry`/`add_entry_normalized`. The
  ten findings (A1, A2, A3, A4, E1, E2, D7, D8, D9, E4) are addressed; E4 keeps `Option<&str>` and
  makes the degraded `same_file`/listing paths explicit rather than silent (see below).
- **Refinements over the sketch:**
  - A1's `FnMut(&str, EntryContent)` became `FnMut(&NormalizedArchivePath, EntryContent)`: the
    reader's names are already normalized, so carrying the typed path end-to-end is what lets
    `add_entry_normalized` take `&NormalizedArchivePath` (D7) without re-parsing.
  - `NormalizedArchivePath` also implements `Deref<Target = str>` (not in §2.2's sketch) so a
    normalized path can be read through `Option::as_deref()` alongside `str`, as the
    `integration_tests::test_normalize_archive_path` assertions do. The construction invariant is
    untouched: it still cannot be built from an arbitrary `&str`.
  - Placement: `EntryKind`/`ArchiveEntry`/`NormalizedArchivePath` in `archive/path.rs`,
    `EntryContent` in `archive/reader.rs`, `BaseName`/`DecodedImage` in `archive/ops.rs`.
- **Behaviour notes:**
  - E2 consumes the parsed `EntryKind` in the directory reader; a Unix file whose *name* ends in a
    backslash is now classified as a directory, matching how the zip/tar/7z/rar backends already
    read `parse_entry_info`. No effect on comic images. The reader still skips sockets/fifos so
    `File::open` cannot block.
  - E4 keeps `Option<&str>` for the destination name and source stem; `is_matching_root` reduces to
    the two `contains` checks, and an absent value behaves exactly like the old `""`. The
    IO-degradation clause is now explicit: `same_file` only blocks a conversion on `Ok(true)` (a
    stat failure means "not the same file"), and a `list_entries` failure is funnelled through the
    documented `single_root_dir` helper, which skips root-stripping and lets `read_entries` surface
    the real error.
- **Scope grew slightly** past the three named call sites: `ebook/input/epub.rs` and
  `ebook/input/pdf.rs` (the other `build_tree` callers), `src/clamp.rs` (`get_images_from_source`,
  D9), `src/convert.rs` (`convert_archive_ext`, A3), and the two test files
  (`integration_tests`, `ebook_input_epub_pdf_tests`).
- **New tests:** four in `archive::path` (empty-path → `None`, file/dir tagging,
  `find_single_root_dir` kinds, `is_matching_root` optionality), one in `archive::reader`
  (directory/file `EntryContent` round-trip), and — added during verification — a `RootStripPolicy`
  round-trip in `archive::ops` plus a trailing-backslash classification case in the `archive::path`
  tagging test.
- **Gate:** `cargo fmt --check` clean · `cargo clippy --all-targets --all-features -- -D warnings`
  clean · `cargo nextest run` → **351 passed, 13 skipped** at the phase commit (was 346; +5 new).
  The verification follow-ups above are included in the tree-wide count reported under Phase 3.
- **Changelog:** `## [Unreleased] → Changed` entry added for the public-surface changes.
- **Follow-on:** `safe_join` stays bespoke here; the optional [§8.1](#81-back-the-path-newtypes-with-relative-path)
  side quest proposes backing the path newtypes with `relative-path` after Phase 6.

### Phase 2 — Typed CLI values

**Status:** ✅ **Complete** — landed on `rusty-refactor`.

- **Findings:** B1, B2, B6, A20.
- **Scope:** `src/cli.rs`, `src/convert.rs`, `ebook/cli.rs` and the parse boundary.
- **Deliverable:** `ValueEnum`s `Splitter`, `Cropping`, `InterPanelCrop`, `MetadataTitle`,
  `BatchSplit`, `ArchiveFormat`; `--borders <white|black>` replacing the two border bools.
  Numeric aliases (`#[value(alias = "0")]`, …) keep `--splitter 1` parsing.
- **Gate:** CLI parsing tests; existing output tests must not change.
- **Note:** this is the only phase that alters the *accepted input surface*; see
  [§6.1](#61-command-line-surface).

**Completed notes.**

  - **Delivered as planned:** `ValueEnum`s `Splitter` (`Split`/`Rotate`/`Both`), `Cropping`
    (`Off`/`Margins`/`PageNumbers`), `InterPanelCrop` (`Off`/`Horizontal`/`Both`), `MetadataTitle`
    (`Default`/`Combine`/`Only`), `BatchSplit` (`None`/`Auto`/`PerSubdirectory`) and `ArchiveFormat`;
    `--borders <white|black>` replacing the two border bools. Every consumer of the old `u8` modes
    (`input/pdf.rs`, `processing/mod.rs`, `processing/page.rs`, `metadata.rs`, `output/epub/mod.rs`,
    `chunk.rs`) now matches the enum exhaustively (B2).
  - **Placement.** The five ebook enums live in `ebook/options.rs` beside `Format`/`DocType`;
    `ArchiveFormat` lives in `archive/kind.rs` beside `ArchiveKind`, and `parse_target_extension` now
    delegates to `ArchiveFormat::from_str`, so there is a single source of the accepted spellings
    (B6). `BorderColor` gained the derive.
  - **Numeric aliases.** Each mode variant carries its old digit as `#[value(alias = …)]`, so
    `--splitter 1`, `--cropping 2`, `--inter-panel-crop 2`, `--metadata-title 2` and
    `--batch-split 2` keep parsing; `Cropping::PageNumbers` and `BatchSplit::PerSubdirectory` use
    `#[value(name = …)]` so the canonical names are `pages`/`per-subdir`. Defaults keep their `0`/`2`
    spellings, so `--help` shows the same `[default: …]` as before.
  - **A20 implementation.** `--borders` is the new flag; `--black-borders`/`--white-borders` are kept
    as separate hidden legacy flags (not `alias` directives) that conflict with each other and with
    `--borders`, so each still works alone but passing both (or mixing a legacy flag with `--borders`)
    is a `clap` error. This is the one accepted-input change; it is recorded in the changelog.
  - **Behaviour-preserving elsewhere.** `run_convert` takes the typed `ArchiveFormat` and derives its
    extension/kind, so the duplicated accepted-list and error string are gone; the test helper
    `tests/common::run_convert` parses its `&str` target through the same `ValueEnum`, keeping the
    `integration_tests` call sites (and the unsupported-target test) unchanged.
  - **Scope grew slightly** past the four named files: the six mode consumers above and
    `archive/mod.rs` (re-export). `src/cli.rs`/`convert.rs` switched `Convert.to` to `ArchiveFormat`,
    which forced the three `cli_tests` assertions on that field onto the enum (the only test edits
    beyond new cases).
  - **New tests:** `ebook_tests` `processing_mode_enums_accept_names_and_numeric_aliases` and
    `border_flags_map_to_colours_and_are_exclusive`; `cli_tests`
    `test_cli_convert_parses_each_target_format`. The existing
    `ebook_robustness_tests::out_of_range_processing_modes_are_rejected` still pins that `0`/`1`/`2`
    parse and `3`/`255`/`-1` do not.
  - **Gate:** `cargo fmt --check` clean · `cargo clippy --all-targets --all-features -- -D warnings`
    clean · `cargo nextest run` → **354 passed, 13 skipped** (was 351; +3 new).
  - **Changelog:** `## [Unreleased] → Changed` entry added for the CLI-surface change.
  - **Review follow-up.** The `docs/cli.md`/`docs/processing.md` reference tables were updated (during
    verification) to describe the named mode values and the consolidated `--borders` flag.

### Phase 3 — Resolved configuration sum types

**Status:** ✅ **Complete** — landed on `rusty-refactor`.

- **Findings:** A12, A13, A14, A18, A19, A21, B5, B15; consumers G2, G4, G8.
- **Scope:** `ebook/options.rs`, `ebook/profiles.rs`, `ebook/mod.rs`, and every `options.*`
  reader in `processing/*` and `output/*`.
- **Deliverable:** `ReaderFamily`, `Geometry`, `PanelView`, `OutputEncoding` (replacing
  `ResolvedFormat` once `write_tome` is exhaustive); `DeviceKind` methods; `TitleOrigin` for
  `assemble`. `Options` drops `is_kindle`/`is_kobo`/`custom_profile`/`kfx`/`kepub`/
  `keep_epub`/`kindle_azw3`/`kindle_scribe_azw3` and adopts the Phase 2 enums for
  `splitter`/`cropping`/`inter_panel_crop`/`metadata_title`/`batch_split`. Split the flat
  `Options` into `DeviceOptions`/`MainOptions`/`ProcessingOptions`/`OutputOptions`/
  `SessionOptions` ([§6.2](#62-resolved-options-target-shape)) and narrow stage signatures
  to the group they read.
- **Gate:** `output/*` and `ebook_*` tests unchanged.
- **Depends on:** Phase 2.

**Completed notes.**

- **Delivered as planned:** `ReaderFamily` (`Kindle`/`Kobo`, where `Kobo` is KCC's
  `isKobo == !isKindle`, covering reMarkable/`Other`), `Geometry` (`Profile`/`Custom`),
  `PanelView`, and `OutputEncoding` (`Epub { kfx }`/`Kepub { short_ext }`/`Mobi { keep_epub }`/
  `Azw3`/`Cbz`/`Pdf`); `TitleOrigin` for `assemble`. `Options` is now a thin aggregate of
  `DeviceOptions`/`MainOptions`/`ProcessingOptions`/`OutputOptions`/`SessionOptions` plus
  `inputs`; the dropped fields are all gone. `write_tome` matches `OutputEncoding` exhaustively,
  so its `bail!` is removed (B5/E8), and `resolve` derives a private `ResolvedFormat` while it
  expands the presets, so building the `OutputEncoding` is exhaustive with no `bail!` either (B5).
- **Refinements over the sketch:**
  - **`PanelView` is `{ Off, Hq, Two, Legacy }`, not `{ Off, Two, Vertical4, Legacy }`.** The
    reachable panel layouts need the plain `-q` mode, and `--vertical-4-panel` is an *independent*
    axis (it only selects the OPF writing mode and may co-occur with any mode), so it stays a
    separate `MainOptions::vertical_4_panel` bool rather than becoming an unreachable union. Both
    deviations are behaviour-preserving (pinned by the golden/panel tests).
  - **`OutputEncoding::Azw3` carries no `scribe` payload.** `kindle_scribe_azw3` is true for EPUB/
    MOBI output on a Scribe too, so it lives on `ProcessingOptions::scribe` (as §6.2 says); the
    AZW3 variant stays a unit variant to avoid a second, weaker source of truth.
  - Also introduced to match §6.2: `Layout` (`Regular`/`LightNovel`/`Wallpaper`), `Gamma`
    (`Auto`/`Linear`), `Autocontrast` (`Contrast`/`Off`/`Level`), and the orthogonal clusters
    `ColorTuning`/`PngOptions`/`Strips`/`Sizing`/`RotationOptions`/`CoverOptions`/`SourceOptions`.
  - `DeviceOptions` also drops the dead `device_kind` field (no reader existed).
  - `Geometry::Custom` replaces both `custom_profile` and the `"Custom"` `ProfileData.name`
    sentinel; `data.name` now keeps the profile label.
- **Derived facts as methods:** `MainOptions::right_to_left()` (`manga && !webtoon`),
  `Options::profile_size()` (`device.data` + `main.hq`), `Options::kindle_azw3()`
  (reader + `output.encoding`), and `Options::panel_view_enabled()` (Kindle reader + a selected
  panel mode), each replacing a duplicated field/predicate (G2, G8).
- **Signatures narrowed where clean:** `metadata::resolve_with`/`resolve` take `&OutputOptions`;
  `chunk::target_size` takes `&MainOptions`, `chunk::assemble` takes `&ProcessingOptions`, and
  `kindle::create_scratch` takes `&SessionOptions`. The cross-group per-page pipeline
  (`processing/*`) and the EPUB builders still take `&Options`; narrowing them needs a borrowed
  context and is left to a later phase.
- **B15:** `Profile::is_kobo_brand()` now delegates to `DeviceKind::Kobo`, and `is_scribe()`
  matches the seven Scribe variants. No `ProfileEntry` column and no `DeviceKind` negation helper
  (both rejected in review as non-idiomatic/boolean-blind).
- **B12 (`naming::NameStyle`)** was pulled in because removing `Options.format` forced
  `slugify`'s parameter: it now takes `NameStyle { Slug, Cbz }` instead of the whole request
  `Format`.
- **Behaviour-preserving remainder:** every consumer edit is a mechanical field-path change.
  `xhtml.rs::panel_layout`'s branch chain became a single `buildHTML` truth-table `match`.
  `--light-novel` shadows `--wallpaper` only on the path where it actually short-circuits the
  `page.rs` pipeline (the non-fusion path); under `--file-fusion` the pipeline runs, so wallpaper
  still selects its fit branch, matching the old two-bool behaviour (pinned by
  `ebook_tests::light_novel_and_wallpaper_resolve_to_a_layout`).
- **Scope grew** past the four named files to every `options.*` reader: `ebook/{chunk,metadata,naming}.rs`,
  `processing/{mod,page,color,cover,webtoon}.rs`, `input/{epub,pdf}.rs`,
  `output/{mod,kindle}.rs`, `output/epub/{mod,opf,xhtml}.rs`, `output/lightnovel.rs`, plus the
  mechanical `Options`-construction updates in `tests/ebook_tests.rs`/`tests/ebook_robustness_tests.rs`.
- **Gate:** `cargo fmt --check` clean · `cargo clippy --all-targets --all-features -- -D warnings`
  clean · `cargo nextest run` → **357 passed, 13 skipped** (Phase 2's 354 plus the review's three
  added tests: the resolution table below, the root-strip policy, and the layout precedence; the
  byte-exact golden tests still pass unchanged).
- **Changelog:** `## [Unreleased] → Changed` entry added for the resolved-configuration change.
- **Review follow-up.** Verification added the §7.6 resolution-table test
  (`ebook_tests::resolved_output_encoding_matches_every_format_and_preset`), finished A19/G2 with
  `Options::panel_view_enabled()`, closed B5 with the private `ResolvedFormat`, and pinned the
  fusion/wallpaper layout precedence. `ResolvedFormat` is a helper enum local to `Options::resolve`,
  not a field on `Options`; the cross-group stage signatures remain `&Options` for now, and the
  optional [§8.1](#81-back-the-path-newtypes-with-relative-path) side quest is unaffected.

### Phase 4 — Geometry and unit newtypes

**Status:** ✅ **Complete** — landed on `rusty-refactor`.

- **Findings:** D2, D3, D4, D5, D6, D16, D17.
- **Scope:** `model.rs`, `processing/{page,crop,kernels,fill,color,cover,webtoon}.rs`,
  `ebook/chunk.rs`, `src/{clamp,image_ops}.rs`, `output/{epub,lightnovel,pdf}`.
- **Deliverable:** `Size`, `IndexBox`, `BBox`, `Range`, `Percent`, `Fraction`, `Pixels`,
  `Bytes`, `Megabytes`, `Quality`, `Palette`/`PaletteIndices`.
- **Gate:** full suite plus a performance baseline (must be neutral). Kernel value semantics
  are untouched.
- **Why before Phase 5:** processing enums then read `Size` rather than writing `(u32, u32)`.

**Completed notes.**

- **Delivered as planned:** every listed type. The cross-cutting ones (`Size`, `Percent`,
  `Fraction`, `Pixels`, `Bytes`, `Megabytes`, `Quality`, `BBox<T>`, `IndexBox`, `Range`) live in a
  new crate-level [`src/units.rs`](../../src/units.rs); `Palette`/`PaletteIndices` are private to
  `processing/page.rs` (they only wrap the quantiser's two `Vec`s, and keeping them non-`Clone`
  local keeps `png::BitDepth` out of the shared module).
- **Refinements over the sketch:**
  - `BBox<T>` is generic over the coordinate type, because the same `(left, upper, right, lower)`
    shape is used with `u32` (Pillow bounding boxes), `usize` (the `kernels::clip` rectangle),
    `i64` (the padded-crop/edge rectangles) and `f64` (the rounded crop rectangle); a single
    concrete `BBox` would have needed three near-identical twins. `IndexBox` keeps its
    axis-grouped order and exposes `dx()`/`dy()` (the raw `x2 - x1` difference the page-number
    guards use, deliberately *not* `+ 1`).
  - `Size` is a two-field `Copy` struct (so it cannot be `#[repr(transparent)]`; the §1 rule is
    about single-field wrappers). It carries `new`/`from_dimensions`/`to_dimensions`.
  - Every single-field wrapper (`Percent`, `Fraction`, `Pixels`, `Bytes`, `Megabytes`, `Quality`) is
    `#[repr(transparent)]`, per §1 rule 1. The span accessors (`Range::spread`, `IndexBox::dx`/`dy`,
    and `BBox`'s `width`/`height`) saturate rather than subtracting unchecked, so a transposed box
    yields `0` instead of panicking a debug build (or wrapping a release one), per §1 rule 6; `f64`
    keeps the plain difference.
  - The raw CLI fields stay primitive (`target_size: Option<u32>`, `preserve_margin: u32`,
    `cropping_minimum: f32`, `jpeg_quality: Option<u8>`); the typed values are constructed in
    `Options::resolve`, matching the other args groups and keeping the clap surface — including
    the `--jpeg-quality 99` rejection — byte-identical. So §6.1's `Option<Quality>` is realised as
    `ProcessingOptions::jpeg_quality: Quality` rather than a fallible CLI parser.
  - `Options::device_size()` was added to centralise the `(data.width, data.height)` pair the
    cover/light-novel/OPF/XHTML builders all read; `profile_size()` now returns a `Size`.
- **Consumers updated (all mechanical):** `Page::dimensions`/`EncodedPage` use `Size`;
  `PageRef` carries `size`; the resize/fit/contain/thumbnail/pad helpers take `Size`; the
  crop/page-number path uses `BBox`/`IndexBox`/`Fraction`/`Percent`; the chunk keeper uses
  `Bytes`/`Megabytes`; `clamp`/`image_ops` use `Pixels`; and the encoders take `Quality`.
- **Gate:** `cargo fmt --check` clean · `cargo clippy --all-targets --all-features -- -D warnings`
  clean · `cargo nextest run` → **364 passed, 13 skipped** (Phase 3's 357 plus seven new `units`
  tests). The ignored geometry tests (`a_tall_scribe_page_splits_at_1920_into_above_and_below`,
  `a_super_long_panel_splits_with_overlap`, `wider_devices_use_the_1072_cap_for_the_virtual_height`,
  `two_panel_and_vertical_4_panel_reshape_the_panel_view`, the cover-crop tests) pass with
  `--run-ignored all`; the byte-exact goldens pass unchanged. The `alloc_count`/`bench.sh`
  performance baseline was measured against a `HEAD` (Phase 3) worktree on the 40-page,
  1600×2400 `target/bench/bench.cbz`: allocations `1 222 972 → 1 222 949`, bytes requested
  `11023.4 MiB` on both sides, peak live `401.7 → 399.3 MiB`, and `user` CPU time overlapping
  run-for-run (baseline 25.4–27.1 s, Phase 4 25.5–26.9 s) — neutral within noise, as the
  transparent newtypes predict.
- **Changelog:** `## [Unreleased] → Changed` entry added; `docs/architecture.md`'s data-model
  block updated for the `Size`/`units` change.
- **Review note.** Only `src/**` unit tests and the integration tests that name a retyped value
  were edited (mechanical `.to_dimensions()`/`BBox::new(..)`/`Fraction::new(..)` conversions); no
  golden or memory reference was regenerated.

### Phase 5 — Processing enums

**Status:** ✅ **Complete** — landed on `rusty-refactor`.

- **Findings:** A5, A6, A7, A8, A9, A10, A11, C4, B13, B14, and the page-part string literals
  (`"above"`/`"below"`/`"whole"`, `page.rs:347,356,369`).
- **Scope:** `processing/{page,color,rainbow,interpanel,kernels,fill,webtoon}.rs`,
  `input/pdf.rs`, and the `PageFlags` readers in `chunk.rs`/`output/epub/*`.
- **Deliverable:** `Detected`/`OutputColor`, `ThresholdKind`, `Axis`, `Band`/`BandKind`,
  `ScribeHalf`, `Orientation`, `StripVote`, `Panel`, `FitPreference`; drop the dead
  `color_check(original_is_grayscale)` parameter.
- **Gate:** processing + output tests. `chunk.rs` and `output/epub` switch from
  `flags.above`/`flags.below` to `flags.half`.
- **SIMD note:** enum/`const` parameterisation only; never `dyn`, never boxed predicates.

**Completed notes.**

- **Delivered as planned:** `Detected`/`OutputColor`, `ThresholdKind`, `Axis`, `Band`,
  `ScribeHalf`, `Orientation`, `StripVote`, `Panel`, `FitPreference`, and the dropped
  `color_check` parameter. The page-part literals became a private `PagePart` (`Above`/`Below`/
  `Whole`) enum with `as_str()`, so the Scribe branch no longer spells the suffix as a bare string.
  Because the output names are frozen bytes, `PagePart::as_str()` returns exactly the old literals
  and `named_page` still takes `Option<&str>`.
- **A7 is the one place the sketch did not map cleanly.** `black_background` was *not* a duplicate of
  `Page::background` (the detected colour): it was the **resolved fill** from `page_fill` (the
  `--borders` override wins over the detection). So `PageFlags` stores `background: Background` fed
  from `fill`, not from `Page::background` — dropping the flag and reading `Page::background` in
  `xhtml.rs` would have changed the XHTML body style under `--borders`. The field keeps the name
  `background` (the finding's wording) with a doc comment marking it as the resolved fill; the
  default `Background::White` reproduces the old `black_background == false` at every
  `PageFlags::default()` site (cover, webtoon, ingest, `--no-processing`).
- **A9 uses a `const`-generic core**, not a runtime enum match inside the loop:
  `threshold_in_place(data, threshold, ThresholdKind)` dispatches once to
  `threshold_mono::<INVERTED: bool>`, which monomorphises the polarity out of both the 16-lane loop
  and the scalar tail (`crop.rs` passes `Below`, `webtoon.rs` passes `Above`). No `dyn`, no closure.
- **A11 is a `Band` struct, not `BandKind`.** The two flags are independent (an empty band is
  neither white nor black), so a sum type would be wrong; `Band { has_white, has_black }` names the
  fields and removes the transposable tuple. The catalogue's `interpanel.rs:293` consumer was stale:
  the only caller is `webtoon::band_is_solid`.
- **A10's two booleans share one polarity** (`horizontal == true` and `remove_rows == true` both mean
  rows), so both became `Axis::Rows`/`Axis::Columns`; the trap was the misleading name `horizontal`,
  not an inverted value.
- **A5's `render_target`/`Cropping` half was already resolved by Phase 2**, so only `render_zoom`
  changed: it takes a `FitPreference` produced by the associated constructor
  `FitPreference::new(pdf_width, page_width, page_height)`, and keeps the `page_height > 0.0`
  fall-through inside the ratio selection.
- **B13 (`Panel`)** is `{ top, bottom }` (`Copy`, private) with a saturating `height()`; the stored
  tuple height was always `bottom - top`, so every construction is byte-identical and the two
  recomputations collapse into the accessor.
- **B14 (`StripVote`)** types both `strip_vote` and the summing `border_fill` (its sign is all
  `fill_check` reads); the `-1`/`0`/`+1` weights now live only in `StripVote::score`.
- **A8 (`Detected`/`OutputColor`).** `color_check` returns `Detected` and the single
  `OutputColor::from_detection(detected, force_color)` call at the payload boundary reproduces
  `color && force_color`; `prepare_image` keeps **both** parameters (detected-colour-but-gray-output
  is a real state). `gamma_correct`/`autocontrast_image`/`autolevel_image`/`black_point` take
  `Detected`, `encode_image`/`erase_rainbow_artifacts` take `OutputColor`. The dead parameter was
  purely a performance shortcut (an `L`/`1` source converted to RGB always tests gray anyway), so the
  guard stays at the two call sites that skip the RGB round-trip.
- **Test note.** The only test change beyond mechanical field renames is the removal of
  `color::tests::grayscale_sources_are_never_colour`, whose subject *was* the deleted parameter; its
  behavioural claim is now covered by the call-site guards and the new
  `color::tests::output_colour_keeps_colour_only_with_force_color` truth-table, plus
  `pdf::tests::fit_preference_only_widens_a_portrait_pdf`. `PageFlags` literals and the
  `flags.half`/`orientation`/`background` reads were updated across `chunk.rs`, `output/epub/*` and
  `tests/ebook_processing_tests.rs`.
- **Gate:** `cargo fmt --check` clean · `cargo clippy --all-targets --all-features -- -D warnings`
  clean · `cargo nextest run` → **365 passed, 13 skipped** (Phase 4's 364, +2 new, −1 removed). The
  Phase 5 ignored gate passes with `--run-ignored all`
  (`scribe_profile_splits_a_tall_page_into_above_and_below` plus the webtoon panel and PDF raster
  tests); the byte-exact goldens are unchanged.
- **Changelog:** `## [Unreleased] → Changed` entry added; `docs/architecture.md`'s data-model block
  updated for the `PageFlags`/`Orientation`/`ScribeHalf` change.
- **Follow-up hardening (compiler-enforced invariants).** Post-review, the consumption sites were
  tightened so the new enums do structural work instead of being read as bare values: `OutputColor`
  is now an opaque newtype (`is_color`/`is_gray`) constructible only by `from_detection`; the
  resolved fill is the `ResolvedFill` newtype so it cannot be swapped with the detected
  `Page::background`; and the `Detected`/`ScribeHalf`/`Orientation` dispatches in `page.rs`,
  `rainbow.rs`, `chunk.rs`, `output/epub/{mod,xhtml}.rs` and `processing/mod.rs` use exhaustive
  `match`es rather than `if … == variant` comparisons or a `bool` re-collapse.

### Phase 6 — Page state machine and name identity

- **Findings:** C1, C2, C3, D1, D14, D15, F1, F2, C12.
- **Scope:** `ebook/model.rs`, `ebook/input/{archive,epub,pdf,fusion}.rs`,
  `ebook/processing/{mod,page}.rs`, `ebook/mod.rs`.
- **Deliverable:** `PageData { Encoded(Source), EncodedDecoded(Source, DynamicImage),
  Pixels(MediaType, DynamicImage), Consumed }`; `SourceName`/`RelPath`/`ChapterName`;
  `LoadedPage` folds into the carrier; `ProcessedBook.cover: Option<Cover>`; delete
  `ComicTree.cover`/`CoverSource`; remove `Page: Clone`; collapse `Fused`.
- **Gate:** **the large-book memory regression tests are mandatory**
  (`tests/ebook_robustness_tests.rs`). Confirm `EncodedDecoded` still retains the encoded
  bytes and that `--no-processing` never decodes.
- **Why late:** largest blast radius; benefits from the naming and geometry types being
  settled first.

### Phase 7 — Output type states and stringly values

- **Findings:** B7, B8, B9, B10, B11, C7, C8, C9, C10, D11, D12, D13, A15, A16, A17, E6,
  E7, E8.
- **Scope:** `ebook/output/**`, including the askama view structs and their templates.
- **Deliverable:** `PageSide`, `WritingMode`, `PanelId`, `ManifestMediaType`, `ColorSpace`;
  bool `region_mag`; `OpfItem` property fields removed; `PageXhtml.below: Option<BelowImage>`;
  `Opf` option fields; move-only `ZipEntry`/`EpubEntries`; `PageRef` newtypes; `MobiFlags`;
  switch on `OutputEncoding` (no `bail!`).
- **Gate:** exact-byte output tests for single and split tomes across EPUB/KePub/MOBI/PDF/CBZ.
- **Depends on:** Phases 2, 3, 6 (the types being formatted).
- **Constraint:** wrappers stay move-only; never `Clone` the borrowed entry list.

### Phase 8 — Remaining guards, wildcards, and duplication

- **Findings:** E3, E9, E10, E11, E12, E13, E14, E15, E16, F3, F4, C6, C11, B16, A21
  (duplication), G1, G3, G5, G6, G7.
- **Scope:** `archive/ops.rs`, `ebook/{progress,metadata,naming,mod}.rs`,
  `processing/{cover,webtoon}.rs`, `src/{convert,clamp,image_ops}.rs`, `ebook/output/**`.
- **Deliverable:** `RootStrip` struct, `CoverPixels`, `Result`-based handling of the
  invariant errors (no `expect`/`unreachable!` — §1 rule 6), exhaustive `Profile::entry`,
  `Reporter` mode enum, `Metadata Field` enum, `detect_panels` `Option` state, shared
  `style()`/`file_name()`/`stem()` helpers.
- **Gate:** full suite plus clippy `-D warnings`.

### Phase 9 — Close-out

- **Deliverable:** update `docs/architecture.md`'s data-model block and any other doc/comment
  that described the old option pairs; finalise the `CHANGELOG.md` entries accumulated across
  phases.
- **Gate:** `cargo doc` clean; docs and code agree.

### Phase dependency and risk summary

| Phase | Depends on | Blast radius | Risk |
|:--|:--|:--|:--|
| 1 Archive ✅ | — | `archive/`, 3 call sites | low |
| 2 CLI | — | CLI + option fields | low (user-visible) |
| 3 Config ✅ | 2 | `options`/`profiles` + all readers | medium |
| 4 Geometry ✅ | — | processing + output boundaries | medium (broad, mechanical) |
| 5 Processing ✅ | 4 | processing hot paths | medium (perf) |
| 6 Page state | 4 | `model` + input/processing | **high (memory)** |
| 7 Output | 2, 3, 6 | `output/**` + templates | high (output bytes) |
| 8 Sweep | 1–7 | cross-cutting | low |
| 9 Close-out | 1–8 | docs | low |

---

## 6. Completed state: flags and options

This is the target the plan converges on. Flag *names* are kept stable for KCC
compatibility; only the *values* of the mode flags change (and they keep numeric aliases),
plus the one border-flag consolidation.

### 6.1 Command-line surface

| Flag (completed) | Today | Change |
|:--|:--|:--|
| `-p, --profile <PROFILE>` | `ValueEnum` | none |
| `-m, --manga` | `bool` | none (genuinely orthogonal) |
| `--light-novel`, `--wallpaper` | two `bool`s | unchanged on the CLI; resolve into `Layout` |
| `-q, --hq`, `-2, --two-panel`, `--vertical-4-panel`, `--legacy-panel-view` | four `bool`s | unchanged on the CLI; resolve into `PanelView` |
| `-w, --webtoon`, `--invert-direction`, `--file-fusion` | `bool`s | none |
| `--target-size <MB>` | `Option<u32>` | `Option<Megabytes>` |
| `-n, --no-processing` | `bool` | none |
| `-r, --splitter <MODE>` | `u8 0\|1\|2` | `<split\|rotate\|both>` (aliases `0`/`1`/`2`) |
| `-g, --gamma <VALUE>` | `f32`, `0` = auto | `<auto\|FLOAT>` (alias `0`/`0.0` = auto) |
| `--auto-level`, `--no-autocontrast` | two `bool`s | unchanged on the CLI; resolve into `Autocontrast` |
| `--color-autocontrast`, `--force-color` | two `bool`s | unchanged (orthogonal) |
| `-c, --cropping <MODE>` | `u8 0\|1\|2` | `<off\|margins\|pages>` (aliases) |
| `--cropping-power <FLOAT>` | `f32` | none |
| `--cropping-minimum <FLOAT>` | `f32` | `Fraction` |
| `--preserve-margin <PERCENT>` | `u32`, `0` = off | `Option<Percent>` (`0` still accepted as off) |
| `--inter-panel-crop <MODE>` | `u8 0\|1\|2` | `<off\|horizontal\|both>` (aliases) |
| `--borders <white\|black>` | `--black-borders` **and** `--white-borders` | one flag; old two kept as hidden aliases |
| `--force-png`, `--force-png-rgb`, `--png-legacy`, `--no-quantize` | four `bool`s | unchanged (orthogonal; see §6.4) |
| `--webp` | `bool` | none |
| `--jpeg-quality <0-95>` | `Option<u8>` | `Option<Quality>` |
| `--maximize-strips` | `bool` | none |
| `-u, --upscale`, `-s, --stretch` | two `bool`s | unchanged (orthogonal) |
| `--no-rotate`, `--rotate-right`, `--rotate-first` | three `bool`s | unchanged (orthogonal axes) |
| `--smart-cover-crop`, `--cover-fill` | two `bool`s | unchanged (orthogonal) |
| `--erase-rainbow`, `--legacy-extract`, `--pdf-width`, `-d, --delete`, `--temp-dir`, `--mozjpeg` | `bool`s | none |
| `-o, --output`, `-t, --title`, `-a, --author`, `--language <BCP47>` | paths/strings | `Language` newtype for `--language` |
| `--metadata-title <MODE>` | `u8 0\|1\|2` | `<default\|combine\|only>` (aliases) |
| `--keep-comicinfo` | `bool` | none |
| `-f, --format <FORMAT>` | `ValueEnum` (request) | unchanged; resolves into `OutputEncoding` |
| `--no-kepub`, `--kepub-short-ext` | two `bool`s | unchanged on the CLI; fold into `OutputEncoding::Kepub` |
| `-b, --batch-split <MODE>` | `u8 0\|1\|2` | `<none\|auto\|per-subdir>` (aliases) |
| `--spread-shift`, `--one-page-landscape` | `bool`s | none |
| `--doc-type <ebok\|pdoc\|none>` | `ValueEnum` | none |
| `--to <FORMAT>` (convert) | `String` + re-parse | `ValueEnum ArchiveFormat` |

Because the retyped flags carry numeric aliases, existing invocations such as
`--splitter 1 --cropping 2` keep parsing. The only input that stops being accepted is
passing **both** `--black-borders` and `--white-borders`; if that tolerance must be kept,
retain the two flags and route them through an `ArgGroup` into `--borders` instead of
deleting them.

### 6.2 Resolved options (target shape)

A single ~40-field `Options` struct is itself a smell: it is a god object where any
function can reach any field, and "which subset does this stage actually read?" is
undocumented. Split it into cohesive groups that mirror the CLI's existing `*Args` groups
(`DeviceArgs`/`MainArgs`/`ProcessingArgs`/`OutputArgs`/`CustomProfileArgs`), so the request
shape and the resolved shape correspond one-to-one.

`Options` remains a thin aggregate so call sites can migrate incrementally, but functions
should take the smallest group they need — e.g. the per-page pipeline takes
`&ProcessingOptions` (plus a `Size` from `DeviceOptions`), and the EPUB/PDF/Kindle builders
take `&OutputOptions` — instead of `&Options`. That narrows what each stage can depend on
and stops a device change from silently affecting the image pipeline.

Illustrative; comments map each field to what it replaces.

```rust
/// A fully resolved `comic-book ebook` run: one small group per concern.
#[derive(Debug, Clone)]
pub struct Options {
    pub inputs: Vec<PathBuf>,
    pub device: DeviceOptions,
    pub main: MainOptions,
    pub processing: ProcessingOptions,
    pub output: OutputOptions,
    pub session: SessionOptions,
}

/// Device profile and screen geometry (`DeviceArgs` + `CustomProfileArgs`).
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    pub profile: Profile,
    pub data: ProfileData,           // `Profile::data()` + the custom override
    pub reader: ReaderFamily,        // was device_kind + is_kindle + is_kobo
    pub geometry: Geometry,          // was custom_profile + the "Custom" name sentinel
}

/// Reading direction and layout switches (`MainArgs`).
#[derive(Debug, Clone)]
pub struct MainOptions {
    pub manga: bool,
    pub hq: bool,
    pub layout: Layout,              // was light_novel + wallpaper
    pub panel_view: PanelView,       // was panel_view + two_panel + legacy_panel_view
    pub vertical_4_panel: bool,      // independent axis: only drives the OPF writing mode
    pub webtoon: bool,
    pub invert_direction: bool,
    pub file_fusion: bool,
    pub target_size: Option<Megabytes>,
}

impl MainOptions {
    /// Derived once (KCC's `rightToLeft`); cleared by webtoon mode.
    pub fn right_to_left(&self) -> bool { self.manga && !self.webtoon }
}

/// Image-processing switches (`ProcessingArgs`).
#[derive(Debug, Clone, Default)]
pub struct ProcessingOptions {
    pub no_processing: bool,
    pub splitter: Splitter,
    pub gamma: Gamma,
    pub autocontrast: Autocontrast,  // was auto_level + no_auto_contrast
    pub color: ColorTuning,          // was force_color + color_auto_contrast
    pub cropping: Cropping,
    pub cropping_power: f32,
    pub cropping_minimum: Fraction,
    pub preserve_margin: Option<Percent>,
    pub inter_panel_crop: InterPanelCrop,
    pub borders: Option<BorderColor>,
    pub png: PngOptions,             // was force_png + force_png_rgb + png_legacy + no_quantize
    pub webp_output: bool,           // derived; was `webp` + `webp_output` duplicated
    pub jpeg_quality: Quality,
    pub strips: Strips,              // was maximize_strips
    pub sizing: Sizing,              // was upscale + stretch
    pub rotation: RotationOptions,   // was no_rotate + rotate_right + rotate_first
    pub cover: CoverOptions,         // was smart_cover_crop + cover_fill
    pub erase_rainbow: bool,
    pub source: SourceOptions,       // was legacy_extract + pdf_width
    pub scribe: bool,                // was kindle_scribe_azw3 (drives the tall-page split)
}

// Orthogonal clusters: independent bools that are clearer as a named struct than as
// loose fields on the processing group. All `Copy`, all `Default`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ColorTuning     { pub force_color: bool, pub autocontrast_color: bool }
#[derive(Debug, Clone, Copy, Default)]
pub struct PngOptions      { pub force: bool, pub force_rgb: bool, pub legacy: bool, pub no_quantize: bool }
#[derive(Debug, Clone, Copy, Default)]
pub struct Strips          { pub maximize: bool }
#[derive(Debug, Clone, Copy, Default)]
pub struct Sizing          { pub upscale: bool, pub stretch: bool }
#[derive(Debug, Clone, Copy, Default)]
pub struct RotationOptions { pub no_rotate: bool, pub right: bool, pub first: bool }
#[derive(Debug, Clone, Copy, Default)]
pub struct CoverOptions    { pub smart_crop: bool, pub fill: bool }
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceOptions   { pub legacy_extract: bool, pub pdf_width: bool }

/// Output selection and metadata (`OutputArgs`).
#[derive(Debug, Clone)]
pub struct OutputOptions {
    pub destination: Option<PathBuf>, // was `output`
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Language,
    pub metadata_title: MetadataTitle,
    pub keep_comicinfo: bool,
    pub doc_type: DocType,
    pub encoding: OutputEncoding,    // was format + kfx + kepub + keep_epub + kindle_azw3 + kindle_scribe_azw3
    pub batch_split: BatchSplit,
    pub spread_shift: bool,
    pub one_page_landscape: bool,
}

/// Cross-cutting side effects (`-d`, `--temp-dir`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionOptions { pub delete: bool, pub temp_dir: bool }
```

Derived, cross-group facts are computed once in `resolve` and exposed as methods rather
than duplicated: `MainOptions::right_to_left()`, and `Options::profile_size()`
(`DeviceOptions.data` + `MainOptions.hq`). `kindle_azw3`/`kindle_scribe_azw3` collapse into
`ProcessingOptions.scribe` (derived from `device.reader` + `output.encoding`) because that
is the consumer that needs them.

### 6.3 Enums introduced, grouped by owner

```rust
// DeviceOptions
enum ReaderFamily   { Kindle, Kobo }                 // Kobo == KCC's isKobo == !isKindle
enum Geometry       { Profile(Profile), Custom { width: u32, height: u32 } }

// MainOptions
enum Layout         { Regular, LightNovel, Wallpaper }
enum PanelView      { Off, Hq, Two, Legacy }   // Hq is KCC's `-q`; `--vertical-4-panel` stays a separate flag

// ProcessingOptions
enum Splitter       { Split, Rotate, Both }
enum Gamma          { Auto, Linear(f32) }
enum Autocontrast   { Contrast, Off, Level }
enum Cropping       { Off, Margins, PageNumbers }
enum InterPanelCrop { Off, Horizontal, Both }

// OutputOptions
enum MetadataTitle  { Default, Combine, Only }
enum BatchSplit     { None, Auto, PerSubdirectory }
enum OutputEncoding {
    Epub { kfx: bool },
    Kepub { short_ext: bool },
    Mobi { keep_epub: bool },
    Azw3,                            // unit: the Scribe split lives on ProcessingOptions::scribe
    Cbz,
    Pdf,
}
```

Plus the process-level enums from Phases 1, 4 and 5: `EntryKind`, `EntryContent`,
`RootStripPolicy`, `Size`, `IndexBox`, `BBox`, `Range`, `Detected`, `OutputColor`,
`ThresholdKind`, `Axis`, `Band`, `ScribeHalf`, `Orientation`, `StripVote`, `Panel`,
`FitPreference`, and the Phase 7 output enums `PageSide`, `WritingMode`, `PanelId`,
`ManifestMediaType`, `ColorSpace`, `ResolvedFormat`/`OutputEncoding` consumers.

### 6.4 Booleans that stay booleans

Not every `bool` is a smell. These encode genuinely independent binary facts and stay
`bool`, grouped into the small `Copy` + `Default` structs above rather than left loose:

- **`MainOptions`:** `manga`, `hq`, `webtoon`, `invert_direction`, `file_fusion`.
- **`ProcessingOptions`:** `no_processing`, `erase_rainbow`, `webp_output`;
  `color`/`png`/`strips`/`sizing`/`rotation`/`cover`/`source` own their clusters.
- **`OutputOptions`:** `keep_comicinfo`, `spread_shift`, `one_page_landscape`.
- **`SessionOptions`:** `delete`, `temp_dir`.

They become an enum only where a combination is genuinely impossible — the clusters listed
in §6.3. A `bool` whose `true`/`false` are both meaningful and independent stays one.

---

## 7. Testing, validation and process

A refactor changes types, not behaviour, so the existing tests are the specification: each
phase must leave them green *unchanged*. The only test files a refactor may touch are new
tests it adds; committed golden references and memory ceilings are never regenerated to
make a refactor pass.

### 7.1 Runner and quality gates

`cargo-nextest` is the standard runner — each test runs in its own process, failures are
clearer and parallelisation is better. There are no doctests, so nextest covers the whole
suite; use `cargo test` only where nextest cannot run a target. Run after **every** phase,
before starting the next:

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo nextest run
```

### 7.2 Test suite map

The integration suites under `tests/` each own a slice of behaviour. A phase should know
which of them is its gate (see §7.6).

| Suite | Covers | Most relevant phases |
|:--|:--|:--|
| `cli_tests` | `convert`/`clamp` argument handling, completions | 2 |
| `integration_tests` | end-to-end convert/clamp over fixtures | 1, 8 |
| `ebook_tests` | CLI, `Options::resolve`, profile tables | 2, 3 |
| `ebook_input_tests` | archive/folder ingest, root stripping, lazy decode | 1, 6 |
| `ebook_input_epub_pdf_tests` | EPUB spine and PDF raster/extract inputs | 1, 6 |
| `ebook_processing_tests` | per-page transform/encode pipeline, pixel release | 4, 5, 6 |
| `ebook_crop_tests` | margin/page-number/inter-panel crop boxes | 4, 5 |
| `ebook_naming_tests` | slugify, sanitize, cover selection, output filenames | 3, 6, 8 |
| `ebook_epub_tests` | EPUB/KePub documents, spread algorithm, Scribe panel view | 3, 5, 7 |
| `ebook_golden_tests` | byte-exact OPF/NCX/NAV/XHTML against committed goldens | 7 |
| `ebook_output_tests` | CBZ/PDF/light-novel builders, tome chunking | 3, 4, 7, 8 |
| `ebook_kindle_tests` | AZW3/MOBI structural readback via `kindling` | 3, 7 |
| `ebook_chunk_tests` | target-size/batch-split tome packing | 3, 4, 8 |
| `ebook_webtoon_tests` | webtoon merge, panel detection, strip splitting | 4, 5, 8 |
| `ebook_robustness_tests` | malformed inputs (error, never panic) + memory ceilings | 1, 6 |
| `src/**` unit tests | algorithm-level tests co-located with the module | all |

`src/**` unit tests are the ones a phase most often extends: the algorithm modules under
`ebook/processing/*`, `ebook/metadata.rs`, `ebook/naming.rs`, `ebook/chunk.rs` and
`ebook/output/epub/*` all carry `#[cfg(test)] mod tests`. Because `clippy --all-targets`
compiles them, new tests must satisfy the same lint contract as library code (§7.5).

### 7.3 Test taxonomy

- **Unit tests** on small synthetic images: colour/fill/split decisions, crop boxes,
  slugify, spread properties, filename logic, OPF/NCX/NAV rendering.
- **Fixture/golden tests** (`tests/ebook_golden_tests.rs`): generated EPUB documents are
  compared byte-for-byte against committed references under `tests/fixtures/epub_golden/`;
  the UUID and `dcterms:modified` are normalised and line endings are folded to LF (the
  generated documents are themselves pinned to LF).
- **Reference-value tests**: `tests/fixtures/crop/` and the webtoon virtual-page sizes pin
  values produced by KCC itself, so they assert exact equality, not a tolerance.
- **Round-trip tests**: CBZ→CBZ and EPUB→input→EPUB.
- **Structural output tests**: EPUB read back via `zip` (mimetype first + stored; OPF
  spine; XHTML image refs); AZW3/MOBI read back via `kindling`'s `mobi_dump`; PDF via
  `lopdf`. A byte diff against `kindlegen` is neither possible nor permitted.
- **Robustness tests** (`tests/ebook_robustness_tests.rs`): malformed inputs are driven
  through `catch_unwind`; the contract is **error, never panic**.
- **Conformance**: an `epubcheck` job (JVM) marked `#[ignore]` by default.
- **Progress quieting**: `tests/common/mod.rs` points fd 2 at `/dev/null` once per test
  binary (Unix); robustness tests also set the `COMIC_BOOK_QUIET` env var
  (`progress::QUIET_ENV`). New tests that drive the pipeline should use the same helpers.

### 7.4 Running the tests

```bash
# Default: the fast set; #[ignore]d slow/memory/conformance tests are skipped.
cargo nextest run

# One suite, including its ignored tests.
cargo nextest run --run-ignored all --test ebook_webtoon_tests

# Only the ignored tests (slow set + stress + epubcheck; epubcheck needs it on PATH).
cargo nextest run --run-ignored ignored-only
```

Long-running and memory tests are `#[ignore]`d because the unoptimised CI build is slow;
they are listed with timings in `docs/development.md`. They must be run explicitly for the
phases that touch their area (§7.6).

### 7.5 The lint contract refactor code must satisfy

`Cargo.toml` sets:

```toml
[lints.clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
```

and the tree contains no `.unwrap()`, `.expect()`, `panic!`, `unreachable!` or
`debug_assert!` anywhere (library or tests). This directly constrains every "impossible
state" fix in the catalogue:

- Guard a state by making it **unrepresentable** (enum/newtype/type state), not by
  `expect`-ing it away.
- Where a fallback is genuinely needed, return `Result`/`Option` and let the caller decide,
  matching the existing `anyhow::Context` style. `unwrap_or`, `unwrap_or_else` and
  `unwrap_or_default` are allowed (they are not the denied lints).
- Error paths must **not panic**: `ebook_robustness_tests` asserts that malformed input
  errors rather than aborts, so a new `expect` would both fail clippy and risk that
  contract.
- If a test-only panic is ever unavoidable, it would need a scoped
  `#[allow(clippy::expect_used)]` with a justifying comment — there are none today, so the
  default is to avoid it.

### 7.6 Per-phase test gates

Run the default suite plus the named ignored tests for the phase, then the full suite
before merging.

| Phase | Default suites to watch | Ignored tests to run explicitly | New tests to add |
|:--|:--|:--|:--|
| 1 Archive ✅ | `integration_tests`, `ebook_input_tests`, `ebook_robustness_tests` | — | directory/file round-trip, empty-path → `None`, root-strip policy |
| 2 CLI | `cli_tests`, `ebook_tests` | — | numeric aliases still parse; named values parse; border conflict is a clap error |
| 3 Config | `ebook_tests`, `ebook_epub_tests`, `ebook_kindle_tests` | — | resolution table: each preset/format → expected `OutputEncoding` |
| 4 Geometry | `ebook_processing_tests`, `ebook_crop_tests`, `ebook_chunk_tests` | (tooling: `alloc_count`/`bench.sh` baseline, §7.7) | coordinate round-trips through the new named types |
| 5 Processing ✅ | `ebook_processing_tests`, `ebook_crop_tests`, `ebook_epub_tests` | `scribe_profile_splits_a_tall_page_into_above_and_below` | `ScribeHalf`/`Axis` behaviour equivalence |
| 6 Page state | `ebook_input_tests`, `ebook_input_epub_pdf_tests`, `ebook_processing_tests`, `ebook_robustness_tests` | `ingest_and_repack_stay_far_below_the_decoded_book_size`, `a_large_book_converts_under_a_memory_ceiling`, `huge_book_stress` | `--no-processing` still never decodes; decoded page still retains its encoded bytes |
| 7 Output | `ebook_epub_tests`, `ebook_golden_tests`, `ebook_output_tests`, `ebook_kindle_tests` | `light_novel_...`, `smart_cover_crop_...`, `two_panel_and_vertical_4_panel_...` | none needed — the byte-exact goldens are the gate |
| 8 Sweep | full suite + clippy | as touched | metadata field identity; exhaustive `Profile::entry`; `file_name` helper |
| 9 Close-out | full suite | — | none |

Existing always-on tests already pin two contracts the refactor must not break:
`ingest_defers_page_decoding` (`ebook_input_tests`) and `processing_releases_decoded_pixels`
(`ebook_processing_tests`).

### 7.7 Regression guardrails

- **Golden references are frozen.** Never run `UPDATE_GOLDEN=1` to make a refactor pass —
  it exists only for an intentional format change. A diff there means the refactor changed
  output, which violates §1 rule 4. (Regenerate, if a real format change is ever intended,
  with `UPDATE_GOLDEN=1 cargo nextest run --test ebook_golden_tests`.)
- **Memory ceilings are frozen.** The `ebook_robustness_tests` ceilings assert peak RSS is
  linear in the *encoded* book (see `docs/development.md`); treat any increase as a failed
  refactor. Phase 6 is the phase this most concerns.
- **Performance is measured, not assumed.** `examples/gen_bench` generates a deterministic
  input; `scripts/bench.sh` times a run; `examples/alloc_count` (a counting global
  allocator) and `scripts/memory_bench.sh` report allocations/bytes/peak live against a
  baseline worktree. Run these before and after Phases 4 and 5, where enum/newtype/
  `const`-generic changes must be neutral. Do **not** judge hot paths by eye.
- **The `#[ignore]`d tests are the guardrails, not optional.** Run the relevant ones for
  the phase (table in §7.6); default CI skips them for time, not because they are
  unimportant.

### 7.8 Process notes

- **Per-step commands** (from `AGENTS.md`): `cargo fmt`, then
  `cargo clippy --all-targets --all-features -- -D warnings`, then `cargo nextest run` — after
  every phase, before starting the next.
- **One finding class is behaviour-visible.** A20 (`--black-borders --white-borders`
  becomes a clap conflict) rejects input that previously ran. That is the only place a
  fix changes accepted input; if the maintainer wants the old tolerance, keep the flags
  and resolve through an `ArgGroup`/enum instead of deleting them. Add a `cli_tests` case
  either way.
- **Changelog.** Each user-visible change (CLI surface, output, dependencies, docs) gets an
  `## [Unreleased]` entry; pure internal type refactors that change neither surface nor
  output do not need one, per Keep a Changelog.
- **Docs close-out (Phase 9).** Update `docs/architecture.md`'s data-model block and any
  comment that described the old option pairs, then confirm `cargo doc` is clean.

---

## 8. Side quests

These are optional and out-of-band with respect to the ordered phases: they are not required for
the type-safety goal, they add a dependency, and they should only be scheduled when the code they
delete is worth more than the crate they add (the off-the-shelf policy in
[docs/dependencies.md](docs/dependencies.md) still holds: prefer the crate when it genuinely
fits). Each is self-contained — land it on top of a green phase, keep the full suite green, and
record a `[Unreleased]` entry.

### 8.1 Back the path newtypes with `relative-path`

**Motivation.** Phase 1 introduced [`NormalizedArchivePath`](#phase-1--archive-tag-union-and-path-identity)
and, with it, a small pile of hand-rolled *cross-platform interop* helpers that re-implement what a
maintained relative-path library already does — on exactly the semantics we care about (always
`/`-separated, platform-independent, no root/prefix). The later phases add more of the same
(D1 `SourceName`/`RelPath`/`PageName`, D10 `naming` slugs and `cover_path`, D12 `PageRef`, D13 the
output ids, D14 `ChapterName`), so the bespoke surface only grows.

**Proposal.** Adopt [`relative-path`](https://docs.rs/relative-path)
(`RelativePath` borrowed, `RelativePathBuf` owned; MIT OR Apache-2.0, pure Rust) and make it the
*backing store* of the path newtypes instead of exposing it:

```rust
#[repr(transparent)]
pub struct NormalizedArchivePath(RelativePathBuf);   // (D7)

impl NormalizedArchivePath {
    pub fn as_str(&self) -> &str { self.0.as_str() }
    pub fn as_relative(&self) -> &RelativePath { self.0.as_relative_path() }
}
```

The outer newtype **stays**: the invariant it encodes — sanitized, non-empty, zip-slip-safe — is
not something `RelativePath` guarantees. `RelativePath::new("../..")` is a legal value, and
`is_normalized()` even counts `"../.."` as normalized. So `relative-path` is a backing type and a
helper source, never the public path type.

**Interop helpers retired.** The helpers below are pure cross-platform string plumbing; each is
subsumed by a `relative-path` method.

| Finding | Hand-rolled helper | Replace with |
|:--|:--|:--|
| E5 | the repeated `.rsplit(['/', '\\']).next().unwrap_or(name)` (`archive/path.rs`, `archive/ops.rs`, `ebook/input/archive.rs`, `ebook/input/epub.rs`, `ebook/naming.rs`, `ebook/chunk.rs`, `output/epub/{mod,xhtml,opf}.rs`) | `RelativePath::file_name()` |
| G6 | `image_extension` (`ebook/input/archive.rs`), the `stem` helper (`ebook/mod.rs`), `split_dir_file` (`ebook/input/archive.rs`), the webtoon/page stem helpers | `extension()`, `file_stem()`, `parent()` / `file_name()` |
| D14 | `Chapter.name`'s `""` root sentinel plus the `split_dir_file`/`compare_dir_paths` walk | `parent()` / `components()` |
| D1 | `LoadedPage.name` / `Page.source_name` / `Page.rel_path` string surgery | `RelativePathBuf` fields behind `SourceName`/`RelPath`/`PageName` |
| — | the `['/', '\\']` separator ladders in `is_os_metadata`, `find_single_root_dir`, `parse_entry_info` and the EPUB container path resolution | `components()` / `iter()` |
| — | `NormalizedArchivePath::strip_prefix` (the one place that still allocates on the root-strip path) | `RelativePath::strip_prefix` (borrowed) — see the caveat below |

**What must stay bespoke.** “Retire every interop helper” is achievable for the read-side
accessors, but two things cannot be delegated and must keep their own code:

- **The sanitizer rules.** Traversal (`.`/`..`), Windows drive prefixes, per-component whitespace
trimming and empty → `None` are KCC-derived and security-relevant.
`RelativePath::normalize()` *keeps* leading `..` and does not drop drive letters or trim, so it
cannot stand in for `normalize_archive_path`; the sanitizer may only *produce* a `RelativePathBuf`.
- **`safe_join`'s semantics.** It *drops* `..` on the floor rather than *popping* the previous
component, so it is deliberately not `RelativePath::to_logical_path` (compare
`integration_tests::test_safe_join`, where `../../../system32/cmd.exe` becomes
`<base>/system32/cmd.exe`). Keep it bespoke, or re-express it as “sanitize, then
`to_logical_path`” — but pin the existing test either way, because output is frozen (§1 rule 4).
- `image_ops::is_image_file` (E16) operates on a `std::path::Path`, not an archive name, so
`relative-path` does not apply there.

**Open design question — the borrowed view.** `RelativePath::strip_prefix` returns a borrowed
`&RelativePath`, but that cannot be turned back into `&NormalizedArchivePath` without `unsafe`
(the remainder is sanitized only by construction, which the type system cannot see). Two honest
options: (a) keep the current one-allocation `strip_prefix` on the root-strip path — short names,
never a pixel buffer; or (b) relax `add_entry_normalized` to take `&RelativePath`, trading the
“already sanitized” guarantee for a zero-allocation strip. Option (a) is the recommendation;
(b) weakens D7.

**Alternatives considered.** `camino` and `typed-path` give UTF-8 / typed paths but a
host-sensitive or otherwise different separator model and no `/`-canonical guarantee;
`path-clean` and `normpath` *keep* leading `..`, which is precisely the zip-slip case they would
need to remove; `path-slash` only converts separators (a `.replace`, not worth a dependency);
`sanitize-filename` works on a single component, not a path. `relative-path` is the only crate
whose model matches “a relative, `/`-separated path” — the shape every name in this pipeline has.

**Cost / risk.**

- ➕ One well-tested, pure-Rust dependency (no default features needed) replaces a growing pile of
bespoke string surgery; aligned with the off-the-shelf policy.
- ➕ Zero runtime cost: `RelativePathBuf` is a `String` wrapper and `&RelativePath` is a fat
pointer, the same shapes as today's `String`/`&str`. A `#[repr(transparent)]` newtype over it stays
compiler-erased.
- ⚠️ **Output is frozen (§1 rule 4).** `relative-path`'s component model and its
`to_path`/`to_logical_path` must be pinned to our current Windows behaviour before they are allowed
to reach `safe_join` or the archive writers; the golden tests and `test_safe_join` are the gate.
- ⚠️ New dependency → a row in [docs/dependencies.md](docs/dependencies.md) and an `[Unreleased]`
entry.

**Sequencing.** Best done *after Phase 6* (page state machine and name identity), where
D1/D12/D14/D15 and the `SourceName`/`RelPath`/`PageName`/`ChapterName` newtypes land: introduce
the crate once, delete the helpers once. It also subsumes the Phase 8 items E5 and G6, so it can
be scheduled as an optional “Phase 8.5” rather than a re-touch of every phase.

**Gate.** Full suite green — in particular `integration_tests::test_normalize_archive_path`,
`test_safe_join`, `test_cross_platform_nested_directory_extraction`, `ebook_input_tests` and
`ebook_robustness_tests` — plus a new cross-check that drives hostile names (`../`, `..\`, a
leading `/`, `C:\`, an empty name) through the sanitizer and asserts it neutralizes exactly what
`zip`'s `enclosed_name()` rejects (the `zip` crate is already a dependency, so this needs no new
code).

---

## Appendix — findings index

Counts by category: **A** boolean blindness 21 · **B** stringly/magic 16 · **C** type
states 12 · **D** newtypes 18 · **E** guards/wildcards 16 · **G** duplication 8 · **F**
memory footguns 4.

The catalogue is executed in the phase order of [§5](#5-ordered-refactor-plan):
archive tag union (1 ✅) → typed CLI values (2 ✅) → config sum types (3 ✅) → geometry newtypes
(4 ✅) → processing enums (5 ✅) → page state machine (6) → output types (7) → guard/dedup
sweep (8) → docs close-out (9). The optional
[crate-backed path layer](#81-back-the-path-newtypes-with-relative-path) is a side quest after (6).
