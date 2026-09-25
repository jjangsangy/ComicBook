//! `toc.ncx` and `nav.xhtml` generation (AGENTS.md §12.2).
//!
//! Both documents are a flat list of chapter entries — one per chapter directory,
//! or one per `ComicInfo.xml` bookmark when the book carries them. The NCX uses
//! `<navPoint>`/`<content>`, the NAV carries the same list twice (a `toc` and a
//! `page-list`), matching KCC's output exactly.

use std::collections::HashMap;

use super::{html_escape, text_dir, PageRef};

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
            .get(entry.file)
            .map(String::as_str)
            .unwrap_or(book_title);
    }
    let folder = text_dir(entry.image_dir);
    let basename = folder.rsplit('/').next().unwrap_or(folder.as_str());
    if basename != "Text" {
        if let Some(title) = chapter_titles.get(basename) {
            return title;
        }
    }
    book_title
}

/// The XHTML source path (relative to `OEBPS`) for an entry's first page.
fn source_path(entry: &PageRef<'_>) -> String {
    format!("{}/{}.xhtml", text_dir(entry.image_dir), entry.stem)
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
) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<ncx version=\"2005-1\" xml:lang=\"{language}\" xmlns=\"http://www.daisy.org/z3986/2005/ncx/\">\n"
    ));
    out.push_str("<head>\n");
    out.push_str(&format!(
        "<meta name=\"dtb:uid\" content=\"urn:uuid:{uuid}\"/>\n"
    ));
    out.push_str("<meta name=\"dtb:depth\" content=\"1\"/>\n");
    out.push_str("<meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
    out.push_str("<meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
    out.push_str("<meta name=\"generated\" content=\"true\"/>\n");
    out.push_str("</head>\n");
    out.push_str(&format!(
        "<docTitle><text>{}</text></docTitle>\n",
        html_escape(title)
    ));
    out.push_str("<navMap>\n");

    for &index in entries {
        let Some(entry) = filelist.get(index) else {
            continue;
        };
        let folder = text_dir(entry.image_dir);
        let source = source_path(entry);
        let nav_id = if page_titles.is_empty() {
            folder.replace('/', "_")
        } else {
            source.replace('/', "_")
        };
        let entry_title = entry_title(entry, title, chapter_titles, page_titles);
        out.push_str(&format!(
            "<navPoint id=\"{nav_id}\"><navLabel><text>{}</text></navLabel><content src=\"{source}\"/></navPoint>\n",
            html_escape(entry_title)
        ));
    }

    out.push_str("</navMap>\n</ncx>");
    out
}

/// Build `OEBPS/nav.xhtml`.
pub(crate) fn build_nav(
    title: &str,
    entries: &[usize],
    filelist: &[PageRef<'_>],
    chapter_titles: &HashMap<String, String>,
    page_titles: &HashMap<String, String>,
) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<!DOCTYPE html>\n");
    out.push_str(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n",
    );
    out.push_str("<head>\n");
    out.push_str(&format!("<title>{}</title>\n", html_escape(title)));
    out.push_str("<meta charset=\"utf-8\"/>\n");
    out.push_str("</head>\n");
    out.push_str("<body>\n");

    let list = |out: &mut String| {
        for &index in entries {
            let Some(entry) = filelist.get(index) else {
                continue;
            };
            let entry_title = entry_title(entry, title, chapter_titles, page_titles);
            out.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>\n",
                source_path(entry),
                html_escape(entry_title)
            ));
        }
    };

    // Both the table of contents and the page list carry the same entries, as the
    // reference writes them.
    out.push_str(
        "<nav xmlns:epub=\"http://www.idpf.org/2007/ops\" epub:type=\"toc\" id=\"toc\">\n",
    );
    out.push_str("<ol>\n");
    list(&mut out);
    out.push_str("</ol>\n");
    out.push_str("</nav>\n");
    out.push_str("<nav epub:type=\"page-list\">\n");
    out.push_str("<ol>\n");
    list(&mut out);
    out.push_str("</ol>\n");
    out.push_str("</nav>\n");
    out.push_str("</body>\n");
    out.push_str("</html>");
    out
}
