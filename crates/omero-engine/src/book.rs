//! An open EPUB laid out chapter by chapter.

use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::dom::{Dom, NodeKind};
use crate::epub::{resolve, Epub, EpubError, Metadata};
use crate::fonts::FontLibrary;
use crate::image;
use crate::layout::flow::{self, Block};
use crate::layout::page::{self, Page};
use crate::layout::paragraph::Typesetter;
use crate::mobi::{Mobi, MobiError};
use crate::style::cascade::{compute, Preferences, Stylesheets};

#[derive(Debug, thiserror::Error)]
pub enum BookError {
    #[error(transparent)]
    Epub(#[from] EpubError),
    #[error(transparent)]
    Mobi(#[from] MobiError),
    #[error("no chapter {0}")]
    NoChapter(usize),
    #[error("{path}: {source}")]
    Xml { path: String, source: crate::dom::ParseError },
}

enum Source {
    Epub(Epub<BufReader<File>>),
    Mobi(Mobi),
}

pub struct Book {
    /// Behind a `RefCell` so drawing (which holds `&Book`) can read images from the archive.
    source: RefCell<Source>,
    metadata: Metadata,
    /// Archive paths of the linear XHTML chapters, in reading order.
    pub chapters: Vec<String>,
}

pub struct Chapter {
    pub blocks: Vec<Block>,
    pub pages: Vec<Page>,
}

impl Book {
    /// Opens an EPUB, or an AZW3/MOBI/AZW file by extension.
    pub fn open(path: &Path) -> Result<Book, BookError> {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        if matches!(ext.as_deref(), Some("azw3" | "azw" | "mobi" | "prc")) {
            let mobi = Mobi::open(path)?;
            let (chapters, metadata) = (mobi.chapters.clone(), mobi.metadata.clone());
            return Ok(Book { source: RefCell::new(Source::Mobi(mobi)), metadata, chapters });
        }
        let epub = Epub::open(BufReader::new(File::open(path).map_err(EpubError::from)?))?;
        let chapters =
            (epub.spine.iter()).filter(|s| s.linear && s.media_type.contains("html")).map(|s| s.path.clone()).collect();
        let metadata = epub.metadata.clone();
        Ok(Book { source: RefCell::new(Source::Epub(epub)), metadata, chapters })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn title(&self) -> Option<&str> {
        self.metadata.title.as_deref()
    }

    /// Bytes of a member (stylesheet, image, font) by archive path.
    pub fn resource(&self, path: &str) -> Option<Vec<u8>> {
        match &mut *self.source.borrow_mut() {
            Source::Epub(e) => e.read(path).ok(),
            Source::Mobi(m) => m.read(path).ok(),
        }
    }

    fn read_dom(&mut self, path: &str) -> Result<Dom, BookError> {
        match self.source.get_mut() {
            Source::Epub(e) => Ok(e.read_dom(path)?),
            Source::Mobi(m) => {
                let bytes = m.read(path)?;
                let text = String::from_utf8_lossy(&bytes);
                Dom::parse_xml(&text).map_err(|source| BookError::Xml { path: path.into(), source })
            }
        }
    }

    /// Lays out chapter `i` into pages of `height` pixels; `prefs.page_width` is the column width.
    pub fn chapter(
        &mut self,
        i: usize,
        prefs: &Preferences,
        fonts: &FontLibrary,
        ts: &mut Typesetter,
        height: f32,
    ) -> Result<Chapter, BookError> {
        let path = self.chapters.get(i).ok_or(BookError::NoChapter(i))?.clone();
        let dom = self.read_dom(&path)?;
        let sheets = self.stylesheets(&dom, &path);
        let styles = compute(&dom, &sheets, prefs);
        let mut blocks = flow::blocks(&dom, &styles, self.metadata.language.as_deref());
        for img in blocks.iter_mut().filter_map(|b| b.image.as_mut()) {
            img.src = resolve(&path, &img.src);
            // ponytail: the whole entry is inflated for its header; read a prefix if it shows up.
            img.intrinsic = self.resource(&img.src).and_then(|b| image::dimensions(&b));
        }
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
                    if let Some(bytes) = self.resource(&path) {
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

    /// Every chapter of every local book lays out, and the images in them land on pages.
    #[test]
    fn dev_books_images_reach_pages() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let Ok(dir) = std::fs::read_dir(root.join("dev-books")) else { return };
        let mut fonts = FontLibrary::new("Bookerly");
        fonts.add_dir(&root.join("dev-fonts"));
        let mut ts = Typesetter::new(Hyphenators::new(root.join("assets/hyphenation")));
        let prefs = Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: Some(1.4),
            justify: Some(true),
            hyphenate: true,
            page_width: 1044.0,
        };
        let height = 1408.0;
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "epub") {
                continue;
            }
            let Ok(mut book) = Book::open(&path) else { continue };
            let (mut sized, mut placed) = (0, 0);
            for i in 0..book.chapters.len() {
                let ch = book.chapter(i, &prefs, &fonts, &mut ts, height).expect("chapter lays out");
                sized += ch.blocks.iter().filter(|b| b.image.as_ref().is_some_and(|i| i.intrinsic.is_some())).count();
                for img in ch.pages.iter().flat_map(|p| &p.images) {
                    placed += 1;
                    let src = &ch.blocks[img.block as usize].image.as_ref().expect("image block").src;
                    assert!(book.resource(src).is_some(), "{src} readable");
                    assert!(img.width <= prefs.page_width + 0.5 && img.y + img.height <= height + 0.5, "{img:?}");
                    assert!(img.x >= -0.5 && img.x + img.width <= prefs.page_width + 0.5, "{img:?}");
                }
            }
            assert_eq!(sized, placed, "{}: every sized image is placed", path.display());
            eprintln!("{}: {placed} images", path.display());
        }
    }
}
