# `convert` — archive conversion

Re-package comic archives and image folders between the supported formats, or unpack them
into plain directories. Everything runs in memory; no external archiver is invoked.

```text
comic-book convert <PATHS>... --to <FORMAT>
```

## Target formats

| `--to` value | Container | Notes |
|:---|:---|:---|
| `cbz`, `zip` | ZIP | `.cbz`/`.zip` are the same container |
| `cbr`, `rar` | RAR 4.0 | written by the pure-Rust `rars` crate |
| `cb7`, `7z` | 7-Zip | written **stored (uncompressed)** by `sevenz-rust2`, so output is larger than a compressed 7z |
| `cbt`, `tar` | TAR | |
| `dir` | directory | unpack (or copy) to a folder |

Sources are identified by **magic bytes** (`infer`), not by extension, so a mislabelled
`.cbz`/`.zip`/`.rar`/`.7z`/`.tar` is still recognised. `--to` accepts exactly the lowercase
values in the table above (it is validated by `clap` before the batch runs).

## Inputs

Each path may be a file or a directory:

- **A file** is converted if its magic bytes match a supported format; anything else is skipped
  with a warning.
- **A directory** is expanded:
  - With `--to dir`, every archive file inside it is extracted into a sibling directory named
    after the archive's stem. A directory that contains no archives is skipped
    (`already a directory`).
  - With an archive target, the directory is treated as **one comic** if it is a standalone
    comic directory (it holds image files directly, or holds images with no archive files and
    at most one comic subdirectory); otherwise each archive file and each image subdirectory
    inside it is converted separately.
- Directory entries are walked in **natural order** (`page_2` before `page_10`).
- A source already in the target format is skipped with a message rather than copied.
- Non-existent or unsupported paths are reported and skipped; they do not abort the batch.

## Output

Each output is written next to its source, named after the source's stem (or the directory
name) with the target extension: `Issue_01.cbz → Issue_01.cbr`, `Chapter_01/ → Chapter_01.cbz`.
For `--to dir`, `Issue_01.cbz → Issue_01/`.

## Mechanics

- Conversion streams entry data through **one reusable scratch buffer** shared across the whole
  batch, so peak memory is bounded by the largest single entry rather than the whole book.
- Converting between identical kinds is a direct byte copy (file) or directory copy.
- Extracting to a directory strips the archive's single redundant wrapper folder when it has
  one, so `Series/Chapter 1/*.png` becomes `Chapter 1/*.png` under the destination.
- Entry names are normalised (forward slashes, no `..`/absolute/drive-letter segments) on both
  read and write, so archives authored on any OS convert safely.
- A partially written archive is removed if the write fails.

## Examples

```bash
# A single CBZ to CBR
comic-book convert Issue_01.cbz --to cbr

# Unpack an archive into a directory
comic-book convert Issue_01.cbz --to dir

# Pack a folder of images into a CBZ
comic-book convert Issue_01 --to cbz

# Convert every archive inside a folder (and any comic subfolders)
comic-book convert ~/Comics/Batman --to cbz

# Batch several inputs into 7-Zip
comic-book convert ~/Comics/SeriesA ~/Comics/SeriesB --to cb7
```

The archive layer that backs this command (readers, writers, format detection and path
normalisation) lives in `src/archive/`; the `ebook` command reuses it for its input adapters.
