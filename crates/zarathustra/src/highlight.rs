//! Highlights in the reading view: long-press selection snapped to words, the action popup,
//! saved highlights drawn under the text, and their anchors in the store.
//!
//! Positions on a page are [`TextPos`] (block + byte in the block's laid-out text). They are
//! turned into DOM positions through the blocks' source maps, stored as a CFI plus a
//! text-quote selector, and resolved back every time a chapter is laid out, so highlights
//! follow any change of font, size or margins.

use std::ops::Range;
use std::path::Path;
use std::time::{Duration, Instant};

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::anchor::{cfi_for, reanchor, resolve_cfi, Anchor, TextMap, TextQuote};
use zarathustra_engine::book::{Book, Chapter};
use zarathustra_engine::dom::{Dom, NodeId};
use zarathustra_engine::layout::flow::Block;
use zarathustra_engine::layout::page::{Page, PageLine};
use zarathustra_engine::library::{BookEntry, Format};
use zarathustra_store::{book_key, Annotation, AnnotationData, BookId, Color, Kind, Store};

use crate::ui::{rounded_rect, Label, INK, PAPER};
use crate::{Ctx, H, W};

/// Hold this long without moving to select a word.
pub const LONG_PRESS: Duration = Duration::from_millis(500);
/// Finger travel (pixels) still counted as holding still.
pub const SLOP: i32 = 24;

/// Background of saved highlights: light enough for black text, dark enough to see.
pub(crate) const HIGHLIGHT_GRAY: u8 = 215;
/// The live selection: highlight gray plus a bar under the line, which still shows with the
/// black-and-white waveform used while dragging.
pub(crate) const SELECTION_BAR: i32 = 5;
const POPUP_H: i32 = 100;
const BUTTON_W: i32 = 240;
/// Clear of the selection knobs, which stick out 2 × HANDLE_R above and below the text.
const POPUP_GAP: i32 = 2 * HANDLE_R + 16;
/// Hold the finger this long past the text's first or last line while selecting to turn the page; the selection carries on there.
pub const EDGE_DWELL: Duration = Duration::from_millis(600);
/// Holding on after a turn turns again only after this long: drawing the new page and e-ink
/// showing it take a few hundred ms of it, and a shorter repeat read as two turns in a row.
pub const EDGE_REPEAT: Duration = Duration::from_millis(1200);
/// Radius of the knobs under the selection's ends, and how near a touch must land to grab one.
const HANDLE_R: i32 = 22;
const GRAB: i32 = 70;

/// Marks band `r` of a selection or an opened search hit over drawn text: the highlight gray
/// and a bar under it, which still shows in the black-and-white waveform.
pub(crate) fn mark(c: &mut Canvas, r: Rect) {
    c.darken(r, HIGHLIGHT_GRAY);
    c.fill(Rect::new(r.x, r.bottom() + 2, r.w, SELECTION_BAR), INK);
}

/// What [`mark`] draws over for band `r`: the band and the bar under it.
pub(crate) fn mark_area(r: Rect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h + 2 + SELECTION_BAR)
}

/// The book's row in `store` (added if new) and its highlights drawn on pages.
pub(crate) fn load(
    store: &Store,
    key: &str,
    entry: &BookEntry,
) -> zarathustra_store::Result<(BookId, Vec<Annotation>)> {
    let id = store.ensure_book(key, entry)?;
    let saved = store.annotations(id)?.into_iter().filter(|a| is_drawn(a.data.kind)).collect();
    Ok((id, saved))
}

/// A position in a laid-out chapter: block index and byte offset in the block's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub block: u32,
    pub offset: u32,
}

impl TextPos {
    pub fn new(block: usize, offset: usize) -> TextPos {
        TextPos { block: u32::try_from(block).unwrap_or(u32::MAX), offset: u32::try_from(offset).unwrap_or(u32::MAX) }
    }
}

pub fn glyph_pos(blocks: &[Block], line: &PageLine, run: u32, offset: u32) -> TextPos {
    let start = blocks.get(line.block as usize).and_then(|b| b.spans.get(run as usize)).map_or(0, |s| s.start);
    TextPos::new(line.block as usize, start + offset as usize)
}

/// Where page `page` starts: its first line's first glyph, or its first image's block.
pub fn page_start(page: &Page, blocks: &[Block]) -> Option<TextPos> {
    let line = page.lines.first().map(|l| match l.glyphs.iter().find(|g| !g.inserted) {
        Some(g) => glyph_pos(blocks, l, g.run, g.offset),
        None => TextPos::new(l.block as usize, 0),
    });
    let image = page.images.first().map(|i| TextPos::new(i.block as usize, 0));
    line.into_iter().chain(image).min()
}

/// Vertical extent of a line's ink (relative to the text area), from its largest glyph.
// ponytail: approximated from the font size; use the faces' ascent/descent if mixed-script
// lines show clipped highlights.
fn line_band(line: &PageLine) -> (f32, f32) {
    let size = line.glyphs.iter().map(|g| g.size).fold(0.0, f32::max);
    (line.baseline - 0.82 * size, line.baseline + 0.28 * size)
}

