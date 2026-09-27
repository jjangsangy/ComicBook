# Contributing to comic-book

Thank you for your interest in contributing to `comic-book`! We welcome contributions, bug reports, and feature suggestions from everyone.

---

## Code of Conduct

Please help maintain a welcoming, inclusive, and respectful environment for all contributors. Be polite, collaborative, and constructive in all discussions, issues, and code reviews.

---

## How Can I Contribute?

### Reporting Bugs

Before creating a bug report, please check existing GitHub issues to verify that the bug hasn't already been reported.

If you find a new bug, please open an issue using the **Bug Report** template and include:
1. **Description**: Clear description of what happened versus what you expected.
2. **Steps to Reproduce**: Minimal steps or sample files demonstrating the issue.
3. **Environment**: Operating system, Rust version (`rustc --version`), and `comic-book` version.
4. **Logs or Output**: Terminal output or error messages.

### Suggesting Enhancements

Feature requests and performance improvement ideas are welcome! Open an issue using the **Feature Request** template, explaining:
- The problem or use-case you are facing.
- The proposed solution or behavior.
- Any alternative approaches you've considered.

### Submitting Pull Requests

1. **Fork and Clone**:
   ```bash
   git clone https://github.com/jjangsangy/ComicBook.git
   cd ComicBook
   ```

2. **Create a Feature Branch**:
   ```bash
   git checkout -b feature/my-new-feature
   ```

3. **Make Your Changes**:
   - Keep changes focused and minimal.
   - Follow existing code idioms and project conventions.
   - Add unit or integration tests for new functionality in `tests/integration_tests.rs`.
   - Record any user-visible change under the top `## [Unreleased]` section of
     [`CHANGELOG.md`](CHANGELOG.md) (Keep a Changelog headings: Added, Changed, Deprecated,
     Removed, Fixed, Security) in the same PR.

4. **Verify Your Code Locally**:
   Run the test suite and quality checks before submitting. Tests run with
   [cargo-nextest](https://nexte.st/) (install once with
   `cargo install cargo-nextest --locked`):
   ```bash
   # Run all tests
   cargo nextest run

   # Check formatting
   cargo fmt --check

   # Run clippy linter
   cargo clippy --all-targets --all-features
   ```

5. **Commit and Push**:
   Write clear, concise commit messages:
   ```bash
   git add .
   git commit -m "feat(clamp): add new image format support"
   git push origin feature/my-new-feature
   ```

6. **Open a Pull Request**:
   Fill out the Pull Request template describing the changes, tests performed, and any related issue numbers.

---

## Development Setup

- **Language**: Rust (2021 edition)
- **Minimum Supported Rust Version (MSRV)**: 1.87+
- **External Dependencies**: None (all formats, including RAR reading and writing, use native Rust libraries)

---

## Releasing

Releases are automated. The pushed tag is the source of truth for the version, so publishing
a new version is just a matter of tagging and pushing it:

```bash
git tag v0.2.4
git push origin v0.2.4
```

Bumping the version on `main` first is optional: the workflow stamps the tag's version and
commits it to the default branch for you.

Pushing a `v*` tag triggers the [`Release` workflow](.github/workflows/release.yml), which:

1. Validates the tag as a version Cargo accepts (failing before anything is published otherwise).
2. Stamps `Cargo.toml`'s `[package] version` from the tag (`scripts/set-version.sh` /
   `scripts/set-version.ps1`) and refreshes the matching `Cargo.lock` entry before building, so
   the compiled binaries report the released tag from `comic-book --version`.
3. Creates a GitHub Release whose notes are the changelog for the release's whole major.minor line
   (`scripts/changelog-notes.sh`) — the tag's own section plus every older `0.Y.x` entry — followed
   by GitHub's auto-generated notes. Tags containing a `-` (e.g. `v0.2.0-rc.1`) are published as pre-releases.
4. Builds and attaches binaries for macOS (arm64, x86_64), Linux (x86_64, aarch64; both static musl and glibc) and Windows (x86_64), each with a `.sha256` checksum.
5. Commits the version stamp back to the default branch (`chore(release): vX.Y.Z`), so `main`
   declares the released version instead of drifting until the next manual bump. The same commit
   rolls the changelog's top `## [Unreleased]` section over to the released version (dating it and
   updating its compare links); only this default-branch step passes `--changelog`, so the build
   jobs above never touch `CHANGELOG.md`.

The version-bump commit is pushed with the workflow's `GITHUB_TOKEN`, so it does **not**
re-trigger CI and it requires the token to be allowed to push to the default branch — branch
protection that demands pull requests or status checks will reject it, in which case switch that
last step to open a pull request instead.

The scripts in `scripts/` resolve the latest release automatically, so nothing else needs updating.
