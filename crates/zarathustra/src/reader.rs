//! The reading view: an open book, its laid-out chapter and the page shown.
//!
//! Tap the right two thirds (or swipe left) for the next page, the left third (or swipe right)
//! for the previous one, the top band to go back home, the top-right corner to bookmark the
//! page (see [`crate::nav`]). A tap on a note reference opens the note over the page
//! ([`crate::footnote`]). The position is saved per book.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::book::{Book, Chapter};
use zarathustra_engine::layout::page::Page;
use zarathustra_engine::style::cascade::Preferences;
use zarathustra_store::{AnnotationData, Kind, Store};

use crate::footnote::{self, Popup};
use crate::highlight::{page_start, resolve, Highlights, Outcome, TextPos};
use crate::menu::{self, Control, Typography};
use crate::nav::{self, Back, Bookmarks, Mark};
use crate::pace::Pace;
use crate::search::{self, Search};
use crate::toc::{self, Toc};
use crate::ui::{draw_lines, Label, INK_SOFT, LINE};
use crate::{Action, Ctx, H, W};

pub(crate) const MARGIN_TOP: i32 = 110;
pub(crate) const MARGIN_BOTTOM: i32 = 130;
/// Clock baseline, well clear of the text starting at `MARGIN_TOP`.
const HEADER_BASELINE: i32 = 66;
/// The band the clock is drawn in, for refreshing it alone.
pub const HEADER: Rect = Rect { x: 0, y: 0, w: W, h: MARGIN_TOP };
/// Taps above this open the reader menu, as on the Kindle.
pub(crate) const TOP_BAND: i32 = 150;
/// Taps left of this go back a page (the Kindle's narrow left strip); elsewhere forward.
pub(crate) const BACK_ZONE: i32 = W / 4;
/// Horizontal travel that makes a touch a swipe.
pub(crate) const SWIPE: i32 = 100;
/// Swipes starting this close to the bottom edge open the menu.
const BOTTOM_EDGE: i32 = 120;

pub struct Reader {
    book: Book,
    prefs: Preferences,
    typo: Typography,
    /// The menu (top bar + typography sheet) is shown over the page.
    pub menu: bool,
    /// The table of contents, flattened, and the open list over the page.
    contents: Vec<toc::Entry>,
    toc: Option<Toc>,
    chapter: Chapter,
    chapter_index: usize,
    pub page: usize,
    /// The book's file: the key of its saved position.
    path: PathBuf,
    /// Decoded images of the pages around the current one (`render` takes `&self`).
    images: std::cell::RefCell<zarathustra_engine::image::ImageCache>,
    /// While selecting: the screen without the selection (chapter, page, pixels), so each frame
    /// of the drag is a copy and a tint instead of drawing the text again.
    drag_base: std::cell::RefCell<Option<(usize, usize, Vec<u8>)>>,
    /// Show the clock in the header (`Settings::clock`, synced by the app).
    pub clock: bool,
    pace: Pace,
    /// Saved highlights and the selection in progress.
    hl: Highlights,
    marks: Bookmarks,
    /// Where the reader was before the last jump.
    back: Back,
    /// The book fraction under the finger while dragging the menu's scrubber.
    scrub: Option<f32>,
    /// The footnote open over the page.
    note: Option<Popup>,
    /// The definition card of the selected word, over the page.
    card: Option<crate::dict::Card>,
    /// The in-book search (kept while closed, for its results) and the hit opened from it,
    /// marked until the page turns.
    search: Option<Search>,
    found: Option<std::ops::Range<TextPos>>,
}

/// A saved reading position: chapter, where the page starts (block and byte offset in the
/// block's text), progress through the book.
pub struct Position {
    pub chapter: usize,
    pub block: usize,
    pub progress: f32,
    /// Files written before offsets were saved have none: the block's start.
    pub offset: usize,
}

impl Position {
    /// "chapter block progress offset"; positions saved before offsets have none.
    pub fn parse(s: &str) -> Option<Position> {
        let mut it = s.split_whitespace();
        Some(Position {
            chapter: it.next()?.parse().ok()?,
            block: it.next()?.parse().ok()?,
            progress: it.next().and_then(|p| p.parse().ok()).unwrap_or(0.0),
            offset: it.next().and_then(|o| o.parse().ok()).unwrap_or(0),
        })
    }

    pub fn to_text(&self) -> String {
        let Position { chapter, block, progress, offset } = self;
        format!("{chapter} {block} {progress:.4} {offset}")
    }

    /// The saved position of `book`; one an older build left in a file is taken in (once).
    pub fn load(store: &Store, state_dir: &Path, book: &Path) -> Option<Position> {
        let key = book.to_string_lossy();
        match store.position(&key) {
            Ok(Some((value, _))) => return Position::parse(&value),
            Ok(None) => {}
            Err(e) => {
                eprintln!("position: {e}");
                return None;
            }
        }
        let file = crate::legacy_position_file(state_dir, book);
        let text = std::fs::read_to_string(&file).ok()?;
        let at = std::fs::metadata(&file).and_then(|m| m.modified()).map_or_else(|_| now_secs(), unix_secs);
        match store.set_position(&key, &text, at) {
            Ok(()) => drop(std::fs::remove_file(&file)),
            Err(e) => eprintln!("position: {e}"),
        }
        Position::parse(&text)
    }

