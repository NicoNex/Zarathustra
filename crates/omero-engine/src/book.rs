//! An open EPUB laid out chapter by chapter.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::dom::{Dom, NodeKind};
use crate::epub::{resolve, Epub, EpubError};
use crate::fonts::FontLibrary;
use crate::layout::flow::{self, Block};
use crate::layout::page::{self, Page};
use crate::layout::paragraph::Typesetter;
use crate::style::cascade::{compute, Preferences, Stylesheets};

pub struct Book {
    epub: Epub<BufReader<File>>,
    /// Archive paths of the linear XHTML chapters, in reading order.
    pub chapters: Vec<String>,
}

pub struct Chapter {
    pub blocks: Vec<Block>,
    pub pages: Vec<Page>,
}

impl Book {
    pub fn open(path: &Path) -> Result<Book, EpubError> {
        let epub = Epub::open(BufReader::new(File::open(path)?))?;
        let chapters =
            (epub.spine.iter()).filter(|s| s.linear && s.media_type.contains("html")).map(|s| s.path.clone()).collect();
        Ok(Book { epub, chapters })
    }

    pub fn title(&self) -> Option<&str> {
        self.epub.metadata.title.as_deref()
    }

    /// Lays out chapter `i` into pages of `height` pixels; `prefs.page_width` is the column width.
    pub fn chapter(
        &mut self,
        i: usize,
        prefs: &Preferences,
        fonts: &FontLibrary,
        ts: &mut Typesetter,
        height: f32,
    ) -> Result<Chapter, EpubError> {
        let path = self.chapters.get(i).ok_or(EpubError::Missing("chapter"))?.clone();
        let dom = self.epub.read_dom(&path)?;
        let sheets = self.stylesheets(&dom, &path);
        let styles = compute(&dom, &sheets, prefs);
        let lang = self.epub.metadata.language.clone();
        let blocks = flow::blocks(&dom, &styles, lang.as_deref());
        let pages = page::layout(&blocks, &styles, fonts, ts, height);
        Ok(Chapter { blocks, pages })
    }

    /// The chapter's `<link rel=stylesheet>` and `<style>` sheets, in document order.
    // ponytail: linked sheets are re-read and re-parsed per chapter; cache by path if it shows up.
    fn stylesheets(&mut self, dom: &Dom, chapter: &str) -> Stylesheets {
        let mut sheets = Stylesheets::default();
        for id in dom.descendants(Dom::ROOT) {
            match dom.name(id) {
                Some("link")
                    if dom.attr(id, "rel").is_some_and(|r| r.split_whitespace().any(|t| t == "stylesheet")) =>
                {
                    let Some(href) = dom.attr(id, "href") else { continue };
                    let path = resolve(chapter, href);
                    if let Ok(bytes) = self.epub.read(&path) {
                        sheets.add_author(&String::from_utf8_lossy(&bytes));
                    }
                }
                Some("style") => {
                    let css: String = (dom.children(id))
                        .filter_map(|c| match dom.kind(c) {
                            NodeKind::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    sheets.add_author(&css);
                }
                _ => {}
            }
        }
        sheets
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::hyphen::Hyphenators;

    #[test]
    fn lays_out_a_real_book() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let Ok(mut book) = Book::open(&Path::new(root).join("dev-books/snakes.epub")) else { return };
        let mut fonts = FontLibrary::new("Bookerly");
        fonts.add_dir(&Path::new(root).join("dev-fonts"));
        let mut ts = Typesetter::new(Hyphenators::new(Path::new(root).join("assets/hyphenation")));
        let prefs = Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: Some(1.4),
            justify: Some(true),
            hyphenate: true,
            page_width: 1044.0,
        };
        let total: usize = (0..book.chapters.len())
            .map(|i| book.chapter(i, &prefs, &fonts, &mut ts, 1420.0).expect("chapter lays out").pages.len())
            .sum();
        assert!(total > 200, "{total} pages");
    }
}
