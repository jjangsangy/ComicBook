# Development

## Testing & validation

- **Run tests with `cargo nextest`, not `cargo test`.** [cargo-nextest](https://nexte.st/)
  (`cargo install cargo-nextest --locked`) is the standard runner here: each test runs in its
  own process, failures are clearer, and parallelisation is better. There are no doctests, so
  nextest covers the whole suite; use `cargo test` only where nextest cannot run a target.
- **Quality gates:** `cargo fmt` and `cargo clippy --all-targets --all-features -D warnings`
  must stay green.
- **Unit tests** per algorithm on small synthetic images: colour-check decisions, fill
  detection, split classification, crop boxes, slugify, spread properties, filename logic,
  OPF/NCX/NAV output.
- **Fixture/golden tests:** commit small `CBZ`/`CBR`/`CB7`/`CBT` inputs and assert structure by
  parsing the output back (mimetype first + stored; OPF spine; XHTML image refs; image
  dimensions). `tests/ebook_golden_tests.rs` compares generated EPUB documents byte-for-byte
  against committed references (only the UUID and `dcterms:modified` are normalised); regenerate
  with `UPDATE_GOLDEN=1 cargo nextest run --test ebook_golden_tests` only for an intentional
  format change.
- **Reference values from KCC.** Some fixtures (`tests/fixtures/crop/`, the webtoon virtual-page
  sizes) pin values produced by KCC itself; the inputs and returned values are committed, so the
  tests assert exact equality instead of a tolerance.
- **Round-trip tests:** CBZ→CBZ and EPUB→input→EPUB where applicable.
- **EPUB conformance:** an opt-in `epubcheck` job (JVM) is marked `#[ignore]` by default.
- **AZW3/MOBI:** structural readback via `kindling::mobi_dump` (see [output.md](output.md)).
- **Progress bars are silenced in tests** via the `tests/common/mod.rs` pattern / `COMIC_BOOK_QUIET`.

Test suites: `ebook_tests` (CLI/options/profiles), `ebook_input_tests`,
`ebook_input_epub_pdf_tests`, `ebook_processing_tests`, `ebook_crop_tests`,
`ebook_naming_tests`, `ebook_epub_tests`, `ebook_golden_tests`, `ebook_output_tests`,
`ebook_kindle_tests`, `ebook_chunk_tests`, `ebook_webtoon_tests`, `ebook_robustness_tests`.

## CI

The existing `ubuntu`/`macos`/`windows` matrix installs cargo-nextest
(`taiki-e/install-action@nextest`) and runs `cargo nextest run`; the release workflow builds
static musl Linux binaries.

## Cross-platform & packaging notes

- All behaviour must compile on Linux, macOS and Windows. Prefer pure-Rust crates; `unrar` and
  `webp` vendor/compile their C sources (acceptable — no external program).
- Keep paths OS-agnostic with `std::path`. Archive entries authored on Windows load identically
  everywhere; hostile entries (`..`, absolute, drive-lettered, backslashes) are normalised to
  interior, forward-slashed paths.
- Chapter/page names are slugified to ASCII with none of Windows' reserved characters
  (`<>:"/\|?*`). The one remaining edge — an output file named after a Windows reserved device
  (`CON.cbz` → `CON.mobi`) — is shared with KCC and left as-is, because changing
  `getOutputFilename` would break format parity.
- Keep the `--completions` generator working with the `ebook` subcommand.

## Risks

| Risk | Impact | Mitigation |
|:---|:---|:---|
| `kindling` API does not fit our EPUB | AZW3 blocked | verified by the Kindle structural tests; it is an accepted dependency |
| Fidelity drift in OPF/spread logic | device breakage | port the algorithm exactly; assert with tests; no casual "improvements" |
| Cropping/panel algorithms differ subtly | visible artifacts | fixture-based box/panel assertions tuned to KCC output |
| Memory blow-up on huge books | OOM | streaming pipeline; memory regression test (peak RSS is linear in the decoded book) |
| New dependencies hurt musl/Windows builds | build/release | prefer pure Rust; validate in the CI matrix before adopting |

## Glossary

- **CBZ/CBR/CB7/CBT** — ZIP/RAR/7-Zip/TAR comic archives.
- **KF8 / AZW3** — Kindle Format 8; `.azw3` is KF8-only.
- **KEPUB** — Kobo's EPUB dialect (`.kepub.epub`).
- **Panel View** — Kindle tap-to-zoom regions (`PV-*` divs + `app-amzn-magnify`).
- **Tome** — one output file when a source is split by size (`--target-size`/`--batch-split`).
- **Fusion** — combining multiple inputs into one book (`--file-fusion`).

## References

- KCC upstream: <https://github.com/ciromattia/kcc>.
- `kindling` (MIT), the native MOBI/AZW3 builder: <https://github.com/ciscoriordan/kindling>.
- [cargo-nextest](https://nexte.st/) — the test runner used here.
- MobileRead MOBI wiki — format background.
