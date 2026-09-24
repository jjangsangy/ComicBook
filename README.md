# comic-book

[![CI](https://github.com/jjangsangy/comic-book/actions/workflows/ci.yml/badge.svg)](https://github.com/jjangsangy/comic-book/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.70%2B-orange.svg)](https://www.rust-lang.org)

A high-performance command-line tool written in Rust for converting comic book archives (`.cbz`, `.cbr`, `.cb7`, `.cbt`, `.zip`, `.rar`, `.7z`, `.tar`) and clamping oversized comic pages to optimize them for e-readers, tablets, and web readers.

---

## Features

- **Multi-Format Archive Conversion**: Batch convert collections across popular comic formats (`cbz`, `cbr`, `cb7`, `cbt`) and standard archive formats (`zip`, `rar`, `7z`, `tar`).
- **Smart Image Size Clamping**:
  - Perfect for hardware with memory/resolution limits (e.g., Kindle, Kobo, e-ink devices) or apps that fail on huge webtoon/manhwa vertical strips.
  - **3 Clamping Approaches**:
    - **`split`** *(default)*: Recursively slices tall images horizontally until every slice is under the pixel threshold, preserving top-to-bottom reading order.
    - **`resize`**: Proportionally downscales images to stay within total pixel limits using high-quality SIMD-accelerated Lanczos3 convolution.
    - **`max-width`**: Constrains horizontal page dimensions to a maximum pixel width while scaling height proportionally.
- **WebP Output**: Clamped images are saved with high-efficiency WebP compression to save storage while preserving crisp comic art.
- **Natural Ordering**: Sorts pages naturally (`page_1.png`, `page_2.png`, ..., `page_10.png`) with `natord` so multi-digit filenames are never scrambled.
- **Multi-Threaded Parallelism**: Leverages all available CPU cores using Rayon, complete with interactive multi-progress bars powered by `indicatif`.
- **Shell Autocompletions**: Built-in completion script generator for Bash, Zsh, Fish, PowerShell, and Elvish.

---

## Supported Formats

| Format | Extension(s) | Extract / Read | Compress / Write | Notes |
|:---|:---|:---:|:---:|:---|
| **Comic Book ZIP** | `.cbz`, `.zip` | Yes | Yes | Native Rust `zip` |
| **Comic Book RAR** | `.cbr`, `.rar` | Yes | Yes | Native Rust `unrar` (read) & `rars` (write) |
| **Comic Book 7-Zip** | `.cb7`, `.7z` | Yes | Yes | Native Rust `sevenz-rust2` (stored / uncompressed) |
| **Comic Book TAR** | `.cbt`, `.tar` | Yes | Yes | Native Rust `tar` |
| **Directory** | Folder of images | Yes | Yes | Plain uncompressed directories |

---

## Installation

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (version 1.87 or newer)

### Build from Source

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

### 3. `completions` — Shell Autocompletions

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

Run unit and integration tests:

```bash
cargo test
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

This project is licensed under the [MIT License](LICENSE).
