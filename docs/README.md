# comic-book — documentation

`comic-book` is a Rust CLI for working with comic-book archives. Everything — archive
extraction and creation, image processing, EPUB/PDF/MOBI building and PDF rasterisation — is
compiled into the binary; no `7z`, `unrar`, `kindlegen`, ImageMagick or other external program
is required, and every command works offline on Linux, macOS and Windows.

## Commands

| Command | Purpose | Documentation |
|:---|:---|:---|
| `convert` | Re-package archives/folders between `cbz`/`cbr`/`cb7`/`cbt` and plain directories | [convert.md](convert.md) |
| `clamp` | Rewrite oversized pages so every image is under a size limit | [clamp.md](clamp.md) |
| `ebook` | Convert comics into e-book formats (EPUB/KePub/AZW3/MOBI/PDF/CBZ) | this folder |
| `completions` | Generate shell completion scripts | `comic-book completions --help` |

Most of this documentation covers `ebook`, the largest command. It is an independent Rust
reimplementation of [Kindle Comic Converter](https://github.com/ciromattia/kcc)'s `kcc-c2e`
CLI; KCC's behaviour is used as a reference only — see [dependencies.md](dependencies.md) for
the clean-room and licensing rules.

## `ebook` goals

- **Inputs:** `.cbz`/`.zip`, `.cbr`/`.rar`, `.cb7`/`.7z`, `.cbt`/`.tar`, image folders, and
  (secondary) `.epub`/`.pdf`.
- **Outputs:** `epub`, `kepub` (Kobo `.kepub.epub`), `azw3`/`mobi` (Kindle), plus `cbz`,
  `pdf`, and the `kfx`-as-EPUB preset.
- **No external programs**, no GUI, and no Kindle-device detection/upload.
- Works offline on Linux, macOS and Windows.

Where Rust allows a materially better memory/CPU/I-O approach, we take it; where the emitted
documents are device-sensitive, we reproduce them faithfully. Both rules are detailed in
[architecture.md](architecture.md).

## `ebook` scope

**In scope:** the `kcc-c2e` pipeline and the modules it uses (orchestration, archive handling,
image processing, colour/crop/moiré/webtoon algorithms, metadata, naming, all output builders),
every `kcc-c2e` CLI option, and all the output formats above.

**Out of scope:** the Qt GUI (`kcc.py`, `KCC_gui.py`, `KCC_ui*.py`, `KCC_rc.py`,
`KCC_spread_label.py`), `startup.py`'s GUI/dependency checks, Kindle device mounting and
thumbnail upload (`kindle.py` device paths), anything requiring `kindlegen`/`7-Zip`/`unar`/
`unrar`/PyMuPDF/Pillow/numpy/Qt at runtime, and `kcc-c2p.py` as a standalone command (its
`comic2panel` logic is folded into `--webtoon`).

## Documentation map

| File | Contents |
|:---|:---|
| [convert.md](convert.md) | `convert`: formats, directory expansion, output naming, mechanics |
| [clamp.md](clamp.md) | `clamp`: approaches, thresholds, output layout |
| [architecture.md](architecture.md) | `ebook` pipeline, module map, data model, design principles, performance |
| [refactor.md](refactor.md) | Type-safety refactor: the completed `make impossible states unrepresentable` pass |
| [cli.md](cli.md) | `ebook` options, formats and device profiles |
| [processing.md](processing.md) | `ebook` image-processing algorithms and fidelity rules |
| [output.md](output.md) | `ebook` EPUB/KePub/CBZ/PDF/light-novel/Kindle document specs, chunking, fusion |
| [dependencies.md](dependencies.md) | Off-the-shelf policy, crate list, licences, clean-room rules |
| [porting.md](porting.md) | Porting history and the decisions/deviations behind the `ebook` code |
| [development.md](development.md) | Testing, CI, cross-platform notes, risks, glossary, references |
| [proptest.md](proptest.md) | Property-based tests: what is covered, candidate properties, findings |

## Definition of done

- `comic-book ebook` converts `.cbz`, `.cbr`, `.cb7`, `.cbt`, folders (and `.epub`/`.pdf`)
  into `epub`, `kepub`, `azw3`, `mobi` (plus `cbz`, `pdf`, `kfx`).
- No external program is invoked or required.
- Every option in [cli.md](cli.md) is implemented with KCC-equivalent semantics.
- New behaviour reuses a maintained crate where one fits; any hand-rolled component is
  justified against the policy in [dependencies.md](dependencies.md).
- Tests, clippy and fmt pass on CI.
