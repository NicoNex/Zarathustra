//! Search inside the open book: a query typed on the on-screen keyboard, then a paginated
//! list of hits (chapter and a snippet with the match in bold). Opened from the reader menu;
//! a tap on a hit opens its page with the match marked until the next page turn.
//!
//! The search walks the chapters' text a few at a time between input events
//! ([`Search::step`]), so the first screen of results shows while the rest is searched.

use std::ops::Range;
use std::time::{Duration, Instant};

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::anchor::TextMap;
use zarathustra_engine::book::Book;
use zarathustra_engine::dom::{Dom, NodeKind};
use zarathustra_engine::font::{Face, Rasterizer};
use zarathustra_engine::layout::flow::Block;
use zarathustra_engine::layout::page::Page;
use zarathustra_engine::search::{snippet, Query};

use crate::highlight::{block_pos, range_bands, TextPos};
use crate::i18n::{lang, t};
use crate::keyboard::{Key, Keyboard, Typed, MAX_QUERY};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::reader::SWIPE;
use crate::toc::{self, next_button, prev_button, BACK_W, FOOTER_H};
use crate::ui::{rounded_outline, rounded_rect, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, H, W};

/// Hits kept per search.
// ponytail: a common word in a long book has thousands of hits and nobody pages through them;
// the list says "first 200". Search on from the last hit if users ask for more.
pub const MAX_HITS: usize = 200;
/// Time spent searching between two looks at the input queue.
const BUDGET: Duration = Duration::from_millis(100);
/// Characters of context before and after a match in its snippet.
const CONTEXT: (usize, usize) = (40, 110);

const FIELD: Rect = Rect { x: PAD, y: TOPBAR_H + 32, w: W - 2 * PAD, h: 104 };
const STATUS_Y: i32 = FIELD.y + FIELD.h + 58;
pub const LIST_TOP: i32 = FIELD.y + FIELD.h + 92;
pub const ROW_H: i32 = 166;
/// Hits per screen.
const ROWS: usize = ((H - LIST_TOP - FOOTER_H) / ROW_H) as usize;

/// A hit: where it is and how the list shows it.
#[derive(Debug, Clone)]
pub struct Found {
    pub chapter: usize,
    /// Byte range in the chapter's [`TextMap`] text.
    pub range: Range<usize>,
    title: String,
    snippet: String,
    bold: Range<usize>,
}

impl Found {
    /// A hit at `range` of `text`, the text of `chapter` (a page, for PDFs), listed under `title`.
    pub fn new(chapter: usize, range: Range<usize>, title: String, text: &str) -> Found {
        let (snippet, bold) = snippet(text, range.clone(), CONTEXT.0, CONTEXT.1);
        Found { chapter, range, title, snippet, bold }
    }
}

/// What a tap on the search screen asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tap {
    None,
    Redraw,
    /// Typing: redraw, refreshing what the keyboard says.
    Typed(Typed),
    /// Back to the reader menu.
    Close,
    /// Open hit `i`.
    Open(usize),
}

#[derive(Default)]
pub struct Search {
    query: String,
    /// Shown while the query is being typed.
    keyboard: Option<Keyboard>,
    pub results: Vec<Found>,
    page: usize,
    /// The search under way and the next chapter to look in.
    run: Option<(Query, usize)>,
    /// Over the reader; kept when closed so reopening shows the last results.
    pub open: bool,
}

impl Search {
    /// A new search, keyboard up.
    pub fn new() -> Search {
        Search { keyboard: Some(Keyboard::default()), open: true, ..Search::default() }
    }

    pub fn is_running(&self) -> bool {
        self.run.is_some()
    }

    /// Opens the search in `slot`, on the last results (or the search under way) when there
    /// are some.
    pub fn show(slot: &mut Option<Search>) {
        match slot {
            Some(s) if !s.results.is_empty() || s.is_running() => s.open = true,
            _ => *slot = Some(Search::new()),
        }
    }

    /// A tap as the view over the search sees it: what to redraw, or (`Err`) the hit to open.
    /// Leaving or opening a hit closes the search.
    pub fn tap_view(&mut self, start: (i32, i32), x: i32) -> Result<Action, Found> {
        Ok(match self.tap(start, x) {
            Tap::None => Action::None,
            Tap::Redraw => Action::Redraw { turn: None },
            Tap::Typed(t) => Action::Typed(t),
            Tap::Close => {
                self.open = false;
                Action::Redraw { turn: None }
            }
            Tap::Open(i) => {
                let Some(hit) = self.results.get(i).cloned() else { return Ok(Action::None) };
                self.open = false;
                return Err(hit);
            }
        })
    }

