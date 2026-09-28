# Dependencies

## Policy: prefer off-the-shelf solutions

This repo has to stay maintainable: code we write, we own — we debug it, test it, and carry it
forever. A maintained crate is somebody else's problem, is battle-tested and can be upgraded.
**Default to an existing library; only write bespoke code when a library genuinely cannot be
used.**

Before hand-rolling anything, check in order:

1. Does an existing dependency already do it? Reuse it instead of adding a crate or writing code.
2. Does a maintained, well-licensed crate do it? Prefer it even if the API is not a perfect fit
   — a thin adapter is cheaper to own than the algorithm.
3. Only then write bespoke code, and say why in the commit: no suitable crate exists, the crate
   is unmaintained or incompatible with the licence/pure-Rust rules below, its output cannot be
   pinned to KCC's format (see [architecture.md](architecture.md)), or the need is genuinely
   trivial (a few lines).

Anything deliberately kept hand-rolled carries a comment pointing here so it is not "fixed"
later. The [porting.md](porting.md) refactor pass records what was swapped and what was kept.

## Pure-Rust / self-contained rule

No external programs may be invoked or required. Prefer pure-Rust crates; crates that
vendor/compile their C sources (e.g. `unrar`, `webp`) are acceptable because they need no
external binary. Avoid crates that pull platform binaries (e.g. `pdfium-render`/`mupdf`).

## Crates in use

**Reused foundational crates:** `clap`, `clap_complete`, `image`, `zip`, `tar`,
`sevenz-rust2`, `unrar`, `rars`, `rayon`, `indicatif`, `natord`, `anyhow`, `tempfile`,
`fast_image_resize`, `webp`, `walkdir`, `same-file`, `infer`.

**Added by the port:**

| Crate | Purpose | Licence |
|:---|:---|:---|
| `kindling-mobi` (lib `kindling`) | EPUB/OPF → AZW3/MOBI (replaces `kindlegen` + `dualmetafix`) | MIT |
| `quick-xml` | ComicInfo/EPUB/OPF parse | MIT |
| `askama` | compiled-in EPUB document skeletons (`templates/`) | MIT OR Apache-2.0 |
| `uuid` (v4) | EPUB `dc:identifier`/`dtb:uid` | MIT OR Apache-2.0 |
| `time` | `dcterms:modified` timestamp | MIT OR Apache-2.0 |
| `imageproc` | histogram statistics, the webtoon 3×3 edge convolution, in-place subpixel LUTs and rectangle fills | MIT |
| `quantette` | fixed-palette quantisation + Floyd–Steinberg dithering | MIT OR Apache-2.0 |
| `wide` | portable SIMD lanes for the per-pixel kernels `imageproc` leaves scalar (`processing/kernels.rs`) | MIT OR Apache-2.0 |
| `png` | indexed/palette PNG output (`image` cannot write indexed PNG) | MIT OR Apache-2.0 |
| `rustfft` | moiré FFT | MIT OR Apache-2.0 |
| `slug` (+ `deunicode`) | chapter slugification | MIT |
| `regex` | KCC number-padding and `\W+` filename rules | MIT OR Apache-2.0 |
| `bitvec` | sub-byte scanline packing (`page.rs::pack_indices`) | MIT |
| `font8x8` | `N/M` tome cover label (bitmap font in source) | MIT |
| `pdf-writer` | PDF document skeleton | MIT OR Apache-2.0 |
| `flate2` | zlib for non-JPEG PDF samples / EPUB | MIT OR Apache-2.0 |
| `pdfboss-render`, `pdfboss-core` | PDF rasterise + embedded-image extraction | MIT OR Apache-2.0 |
| `relative-path` | internal archive/page name model (`RelativePath`/`RelativePathBuf`; backs the path newtypes, REFACTOR.md §8.1) | MIT OR Apache-2.0 |

`relative-path` supplies the in-archive name model: a relative, always-`/`-separated,
platform-independent path, which is the shape of every archive entry and page name the pipeline
carries. It backs the path newtypes (`NormalizedArchivePath`/`SourceName`/`RelPath`/`PageName`/
`ChapterName`) and its `file_name`/`parent`/`file_stem`/`extension`/`components`/`strip_prefix`
operations replace hand-rolled separator ladders; `std::path::Path` remains the host-filesystem
type. Only its `alloc`/`std` features are enabled (explicitly — this crate's `std` does not imply
`alloc`); see [porting.md](porting.md).

**Dev-dependency:** `lopdf` (PDF readback in tests; never ships in the binary). `zip` is also
declared as a dev-dependency so tests can read the EPUB container back.

**Deliberately avoided:** `boko` (GPL-3.0-or-later — incompatible with this MIT project) and
`mobi-sys` (FFI to a C library we do not need).

`imageproc`'s default features are disabled so they do not turn on `image`'s default codecs;
`quantette`'s `kmeans` default is disabled because only its `CustomPalette` path is used.
`askama`'s `config` feature is what enables `path = "…"` templates; those extra crates are
build-time only, so the runtime dependency is just `itoa`.

`imageproc`'s `map`/`stats`/`contrast` helpers are scalar and `map_pixels` even allocates a `Vec`
per pixel, which dominated a conversion's profile; the hot ones are replaced by the hand-written
SIMD kernels in [`processing/kernels.rs`](../src/ebook/processing/kernels.rs) (see
[processing.md](processing.md)). `wide` is already in the tree transitively via `quantette`, so
promoting it to a direct dependency adds no new code to the build. The remaining `imageproc` calls
are the ones with no vector analogue (a histogram scatter, a 3×3 convolution and small drawing
primitives).

## Clean-room re-implementation and licensing

This repo ships under **MIT** (`LICENSE`). KCC is distributed under **ISC**, but its
`image.py` and `dualmetafix.py` carry **GPL-3** headers. Therefore:

- The image-processing behaviour (the `ComicPage`/`Cover` code and the crop/split/colour/fill
  algorithms) is **re-implemented from scratch** from documented behaviour and observable
  output, never transliterated from KCC source. Implement each algorithm from its spec
  (thresholds, order of operations, geometry) and validate against fixtures.
- `dualmetafix.py` is **not ported at all**: MOBI/AZW3 encoding *and* its EXTH/doc-type/ASIN
  handling come from the MIT `kindling` crate.
- Do **not** paste GPL code, comments or identifiers from those files into this repo. When in
  doubt, write the implementation from the described behaviour and add a test that pins it.
- Third-party dependencies keep their own licences; verify each new crate before adding it.

The clean-room requirement is not a licence to hand-roll: it means we must not copy GPL
source, not that we must avoid libraries.

Because the design is derived from KCC and the emitted document formats intentionally match
it, KCC's ISC notice is retained in the repository — see [`NOTICE`](../NOTICE).
