//! Target-size / batch-split tome keeper.
//!
//! KCC's `chunk_directory`/`chunk_process`/`createNewTome` move already-processed
//! page files between temp directories to keep each output under a size cap (or to
//! split per subdirectory). This port does the same on the in-memory
//! [`ProcessedBook`]: [`split`] repartitions the pages into tomes, moving each
//! `EncodedPage` into exactly one tome so memory stays flat (see docs/architecture.md), and
//! labels each tome's cover with its `N/M` number (`Cover.save_to_folder`).
//!
//! The reference's level detection decides the split unit: a flat `Images` tree
//! splits on individual pages, one level of chapter directories splits on whole
//! chapters, and a deeper (or `--batch-split 2`) tree gives every top-level
//! directory its own tome. A tree whose pages sit at mixed depths — or a
//! `--batch-split 1` tree with an oversized chapter — is flattened first, exactly
//! as `flattenTree` does.
//!
//! One deliberate deviation (see docs/porting.md): KCC creates an empty leading
//! tome when the very first unit already exceeds the cap; the port never emits an
//! empty tome, so a single oversized unit simply becomes its own tome.

use anyhow::Result;

use crate::ebook::model::EncodedPage;
use crate::ebook::options::{BatchSplit, MainOptions, Options, ProcessingOptions};
use crate::ebook::processing::cover;
use crate::ebook::processing::{ProcessedBook, ProcessedChapter};

/// KCC's default cap when neither `--target-size` nor webtoon mode applies (400 MB).
const DEFAULT_TARGET_SIZE: u64 = 419_430_400;
/// KCC's webtoon cap when no `--target-size` is given (100 MB).
const WEBTOON_TARGET_SIZE: u64 = 104_857_600;
const MEGABYTE: u64 = 1_048_576;

/// Split a processed book into output tomes.
///
/// Returns the book as a single tome when chunking was not requested, and always
/// at least one tome.
pub fn split(mut book: ProcessedBook, options: &Options) -> Result<Vec<ProcessedBook>> {
    if options.output.batch_split == BatchSplit::None && options.main.target_size.is_none() {
        return Ok(vec![book]);
    }
    if book.chapters.iter().all(|chapter| chapter.pages.is_empty()) {
        return Ok(vec![book]);
    }

    // The common nesting depth of every page (`chunk_directory`'s `level`); mixed
    // depths flatten the tree and split on pages.
    let (mut level, mixed) = image_level(&book);
    if mixed {
        flatten(&mut book);
        level = 1;
    }

    let target = target_size(&options.main);
    let mut mode = level;
    if options.output.batch_split == BatchSplit::PerSubdirectory && mode == 2 {
        mode = 3;
    }
    if options.output.batch_split == BatchSplit::Auto
        && mode == 2
        && chapters_exceed_target(&book, target)
    {
        // A chapter that is itself over the cap cannot be split as a whole.
        flatten(&mut book);
        mode = 1;
    }

    let chapters = std::mem::take(&mut book.chapters);
    let tome_chapters = if mode >= 3 {
        per_top_level(chapters)
    } else if mode == 1 {
        split_pages(chapters, target)
    } else {
        split_chapters(chapters, target)
    };

    assemble(
        book.cover,
        book.cover_smart_crop,
        tome_chapters,
        &options.processing,
    )
}

/// The number of path segments of an encoded page (`Images/<name>` → `split('/')`).
fn depth(page: &EncodedPage) -> usize {
    page.name.split('/').count()
}

/// KCC's `level`: the shared page depth, or `(_, true)` when depths differ.
fn image_level(book: &ProcessedBook) -> (usize, bool) {
    let mut level = 1;
    let mut seen = false;
    let mut mixed = false;
    for chapter in &book.chapters {
        for page in &chapter.pages {
            let page_depth = depth(page);
            if !seen {
                level = page_depth;
                seen = true;
            } else if page_depth != level {
                mixed = true;
            }
        }
    }
    (level, mixed)
}

/// KCC's `flattenTree`: move every page into one root chapter, dropping its
/// directory.
fn flatten(book: &mut ProcessedBook) {
    let mut pages = Vec::new();
    for chapter in &mut book.chapters {
        for mut page in chapter.pages.drain(..) {
            page.name = basename(&page.name).to_string();
            pages.push(page);
        }
    }
    book.chapters = if pages.is_empty() {
        Vec::new()
    } else {
        vec![ProcessedChapter {
            name: String::new(),
            pages,
        }]
    };
}

