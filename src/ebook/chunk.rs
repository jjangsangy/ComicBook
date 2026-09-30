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
use relative_path::Component;

use crate::ebook::model::{ChapterName, EncodedPage, PageName, ScribeHalf};
use crate::ebook::options::{BatchSplit, MainOptions, Options, ProcessingOptions};
use crate::ebook::processing::cover::{self, Cover};
use crate::ebook::processing::{ProcessedBook, ProcessedChapter};
use crate::units::Bytes;

/// KCC's default cap when neither `--target-size` nor webtoon mode applies (400 MB).
const DEFAULT_TARGET_SIZE: Bytes = Bytes::new(419_430_400);
/// KCC's webtoon cap when no `--target-size` is given (100 MB).
const WEBTOON_TARGET_SIZE: Bytes = Bytes::new(104_857_600);

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

    assemble(book.cover, tome_chapters, &options.processing)
}

/// The number of path segments of an encoded page (`Images/<name>` → its components).
fn depth(page: &EncodedPage) -> usize {
    page.name.as_relative().components().count()
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
            page.name = PageName::new(page.name.as_relative().file_name().unwrap_or(""));
            pages.push(page);
        }
    }
    book.chapters = if pages.is_empty() {
        Vec::new()
    } else {
        vec![ProcessedChapter {
            name: ChapterName::root(),
            pages,
        }]
    };
}

/// The size a webtoon/target-size run splits against (`chunk_process`).
fn target_size(options: &MainOptions) -> Bytes {
    match options.target_size {
        Some(megabytes) => megabytes.to_bytes(),
        None if options.webtoon => WEBTOON_TARGET_SIZE,
        None => DEFAULT_TARGET_SIZE,
    }
}

/// Whether any single chapter is larger than the cap (`chunk_process`'s
/// `--batch-split 1` pre-check).
fn chapters_exceed_target(book: &ProcessedBook, target: Bytes) -> bool {
    book.chapters
        .iter()
        .filter(|chapter| !chapter.pages.is_empty())
        .any(|chapter| chapter_size(chapter) > target)
}

/// The on-disk size of a chapter's pages (`getDirectorySize`).
fn chapter_size(chapter: &ProcessedChapter) -> Bytes {
    chapter
        .pages
        .iter()
        .map(|page| Bytes::new(page.bytes.len() as u64))
        .sum()
}

/// Split a flat tree's pages by size, keeping Scribe `-above`/`-below` pairs
/// together.
fn split_pages(chapters: Vec<ProcessedChapter>, target: Bytes) -> Vec<Vec<ProcessedChapter>> {
    let name = chapters
        .iter()
        .find(|chapter| !chapter.pages.is_empty())
        .map(|chapter| chapter.name.clone())
        .unwrap_or_else(ChapterName::root);
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
        // A Scribe `-above` page carries its immediately-following `-below`
        // companion; an unpaired `-above` and every other half is its own unit.
        let below = match page.flags.half {
            ScribeHalf::Above => pages.next_if(|next| next.flags.half == ScribeHalf::Below),
            ScribeHalf::Below | ScribeHalf::NotSplit => None,
        };
        match below {
            Some(below) => units.push(vec![page, below]),
            None => units.push(vec![page]),
        }
    }
    units
}

/// Pack page units into tomes no larger than `target`.
fn pack_units(units: Vec<Vec<EncodedPage>>, target: Bytes) -> Vec<Vec<EncodedPage>> {
    let mut tomes: Vec<Vec<EncodedPage>> = Vec::new();
    let mut current: Vec<EncodedPage> = Vec::new();
    let mut current_size = Bytes::ZERO;
    for unit in units {
        let size: Bytes = unit
            .iter()
            .map(|page| Bytes::new(page.bytes.len() as u64))
            .sum();
        if !current.is_empty() && current_size + size > target {
            tomes.push(std::mem::take(&mut current));
            current_size = Bytes::ZERO;
        }
        current_size = current_size + size;
        current.extend(unit);
    }
    if !current.is_empty() || tomes.is_empty() {
        tomes.push(current);
    }
    tomes
}

