//! An open EPUB laid out chapter by chapter.

use std::cell::RefCell;
use std::fs::File;
use std::io::BufReader;
use std::mem::take;
use std::path::Path;

use crate::dom::{Dom, NodeKind};
use crate::epub::{resolve, Epub, EpubError, Metadata, TocEntry};
use crate::fonts::FontLibrary;
use crate::image;
use crate::layout::flow::{self, Block};
use crate::layout::page::{self, Page};
use crate::layout::paragraph::Typesetter;
use crate::library::Format;
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
    /// A plain-text file's synthetic XHTML chapters (paths in [`crate::txt::chapter_path`] order).
    Txt(Vec<String>),
}

pub struct Book {
    /// Behind a `RefCell` so drawing (which holds `&Book`) can read images from the archive.
    source: RefCell<Source>,
    metadata: Metadata,
    /// Archive paths of the linear XHTML chapters, in reading order.
    pub chapters: Vec<String>,
    /// Spine position of each chapter (the CFI's package step); the spine also holds
    /// non-linear and non-XHTML items that `chapters` leaves out.
    pub spine: Vec<usize>,
    /// Table of contents; targets are archive paths (as in `chapters`) plus `#fragment`.
    pub toc: Vec<TocEntry>,
}

#[derive(Default)]
pub struct Chapter {
    /// The chapter document, kept to anchor highlights and positions (see [`crate::anchor`]).
    pub dom: Dom,
    pub blocks: Vec<Block>,
    pub pages: Vec<Page>,
}