/// The glyph under (or nearest to) `(x, y)` in text-area coordinates: the closest line within
/// a line's height, then the closest glyph on it.
// ponytail: linear scans over ~30 lines and ~60 glyphs a line, microseconds; binary search
// (spec §8.1.5) only if pages with thousands of glyphs show up.
pub fn hit(page: &Page, blocks: &[Block], x: f32, y: f32) -> Option<TextPos> {
    let dist = |v: f32, lo: f32, hi: f32| if v < lo { lo - v } else { (v - hi).max(0.0) };
    let line = (page.lines.iter().filter(|l| !l.glyphs.is_empty()))
        .map(|l| {
            let (top, bottom) = line_band(l);
            (l, dist(y, top, bottom), bottom - top)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .filter(|&(_, d, h)| d <= h)?
        .0;
    let glyph = (line.glyphs.iter().filter(|g| !g.inserted)).min_by(|a, b| {
        let da = dist(x, line.x + a.x, line.x + a.x + a.advance);
        let db = dist(x, line.x + b.x, line.x + b.x + b.advance);
        da.total_cmp(&db)
    })?;
    Some(glyph_pos(blocks, line, glyph.run, glyph.offset))
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\'' | '’' | '-' | '\u{ad}')
}

/// The word around byte `pos` of `text`, or the single character there when it is not part
/// of a word (punctuation, a symbol).
// ponytail: letters, digits, apostrophes and hyphens make a word; switch to the UAX #29
// word segmenter (icu_segmenter, already a dependency of the engine) for CJK and Thai.
pub fn word_at(text: &str, pos: usize) -> Range<usize> {
    let pos = text.floor_char_boundary(pos.min(text.len()));
    let Some(c) = text[pos..].chars().next() else { return pos..pos };
    if !is_word_char(c) {
        return pos..pos + c.len_utf8();
    }
    let start = text[..pos].char_indices().rev().take_while(|&(_, c)| is_word_char(c)).last().map_or(pos, |(i, _)| i);
    let end = text[pos..].char_indices().find(|&(_, c)| !is_word_char(c)).map_or(text.len(), |(i, _)| pos + i);
    // Quotes and hyphens hug words; they are not part of them at the edges.
    let trim = |c: char| matches!(c, '\'' | '’' | '-' | '\u{ad}');
    let word = &text[start..end];
    let trimmed_start = start + (word.len() - word.trim_start_matches(trim).len());
    let trimmed_end = start + word.trim_end_matches(trim).len();
    if trimmed_start < trimmed_end {
        trimmed_start..trimmed_end
    } else {
        start..end
    }
}

/// Closing punctuation right after a word, taken along when a selection ends there: a missed
/// full stop is the usual reason to select again.
fn closes(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '…' | '»' | '›' | '”' | '’' | '"' | '\'' | ')' | ']')
}

/// The word at `p` with the closing punctuation right after it.
fn with_punct(blocks: &[Block], p: TextPos) -> Range<TextPos> {
    let word = word_range(blocks, p);
    let text = blocks.get(word.end.block as usize).map_or("", |b| b.text.as_str());
    let at = (word.end.offset as usize).min(text.len());
    let tail = text.get(at..).unwrap_or("");
    let n: usize = tail.chars().take_while(|&c| closes(c)).map(char::len_utf8).sum();
    word.start..TextPos::new(word.end.block as usize, at + n)
}

fn word_range(blocks: &[Block], p: TextPos) -> Range<TextPos> {
    let text = blocks.get(p.block as usize).map_or("", |b| b.text.as_str());
    let w = word_at(text, p.offset as usize);
    TextPos::new(p.block as usize, w.start)..TextPos::new(p.block as usize, w.end)
}

/// A selection being made: the word first pressed stays selected while the other end follows
/// the finger, word by word, in either direction.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    anchor: Range<TextPos>,
    pub range: Range<TextPos>,
    /// The finger past the page's text: when the page turns, and whether forward.
    edge: Option<(Instant, bool)>,
}

impl Selection {
    pub fn new(word: Range<TextPos>) -> Selection {
        Selection { anchor: word.clone(), range: word, edge: None }
    }

    /// Moves the free end to `word`.
    pub fn extend(&mut self, word: Range<TextPos>) {
        self.range = if word.start >= self.anchor.start {
            self.anchor.start..word.end.max(self.anchor.end)
        } else {
            word.start..self.anchor.end
        };
    }
}

/// One line's share of a range, in text-area coordinates: left, right, top, bottom.
type Band = (f32, f32, f32, f32);

/// The parts of `range` on each line of `page`.
pub fn range_bands(page: &Page, blocks: &[Block], range: &Range<TextPos>) -> Vec<Band> {
    let mut out = Vec::new();
    for line in &page.lines {
        let first = TextPos::new(line.block as usize, 0);
        let last = TextPos::new(line.block as usize + 1, 0);
        if range.end <= first || range.start >= last {
            continue;
        }
        let mut span: Option<(f32, f32)> = None;
        for g in line.glyphs.iter().filter(|g| !g.inserted) {
            if range.contains(&glyph_pos(blocks, line, g.run, g.offset)) {
                let (l, r) = (line.x + g.x, line.x + g.x + g.advance);
                span = Some(span.map_or((l, r), |(a, b)| (a.min(l), b.max(r))));
            }
        }
        if let Some((l, r)) = span {
            let (top, bottom) = line_band(line);
            out.push((l, r, top, bottom));
        }
    }
    out
}

/// Turns a laid-out range into what the store keeps: CFIs for both ends and a text quote.
pub fn anchor_range(dom: &Dom, blocks: &[Block], spine: u32, range: &Range<TextPos>) -> Option<AnnotationData> {
    let to_dom = |p: TextPos| blocks.get(p.block as usize).and_then(|b| b.source_at(p.offset as usize));
    let (start_node, start_off) = to_dom(range.start)?;
    let (end_node, end_off) = to_dom(range.end)?;
    let map = TextMap::new(dom);
    let (s, e) = (map.to_text(start_node, start_off)?, map.to_text(end_node, end_off)?);
    Some(AnnotationData {
        kind: Kind::Highlight,
        start: Anchor::new(dom, spine, start_node, start_off),
        end_cfi: Some(cfi_for(dom, spine, end_node, end_off)),
        quote: Some(TextQuote::new(map.text(), s..e.max(s))),
        chapter: None,
        page_hint: None,
        note: None,
        color: Color::Gray,
    })
}

/// Block position of a DOM position, searching every block's source map.
pub(crate) fn block_pos(blocks: &[Block], node: NodeId, offset: usize) -> Option<TextPos> {
    blocks.iter().enumerate().find_map(|(i, b)| b.position_of(node, offset).map(|o| TextPos::new(i, o)))
}

/// Where a stored highlight lies in the laid-out chapter: its CFIs when the text there still
/// matches the quote, else wherever the quote is found again (the book was edited).
pub fn resolve(dom: &Dom, map: &TextMap, blocks: &[Block], data: &AnnotationData) -> Option<Range<TextPos>> {
    let quote = data.quote.as_ref();
    let by_cfi = || {
        let (sn, so) = resolve_cfi(dom, &data.start.cfi)?;
        let (en, eo) = resolve_cfi(dom, data.end_cfi.as_deref()?)?;
        let (s, e) = (map.to_text(sn, so)?, map.to_text(en, eo)?);
        let fits = quote.is_none_or(|q| s <= e && TextQuote::new(map.text(), s..e).exact == q.exact);
        fits.then_some(s..e)
    };
    let text = by_cfi().or_else(|| reanchor(map.text(), quote?))?;
    let (sn, so) = map.to_dom(text.start)?;
    // The end leans left: a range ending a paragraph stays in it.
    let (en, eo) = map.to_dom_end(text.end)?;
    let range = block_pos(blocks, sn, so)?..block_pos(blocks, en, eo)?;
    (range.start < range.end).then_some(range)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Highlight,
    Delete,
    Cancel,
}

