//! A book's highlights as a paginated list over the whole screen, opened by a long press on
//! the book in the home. Each entry shows its chapter and the quoted text; a tap opens the
//! book on that highlight.

use std::path::Path;

use omero_device::{Canvas, Rect};
use omero_store::{Annotation, Kind, Store};

use crate::i18n::{lang, t};
use crate::menu::{topbar, PAD, ROW_H, TOPBAR_H};
use crate::ui::{button, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

/// Book title, author and count under the top bar.
const INTRO_H: i32 = 200;
const LIST_TOP: i32 = TOPBAR_H + INTRO_H;
const FOOTER_H: i32 = 150;
const QUOTE_SIZE: f32 = 34.0;
const QUOTE_LINE: i32 = 50;
const MAX_LINES: usize = 5;
const CHAPTER_H: i32 = 46;
/// Space above and below an entry's text, inside its touch area.
const ENTRY_PAD: i32 = 30;
/// The quote sits right of a bar, as a pull quote.
const BAR_W: i32 = 6;
const QUOTE_X: i32 = PAD + BAR_W + 32;
const BACK_W: i32 = 360;
const PAGE_BUTTON_W: i32 = 300;
/// Horizontal travel that makes a touch a swipe (as in the reader).
const SWIPE: i32 = 100;

/// What a tap in the list asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tap {
    None,
    Redraw,
    Back,
    /// Open the book on this highlight (index into [`Notebook::highlights`]).
    Open(usize),
}

/// An entry laid out on its screen.
struct Item {
    index: usize,
    chapter: Option<String>,
    lines: Vec<String>,
    rect: Rect,
}

pub struct Notebook {
    title: String,
    author: String,
    pub highlights: Vec<Annotation>,
    /// Entries per screen, laid out once (the fonts don't change while it is open).
    pages: Vec<Vec<Item>>,
    page: usize,
}

/// Highlights, underlines and notes of the book at `path`, in reading order. Empty when the
/// book was never opened in the reader (it registers it) or the database can't be read.
pub fn load(state_dir: &Path, path: &Path) -> Vec<Annotation> {
    let found = Store::open(&state_dir.join("omero.db")).and_then(|s| match s.book_by_path(path)? {
        Some(id) => s.annotations(id),
        None => Ok(Vec::new()),
    });
    let all = found.map_err(|e| eprintln!("notebook: {e}")).unwrap_or_default();
    all.into_iter().filter(|a| a.data.kind != Kind::Bookmark).collect()
}

impl Notebook {
    pub fn new(ctx: &Ctx, title: &str, author: &str, highlights: Vec<Annotation>) -> Notebook {
        let mut pages = vec![Vec::new()];
        if let (Some(serif), Some(sans)) = (ctx.serif(false), ctx.sans(false)) {
            let quote = Label { face: serif, size: QUOTE_SIZE, ink: INK };
            let chapter = Label { face: sans, size: 24.0, ink: INK_SOFT };
            let width = (W - PAD - QUOTE_X) as f32;
            let mut y = LIST_TOP;
            for (index, a) in highlights.iter().enumerate() {
                let text = a.data.quote.as_ref().map_or("", |q| q.exact.as_str());
                let lines = quote.wrap(text, width, MAX_LINES);
                let ch = a.data.chapter.as_deref().filter(|c| !c.is_empty()).map(|c| chapter.fit(c, width));
                let h = 2 * ENTRY_PAD + lines.len().max(1) as i32 * QUOTE_LINE + ch.as_ref().map_or(0, |_| CHAPTER_H);
                if y + h > H - FOOTER_H && pages.last().is_some_and(|p| !p.is_empty()) {
                    pages.push(Vec::new());
                    y = LIST_TOP;
                }
                let item = Item { index, chapter: ch, lines, rect: Rect::new(0, y, W, h) };
                if let Some(p) = pages.last_mut() {
                    p.push(item);
                }
                y += h;
            }
        }
        Notebook { title: title.to_string(), author: author.to_string(), highlights, pages, page: 0 }
    }

    fn prev_button() -> Rect {
        Rect::new(PAD, H - FOOTER_H + (FOOTER_H - ROW_H) / 2, PAGE_BUTTON_W, ROW_H)
    }

    fn next_button() -> Rect {
        Rect::new(W - PAD - PAGE_BUTTON_W, H - FOOTER_H + (FOOTER_H - ROW_H) / 2, PAGE_BUTTON_W, ROW_H)
    }

