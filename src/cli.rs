use anyhow::{anyhow, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::Shell;
use std::path::PathBuf;

use crate::clamp::{self, Approach};
use crate::convert;

#[derive(Parser, Debug)]
#[command(
    name = "comic-book",
    version,
    about = "Comic book archive conversion and image clamping tool",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Convert different archive formats (cbr, cbz, etc..)
    Convert {
        /// Files or directories to convert
        #[arg(required = true, value_name = "PATHS", value_hint = ValueHint::AnyPath)]
        directories: Vec<PathBuf>,

        /// Target format or extension (e.g. cbz, cbr, cb7, cbt, zip, rar, 7z, tar, dir)
        #[arg(long = "to", value_parser = ["cbz", "zip", "cbr", "rar", "cb7", "7z", "cbt", "tar", "dir"])]
        to: String,
    },

    /// Clamp image sizes in comic archives to all be under a size threshold
    Clamp {
        /// Input directory or file to process
        #[arg(required = true, value_hint = ValueHint::AnyPath)]
        input_dir: PathBuf,

        /// Output directory to store results in
        #[arg(short = 'o', long = "output-dir", default_value = "Results", value_hint = ValueHint::DirPath)]
        output_dir: PathBuf,

        /// Maximum size (in total pixels for split/resize, or max width for max-width)
        #[arg(short = 's', long = "size-threshold", default_value = "5000000")]
        size_threshold: u64,

        /// Approach to enforce size threshold: split, resize, or max-width
        #[arg(short = 'a', long = "approach", value_enum, default_value = "split")]
        approach: Approach,

        /// Number of worker threads to use for parallel processing
        #[arg(short = 'w', long = "workers", default_value_t = num_cpus())]
        workers: usize,
    },

    /// Generate shell completion scripts
    #[command(alias = "completion")]
    Completions {
        /// Shell to generate completions for (auto-detected from $SHELL if omitted)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Shell to generate completions for
        #[arg(
            short = 's',
            long = "shell",
            value_enum,
            value_name = "SHELL",
            conflicts_with = "shell"
        )]
        shell_flag: Option<Shell>,
    },
}

pub fn generate_completions<W: std::io::Write>(shell: Shell, buf: &mut W) {
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, bin_name, buf);
}

pub fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

pub fn run_completions(shell: Option<Shell>, shell_flag: Option<Shell>) -> Result<()> {
    let selected_shell = shell
        .or(shell_flag)
        .or_else(Shell::from_env)
        .ok_or_else(|| {
            anyhow!(
                "Could not determine shell from environment. Please specify a shell: bash, elvish, fish, powershell, zsh"
            )
        })?;

    generate_completions(selected_shell, &mut std::io::stdout());
    Ok(())
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Convert { directories, to } => convert::run_convert(&directories, &to),
        Commands::Clamp {
            input_dir,
            output_dir,
            size_threshold,
            approach,
            workers,
        } => clamp::run_clamp(&input_dir, &output_dir, size_threshold, approach, workers),
        Commands::Completions { shell, shell_flag } => run_completions(shell, shell_flag),
    }
}