impl Button {
    fn label(self) -> &'static str {
        match self {
            Button::Highlight => crate::i18n::t().highlight,
            Button::Delete => crate::i18n::t().delete,
            Button::Cancel => crate::i18n::t().cancel,
        }
    }
}

/// The action bar next to a selection (new highlight) or a tapped highlight (index into
/// [`Highlights::saved`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub target: Option<usize>,
    pub rect: Rect,
    pub buttons: Vec<(Button, Rect)>,
}

impl Popup {
    /// Centred over the bands, above them when there is room, else below, else (a page-long
    /// selection) in the middle of the screen, over the text.
    pub(crate) fn new(target: Option<usize>, buttons: &[Button], bands: &[Rect]) -> Popup {
        let w = BUTTON_W * buttons.len() as i32;
        let union = bands.iter().fold(Rect::default(), |u, r| u.union(r));
        let x = (union.x + union.w / 2 - w / 2).clamp(16, W - 16 - w);
        let above = union.y - POPUP_GAP - POPUP_H;
        let below = union.bottom() + POPUP_GAP;
        let y = if above >= crate::menu::TOPBAR_H {
            above
        } else if below + POPUP_H <= H - crate::menu::TOPBAR_H {
            below
        } else {
            (H - POPUP_H) / 2
        };
        let rect = Rect::new(x, y, w, POPUP_H);
        let buttons = buttons
            .iter()
            .enumerate()
            .map(|(i, &b)| (b, Rect::new(x + BUTTON_W * i as i32, y, BUTTON_W, POPUP_H)))
            .collect();
        Popup { target, rect, buttons }
    }

    pub(crate) fn button_at(&self, (x, y): (i32, i32)) -> Option<Button> {
        self.buttons.iter().find(|(_, r)| r.contains(x, y)).map(|&(b, _)| b)
    }
}

/// What a touch did to the highlights layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not for us: the reader handles it (page turn, menu).
    Ignored,
    /// Handled; redraw this screen rectangle.
    Redraw(Rect),
}

/// The highlights of the open book and the selection in progress.
#[derive(Default)]
pub struct Highlights {
    store: Option<(Store, BookId)>,
    /// The book's highlights, in reading order.
    saved: Vec<Annotation>,
    /// Highlights found in the laid-out chapter: index into `saved`, range.
    resolved: Vec<(usize, Range<TextPos>)>,
    spine: u32,
    selection: Option<Selection>,
    popup: Option<Popup>,
    /// Table-of-contents title of the page shown, stored with new highlights for lists.
    pub chapter_title: Option<String>,
}

impl Highlights {
    /// Opens the database in `state_dir` and loads `book`'s highlights. Without a database
    /// (unwritable state dir) highlighting still works for the session, it is just not saved.
    pub fn open(path: &Path, book: &Book, state_dir: &Path) -> Highlights {
        let store = Store::open(&state_dir.join("zarathustra.db")).and_then(|store| {
            let meta = book.metadata();
            // ponytail: without a dc:identifier the key hashes the full path (books with the
            // same file name stay apart), so a moved file loses its highlights; hash the OPF
            // bytes once the book exposes them.
            let key = book_key(meta.identifier.as_deref(), path.as_os_str().as_encoded_bytes());
            let entry = BookEntry {
                path: path.to_path_buf(),
                title: meta.title.clone().unwrap_or_default(),
                authors: meta.authors.clone(),
                language: meta.language.clone(),
                series: meta.series.clone(),
                series_index: meta.series_index,
                format: Format::from_path(path).unwrap_or(Format::Epub),
                size: 0,
                modified: std::time::SystemTime::UNIX_EPOCH,
                cover_thumb: None,
            };
            let (id, saved) = load(&store, &key, &entry)?;
            Ok(((store, id), saved))
        });
        let (store, saved) = match store {
            Ok((store, saved)) => (Some(store), saved),
            Err(e) => {
                eprintln!("highlights: {e}");
                (None, Vec::new())
            }
        };
        Highlights { store, saved, ..Highlights::default() }
    }

    /// Finds this chapter's highlights in its new layout; drops any selection.
    pub fn chapter_loaded(&mut self, chapter: &Chapter, spine: u32) {
        self.spine = spine;
        self.selection = None;
        self.popup = None;
        let map = TextMap::new(&chapter.dom);
        self.resolved = (self.saved.iter().enumerate())
            .filter(|(_, a)| a.data.start.spine == spine)
            .filter_map(|(i, a)| Some((i, resolve(&chapter.dom, &map, &chapter.blocks, &a.data)?)))
            .collect();
    }

    /// The book's database row, shared with the bookmarks; `None` without a database.
    pub fn store(&self) -> Option<(&Store, BookId)> {
        self.store.as_ref().map(|(s, b)| (s, *b))
    }

    /// Logs `secs` of reading and `pages` pages read (1 for a forward turn, else 0) for today
    /// in the reading statistics.
    pub fn log_page(&self, secs: u32, pages: u32) {
        let Some((store, book)) = &self.store else { return };
        if let Err(e) = store.add_reading(*book, crate::clock::local_now().0, secs, pages) {
            eprintln!("stats: {e}");
        }
    }

    /// A selection or its popup is showing: page turns wait.
    pub fn is_active(&self) -> bool {
        self.selection.is_some() || self.popup.is_some()
    }

    /// Where button `b` of the open popup is on screen.
    pub fn button(&self, b: Button) -> Option<Rect> {
        self.popup.as_ref()?.buttons.iter().find(|(x, _)| *x == b).map(|&(_, r)| r)
    }

    /// Clears the selection and popup (page turned, menu opened).
    pub fn dismiss(&mut self) {
        self.selection = None;
        self.popup = None;
    }

