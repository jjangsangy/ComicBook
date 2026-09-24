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
   git clone https://github.com/<your-username>/comic-book.git
   cd comic-book
   ```

2. **Create a Feature Branch**:
   ```bash
   git checkout -b feature/my-new-feature
   ```

3. **Make Your Changes**:
   - Keep changes focused and minimal.
   - Follow existing code idioms and project conventions.
   - Add unit or integration tests for new functionality in `tests/integration_tests.rs`.

4. **Verify Your Code Locally**:
   Run the test suite and quality checks before submitting:
   ```bash
   # Run all tests
   cargo test

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
