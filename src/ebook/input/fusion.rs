//! `--file-fusion`: combine multiple inputs into a single book (Phase 9).
//!
//! KCC's `makeFusion` extracts every source into a subdirectory of one temp tree
//! (adding a `fusion_NNNN_` prefix when the user's order differs from natural
//! order), flattens each source's own directory structure, and then converts the
//! combined tree with an optional shared `Covers/` cover. This port builds the
//! combined [`ComicTree`] directly in memory — one chapter per source, pages in
//! their original order — so no temp tree is written (AGENTS.md §5.1), and the
//! caller converts it like any other book.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::ebook::model::{Chapter, ComicTree};
use crate::ebook::naming;
use crate::ebook::options::Options;

use super::load_tree;

/// A fused source, ready to be converted as one book.
#[derive(Debug)]
pub struct Fused {
    /// The combined tree: one chapter per input.
    pub tree: ComicTree,
    /// A shared `Covers/` cover, if the first input's directory has one.
    pub cover: Option<PathBuf>,
    /// The synthetic source path KCC converts (`<first name> [fused]`), used for
    /// the default title and the output file name.
    pub source: PathBuf,
    /// The resolved default title (`<first name> [fused]`).
    pub title: String,
    /// The directory outputs land in by default: the first input's parent.
    pub output_dir: PathBuf,
}

/// Merge the inputs into a single [`ComicTree`] (`makeFusion`).
pub fn build(sources: &[PathBuf], options: &Options) -> Result<Fused> {
    if sources.len() < 2 {
        bail!("Fusion requires at least 2 sources. Did you forget to uncheck fusion?");
    }

    let names: Vec<String> = sources.iter().map(|source| source_name(source)).collect();
    // KCC prefixes the fused directories with their index whenever the inputs are
    // not already in natural order, so the user's order survives `sanitizeTree`.
    let needs_prefix = {
        let mut sorted = names.clone();
        sorted.sort_by(|a, b| natord::compare_ignore_case(a, b));
        sorted != names
    };

    let mut chapters = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let tree = load_tree(source, options)?;
        let prefix = if needs_prefix {
            format!("fusion_{:04}_", index + 1)
        } else {
            String::new()
        };
        // Every source becomes one chapter: the reference flattens each source's
        // own subdirectories (`flattenTree`) before the combined tree is walked.
        let pages = tree
            .chapters
            .into_iter()
            .flat_map(|chapter| chapter.pages)
            .collect();
        chapters.push(Chapter {
            name: format!("{prefix}{}", names[index]),
            pages,
        });
    }

    let parent = sources[0]
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let title = format!("{} [fused]", names[0]);
    let source = parent.join(&title);
    let cover = naming::first_cover(&parent);

    Ok(Fused {
        tree: ComicTree {
            chapters,
            cover: None,
            comicinfo: None,
        },
        cover,
        source,
        title,
        output_dir: parent,
    })
}

/// KCC's `Path(s).stem if Path(s).is_file() else Path(s).name`.
fn source_name(source: &Path) -> String {
    let name = if source.is_dir() {
        source.file_name()
    } else {
        source.file_stem()
    };
    name.map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn a_file_source_uses_its_stem_and_a_folder_its_name() {
        assert_eq!(source_name(Path::new("/x/Vol.1.cbz")), "Vol.1");
        assert_eq!(source_name(Path::new("/x/Book")), "Book");
    }

    #[test]
    fn fusion_needs_at_least_two_sources() {
        let cli = crate::cli::Cli::try_parse_from(["comic-book", "ebook", "book.cbz"])
            .expect("CLI parses");
        let options = match cli.command {
            crate::cli::Commands::Ebook(args) => {
                crate::ebook::options::Options::resolve(&args).expect("resolves")
            }
            _ => unreachable!(),
        };
        let error = build(&[PathBuf::from("only.cbz")], &options)
            .unwrap_err()
            .to_string();
        assert!(error.contains("at least 2"), "unexpected error: {error}");
    }
}