    /// Saves the position of `book`, logging a failure.
    pub fn save(&self, store: &Store, book: &Path) {
        if let Err(e) = store.set_position(&book.to_string_lossy(), &self.to_text(), now_secs()) {
            eprintln!("position: {e}");
        }
    }
}

/// Leaving the reader (home, another screen, the app quitting) logs the page in front.
impl Drop for Reader {
    fn drop(&mut self) {
        self.flush_reading();
    }
}

pub(crate) fn unix_secs(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

pub(crate) fn now_secs() -> i64 {
    unix_secs(std::time::SystemTime::now())
}

impl Reader {
    pub fn open(path: &Path, ctx: &mut Ctx) -> Result<Reader> {
        let book = Book::open(path).with_context(|| path.display().to_string())?;
        if book.chapters.is_empty() {
            bail!("no chapters");
        }
        let typo = Typography::load(&ctx.state_dir);
        let prefs = preferences(&typo);
        let store = ctx.store();
        let saved = store.as_ref().and_then(|s| Position::load(s, &ctx.state_dir, path));
        let contents = toc::entries(&book);
        let hl = Highlights::open(path, &book, &ctx.state_dir);
        let marks = Bookmarks::load(hl.store());
        let mut r = Reader {
            chapter_index: saved.as_ref().map_or(0, |p| p.chapter.min(book.chapters.len() - 1)),
            book,
            prefs,
            typo,
            menu: false,
            contents,
            toc: None,
            chapter: Chapter::default(),
            page: 0,
            path: path.to_path_buf(),
            images: Default::default(),
            drag_base: Default::default(),
            // The app syncs it from the settings before every render.
            clock: false,
            pace: Pace::load(&ctx.state_dir),
            hl,
            marks,
            back: Back::default(),
            scrub: None,
            note: None,
            card: None,
            search: None,
            found: None,
        };
        crate::dict::dictionaries(&ctx.state_dir); // opened now, indexed in the background
        r.pace.resume(Instant::now());
        // A cover-only, empty or broken chapter: start at the nearest one with pages.
        let start = r.chapter_index;
        if !r.load_readable(start, true, ctx) && !r.load_readable(start, false, ctx) {
            bail!("no readable chapter");
        }
        if let Some(p) = saved.filter(|p| p.chapter == r.chapter_index) {
            r.page = r.page_of(TextPos::new(p.block, p.offset));
        }
        Ok(r)
    }

    /// Goes to the page showing a saved highlight or bookmark; stays put when its chapter
    /// doesn't load.
    pub fn goto(&mut self, data: &AnnotationData, ctx: &mut Ctx) {
        let Some(i) = self.book.spine.iter().position(|&s| u32::try_from(s) == Ok(data.start.spine)) else { return };
        let (chapter, page) = (self.chapter_index, self.page);
        let from = self.mark();
        // `open` laid out the saved chapter: often the highlight's own.
        let loaded = i == chapter && !self.chapter.pages.is_empty();
        match if loaded { Ok(()) } else { self.load(i, ctx) } {
            Ok(()) if !self.chapter.pages.is_empty() => {}
            Ok(()) => return self.restore(chapter, page, ctx),
            Err(e) => {
                eprintln!("chapter {i}: {e:#}");
                return self.restore(chapter, page, ctx);
            }
        }
        let map = zarathustra_engine::anchor::TextMap::new(&self.chapter.dom);
        let (dom, blocks) = (&self.chapter.dom, &self.chapter.blocks);
        let pos = match data.kind {
            Kind::Bookmark => nav::resolve(dom, &map, blocks, data),
            _ => resolve(dom, &map, blocks, data).map(|r| r.start),
        };
        self.page = pos.map_or(0, |p| self.page_of(p));
        self.found = None;
        self.landed(from);
    }

    /// A jump from `from` ended here: the back pill returns there, the position is saved.
    fn landed(&mut self, from: Mark) {
        self.leave_page(false);
        self.back.jumped(from, self.mark());
        self.save();
    }

    /// Where the page shown is, to come back to.
    fn mark(&self) -> Mark {
        Mark { chapter: self.chapter_index, pos: self.start(), page: self.page, progress: self.progress() }
    }

    /// The page shown stops being in front (`forward`: turned to the next one): its time
    /// goes into today's statistics, a forward turn counting one page read.
    fn leave_page(&mut self, forward: bool) {
        if let Some(d) = self.pace.leave(Instant::now(), forward) {
            self.hl.log_page(u32::try_from(d.as_secs()).unwrap_or(u32::MAX), u32::from(forward));
        }
    }

    /// Logs the time on the page shown so far (closing the book); the page stays and its
    /// clock starts over.
    pub fn flush_reading(&mut self) {
        self.leave_page(false);
    }

    /// Logs the page's time so far before sleep or exit, keeping the visit going: a page read
    /// across a sleep stays one visit, and is still counted when turned.
    pub fn bank_reading(&mut self) {
        if let Some(d) = self.pace.bank(Instant::now()) {
            self.hl.log_page(u32::try_from(d.as_secs()).unwrap_or(u32::MAX), 0);
        }
    }

    /// Runs the page's clock while the page is in front, pauses it while anything covers it:
    /// an overlay, a selection or its popup, or `away` (the quick panel, the device asleep).
    /// Idempotent: the app calls it after every batch of events.
    pub fn clock(&mut self, away: bool) {
        let now = Instant::now();
        if away || self.overlay_open() || self.hl.is_active() {
            self.pace.pause(now);
        } else {
            self.pace.resume(now);
        }
    }

    /// Back to where the reader was before the last jump.
    fn go_back(&mut self, ctx: &mut Ctx) {
        let Some(m) = self.back.take() else { return };
        self.leave_page(false);
        self.found = None;
        if m.chapter != self.chapter_index {
            if let Err(e) = self.load(m.chapter, ctx) {
                return eprintln!("chapter {}: {e:#}", m.chapter);
            }
        }
        self.page = self.page_of(m.pos);
        self.save();
    }

    pub fn title(&self) -> Option<&str> {
        self.book.title()
    }

    /// Draws the sleep screen for this book: its cover, or its title page without one.
    pub fn render_sleep(&mut self, c: &mut Canvas, ctx: &mut Ctx, battery: Option<zarathustra_device::Battery>) {
        let cover = self.book.cover();
        let title = self.book.title().filter(|t| !t.is_empty()).unwrap_or(crate::i18n::t().untitled);
        crate::sleep::render_sleep(c, ctx, cover.as_deref(), title, self.book.author().unwrap_or(""), battery);
    }

    pub fn load(&mut self, i: usize, ctx: &mut Ctx) -> Result<()> {
        let t = Instant::now();
        let height = (H - MARGIN_TOP - MARGIN_BOTTOM) as f32;
        self.chapter = self.book.chapter(i, &self.prefs, &ctx.fonts, &mut ctx.ts, height)?;
        self.chapter_index = i;
        self.page = 0;
        self.note = None;
        // The card belongs to the selection `chapter_loaded` drops.
        self.card = None;
        let spine = self.book.spine.get(i).copied().unwrap_or(i);
        let spine = u32::try_from(spine).unwrap_or(u32::MAX);
        self.hl.chapter_loaded(&self.chapter, spine);
        self.marks.chapter_loaded(&self.chapter, spine);
        eprintln!("chapter {i}: {} pages in {:?}", self.chapter.pages.len(), t.elapsed());
        Ok(())
    }

    /// Loads the first chapter with pages from `i` on (`forward`) or back, skipping empty
    /// chapters and ones that fail to load. False when there is none; an empty chapter may
    /// then be loaded (see [`Reader::restore`]).
    fn load_readable(&mut self, mut i: usize, forward: bool, ctx: &mut Ctx) -> bool {
        loop {
            match self.load(i, ctx) {
                Ok(()) if !self.chapter.pages.is_empty() => return true,
                Ok(()) => {}
                Err(e) => eprintln!("chapter {i}: {e:#}"),
            }
            i = match (forward, i) {
                (true, i) if i + 1 < self.book.chapters.len() => i + 1,
                (false, i) if i > 0 => i - 1,
                _ => return false,
            };
        }
    }

    /// Back to page `page` of chapter `chapter` after a move found nothing to show.
    fn restore(&mut self, chapter: usize, page: usize, ctx: &mut Ctx) {
        if self.chapter_index != chapter || self.chapter.pages.is_empty() {
            if let Err(e) = self.load(chapter, ctx) {
                eprintln!("chapter {chapter}: {e:#}");
            }
        }
        self.page = page.min(self.chapter.pages.len().saturating_sub(1));
    }

    /// The page showing position `pos`: the last page starting at or before it.
    fn page_of(&self, pos: TextPos) -> usize {
        nav::page_of(&self.chapter, pos)
    }

    /// The first page showing block `block` (or what follows it, for a block without text).
    fn first_page_of(&self, block: usize) -> usize {
        let shows = |p: &Page| {
            let lines = p.lines.iter().map(|l| l.block);
            lines.chain(p.images.iter().map(|i| i.block)).any(|b| b as usize >= block)
        };
        self.chapter.pages.iter().position(shows).unwrap_or(self.chapter.pages.len().saturating_sub(1))
    }

    /// Where the page shown starts.
    fn start(&self) -> TextPos {
        let page = self.chapter.pages.get(self.page);
        page.and_then(|p| page_start(p, &self.chapter.blocks)).unwrap_or(TextPos::new(0, 0))
    }

    pub fn pages(&self) -> usize {
        self.chapter.pages.len()
    }

    /// Moves one page forward or back across chapters, skipping chapters that are empty or
    /// fail to load; false (and the page unchanged) at either end of the book.
    pub fn turn(&mut self, forward: bool, ctx: &mut Ctx) -> bool {
        if forward && self.page + 1 < self.chapter.pages.len() {
            self.page += 1;
            self.hl.dismiss();
        } else if !forward && self.page > 0 {
            self.page -= 1;
            self.hl.dismiss();
        } else {
            let (chapter, page) = (self.chapter_index, self.page);
            let next = match forward {
                true if chapter + 1 < self.book.chapters.len() => chapter + 1,
                false if chapter > 0 => chapter - 1,
                _ => return false,
            };
            if !self.load_readable(next, forward, ctx) {
                self.restore(chapter, page, ctx);
                return false;
            }
            self.page = if forward { 0 } else { self.chapter.pages.len() - 1 };
        }
        self.leave_page(forward);
        self.turned();
        true
    }

    /// Bookkeeping after any page turn.
    fn turned(&mut self) {
        self.found = None;
        self.back.turned(self.mark());
        self.save();
    }

    /// Progress through the book, 0–1, by chapters and pages.
    pub fn progress(&self) -> f32 {
        let pages = self.chapter.pages.len().max(1) as f32;
        (self.chapter_index as f32 + (self.page as f32 + 1.0) / pages) / self.book.chapters.len() as f32
    }

    pub fn save(&self) {
        let Some((store, _)) = self.hl.store() else { return };
        let TextPos { block, offset } = self.start();
        let position = Position {
            chapter: self.chapter_index,
            block: block as usize,
            progress: self.progress(),
            offset: offset as usize,
        };
        position.save(store, &self.path);
    }

    /// A swipe up from the bottom edge opens the menu (typography sheet), as the quick panel
    /// opens from the top. True when it did.
    pub fn swipe_up(&mut self, (_, y0): (i32, i32), (_, y): (i32, i32)) -> bool {
        let opens = !self.menu
            && self.toc.is_none()
            && self.note.is_none()
            && self.card.is_none()
            && y0 >= H - BOTTOM_EDGE
            && y0 - y > SWIPE;
        if opens {
            self.hl.dismiss();
            self.menu = true;
        }
        opens
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32, ctx: &mut Ctx) -> Action {
        if self.search_shown() {
            return self.search_tap((x0, y0), x, ctx);
        }
        if self.toc.is_some() {
            return self.toc_tap((x0, y0), x, ctx);
        }
        if self.menu {
            if menu::scrubber().contains(x0, y0) {
                return self.scrub_to(x, ctx);
            }
            return self.menu_tap((x0, y0), ctx);
        }
        if let Some(card) = &mut self.card {
            let save = match card.tap((x0, y0), x) {
                crate::dict::Tap::None => return Action::None,
                crate::dict::Tap::Page(rect) => return Action::Region(rect),
                crate::dict::Tap::Highlight => true,
                crate::dict::Tap::Close => false,
            };
            let rect = card.area();
            self.card = None;
            if save {
                self.file_highlight();
            }
            let sel = self.hl.end_selection(&self.chapter, self.page, self.origin(), save);
            return Action::Region(rect.union(&sel));
        }
        if let Some(popup) = &mut self.note {
            let area = popup.bounds();
            return match popup.tap((x0, y0), x) {
                footnote::Tap::None => Action::None,
                footnote::Tap::Redraw => Action::Region(area),
                footnote::Tap::Close => {
                    self.note = None;
                    Action::Region(area)
                }
            };
        }
        let still = (x - x0).abs() <= SWIPE;
        if still && !self.hl.is_active() {
            // The dog-ear corner wins over the top band that opens the menu.
            if nav::in_corner((x0, y0)) {
                let title = self.current_entry().and_then(|i| self.contents.get(i)).map(|e| e.title.clone());
                self.marks.toggle(self.hl.store(), &self.chapter, self.page, title);
                return Action::Region(nav::ribbon_area());
            }
            if self.back.get().is_some() && nav::pill_close().contains(x0, y0) {
                self.back.clear();
                return Action::Region(nav::pill_halo());
            }
            if self.back.get().is_some() && nav::pill().contains(x0, y0) {
                self.go_back(ctx);
                return Action::Redraw { turn: None };
            }
            if let Some(area) = self.open_note((x0, y0), ctx) {
                return Action::Region(area);
            }
        }
        if still {
            if self.hl.is_active() {
                self.file_highlight();
            }
            if let Outcome::Redraw(rect) = self.hl.tap(&self.chapter, self.page, self.origin(), (x0, y0)) {
                return Action::Region(rect);
            }
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
        let active = self.hl.is_active();
        if !self.turn(forward, ctx) {
            // At either end of the book; a chapter reload on the way may have dropped the popup.
            return if active && !self.hl.is_active() { Action::Redraw { turn: None } } else { Action::None };
        }
        Action::Redraw { turn: Some(forward) }
    }

    /// Opens the note whose reference is under a tap at `(x, y)`; returns the area to refresh.
    // ponytail: other internal links (a table of contents in the text, "see chapter 3") still
    // turn the page; follow them with `fragment_blocks` + `first_page_of` and a way back.
    fn open_note(&mut self, (x, y): (i32, i32), ctx: &mut Ctx) -> Option<Rect> {
        let page = self.chapter.pages.get(self.page)?;
        let (ox, oy) = self.origin();
        let a = footnote::link_at(page, &self.chapter.blocks, &self.chapter.dom, (x - ox) as f32, (y - oy) as f32)?;
        let prefs =
            Preferences { font_size: self.prefs.font_size * 0.9, page_width: footnote::TEXT_W, ..self.prefs.clone() };
        let (fonts, ts) = (&ctx.fonts, &mut ctx.ts);
        let note = self.book.note(self.chapter_index, &self.chapter.dom, a, &prefs, fonts, ts, footnote::BODY_H)?;
        let popup = Popup::new(note, &self.chapter.dom.text(a), y);
        let area = popup.bounds();
        self.note = Some(popup);
        Some(area)
    }

    /// Desktop preview: goes to the first page from here on with a note reference and opens
    /// its note (through the tap path); false when the rest of the book has none.
    pub fn open_first_note(&mut self, ctx: &mut Ctx) -> bool {
        loop {
            for p in self.page..self.chapter.pages.len() {
                let page = &self.chapter.pages[p];
                let (ox, oy) = self.origin();
                let refs = page.lines.iter().flat_map(|l| l.glyphs.iter().map(move |g| (l, g))).filter(|(l, g)| {
                    let pos = crate::highlight::glyph_pos(&self.chapter.blocks, l, g.run, g.offset);
                    let node = self.chapter.blocks[pos.block as usize].source_at(pos.offset as usize);
                    let link = node.and_then(|(n, _)| zarathustra_engine::note::link_at(&self.chapter.dom, n));
                    link.is_some_and(|a| zarathustra_engine::note::is_noteref(&self.chapter.dom, a))
                });
                let taps: Vec<(i32, i32)> = refs
                    .map(|(l, g)| {
                        (ox + (l.x + g.x + g.advance / 2.0) as i32, oy + (l.baseline + g.y - 0.3 * g.size) as i32)
                    })
                    .collect();
                for at in taps {
                    self.page = p;
                    if self.open_note(at, ctx).is_some() {
                        return true;
                    }
                }
            }
            if !self.turn(true, ctx) {
                return false;
            }
        }
    }

    /// The open footnote, for the desktop preview.
    pub fn note_mut(&mut self) -> Option<&mut Popup> {
        self.note.as_mut()
    }

    /// A highlight may be saved: file it under the chapter being read.
    fn file_highlight(&mut self) {
        let entry = self.current_entry().and_then(|i| self.contents.get(i));
        self.hl.chapter_title = entry.map(|e| e.title.clone());
    }

    fn menu_tap(&mut self, (x, y): (i32, i32), ctx: &mut Ctx) -> Action {
        let hit = menu::controls(true).into_iter().find(|(_, r)| r.contains(x, y));
        match hit {
            Some((Control::Library, _)) => {
                self.save();
                self.menu = false;
                Action::Home
            }
            Some((Control::Contents, _)) => {
                self.show_contents();
                Action::Redraw { turn: None }
            }
            Some((Control::Search, _)) => {
                self.show_search();
                Action::Redraw { turn: None }
            }
            Some((ctl, _)) => {
                if menu::apply(&mut self.typo, &ctl, ctx) {
                    self.typo.save(&ctx.state_dir);
                    // Weight only changes how glyphs are drawn: no layout, the page stays.
                    if !matches!(ctl, Control::Lighter | Control::Heavier) {
                        self.relayout(ctx);
                    }
                    return Action::Redraw { turn: None };
                }
                Action::None
            }
            None if menu::covers(y, true) => Action::None,
            None => {
                self.menu = false;
                Action::Redraw { turn: None }
            }
        }
    }

    /// Opens the table of contents (the bookmarks, for a book without one) over the page.
    pub fn show_contents(&mut self) {
        let tab = if self.contents.is_empty() { toc::Tab::Bookmarks } else { toc::Tab::Contents };
        self.show_toc(tab);
    }

    pub fn show_toc(&mut self, tab: toc::Tab) {
        self.toc = Some(Toc::new(self.current_entry(), tab));
    }

    /// The finger moved from `start` to `at`: while it drags the menu's scrubber, the area
    /// to redraw (the scrubber); `None` otherwise or when nothing changed.
    pub fn scrub(&mut self, start: (i32, i32), (x, _): (i32, i32)) -> Option<Rect> {
        if !self.menu || self.toc.is_some() || self.search_shown() {
            return None;
        }
        // Checked after the flags: it lays the sheet out, and this runs on every finger move.
        let r = menu::scrubber();
        if !r.contains(start.0, start.1) {
            return None;
        }
        let f = nav::fraction(r, x);
        (self.scrub.replace(f) != Some(f)).then_some(r)
    }

    /// The finger lifted off the scrubber at `x`: goes to that point of the book, its chapter
    /// and the page as far into it, and closes the menu.
    fn scrub_to(&mut self, x: i32, ctx: &mut Ctx) -> Action {
        self.scrub = None;
        self.found = None;
        let from = self.mark();
        let (chapter, within) = nav::target(nav::fraction(menu::scrubber(), x), self.book.chapters.len());
        if chapter != self.chapter_index && !self.load_readable(chapter, true, ctx) {
            self.restore(from.chapter, from.page, ctx);
        } else if self.chapter_index == chapter {
            self.page = nav::page_in(within, self.chapter.pages.len());
        }
        self.menu = false;
        self.landed(from);
        Action::Redraw { turn: None }
    }

    fn toc_tap(&mut self, start: (i32, i32), x: i32, ctx: &mut Ctx) -> Action {
        let Some(view) = &mut self.toc else { return Action::None };
        match view.tap(&self.contents, self.marks.list().len(), start, x) {
            toc::Tap::None => Action::None,
            toc::Tap::Redraw => Action::Redraw { turn: None },
            toc::Tap::Back => {
                self.toc = None;
                Action::Redraw { turn: None }
            }
            toc::Tap::Jump(i) => {
                self.jump(i, ctx);
                Action::Redraw { turn: None }
            }
            toc::Tap::Bookmark(i) => {
                let Some(a) = self.marks.list().get(i) else { return Action::None };
                let data = a.data.clone();
                self.goto(&data, ctx);
                self.toc = None;
                self.menu = false;
                Action::Redraw { turn: None }
            }
            toc::Tap::Remove(i) => {
                self.marks.remove(self.hl.store(), i);
                Action::Redraw { turn: None }
            }
        }
    }

    /// Opens table-of-contents entry `i` (or the next readable chapter) and closes the list
    /// and the menu. When nothing from there on loads, the page shown stays.
    fn jump(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(e) = self.contents.get(i).cloned() else { return };
        let (chapter, page) = (self.chapter_index, self.page);
        let from = self.mark();
        self.found = None;
        if !self.load_readable(e.chapter, true, ctx) {
            self.restore(chapter, page, ctx);
        } else if let Some(f) = e.fragment.as_deref().filter(|_| self.chapter_index == e.chapter) {
            let block = self.book.fragment_blocks(e.chapter, &self.chapter, &[f]).pop().flatten();
            self.page = block.map_or(0, |b| self.first_page_of(b));
        }
        self.toc = None;
        self.menu = false;
        self.landed(from);
    }

    /// Opens the search over the page, on the last results when there are some.
    pub fn show_search(&mut self) {
        Search::show(&mut self.search);
    }

    /// The search screen (and its keyboard) is shown over the page.
    pub fn search_shown(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.open)
    }

    /// A search shown is under way: call [`Reader::search_step`] until it is not.
    pub fn is_searching(&self) -> bool {
        self.search.as_ref().is_some_and(|s| s.open && s.is_running())
    }

    /// Searches on for a moment; returns the area to refresh.
    pub fn search_step(&mut self) -> Option<Rect> {
        self.search.as_mut().filter(|s| s.open)?.step(&self.book, &self.contents)
    }

    /// The finger went down: a key of the search keyboard shows pressed. Returns its area.
    pub fn press(&mut self, at: (i32, i32)) -> Option<Rect> {
        self.search.as_mut().filter(|s| s.open)?.press(at)
    }

    /// The search keyboard, while it is up.
    pub fn keyboard(&self) -> Option<&crate::keyboard::Keyboard> {
        self.search.as_ref().filter(|s| s.open)?.keyboard()
    }

    fn search_tap(&mut self, start: (i32, i32), x: i32, ctx: &mut Ctx) -> Action {
        let Some(s) = &mut self.search else { return Action::None };
        s.tap_view(start, x).unwrap_or_else(|hit| {
            self.open_hit(&hit, ctx);
            Action::Redraw { turn: None }
        })
    }

    /// Shows the page with search hit `hit`, marked; stays put when its chapter doesn't load.
    fn open_hit(&mut self, hit: &search::Found, ctx: &mut Ctx) {
        let (chapter, page) = (self.chapter_index, self.page);
        let from = self.mark();
        if hit.chapter != chapter {
            match self.load(hit.chapter, ctx) {
                Ok(()) if !self.chapter.pages.is_empty() => {}
                Ok(()) => return self.restore(chapter, page, ctx),
                Err(e) => {
                    eprintln!("chapter {}: {e:#}", hit.chapter);
                    return self.restore(chapter, page, ctx);
                }
            }
        }
        let map = zarathustra_engine::anchor::TextMap::new(&self.chapter.dom);
        self.found = search::block_range(&map, &self.chapter.blocks, hit.range.clone());
        self.toc = None;
        self.page = self.found.as_ref().map_or(0, |r| self.page_of(r.start));
        self.menu = false;
        self.landed(from);
    }

    /// The table-of-contents entry being read.
    fn current_entry(&self) -> Option<usize> {
        toc::current(&self.contents, &self.book, self.chapter_index, &self.chapter, self.page)
    }

    /// Lays the chapter out again with the current typography, keeping the reading position.
    fn relayout(&mut self, ctx: &mut Ctx) {
        let start = self.start();
        // ponytail: the way back names a page of the old layout; dropped rather than
        // recomputed, since a typography change mid-jump is rare.
        self.back.clear();
        self.prefs = preferences(&self.typo);
        // On failure the old layout stays up.
        match self.load(self.chapter_index, ctx) {
            Ok(()) => self.page = self.page_of(start),
            Err(e) => eprintln!("chapter {}: {e:#}", self.chapter_index),
        }
        self.save();
    }

    fn margin_x(&self) -> i32 {
        self.typo.margin_x()
    }

    /// Top-left corner of the text area on screen.
    fn origin(&self) -> (i32, i32) {
        (self.margin_x(), MARGIN_TOP)
    }

    /// Something is open over the page (menu, contents, search, a note, a definition).
    fn overlay_open(&self) -> bool {
        self.menu || self.toc.is_some() || self.search_shown() || self.note.is_some() || self.card.is_some()
    }

    /// Long press at `at`: starts a selection on the word there. Returns the screen area to
    /// refresh, `None` when nothing was selected.
    pub fn long_press(&mut self, at: (i32, i32)) -> Option<Rect> {
        if let Some(s) = self.search.as_mut().filter(|s| s.open) {
            return s.long_press(at);
        }
        if self.overlay_open() {
            return None;
        }
        outcome_rect(self.hl.long_press(&self.chapter, self.page, self.origin(), at))
    }

    /// The finger moved during a selection; returns the (possibly empty) area to refresh.
    pub fn drag(&mut self, at: (i32, i32)) -> Option<Rect> {
        outcome_rect(self.hl.drag(&self.chapter, self.page, self.origin(), at))
    }

    /// A finger down on a knob of the selection: that end follows the finger. Returns the area
    /// to refresh, `None` when no knob is there.
    pub fn grab(&mut self, at: (i32, i32)) -> Option<Rect> {
        if self.overlay_open() {
            return None;
        }
        outcome_rect(self.hl.grab(&self.chapter, self.page, self.origin(), at))
    }

    /// While selecting with the finger held at a page edge: how long until the page turns.
    pub fn edge_wait(&self) -> Option<std::time::Duration> {
        self.hl.edge_wait()
    }

    /// The finger held at a page edge long enough while selecting: turns the page within the
    /// chapter, the selection carrying on there. True when the page turned.
    pub fn edge_turn(&mut self) -> bool {
        let Some(next) = self.hl.edge_turn(&self.chapter, self.page) else { return false };
        self.leave_page(next > self.page);
        self.page = next;
        self.turned();
        true
    }

    /// Screen rectangle of a button of the highlight popup, when one is open.
    pub fn popup_button(&self, b: crate::highlight::Button) -> Option<Rect> {
        self.hl.button(b)
    }

    /// The finger lifted after selecting: a single word opens its definition card (which
    /// offers to highlight it), a longer selection the highlight actions.
    pub fn release(&mut self, ctx: &mut Ctx) -> Option<Rect> {
        let origin = self.origin();
        let word = self.hl.selected_word(&self.chapter);
        let at = self.hl.selection_rect(&self.chapter, self.page, origin);
        if let (Some(word), Some(at)) = (word, at) {
            let dicts = crate::dict::dictionaries(&ctx.state_dir);
            let lang = self.book.metadata().language.clone();
            let card = crate::dict::Card::open(dicts, &word, lang.as_deref(), at, &self.prefs, ctx);
            let rect = card.area().union(&at);
            self.card = Some(card);
            return Some(rect);
        }
        outcome_rect(self.hl.release(&self.chapter, self.page, origin))
    }

    /// The definition card is open.
    pub fn card_open(&self) -> bool {
        self.card.is_some()
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(s) = self.search.as_ref().filter(|s| s.open) {
            return s.render(c, ctx);
        }
        if let Some(view) = &self.toc {
            return view.render(c, ctx, &self.contents, self.marks.list());
        }
        let dragging = self.hl.is_dragging() && !self.menu && self.note.is_none() && self.card.is_none();
        let mut base = self.drag_base.borrow_mut();
        if !dragging {
            *base = None;
        }
        match base.as_ref().filter(|(ch, p, _)| dragging && (*ch, *p) == (self.chapter_index, self.page)) {
            Some((_, _, pixels)) => c.restore(pixels),
            None => {
                self.render_page(c, ctx);
                if dragging {
                    *base = Some((self.chapter_index, self.page, c.snapshot()));
                }
            }
        }
        self.hl.render_selection(c, &self.chapter, self.page, self.origin());
        if dragging {
            return;
        }
        self.hl.render_over(c, ctx, &self.chapter, self.page, self.origin());
        if let Some(popup) = &self.note {
            popup.render(c, ctx, self.typo.weight);
        }
        if let Some(card) = &self.card {
            card.render(c, ctx);
        }
        if self.menu {
            let chapter = self.current_entry().and_then(|i| self.contents.get(i)).map(|e| e.title.as_str());
            let chapter = chapter.or((!self.contents.is_empty()).then_some(""));
            menu::render(c, ctx, Some(&self.typo), self.book.title().unwrap_or(""), chapter);
            let f = self.scrub.unwrap_or_else(|| self.progress());
            let chapters = self.book.chapters.len();
            let title = nav::chapter_title(&self.contents, nav::target(f, chapters).0).unwrap_or("");
            nav::draw_scrubber(c, ctx, menu::scrubber(), f, self.scrub.is_some(), title, chapters);
        }
    }

    /// The page as read: images, saved highlights, text, header, footer, ribbon and the way
    /// back; no selection or anything opened over it.
    fn render_page(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), 255);
        if let Some(page) = self.chapter.pages.get(self.page) {
            let mut images = self.images.borrow_mut();
            for im in &page.images {
                let block = self.chapter.blocks.get(im.block as usize);
                let Some(src) = block.and_then(|b| b.image.as_ref()).map(|i| i.src.as_str()) else { continue };
                let (w, h) = (im.width.round() as u32, im.height.round() as u32);
                if let Some(img) = images.get(src, w, h, || self.book.resource(src)) {
                    // Centred in the laid-out box, which may be a pixel off the decoded size.
                    let x = self.margin_x() as f32 + im.x + (im.width - img.width as f32) / 2.0;
                    let y = MARGIN_TOP as f32 + im.y + (im.height - img.height as f32) / 2.0;
                    zarathustra_engine::image::draw(c, img, x.round() as i32, y.round() as i32);
                }
            }
            self.hl.render_under(c, &self.chapter, self.page, self.origin());
            if let Some(found) = &self.found {
                search::render_found(c, page, &self.chapter.blocks, found, self.origin());
            }
            let origin = (self.margin_x() as f32, MARGIN_TOP as f32);
            draw_lines(c, &ctx.fonts, &mut ctx.raster, page, origin, self.typo.weight);
        }
        self.header(c, ctx);
        self.footer(c, ctx);
        if self.marks.marked(&self.chapter, self.page) {
            nav::draw_ribbon(c);
        }
        if let Some(m) = self.back.get() {
            nav::draw_pill(c, ctx, m.progress);
        }
    }

    /// The clock, centred above the text.
    // ponytail: the time is drawn when the page is (turns, plus the app's 5-minute tick), so
    // it can lag a few minutes while a page stays up. Wake on the minute if that matters.
    fn header(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if self.clock {
            header(c, ctx);
        }
    }

    /// Page count and time left in the chapter, and a thin progress bar through the book.
    fn footer(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let y = H - MARGIN_BOTTOM / 2;
        let margin = self.margin_x();
        footer(c, ctx, margin, self.progress(), (self.page, self.chapter.pages.len()));

        let Ctx { fonts, raster, .. } = ctx;
        let Some(face) = crate::sans(fonts, false) else { return };
        let label = Label { face, size: 24.0, ink: INK_SOFT };
        let left = self.pace.minutes(self.chapter.pages.len().saturating_sub(self.page));
        label.draw(c, raster, &crate::i18n::lang().minutes_left(left), margin as f32, y + 44);
    }
}

