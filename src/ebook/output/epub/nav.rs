//! `toc.ncx` and `nav.xhtml` generation (see docs/output.md).
//!
//! Both documents are a flat list of chapter entries — one per chapter directory,
//! or one per `ComicInfo.xml` bookmark when the book carries them. The NCX uses
//! `<navPoint>`/`<content>`, the NAV carries the same list twice (a `toc` and a
//! `page-list`), matching KCC's output exactly.
//!
//! The document skeletons live in `templates/toc.ncx` and `templates/nav.xhtml`;
//! this module computes the navigation entries they render (see docs/dependencies.md).

use std::collections::HashMap;

use anyhow::Result;
use relative_path::RelativePath;

use super::html_escape;
use super::templates::{render_lf, Href, Nav, NavEntry, NavId, NavTitle, Ncx};
use super::{text_dir, PageRef};

/// Title for a chapter entry: a bookmark title, the chapter's original basename,
/// or the book title for the implicit root chapter.
fn entry_title<'a>(
    entry: &PageRef<'_>,
    book_title: &'a str,
    chapter_titles: &'a HashMap<String, String>,
    page_titles: &'a HashMap<String, String>,
) -> &'a str {
    if !page_titles.is_empty() {
        return page_titles
            .get(entry.file.as_str())
            .map(String::as_str)
            .unwrap_or(book_title);
    }
    let folder = text_dir(entry.image_dir);
    let basename = RelativePath::new(&folder).file_name().unwrap_or("");
    if basename != "Text" {
        if let Some(title) = chapter_titles.get(basename) {
            return title;
        }
    }
    book_title
}

/// The navigation targets shared by the NCX and NAV documents.
fn nav_entries(
    entries: &[usize],
    filelist: &[PageRef<'_>],
    title: &str,
    chapter_titles: &HashMap<String, String>,
    page_titles: &HashMap<String, String>,
) -> Vec<NavEntry> {
    let mut out = Vec::with_capacity(entries.len());
    for &index in entries {
        let Some(entry) = filelist.get(index) else {
            continue;
        };
        let folder = text_dir(entry.image_dir);
        let source = Href::xhtml(&folder, entry.stem());
        let id = if page_titles.is_empty() {
            NavId::folded(&folder)
        } else {
            NavId::folded(source.as_str())
        };
        let entry_title = entry_title(entry, title, chapter_titles, page_titles);
        out.push(NavEntry {
            id,
            title: NavTitle::new(entry_title),
            source,
        });
    }
    out
}

/// Build `OEBPS/toc.ncx`.
pub(crate) fn build_ncx(
    title: &str,
    entries: &[usize],
    filelist: &[PageRef<'_>],
    chapter_titles: &HashMap<String, String>,
    page_titles: &HashMap<String, String>,
    language: &str,
    uuid: &str,
) -> Result<String> {
    let navpoints = nav_entries(entries, filelist, title, chapter_titles, page_titles);
    let escaped_title = html_escape(title);
    let view = Ncx {
        title: &escaped_title,
        language,
        uuid,
        navpoints: &navpoints,
    };
    render_lf(&view)
}

/// Build `OEBPS/nav.xhtml`.
pub(crate) fn build_nav(
    title: &str,
    entries: &[usize],
    filelist: &[PageRef<'_>],
    chapter_titles: &HashMap<String, String>,
    page_titles: &HashMap<String, String>,
) -> Result<String> {
    let entries = nav_entries(entries, filelist, title, chapter_titles, page_titles);
    let escaped_title = html_escape(title);
    let view = Nav {
        title: &escaped_title,
        entries: &entries,
    };
    render_lf(&view)
}
