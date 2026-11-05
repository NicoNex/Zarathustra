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
use omero_engine::style::cascade::Preferences;

use crate::menu::{self, Control, Typography};
use crate::pace::Pace;
use crate::toc::{self, Toc};
use crate::ui::{Label, INK_SOFT, LINE};
use crate::{Action, Ctx, H, W};

const MARGIN_TOP: i32 = 110;
const MARGIN_BOTTOM: i32 = 130;
/// Clock baseline, well clear of the text starting at `MARGIN_TOP`.
const HEADER_BASELINE: i32 = 66;
/// The band the clock is drawn in, for refreshing it alone.
pub const HEADER: Rect = Rect { x: 0, y: 0, w: W, h: MARGIN_TOP };
/// Taps above this open the reader menu, as on the Kindle.
const TOP_BAND: i32 = 150;
/// Taps left of this go back a page (the Kindle's narrow left strip); elsewhere forward.
const BACK_ZONE: i32 = W / 4;
/// Horizontal travel that makes a touch a swipe.
const SWIPE: i32 = 100;

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
}

/// A saved reading position: chapter, first block on the page, progress through the book.
pub struct Position {
    pub chapter: usize,
    pub block: usize,
    pub progress: f32,
}

impl Position {
    pub fn load(file: &Path) -> Option<Position> {
        let s = std::fs::read_to_string(file).ok()?;
        let mut it = s.split_whitespace();
        Some(Position {
            chapter: it.next()?.parse().ok()?,
            block: it.next()?.parse().ok()?,
            progress: it.next().and_then(|p| p.parse().ok()).unwrap_or(0.0),
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
        let mut r = Reader {
            chapter_index: saved.as_ref().map_or(0, |p| p.chapter.min(book.chapters.len() - 1)),
            book,
            prefs,
            typo,
            menu: false,
            contents,
            toc: None,
            chapter: Chapter { blocks: Vec::new(), pages: Vec::new() },
            page: 0,
            state_file,
            images: Default::default(),
            clock: crate::settings::Settings::load(&ctx.state_dir).clock,
            pace: Pace::load(&ctx.state_dir),
        };
        r.pace.restart(Instant::now());
        r.load(r.chapter_index, ctx)?;
        if let Some(block) = saved.map(|p| p.block) {
            r.page = r.page_of(block);
        }
        if r.chapter.pages.is_empty() {
            r.turn(true, ctx)?; // cover-only or empty chapter: start at the first page with text
        }
        Ok(r)
    }

    pub fn title(&self) -> Option<&str> {
        self.book.title()
    }

    /// Draws the sleep screen for this book: its cover, or its title page without one.
    pub fn render_sleep(&mut self, c: &mut Canvas, ctx: &mut Ctx, battery: Option<omero_device::Battery>) {
        let cover = self.book.cover();
        let title = self.book.title().filter(|t| !t.is_empty()).unwrap_or("Untitled");
        crate::sleep::render_sleep(c, ctx, cover.as_deref(), title, self.book.author().unwrap_or(""), battery);
    }

    pub fn load(&mut self, i: usize, ctx: &mut Ctx) -> Result<()> {
        let t = Instant::now();
        let height = (H - MARGIN_TOP - MARGIN_BOTTOM) as f32;
        self.chapter = self.book.chapter(i, &self.prefs, &ctx.fonts, &mut ctx.ts, height)?;
        self.chapter_index = i;
        self.page = 0;
        eprintln!("chapter {i}: {} pages in {:?}", self.chapter.pages.len(), t.elapsed());
        Ok(())
    }

    /// The page showing block `block`: the last page starting at or before it.
    fn page_of(&self, block: usize) -> usize {
        (self.chapter.pages.iter()).rposition(|p| p.first_block().is_some_and(|b| b as usize <= block)).unwrap_or(0)
    }

    fn first_block(&self) -> usize {
        self.chapter.pages.get(self.page).and_then(|p| p.first_block()).map_or(0, |b| b as usize)
    }

    pub fn pages(&self) -> usize {
        self.chapter.pages.len()
    }

    /// Moves one page forward or back across chapters; false at either end of the book.
    pub fn turn(&mut self, forward: bool, ctx: &mut Ctx) -> Result<bool> {
        if forward && self.page + 1 < self.chapter.pages.len() {
            self.page += 1;
        } else if !forward && self.page > 0 {
            self.page -= 1;
        } else {
            let mut i = self.chapter_index;
            loop {
                i = match (forward, i) {
                    (true, i) if i + 1 < self.book.chapters.len() => i + 1,
                    (false, i) if i > 0 => i - 1,
                    _ => return Ok(false),
                };
                self.load(i, ctx)?;
                if !self.chapter.pages.is_empty() {
                    break;
                }
            }
            self.page = if forward { 0 } else { self.chapter.pages.len() - 1 };
        }
        self.save();
        Ok(true)
    }

    /// Progress through the book, 0–1, by chapters and pages.
    pub fn progress(&self) -> f32 {
        let pages = self.chapter.pages.len().max(1) as f32;
        (self.chapter_index as f32 + (self.page as f32 + 1.0) / pages) / self.book.chapters.len() as f32
    }

    pub fn save(&self) {
        let line = format!("{} {} {:.4}\n", self.chapter_index, self.first_block(), self.progress());
        std::fs::write(&self.state_file, line).ok();
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32, ctx: &mut Ctx) -> Result<Action> {
        if self.toc.is_some() {
            return self.toc_tap((x0, y0), x, ctx);
        }
        if self.menu {
            return self.menu_tap((x0, y0), ctx);
        }
        let forward = match x - x0 {
            dx if dx < -SWIPE => true,
            dx if dx > SWIPE => false,
            _ if y0 < TOP_BAND => {
                self.menu = true;
                return Ok(Action::Redraw { turn: None });
            }
            _ => x0 >= BACK_ZONE,
        };
        if !self.turn(forward, ctx)? {
            return Ok(Action::None);
        }
        if forward {
            self.pace.turned(Instant::now());
        } else {
            self.pace.restart(Instant::now()); // re-reading a page isn't reading speed
        }
        Ok(Action::Redraw { turn: Some(forward) })
    }

    fn menu_tap(&mut self, (x, y): (i32, i32), ctx: &mut Ctx) -> Result<Action> {
        let hit =
            menu::controls(ctx).into_iter().find(|(_, r)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom());
        match hit {
            Some((Control::Library, _)) => {
                self.save();
                self.menu = false;
                Ok(Action::Home)
            }
            Some((Control::Contents, _)) if !self.contents.is_empty() => {
                self.show_contents();
                Ok(Action::Redraw { turn: None })
            }
            Some((ctl, _)) => {
                if menu::apply(&mut self.typo, &ctl) {
                    self.typo.save(&ctx.state_dir);
                    self.relayout(ctx)?;
                    return Ok(Action::Redraw { turn: None });
                }
                Ok(Action::None)
            }
            None if menu::covers(ctx, y) => Ok(Action::None),
            None => {
                self.menu = false;
                Ok(Action::Redraw { turn: None })
            }
        }
    }

    /// Opens the table of contents over the page.
    pub fn show_contents(&mut self) {
        self.toc = Some(Toc::new(self.current_entry()));
    }

    fn toc_tap(&mut self, start: (i32, i32), x: i32, ctx: &mut Ctx) -> Result<Action> {
        let Some(view) = &mut self.toc else { return Ok(Action::None) };
        Ok(match view.tap(&self.contents, start, x) {
            toc::Tap::None => Action::None,
            toc::Tap::Redraw => Action::Redraw { turn: None },
            toc::Tap::Back => {
                self.toc = None;
                Action::Redraw { turn: None }
            }
            toc::Tap::Jump(i) => {
                self.jump(i, ctx)?;
                Action::Redraw { turn: None }
            }
        })
    }

    /// Opens table-of-contents entry `i` and closes the list and the menu.
    fn jump(&mut self, i: usize, ctx: &mut Ctx) -> Result<()> {
        let Some(e) = self.contents.get(i).cloned() else { return Ok(()) };
        self.load(e.chapter, ctx)?;
        if let Some(f) = e.fragment.as_deref() {
            let block = self.book.fragment_blocks(e.chapter, &self.chapter, &[f]).pop().flatten();
            self.page = block.map_or(0, |b| self.page_of(b));
        }
        if self.chapter.pages.is_empty() {
            self.turn(true, ctx)?;
        }
        self.toc = None;
        self.menu = false;
        self.save();
        Ok(())
    }

    /// The table-of-contents entry being read.
    fn current_entry(&self) -> Option<usize> {
        toc::current(&self.contents, &self.book, self.chapter_index, &self.chapter, self.page)
    }

    /// Lays the chapter out again with the current typography, keeping the reading position.
    fn relayout(&mut self, ctx: &mut Ctx) -> Result<()> {
        let block = self.first_block();
        self.prefs = preferences(&self.typo);
        self.load(self.chapter_index, ctx)?;
        self.page = self.page_of(block);
        self.save();
        Ok(())
    }

    fn margin_x(&self) -> i32 {
        self.typo.margin_x()
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
        if !self.clock {
            return;
        }
        let Ctx { fonts, raster, .. } = ctx;
        let Some(face) = fonts.pick(&["sans-serif".into()], 400, false) else { return };
        let label = Label { face, size: 24.0, ink: INK_SOFT };
        let minutes = crate::quick::local_now().1;
        let text = format!("{:02}:{:02}", minutes / 60, minutes % 60);
        label.draw(c, raster, &text, (W as f32 - label.width(&text)) / 2.0, HEADER_BASELINE);
    }

    /// Page count and time left in the chapter, and a thin progress bar through the book.
    fn footer(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let y = H - MARGIN_BOTTOM / 2;
        let margin = self.margin_x();
        let width = W - 2 * margin;
        c.fill(Rect::new(margin, y, width, 2), LINE);
        c.fill(Rect::new(margin, y - 1, (width as f32 * self.progress()) as i32, 4), 60);

        let Ctx { fonts, raster, .. } = ctx;
        let Some(face) = fonts.pick(&["serif".into()], 400, false) else { return };
        let label = Label { face, size: 26.0, ink: INK_SOFT };
        let text = format!("{} / {}", self.page + 1, self.chapter.pages.len());
        let x = (W - margin) as f32 - label.width(&text);
        label.draw(c, raster, &text, x, y + 44);

        let Some(face) = fonts.pick(&["sans-serif".into()], 400, false) else { return };
        let label = Label { face, size: 24.0, ink: INK_SOFT };
        let left = self.pace.minutes(self.chapter.pages.len().saturating_sub(self.page));
        label.draw(c, raster, &format!("{left} min left in chapter"), margin as f32, y + 44);
    }
}

fn preferences(t: &Typography) -> Preferences {
    Preferences {
        font_size: t.font_size(),
        font_family: t.family.clone(),
        line_height: Some(t.line_height()),
        justify: Some(t.justify),
        hyphenate: true,
        page_width: (W - 2 * t.margin_x()) as f32,
    }
}
