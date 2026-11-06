//! The reading view: an open book, its laid-out chapter and the page shown.
//!
//! Tap the right two thirds (or swipe left) for the next page, the left third (or swipe right)
//! for the previous one, the top band to go back home. The position is saved per book.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use omero_device::{Canvas, Rect};
use omero_engine::book::{Book, Chapter};
use omero_engine::font::draw_glyph;
use omero_engine::layout::page::Page;
use omero_engine::style::cascade::Preferences;
use omero_store::AnnotationData;

use crate::highlight::{page_start, resolve, Highlights, Outcome, TextPos};
use crate::menu::{self, Control, Typography};
use crate::pace::Pace;
use crate::toc::{self, Toc};
use crate::ui::{Label, INK_SOFT, LINE};
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
    state_file: PathBuf,
    /// Decoded images of the pages around the current one (`render` takes `&self`).
    images: std::cell::RefCell<omero_engine::image::ImageCache>,
    /// Show the clock in the header (`Settings::clock`, synced by the app).
    pub clock: bool,
    pace: Pace,
    /// Saved highlights and the selection in progress.
    hl: Highlights,
    /// The definition card of the selected word, over the page.
    card: Option<crate::dict::Card>,
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
    pub fn load(file: &Path) -> Option<Position> {
        let s = std::fs::read_to_string(file).ok()?;
        let mut it = s.split_whitespace();
        Some(Position {
            chapter: it.next()?.parse().ok()?,
            block: it.next()?.parse().ok()?,
            progress: it.next().and_then(|p| p.parse().ok()).unwrap_or(0.0),
            offset: it.next().and_then(|o| o.parse().ok()).unwrap_or(0),
        })
    }
}

impl Reader {
    pub fn open(path: &Path, ctx: &mut Ctx) -> Result<Reader> {
        let book = Book::open(path).with_context(|| path.display().to_string())?;
        if book.chapters.is_empty() {
            bail!("no chapters");
        }
        let typo = Typography::load(&ctx.state_dir);
        let prefs = preferences(&typo);
        let state_file = ctx.state_file(path);
        let saved = Position::load(&state_file);
        let contents = toc::entries(&book);
        let hl = Highlights::open(path, &book, &ctx.state_dir);
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
            state_file,
            images: Default::default(),
            // The app syncs it from the settings before every render.
            clock: false,
            pace: Pace::load(&ctx.state_dir),
            hl,
            card: None,
        };
        crate::dict::dictionaries(&ctx.state_dir); // opened now, indexed in the background
        r.pace.restart(Instant::now());
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

    /// Goes to the page showing a saved highlight; stays put when its chapter doesn't load.
    pub fn goto(&mut self, data: &AnnotationData, ctx: &mut Ctx) {
        let Some(i) = self.book.spine.iter().position(|&s| u32::try_from(s) == Ok(data.start.spine)) else { return };
        let (chapter, page) = (self.chapter_index, self.page);
        match self.load(i, ctx) {
            Ok(()) if !self.chapter.pages.is_empty() => {}
            Ok(()) => return self.restore(chapter, page, ctx),
            Err(e) => {
                eprintln!("chapter {i}: {e:#}");
                return self.restore(chapter, page, ctx);
            }
        }
        let map = omero_engine::anchor::TextMap::new(&self.chapter.dom);
        let range = resolve(&self.chapter.dom, &map, &self.chapter.blocks, data);
        self.page = range.map_or(0, |r| self.page_of(r.start));
        self.save();
    }

    pub fn title(&self) -> Option<&str> {
        self.book.title()
    }

    /// Draws the sleep screen for this book: its cover, or its title page without one.
    pub fn render_sleep(&mut self, c: &mut Canvas, ctx: &mut Ctx, battery: Option<omero_device::Battery>) {
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
        let spine = self.book.spine.get(i).copied().unwrap_or(i);
        self.hl.chapter_loaded(&self.chapter, u32::try_from(spine).unwrap_or(u32::MAX));
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
        let blocks = &self.chapter.blocks;
        (self.chapter.pages.iter()).rposition(|p| page_start(p, blocks).is_some_and(|s| s <= pos)).unwrap_or(0)
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
        self.save();
        true
    }

    /// Progress through the book, 0–1, by chapters and pages.
    pub fn progress(&self) -> f32 {
        let pages = self.chapter.pages.len().max(1) as f32;
        (self.chapter_index as f32 + (self.page as f32 + 1.0) / pages) / self.book.chapters.len() as f32
    }

    pub fn save(&self) {
        let TextPos { block, offset } = self.start();
        let line = format!("{} {block} {:.4} {offset}\n", self.chapter_index, self.progress());
        if let Err(e) = crate::write_atomic(&self.state_file, line) {
            eprintln!("position: {e}");
        }
    }