/// The size a webtoon/target-size run splits against (`chunk_process`).
fn target_size(options: &MainOptions) -> u64 {
    match options.target_size {
        Some(megabytes) => u64::from(megabytes) * MEGABYTE,
        None if options.webtoon => WEBTOON_TARGET_SIZE,
        None => DEFAULT_TARGET_SIZE,
    }
}

/// Whether any single chapter is larger than the cap (`chunk_process`'s
/// `--batch-split 1` pre-check).
fn chapters_exceed_target(book: &ProcessedBook, target: u64) -> bool {
    book.chapters
        .iter()
        .filter(|chapter| !chapter.pages.is_empty())
        .any(|chapter| chapter_size(chapter) > target)
}

/// The on-disk size of a chapter's pages (`getDirectorySize`).
fn chapter_size(chapter: &ProcessedChapter) -> u64 {
    chapter
        .pages
        .iter()
        .map(|page| page.bytes.len() as u64)
        .sum()
}

/// Split a flat tree's pages by size, keeping Scribe `-above`/`-below` pairs
/// together.
fn split_pages(chapters: Vec<ProcessedChapter>, target: u64) -> Vec<Vec<ProcessedChapter>> {
    let name = chapters
        .iter()
        .find(|chapter| !chapter.pages.is_empty())
        .map(|chapter| chapter.name.clone())
        .unwrap_or_default();
    let pages = chapters.into_iter().flat_map(|chapter| chapter.pages);
    pack_units(page_units(pages), target)
        .into_iter()
        .map(|pages| {
            vec![ProcessedChapter {
                name: name.clone(),
                pages,
            }]
        })
        .collect()
}

/// Group pages into unsplittable units: a Scribe `-above` page carries its
/// `-below` companion with it, everything else is one page.
fn page_units(pages: impl IntoIterator<Item = EncodedPage>) -> Vec<Vec<EncodedPage>> {
    let mut units = Vec::new();
    let mut pages = pages.into_iter().peekable();
    while let Some(page) = pages.next() {
        let below = if page.flags.above && pages.peek().is_some_and(|next| next.flags.below) {
            pages.next()
        } else {
            None
        };
        match below {
            Some(below) => units.push(vec![page, below]),
            None => units.push(vec![page]),
        }
    }
    units
}

/// Pack page units into tomes no larger than `target`.
fn pack_units(units: Vec<Vec<EncodedPage>>, target: u64) -> Vec<Vec<EncodedPage>> {
    let mut tomes: Vec<Vec<EncodedPage>> = Vec::new();
    let mut current: Vec<EncodedPage> = Vec::new();
    let mut current_size = 0u64;
    for unit in units {
        let size: u64 = unit.iter().map(|page| page.bytes.len() as u64).sum();
        if !current.is_empty() && current_size + size > target {
            tomes.push(std::mem::take(&mut current));
            current_size = 0;
        }
        current_size += size;
        current.extend(unit);
    }
    if !current.is_empty() || tomes.is_empty() {
        tomes.push(current);
    }
    tomes
}

/// Split a one-level tree by whole chapters, packed under `target`.
fn split_chapters(chapters: Vec<ProcessedChapter>, target: u64) -> Vec<Vec<ProcessedChapter>> {
    let mut tomes: Vec<Vec<ProcessedChapter>> = Vec::new();
    let mut current: Vec<ProcessedChapter> = Vec::new();
    let mut current_size = 0u64;
    for chapter in chapters
        .into_iter()
        .filter(|chapter| !chapter.pages.is_empty())
    {
        let size = chapter_size(&chapter);
        if !current.is_empty() && current_size + size > target {
            tomes.push(std::mem::take(&mut current));
            current_size = 0;
        }
        current_size += size;
        current.push(chapter);
    }
    if !current.is_empty() || tomes.is_empty() {
        tomes.push(current);
    }
    tomes
}

/// Give every top-level directory its own tome (`chunk_process`'s `mode >= 3`).
fn per_top_level(chapters: Vec<ProcessedChapter>) -> Vec<Vec<ProcessedChapter>> {
    let mut tomes: Vec<Vec<ProcessedChapter>> = Vec::new();
    let mut current: Option<String> = None;
    for chapter in chapters
        .into_iter()
        .filter(|chapter| !chapter.pages.is_empty())
    {
        let top = chapter.name.split('/').next().unwrap_or("").to_string();
        if current.as_deref() != Some(top.as_str()) {
            tomes.push(Vec::new());
            current = Some(top);
        }
        if let Some(tome) = tomes.last_mut() {
            tome.push(chapter);
        }
    }
    if tomes.is_empty() {
        tomes.push(Vec::new());
    }
    tomes
}

