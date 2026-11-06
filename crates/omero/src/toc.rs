//! The table of contents: a paginated list over the whole screen, opened from the reader
//! menu. The entry being read is marked; a tap on an entry jumps to it.

use omero_device::{Canvas, Rect};
use omero_engine::book::{Book, Chapter};
use omero_engine::epub::TocEntry;
use omero_engine::font::{Face, Rasterizer};

use crate::i18n::t;
use crate::menu::{topbar, PAD, ROW_H, TOPBAR_H};
use crate::ui::{button, rounded_rect, Label, INK, INK_SOFT};
use crate::{Ctx, H, W};

const LIST_TOP: i32 = TOPBAR_H + 20;
const ENTRY_H: i32 = 110;
pub(crate) const FOOTER_H: i32 = 150;
/// Entries per screen.
const ROWS: usize = ((H - LIST_TOP - FOOTER_H) / ENTRY_H) as usize;
/// Indent per nesting level, up to `MAX_DEPTH` levels.
const INDENT: i32 = 56;
const MAX_DEPTH: usize = 3;
pub(crate) const BACK_W: i32 = 360;
const PAGE_BUTTON_W: i32 = 300;
/// Horizontal travel that makes a touch a swipe (as in the reader).
const SWIPE: i32 = 100;

/// A table-of-contents entry resolved to the reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    pub depth: usize,
    /// Index into the book's chapters.
    pub chapter: usize,
    /// Element id inside the chapter, when the entry points into it.
    pub fragment: Option<String>,
}

/// The book's table of contents, flattened depth first. Entries whose target is not a
/// chapter in the reading order (or is missing) are dropped; their children are kept.
pub fn entries(book: &Book) -> Vec<Entry> {
    fn walk(toc: &[TocEntry], depth: usize, chapters: &[String], out: &mut Vec<Entry>) {
        for e in toc {
            let (path, fragment) = e.target.split_once('#').unwrap_or((&e.target, ""));
            let title = e.title.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(chapter) = chapters.iter().position(|c| c == path).filter(|_| !title.is_empty()) {
                let fragment = Some(fragment.to_string()).filter(|f| !f.is_empty());
                out.push(Entry { title, depth, chapter, fragment });
            }
            walk(&e.children, depth + 1, chapters, out);
        }
    }
    let mut out = Vec::new();
    walk(&book.toc, 0, &book.chapters, &mut out);
    out
}

/// The entry being read: the last one that starts before the end of page `page` of chapter
/// `index` (laid out as `chapter`).
pub fn current(entries: &[Entry], book: &Book, index: usize, chapter: &Chapter, page: usize) -> Option<usize> {
    let here: Vec<&str> = entries.iter().filter(|e| e.chapter == index).filter_map(|e| e.fragment.as_deref()).collect();
    let mut blocks = book.fragment_blocks(index, chapter, &here).into_iter();
    let end = chapter.pages.get(page + 1).and_then(|p| p.first_block()).map_or(usize::MAX, |b| b as usize);
    let mut found = None;
    for (i, e) in entries.iter().enumerate() {
        let starts_before = match (e.chapter.cmp(&index), &e.fragment) {
            (std::cmp::Ordering::Less, _) => true,
            (std::cmp::Ordering::Equal, None) => true,
            (std::cmp::Ordering::Equal, Some(_)) => blocks.next().flatten().unwrap_or(0) < end,
            (std::cmp::Ordering::Greater, _) => false,
        };
        if starts_before {
            found = Some(i);
        }
    }
    found
}

/// What a tap in the list asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tap {
    None,
    Redraw,
    /// Back to the reader menu.
    Back,
    /// Go to this entry.
    Jump(usize),
}

/// The open list: which screen of entries is shown and which entry is being read.
pub struct Toc {
    page: usize,
    current: Option<usize>,
}

impl Toc {
    /// Opens on the screen holding the current entry.
    pub fn new(current: Option<usize>) -> Toc {
        Toc { page: current.map_or(0, |i| i / ROWS), current }
    }

    fn pages(entries: &[Entry]) -> usize {
        entries.len().div_ceil(ROWS).max(1)
    }