impl Book {
    /// Opens an EPUB (or kepub), or an AZW3/MOBI/AZW or TXT file by extension.
    pub fn open(path: &Path) -> Result<Book, BookError> {
        if Format::from_path(path) == Some(Format::Txt) {
            let (xhtml, toc) = crate::txt::chapters(&std::fs::read(path).map_err(EpubError::from)?);
            let chapters = (0..xhtml.len()).map(crate::txt::chapter_path).collect();
            let spine = (0..xhtml.len()).collect();
            let title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            let metadata = Metadata { title, ..Metadata::default() };
            return Ok(Book { source: RefCell::new(Source::Txt(xhtml)), metadata, chapters, spine, toc });
        }
        if matches!(Format::from_path(path), Some(Format::Azw3 | Format::Mobi)) {
            let mut mobi = Mobi::open(path)?;
            let (chapters, metadata, toc) = (take(&mut mobi.chapters), take(&mut mobi.metadata), take(&mut mobi.toc));
            let spine = (0..chapters.len()).collect();
            return Ok(Book { source: RefCell::new(Source::Mobi(mobi)), metadata, chapters, spine, toc });
        }
        let mut epub = Epub::open(BufReader::new(File::open(path).map_err(EpubError::from)?))?;
        let (spine, chapters) = (epub.spine.iter().enumerate())
            .filter(|(_, s)| s.linear && s.media_type.contains("html"))
            .map(|(i, s)| (i, s.path.clone()))
            .unzip();
        let (metadata, toc) = (take(&mut epub.metadata), take(&mut epub.toc));
        Ok(Book { source: RefCell::new(Source::Epub(epub)), metadata, chapters, spine, toc })
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
            Source::Txt(_) => None,
        }
    }

    /// Pixel size of an image member, from its header.
    fn image_size(&self, path: &str) -> Option<(u32, u32)> {
        match &mut *self.source.borrow_mut() {
            // ponytail: the whole entry is inflated for its header; read a prefix if it shows up.
            Source::Epub(e) => image::dimensions(&e.read(path).ok()?),
            Source::Mobi(m) => m.image_size(path),
            Source::Txt(_) => None,
        }
    }

    fn read_dom(&self, path: &str) -> Result<Dom, BookError> {
        match &mut *self.source.borrow_mut() {
            Source::Epub(e) => Ok(e.read_dom(path)?),
            Source::Mobi(m) => {
                let bytes = m.read(path)?;
                let text = String::from_utf8_lossy(&bytes);
                Dom::parse_xml(&text).map_err(|source| BookError::Xml { path: path.into(), source })
            }
            Source::Txt(xhtml) => {
                let text = (self.chapters.iter().position(|c| c == path)).and_then(|i| xhtml.get(i));
                let text = text.ok_or(EpubError::Missing("chapter"))?;
                Dom::parse_xml(text).map_err(|source| BookError::Xml { path: path.into(), source })
            }
        }
    }

    /// Chapter `i`'s document, parsed but neither styled nor laid out (for search).
    pub fn chapter_dom(&self, i: usize) -> Result<Dom, BookError> {
        self.read_dom(self.chapters.get(i).ok_or(BookError::NoChapter(i))?)
    }

    /// The first listed author.
    pub fn author(&self) -> Option<&str> {
        self.metadata.authors.first().map(String::as_str)
    }

    /// The cover image file's bytes, at full resolution (for the sleep screen).
    pub fn cover(&mut self) -> Option<Vec<u8>> {
        match self.source.get_mut() {
            Source::Epub(e) => {
                let path = e.cover.clone()?;
                e.read(&path).ok()
            }
            Source::Mobi(m) => m.cover(),
            Source::Txt(_) => None,
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
            img.intrinsic = self.image_size(&img.src);
        }
        let pages = page::layout(&blocks, &styles, fonts, ts, height);
        Ok(Chapter { dom, blocks, pages })
    }

    /// The blocks of laid-out chapter `i` that the elements with `ids` (link fragments) land in.
    pub fn fragment_blocks(&self, i: usize, chapter: &Chapter, ids: &[&str]) -> Vec<Option<usize>> {
        let Some(dom) = self.chapters.get(i).and_then(|p| self.read_dom(p).ok()) else { return vec![None; ids.len()] };
        let by_id = |id: &str| dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id));
        ids.iter().map(|id| flow::block_of(&dom, &chapter.blocks, by_id(id)?)).collect()
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

    fn layout_env() -> (FontLibrary, Typesetter, Preferences) {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let ts = Typesetter::new(Hyphenators::new(root.join("assets/hyphenation")));
        let prefs = Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: None,
            align: None,
            hyphenate: false,
            page_width: 1000.0,
        };
        (FontLibrary::new("Bookerly"), ts, prefs)
    }

    /// Writes `bytes` to a fresh temp file named `name` and opens it as a book.
    fn open_temp(name: &str, bytes: &[u8]) -> Book {
        let dir = std::env::temp_dir().join(format!("omero-book-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, bytes).expect("write book");
        let book = Book::open(&path).expect("opens");
        std::fs::remove_dir_all(&dir).expect("clean up");
        book
    }

    #[test]
    fn opens_txt_as_chapters() {
        let mut book = open_temp("Moby Dick.txt", b"Call me\nIshmael.\n\nChapter 1\n\nLoomings.");
        assert_eq!(book.title(), Some("Moby Dick"));
        assert_eq!(book.chapters, ["text0000.xhtml", "text0001.xhtml"]);
        assert_eq!(book.spine, [0, 1]);
        assert_eq!(book.toc[0].target, "text0001.xhtml");
        let (fonts, mut ts, prefs) = layout_env();
        let ch = book.chapter(0, &prefs, &fonts, &mut ts, 1400.0).expect("lays out");
        assert_eq!(ch.blocks[0].text, "Call me Ishmael.");
    }

    /// Kobo's kepubs wrap every sentence in `<span class="koboSpan">`; the text flows as usual.
    #[test]
    fn opens_kepub_with_kobo_spans() {
        let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>
            <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/>
            <manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
            <spine><itemref idref="c"/></spine></package>"#;
        let xhtml = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><div id="book-columns"><div id="book-inner">
            <p><span class="koboSpan" id="kobo.1.1">Call me Ishmael. </span><span class="koboSpan" id="kobo.1.2">Some years ago.</span></p>
            </div></div></body></html>"#;
        let files = [("META-INF/container.xml", container), ("content.opf", opf), ("c.xhtml", xhtml)];
        let mut book = open_temp("moby.kepub.epub", crate::epub::tests::epub(&files).get_ref());
        let (fonts, mut ts, prefs) = layout_env();
        let ch = book.chapter(0, &prefs, &fonts, &mut ts, 1400.0).expect("lays out");
        let text: Vec<_> = ch.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(text, ["Call me Ishmael. Some years ago."]);
    }

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
            align: Some(crate::style::css::TextAlign::Justify),
            hyphenate: true,
            page_width: 1044.0,
        };
        let total: usize = (0..book.chapters.len())
            .map(|i| book.chapter(i, &prefs, &fonts, &mut ts, 1420.0).expect("chapter lays out").pages.len())
            .sum();
        assert!(total > 200, "{total} pages");
    }

    /// The table-of-contents targets of the local books name chapters, and their fragments
    /// land in blocks.
    #[test]
    fn dev_books_toc_targets_resolve() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let fonts = FontLibrary::new("Bookerly");
        let mut ts = Typesetter::new(Hyphenators::new(root.join("assets/hyphenation")));
        let prefs = Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: None,
            align: None,
            hyphenate: false,
            page_width: 1000.0,
        };
        let books = ["dev-books", "dev-books/azw3"].iter().filter_map(|d| std::fs::read_dir(root.join(d)).ok());
        for path in books.flatten().flatten().map(|e| e.path()).take(8) {
            let Ok(mut book) = Book::open(&path) else { continue };
            let mut flat = Vec::new();
            let mut stack: Vec<&TocEntry> = book.toc.iter().collect();
            while let Some(e) = stack.pop() {
                flat.push(e.target.clone());
                stack.extend(&e.children);
            }
            let (mut found, mut total) = (0, 0);
            for target in flat {
                let (file, id) = target.split_once('#').unwrap_or((&target, ""));
                let Some(i) = book.chapters.iter().position(|c| c == file) else { continue };
                // Some books link to ids that are not there; those open at the chapter start.
                let source = book.resource(file).map(|b| String::from_utf8_lossy(&b).into_owned());
                if id.is_empty() || !source.is_some_and(|s| s.contains(&format!("\"{id}\""))) {
                    continue;
                }
                let ch = book.chapter(i, &prefs, &fonts, &mut ts, 1400.0).expect("chapter lays out");
                total += 1;
                found += usize::from(book.fragment_blocks(i, &ch, &[id])[0].is_some());
            }
            eprintln!("{}: {found}/{total} toc fragments resolve", path.display());
            assert_eq!(found, total, "{}", path.display());
        }
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
            align: Some(crate::style::css::TextAlign::Justify),
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
