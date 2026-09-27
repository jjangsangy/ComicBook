# Processing

The per-page image pipeline (`src/ebook/processing/`) and the KCC algorithms it reproduces.
Everything operates on decoded RGB/RGBA/grayscale images.

## Per-page order

For each page: background detection (`fillCheck`) → cropping (margin/page-number, inter-panel)
→ splitter (`splitCheck`) → gamma → grayscale (if B/W) → autocontrast/autolevel → resize →
moiré erase → quantize/convert → encode.

## Algorithms

1. **`colorCheck`** (`color.rs`) — convert to YCbCr, take Cb/Cr histograms, apply the
   cutoff/diff-threshold cascade `((0,0),22) → ((.2,.2),10) → ((3,3),4)`; shortcut for original
   mode `L`/`1`; always colour in webtoon mode. `--force-color` changes the precision test.
   Pillow's JFIF YCbCr and Rec. 601 luma formulas are re-implemented here (not `image`'s
   Rec. 709 grayscale, which would drift from KCC).
2. **`fillCheck`** (`fill.rs`) — threshold ≤128 → 1-bit; compare black/white `getbbox` surface
   areas; if close, sample 5-px rows/columns via histograms to decide `white`/`black`. Returns
   the page background; overridable with `--black-borders`/`--white-borders`.
3. **`splitCheck`** (`page.rs`) — decide `N` (normal), `R` (rotated spread), `S1`/`S2` (split
   halves) from the aspect ratios vs. the device ratio, `--splitter`, `--no-rotate`,
   `--rotate-right`, `--maximize-strips` and webtoon. `BISECT_THRESHOLD = 1.8`; split when
   `w/h > 1.16`.
4. **Cropping** (`crop.rs`) — `get_bbox_crop_margin[_page_number]`: grayscale, optional invert
   (non-white background), `autocontrast(1)`, box blur(1), threshold `240 - power*64`, ignore
   pixels near the edges, then page-number detection via row grouping + box merging
   (`merge_boxes`, `group_close_values`). Cap the crop to 10 % per side; respect
   `--preserve-margin` and `--cropping-minimum`.
5. **Inter-panel crop** (`interpanel.rs`) — find empty rows/columns (not near the borders),
   keep a 4 % gutter, delete those rows/columns.
6. **`ComicPage` pipeline** (`page.rs`) — KCC's order: gamma → grayscale (if B/W) →
   autocontrast (`preserve_tone`, skipped when low-contrast: `max - min < 159`) → `autolevel`
   when requested → resize → moiré erase → quantize/convert → encode. Resize uses
   `contain`/`fit`/`pad` semantics with `BICUBIC` when downscaling within the profile and
   `LANCZOS` otherwise, plus the `--stretch`/`--wallpaper`/`--upscale` paths.
7. **Encoding** (`page.rs`) — JPEG (quality), PNG (`--force-png`, lossless-ish), GIF (Kindle
   Scribe B/W), WebP (`--webp`), in `save_with_codec`'s branch order. The OPF manifest media
   type must match (`image/jpeg|png|gif|webp`).
8. **`Cover`** (`cover.rs`) — flatten to RGB → unconditional `autocontrast(preserve_tone=True)`
   → optional grayscale (`--force-color` keeps colour) → optional `--smart-cover-crop` → fit
   to the profile (thumbnail with `--cover-fill`) → JPEG. A split book's cover also gets the
   `N/M` tome label. Reuses the pinned page helpers — no separate image algorithm.
9. **Webtoon (`comic2panel`)** (`webtoon.rs`) — merge chapter images vertically, detect panels
   via `FIND_EDGES` (a 3×3 Laplacian) thresholded at `> 6` and solid-row scanning, split
   over-long panels with overlap, repack into virtual pages at the device width/height (max
   width 1072). See [porting.md](porting.md) for the reproduced quirks.
10. **Moiré eraser** (`rainbow.rs`) — RGB→YUV, FFT the luminance (`rustfft`), attenuate the
    diagonal frequencies ≥ 0.30 cycles/px around 135°±10° (and perpendicular) by 0.10, inverse
    FFT, clip. The grayscale path operates on `L` directly.

## SIMD kernels

The per-pixel hotspots that `imageproc` leaves scalar — and where its `map_pixels` allocated a
`Vec` per pixel — live in `processing/kernels.rs` and are vectorised with the portable `wide`
lane types (NEON on aarch64, SSE2 on x86_64, portable fallback elsewhere):

- Rec.601 luma min/max, a 16-lane reduction, for the autocontrast range.
- In-place inversion and binary threshold of `L8` planes.
- The 3-tap `BoxBlur(1)`, horizontal then vertical, with the exact `(a + b + c + 1) / 3` rounding.
- Bounding boxes (`>= t`, `< t`, non-zero) and rectangle counts, via 16-lane compare bitmasks.
- Row/column emptiness (inter-panel crop) and the webtoon band's white/black test.
- The autocontrast stretch as one 256-entry LUT, replacing a per-pixel integer division.

Two rules constrain what may be vectorised:

1. **Bit-exactness.** Every kernel does integer arithmetic that reproduces the scalar reference
exactly (the box-blur divide is an exact magic multiply, the stretch LUT applies the same integer
formula). Nothing reassociates a pinned float computation.
2. **Pinned float weights stay scalar.** The Rec.601 grayscale conversion and `colorCheck`'s Cb/Cr
histograms keep their `f64` per-pixel arithmetic: their decimal weights are pinned to
Pillow/KCC, and a fixed-point reimplementation rounds differently from the `f64` result on the
exact-half inputs (~0.02 % of RGB triples). Their loops are allocation-free but deliberately not
vectorised, so the emitted pages do not change.

The kernels are pinned against scalar references by the tests in `kernels.rs`, so a vectorisation
bug fails a test instead of silently changing an image.

## Fidelity rules

KCC/Pillow quirks that are reproduced deliberately (each pinned by tests; the full rationale is
in [porting.md](porting.md)):

- **Pillow primitives are reproduced, not approximated.** `ImageOps.autocontrast(cutoff=1)`
  drops 1 % of the histogram per end and stretches through a truncated linear LUT;
  `ImageFilter.BoxBlur(1)` is a horizontal then vertical 3-tap average, each pass rounded,
  with edge replication; `Image.crop` rounds half-to-even and zero-fills out-of-bounds pixels;
  `getbbox` uses the non-zero extent.
- **`group_close_values`' value drop is preserved** (a value that opens a new group is
  discarded, so `[1,2,3,10,11]` groups to `[(1,3),(11,11)]`).
- **The inter-panel finder's height/width mix-up is preserved** (the column pass compares
  against the page height, not width).
- **The moiré eraser filters the full complex spectrum**, equivalent to numpy's `rfft2`
  half-spectrum because the mask is symmetric under `f → -f`; tests assert behaviour rather
  than exact bytes.
- **`--wallpaper` implements the documented intent** (crop to fill), because KCC 9.x's
  `resizeImage` branch is unreachable after a bare `pass`.
- **Webtoon merge reproduces the undersized-canvas quirk** (`mergeDirectory` sizes the canvas
  from the original heights, clipping a widened page at the bottom).
- **The webtoon edge filter restores Pillow's border ring** after the `imageproc` convolution,
  whose padding differs from Pillow's leave-unchanged border.

Cropping runs in `prepare_page`, *before* the splitter (as KCC does in
`ComicPageParser.__init__`), and is skipped in webtoon mode and for a colour first page.
