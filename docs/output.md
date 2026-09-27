# Output

The output builders (`src/ebook/output/`) reproduce KCC's document formats. These documents
are **device-sensitive**: preserve their structure and semantics, and pin any intentional
deviation with a test (see [porting.md](porting.md)).

## EPUB / KePub

Layout:

```text
mimetype                     (stored, first)
META-INF/container.xml
OEBPS/Text/style.css
OEBPS/Text/**/*.xhtml        (one per page; Panel View divs when enabled)
OEBPS/Images/**              (cover.jpg + page images, chapters preserved)
OEBPS/toc.ncx
OEBPS/nav.xhtml
OEBPS/content.opf
```

- **XHTML** (`output/epub/xhtml.rs`): `<!DOCTYPE html>`, a `viewport` meta with the image
  width/height (÷1.5 when `--hq`), an `img` with absolute `width`/`height`, `../` backrefs
  computed from the `Images` depth, a `display:none` div for Kindle panel mode, and the `PV-*`
  Panel View divs.
- **OPF** (`output/epub/opf.rs`): `package version="3.0"`, Dublin Core metadata,
  `dc:contributor` `KindleComicConverter-<ver>`, `belongs-to-collection`/`group-position`
  (series/volume/number, non-Kindle), `dcterms:modified`, the Kindle fixed-layout metas
  (`fixed-layout`/`original-resolution`/`book-type`/`primary-writing-mode`/`zero-gutter`/
  `zero-margin`/`ke-border-*`/`orientation-lock`/`region-mag`), `rendition:spread`/
  `rendition:layout`, the manifest, and the spine with the two-pass spread-property algorithm
  (forward alternation with the `-kcc-a/b/c/d/x` specials, backward fix-up, `--spread-shift`,
  `--one-page-landscape`, `page-progression-direction`).
- **NCX/NAV** (`output/epub/nav.rs`): `toc.ncx` plus `nav.xhtml` with `epub:type="toc"` and a
  `page-list`; `ComicInfo.xml` bookmarks become the navigation entries.
- **Packaging** (`output/epub/package.rs`): `mimetype` first and stored, every other entry
  stored (KCC's payloads are already compressed images).
- **KePub** (`output/kepub.rs`): the same documents with a `.kepub.epub` extension
  (`--kepub-short-ext` trims it to `.kepub`) and `rendition:page-spread-*` properties (the
  `isKobo` branch of the spread-property algorithm).

IDs and timestamps come from crates (`uuid` v4 for `dc:identifier`/`dtb:uid`, `time` for
`dcterms:modified`), not hand-rolled code.

Line endings are pinned to `\n`: askama embeds `templates/` verbatim, so a CRLF checkout (Git for
Windows' `core.autocrlf`, or a Windows text-mode editor) would otherwise leak `\r\n` into the
documents. Every template is rendered through `output/epub/templates.rs::render_lf`, so the emitted
EPUB is byte-identical on Linux, macOS and Windows.

## CBZ / PDF / light-novel

- **CBZ** (`output/cbz.rs`) — writes the processed `EncodedPage` payloads (whose sanitized
  `kcc-NNNN-kcc-<order>` names and chapter directories are exactly what KCC left on disk)
  through `crate::archive::ArchiveWriter`, stored rather than deflated. A `##cover.jpg` is
  added only when the cover was smart-cropped or came from a sibling `Covers/` override, and
  `ComicInfo.xml` only when `--keep-comicinfo` retained it.
- **PDF** (`output/pdf.rs`) — one page per image at its pixel size. A JPEG whose header
  declares 8-bit gray or RGB is embedded verbatim via `DCTDecode`; PNG/GIF/WebP and exotic
  JPEGs are decoded and `FlateDecode`d. The cover is prepended under the same
  smart-crop/custom-cover condition. Document info carries the title and first author.
  `pdf-writer` supplies the catalog/page-tree/xref skeleton; content streams, XObject
  dictionaries and filters are ours.
- **Light-novel** (`output/lightnovel.rs`) — `--light-novel` skips most of the comic pipeline:
  it preserves the source structure, copies pages that already fit byte-for-byte, and
  grayscale-`contain`s only oversized ones into a CBZ. It bypasses `prepare_book`, so pages
  keep their original names and no cover is built. Deliberate deviations: only images are
  carried (no non-image entries other than `ComicInfo.xml`), and KCC's `RGBA → LA` becomes
  `RGBA → L`.

## Kindle (AZW3 / MOBI)

KCC's MOBI path = fixed-layout EPUB → `kindlegen` → `dualmetafix` EXTH patch. This port
replaces all of that with the MIT `kindling` crate — no subprocess and no sibling binary:

1. Build the same fixed-layout EPUB as `-f epub`.
2. Encode with `kindling`'s library API:
   - `azw3` → KF8-only `.azw3` (`kf8_only = true`; modern Kindles).
   - `mobi` → dual MOBI7+KF8 `.mobi` (legacy devices).
3. `--doc-type ebok|pdoc|none` maps to EXTH 501 (`none`, the default, omits the tag to avoid
   the firmware "back-to-library" issue). Cover/thumbnail are handled by `kindling`.

`mobi+epub` keeps the intermediate EPUB. `-f auto` on a Kindle profile resolves to `mobi` and
takes this path.

The builders do not round-trip through a zip: `epub::build_entries` returns the OEBPS entry
list, which `build_epub` writes as a zip and the Kindle path materialises into a `tempfile`
scratch directory (deleted on every exit) that `kindling` reads via `OEBPS/content.opf`.
Naming follows KCC: the intermediate EPUB name is resolved first (with the `_kcc<N>` collision
rule covering `Azw3`/`Mobi`), and the Kindle name is derived from it by replacing the
extension.

`kfx` is the EPUB preset for Calibre's KFX Output plugin (with `region-mag=false`); no KFX
encoder is needed. The KFX "most common input resolution" override is not implemented.

## Chunking and fusion

- **Chunking** (`chunk.rs`) repartitions a `ProcessedBook` into tomes in memory, moving every
  `EncodedPage` into exactly one tome. It reproduces KCC's level detection, its per-page (flat)
  vs per-chapter (one level) split, the `--batch-split 1` oversized-chapter flatten, and the
  mode ≥ 3 "every top-level directory its own tome" rule. A split book's tomes are titled
  `base [i/n]` (`[ii/nn]` at ten or more), named `base i.ext`, each with a fresh UUID, a
  labelled cover and dropped `ComicInfo.xml` bookmarks. Unlike KCC, an oversized first unit is
  its own tome rather than being preceded by an empty one.
- **Fusion** (`input/fusion.rs`) loads each source and merges them into one `ComicTree` — one
  chapter per source, each source's own directories flattened — adding a `fusion_NNNN_` order
  prefix only when the user's order differs from natural order, and taking a shared `Covers/`
  cover as the fused cover. The output directory defaults to the first source's directory and
  the name to `<first name> [fused]`; `--delete` leaves the sources alone.

Tome-file naming and titles for all formats follow `naming.rs::output_filename`; see
[cli.md](cli.md) for `--target-size`/`--batch-split` and the size-capped presets.