    fn pages(&self) -> usize {
        self.results.len().div_ceil(ROWS).max(1)
    }

    /// A finger held on a key: its accents, if any. Returns the area to refresh.
    pub fn long_press(&mut self, (x, y): (i32, i32)) -> Option<Rect> {
        self.keyboard.as_mut()?.long_press(x, y)
    }

    /// The finger went down: the key under it shows pressed. Returns the area to refresh.
    pub fn press(&mut self, (x, y): (i32, i32)) -> Option<Rect> {
        self.keyboard.as_mut()?.press(x, y)
    }

    /// The keyboard, while it is up.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.keyboard.as_ref()
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, _)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32) -> Tap {
        if y0 < TOPBAR_H {
            return if x0 < BACK_W { Tap::Close } else { Tap::None };
        }
        if let Some(kb) = &mut self.keyboard {
            let Some((key, typed)) = kb.tap(x0, y0) else { return Tap::None };
            return match key {
                Key::Char(c) if self.query.chars().count() < MAX_QUERY => self.edit(|q| q.push(c), typed),
                Key::Space if !self.query.is_empty() && !self.query.ends_with(' ') => self.edit(|q| q.push(' '), typed),
                Key::Backspace => self.edit(
                    |q| {
                        q.pop();
                    },
                    typed,
                ),
                Key::Enter => self.start(),
                // The layer or shift switched, a full field or a second space: just the keys.
                _ => Tap::Typed(typed),
            };
        }
        if FIELD.contains(x0, y0) {
            self.keyboard = Some(Keyboard::default());
            return Tap::Redraw;
        }
        match x - x0 {
            dx if dx < -SWIPE => return self.flip(true),
            dx if dx > SWIPE => return self.flip(false),
            _ => {}
        }
        if y0 >= H - FOOTER_H {
            return match () {
                _ if prev_button().contains(x0, y0) => self.flip(false),
                _ if next_button().contains(x0, y0) => self.flip(true),
                _ => Tap::None,
            };
        }
        let row = (y0 - LIST_TOP) / ROW_H;
        let i = self.page * ROWS + row.max(0) as usize;
        if y0 >= LIST_TOP && (row as usize) < ROWS && i < self.results.len() {
            Tap::Open(i)
        } else {
            Tap::None
        }
    }

    fn edit(&mut self, f: impl FnOnce(&mut String), typed: Typed) -> Tap {
        f(&mut self.query);
        Tap::Typed(typed.and_text(FIELD))
    }

    fn flip(&mut self, forward: bool) -> Tap {
        let page = if forward { (self.page + 1).min(self.pages() - 1) } else { self.page.saturating_sub(1) };
        if page == self.page {
            return Tap::None;
        }
        self.page = page;
        Tap::Redraw
    }

    /// Starts searching for the query typed; the keyboard goes down.
    fn start(&mut self) -> Tap {
        let Some(query) = Query::new(&self.query) else { return Tap::None };
        self.keyboard = None;
        self.results.clear();
        self.page = 0;
        self.run = Some((query, 0));
        Tap::Redraw
    }

    /// Searches on for a while. Returns the area to refresh when the hits on screen changed
    /// or the search finished.
    pub fn step(&mut self, book: &Book, entries: &[toc::Entry]) -> Option<Rect> {
        self.step_with(book.chapters.len(), |query, i, limit| {
            let Ok(dom) = book.chapter_dom(i) else { return Vec::new() };
            let map = TextMap::new(&dom);
            let hits = query.find(map.text(), limit);
            if hits.is_empty() {
                return Vec::new();
            }
            let marks = titles(&dom, &map, entries, i);
            let title = |at| marks.iter().rev().find(|m| m.0 <= at).map_or("", |m| m.1).to_string();
            hits.into_iter().map(|range| Found::new(i, range.clone(), title(range.start), map.text())).collect()
        })
    }

    /// [`Search::step`] over `units` chapters or pages, `find(query, unit, limit)` giving
    /// at most `limit` hits of one.
    pub fn step_with(
        &mut self,
        units: usize,
        mut find: impl FnMut(&Query, usize, usize) -> Vec<Found>,
    ) -> Option<Rect> {
        let (query, next) = self.run.as_mut()?;
        let t0 = Instant::now();
        let before = self.results.len();
        let screen_end = (self.page + 1) * ROWS;
        while *next < units && self.results.len() < MAX_HITS && t0.elapsed() < BUDGET {
            let i = *next;
            *next += 1;
            self.results.extend(find(query, i, MAX_HITS - self.results.len()));
            // The screen shown just filled up: draw it now, search on afterwards.
            if before < screen_end && self.results.len() >= screen_end {
                break;
            }
        }
        let done = *next >= units || self.results.len() >= MAX_HITS;
        if done {
            eprintln!("search: {} hits in {next} chapters", self.results.len());
            self.run = None;
        }
        let shown = before < screen_end && self.results.len() > before;
        (self.keyboard.is_none() && (shown || done)).then_some(Rect::new(0, FIELD.bottom(), W, H - FIELD.bottom()))
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        if let Some(kb) = &self.keyboard {
            kb.render(c, ctx);
        }
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        topbar(c, raster, sans, serif_bold, t().back, t().search_book);
        self.field(c, raster, serif, sans);

        let status = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let text = match () {
            _ if self.keyboard.is_some() => t().search_hint.to_string(),
            _ if self.run.is_some() => t().searching.to_string(),
            _ if self.results.is_empty() => t().no_results.to_string(),
            _ => lang().results(self.results.len(), self.results.len() >= MAX_HITS),
        };
        status.draw(c, raster, &text, PAD as f32, STATUS_Y);
        if self.keyboard.is_some() {
            return;
        }

        let chapter = Label { face: sans, size: 24.0, ink: INK_SOFT };
        let (regular, bold) =
            (Label { face: serif, size: 32.0, ink: INK }, Label { face: serif_bold, size: 32.0, ink: INK });
        let first = self.page * ROWS;
        for (row, f) in self.results.iter().skip(first).take(ROWS).enumerate() {
            let y = LIST_TOP + row as i32 * ROW_H;
            if row > 0 {
                c.fill(Rect::new(PAD, y, W - 2 * PAD, 2), LINE);
            }
            let title = chapter.fit(&f.title, (W - 2 * PAD) as f32);
            chapter.draw(c, raster, &title, PAD as f32, y + 46);
            let lines = rich_lines(&regular, &bold, &f.snippet, &f.bold, (W - 2 * PAD) as f32, 2);
            for (n, line) in lines.iter().enumerate() {
                let mut x = PAD as f32;
                for (text, b) in line {
                    x += if *b { &bold } else { &regular }.draw(c, raster, text, x, y + 98 + 46 * n as i32);
                }
            }
        }
        toc::pager(c, raster, sans, self.page, self.pages());
    }

    /// The query field: a magnifier, the query (or a placeholder) and, while typing, a caret.
    fn field(&self, c: &mut Canvas, raster: &mut Rasterizer, serif: &Face, sans: &Face) {
        rounded_outline(c, FIELD, FIELD.h / 2, 3, INK, PAPER);
        icon(c, Rect::new(FIELD.x + 30, FIELD.y + 30, 44, 44), INK, PAPER);
        let x = (FIELD.x + 100) as f32;
        let width = (FIELD.right() - 60) as f32 - x;
        if self.query.is_empty() {
            let hint = Label { face: sans, size: 34.0, ink: INK_SOFT };
            hint.draw(c, raster, t().search_book, x, hint.baseline(FIELD));
        } else {
            let text = Label { face: serif, size: 40.0, ink: INK };
            // Long queries show their end, where the typing is.
            let mut shown = self.query.as_str();
            while text.width(shown) > width {
                let mut chars = shown.chars();
                chars.next();
                shown = chars.as_str();
            }
            let advance = text.draw(c, raster, shown, x, text.baseline(FIELD));
            if self.keyboard.is_some() {
                c.fill(Rect::new((x + advance) as i32 + 4, FIELD.y + 26, 3, FIELD.h - 52), INK);
            }
        }
        if self.keyboard.is_some() && self.query.is_empty() {
            c.fill(Rect::new(x as i32 - 6, FIELD.y + 26, 3, FIELD.h - 52), INK);
        }
    }
}

