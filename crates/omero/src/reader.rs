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

use crate::highlight::{Highlights, Outcome};
use crate::menu::{self, Control, Typography};
use crate::ui::{Label, INK_SOFT, LINE};
use crate::{Action, Ctx, H, W};

const MARGIN_TOP: i32 = 110;
const MARGIN_BOTTOM: i32 = 130;
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
    chapter: Chapter,
    chapter_index: usize,
    pub page: usize,
    state_file: PathBuf,
    /// Decoded images of the pages around the current one (`render` takes `&self`).
    images: std::cell::RefCell<omero_engine::image::ImageCache>,
    /// Saved highlights and the selection in progress.
    hl: Highlights,
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
        let hl = Highlights::open(path, &book, &ctx.state_dir);
        let mut r = Reader {
            chapter_index: saved.as_ref().map_or(0, |p| p.chapter.min(book.chapters.len() - 1)),
            book,
            prefs,
            typo,
            menu: false,
            chapter: Chapter::default(),
            page: 0,
            state_file,
            images: Default::default(),
            hl,
        };
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
        self.hl.chapter_loaded(&self.chapter, u32::try_from(i).unwrap_or(u32::MAX));
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
        self.hl.dismiss();
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
        if self.menu {
            return self.menu_tap((x0, y0), ctx);
        }
        if (x - x0).abs() <= SWIPE {
            if let Outcome::Redraw(rect) = self.hl.tap(&self.chapter, self.page, self.origin(), (x0, y0)) {
                return Ok(Action::Region(rect));
            }
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
        Ok(if self.turn(forward, ctx)? { Action::Redraw { turn: Some(forward) } } else { Action::None })
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

    /// Top-left corner of the text area on screen.
    fn origin(&self) -> (i32, i32) {
        (self.margin_x(), MARGIN_TOP)
    }

    /// Long press at `at`: starts a selection on the word there. Returns the screen area to
    /// refresh, `None` when nothing was selected.
    pub fn long_press(&mut self, at: (i32, i32)) -> Option<Rect> {
        if self.menu {
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

    /// The finger lifted after selecting: shows the highlight actions.
    pub fn release(&mut self) -> Option<Rect> {
        outcome_rect(self.hl.release(&self.chapter, self.page, self.origin()))
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
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
        self.footer(c, ctx);
        self.hl.render_over(c, ctx);
        if self.menu {
            menu::render(c, ctx, &self.typo, self.book.title().unwrap_or(""));
        }
    }

    /// Page count in the chapter and a thin progress bar through the book.
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
    }
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
        justify: Some(t.justify),
        hyphenate: true,
        page_width: (W - 2 * t.margin_x()) as f32,
    }
}
