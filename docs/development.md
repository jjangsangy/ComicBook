# Development

## Changelog

Every user-visible change (behaviour, CLI surface, output, dependencies, docs) gets an entry
under the top `## [Unreleased]` section of [`CHANGELOG.md`](../CHANGELOG.md) in the same change,
using Keep a Changelog's headings (Added, Changed, Deprecated, Removed, Fixed, Security). The
release workflow dates that section into the tagged release
(see [Releasing](../CONTRIBUTING.md#releasing)), so anything left out of the changelog ships
undocumented.

## Testing & validation

- **Run tests with `cargo nextest`, not `cargo test`.** [cargo-nextest](https://nexte.st/)
  (`cargo install cargo-nextest --locked`) is the standard runner here: each test runs in its
  own process, failures are clearer, and parallelisation is better. There are no doctests, so
  nextest covers the whole suite; use `cargo test` only where nextest cannot run a target.
- **Always pipe `cargo nextest` through `tail`.** A full run prints hundreds of lines, and only
  whether the run passed and which cases failed matter — both are in the trailing summary. `2>&1`
  is required because nextest writes to stderr; without it `tail` sees nothing. `tail -n 20`
  captures the summary plus the failing cases; raise the count only when a run has more failures
  than fit.
- **Quality gates:** `cargo fmt` and `cargo clippy --all-targets --all-features -- -D warnings`
  must stay green.
- **Coverage:** `cargo-llvm-cov` runs the same suite under LLVM source-based coverage
  (`cargo install cargo-llvm-cov --locked`, plus `rustup component add llvm-tools-preview` for the
  `llvm-cov`/`llvm-profdata` tools). Run it with nextest as the test runner:
  `cargo llvm-cov nextest`, or `cargo llvm-cov nextest --open` to browse the report. CI runs the
  suite once on a single `ubuntu` job through the `ci` profile in
  [`.config/nextest.toml`](../.config/nextest.toml) — selected with `NEXTEST_PROFILE=ci`, since
  `--profile` is also a cargo-llvm-cov option and would not reach nextest — which also writes the
  JUnit report Codecov Test Analytics ingests. CI enforces a minimum line-coverage floor through
  Codecov and publishes the result and the graph to Codecov (which serves the README badge and
  icicle graph) and the job summary (see [CI](#ci)).
- **Unit tests** per algorithm on small synthetic images: colour-check decisions, fill
  detection, split classification, crop boxes, slugify, spread properties, filename logic,
  OPF/NCX/NAV output.
- **Fixture/golden tests:** commit small `CBZ`/`CBR`/`CB7`/`CBT` inputs and assert structure by
  parsing the output back (mimetype first + stored; OPF spine; XHTML image refs; image
  dimensions). `tests/ebook_golden_tests.rs` pins the generated EPUB documents (OPF/NCX/NAV/XHTML/
  CSS) with [`insta`](https://insta.rs/) snapshots in `tests/snapshots/`, one per scenario and
  document. Two `insta` filters normalise the only volatile fields — the `dc:identifier`/`dtb:uid`
  the `dcterms:modified` timestamp — so the snapshots are stable, and `insta` normalises
  the snapshot files' line endings to LF itself (the whole tree is LF-pinned, see
  [Cross-platform & packaging notes](#cross-platform--packaging-notes)), so a CRLF checkout does
  not matter. Because
  `insta` folds CRLF→LF *and* trims one trailing newline before comparing, `snapshot_payload`
  asserts the LF-only contract and appends an `<EOF: newline>`/`<EOF: no newline>` marker recording
  each document's exact tail; without that the LF-only and no-final-newline properties
  (see [output.md](output.md)) would go unchecked. Regenerate only for an intentional format change
  with `INSTA_UPDATE=always cargo nextest run --test ebook_golden_tests`, or review pending changes
  interactively with `cargo insta review` / `cargo insta test` (installed with
  `cargo install cargo-insta`; `.config/insta.yaml` makes it run the suite through nextest).
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
cargo nextest run --run-ignored ignored-only 2>&1 | tail -n 20

# One suite, its ignored tests included (no external tools needed)
cargo nextest run --run-ignored all --test ebook_webtoon_tests 2>&1 | tail -n 20
```

## Profiling

Optimise what the profile says is hot, not what looks slow. Generate a deterministic input and
trace a real conversion:

```bash
cargo run --release --example gen_bench -- target/bench/bench.cbz 400 1600 2400
```

A few hundred pages is enough that the process outlives the sampler; keep runs to a handful of
seconds so the CPU does not thermally throttle and distort the comparison. `scripts/bench.sh`
wraps the generation and timing (report `user`, the total CPU work).

On macOS, attach `sample` while a conversion runs and read its **"Sort by top of stack"** (the
leaf/self-time distribution) and **"Call graph"** (the call tree):

```bash
./target/release/comic-book ebook target/bench/bench.cbz -f epub -o target/bench/out &
sample <pid> 5 -file target/bench/sample.txt
python3 scripts/flamegraph.py target/bench/sample.txt target/bench/flame.svg
```

On Linux, `perf record -g` or `cargo flamegraph` give the same shape. `scripts/flamegraph.py`
demangles Rust symbols through `c++filt`.

The check that matters is that a change moves the hot path: the pre-`kernels.rs` trace is led by
`malloc`/`free` (from `imageproc::map::map_pixels`' per-pixel `Vec`), and the trace after the SIMD
and clone-removal pass is led by the JPEG codec and `fast_image_resize`, i.e. work that cannot be
removed.

### Memory

A change can lower CPU time and still raise memory, so measure both. The dev-only
`alloc_count` example (a counting global allocator) reports, for one conversion, the
number of allocation calls, the total bytes requested and the peak live heap:

```bash
cargo build --release --example alloc_count
./target/release/examples/alloc_count target/bench/bench.cbz target/bench/out --force-png
```

Any arguments after the output directory are forwarded to `ebook`, so every scenario can be
measured. `scripts/memory_bench.sh [baseline-worktree] [input.cbz]` runs the default EPUB,
`--force-png`, `--webtoon`, `-f pdf`, `--light-novel` and `--no-processing` runs against a
baseline git worktree (`target/bench/base3` is the checkout the clone-removal pass used), which
is the fastest way to answer "did this raise peak memory?" against a known-good tree.

Allocation count and requested bytes are stable, reproducible signals; peak live is noisier, so
run it a few times and read the direction. On the 40-page `gen_bench` input (1600x2400), the
SIMD + clone-removal pass moved every scenario the right way — before (`cf5b9be`) → after:

| scenario | allocations | allocated MiB | peak live MiB |
|:---|:---|---:|---:|
| `epub` (default) | 222,442,184 → 125,463 | 2,577.8 → 1,124.4 | 147 → 117 |
| `--force-png` | 222,445,501 → 128,781 | 3,217.1 → 1,763.7 | 197 → 167 |
| `--webtoon` | 464,198,522 → 144,574 | 5,827.5 → 3,058.8 | 1,302 → 1,046 |
| `-f pdf` | 222,440,548 → 123,788 | 2,683.2 → 1,222.0 | 160 → 131 |
| `--light-novel` | 15,459,373 → 99,257 | 604.2 → 425.7 | 110 → 91 |
| `--no-processing` | 7,685,619 → 5,573 | 102.1 → 67.8 | 47 → 36 |

The `ebook_robustness_tests` memory ceilings above pin the contract; the allocator example is
how a new clone or an accidental `to_owned` is caught before it reaches them.

## CI

The existing `ubuntu`/`macos`/`windows` matrix installs cargo-nextest
(`taiki-e/install-action@nextest`) and runs `cargo nextest run --no-fail-fast`, since nextest is
fail-fast by default: without the flag a single failure (e.g. on Windows) aborts the run and the
remaining tests never produce output. The release workflow builds static musl Linux binaries.

Coverage is a separate workflow ([`.github/workflows/coverage.yml`](../.github/workflows/coverage.yml)),
so its check and badge are independent of the main `CI` badge. It runs the suite once under
`cargo llvm-cov nextest --lcov` on `ubuntu-latest`, with `NEXTEST_PROFILE=ci` selecting the `ci`
profile from [`.config/nextest.toml`](../.config/nextest.toml) (every test runs even after a
failure, and the run writes the JUnit report at `target/nextest/ci/junit.xml`); that step fails only
on a test failure. The coverage *gate* is the `codecov/project` status configured in
[`codecov.yml`](../codecov.yml) (target `95%`, no threshold), which Codecov evaluates against the
uploaded `lcov.info`. Mark `codecov/project` as a required status check in branch protection to
block merges that would take coverage under the floor.

Keeping the floor in Codecov rather than passing a `cargo llvm-cov --fail-under-*` flag is
deliberate: Codecov renders the badge, so making it authoritative means the gate and the badge can
never disagree. Codecov scores *line* coverage from the lcov `DA:` records (~97.5%). llvm-cov
cannot produce that same figure itself — it reports a different line total in its summary/`LF`
header (~96.6%) than in its own lcov `DA` export, and its *region* coverage (~93.9%) has no lcov
record at all — so a `--fail-under-regions`/`--fail-under-lines` floor would gate on a number
Codecov never shows. Name the metric you mean rather than assuming the tools agree.

The result is published five ways: the `lcov.info` report is uploaded to Codecov (which serves the
badge and the icicle graph in the [README](../README.md), annotates pull requests and enforces
the gate), the `target/nextest/ci/junit.xml` report is uploaded separately with
`report_type: test_results` for Codecov Test Analytics (per-test timing and flakiness, plus the
failing-test annotation on a pull request), the per-file table is appended to the run's job summary,
a same-repo pull request gets a sticky comment (a PR from a fork has a read-only `GITHUB_TOKEN` and
is skipped, but still gets the job summary), and the `lcov.info` data, the JUnit report, an HTML
report and the summary markdown are uploaded as the `coverage` artifact.
The Codecov upload is authenticated with a `CODECOV_TOKEN` repository secret — required because a
public repo still needs auth on a protected branch like `main` — so install the Codecov GitHub App
for the repo on first use. Even when a test fails the summary and report are still produced, because
`cargo llvm-cov report` re-renders the previous run's profile data rather than rerunning the tests.
The job is deliberately not part of the OS matrix — instrumentation is much slower and coverage
does not vary meaningfully between platforms.

On a `v*` tag, that release workflow validates the tag as SemVer, stamps `Cargo.toml`'s
`[package] version` from it (and refreshes the root `Cargo.lock` entry so the `--locked` build
succeeds), so every release reports its own version from `comic-book --version`. Once the build
matrix succeeds it commits the same stamp to the default branch, so `main` always declares the
last released version.

## Cross-platform & packaging notes

- All behaviour must compile on Linux, macOS and Windows. Prefer pure-Rust crates; `unrar` and
  `webp` vendor/compile their C sources (acceptable — no external program).
- Line endings are LF everywhere: `.gitattributes` sets `* text=auto eol=lf`, so git normalises
  text files to LF on commit and checks them out as LF, matching the LF-pinned generated documents
  ([output.md](output.md)). `text=auto` still detects binary fixtures and leaves them untouched.
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