/// Where each table-of-contents entry starts in chapter `i`'s text, for naming the chapter a
/// hit is in; the entry read before the chapter starts covers its beginning.
fn titles<'a>(dom: &Dom, map: &TextMap, entries: &'a [toc::Entry], i: usize) -> Vec<(usize, &'a str)> {
    let before = entries.iter().rfind(|e| e.chapter < i).map(|e| (0, e.title.as_str()));
    let mut marks: Vec<_> = before.into_iter().collect();
    for e in entries.iter().filter(|e| e.chapter == i) {
        let at = match &e.fragment {
            None => Some(0),
            Some(id) => (dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id)))
                .and_then(|el| dom.nodes_from(el).find(|&n| matches!(dom.kind(n), NodeKind::Text { .. })))
                .and_then(|n| map.to_text(n, 0)),
        };
        marks.extend(at.map(|at| (at, e.title.as_str())));
    }
    marks.sort_by_key(|m| m.0); // stable: entries at the same place keep the deepest last
    marks
}

/// Greedy wrap of `text` into at most `max` lines of (fragment, bold) runs, the `bold` byte
/// range set in the bold face; the last line ends in an ellipsis when text is left over.
fn rich_lines(
    regular: &Label,
    bold: &Label,
    text: &str,
    b: &Range<usize>,
    width: f32,
    max: usize,
) -> Vec<Vec<(String, bool)>> {
    let words = text.split(' ').scan(0, |at, w| {
        let start = *at;
        *at += w.len() + 1;
        Some(start..start + w.len())
    });
    let mut lines: Vec<Vec<(String, bool)>> = vec![Vec::new()];
    let mut x = 0.0;
    let space = regular.width(" ");
    for w in words {
        // The word split at the edges of the bold range.
        let cuts = [w.start, b.start.clamp(w.start, w.end), b.end.clamp(w.start, w.end), w.end];
        let runs: Vec<(String, bool)> = (cuts.windows(2).enumerate())
            .filter(|(_, c)| c[0] < c[1])
            .map(|(k, c)| (text[c[0]..c[1]].to_string(), k == 1))
            .collect();
        let w_width: f32 = runs.iter().map(|(s, bo)| if *bo { bold } else { regular }.width(s)).sum();
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && x + space + w_width > width {
            if lines.len() == max {
                ellipsize(lines.last_mut().expect("never empty"), regular, bold, x, width);
                return lines;
            }
            lines.push(Vec::new());
            x = 0.0;
        }
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() {
            line.push((" ".into(), false));
            x += space;
        }
        line.extend(runs);
        x += w_width;
    }
    lines
}

