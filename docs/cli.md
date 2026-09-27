# CLI

```text
comic-book ebook [OPTIONS] <INPUT>...
```

**Inputs:** one or more comic archives (`.cbz`/`.zip`, `.cbr`/`.rar`, `.cb7`/`.7z`,
`.cbt`/`.tar`), image folders, or (secondary) `.epub`/`.pdf` files. A folder of comics is
expanded; `--file-fusion` combines several inputs into one book.

The `clap` surface is grouped with `#[command(flatten)]` structs mirroring KCC's parser
groups: **Device**, **Main**, **Processing**, **Output**, **Custom profile**.

## Option map (`kcc-c2e` → `comic-book ebook`)

Flags are renamed to snake-case for clarity; semantics are kept.

### Device / profile

| KCC | New | Notes |
|:---|:---|:---|
| `-p/--profile` | `-p/--profile` | default `KV`; case-insensitive; see [Profiles](#profiles) |
| `--customwidth` | `--custom-width` | override profile width |
| `--customheight` | `--custom-height` | override profile height |

### Main

| KCC | New | Notes |
|:---|:---|:---|
| `-m/--manga-style` | `-m/--manga` | RTL reading/splitting |
| `--lightnovel` | `--light-novel` | resize only, preserve structure, output CBZ |
| `--wallpaper` | `--wallpaper` | crop to fill screen |
| `--invertdirection` | `--invert-direction` | invert page-turn direction |
| `-q/--hq` | `-q/--hq` | higher-res panel view |
| `-2/--two-panel` | `-2/--two-panel` | 2-panel (not 4) panel view |
| `--vertical4panel` | `--vertical-4-panel` | side panels first |
| `--legacypanelview` | `--legacy-panel-view` | legacy panel view |
| `-w/--webtoon` | `-w/--webtoon` | webtoon mode |
| `--ts/--targetsize` | `--target-size <MB>` | max output size |
| `--filefusion` | `--file-fusion` | combine inputs into one book |

### Processing

| KCC | New | Notes |
|:---|:---|:---|
| `-n/--noprocessing` | `-n/--no-processing` | pass pages through untouched |
| `-r/--splitter` | `-r/--splitter <0\|1\|2>` | 0 split, 1 rotate, 2 both |
| `-g/--gamma` | `-g/--gamma <float>` | gamma correction (auto when 0) |
| `-c/--cropping` | `-c/--cropping <0\|1\|2>` | 0 off, 1 margins, 2 margins+page numbers |
| `--cp/--croppingpower` | `--cropping-power <float>` | |
| `--cm/--croppingminimum` | `--cropping-minimum <float>` | |
| `--preservemargin` | `--preserve-margin <int>` | |
| `--ipc/--interpanelcrop` | `--inter-panel-crop <0\|1\|2>` | |
| `--blackborders` / `--whiteborders` | `--black-borders` / `--white-borders` | |
| `--forcecolor` | `--force-color` | do not grayscale |
| `--forcepng` | `--force-png` | PNG for B/W pages |
| `--force-png-rgb` | `--force-png-rgb` | colour pages as PNG |
| `--webp` | `--webp` | lossy/lossless WebP output |
| `--noquantize` | `--no-quantize` | |
| `--pnglegacy` | `--png-legacy` | 8-bit PNG (legacy devices) |
| `--jpeg-quality` | `--jpeg-quality <0-95>` | default depends on profile |
| `--maximizestrips` | `--maximize-strips` | 1×4 → 2×2 strips |
| `--autolevel` | `--auto-level` | |
| `--noautocontrast` | `--no-autocontrast` | |
| `--colorautocontrast` | `--color-autocontrast` | |
| `--eraserainbow` | `--erase-rainbow` | moiré filter |
| `--smartcovercrop` | `--smart-cover-crop` | |
| `--coverfill` | `--cover-fill` | crop cover to fill |
| `-u/--upscale` | `-u/--upscale` | |
| `-s/--stretch` | `-s/--stretch` | |
| `--norotate` | `--no-rotate` | |
| `--rotateright` | `--rotate-right` | |
| `--rotatefirst` | `--rotate-first` | |
| `--legacyextract` | `--legacy-extract` | PDF/EPUB legacy image extraction |
| `--pdfwidth` | `--pdf-width` | render vector PDFs to width |
| `-d/--delete` | `-d/--delete` | delete source after success |
| `--tempdir` | `--temp-dir` | spool temp files on the source drive |
| `--mozjpeg` | `--mozjpeg` | accepted but rejected at resolve time (not supported) |

`--splitter`, `--cropping` and `--inter-panel-crop` are constrained to `0..=2`, matching KCC's
`choices`; an out-of-range value is a clap usage error.

### Output

| KCC | New | Notes |
|:---|:---|:---|
| `-o/--output` | `-o/--output <path>` | output directory or filename |
| `-t/--title` | `-t/--title <str>` | default = source name |
| `--metadatatitle` | `--metadata-title <0\|1\|2>` | |
| `--keepcomicinfo` | `--keep-comicinfo` | keep original ComicInfo.xml (CBZ) |
| `-a/--author` | `-a/--author <str>` | default = ComicInfo / `KCC` |
| `--language` | `--language <bcp47>` | default `en-US` |
| `-f/--format` | `-f/--format <FMT>` | see [Formats](#formats) |
| `--nokepub` | `--no-kepub` | `.epub` instead of `.kepub.epub` |
| — | `--kepub-short-ext` | single `.kepub` instead of `.kepub.epub` (requires KePub output) |
| `-b/--batchsplit` | `-b/--batch-split <0\|1\|2>` | |
| `--spreadshift` | `--spread-shift` | |
| `--onepagelandscape` | `--one-page-landscape` | |
| `--ebok` | `--doc-type <ebok\|pdoc\|none>` | replaces the `--ebok` boolean |

`-h/--help` and `-V/--version` are provided by clap (the root `Cli` sets
`propagate_version`). Usage errors exit `2`; runtime errors exit `1`.

## Formats

`-f/--format` values:

| Value | Meaning |
|:---|:---|
| `auto` | pick by profile (MOBI for Kindle, PDF for reMarkable, else EPUB) |
| `epub` | fixed-layout EPUB 3 |
| `kepub` | KePub (EPUB with `.kepub.epub` + Kobo spread properties; `--kepub-short-ext` shortens the extension to `.kepub`) |
| `azw3` | KF8-only Kindle file |
| `mobi` | dual MOBI7+KF8 `.mobi` (legacy devices) |
| `mobi+epub` | keep the intermediate EPUB alongside the MOBI |
| `cbz` | repackage images (no EPUB) |
| `pdf` | PDF |
| `kfx` | EPUB preset for Calibre's KFX Output plugin |
| `epub-200mb` / `pdf-200mb` / `mobi+epub-200mb` | size-capped presets (→ 195 MB target, batch split) |

## Profiles

`--profile` accepts KCC's canonical codes (case-insensitive). The `Profile` `ValueEnum` is
hand-written over the profile table, so `--help`, errors and completions list the codes rather
than heck-cased variants. Each possible value carries the device name as its help text, so
`--help` renders the list with every acronym expanded (e.g. `KPW  Kindle Paperwhite 1/2`), and
the zsh and fish completion scripts list the same descriptions. The tables in
`src/ebook/profiles.rs` are the source of truth for the codes and names.

- **Kindle:** `K1`, `K2`, `KDX`, `K34`, `K57`, `KPW`, `KV`, `KPW34`, `K810`, `KO`, `K11`,
  `KPW5`, `KPW6`, `KS1860`, `KS1920`, `KS1240`, `KS1324`, `KS`, `KCS`, `KS3`, `KSCS`.
- **Kobo:** `KoMT` … `KoE`.
- **reMarkable:** `Rmk1`, `Rmk2`, `RmkPP`, `RmkPPMove`.
- **OTHER:** no screen geometry; requires `--custom-width`/`--custom-height`.

Each row pins `(width, height)`, a quantisation palette (`Palette4`/`15`/`16` or none) and a
device family. `iskindle` = profile ∈ Kindle set, else `isKobo`; `KDX` + CBZ raises the height
to 1200, and Kindle Scribe profiles cap the width at 1920. `OTHER` carries no geometry of its
own, so `Options::resolve` rejects it unless a custom size is supplied (KCC divides by the zero
width there and raises).

`--profile` only selects the device; the output format is chosen separately (see
[Formats](#formats)). `--custom-width`/`--custom-height` override the profile geometry.
