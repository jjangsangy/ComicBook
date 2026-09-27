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
  against committed references (the UUID and `dcterms:modified` are normalised, and the references'
  line endings are normalised to LF so a CRLF checkout does not matter — the generated documents
  themselves are pinned to LF, see [output.md](output.md)); regenerate with
  `UPDATE_GOLDEN=1 cargo nextest run --test ebook_golden_tests` only for an intentional format
  change.
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

## Long-running tests

The image-processing tests run the real pipeline over full-size pages, which is slow in the
unoptimised build CI uses. The slowest were profiled with
`RAYON_NUM_THREADS=1 cargo nextest run -j 4` (four tests at a time, one rayon thread each, to
approximate a four-core runner) and are marked `#[ignore]`, so the default `cargo nextest run` —
and therefore CI — skips them.

| Test | Suite | Time |
|:---|:---|---:|
| `a_large_book_converts_under_a_memory_ceiling` | `ebook_robustness_tests` | 26.0s |
| `scribe_profile_splits_a_tall_page_into_above_and_below` | `ebook_epub_tests` | 16.5s |
| `two_panel_and_vertical_4_panel_reshape_the_panel_view` | `ebook_epub_tests` | 7.9s |
| `test_clamp_single_file_input` | `integration_tests` | 7.3s |
| `smart_cover_crop_takes_a_single_side_and_the_cover_is_fitted` | `ebook_epub_tests` | 7.3s |
| `a_tall_scribe_page_splits_at_1920_into_above_and_below` | `ebook::processing::page` | 6.9s |
| `light_novel_preserves_structure_and_only_resizes_oversized_pages` | `ebook_output_tests` | 6.3s |
| `wider_devices_use_the_1072_cap_for_the_virtual_height` | `ebook::processing::webtoon` | 5.6s |
| `a_super_long_panel_splits_with_overlap` | `ebook::processing::webtoon` | 5.5s |
| `a_custom_cover_is_kept_in_webtoon_mode` | `ebook_webtoon_tests` | 5.1s |

The memory guards in `ebook_robustness_tests` are also `#[ignore]`:

| Test | What it pins |
|:---|:---|
| `ingest_and_repack_stay_far_below_the_decoded_book_size` | A 256-page `--no-processing` run peaks far below the decoded book (quick: one encode plus a byte copy per page). |
| `a_large_book_converts_under_a_memory_ceiling` | A full 128-page conversion under a generous ceiling (the original hardening guard). |
| `huge_book_stress` | The same for 600 pages; prints peak RSS for manual profiling. |

The lazy-decode contract itself is pinned by the cheap, always-on
`ingest_defers_page_decoding` (`ebook_input_tests`) and `processing_releases_decoded_pixels`
(`ebook_processing_tests`) tests.

Run them explicitly with `--run-ignored`:

```bash
# Every ignored test: the long-running set, the stress test, and the
# `epubcheck` conformance test (which needs `epubcheck` on PATH)
cargo nextest run --run-ignored ignored-only

# One suite, its ignored tests included (no external tools needed)
cargo nextest run --run-ignored all --test ebook_webtoon_tests
```

## CI

The existing `ubuntu`/`macos`/`windows` matrix installs cargo-nextest
(`taiki-e/install-action@nextest`) and runs `cargo nextest run --no-fail-fast`, since nextest is
fail-fast by default: without the flag a single failure (e.g. on Windows) aborts the run and the
remaining tests never produce output. The release workflow builds static musl Linux binaries.

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
| Memory blow-up on huge books | OOM | lazy decode (peak RSS is linear in the *encoded* book); memory regression tests |
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
