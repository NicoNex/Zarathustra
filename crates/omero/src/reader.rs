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

use crate::ui::{Label, INK_SOFT, LINE};
use crate::{Action, Ctx, H, W};

const MARGIN_X: i32 = 96;
const MARGIN_TOP: i32 = 110;
const MARGIN_BOTTOM: i32 = 130;
/// Taps above this go back home (until the reader menu exists).
const TOP_BAND: i32 = 140;
/// Horizontal travel that makes a touch a swipe.
const SWIPE: i32 = 100;

pub struct Reader {
    book: Book,
    prefs: Preferences,
    chapter: Chapter,
    chapter_index: usize,
    pub page: usize,
    state_file: PathBuf,
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
        let prefs = Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: Some(1.45),
            justify: Some(true),
            hyphenate: true,
            page_width: (W - 2 * MARGIN_X) as f32,
        };
        let state_file = ctx.state_file(path);
        let saved = Position::load(&state_file);
        let mut r = Reader {
            chapter_index: saved.as_ref().map_or(0, |p| p.chapter.min(book.chapters.len() - 1)),
            book,
            prefs,
            chapter: Chapter { blocks: Vec::new(), pages: Vec::new() },
            page: 0,
            state_file,
        };
        r.load(r.chapter_index, ctx)?;
        if let Some(block) = saved.map(|p| p.block) {
            // The saved block's page: the last page starting at or before it.
            r.page = (r.chapter.pages.iter())
                .rposition(|p| p.lines.first().is_some_and(|l| l.block as usize <= block))
                .unwrap_or(0);
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
        let page = self.chapter.pages.get(self.page);
        let block = page.and_then(|p| p.lines.first()).map_or(0, |l| l.block);
        let line = format!("{} {block} {:.4}\n", self.chapter_index, self.progress());
        std::fs::write(&self.state_file, line).ok();
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32, ctx: &mut Ctx) -> Result<Action> {
        let forward = match x - x0 {
            dx if dx < -SWIPE => true,
            dx if dx > SWIPE => false,
            _ if y0 < TOP_BAND => {
                self.save();
                return Ok(Action::Home);
            }
            _ => x0 >= W / 3,
        };
        Ok(if self.turn(forward, ctx)? { Action::Redraw { turn: Some(forward) } } else { Action::None })
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), 255);
        if let Some(page) = self.chapter.pages.get(self.page) {
            for line in &page.lines {
                for g in &line.glyphs {
                    let Some(face) = ctx.fonts.face(g.face) else { continue };
                    let x = MARGIN_X as f32 + line.x + g.x;
                    let (gx, glyph) = ctx.raster.glyph(face, g.gid, g.size, x);
                    let y = (MARGIN_TOP as f32 + line.baseline + g.y).round() as i32;
                    draw_glyph(c, glyph, gx, y, 0);
                }
            }
        }
        self.footer(c, ctx);
    }

    /// Page count in the chapter and a thin progress bar through the book.
    fn footer(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let y = H - MARGIN_BOTTOM / 2;
        let width = W - 2 * MARGIN_X;
        c.fill(Rect::new(MARGIN_X, y, width, 2), LINE);
        c.fill(Rect::new(MARGIN_X, y - 1, (width as f32 * self.progress()) as i32, 4), 60);

        let Ctx { fonts, raster, .. } = ctx;
        let Some(face) = fonts.pick(&["serif".into()], 400, false) else { return };
        let label = Label { face, size: 26.0, ink: INK_SOFT };
        let text = format!("{} / {}", self.page + 1, self.chapter.pages.len());
        let x = (W - MARGIN_X) as f32 - label.width(&text);
        label.draw(c, raster, &text, x, y + 44);
    }
}
