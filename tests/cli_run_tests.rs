//! Coverage for the `cli::run` dispatch, `run_completions`, and the `comic-book`
//! binary's `main`: each subcommand is driven through `run` once, and the binary is
//! exercised as a subprocess so `main`'s success and failure exits are covered.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use clap::Parser;
use clap_complete::Shell;
use comic_book::archive::{compress_archive, ArchiveKind};
use comic_book::cli::{generate_completions, run, run_completions, Cli};
use image::{DynamicImage, Rgb, RgbImage};
use tempfile::tempdir;

/// Suppress interactive progress for the rest of this test process.
fn quiet() {
    std::env::set_var(comic_book::ebook::progress::QUIET_ENV, "1");
}

fn parse(args: &[&str]) -> Result<Cli> {
    Ok(Cli::try_parse_from(args)?)
}

/// Write a solid-colour PNG, creating parent directories.
fn write_png(path: &Path, width: u32, height: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([20, 20, 20]))).save(path)?;
    Ok(())
}

#[test]
fn run_dispatches_convert() -> Result<()> {
    quiet();
    let tmp = tempdir()?;
    let source = tmp.path().join("book");
    write_png(&source.join("01.png"), 40, 60)?;

    let cli = parse(&[
        "comic-book",
        "convert",
        source.to_str().context("utf8 path")?,
        "--to",
        "cbz",
    ])?;
    run(cli)?;
    assert!(tmp.path().join("book.cbz").is_file());
    Ok(())
}

#[test]
fn run_dispatches_clamp() -> Result<()> {
    quiet();
    let tmp = tempdir()?;
    let raw = tmp.path().join("raw");
    write_png(&raw.join("big.png"), 2000, 2000)?;
    let input = tmp.path().join("input");
    fs::create_dir_all(&input)?;
    compress_archive(ArchiveKind::Cbz, &raw, input.join("chap.cbz"))?;
    let output = tmp.path().join("output");

    let cli = parse(&[
        "comic-book",
        "clamp",
        input.to_str().context("utf8 path")?,
        "-o",
        output.to_str().context("utf8 path")?,
        "-s",
        "1000000",
        "-a",
        "resize",
        "-w",
        "1",
    ])?;
    run(cli)?;
    assert!(output.join("chap").is_dir());
    Ok(())
}

#[test]
fn run_dispatches_ebook() -> Result<()> {
    quiet();
    let tmp = tempdir()?;
    let raw = tmp.path().join("raw");
    write_png(&raw.join("01.png"), 40, 60)?;
    let cbz = tmp.path().join("book.cbz");
    compress_archive(ArchiveKind::Cbz, &raw, &cbz)?;

    let cli = parse(&[
        "comic-book",
        "ebook",
        cbz.to_str().context("utf8 path")?,
        "-f",
        "epub",
        "-p",
        "KoE",
        "--no-kepub",
    ])?;
    run(cli)?;
    assert!(tmp.path().join("book.epub").is_file());
    Ok(())
}

#[test]
fn generate_completions_emits_a_shell_specific_script() -> Result<()> {
    // The public writer-taking entry point, so the output can be asserted directly.
    let mut bash = Vec::new();
    generate_completions(Shell::Bash, &mut bash);
    let mut zsh = Vec::new();
    generate_completions(Shell::Zsh, &mut zsh);
    let bash = String::from_utf8(bash)?;
    let zsh = String::from_utf8(zsh)?;
    assert!(
        bash.contains("complete"),
        "bash uses the `complete` builtin"
    );
    assert!(
        zsh.starts_with("#compdef"),
        "the zsh script declares its command"
    );
    assert!(zsh.contains("comic-book"));
    assert_ne!(bash, zsh, "each shell gets its own script");
    Ok(())
}

#[test]
fn run_completions_selects_from_the_positional_flag_or_env() -> Result<()> {
    // A positional shell, the `--shell` flag, and `$SHELL` all select a script; the
    // binary is driven as a subprocess so the stdout can be inspected. (Positional and
    // `--shell` cannot both be given: clap declares them in conflict.)
    let bin = env!("CARGO_BIN_EXE_comic-book");

    let positional = Command::new(bin).args(["completions", "bash"]).output()?;
    assert!(positional.status.success());
    assert!(String::from_utf8(positional.stdout)?.contains("complete"));

    let flag = Command::new(bin)
        .args(["completions", "--shell", "zsh"])
        .output()?;
    assert!(flag.status.success());
    assert!(String::from_utf8(flag.stdout)?.starts_with("#compdef"));

    let env = Command::new(bin)
        .args(["completions"])
        .env("SHELL", "/usr/bin/fish")
        .output()?;
    assert!(env.status.success());
    assert!(
        String::from_utf8(env.stdout)?.contains("complete -c"),
        "a fish script was generated from $SHELL"
    );
    Ok(())
}

#[test]
fn run_completions_without_a_detectable_shell_errors() -> Result<()> {
    // `SHELL` must be set to a value clap cannot map to a shell rather than unset: on
    // Windows an unset `SHELL` still resolves to PowerShell (`Shell::from_env` falls back
    // to it), so only an unrecognised value is undetectable on every platform.
    std::env::set_var("SHELL", "not-a-shell");
    let error = run_completions(None, None)
        .err()
        .context("an undetectable shell should error")?;
    assert!(error.to_string().contains("Could not determine shell"));
    Ok(())
}

#[test]
fn main_binary_exits_zero_on_success() -> Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_comic-book"))
        .args(["completions", "--shell", "bash"])
        .output()?;
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("comic-book"));
    Ok(())
}

#[test]
fn main_binary_exits_one_on_failure() -> Result<()> {
    let tmp = tempdir()?;
    let dir = tmp.path().to_str().context("utf8 path")?;
    // Clamping a directory into itself is rejected, so `run` returns an error.
    let output = Command::new(env!("CARGO_BIN_EXE_comic-book"))
        .args(["clamp", dir, "-o", dir])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
    Ok(())
}