    /// Long press at screen `(x, y)`: selects the word there. `origin` is the text area's
    /// top-left corner on screen.
    pub fn long_press(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32), (x, y): (i32, i32)) -> Outcome {
        let Some(p) = chapter.pages.get(page) else { return Outcome::Ignored };
        if self.popup.as_ref().is_some_and(|popup| popup.rect.contains(x, y)) {
            return Outcome::Ignored; // a press on the popup is for its buttons
        }
        let Some(pos) = hit(p, &chapter.blocks, (x - origin.0) as f32, (y - origin.1) as f32) else {
            return Outcome::Ignored;
        };
        // The popup and selection this press replaces are erased too.
        let mut damage = self.popup.take().map_or(Rect::default(), |popup| popup.rect);
        if let Some(old) = self.selection.take() {
            damage = damage.union(&self.bounds(p, &chapter.blocks, &old.range, origin));
        }
        let sel = Selection::new(word_range(&chapter.blocks, pos));
        let damage = damage.union(&self.bounds(p, &chapter.blocks, &sel.range, origin));
        self.selection = Some(sel);
        Outcome::Redraw(damage)
    }

    /// The finger moved while selecting: extends the selection to the word under it.
    pub fn drag(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32), (x, y): (i32, i32)) -> Outcome {
        let (Some(p), Some(sel)) = (chapter.pages.get(page), self.selection.as_mut()) else {
            return Outcome::Ignored;
        };
        let dir = edge_dir(p, origin, y);
        if sel.edge.map(|(_, f)| f) != dir {
            sel.edge = dir.map(|f| (Instant::now() + EDGE_DWELL, f));
        }
        let Some(pos) = hit(p, &chapter.blocks, (x - origin.0) as f32, (y - origin.1) as f32) else {
            return Outcome::Redraw(Rect::default());
        };
        let before = sel.range.clone();
        sel.extend(with_punct(&chapter.blocks, pos));
        if sel.range == before {
            return Outcome::Redraw(Rect::default());
        }
        let after = sel.range.clone();
        // ponytail: damage is the bounding box of old and new selection, not their symmetric
        // difference per line (spec §8.1.6); fine while selections stay a few lines tall.
        let damage =
            self.bounds(p, &chapter.blocks, &before, origin).union(&self.bounds(p, &chapter.blocks, &after, origin));
        Outcome::Redraw(damage)
    }

    /// How long until the finger held at a page edge turns the page, while selecting.
    pub fn edge_wait(&self) -> Option<Duration> {
        let (due, _) = self.selection.as_ref()?.edge?;
        Some(due.saturating_duration_since(Instant::now()))
    }

    /// The finger held at a page edge long enough: the page to turn to (same chapter only),
    /// with the selection stretched to that page's last word (forward) or first (back) and
    /// the dwell restarted, so holding on keeps turning.
    // ponytail: stops at the chapter's ends (a chapter load drops the selection); carry the
    // selection across chapters if highlights spanning them are wanted.
    pub fn edge_turn(&mut self, chapter: &Chapter, page: usize) -> Option<usize> {
        let sel = self.selection.as_mut()?;
        let (_, forward) = sel.edge.filter(|(due, _)| *due <= Instant::now())?;
        let next = if forward { page.checked_add(1) } else { page.checked_sub(1) };
        let Some((next, p)) = next.and_then(|n| Some((n, chapter.pages.get(n)?))) else {
            // Nowhere to go: stop timing until the finger moves out and back.
            sel.edge = None;
            return None;
        };
        sel.edge = Some((Instant::now() + EDGE_REPEAT, forward));
        let last = |l: &PageLine| {
            let g = l.glyphs.iter().rfind(|g| !g.inserted)?;
            Some(glyph_pos(&chapter.blocks, l, g.run, g.offset))
        };
        let pos = if forward { p.lines.iter().rev().find_map(last) } else { page_start(p, &chapter.blocks) };
        if let Some(pos) = pos {
            sel.extend(with_punct(&chapter.blocks, pos));
        }
        Some(next)
    }

    /// The selection's ends on page `page`, on screen: the knob under its start and its end
    /// (`true`), for the ends that are on this page.
    fn handles(&self, chapter: &Chapter, page: usize, origin: (i32, i32)) -> Vec<((i32, i32), bool)> {
        let (Some(p), Some(sel)) = (chapter.pages.get(page), self.selection.as_ref()) else { return Vec::new() };
        let bands = screen_bands(p, &chapter.blocks, &sel.range, origin);
        let starts_here = page_start(p, &chapter.blocks).is_none_or(|s| sel.range.start >= s);
        let ends_here =
            chapter.pages.get(page + 1).and_then(|n| page_start(n, &chapter.blocks)).is_none_or(|e| sel.range.end <= e);
        let start = bands.first().filter(|_| starts_here).map(|r| ((r.x, r.y), false));
        let end = bands.last().filter(|_| ends_here).map(|r| ((r.right(), r.bottom()), true));
        start.into_iter().chain(end).collect()
    }

    /// A finger down on a knob of the selection (its actions showing): that end follows the
    /// finger from now on, the other stays. Returns the area to refresh.
    pub fn grab(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32), (x, y): (i32, i32)) -> Outcome {
        // Only a new selection's actions show knobs; a touch on the bar is for its buttons.
        if self.popup.as_ref().is_none_or(|p| p.target.is_some() || p.rect.contains(x, y)) {
            return Outcome::Ignored;
        }
        let handles = self.handles(chapter, page, origin);
        // Measured from the drop's round part, which hangs off the tip.
        let near = handles.iter().find(|((hx, hy), end)| {
            let d = if *end { HANDLE_R } else { -HANDLE_R };
            (hx + d - x).pow(2) + (hy + d - y).pow(2) <= GRAB * GRAB
        });
        let Some(&(_, end)) = near else { return Outcome::Ignored };
        let (Some(p), Some(sel)) = (chapter.pages.get(page), self.selection.as_mut()) else {
            return Outcome::Ignored;
        };
        // The word at the end that stays becomes the anchor; `extend` then moves the other.
        sel.anchor =
            if end { word_range(&chapter.blocks, sel.range.start) } else { last_word(&chapter.blocks, sel.range.end) };
        let range = sel.range.clone();
        let damage = self.popup.take().map_or(Rect::default(), |popup| popup.rect);
        Outcome::Redraw(damage.union(&self.bounds(p, &chapter.blocks, &range, origin)))
    }

    /// The finger lifted after selecting: shows the action bar.
    pub fn release(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32)) -> Outcome {
        let (Some(p), Some(sel)) = (chapter.pages.get(page), self.selection.as_mut()) else {
            return Outcome::Ignored;
        };
        sel.edge = None;
        let range = sel.range.clone();
        let bands = screen_bands(p, &chapter.blocks, &range, origin);
        let popup = Popup::new(None, &[Button::Highlight, Button::Cancel], &bands);
        // Knobs included.
        let damage = popup.rect.union(&self.bounds(p, &chapter.blocks, &range, origin));
        self.popup = Some(popup);
        Outcome::Redraw(damage)
    }

    /// The selected word, when the selection is one word (the finger did not drag) with a
    /// letter in it: what a dictionary can define.
    pub fn selected_word(&self, chapter: &Chapter) -> Option<String> {
        let sel = self.selection.as_ref().filter(|s| s.range == s.anchor)?;
        let block = chapter.blocks.get(sel.range.start.block as usize)?;
        let word = block.text.get(sel.range.start.offset as usize..sel.range.end.offset as usize)?;
        word.chars().any(char::is_alphabetic).then(|| word.to_string())
    }

    /// Screen rectangle of the selection on page `page`.
    pub fn selection_rect(&self, chapter: &Chapter, page: usize, origin: (i32, i32)) -> Option<Rect> {
        let (p, sel) = (chapter.pages.get(page)?, self.selection.as_ref()?);
        Some(self.bounds(p, &chapter.blocks, &sel.range, origin))
    }

    /// Ends the selection, saving it as a highlight when `save`; the screen area to refresh.
    pub fn end_selection(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32), save: bool) -> Rect {
        let rect = self.selection_rect(chapter, page, origin).unwrap_or_default();
        if let Some(sel) = self.selection.take() {
            if save {
                self.save(chapter, page, &sel.range);
            }
        }
        self.popup = None;
        rect
    }

    /// A tap: a popup button, a tap outside the popup (cancels), or a tap on a saved
    /// highlight (opens its popup). Anything else is the reader's.
    pub fn tap(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32), (x, y): (i32, i32)) -> Outcome {
        let Some(p) = chapter.pages.get(page) else { return Outcome::Ignored };
        if let Some(popup) = self.popup.take() {
            let mut damage = popup.rect;
            if let Some(sel) = self.selection.take() {
                damage = damage.union(&self.bounds(p, &chapter.blocks, &sel.range, origin));
                if popup.button_at((x, y)) == Some(Button::Highlight) {
                    self.save(chapter, page, &sel.range);
                }
            } else if let Some(i) = popup.target {
                if let Some((_, r)) = self.resolved.iter().find(|(j, _)| *j == i) {
                    damage = damage.union(&self.bounds(p, &chapter.blocks, r, origin));
                }
                if popup.button_at((x, y)) == Some(Button::Delete) {
                    self.delete(i);
                }
            }
            return Outcome::Redraw(damage);
        }
        if self.selection.take().is_some() {
            return Outcome::Redraw(Rect::new(0, 0, W, crate::H));
        }
        let Some(pos) = hit(p, &chapter.blocks, (x - origin.0) as f32, (y - origin.1) as f32) else {
            return Outcome::Ignored;
        };
        let Some((i, range)) = self.resolved.iter().find(|(_, r)| r.contains(&pos)).cloned() else {
            return Outcome::Ignored;
        };
        let bands = screen_bands(p, &chapter.blocks, &range, origin);
        let popup = Popup::new(Some(i), &[Button::Delete, Button::Cancel], &bands);
        let damage = popup.rect;
        self.popup = Some(popup);
        Outcome::Redraw(damage)
    }

    fn save(&mut self, chapter: &Chapter, page: usize, range: &Range<TextPos>) {
        let Some(mut data) = anchor_range(&chapter.dom, &chapter.blocks, self.spine, range) else {
            eprintln!("highlights: selection has no DOM anchor");
            return;
        };
        // The page the highlight starts on: a selection may have been carried to a later one.
        let starts =
            chapter.pages.iter().rposition(|p| page_start(p, &chapter.blocks).is_some_and(|s| s <= range.start));
        data.page_hint = u32::try_from(starts.unwrap_or(page) + 1).ok();
        data.chapter = self.chapter_title.clone();
        let annotation = match &self.store {
            Some((store, book)) => match store.add_annotation(*book, &data) {
                Ok(a) => a,
                Err(e) => return eprintln!("highlights: {e}"),
            },
            // No database: keep it for this session.
            None => Annotation { id: String::new(), book: BookId(0), data, created_at: 0, updated_at: 0 },
        };
        self.saved.push(annotation);
        self.resolved.push((self.saved.len() - 1, range.clone()));
    }

    fn delete(&mut self, i: usize) {
        let Some(a) = self.saved.get(i) else { return };
        if let Some((store, _)) = &self.store {
            if let Err(e) = store.delete_annotation(&a.id) {
                return eprintln!("highlights: {e}");
            }
        }
        self.saved.remove(i);
        self.resolved.retain(|(j, _)| *j != i);
        for (j, _) in &mut self.resolved {
            if *j > i {
                *j -= 1;
            }
        }
    }

    /// Screen rectangle covering `range` on page `p` (selection bar included).
    fn bounds(&self, p: &Page, blocks: &[Block], range: &Range<TextPos>, origin: (i32, i32)) -> Rect {
        let r = screen_bands(p, blocks, range, origin).iter().fold(Rect::default(), |u, r| u.union(r));
        // The knobs hang off the ends.
        if r.is_empty() {
            r
        } else {
            Rect::new(r.x - 2 * HANDLE_R, r.y - 2 * HANDLE_R, r.w + 4 * HANDLE_R, r.h + 4 * HANDLE_R)
        }
    }

    /// Draws saved highlights; call before the page's glyphs.
    pub fn render_under(&self, c: &mut Canvas, chapter: &Chapter, page: usize, origin: (i32, i32)) {
        let Some(p) = chapter.pages.get(page) else { return };
        for (_, range) in &self.resolved {
            for r in screen_bands(p, &chapter.blocks, range, origin) {
                c.fill(Rect::new(r.x, r.y, r.w, r.h - SELECTION_BAR), HIGHLIGHT_GRAY);
            }
        }
    }

    /// Draws the selection over the drawn page: its lines tinted the highlight gray, a bar
    /// under each. Over the text, so a page drawn once can take every frame of a drag.
    pub fn render_selection(&self, c: &mut Canvas, chapter: &Chapter, page: usize, origin: (i32, i32)) {
        let (Some(p), Some(sel)) = (chapter.pages.get(page), &self.selection) else { return };
        for r in screen_bands(p, &chapter.blocks, &sel.range, origin) {
            c.darken(Rect::new(r.x, r.y, r.w, r.h - SELECTION_BAR), HIGHLIGHT_GRAY);
            c.fill(Rect::new(r.x, r.bottom() - SELECTION_BAR, r.w, SELECTION_BAR), INK);
        }
    }

    /// The finger is selecting (no actions showing yet): the page under it doesn't change.
    pub fn is_dragging(&self) -> bool {
        self.selection.is_some() && self.popup.is_none()
    }

    /// Draws the popup; call after the page.
    pub fn render_over(&self, c: &mut Canvas, ctx: &mut Ctx, chapter: &Chapter, page: usize, origin: (i32, i32)) {
        let Some(popup) = &self.popup else { return };
        if popup.target.is_none() {
            for ((x, y), end) in self.handles(chapter, page, origin) {
                crate::ui::drop_knob(c, (x, y), HANDLE_R, end, INK);
            }
        }
        popup.render(c, ctx);
    }
}