/// Turn the per-tome chapter lists into [`ProcessedBook`]s, labelling each cover.
fn assemble(
    cover: Option<EncodedPage>,
    cover_smart_crop: bool,
    tome_chapters: Vec<Vec<ProcessedChapter>>,
    options: &ProcessingOptions,
) -> Result<Vec<ProcessedBook>> {
    let total = tome_chapters.len();
    let mut tomes = Vec::with_capacity(total);
    for (index, chapters) in tome_chapters.into_iter().enumerate() {
        let page_count = chapters.iter().map(|chapter| chapter.pages.len()).sum();
        // Every tome of a split book gets the label (KCC increments `tomeid`
        // before saving); a single tome keeps its cover untouched.
        let cover = match &cover {
            Some(page) if total > 1 => Some(cover::labelled(
                page,
                index + 1,
                total,
                options.jpeg_quality,
            )?),
            Some(page) => Some(page.clone()),
            None => None,
        };
        tomes.push(ProcessedBook {
            chapters,
            cover,
            cover_smart_crop,
            page_count,
        });
    }
    Ok(tomes)
}

/// The final path component (`os.path.basename`).
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::model::{MediaType, OrderClass, PageFlags};

    fn page(name: &str, len: usize) -> EncodedPage {
        EncodedPage {
            name: name.to_string(),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            bytes: vec![0; len],
            width: 1,
            height: 1,
            flags: PageFlags::default(),
        }
    }

    fn chapter(name: &str, pages: Vec<EncodedPage>) -> ProcessedChapter {
        ProcessedChapter {
            name: name.to_string(),
            pages,
        }
    }

    #[test]
    fn page_units_keep_a_scribe_pair_together() {
        let mut above = page("a-above.jpg", 10);
        above.flags.above = true;
        let mut below = page("a-below.jpg", 20);
        below.flags.below = true;
        let normal = page("b.jpg", 30);

        let units = page_units(vec![above, below, normal]);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].len(), 2, "the above/below halves are one unit");
        assert_eq!(units[1].len(), 1);
    }

    #[test]
    fn packing_splits_on_the_size_boundary() {
        let units = vec![
            vec![page("1.jpg", 60)],
            vec![page("2.jpg", 60)],
            vec![page("3.jpg", 30)],
        ];
        // 60 then 60+60 > 100, so the second starts a new tome.
        let tomes = pack_units(units, 100);
        assert_eq!(tomes.len(), 2);
        assert_eq!(tomes[0].len(), 1);
        assert_eq!(tomes[1].len(), 2);
    }

    #[test]
    fn an_oversized_unit_is_its_own_tome_without_an_empty_leading_tome() {
        let units = vec![vec![page("big.jpg", 500)], vec![page("small.jpg", 10)]];
        let tomes = pack_units(units, 100);
        assert_eq!(
            tomes.len(),
            2,
            "no empty tome is emitted (see docs/porting.md)"
        );
        assert_eq!(tomes[0][0].name, "big.jpg");
        assert_eq!(tomes[1][0].name, "small.jpg");
    }

    #[test]
    fn flatten_moves_pages_to_the_root_and_drops_directories() {
        let mut book = ProcessedBook {
            chapters: vec![
                chapter("Chapter 1", vec![page("Chapter 1/a.jpg", 1)]),
                chapter("Chapter 1/Sub", vec![page("Chapter 1/Sub/b.jpg", 1)]),
            ],
            cover: None,
            cover_smart_crop: false,
            page_count: 2,
        };
        flatten(&mut book);
        assert_eq!(book.chapters.len(), 1);
        assert_eq!(book.chapters[0].name, "");
        let names: Vec<&str> = book.chapters[0]
            .pages
            .iter()
            .map(|page| page.name.as_str())
            .collect();
        assert_eq!(names, vec!["a.jpg", "b.jpg"]);
    }

    #[test]
    fn a_mixed_depth_tree_reports_mixed_levels() {
        let book = ProcessedBook {
            chapters: vec![
                chapter("", vec![page("a.jpg", 1)]),
                chapter("Chapter 1", vec![page("Chapter 1/b.jpg", 1)]),
            ],
            cover: None,
            cover_smart_crop: false,
            page_count: 2,
        };
        assert!(image_level(&book).1);
    }
}