    fn flip(&mut self, forward: bool) -> Tap {
        let page = if forward { (self.page + 1).min(self.pages.len() - 1) } else { self.page.saturating_sub(1) };
        if page == self.page {
            return Tap::None;
        }
        self.page = page;
        Tap::Redraw
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32) -> Tap {
        match x - x0 {
            dx if dx < -SWIPE => return self.flip(true),
            dx if dx > SWIPE => return self.flip(false),
            _ => {}
        }
        if y0 < TOPBAR_H {
            return if x0 < BACK_W { Tap::Back } else { Tap::None };
        }
        if y0 >= H - FOOTER_H {
            return match () {
                _ if Self::prev_button().contains(x0, y0) => self.flip(false),
                _ if Self::next_button().contains(x0, y0) => self.flip(true),
                _ => Tap::None,
            };
        }
        let items = &self.pages[self.page];
        items.iter().find(|i| i.rect.contains(x0, y0)).map_or(Tap::None, |i| Tap::Open(i.index))
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        topbar(c, raster, sans, serif_bold, t().back_library, t().highlights);

        let width = (W - 2 * PAD) as f32;
        let title = Label { face: serif_bold, size: 46.0, ink: INK };
        title.draw(c, raster, &title.fit(&self.title, width), PAD as f32, TOPBAR_H + 82);
        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let count = lang().highlights(self.highlights.len());
        let byline = if self.author.is_empty() { count } else { format!("{}  ·  {count}", self.author) };
        soft.draw(c, raster, &soft.fit(&byline, width), PAD as f32, TOPBAR_H + 134);
        c.fill(Rect::new(PAD, LIST_TOP - 2, W - 2 * PAD, 2), LINE);

        if self.highlights.is_empty() {
            soft.draw(c, raster, t().no_highlights, PAD as f32, LIST_TOP + 90);
            soft.draw(c, raster, t().no_highlights_hint, PAD as f32, LIST_TOP + 136);
        }
        let quote = Label { face: serif, size: QUOTE_SIZE, ink: INK };
        let chapter = Label { face: sans, size: 24.0, ink: INK_SOFT };
        for item in &self.pages[self.page] {
            let r = item.rect;
            let mut y = r.y + ENTRY_PAD;
            if let Some(ch) = &item.chapter {
                chapter.draw(c, raster, ch, QUOTE_X as f32, y + 26);
                y += CHAPTER_H;
            }
            let lines = item.lines.len().max(1) as i32;
            c.fill(Rect::new(PAD, y + 8, BAR_W, lines * QUOTE_LINE - 12), INK_SOFT);
            for (n, line) in item.lines.iter().enumerate() {
                quote.draw(c, raster, line, QUOTE_X as f32, y + 36 + n as i32 * QUOTE_LINE);
            }
            c.fill(Rect::new(PAD, r.bottom() - 1, W - 2 * PAD, 1), LINE);
        }

        let pages = self.pages.len();
        if pages > 1 {
            let nav = Label { face: sans, size: 30.0, ink: INK };
            if self.page > 0 {
                button(c, Self::prev_button(), false);
                nav.centered(c, raster, t().previous, Self::prev_button());
            }
            if self.page + 1 < pages {
                button(c, Self::next_button(), false);
                nav.centered(c, raster, t().next, Self::next_button());
            }
            let footer = Rect::new(0, H - FOOTER_H, W, FOOTER_H);
            soft.centered(c, raster, &format!("{} / {pages}", self.page + 1), footer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omero_engine::anchor::{Anchor, TextQuote};
    use omero_store::{AnnotationData, BookId, Color};

    fn highlight(n: usize) -> Annotation {
        let start = Anchor { cfi: String::new(), spine: 0, pos: n as u32 };
        let exact = "Nel mezzo del cammin di nostra vita mi ritrovai per una selva oscura ".repeat(3);
        let data = AnnotationData {
            kind: Kind::Highlight,
            start,
            end_cfi: None,
            quote: Some(TextQuote { exact, prefix: String::new(), suffix: String::new() }),
            chapter: Some("Canto primo".into()),
            page_hint: None,
            note: None,
            color: Color::Gray,
        };
        Annotation { id: n.to_string(), book: BookId(1), data, created_at: 0, updated_at: 0 }
    }

    #[test]
    fn entries_paginate_and_open() {
        let ctx = Ctx::new();
        if ctx.serif(false).is_none() {
            return; // no fonts on this machine
        }
        let mut nb = Notebook::new(&ctx, "Inferno", "Dante", (0..12).map(highlight).collect());
        assert!(nb.pages.len() > 1, "twelve long quotes don't fit one screen");
        assert!(nb.pages.iter().flatten().all(|i| i.rect.bottom() <= H - FOOTER_H));
        let first = nb.pages[0][0].rect;
        assert_eq!(nb.tap((600, first.y + 10), 600), Tap::Open(0));
        assert_eq!(nb.tap((600, first.y + 10), 300), Tap::Redraw, "swipe left: next screen");
        let n = nb.pages[1][0].index;
        assert_eq!(nb.tap((600, first.y + 10), 600), Tap::Open(n));
        assert_eq!(nb.tap((50, 50), 50), Tap::Back);
    }
}
