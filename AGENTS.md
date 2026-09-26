# AGENTS.md

`comic-book` is a Rust CLI for converting comic archives. Its `convert` and `clamp`
subcommands re-package and resize archives; its `ebook` subcommand is a self-contained,
cross-platform reimplementation of Kindle Comic Converter's `kcc-c2e` comic→ebook pipeline.
The design and specifications live in [`docs/`](docs/):

| Document | Contents |
|:---|:---|
| [docs/README.md](docs/README.md) | Overview, commands, `ebook` goals/scope, definition of done |
| [docs/convert.md](docs/convert.md) | `convert`: formats, directory expansion, output naming, mechanics |
| [docs/clamp.md](docs/clamp.md) | `clamp`: approaches, thresholds, output layout |
| [docs/architecture.md](docs/architecture.md) | `ebook` pipeline, module map, data model, design principles, performance |
| [docs/cli.md](docs/cli.md) | `ebook` options, formats, device profiles |
| [docs/processing.md](docs/processing.md) | `ebook` image-processing algorithms and fidelity rules |
| [docs/output.md](docs/output.md) | `ebook` EPUB/KePub/CBZ/PDF/light-novel/Kindle document specs, chunking, fusion |
| [docs/dependencies.md](docs/dependencies.md) | Off-the-shelf policy, crates, licences, clean-room rules |
| [docs/porting.md](docs/porting.md) | Porting history and the decisions/deviations behind the `ebook` code |
| [docs/development.md](docs/development.md) | Testing, CI, cross-platform notes, glossary, references |

## Build, test and lint

```bash
cargo build --release
cargo fmt
cargo clippy --all-targets --all-features -D warnings
cargo nextest run          # standard test runner; use `cargo test` only where nextest can't
```

## Hard rules

- **Clean-room.** Do not copy code, comments or identifiers from KCC's GPL-3 sources into this
  repository. Reimplement behaviour from its description and pin it with a test. MOBI/AZW3
  encoding is delegated to the MIT `kindling` crate. See [docs/dependencies.md](docs/dependencies.md).
- **No external programs.** Nothing may shell out to `7z`, `unrar`, `kindlegen`, ImageMagick,
  etc.; everything is compiled into the binary (pure-Rust crates, or crates that vendor their C
  sources).
- **Prefer off-the-shelf crates.** Reach for an existing maintained crate before writing a
  bespoke algorithm; justify any hand-rolled code against the policy in
  [docs/dependencies.md](docs/dependencies.md).
- **Output fidelity.** The emitted EPUB/OPF/NCX/NAV/XHTML and MOBI documents are
  device-sensitive: preserve their structure and semantics, and pin any intentional deviation
  with a test (see [docs/output.md](docs/output.md) and [docs/porting.md](docs/porting.md)).

When a comment needs to cite a design rationale, reference the relevant file under `docs/`.