/// A thin progress bar through the book (`progress` 0–1) and "page / pages" under its right
/// end, `margin` in from the screen edges (also the PDF view's).
pub(crate) fn footer(c: &mut Canvas, ctx: &mut Ctx, margin: i32, progress: f32, (page, pages): (usize, usize)) {
    let y = H - MARGIN_BOTTOM / 2;
    let width = W - 2 * margin;
    c.fill(Rect::new(margin, y, width, 2), LINE);
    c.fill(Rect::new(margin, y - 1, (width as f32 * progress) as i32, 4), 60);

    let Ctx { fonts, raster, .. } = ctx;
    let Some(face) = crate::serif(fonts, false) else { return };
    let label = Label { face, size: 26.0, ink: INK_SOFT };
    let text = format!("{} / {}", page + 1, pages);
    let x = (W - margin) as f32 - label.width(&text);
    label.draw(c, raster, &text, x, y + 44);
}

/// The clock, centred above the page (also the PDF view's).
pub(crate) fn header(c: &mut Canvas, ctx: &mut Ctx) {
    let Ctx { fonts, raster, .. } = ctx;
    let Some(face) = crate::sans(fonts, false) else { return };
    let label = Label { face, size: 24.0, ink: INK_SOFT };
    let text = crate::clock::hhmm(crate::clock::local_now().1);
    label.draw_centred(c, raster, &text, W as f32 / 2.0, HEADER_BASELINE);
}

