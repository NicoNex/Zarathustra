//! The reading view for PDFs: fixed-layout pages rendered by MuPDF, one per screen, white
//! margins cropped. Taps and swipes turn pages as in the [`Reader`](crate::reader::Reader);
//! the menu offers the library, the contents and the search. A long press selects text to
//! highlight ([`PdfMarks`]).

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::image::{self, GrayImage};
use zarathustra_engine::pdf::{PageText, Pdf};

use crate::highlight::mark;
use crate::keyboard::Keyboard;
use crate::menu::{self, Control};
use crate::pdfmarks::{bands, PdfMarks, Place};
use crate::reader::{footer, header, Position, BACK_ZONE, MARGIN_BOTTOM, MARGIN_TOP, SWIPE, TOP_BAND};
use crate::search::{Found, Search};
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
    rendered: Option<Rendered>,
    /// The search (kept while closed, for its results) and the hit opened from it: its
    /// byte range in the page's text, marked until the next turn.
    search: Option<Search>,
    found: Option<Range<usize>>,
    marks: PdfMarks,
    /// Where the page read is saved, and the book's key there.
    store: Option<std::rc::Rc<zarathustra_store::Store>>,
    path: PathBuf,
    pub clock: bool,
}

/// The title of the outline entry `page` is in: the last one starting on or before it.
fn entry_at(contents: &[toc::Entry], page: usize) -> Option<&str> {
    contents.iter().rev().find(|e| e.chapter <= page).map(|e| e.title.as_str())
}

