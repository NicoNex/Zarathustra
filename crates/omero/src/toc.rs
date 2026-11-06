//! The table of contents and the bookmarks: paginated lists over the whole screen, opened
//! from the reader menu, one tab each. The entry being read is marked; a tap on an entry or a
//! bookmark jumps to it, the cross at a bookmark's end removes it.

use omero_device::{Canvas, Rect};
use omero_engine::book::{Book, Chapter};
use omero_engine::epub::TocEntry;
use omero_store::Annotation;

use crate::i18n::t;
use crate::menu::{topbar, PAD, ROW_H, TOPBAR_H};
use crate::ui::{button, rounded_rect, segmented, Label, INK, INK_SOFT, PAPER};
use crate::{Ctx, H, W};

/// The tabs: contents | bookmarks.
const TABS: Rect = Rect { x: PAD, y: TOPBAR_H + 28, w: W - 2 * PAD, h: ROW_H };
/// Top of the list under the tabs, and without them (PDFs: contents only).
const LIST_TOP: i32 = TOPBAR_H + 28 + ROW_H + 20;
const BARE_TOP: i32 = TOPBAR_H + 20;
const ENTRY_H: i32 = 110;
const FOOTER_H: i32 = 150;
/// Indent per nesting level, up to `MAX_DEPTH` levels.
const INDENT: i32 = 56;
const MAX_DEPTH: usize = 3;
const BACK_W: i32 = 360;
const PAGE_BUTTON_W: i32 = 300;
/// The cross removing a bookmark: this wide at the row's right end.
const REMOVE_W: i32 = 130;
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
    /// Go to this bookmark.
    Bookmark(usize),
    /// Delete this bookmark.
    Remove(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Contents,
    Bookmarks,
}

/// The open list: which screen of entries is shown and which entry is being read.
pub struct Toc {
    page: usize,
    current: Option<usize>,
    tab: Tab,
    /// The tabs are shown (else the contents only).
    tabs: bool,
}

impl Toc {
    /// Opens on `tab`; the contents on the screen holding the current entry.
    pub fn new(current: Option<usize>, tab: Tab) -> Toc {
        Toc { page: 0, current, tab, tabs: true }.on_current()
    }

    /// The contents alone, without tabs.
    pub fn contents(current: Option<usize>) -> Toc {
        Toc { page: 0, current, tab: Tab::Contents, tabs: false }.on_current()
    }

    fn on_current(mut self) -> Toc {
        if self.tab == Tab::Contents {
            self.page = self.current.map_or(0, |i| i / self.per_screen());
        }
        self
    }

    fn top(&self) -> i32 {
        if self.tabs {
            LIST_TOP
        } else {
            BARE_TOP
        }
    }

    /// Rows per screen.
    fn per_screen(&self) -> usize {
        ((H - self.top() - FOOTER_H) / ENTRY_H) as usize
    }

    fn pages(&self, rows: usize) -> usize {
        rows.div_ceil(self.per_screen()).max(1)
    }

    /// Rows in the tab shown.
    fn rows(&self, entries: usize, marks: usize) -> usize {
        match self.tab {
            Tab::Contents => entries,
            Tab::Bookmarks => marks,
        }
    }

    fn prev_button() -> Rect {
        Rect::new(PAD, H - FOOTER_H + (FOOTER_H - ROW_H) / 2, PAGE_BUTTON_W, ROW_H)
    }

    fn next_button() -> Rect {
        Rect::new(W - PAD - PAGE_BUTTON_W, H - FOOTER_H + (FOOTER_H - ROW_H) / 2, PAGE_BUTTON_W, ROW_H)
    }