/// Ends `line` (`x` wide) with "…", dropping trailing runs until it fits in `width`.
fn ellipsize(line: &mut Vec<(String, bool)>, regular: &Label, bold: &Label, mut x: f32, width: f32) {
    let dots = regular.width("…");
    while x + dots > width {
        let Some((s, b)) = line.pop() else { break };
        x -= if b { bold } else { regular }.width(&s);
    }
    if line.last().is_some_and(|(s, _)| s == " ") {
        line.pop();
    }
    line.push(("…".into(), false));
}

/// The laid-out range of a hit: its [`TextMap`] range mapped to block positions.
pub fn block_range(map: &TextMap, blocks: &[Block], range: Range<usize>) -> Option<Range<TextPos>> {
    let (sn, so) = map.to_dom(range.start)?;
    let (en, eo) = map.to_dom_end(range.end)?;
    let r = block_pos(blocks, sn, so)?..block_pos(blocks, en, eo)?;
    (r.start < r.end).then_some(r)
}

/// Marks an opened hit on the page: gray under the text and a black rule beneath it (saved
/// highlights have no rule). Call before the page's glyphs.
pub fn render_found(c: &mut Canvas, page: &Page, blocks: &[Block], range: &Range<TextPos>, origin: (i32, i32)) {
    for (l, r, top, bottom) in range_bands(page, blocks, range) {
        let (x0, x1) = (origin.0 + l.floor() as i32, origin.0 + r.ceil() as i32);
        let (y0, y1) = (origin.1 + top.floor() as i32, origin.1 + bottom.ceil() as i32);
        c.fill(Rect::new(x0, y0, x1 - x0, y1 - y0), 215);
        c.fill(Rect::new(x0, y1 + 2, x1 - x0, 4), INK);
    }
}

