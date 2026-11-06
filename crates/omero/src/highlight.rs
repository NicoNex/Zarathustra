//! Highlights in the reading view: long-press selection snapped to words, the action popup,
//! saved highlights drawn under the text, and their anchors in the store.
//!
//! Positions on a page are [`TextPos`] (block + byte in the block's laid-out text). They are
//! turned into DOM positions through the blocks' source maps, stored as a CFI plus a
//! text-quote selector, and resolved back every time a chapter is laid out, so highlights
//! follow any change of font, size or margins.

use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use omero_device::{Canvas, Rect};
use omero_engine::anchor::{cfi_for, reanchor, resolve_cfi, Anchor, TextMap, TextQuote};
use omero_engine::book::{Book, Chapter};
use omero_engine::dom::{Dom, NodeId};
use omero_engine::layout::flow::Block;
use omero_engine::layout::page::{Page, PageLine};
use omero_engine::library::{BookEntry, Format};
use omero_store::{book_key, Annotation, AnnotationData, BookId, Color, Kind, Store};

use crate::ui::{rounded_rect, Label, INK, PAPER};
use crate::{Ctx, W};

/// Hold this long without moving to select a word.
pub const LONG_PRESS: Duration = Duration::from_millis(500);
/// Finger travel (pixels) still counted as holding still.
pub const SLOP: i32 = 24;

/// Background of saved highlights: light enough for black text, dark enough to see.
const HIGHLIGHT_GRAY: u8 = 215;
/// The live selection: highlight gray plus a bar under the line, which still shows with the
/// black-and-white waveform used while dragging.
const SELECTION_BAR: i32 = 5;
const POPUP_H: i32 = 100;
const BUTTON_W: i32 = 240;
const POPUP_GAP: i32 = 28;

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
}

impl Selection {
    pub fn new(word: Range<TextPos>) -> Selection {
        Selection { anchor: word.clone(), range: word }
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
fn block_pos(blocks: &[Block], node: NodeId, offset: usize) -> Option<TextPos> {
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
    /// Centred over the bands, above them when there is room, else below.
    fn new(target: Option<usize>, buttons: &[Button], bands: &[Rect]) -> Popup {
        let w = BUTTON_W * buttons.len() as i32;
        let union = bands.iter().fold(Rect::default(), |u, r| u.union(r));
        let x = (union.x + union.w / 2 - w / 2).clamp(16, W - 16 - w);
        let above = union.y - POPUP_GAP - POPUP_H;
        let y = if above >= crate::menu::TOPBAR_H { above } else { union.bottom() + POPUP_GAP };
        let rect = Rect::new(x, y, w, POPUP_H);
        let buttons = buttons
            .iter()
            .enumerate()
            .map(|(i, &b)| (b, Rect::new(x + BUTTON_W * i as i32, y, BUTTON_W, POPUP_H)))
            .collect();
        Popup { target, rect, buttons }
    }

    fn button_at(&self, (x, y): (i32, i32)) -> Option<Button> {
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
        let store = Store::open(&state_dir.join("omero.db")).and_then(|store| {
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
            let id = store.ensure_book(&key, &entry)?;
            Ok((store, id))
        });
        let store = store.map_err(|e| eprintln!("highlights: {e}")).ok();
        let saved =
            store.as_ref().and_then(|(s, id)| s.annotations(*id).map_err(|e| eprintln!("highlights: {e}")).ok());
        let saved = saved.unwrap_or_default().into_iter().filter(|a| is_drawn(a.data.kind)).collect();
        Highlights { store, saved, resolved: Vec::new(), spine: 0, selection: None, popup: None, chapter_title: None }
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

    /// Logs `secs` of reading and one page for today in the reading statistics.
    pub fn log_page(&self, secs: u32) {
        let Some((store, book)) = &self.store else { return };
        if let Err(e) = store.add_reading(*book, crate::clock::local_now().0, secs, 1) {
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
        let Some(pos) = hit(p, &chapter.blocks, (x - origin.0) as f32, (y - origin.1) as f32) else {
            return Outcome::Redraw(Rect::default());
        };
        let before = sel.range.clone();
        sel.extend(word_range(&chapter.blocks, pos));
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

    /// The finger lifted after selecting: shows the action bar.
    pub fn release(&mut self, chapter: &Chapter, page: usize, origin: (i32, i32)) -> Outcome {
        let (Some(p), Some(sel)) = (chapter.pages.get(page), self.selection.as_ref()) else {
            return Outcome::Ignored;
        };
        let bands = screen_bands(p, &chapter.blocks, &sel.range, origin);
        let popup = Popup::new(None, &[Button::Highlight, Button::Cancel], &bands);
        let damage = popup.rect.union(&bands.iter().fold(Rect::default(), |u, r| u.union(r)));
        self.popup = Some(popup);
        Outcome::Redraw(damage)
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
        data.page_hint = u32::try_from(page + 1).ok();
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
        screen_bands(p, blocks, range, origin).iter().fold(Rect::default(), |u, r| u.union(r))
    }

    /// Draws saved highlights and the selection; call before the page's glyphs.
    pub fn render_under(&self, c: &mut Canvas, chapter: &Chapter, page: usize, origin: (i32, i32)) {
        let Some(p) = chapter.pages.get(page) else { return };
        for (_, range) in &self.resolved {
            for r in screen_bands(p, &chapter.blocks, range, origin) {
                c.fill(Rect::new(r.x, r.y, r.w, r.h - SELECTION_BAR), HIGHLIGHT_GRAY);
            }
        }
        if let Some(sel) = &self.selection {
            for r in screen_bands(p, &chapter.blocks, &sel.range, origin) {
                c.fill(Rect::new(r.x, r.y, r.w, r.h - SELECTION_BAR), HIGHLIGHT_GRAY);
                c.fill(Rect::new(r.x, r.bottom() - SELECTION_BAR, r.w, SELECTION_BAR), INK);
            }
        }
    }

    /// Draws the popup; call after the page.
    pub fn render_over(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let Some(popup) = &self.popup else { return };
        rounded_rect(c, popup.rect, 20, INK);
        let Ctx { fonts, raster, .. } = ctx;
        let Some(face) = crate::sans(fonts, true) else { return };
        let label = Label { face, size: 36.0, ink: PAPER };
        for (i, (b, r)) in popup.buttons.iter().enumerate() {
            if i > 0 {
                c.fill(Rect::new(r.x, r.y + 24, 2, r.h - 48), 120);
            }
            label.centered(c, raster, b.label(), *r);
        }
    }
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
    use omero_engine::layout::flow;
    use omero_engine::layout::paragraph::Placed;
    use omero_engine::style::cascade::{compute, Preferences, Stylesheets};

    const CHAPTER: &str = r#"<html><body>
      <h1 id="c1">Capitolo  primo</h1>
      <p>Nel mezzo del <i>cammin</i> di nostra vita
         mi ritrovai per una selva oscura,</p>
      <p>ché la diritta via era smarrita.</p>
    </body></html>"#;

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
        let mut h = Highlights {
            store: None,
            saved: Vec::new(),
            resolved: Vec::new(),
            spine: 0,
            selection: None,
            popup: None,
            chapter_title: None,
        };
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
}