impl Popup {
    /// The black bar and its buttons.
    pub(crate) fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        rounded_rect(c, self.rect, 20, INK);
        let Ctx { fonts, raster, .. } = ctx;
        let Some(face) = crate::sans(fonts, true) else { return };
        let label = Label { face, size: 36.0, ink: PAPER };
        for (i, (b, r)) in self.buttons.iter().enumerate() {
            if i > 0 {
                c.fill(Rect::new(r.x, r.y + 24, 2, r.h - 48), 120);
            }
            label.centered(c, raster, b.label(), *r);
        }
    }
}

/// Whether screen row `y` is past page `p`'s text: below its last line (forward) or above its
/// first (back). Never on a page without text, where any row would count.
fn edge_dir(p: &Page, origin: (i32, i32), y: i32) -> Option<bool> {
    let mut lines = p.lines.iter().filter(|l| !l.glyphs.is_empty());
    let first = lines.next()?;
    let (top, bottom) = (line_band(first).0, line_band(lines.next_back().unwrap_or(first)).1);
    let y_text = (y - origin.1) as f32;
    if y_text > bottom + SLOP as f32 {
        Some(true)
    } else if y_text < top - SLOP as f32 {
        Some(false)
    } else {
        None
    }
}

/// The word ending a selection that ends at `end`, its closing punctuation included.
fn last_word(blocks: &[Block], end: TextPos) -> Range<TextPos> {
    let text = blocks.get(end.block as usize).map_or("", |b| b.text.as_str());
    let before = text.get(..end.offset as usize).unwrap_or("").trim_end_matches(closes);
    let at = before.char_indices().next_back().map_or(0, |(i, _)| i);
    word_range(blocks, TextPos::new(end.block as usize, at)).start..end
}