/// A magnifier filling square `r`: a ring and a round-capped handle, `ink` on `bg`.
pub fn icon(c: &mut Canvas, r: Rect, ink: u8, bg: u8) {
    let d = r.w * 7 / 10;
    rounded_outline(c, Rect::new(r.x, r.y, d, d), d / 2, (r.w / 11).max(3), ink, bg);
    let stroke = (r.w / 7).max(4);
    // The handle: discs along the diagonal from the ring's edge to the corner.
    let from = r.x + d * 85 / 100;
    for p in from - stroke / 2..=r.right() - stroke {
        rounded_rect(c, Rect::new(p, p - r.x + r.y, stroke, stroke), stroke / 2, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(n: usize) -> Vec<Found> {
        (0..n)
            .map(|i| Found { chapter: i, range: 0..1, title: String::new(), snippet: "x".into(), bold: 0..1 })
            .collect()
    }

    #[test]
    fn typing_edits_the_query_and_enter_starts() {
        let mut s = Search::new();
        let key = |s: &Search, k: Key| {
            let r = s.keyboard.as_ref().and_then(|kb| kb.keys().into_iter().find(|&(key, _)| key == k)).expect("key").1;
            (r.x + r.w / 2, r.y + r.h / 2)
        };
        for k in [Key::Char('p'), Key::Char(','), Key::Space, Key::Space, Key::Char('a'), Key::Backspace] {
            let at = key(&s, k);
            assert!(matches!(s.tap(at, at.0), Tap::Typed(t) if t.keys.contains(at.0, at.1)));
        }
        assert_eq!(s.query, "p, ");
        // Held, e opens its accents; a tap off the strip only closes it.
        let (e, q) = (key(&s, Key::Char('e')), key(&s, Key::Char('q')));
        assert!(s.long_press(e).is_some());
        assert!(matches!(s.tap(q, q.0), Tap::Typed(_)));
        assert_eq!(s.query, "p, ");
        let at = key(&s, Key::Enter);
        assert_eq!(s.tap(at, at.0), Tap::Redraw);
        assert!(s.keyboard.is_none() && s.is_running());
        assert_eq!(s.tap((FIELD.x + 200, FIELD.y + 50), FIELD.x + 200), Tap::Redraw, "the field reopens the keyboard");
        assert!(s.keyboard.is_some());
        assert_eq!(s.tap((50, 50), 50), Tap::Close);
    }

    #[test]
    fn result_rows_open_and_pages_flip() {
        let mut s = Search { results: found(ROWS + 2), ..Search::default() };
        let row = |r: i32| LIST_TOP + r * ROW_H + ROW_H / 2;
        assert_eq!(s.tap((600, row(1)), 600), Tap::Open(1));
        assert_eq!(s.tap((600, row(1)), 200), Tap::Redraw, "swipe left: next screen");
        assert_eq!(s.tap((600, row(1)), 600), Tap::Open(ROWS + 1));
        assert_eq!(s.tap((600, row(2)), 600), Tap::None, "past the last hit");
        assert_eq!(s.tap((600, STATUS_Y), 600), Tap::None);
    }

    #[test]
    fn hits_stream_across_chapters_with_titles() {
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let Ok(book) = Book::open(&root.join("dev-books/snakes.epub")) else { return };
        let entries = toc::entries(&book);
        let mut s = Search { query: "the".into(), ..Search::default() };
        s.start();
        let mut steps = 0;
        while s.is_running() {
            s.step(&book, &entries);
            steps += 1;
        }
        assert_eq!(s.results.len(), MAX_HITS, "a common word fills the list");
        let chapters: std::collections::BTreeSet<_> = s.results.iter().map(|f| f.chapter).collect();
        assert!(chapters.len() > 1, "hits from several chapters");
        assert!(s.results.windows(2).all(|w| (w[0].chapter, w[0].range.start) < (w[1].chapter, w[1].range.start)));
        assert!(s.results.iter().all(|f| zarathustra_engine::search::fold(&f.snippet[f.bold.clone()]) == "the"));
        assert!(s.results.iter().any(|f| !f.title.is_empty()), "hits are named after their chapter");
        eprintln!("{steps} steps");

        s.query = "zzqxj".into();
        s.start();
        while s.is_running() {
            s.step(&book, &entries);
        }
        assert!(s.results.is_empty());
    }
}