    /// A swipe up from the bottom edge opens the menu (typography sheet), as the quick panel
    /// opens from the top. True when it did.
    pub fn swipe_up(&mut self, (_, y0): (i32, i32), (_, y): (i32, i32)) -> bool {
        let opens = !self.menu && self.toc.is_none() && self.card.is_none() && y0 >= H - BOTTOM_EDGE && y0 - y > SWIPE;
        if opens {
            self.hl.dismiss();
            self.menu = true;
        }
        opens
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32, ctx: &mut Ctx) -> Action {
        if self.toc.is_some() {
            return self.toc_tap((x0, y0), x, ctx);
        }
        if self.menu {
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
        if (x - x0).abs() <= SWIPE {
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
        if forward {
            if let Some(d) = self.pace.turned(Instant::now()) {
                self.hl.log_page(u32::try_from(d.as_secs()).unwrap_or(u32::MAX));
            }
        } else {
            self.pace.restart(Instant::now()); // re-reading a page isn't reading speed
        }
        Action::Redraw { turn: Some(forward) }
    }

    /// A highlight may be saved: file it under the chapter being read.
    fn file_highlight(&mut self) {
        let entry = self.current_entry().and_then(|i| self.contents.get(i));
        self.hl.chapter_title = entry.map(|e| e.title.clone());
    }

    fn menu_tap(&mut self, (x, y): (i32, i32), ctx: &mut Ctx) -> Action {
        let hit = menu::controls(ctx).into_iter().find(|(_, r)| r.contains(x, y));
        match hit {
            Some((Control::Library, _)) => {
                self.save();
                self.menu = false;
                Action::Home
            }
            Some((Control::Contents, _)) if !self.contents.is_empty() => {
                self.show_contents();
                Action::Redraw { turn: None }
            }
            Some((ctl, _)) => {
                if menu::apply(&mut self.typo, &ctl) {
                    self.typo.save(&ctx.state_dir);
                    self.relayout(ctx);
                    return Action::Redraw { turn: None };
                }
                Action::None
            }
            None if menu::covers(ctx, y) => Action::None,
            None => {
                self.menu = false;
                Action::Redraw { turn: None }
            }
        }
    }

    /// Opens the table of contents over the page.
    pub fn show_contents(&mut self) {
        self.toc = Some(Toc::new(self.current_entry()));
    }

    fn toc_tap(&mut self, start: (i32, i32), x: i32, ctx: &mut Ctx) -> Action {
        let Some(view) = &mut self.toc else { return Action::None };
        match view.tap(&self.contents, start, x) {
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
        }
    }

    /// Opens table-of-contents entry `i` (or the next readable chapter) and closes the list
    /// and the menu. When nothing from there on loads, the page shown stays.
    fn jump(&mut self, i: usize, ctx: &mut Ctx) {
        let Some(e) = self.contents.get(i).cloned() else { return };
        let (chapter, page) = (self.chapter_index, self.page);
        if !self.load_readable(e.chapter, true, ctx) {
            self.restore(chapter, page, ctx);
        } else if let Some(f) = e.fragment.as_deref().filter(|_| self.chapter_index == e.chapter) {
            let block = self.book.fragment_blocks(e.chapter, &self.chapter, &[f]).pop().flatten();
            self.page = block.map_or(0, |b| self.first_page_of(b));
        }
        self.toc = None;
        self.menu = false;
        self.save();
    }

    /// The table-of-contents entry being read.
    fn current_entry(&self) -> Option<usize> {
        toc::current(&self.contents, &self.book, self.chapter_index, &self.chapter, self.page)
    }

    /// Lays the chapter out again with the current typography, keeping the reading position.
    fn relayout(&mut self, ctx: &mut Ctx) {
        let start = self.start();
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

    /// Long press at `at`: starts a selection on the word there. Returns the screen area to
    /// refresh, `None` when nothing was selected.
    pub fn long_press(&mut self, at: (i32, i32)) -> Option<Rect> {
        if self.menu || self.card.is_some() {
            return None;
        }
        outcome_rect(self.hl.long_press(&self.chapter, self.page, self.origin(), at))
    }

    /// The finger moved during a selection; returns the (possibly empty) area to refresh.
    pub fn drag(&mut self, at: (i32, i32)) -> Option<Rect> {
        outcome_rect(self.hl.drag(&self.chapter, self.page, self.origin(), at))
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
        if let Some(view) = &self.toc {
            return view.render(c, ctx, &self.contents);
        }
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
                    omero_engine::image::draw(c, img, x.round() as i32, y.round() as i32);
                }
            }
            self.hl.render_under(c, &self.chapter, self.page, self.origin());
            for line in &page.lines {
                for g in &line.glyphs {
                    let Some(face) = ctx.fonts.face(g.face) else { continue };
                    let x = self.margin_x() as f32 + line.x + g.x;
                    let (gx, glyph) = ctx.raster.glyph(face, g.gid, g.size, x);
                    let y = (MARGIN_TOP as f32 + line.baseline + g.y).round() as i32;
                    draw_glyph(c, glyph, gx, y, 0);
                }
            }
        }
        self.header(c, ctx);
        self.footer(c, ctx);
        self.hl.render_over(c, ctx);
        if let Some(card) = &self.card {
            card.render(c, ctx);
        }
        if self.menu {
            let chapter = self.current_entry().and_then(|i| self.contents.get(i)).map(|e| e.title.as_str());
            let chapter = chapter.or((!self.contents.is_empty()).then_some(""));
            menu::render(c, ctx, &self.typo, self.book.title().unwrap_or(""), chapter);
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
        let dir = std::env::temp_dir().join(format!("omero-pos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("book.pos");
        std::fs::write(&file, "3 7 0.5000\n").expect("write");
        let p = Position::load(&file).expect("old format");
        assert_eq!((p.chapter, p.block, p.offset), (3, 7, 0));
        std::fs::write(&file, "3 7 0.5000 42\n").expect("write");
        assert_eq!(Position::load(&file).map(|p| p.offset), Some(42));
        std::fs::remove_dir_all(dir).ok();
    }
}
