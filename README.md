# comic-book

[![CI](https://github.com/jjangsangy/ComicBook/actions/workflows/ci.yml/badge.svg)](https://github.com/jjangsangy/ComicBook/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/jjangsangy/ComicBook/branch/main/graph/badge.svg)](https://codecov.io/gh/jjangsangy/ComicBook)
[![Release](https://github.com/jjangsangy/ComicBook/actions/workflows/release.yml/badge.svg)](https://github.com/jjangsangy/ComicBook/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.93%2B-orange.svg)](https://www.rust-lang.org)

[![Codecov icicle coverage graph](https://codecov.io/gh/jjangsangy/ComicBook/graphs/icicle.svg)](https://codecov.io/gh/jjangsangy/ComicBook)

A high-performance command-line tool written in Rust for working with comic-book archives: repackage them between `.cbz`/`.cbr`/`.cb7`/`.cbt` (and plain folders), clamp oversized pages for size-limited readers, and convert them into e-book formats — fixed-layout EPUB, Kobo KePub, Kindle AZW3/MOBI and PDF — for e-readers, tablets and phones. Everything is compiled into the binary; no `7z`, `unrar`, `kindlegen`, `ImageMagick` or other external program is required.

---

## Features

- **Multi-Format Archive Conversion** (`convert`): Batch convert collections across popular comic formats (`cbz`, `cbr`, `cb7`, `cbt`) and standard archive formats (`zip`, `rar`, `7z`, `tar`), or unpack them to folders.
- **E-book Conversion** (`ebook`): Convert comics into fixed-layout EPUB 3, Kobo KePub, Kindle AZW3/MOBI and PDF (or repackaged CBZ), with per-device profiles, Panel View, webtoon mode, cropping/enhancement, `ComicInfo.xml` metadata, and size-based tome chunking.
- **Smart Image Size Clamping** (`clamp`):
  - Perfect for hardware with memory/resolution limits (e.g., Kindle, Kobo, e-ink devices) or apps that fail on huge webtoon/manhwa vertical strips.
  - **3 Clamping Approaches**:
    - **`split`** *(default)*: Recursively slices tall images horizontally until every slice is under the pixel threshold, preserving top-to-bottom reading order.
    - **`resize`**: Proportionally downscales images to stay within total pixel limits using high-quality SIMD-accelerated Lanczos3 convolution.
    - **`max-width`**: Constrains horizontal page dimensions to a maximum pixel width while scaling height proportionally.
- **No External Programs**: Archive extraction, image processing, EPUB/PDF/MOBI authoring and PDF rasterisation are all native Rust — nothing shells out to `7z`, `unrar`, `kindlegen` or ImageMagick.
- **WebP Output**: Clamped images are saved with high-efficiency WebP compression to save storage while preserving crisp comic art.
- **Natural Ordering**: Sorts pages naturally (`page_1.png`, `page_2.png`, ..., `page_10.png`) with `natord` so multi-digit filenames are never scrambled.
- **Robust Input Handling**: Malformed, truncated or hostile archives, EPUBs and PDFs are rejected with a clear error rather than crashing; the pipeline is fuzzed and memory-tested across Linux, macOS and Windows in CI.
- **Multi-Threaded Parallelism**: Leverages all available CPU cores using Rayon, complete with interactive multi-progress bars powered by `indicatif`.
- **Shell Autocompletions**: Built-in completion script generator for Bash, Zsh, Fish, PowerShell, and Elvish.

---

## Documentation

The reference documentation lives in [`docs/`](docs/):

| Document | Contents |
|:---|:---|
| [docs/convert.md](docs/convert.md) | `convert`: formats, directory expansion, output naming, mechanics |
| [docs/clamp.md](docs/clamp.md) | `clamp`: approaches, thresholds, output layout |
| [docs/cli.md](docs/cli.md) | `ebook`: options, formats and device profiles |
| [docs/architecture.md](docs/architecture.md) | `ebook`: pipeline, module map, data model, design principles |
| [docs/refactor.md](docs/refactor.md) | Type-safety refactor: the completed `make impossible states unrepresentable` pass |
| [docs/processing.md](docs/processing.md) | `ebook`: image-processing algorithms and fidelity rules |
| [docs/output.md](docs/output.md) | `ebook`: EPUB/KePub/CBZ/PDF/Kindle document specs, chunking, fusion |
| [docs/dependencies.md](docs/dependencies.md) | Off-the-shelf policy, crates, licences, clean-room rules |
| [docs/porting.md](docs/porting.md) | Porting history and the decisions/deviations behind the code |
| [docs/development.md](docs/development.md) | Testing, CI, cross-platform notes, glossary |

Run `comic-book <command> --help` for the complete flag reference.

---

## Supported Formats

### Archives & folders

| Format | Extension(s) | Extract / Read | Compress / Write | Notes |
|:---|:---|:---:|:---:|:---|
| **Comic Book ZIP** | `.cbz`, `.zip` | Yes | Yes | Native Rust `zip` |
| **Comic Book RAR** | `.cbr`, `.rar` | Yes | Yes | Native Rust `unrar` (read) & `rars` (write) |
| **Comic Book 7-Zip** | `.cb7`, `.7z` | Yes | Yes | Native Rust `sevenz-rust2` (stored / uncompressed) |
| **Comic Book TAR** | `.cbt`, `.tar` | Yes | Yes | Native Rust `tar` |
| **Directory** | Folder of images | Yes | Yes | Plain uncompressed directories |

### E-book formats (`ebook`)

| Format | Extension(s) | Read (input) | Write (output) | Notes |
|:---|:---|:---:|:---:|:---|
| **EPUB 3** | `.epub` | Yes | Yes | Fixed-layout output; input reads spine-ordered images |
| **PDF** | `.pdf` | Yes | Yes | Input extracts embedded images / rasterises vector pages |
| **KePub** | `.kepub.epub` | Yes | Yes | Kobo EPUB variant (page-spread properties) |
| **Kindle AZW3** | `.azw3` | No | Yes | KF8-only, via the `kindling` crate |
| **Kindle MOBI** | `.mobi` | No | Yes | Dual MOBI7 + KF8, for legacy devices |
| **CBZ** | `.cbz` | Yes | Yes | Plain repackage of the processed images |

---

## Installation

### Install a Prebuilt Release

Prebuilt binaries are attached to every tagged release:

| Platform | Architecture | Archive |
|:---|:---|:---|
| macOS | Apple Silicon (arm64) | `comic-book-aarch64-apple-darwin.tar.gz` |
| macOS | Intel (x86_64) | `comic-book-x86_64-apple-darwin.tar.gz` |
| Linux | x86_64 | `comic-book-x86_64-unknown-linux-musl.tar.gz` (static) |
| Linux | aarch64 / arm64 | `comic-book-aarch64-unknown-linux-musl.tar.gz` (static) |
| Windows | x86_64 | `comic-book-x86_64-pc-windows-msvc.zip` |

#### macOS & Linux

```bash
curl -fsSL https://raw.githubusercontent.com/jjangsangy/ComicBook/main/scripts/install.sh | sh
```

The script detects your OS and architecture, downloads the matching binary, verifies its SHA-256 checksum, and installs it to `~/.local/bin` (or `/usr/local/bin` when run as root). It prints a `PATH` hint if the install directory is not already on your `PATH`.

Pass options to the piped script with `sh -s --`:

```bash
# Install a specific version
curl -fsSL https://raw.githubusercontent.com/jjangsangy/ComicBook/main/scripts/install.sh | sh -s -- --version v0.2.0

# Choose where the binary is installed
curl -fsSL https://raw.githubusercontent.com/jjangsangy/ComicBook/main/scripts/install.sh | sh -s -- --install-dir "$HOME/bin"
```

Or clone the repository and run it directly (see `./scripts/install.sh --help` for every option):

```bash
./scripts/install.sh
```

#### Windows

```powershell
irm https://raw.githubusercontent.com/jjangsangy/ComicBook/main/scripts/install.ps1 | iex
```

This installs `comic-book.exe` to `%LOCALAPPDATA%\Programs\comic-book\bin` and adds that directory to your user `PATH` (restart your terminal afterwards).

To pass options, download the script and run it:

```powershell
.\scripts\install.ps1 -Version v0.2.0 -InstallDir "$env:USERPROFILE\bin"
```

#### Verify the Install

```bash
comic-book --version
```

### Build from Source

Requires [Rust](https://www.rust-lang.org/tools/install) (version 1.93 or newer).

```bash
git clone https://github.com/jjangsangy/comic-book.git
cd comic-book

# Build release binary
cargo build --release

# Install locally to Cargo bin directory
cargo install --path .
```

The compiled binary will be placed at `target/release/comic-book` (or in your `~/.cargo/bin` directory if installed).

---

## Usage

```text
Comic book archive conversion and image clamping tool

Usage: comic-book <COMMAND>

Commands:
  convert      Convert different archive formats (cbr, cbz, etc..)
  clamp        Clamp image sizes in comic archives to all be under a size threshold
  ebook        Convert comics into e-book formats (epub, kepub, azw3, mobi, pdf, cbz)
  completions  Generate shell completion scripts (alias: completion)
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

---

### 1. `convert` — Archive Conversion

Convert comic archives between formats — taking individual comic files (e.g. `.cbz`, `.cbr`, etc.) or directories of archives, and generating the corresponding converted archives with `--to <FORMAT>`. Conversions between archives and image extractions are performed in-memory and streamed using buffered I/O, avoiding expensive temporary file creation on network filesystems.

```bash
comic-book convert <PATHS>... --to <FORMAT>
```

See [`docs/convert.md`](docs/convert.md) for the full format table, directory-expansion rules and conversion mechanics.

#### Supported Target Formats
`cbz`, `zip`, `cbr`, `rar`, `cb7`, `7z`, `cbt`, `tar`, `dir`

#### Examples

```bash
# Convert a single CBZ file into CBR
comic-book convert Issue_01.cbz --to cbr

# Unpack an archive into a directory
comic-book convert Issue_01.cbz --to dir

# Convert a directory of images into a comic book archive
comic-book convert Issue_01 --to cbz

# Convert multiple specific files or folders
comic-book convert Issue_01.cbz Issue_02.cbz --to cbt
comic-book convert Chapter_01 Chapter_02 --to cbz

# Convert all archives or chapter folders inside one or more directories
comic-book convert ~/Comics/Batman --to cbz
comic-book convert ~/Comics/Batman --to dir
comic-book convert ~/Comics/SeriesA ~/Comics/SeriesB --to cb7
```

---

### 2. `clamp` — Optimize & Clamp Image Sizes

Clamp image dimensions within comic archives or directories, either by splitting tall vertical strips or downscaling them.

```bash
comic-book clamp [OPTIONS] <INPUT_DIR_OR_FILE>
```

See [`docs/clamp.md`](docs/clamp.md) for the threshold rules, fast path and output layout.

#### Options

| Option | Flag | Default | Description |
|:---|:---|:---|:---|
| `<INPUT_DIR_OR_FILE>` | *Positional* | *(required)* | Path to a comic archive file or a directory of chapters/archives. |
| `--output-dir` | `-o` | `Results` | Directory where processed chapter folders will be saved. |
| `--size-threshold` | `-s` | `5000000` | Size limit (total pixels for `split`/`resize`, or pixel width for `max-width`). |
| `--approach` | `-a` | `split` | Size-enforcement strategy: `split`, `resize`, or `max-width`. |
| `--workers` | `-w` | *(CPU cores)* | Number of worker threads for parallel processing. |

#### Clamping Approaches

- **`split` (default)**:  
  Iteratively halves vertical strips that exceed `--size-threshold` pixels until every segment is beneath the limit. Ideal for reading long webtoons/manhwa on devices with image dimension limits.
- **`resize`**:  
  Scales down images exceeding `--size-threshold` total pixels (`width * height`) proportionally using high-quality Lanczos3 resampling.
- **`max-width`**:  
  Enforces a strict maximum width (in pixels) via `--size-threshold`. Useful for standardizing page widths across varied scans.

#### Examples

```bash
# Split oversized webtoon pages in a single chapter archive
comic-book clamp Chapter_01.cbz -o Processed -s 4000000 --approach split

# Downscale all chapters in a folder to fit within 5 megapixels
comic-book clamp ~/Manga/Series -o ~/Manga/Series_Optimized --approach resize

# Resize all pages to a maximum width of 1200px using 4 worker threads
comic-book clamp ~/Comics/Issue1.cbz -o Clamped -s 1200 -a max-width -w 4
```

---

### 3. `ebook` — Comic to E-book Conversion

Convert comic archives (`.cbz`/`.cbr`/`.cb7`/`.cbt`), image folders, and (as input)
`.epub`/`.pdf` files into e-book formats for Kindle, Kobo, reMarkable and generic EPUB
readers. Everything — archive extraction, image processing, EPUB/PDF/MOBI building and PDF
rasterisation — is compiled into the binary: no `7z`, `unrar`, `kindlegen`, ImageMagick or
other external program is required.

```bash
comic-book ebook [OPTIONS] <INPUT>...
```

#### Output formats (`-f/--format`)

| Value | Meaning |
|:---|:---|
| `auto` (default) | MOBI for Kindle profiles, PDF for reMarkable, otherwise EPUB |
| `epub` | fixed-layout EPUB 3 |
| `kepub` | KePub (`.kepub.epub` for Kobo) |
| `azw3` / `mobi` | Kindle KF8 (`.azw3`) or dual MOBI7+KF8 (`.mobi`) |
| `mobi+epub` | keep the intermediate EPUB alongside the MOBI |
| `cbz` / `pdf` | repackaged images, or a PDF |
| `kfx` | EPUB preset for Calibre's KFX Output plugin |
| `epub-200mb` / `pdf-200mb` / `mobi+epub-200mb` | size-capped presets |

#### Inputs

- Comic archives `.cbz`/`.zip`, `.cbr`/`.rar`, `.cb7`/`.7z`, `.cbt`/`.tar`, and image folders.
- EPUBs (spine-ordered images) and PDFs (embedded images extracted, vector pages rasterised).
- A folder of comics is expanded; `--file-fusion` combines several inputs into one book.

#### Examples

```bash
# A CBZ to its default e-book for the default Kindle profile
comic-book ebook Issue_01.cbz

# Right-to-left manga at a fixed size for Kobo
comic-book ebook "Vol 1.cbz" -m -p KoE --target-size 100

# AZW3 for a Kindle, with panel view
comic-book ebook Issue_01.cbz -f azw3 -q

# A PDF (vector pages rasterised) into an EPUB
comic-book ebook scan.pdf -f epub

# Merge several sources into a single book
comic-book ebook ch1.cbz ch2.cbz ch3.cbz --file-fusion -t "Omnibus"
```

#### Full option reference

Run `comic-book ebook --help` to print the complete flag reference (profiles, cropping, colour
handling, Panel View, webtoon mode, chunking and more):

```text
Convert comics into e-book formats (epub, kepub, azw3, mobi, pdf, cbz)

Usage: comic-book ebook [OPTIONS] <INPUT>...

Arguments:
  <INPUT>...
          Files, folders or archives to convert

Options:
  -p, --profile <PROFILE>
          Device profile

          Possible values:
          - K1:        Kindle 1
          - K2:        Kindle 2
          - KDX:       Kindle DX/DXG
          - K34:       Kindle Keyboard/Touch
          - K57:       Kindle 5/7
          - KPW:       Kindle Paperwhite 1/2
          - KV:        Kindle Voyage
          - KPW34:     Kindle Paperwhite 3/4/Oasis
          - K810:      Kindle 8/10
          - KO:        Kindle Oasis 2/3
          - K11:       Kindle 11
          - KPW5:      Kindle Paperwhite 5/Signature Edition
          - KPW6:      Kindle Paperwhite 6
          - KS1860:    Kindle 1860
          - KS1920:    Kindle 1920
          - KS1240:    Kindle 1240
          - KS1324:    Kindle 1324
          - KS:        Kindle Scribe 1/2
          - KCS:       Kindle Colorsoft
          - KS3:       Kindle Scribe 3
          - KSCS:      Kindle Scribe Colorsoft
          - KoMT:      Kobo Mini/Touch
          - KoG:       Kobo Glo
          - KoGHD:     Kobo Glo HD
          - KoA:       Kobo Aura
          - KoAHD:     Kobo Aura HD
          - KoAH2O:    Kobo Aura H2O
          - KoAO:      Kobo Aura ONE
          - KoN:       Kobo Nia
          - KoC:       Kobo Clara HD/Kobo Clara 2E
          - KoCC:      Kobo Clara Colour
          - KoL:       Kobo Libra H2O/Kobo Libra 2
          - KoLC:      Kobo Libra Colour
          - KoF:       Kobo Forma
          - KoS:       Kobo Sage
          - KoE:       Kobo Elipsa
          - Rmk1:      reMarkable 1
          - Rmk2:      reMarkable 2
          - RmkPP:     reMarkable Paper Pro
          - RmkPPMove: reMarkable Paper Pro Move
          - OTHER:     Other

          [default: KV]

  -m, --manga
          Manga style (right-to-left reading and splitting)

      --light-novel
          Only resize images and preserve the original file structure

      --wallpaper
          Crop to fill the screen

      --invert-direction
          Invert page turn direction

  -q, --hq
          Try to increase the quality of magnification

  -2, --two-panel
          Display two, not four, panels in Panel View mode

      --vertical-4-panel
          Display side panels first in virtual panel view

      --legacy-panel-view
          Use the legacy panel view method from KCC 6

  -w, --webtoon
          Webtoon processing mode

      --target-size <MB>
          Maximal size of the output file in MB

      --file-fusion
          Combine all input files into a single book

  -n, --no-processing
          Do not modify images and ignore any profile or processing option

  -r, --splitter <0|1|2>
          Double page parsing mode: 0 split, 1 rotate, 2 both

          [default: 0]

  -g, --gamma <FLOAT>
          Apply gamma correction to linearize the image (auto when 0)

          [default: 0.0]

  -c, --cropping <0|1|2>
          Cropping mode: 0 disabled, 1 margins, 2 margins + page numbers

          [default: 2]

      --cropping-power <FLOAT>
          Cropping power

          [default: 1.0]

      --cropping-minimum <FLOAT>
          Cropping minimum area ratio

          [default: 0.0]

      --preserve-margin <PERCENT>
          After calculating the crop, back up the specified percentage

          [default: 0]

      --inter-panel-crop <0|1|2>
          Crop empty sections: 0 disabled, 1 horizontally, 2 both

          [default: 0]

      --black-borders
          Disable border autodetection and force black borders

      --white-borders
          Disable border autodetection and force white borders

      --force-color
          Don't convert images to grayscale

      --force-png
          Create PNG files instead of JPEG for black and white images

      --force-png-rgb
          Force colour images to be saved as PNG

      --webp
          Replace JPEG with lossy WebP and PNG with lossless WebP

      --png-legacy
          Use a more compatible 8-bit PNG instead of 4-bit

      --no-quantize
          Don't quantize to a 16-colour PNG

      --jpeg-quality <0-95>
          The JPEG quality, on a scale from 0 (worst) to 95 (best)

      --maximize-strips
          Turn 1x4 strips into 2x2 strips

      --auto-level
          Set the most common dark pixel value as the black point for leveling

      --no-autocontrast
          Disable autocontrast

      --color-autocontrast
          Autocontrast colour pages too

      --erase-rainbow
          Erase the rainbow effect on colour e-ink screens

      --smart-cover-crop
          Attempt to crop the main cover from a wide image

      --cover-fill
          Crop the cover to fill the screen

  -u, --upscale
          Resize images smaller than the device's resolution

  -s, --stretch
          Stretch images to the device's resolution

      --no-rotate
          Do not rotate double-page spreads in the spread splitter

      --rotate-right
          Rotate double-page spreads in the opposite direction

      --rotate-first
          Put the rotated 2-page spread first in the spread splitter

      --legacy-extract
          Use the legacy PDF/EPUB image extraction method from older KCC versions

      --pdf-width
          Render vector PDFs to device width instead of height

  -d, --delete
          Delete source files or directories after a successful conversion

      --temp-dir
          Create temporary files on the source file's drive

      --mozjpeg
          Create JPEG files using mozjpeg (unsupported; use --jpeg-quality)

  -o, --output <PATH>
          Output directory or file

  -t, --title <TITLE>
          Comic title (default: filename or directory name)

      --metadata-title <0|1|2>
          Write title using embedded metadata: 1 combine with the default schema, 2 use it only

          [default: 0]

      --keep-comicinfo
          Keep any original ComicInfo.xml files

  -a, --author <AUTHOR>
          Author name (default: KCC)

      --language <BCP47>
          EPUB language

          [default: en-US]

  -f, --format <FORMAT>
          Output format

          Possible values:
          - auto:            Pick by profile: MOBI for Kindle, PDF for reMarkable, otherwise EPUB
          - epub:            Fixed-layout EPUB 3
          - kepub:           Kobo KePub (`.kepub.epub`)
          - azw3:            KF8-only Kindle file
          - mobi:            Dual MOBI7 + KF8 `.mobi`
          - mobi+epub:       Keep the intermediate EPUB alongside the MOBI
          - cbz:             Repackage the processed images as a comic archive
          - pdf:             PDF
          - kfx:             EPUB preset for Calibre's KFX Output plugin
          - epub-200mb:      EPUB preset capped at ~200 MB
          - pdf-200mb:       PDF preset capped at ~200 MB
          - mobi+epub-200mb: MOBI + EPUB preset capped at ~200 MB

          [default: auto]

      --no-kepub
          Output EPUB with a `.epub` extension rather than `.kepub.epub`

      --kepub-short-ext
          Output KePub with a single `.kepub` extension rather than `.kepub.epub`

  -b, --batch-split <0|1|2>
          Split output into multiple files: 0 none, 1 automatic, 2 per subdirectory

          [default: 0]

      --spread-shift
          Shift the first page to the opposite side in landscape for spread alignment

      --one-page-landscape
          Show a single centred page in landscape

      --doc-type <ebok|pdoc|none>
          Kindle doc-type tag

          Possible values:
          - none: Leave the doc-type untouched (avoids the firmware "back-to-library" issue)
          - ebok: Force the EBOK tag
          - pdoc: Force the PDOC tag

          [default: none]

      --custom-width <PX>
          Replace the screen width provided by the device profile

          [default: 0]

      --custom-height <PX>
          Replace the screen height provided by the device profile

          [default: 0]

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

See [`docs/cli.md`](docs/cli.md) for every option and device profile, and
[`docs/output.md`](docs/output.md) for the output document formats.

---

### 4. `completions` — Shell Autocompletions

Generate completion scripts for your shell.

```bash
# Bash
comic-book completions bash > ~/.local/share/bash-completion/completions/comic-book

# Zsh
comic-book completions zsh > ~/.zfunc/_comic-book

# Fish
comic-book completions fish > ~/.config/fish/completions/comic-book.fish

# PowerShell
comic-book completions powershell >> $PROFILE
```

---

## Development & Testing

Run unit and integration tests with [cargo-nextest](https://nexte.st/) (install once with
`cargo install cargo-nextest --locked`). Pipe the run through `tail` so only the pass/fail
summary and the failing cases survive:

```bash
cargo nextest run 2>&1 | tail -n 20
```

Check code style and linter warnings:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features
```

---

## Contributing

Contributions are welcome! Please check out [CONTRIBUTING.md](CONTRIBUTING.md) for details on submitting issues and pull requests.

---

## License

This project is licensed under the [MIT License](LICENSE). Third-party notices, including the
ISC notice for Kindle Comic Converter, whose command-line behaviour this project reimplements,
are recorded in [NOTICE](NOTICE).
