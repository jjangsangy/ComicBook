use clap::Parser;
use comic_book::cli::{run, Cli};
use std::process::exit;

fn main() {
    let cli = Cli::parse();

    if let Err(err) = run(cli) {
        eprintln!("{err}");
        exit(1);
    }
}