    fn flip(&mut self, rows: usize, forward: bool) -> Tap {
        let page = if forward { (self.page + 1).min(self.pages(rows) - 1) } else { self.page.saturating_sub(1) };
        if page == self.page {
            return Tap::None;
        }
        self.page = page;
        Tap::Redraw
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`; `marks` bookmarks are listed.
    pub fn tap(&mut self, entries: &[Entry], marks: usize, (x0, y0): (i32, i32), x: i32) -> Tap {
        let inside = |r: Rect| r.contains(x0, y0);
        let rows = self.rows(entries.len(), marks);
        match x - x0 {
            dx if dx < -SWIPE => return self.flip(rows, true),
            dx if dx > SWIPE => return self.flip(rows, false),
            _ => {}
        }
        if y0 < TOPBAR_H {
            return if x0 < BACK_W { Tap::Back } else { Tap::None };
        }
        if self.tabs && inside(TABS) {
            let tab = if x0 < TABS.x + TABS.w / 2 { Tab::Contents } else { Tab::Bookmarks };
            if tab == self.tab {
                return Tap::None;
            }
            *self = Toc::new(self.current, tab);
            return Tap::Redraw;
        }
        if y0 >= H - FOOTER_H {
            return match () {
                _ if inside(Self::prev_button()) => self.flip(rows, false),
                _ if inside(Self::next_button()) => self.flip(rows, true),
                _ => Tap::None,
            };
        }
        if y0 < self.top() {
            return Tap::None;
        }
        let row = ((y0 - self.top()) / ENTRY_H) as usize;
        let i = self.page * self.per_screen() + row;
        match self.tab {
            _ if row >= self.per_screen() || i >= rows => Tap::None,
            Tab::Contents => Tap::Jump(i),
            Tab::Bookmarks if x0 >= W - REMOVE_W => Tap::Remove(i),
            Tab::Bookmarks => Tap::Bookmark(i),
        }
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx, entries: &[Entry], marks: &[Annotation]) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(sans_bold), Some(serif), Some(serif_bold)) = (
            crate::sans(fonts, false),
            crate::sans(fonts, true),
            crate::serif(fonts, false),
            crate::serif(fonts, true),
        ) else {
            return;
        };
        topbar(c, raster, sans, serif_bold, t().back, if self.tabs { "" } else { t().contents });
        if self.tabs {
            let on = usize::from(self.tab == Tab::Bookmarks);
            let names = [t().contents, t().bookmarks];
            for (i, (name, r)) in names.into_iter().zip(segmented(c, TABS, 2, Some(on))).enumerate() {
                Label { face: sans_bold, size: 30.0, ink: if i == on { PAPER } else { INK } }
                    .centered(c, raster, name, r);
            }
        }

        let (top, per) = (self.top(), self.per_screen());
        let first = self.page * per;
        if self.tab == Tab::Bookmarks {
            if marks.is_empty() {
                let mid = (top + H - FOOTER_H) / 2;
                let head = Label { face: serif_bold, size: 36.0, ink: INK };
                head.draw_centred(c, raster, t().no_bookmarks, W as f32 / 2.0, mid - 30);
                let hint = Label { face: sans, size: 28.0, ink: INK_SOFT };
                for (n, line) in hint.wrap(t().no_bookmarks_hint, (W - 4 * PAD) as f32, 2).iter().enumerate() {
                    hint.draw_centred(c, raster, line, W as f32 / 2.0, mid + 30 + 42 * n as i32);
                }
            }
            for (row, a) in marks.iter().skip(first).take(per).enumerate() {
                let r = Rect::new(0, top + row as i32 * ENTRY_H, W, ENTRY_H);
                // The ribbon drawn on bookmarked pages, small, in the margin.
                c.fill(Rect::new(PAD - 40, r.y + 24, 14, 34), INK);
                let room = (W - PAD - REMOVE_W) as f32;
                let title = Label { face: serif_bold, size: 33.0, ink: INK };
                let name = a.data.chapter.as_deref().filter(|s| !s.is_empty()).unwrap_or(t().bookmark);
                title.draw(c, raster, &title.fit(name, room - PAD as f32), PAD as f32, r.y + 50);
                if let Some(q) = a.data.quote.as_ref().filter(|q| !q.exact.is_empty()) {
                    let quote = Label { face: serif, size: 28.0, ink: INK_SOFT };
                    let text = quote.fit(&format!("“{}…”", q.exact), room - PAD as f32);
                    quote.draw(c, raster, &text, PAD as f32, r.y + 92);
                }
                let cross = Label { face: sans, size: 44.0, ink: INK_SOFT };
                cross.centered(c, raster, "×", Rect::new(W - REMOVE_W, r.y, REMOVE_W - PAD / 2, r.h));
            }
        }
        for (row, (i, e)) in
            entries.iter().enumerate().skip(first).take(if self.tab == Tab::Contents { per } else { 0 }).enumerate()
        {
            let r = Rect::new(0, top + row as i32 * ENTRY_H, W, ENTRY_H);
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

        let pages = self.pages(self.rows(entries.len(), marks.len()));
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
            let count = Label { face: sans, size: 30.0, ink: INK_SOFT };
            let footer = Rect::new(0, H - FOOTER_H, W, FOOTER_H);
            count.centered(c, raster, &format!("{} / {pages}", self.page + 1), footer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry { title: format!("{i}"), depth: 0, chapter: i, fragment: None }).collect()
    }

    #[test]
    fn taps_pick_rows_and_flip_pages() {
        const ROWS: usize = ((H - LIST_TOP - FOOTER_H) / ENTRY_H) as usize;
        let e = entries(ROWS + 3);
        let mut t = Toc::new(Some(ROWS + 1), Tab::Contents);
        assert_eq!(t.page, 1, "opens on the current entry's screen");
        let row_y = |row: i32| LIST_TOP + row * ENTRY_H + ENTRY_H / 2;
        assert_eq!(t.tap(&e, 0, (600, row_y(2)), 600), Tap::Jump(ROWS + 2));
        assert_eq!(t.tap(&e, 0, (600, row_y(3)), 600), Tap::None, "past the last entry");
        assert_eq!(t.tap(&e, 0, (600, row_y(0)), 900), Tap::Redraw, "swipe right: previous screen");
        assert_eq!(t.page, 0);
        assert_eq!(t.tap(&e, 0, (600, row_y(0)), 900), Tap::None, "already on the first screen");
        let next = Toc::next_button();
        assert_eq!(t.tap(&e, 0, (next.x + 10, next.y + 10), next.x + 10), Tap::Redraw);
        assert_eq!(t.tap(&e, 0, (50, 50), 50), Tap::Back);

        // The bookmarks tab: rows jump, the cross at their end removes.
        let tab = (TABS.right() - 50, TABS.y + 10);
        assert_eq!(t.tap(&e, 2, tab, tab.0), Tap::Redraw);
        assert_eq!((t.tab, t.page), (Tab::Bookmarks, 0));
        assert_eq!(t.tap(&e, 2, (600, row_y(1)), 600), Tap::Bookmark(1));
        assert_eq!(t.tap(&e, 2, (W - 40, row_y(1)), W - 40), Tap::Remove(1));
        assert_eq!(t.tap(&e, 2, (600, row_y(2)), 600), Tap::None, "past the last bookmark");
        assert_eq!(t.tap(&e, 2, (TABS.x + 10, TABS.y + 10), TABS.x + 10), Tap::Redraw);
        assert_eq!((t.tab, t.page), (Tab::Contents, 1), "back on the current entry's screen");

        // Without tabs (PDFs) the list starts higher and the tab band is a row.
        let mut bare = Toc::contents(None);
        assert_eq!(bare.tap(&e, 0, (600, BARE_TOP + 10), 600), Tap::Jump(0));
        assert!(bare.per_screen() > ROWS);
    }
}
