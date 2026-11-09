//! The reading view for PDFs: fixed-layout pages rendered by MuPDF, one per screen, white
//! margins cropped. Taps and swipes turn pages as in the [`Reader`](crate::reader::Reader);
//! the menu offers the library and the contents.
//! No highlights.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zarathustra_device::Canvas;
use zarathustra_engine::image::{self, GrayImage};
use zarathustra_engine::pdf::Pdf;

use crate::menu::{self, Control};
use crate::reader::{footer, header, Position, BACK_ZONE, MARGIN_BOTTOM, MARGIN_TOP, SWIPE, TOP_BAND};
use crate::toc::{self, Toc};
use crate::{Action, Ctx, H, W};

/// Space left and right of the page: cropped pages run almost edge to edge.
const GUTTER: i32 = 8;
/// Footer inset, as the reader's normal margin.
const FOOTER_MARGIN: i32 = 96;

pub struct PdfView {
    pdf: Pdf,
    /// From the document info, else the file name.
    title: String,
    author: Option<String>,
    pub page: usize,
    pub menu: bool,
    /// The outline as table-of-contents entries, `chapter` holding the page index.
    contents: Vec<toc::Entry>,
    toc: Option<Toc>,
    /// The page shown, rendered once (the screen redraws for the menu and the clock too).
    // ponytail: renders on the turn; pre-rendering the next page while idle would hide the
    // MuPDF time on heavy (scanned) pages.
    rendered: Option<(usize, GrayImage)>,
    /// Where the page read is saved, and the book's key there.
    store: Option<std::rc::Rc<zarathustra_store::Store>>,
    path: PathBuf,
    pub clock: bool,
}

impl PdfView {
    pub fn open(path: &Path, ctx: &mut Ctx) -> Result<PdfView> {
        let pdf = Pdf::open(path).with_context(|| path.display().to_string())?;
        if pdf.page_count() == 0 {
            bail!("no pages");
        }
        let store = ctx.store();
        let saved = store.as_ref().and_then(|s| Position::load(s, &ctx.state_dir, path));
        let page = saved.map_or(0, |p| p.chapter.min(pdf.page_count() - 1));
        let contents = (pdf.outline().into_iter())
            .map(|e| toc::Entry { title: e.title, depth: e.depth, chapter: e.page, fragment: None })
            .collect();
        Ok(PdfView {
            title: pdf.title().unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned()),
            author: pdf.author(),
            pdf,
            page,
            menu: false,
            contents,
            toc: None,
            rendered: None,
            store,
            path: path.to_path_buf(),
            clock: false,
        })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn pages(&self) -> usize {
        self.pdf.page_count()
    }

    fn progress(&self) -> f32 {
        (self.page + 1) as f32 / self.pages() as f32
    }

    /// Saved as the reader's position format, the page index in place of the chapter.
    fn save(&self) {
        if let Some(store) = &self.store {
            Position { chapter: self.page, block: 0, progress: self.progress(), offset: 0 }.save(store, &self.path);
        }
    }

    /// One page forward or back; false at either end.
    pub fn turn(&mut self, forward: bool) -> bool {
        let next = match forward {
            true if self.page + 1 < self.pages() => self.page + 1,
            false if self.page > 0 => self.page - 1,
            _ => return false,
        };
        self.page = next;
        self.save();
        self.raster();
        true
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32) -> Action {
        if let Some(view) = &mut self.toc {
            return match view.tap(&self.contents, 0, (x0, y0), x) {
                toc::Tap::None | toc::Tap::Bookmark(_) | toc::Tap::Remove(_) => Action::None,
                toc::Tap::Redraw => Action::Redraw { turn: None },
                toc::Tap::Back => {
                    self.toc = None;
                    Action::Redraw { turn: None }
                }
                toc::Tap::Jump(i) => {
                    if let Some(e) = self.contents.get(i) {
                        self.page = e.chapter.min(self.pages() - 1);
                        self.save();
                        self.raster();
                    }
                    self.toc = None;
                    self.menu = false;
                    Action::Redraw { turn: None }
                }
            };
        }
        if self.menu {
            let hit = menu::controls(false).into_iter().find(|(_, r)| r.contains(x0, y0));
            return match hit {
                Some((Control::Library, _)) => {
                    self.save();
                    self.menu = false;
                    Action::Home
                }
                Some((Control::Contents, _)) if !self.contents.is_empty() => {
                    self.show_contents();
                    Action::Redraw { turn: None }
                }
                Some(_) => Action::None,
                None if menu::covers(y0, false) => Action::None,
                None => {
                    self.menu = false;
                    Action::Redraw { turn: None }
                }
            };
        }
        let forward = match x - x0 {
            dx if dx < -SWIPE => true,
            dx if dx > SWIPE => false,
            _ if y0 < TOP_BAND => {
                self.menu = true;
                return Action::Redraw { turn: None };
            }
            _ => x0 >= BACK_ZONE,
        };
        if self.turn(forward) {
            Action::Redraw { turn: Some(forward) }
        } else {
            Action::None
        }
    }

    /// The outline entry being read: the last one starting on or before this page.
    fn current_entry(&self) -> Option<usize> {
        self.contents.iter().rposition(|e| e.chapter <= self.page)
    }

    pub fn show_contents(&mut self) {
        self.toc = Some(Toc::contents(self.current_entry()));
    }

    /// Renders the page shown into the cache unless it is there. The turns call it, so MuPDF
    /// runs while the previous screen update finishes, not after the app waited for it.
    fn raster(&mut self) {
        if self.rendered.as_ref().is_some_and(|(n, _)| *n == self.page) {
            return;
        }
        let t = std::time::Instant::now();
        let (w, h) = (W - 2 * GUTTER, H - MARGIN_TOP - MARGIN_BOTTOM);
        match self.pdf.render(self.page, w as u32, h as u32, true) {
            Ok(img) => self.rendered = Some((self.page, img)),
            Err(e) => {
                eprintln!("page {}: {e}", self.page);
                self.rendered = None;
            }
        }
        eprintln!("page {} rendered in {:?}", self.page, t.elapsed());
    }

    pub fn render(&mut self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(view) = &self.toc {
            return view.render(c, ctx, &self.contents, &[]);
        }
        c.fill(c.bounds(), 255);
        let (w, h) = (W - 2 * GUTTER, H - MARGIN_TOP - MARGIN_BOTTOM);
        self.raster();
        if let Some((_, img)) = &self.rendered {
            let x = GUTTER + (w - img.width as i32) / 2;
            let y = MARGIN_TOP + (h - img.height as i32) / 2;
            image::draw(c, img, x, y);
        }
        if self.clock {
            header(c, ctx);
        }
        footer(c, ctx, FOOTER_MARGIN, self.progress(), (self.page, self.pages()));
        if self.menu {
            let chapter = self.current_entry().and_then(|i| self.contents.get(i)).map(|e| e.title.as_str());
            let chapter = chapter.or((!self.contents.is_empty()).then_some(""));
            menu::render(c, ctx, None, &self.title, chapter);
        }
    }

    /// The sleep screen: this document's title (its first page is not a cover).
    pub fn render_sleep(&self, c: &mut Canvas, ctx: &mut Ctx, battery: Option<zarathustra_device::Battery>) {
        crate::sleep::render_sleep(c, ctx, None, &self.title, self.author.as_deref().unwrap_or(""), battery);
    }
}
