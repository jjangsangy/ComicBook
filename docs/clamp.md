# `clamp` — image size clamping

Rewrite oversized comic pages so every image in a book is under a size limit. This is a
standalone utility (not part of the KCC port); it is useful for devices or readers that choke on
large scans or very tall webtoon strips.

```text
comic-book clamp [OPTIONS] <INPUT>
```

## Options

| Option | Flag | Default | Description |
|:---|:---|:---|:---|
| `<INPUT>` | *positional* | *(required)* | A comic archive/folder, or a directory of archives/folders. |
| `--output-dir` | `-o` | `Results` | Directory the clamped results are written under. |
| `--size-threshold` | `-s` | `5000000` | Total-pixel limit (`split`/`resize`) or maximum width in pixels (`max-width`). |
| `--approach` | `-a` | `split` | One of `split`, `resize`, `max-width`. |
| `--workers` | `-w` | CPU count | Worker threads for the rayon pool. |

## Approaches

- **`split`** (default) — iteratively halves the height of any image whose total pixels
  (`width × height`) are ≥ the threshold, repeating until every segment is below it. Segments
  keep top-to-bottom reading order. Best for tall webtoon/manhwa strips.
- **`resize`** — proportionally downscales any image ≥ the threshold so its total pixel count
  fits, using SIMD-accelerated Lanczos3 resampling.
- **`max-width`** — downscales so the width is ≤ the threshold, preserving the aspect ratio.

`split` and `resize` enforce the same total-pixel rule, so they share the same minimum
threshold. Each approach rejects a threshold it cannot make progress with:

| Approach | Minimum `--size-threshold` |
|:---|:---|
| `split`, `resize` | > 500,000 (total pixels) |
| `max-width` | > 400 (pixels wide) |

## Behaviour

- **Input.** A file must be a supported archive. A directory is scanned one level deep: each
  archive file and each subdirectory (treated as an image folder) becomes a book, processed in
  natural order. An entry that is the output directory is skipped, and the run refuses to write
  into the directory it reads from.
- **Fast path.** If every page in a book already measures below the threshold, the book is
  extracted **verbatim** (no decode/re-encode). Otherwise its pages are decoded, clamped, and
  re-encoded.
- **Output.** Images are always written as WebP (quality 90) under
  `<output-dir>/<chapter-name>/NNN.webp`, numbered from `001` in reading order. An existing
  chapter directory is replaced. (The `ebook` command instead keeps each device profile's
  format; `clamp` is WebP-only by design.)
- **Parallelism.** Books are processed in parallel across `--workers` threads, with an overall
  progress bar and one bar per book. A per-book error is reported without aborting the batch.
- Pages are matched by the wider `image_ops::IMG_EXTENSIONS` set
  (`.jpg/.jpeg/.png/.tiff/.tif/.bmp/.webp/.gif/.pgm`), broader than the `ebook` page set.

## Examples

```bash
# Split oversized pages of one archive to below 4 megapixels
comic-book clamp Chapter_01.cbz -o Processed -s 4000000 --approach split

# Downscale every book in a folder to fit within 5 megapixels
comic-book clamp ~/Manga/Series -o ~/Manga/Series_Optimized --approach resize

# Resize all pages to a maximum width of 1200 px using 4 worker threads
comic-book clamp ~/Comics/Issue1.cbz -o Clamped -s 1200 -a max-width -w 4
```