fn outcome_rect(o: Outcome) -> Option<Rect> {
    match o {
        Outcome::Redraw(r) => Some(r),
        Outcome::Ignored => None,
    }
}

fn preferences(t: &Typography) -> Preferences {
    Preferences {
        font_size: t.font_size(),
        font_family: t.family.clone(),
        line_height: Some(t.line_height()),
        align: Some(t.text_align()),
        hyphenate: true,
        page_width: (W - 2 * t.margin_x()) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_load_with_and_without_offset() {
        let p = Position::parse("3 7 0.5000\n").expect("old format");
        assert_eq!((p.chapter, p.block, p.offset), (3, 7, 0));
        assert_eq!(Position::parse("3 7 0.5000 42").map(|p| p.offset), Some(42));
        let store = Store::open_in_memory().expect("db");
        let dir = std::env::temp_dir().join(format!("zarathustra-pos-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let book = Path::new("/mnt/us/library/Libro.epub");
        assert!(Position::load(&store, &dir, book).is_none());
        Position { chapter: 2, block: 5, progress: 0.25, offset: 9 }.save(&store, book);
        let p = Position::load(&store, &dir, book).expect("saved");
        assert_eq!((p.chapter, p.block, p.offset, p.to_text()), (2, 5, 9, "2 5 0.2500 9".into()));
        std::fs::remove_dir_all(dir).ok();
    }
}