    fn flip(&mut self, entries: &[Entry], forward: bool) -> Tap {
        let page = if forward { (self.page + 1).min(Self::pages(entries) - 1) } else { self.page.saturating_sub(1) };
        if page == self.page {
            return Tap::None;
        }
        self.page = page;
        Tap::Redraw
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, entries: &[Entry], (x0, y0): (i32, i32), x: i32) -> Tap {
        let inside = |r: Rect| r.contains(x0, y0);
        match x - x0 {
            dx if dx < -SWIPE => return self.flip(entries, true),
            dx if dx > SWIPE => return self.flip(entries, false),
            _ => {}
        }
        if y0 < TOPBAR_H {
            return if x0 < BACK_W { Tap::Back } else { Tap::None };
        }
        if y0 >= H - FOOTER_H {
            return match () {
                _ if inside(prev_button()) => self.flip(entries, false),
                _ if inside(next_button()) => self.flip(entries, true),
                _ => Tap::None,
            };
        }
        let row = (y0 - LIST_TOP).max(0) / ENTRY_H;
        let i = self.page * ROWS + row as usize;
        if (row as usize) < ROWS && i < entries.len() {
            Tap::Jump(i)
        } else {
            Tap::None
        }
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx, entries: &[Entry]) {
        c.fill(c.bounds(), crate::ui::PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        topbar(c, raster, sans, serif_bold, t().back, t().contents);

        let first = self.page * ROWS;
        for (row, (i, e)) in entries.iter().enumerate().skip(first).take(ROWS).enumerate() {
            let r = Rect::new(0, LIST_TOP + row as i32 * ENTRY_H, W, ENTRY_H);
            let x = PAD + INDENT * e.depth.min(MAX_DEPTH) as i32;
            let on = self.current == Some(i);
            if on {
                // A solid bar in the margin, as a bookmark ribbon.
                rounded_rect(c, Rect::new(PAD - 40, r.y + 22, 12, r.h - 44), 6, INK);
            }
            // Nesting shows as indent and a smaller size, not as gray text.
            let size = if e.depth == 0 { 36.0 } else { 33.0 };
            let label = Label { face: if on { serif_bold } else { serif }, size, ink: INK };
            let text = label.fit(&e.title, (W - PAD - x) as f32);
            label.draw(c, raster, &text, x as f32, label.baseline(r));
        }

        pager(c, raster, sans, self.page, Self::pages(entries));
    }
}

/// The footer's "previous screen" button (also the search results').
pub(crate) fn prev_button() -> Rect {
    Rect::new(PAD, H - FOOTER_H + (FOOTER_H - ROW_H) / 2, PAGE_BUTTON_W, ROW_H)
}

pub(crate) fn next_button() -> Rect {
    Rect::new(W - PAD - PAGE_BUTTON_W, H - FOOTER_H + (FOOTER_H - ROW_H) / 2, PAGE_BUTTON_W, ROW_H)
}

/// The footer of a paginated list: previous/next buttons and "page / pages", when there is
/// more than one screen.
pub(crate) fn pager(c: &mut Canvas, raster: &mut Rasterizer, sans: &Face, page: usize, pages: usize) {
    if pages <= 1 {
        return;
    }
    let nav = Label { face: sans, size: 30.0, ink: INK };
    if page > 0 {
        button(c, prev_button(), false);
        nav.centered(c, raster, t().previous, prev_button());
    }
    if page + 1 < pages {
        button(c, next_button(), false);
        nav.centered(c, raster, t().next, next_button());
    }
    let count = Label { face: sans, size: 30.0, ink: INK_SOFT };
    let footer = Rect::new(0, H - FOOTER_H, W, FOOTER_H);
    count.centered(c, raster, &format!("{} / {pages}", page + 1), footer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry { title: format!("{i}"), depth: 0, chapter: i, fragment: None }).collect()
    }

    #[test]
    fn taps_pick_rows_and_flip_pages() {
        let e = entries(ROWS + 3);
        let mut t = Toc::new(Some(ROWS + 1));
        assert_eq!(t.page, 1, "opens on the current entry's screen");
        let row_y = |row: i32| LIST_TOP + row * ENTRY_H + ENTRY_H / 2;
        assert_eq!(t.tap(&e, (600, row_y(2)), 600), Tap::Jump(ROWS + 2));
        assert_eq!(t.tap(&e, (600, row_y(3)), 600), Tap::None, "past the last entry");
        assert_eq!(t.tap(&e, (600, row_y(0)), 900), Tap::Redraw, "swipe right: previous screen");
        assert_eq!(t.page, 0);
        assert_eq!(t.tap(&e, (600, row_y(0)), 900), Tap::None, "already on the first screen");
        let next = next_button();
        assert_eq!(t.tap(&e, (next.x + 10, next.y + 10), next.x + 10), Tap::Redraw);
        assert_eq!(t.tap(&e, (50, 50), 50), Tap::Back);
    }
}