/// A page as shown: its image, where the image is, and its text.
struct Rendered {
    page: usize,
    img: GrayImage,
    place: Place,
    text: PageText,
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
        let title = pdf.title().unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
        let author = pdf.author();
        let marks = PdfMarks::open(store.clone(), path, &title, author.as_deref());
        Ok(PdfView {
            title,
            author,
            pdf,
            page,
            menu: false,
            contents,
            toc: None,
            rendered: None,
            search: None,
            found: None,
            marks,
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
        self.goto(next);
        true
    }

    /// Shows page `page` (clamped), saved as the place read.
    pub fn goto(&mut self, page: usize) {
        self.page = page.min(self.pages() - 1);
        self.found = None;
        self.save();
        self.raster();
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
                    if let Some(page) = self.contents.get(i).map(|e| e.chapter) {
                        self.goto(page);
                    }
                    self.toc = None;
                    self.menu = false;
                    Action::Redraw { turn: None }
                }
            };
        }
        if let Some(s) = self.search_open_mut() {
            return s.tap_view((x0, y0), x).unwrap_or_else(|hit| {
                self.menu = false;
                self.goto(hit.chapter);
                self.found = Some(hit.range);
                Action::Redraw { turn: None }
            });
        }
        if self.menu {
            let hit = menu::controls(false).into_iter().find(|(_, r)| r.contains(x0, y0));
            return match hit {
                Some((Control::Library, _)) => {
                    self.save();
                    self.menu = false;
                    Action::Home
                }
                Some((Control::Search, _)) => {
                    self.show_search();
                    Action::Redraw { turn: None }
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
        // A tap anywhere while text is selected, or on a highlight below the menu band, is the
        // highlights'.
        if (x - x0).abs() <= SWIPE && (self.marks.is_active() || y0 >= TOP_BAND) {
            let chapter = entry_at(&self.contents, self.page);
            if let Some(r) = &self.rendered {
                if let Some(rect) = self.marks.tap(&r.text, &r.place, (x0, y0), chapter) {
                    return Action::Region(rect);
                }
            }
        }
        let forward = match x - x0 {
            dx if dx < -SWIPE => true,
            dx if dx > SWIPE => false,
            _ if y0 < TOP_BAND => {
                self.menu = true;
                self.marks.dismiss();
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

    /// The index of the outline entry being read: the last one starting on or before this page.
    fn current_entry(&self) -> Option<usize> {
        self.contents.iter().rposition(|e| e.chapter <= self.page)
    }

    pub fn show_contents(&mut self) {
        self.toc = Some(Toc::contents(self.current_entry()));
    }

    /// Opens the search over the page, on the last results when there are some.
    pub fn show_search(&mut self) {
        Search::show(&mut self.search);
    }

    fn search_open(&self) -> Option<&Search> {
        self.search.as_ref().filter(|s| s.open)
    }

    fn search_open_mut(&mut self) -> Option<&mut Search> {
        self.search.as_mut().filter(|s| s.open)
    }

    /// A search shown is under way: call [`PdfView::search_step`] until it is not.
    pub fn is_searching(&self) -> bool {
        self.search_open().is_some_and(Search::is_running)
    }

    /// Searches on through the pages for a while; the area to refresh, if any.
    pub fn search_step(&mut self) -> Option<Rect> {
        let (pdf, contents) = (&self.pdf, &self.contents);
        self.search.as_mut().filter(|s| s.open)?.step_with(pdf.page_count(), |query, page, limit| {
            let Ok(text) = pdf.text(page) else { return Vec::new() };
            let hits = query.find(&text.text, limit);
            if hits.is_empty() {
                return Vec::new();
            }
            let title = match entry_at(contents, page) {
                Some(e) => format!("{e} · p. {}", page + 1),
                None => format!("p. {}", page + 1),
            };
            hits.into_iter().map(|r| Found::new(page, r, title.clone(), &text.text)).collect()
        })
    }

    /// The search keyboard, while it is up.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.search_open()?.keyboard()
    }

    /// The finger went down: a key of the search keyboard shows pressed. Returns its area.
    pub fn press(&mut self, at: (i32, i32)) -> Option<Rect> {
        self.search_open_mut()?.press(at)
    }

    /// A finger held still: a key's accents on the search keyboard, else the word under it
    /// selected on the page. The area to refresh.
    pub fn long_press(&mut self, at: (i32, i32)) -> Option<Rect> {
        if let Some(s) = self.search_open_mut() {
            return s.long_press(at);
        }
        if self.menu || self.toc.is_some() {
            return None;
        }
        let r = self.rendered.as_ref()?;
        self.marks.long_press(&r.text, &r.place, at)
    }

    /// The finger moved while selecting.
    pub fn drag(&mut self, at: (i32, i32)) -> Option<Rect> {
        let r = self.rendered.as_ref()?;
        self.marks.drag(&r.text, &r.place, at)
    }

    /// The finger lifted after a long press: the actions for the selection.
    pub fn release(&mut self) -> Option<Rect> {
        let r = self.rendered.as_ref()?;
        self.marks.release(&r.text, &r.place)
    }

    /// Renders the page shown into the cache unless it is there. The turns call it, so MuPDF
    /// runs while the previous screen update finishes, not after the app waited for it.
    fn raster(&mut self) {
        if self.rendered.as_ref().is_some_and(|r| r.page == self.page) {
            return;
        }
        let t = std::time::Instant::now();
        let (w, h) = (W - 2 * GUTTER, H - MARGIN_TOP - MARGIN_BOTTOM);
        self.rendered = match self.pdf.render_with_text(self.page, w as u32, h as u32, true) {
            Ok((img, frame, text)) => {
                let x = GUTTER + (w - img.width as i32) / 2;
                let y = MARGIN_TOP + (h - img.height as i32) / 2;
                self.marks.page_loaded(self.page, &text);
                Some(Rendered { page: self.page, img, place: Place { x, y, frame }, text })
            }
            Err(e) => {
                eprintln!("page {}: {e}", self.page);
                None
            }
        };
        eprintln!("page {} rendered in {:?}", self.page, t.elapsed());
    }

    pub fn render(&mut self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(view) = &self.toc {
            return view.render(c, ctx, &self.contents, &[]);
        }
        if let Some(s) = self.search_open() {
            return s.render(c, ctx);
        }
        c.fill(c.bounds(), 255);
        self.raster();
        if let Some(r) = &self.rendered {
            image::draw(c, &r.img, r.place.x, r.place.y);
            // An opened search hit: gray over the text and a rule beneath, as in the reader.
            if let Some(range) = &self.found {
                bands(&r.text, &r.place, range).into_iter().for_each(|b| mark(c, b));
            }
            self.marks.render(c, ctx, &r.text, &r.place);
        }
        if self.clock {
            header(c, ctx);
        }
        footer(c, ctx, FOOTER_MARGIN, self.progress(), (self.page, self.pages()));
        if self.menu {
            let chapter = entry_at(&self.contents, self.page).or((!self.contents.is_empty()).then_some(""));
            menu::render(c, ctx, None, &self.title, chapter);
        }
    }

    /// The sleep screen: this document's title (its first page is not a cover).
    pub fn render_sleep(&self, c: &mut Canvas, ctx: &mut Ctx, battery: Option<zarathustra_device::Battery>) {
        crate::sleep::render_sleep(c, ctx, None, &self.title, self.author.as_deref().unwrap_or(""), battery);
    }
}