/// Split a one-level tree by whole chapters, packed under `target`.
fn split_chapters(chapters: Vec<ProcessedChapter>, target: Bytes) -> Vec<Vec<ProcessedChapter>> {
    let mut tomes: Vec<Vec<ProcessedChapter>> = Vec::new();
    let mut current: Vec<ProcessedChapter> = Vec::new();
    let mut current_size = Bytes::ZERO;
    for chapter in chapters
        .into_iter()
        .filter(|chapter| !chapter.pages.is_empty())
    {
        let size = chapter_size(&chapter);
        if !current.is_empty() && current_size + size > target {
            tomes.push(std::mem::take(&mut current));
            current_size = Bytes::ZERO;
        }
        current_size = current_size + size;
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
        let top = match chapter.name.as_relative().components().next() {
            Some(Component::Normal(segment)) => segment,
            _ => "",
        };
        if current.as_deref() != Some(top) {
            tomes.push(Vec::new());
            current = Some(top.to_string());
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
    cover: Option<Cover>,
    tome_chapters: Vec<Vec<ProcessedChapter>>,
    options: &ProcessingOptions,
) -> Result<Vec<ProcessedBook>> {
    let total = tome_chapters.len();
    let mut cover = cover;
    let mut tomes = Vec::with_capacity(total);
    for (index, chapters) in tome_chapters.into_iter().enumerate() {
        let page_count = chapters.iter().map(|chapter| chapter.pages.len()).sum();
        // Every tome of a split book gets the label (KCC increments `tomeid`
        // before saving); a single tome keeps its cover and moves it through.
        let cover = if total > 1 {
            match &cover {
                Some(existing) => Some(Cover {
                    page: cover::labelled(&existing.page, index + 1, total, options.jpeg_quality)?,
                    smart_cropped: existing.smart_cropped,
                }),
                None => None,
            }
        } else {
            cover.take()
        };
        tomes.push(ProcessedBook {
            chapters,
            cover,
            page_count,
        });
    }
    Ok(tomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ebook::model::{
        ChapterName, MediaType, OrderClass, PageFlags, PageName, ScribeHalf,
    };
    use crate::units::Size;

    fn page(name: &str, len: usize) -> EncodedPage {
        EncodedPage {
            name: PageName::new(name),
            order_class: OrderClass::Normal,
            media_type: MediaType::Jpeg,
            bytes: vec![0; len],
            size: Size::new(1, 1),
            flags: PageFlags::default(),
        }
    }

    fn chapter(name: &str, pages: Vec<EncodedPage>) -> ProcessedChapter {
        ProcessedChapter {
            name: ChapterName::new(name),
            pages,
        }
    }

    #[test]
    fn page_units_keep_a_scribe_pair_together() {
        let mut above = page("a-above.jpg", 10);
        above.flags.half = ScribeHalf::Above;
        let mut below = page("a-below.jpg", 20);
        below.flags.half = ScribeHalf::Below;
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
        let tomes = pack_units(units, Bytes::new(100));
        assert_eq!(tomes.len(), 2);
        assert_eq!(tomes[0].len(), 1);
        assert_eq!(tomes[1].len(), 2);
    }

    #[test]
    fn an_oversized_unit_is_its_own_tome_without_an_empty_leading_tome() {
        let units = vec![vec![page("big.jpg", 500)], vec![page("small.jpg", 10)]];
        let tomes = pack_units(units, Bytes::new(100));
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
            page_count: 2,
        };
        flatten(&mut book);
        assert_eq!(book.chapters.len(), 1);
        assert!(book.chapters[0].name.is_root());
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
            page_count: 2,
        };
        assert!(image_level(&book).1);
    }

    /// Resolve options from a `comic-book ebook` command line.
    fn options(args: &[&str]) -> Result<Options> {
        use anyhow::bail;
        use clap::Parser;
        let mut full = vec!["comic-book", "ebook", "book.cbz"];
        full.extend_from_slice(args);
        let cli = crate::cli::Cli::try_parse_from(full)?;
        match cli.command {
            crate::cli::Commands::Ebook(args) => Options::resolve(&args),
            _ => bail!("expected the ebook subcommand"),
        }
    }

    fn book(chapters: Vec<ProcessedChapter>) -> ProcessedBook {
        let page_count = chapters.iter().map(|chapter| chapter.pages.len()).sum();
        ProcessedBook {
            chapters,
            cover: None,
            page_count,
        }
    }

    #[test]
    fn a_book_with_no_pages_is_kept_as_one_tome() -> Result<()> {
        let tomes = split(
            book(vec![chapter("Chapter", vec![])]),
            &options(&["--target-size", "1"])?,
        )?;
        // The early return keeps the tree as-is; splitting it would have flattened the
        // (already empty) chapter to a root chapter named "" instead.
        assert_eq!(tomes.len(), 1);
        assert_eq!(tomes[0].chapters.len(), 1);
        assert!(!tomes[0].chapters[0].name.is_root());
        Ok(())
    }

    #[test]
    fn a_two_level_tree_splits_by_chapter_under_the_cap() -> Result<()> {
        // Two 0.6 MB chapters under a 1 MB cap: neither exceeds on its own, so the
        // tree splits as whole chapters, one per tome.
        let tomes = split(
            book(vec![
                chapter("Chapter 1", vec![page("Chapter 1/a.jpg", 600_000)]),
                chapter("Chapter 2", vec![page("Chapter 2/b.jpg", 600_000)]),
            ]),
            &options(&["--target-size", "1"])?,
        )?;
        assert_eq!(tomes.len(), 2);
        assert_eq!(tomes[0].chapters.len(), 1);
        assert_eq!(tomes[1].chapters.len(), 1);
        Ok(())
    }

    #[test]
    fn an_oversized_chapter_flattens_an_auto_split() -> Result<()> {
        // Chapter 1 is 1.2 MB across three 0.4 MB pages: over the 1 MB cap as a whole,
        // but each page fits. Auto splitting flattens and repacks the pages, so the cap
        // is honoured (2 pages, then 2) instead of emitting the 1.2 MB chapter whole.
        let tomes = split(
            book(vec![
                chapter(
                    "Chapter 1",
                    vec![
                        page("Chapter 1/a.jpg", 400_000),
                        page("Chapter 1/b.jpg", 400_000),
                        page("Chapter 1/c.jpg", 400_000),
                    ],
                ),
                chapter("Chapter 2", vec![page("Chapter 2/d.jpg", 600_000)]),
            ]),
            &options(&["--target-size", "1"])?,
        )?;
        assert_eq!(tomes.len(), 2);
        assert_eq!(tomes[0].page_count, 2, "the oversized chapter is repacked");
        assert_eq!(tomes[1].page_count, 2);
        assert!(tomes.iter().all(|tome| tome.chapters.len() == 1));
        Ok(())
    }

    #[test]
    fn flattening_a_book_with_no_pages_empties_its_chapters() {
        let mut book = book(vec![chapter("Chapter", vec![])]);
        flatten(&mut book);
        assert!(book.chapters.is_empty());
    }

    #[test]
    fn webtoon_without_a_target_uses_the_webtoon_cap() -> Result<()> {
        let webtoon = options(&["--webtoon"])?;
        assert_eq!(target_size(&webtoon.main), WEBTOON_TARGET_SIZE);
        let plain = options(&[])?.main;
        assert_eq!(target_size(&plain), DEFAULT_TARGET_SIZE);
        Ok(())
    }

    #[test]
    fn chapter_size_sums_page_bytes_and_can_exceed_the_target() {
        let book = book(vec![chapter("Chapter", vec![page("a.jpg", 20)])]);
        assert_eq!(chapter_size(&book.chapters[0]), Bytes::new(20));
        assert!(chapters_exceed_target(&book, Bytes::new(10)));
        assert!(!chapters_exceed_target(&book, Bytes::new(30)));
    }

    #[test]
    fn per_top_level_handles_a_root_chapter_and_an_empty_book() {
        // A root-level chapter has no first path component.
        let tomes = per_top_level(vec![chapter("", vec![page("a.jpg", 1)])]);
        assert_eq!(tomes.len(), 1);
        assert_eq!(tomes[0].len(), 1);

        let empty = per_top_level(Vec::new());
        assert_eq!(empty.len(), 1);
        assert!(empty[0].is_empty());
    }

    #[test]
    fn a_split_book_without_a_cover_has_no_labelled_cover() -> Result<()> {
        let options = options(&[])?;
        let tomes = assemble(
            None,
            vec![
                vec![chapter("A", vec![page("A/a.jpg", 1)])],
                vec![chapter("B", vec![page("B/b.jpg", 1)])],
            ],
            &options.processing,
        )?;
        assert_eq!(tomes.len(), 2);
        assert!(tomes.iter().all(|tome| tome.cover.is_none()));
        assert_eq!(tomes[0].page_count, 1);
        assert_eq!(tomes[1].page_count, 1);
        Ok(())
    }

    /// A cover whose page holds a valid (decodable) JPEG, as `cover::process` produces.
    fn decodable_cover(options: &Options) -> Result<Cover> {
        use image::{Rgb, RgbImage};
        let image =
            image::DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 300, Rgb([255, 255, 255])));
        let bytes =
            crate::ebook::processing::page::encode_jpeg(&image, options.processing.jpeg_quality)?;
        Ok(Cover {
            page: EncodedPage {
                name: PageName::new("cover.jpg"),
                order_class: OrderClass::Normal,
                media_type: MediaType::Jpeg,
                bytes,
                size: Size::new(200, 300),
                flags: PageFlags::default(),
            },
            smart_cropped: false,
        })
    }

    #[test]
    fn every_tome_of_a_split_book_gets_its_own_cover_label() -> Result<()> {
        let options = options(&[])?;
        let tomes = assemble(
            Some(decodable_cover(&options)?),
            vec![
                vec![chapter("A", vec![page("A/a.jpg", 1)])],
                vec![chapter("B", vec![page("B/b.jpg", 1)])],
            ],
            &options.processing,
        )?;
        let bytes: Vec<Vec<u8>> = tomes
            .iter()
            .filter_map(|tome| tome.cover.as_ref().map(|cover| cover.page.bytes.clone()))
            .collect();
        assert_eq!(bytes.len(), 2, "every tome keeps a labelled cover");
        assert!(!bytes[0].is_empty());
        assert_ne!(bytes[0], bytes[1], "each tome gets its own N/M label");
        Ok(())
    }
}