fn is_drawn(kind: Kind) -> bool {
    matches!(kind, Kind::Highlight | Kind::Underline | Kind::Note)
}

/// Screen rectangles of `range` on page `p`, one per line, with room for the selection bar.
fn screen_bands(p: &Page, blocks: &[Block], range: &Range<TextPos>, origin: (i32, i32)) -> Vec<Rect> {
    range_bands(p, blocks, range)
        .into_iter()
        .map(|(l, r, t, b)| {
            let (x0, x1) = (origin.0 + l.floor() as i32 - 2, origin.0 + r.ceil() as i32 + 2);
            let (y0, y1) = (origin.1 + t.floor() as i32, origin.1 + b.ceil() as i32 + SELECTION_BAR + 2);
            Rect::new(x0, y0, x1 - x0, y1 - y0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zarathustra_engine::layout::flow;
    use zarathustra_engine::layout::paragraph::Placed;
    use zarathustra_engine::style::cascade::{compute, Preferences, Stylesheets};

    const CHAPTER: &str = r#"<html><body>
      <h1 id="c1">Capitolo  primo</h1>
      <p>Nel mezzo del <i>cammin</i> di nostra vita
         mi ritrovai per una selva oscura,</p>
      <p>ché la diritta via era smarrita.</p>
    </body></html>"#;

    #[test]
    fn popup_stays_on_screen() {
        let b = [Button::Delete, Button::Cancel];
        let line = |y| Rect::new(100, y, 800, 40);
        assert!(Popup::new(None, &b, &[line(800)]).rect.bottom() < 800, "above when there is room");
        assert!(Popup::new(None, &b, &[line(160)]).rect.y > 200, "below under the top bar");
        let page = Popup::new(None, &b, &[line(160), line(1500)]).rect;
        assert!(page.y > 0 && page.bottom() < H, "a page-long selection: on screen, {page:?}");
    }

    fn chapter(src: &str) -> (Dom, Vec<Block>) {
        let dom = Dom::parse_xml(src).expect("parses");
        let prefs = Preferences {
            font_size: 20.0,
            font_family: None,
            line_height: None,
            align: None,
            hyphenate: true,
            page_width: 500.0,
        };
        let styles = compute(&dom, &Stylesheets::default(), &prefs);
        let blocks = flow::blocks(&dom, &styles, Some("it"));
        (dom, blocks)
    }

    /// Backdates the edge dwell, as if the finger had held still past it.
    fn expire(h: &mut Highlights) {
        let sel = h.selection.as_mut().unwrap();
        sel.edge = sel.edge.map(|(_, f)| (Instant::now(), f));
    }

    fn pos_of(blocks: &[Block], needle: &str) -> Range<TextPos> {
        let (i, at) = blocks.iter().enumerate().find_map(|(i, b)| Some((i, b.text.find(needle)?))).expect("in text");
        TextPos::new(i, at)..TextPos::new(i, at + needle.len())
    }

    /// One line per block, one 10 px glyph per byte, baselines 40 px apart.
    fn page(blocks: &[Block]) -> Page {
        let lines = (blocks.iter().enumerate())
            .map(|(i, b)| PageLine {
                block: i as u32,
                x: 0.0,
                baseline: 40.0 * (i as f32 + 1.0),
                glyphs: (b.text.char_indices())
                    .filter(|&(_, c)| c != ' ')
                    .map(|(o, _)| Placed {
                        run: (b.spans.iter().rposition(|s| s.start <= o).unwrap_or(0)) as u32,
                        offset: (o - b.spans.iter().rfind(|s| s.start <= o).map_or(0, |s| s.start)) as u32,
                        face: 0,
                        gid: 0,
                        size: 30.0,
                        x: 10.0 * o as f32,
                        y: 0.0,
                        advance: 10.0,
                        inserted: false,
                    })
                    .collect(),
            })
            .collect();
        Page { lines, images: Vec::new() }
    }

    #[test]
    fn words_snap_to_letters_apostrophes_and_hyphens() {
        let t = "l'aria, ben-noto «così» — fine.";
        let w = |at: &str| &t[word_at(t, t.find(at).expect("x"))];
        assert_eq!(w("aria"), "l'aria");
        assert_eq!(w("noto"), "ben-noto");
        assert_eq!(w("così"), "così");
        assert_eq!(w("«"), "«");
        assert_eq!(w(","), ",");
        assert_eq!(w("ine."), "fine");
        assert_eq!(word_at(t, t.len()), t.len()..t.len());
        assert_eq!(word_at("'tis", 0), 1.."'tis".len());
    }

    #[test]
    fn selection_extends_word_by_word_both_ways() {
        let (_, blocks) = chapter(CHAPTER);
        let mut sel = Selection::new(pos_of(&blocks, "cammin"));
        sel.extend(pos_of(&blocks, "nostra"));
        assert_eq!(sel.range, pos_of(&blocks, "cammin di nostra"));
        // Back before the anchor: the anchor word stays, the start moves.
        sel.extend(pos_of(&blocks, "mezzo"));
        assert_eq!(sel.range, pos_of(&blocks, "mezzo del cammin"));
        // Into the next paragraph.
        sel.extend(pos_of(&blocks, "diritta"));
        assert_eq!(sel.range.start, pos_of(&blocks, "cammin").start);
        assert_eq!(sel.range.end, pos_of(&blocks, "diritta").end);
        // Back onto the anchor itself.
        sel.extend(pos_of(&blocks, "cammin"));
        assert_eq!(sel.range, pos_of(&blocks, "cammin"));
    }

    #[test]
    fn hit_testing_finds_words_and_bands_cover_them() {
        let (_, blocks) = chapter(CHAPTER);
        let page = page(&blocks);
        let target = pos_of(&blocks, "cammin");
        // Middle of "cammin" (block 1) on line 2 (baseline 80).
        let x = 10.0 * (target.start.offset as f32 + 2.5);
        let p = hit(&page, &blocks, x, 70.0).expect("hit");
        assert_eq!(word_range(&blocks, p), target);
        // Far to the right of a line: its last glyph; far below the text: nothing.
        let end = hit(&page, &blocks, 5000.0, 80.0).expect("hit");
        assert_eq!(&blocks[1].text[word_at(&blocks[1].text, end.offset as usize)], ",");
        assert_eq!(hit(&page, &blocks, 10.0, 500.0), None);

        let bands = range_bands(&page, &blocks, &pos_of(&blocks, "cammin di"));
        assert_eq!(bands.len(), 1);
        let (l, r, top, bottom) = bands[0];
        assert_eq!((l, r), (10.0 * target.start.offset as f32, 10.0 * (target.start.offset as f32 + 9.0)));
        assert!(top < 80.0 && bottom > 80.0);
        // A range across paragraphs has a band on each line it touches.
        let across = pos_of(&blocks, "cammin").start..pos_of(&blocks, "diritta").end;
        assert_eq!(range_bands(&page, &blocks, &across).len(), 2);
    }

    #[test]
    fn anchors_round_trip_and_survive_edits() {
        let (dom, blocks) = chapter(CHAPTER);
        let range = pos_of(&blocks, "selva oscura");
        let data = anchor_range(&dom, &blocks, 4, &range).expect("anchors");
        let quote = data.quote.clone().expect("quote");
        assert_eq!(quote.exact, "selva oscura");
        assert!(data.start.cfi.starts_with("epubcfi(/6/10!/2/4/3:"), "{}", data.start.cfi);
        let map = TextMap::new(&dom);
        assert_eq!(resolve(&dom, &map, &blocks, &data), Some(range.clone()));

        // A range spanning an inline element and a whitespace run in the source.
        let span = pos_of(&blocks, "del cammin di nostra vita mi");
        let d = anchor_range(&dom, &blocks, 0, &span).expect("anchors");
        assert_eq!(resolve(&dom, &map, &blocks, &d), Some(span));

        // Ranges ending exactly at a paragraph's end (inter-block whitespace follows).
        for end in ["selva oscura,", "era smarrita."] {
            let r = pos_of(&blocks, end);
            assert_eq!(r.end.offset as usize, blocks[r.end.block as usize].text.len(), "{end} ends its block");
            let d = anchor_range(&dom, &blocks, 0, &r).expect("anchors");
            assert_eq!(resolve(&dom, &map, &blocks, &d), Some(r), "{end}");
        }

        // The book changed: markup inserted before (CFI points elsewhere), text reflowed.
        let edited = CHAPTER.replace("<p>Nel", "<p>Premessa.</p><p>Nel").replace("selva oscura", "selva\n oscura");
        let (dom2, blocks2) = chapter(&edited);
        let map2 = TextMap::new(&dom2);
        let r = resolve(&dom2, &map2, &blocks2, &data).expect("re-anchored");
        let b = &blocks2[r.start.block as usize];
        assert_eq!(&b.text[r.start.offset as usize..r.end.offset as usize], "selva oscura");

        // The text is gone: no range.
        let (dom3, blocks3) = chapter(&CHAPTER.replace("per una selva oscura", "altrove"));
        assert_eq!(resolve(&dom3, &TextMap::new(&dom3), &blocks3, &data), None);
    }

    #[test]
    fn highlights_save_resolve_and_delete_through_the_store() {
        let (dom, blocks) = chapter(CHAPTER);
        let page = page(&blocks);
        let chapter = Chapter { dom, blocks, pages: vec![page] };
        let mut h = Highlights::default();
        let store = Store::open_in_memory().expect("db");
        let entry = BookEntry {
            path: "x.epub".into(),
            title: "x".into(),
            authors: Vec::new(),
            language: None,
            series: None,
            series_index: None,
            format: Format::Epub,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            cover_thumb: None,
        };
        let book = store.ensure_book("id:x", &entry).expect("book");
        h.store = Some((store, book));
        h.chapter_loaded(&chapter, 0);

        let target = pos_of(&chapter.blocks, "cammin");
        let at = |p: TextPos| (10 * p.offset as i32 + 25, 40 * (p.block as i32 + 1) - 10);
        // Long press, drag to "nostra", release, tap Highlight.
        assert!(matches!(h.long_press(&chapter, 0, (0, 0), at(target.start)), Outcome::Redraw(_)));
        h.drag(&chapter, 0, (0, 0), at(pos_of(&chapter.blocks, "nostra").start));
        h.release(&chapter, 0, (0, 0));
        let button = |h: &Highlights, b| {
            let (_, r) = *h.popup.as_ref().expect("popup").buttons.iter().find(|(x, _)| *x == b).expect("button");
            (r.x + 1, r.y + 1)
        };
        let save = button(&h, Button::Highlight);
        h.tap(&chapter, 0, (0, 0), save);
        assert!(!h.is_active());
        let (store, book) = h.store.as_ref().expect("store");
        let saved = store.annotations(*book).expect("list");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].data.quote.as_ref().map(|q| q.exact.as_str()), Some("cammin di nostra"));

        // A fresh layout finds it again; a tap on it offers Delete.
        h.saved = saved;
        h.chapter_loaded(&chapter, 0);
        assert_eq!(h.resolved, [(0, pos_of(&chapter.blocks, "cammin di nostra"))]);
        assert_eq!(h.tap(&chapter, 0, (0, 0), (5000, 500)), Outcome::Ignored);
        assert!(matches!(h.tap(&chapter, 0, (0, 0), at(target.start)), Outcome::Redraw(_)));
        let delete = button(&h, Button::Delete);
        h.tap(&chapter, 0, (0, 0), delete);
        assert!(h.resolved.is_empty() && h.saved.is_empty());
        let (store, book) = h.store.as_ref().expect("store");
        assert!(store.annotations(*book).expect("list").is_empty());
    }

    #[test]
    fn holding_past_the_last_line_turns_the_page_and_the_selection_follows() {
        let (dom, blocks) = chapter(CHAPTER);
        // Page 0: the heading and the first paragraph; page 1: the second paragraph.
        let mut first = page(&blocks);
        let second = Page { lines: first.lines.split_off(2), images: Vec::new() };
        let chapter = Chapter { dom, blocks, pages: vec![first, second] };
        let mut h = Highlights::default();
        let target = pos_of(&chapter.blocks, "cammin");
        h.long_press(&chapter, 0, (0, 0), (10 * target.start.offset as i32 + 25, 70));
        // Within the text: no edge. Below the last line: forward, after the dwell.
        h.drag(&chapter, 0, (0, 0), (100, 70));
        assert_eq!(h.edge_wait(), None);
        h.drag(&chapter, 0, (0, 0), (100, 400));
        assert!(h.edge_wait().is_some());
        assert_eq!(h.edge_turn(&chapter, 0), None, "dwell not over");
        expire(&mut h);
        assert_eq!(h.edge_turn(&chapter, 0), Some(1));
        // Holding on: the next turn waits the longer repeat.
        assert!(h.edge_wait().is_some_and(|w| w > EDGE_DWELL));
        assert_eq!(h.edge_turn(&chapter, 1), None);
        let sel = h.selection.clone().expect("still selecting").range;
        assert_eq!(sel.start, target.start);
        assert_eq!(sel.end, TextPos::new(2, chapter.blocks[2].text.len()));
        // On the new page the free end follows the finger again, back and forth.
        let diritta = pos_of(&chapter.blocks, "diritta");
        h.drag(&chapter, 1, (0, 0), (10 * diritta.start.offset as i32 + 25, 110));
        assert_eq!(h.selection.as_ref().expect("selecting").range, target.start..diritta.end);
        h.drag(&chapter, 1, (0, 0), (100, 400));
        // The last page: nowhere further to go, and no more waking up for it.
        expire(&mut h);
        assert_eq!(h.edge_turn(&chapter, 1), None);
        assert_eq!(h.edge_wait(), None);
        // A page without text has no edge.
        assert_eq!(edge_dir(&Page { lines: Vec::new(), images: Vec::new() }, (0, 0), 400), None);
        // Saved as one highlight, drawn on both pages.
        h.end_selection(&chapter, 1, (0, 0), true);
        assert_eq!(h.resolved.len(), 1);
        let range = &h.resolved[0].1;
        assert!(!range_bands(&chapter.pages[0], &chapter.blocks, range).is_empty());
        assert!(!range_bands(&chapter.pages[1], &chapter.blocks, range).is_empty());
    }

    #[test]
    fn selections_take_closing_punctuation_and_their_ends_can_be_grabbed() {
        let (dom, blocks) = chapter(CHAPTER);
        let chapter = Chapter { pages: vec![page(&blocks)], dom, blocks };
        let mut h = Highlights::default();
        let at = |r: &Range<TextPos>| (10 * r.start.offset as i32 + 15, 40 * (r.start.block as i32 + 1) - 10);
        let (via, smarrita) = (pos_of(&chapter.blocks, "via"), pos_of(&chapter.blocks, "smarrita"));
        h.long_press(&chapter, 0, (0, 0), at(&via));
        h.drag(&chapter, 0, (0, 0), at(&smarrita));
        // Ending on "smarrita" takes the full stop after it.
        assert_eq!(h.selection.as_ref().expect("sel").range, pos_of(&chapter.blocks, "via era smarrita."));
        h.release(&chapter, 0, (0, 0));
        // Away from the knobs: nothing grabbed.
        assert_eq!(h.grab(&chapter, 0, (0, 0), (0, 0)), Outcome::Ignored);
        // The start knob: the end stays, the start follows the finger.
        let knobs = h.handles(&chapter, 0, (0, 0));
        let (start, _) = *knobs.iter().find(|(_, end)| !end).expect("start knob");
        assert!(matches!(h.grab(&chapter, 0, (0, 0), start), Outcome::Redraw(_)));
        assert!(h.popup.is_none());
        h.drag(&chapter, 0, (0, 0), at(&pos_of(&chapter.blocks, "diritta")));
        assert_eq!(h.selection.as_ref().expect("sel").range, pos_of(&chapter.blocks, "diritta via era smarrita."));
        // The end knob dragged back past the start: the start word stays selected.
        h.release(&chapter, 0, (0, 0));
        let knobs = h.handles(&chapter, 0, (0, 0));
        let (end, _) = *knobs.iter().find(|(_, end)| *end).expect("end knob");
        assert!(matches!(h.grab(&chapter, 0, (0, 0), end), Outcome::Redraw(_)));
        h.drag(&chapter, 0, (0, 0), at(&pos_of(&chapter.blocks, "ché")));
        assert_eq!(h.selection.as_ref().expect("sel").range, pos_of(&chapter.blocks, "ché la diritta"));
    }
}
