//! Moving around a book: bookmarks (tap the page's top-right corner, as a dog-ear), the
//! progress scrubber in the reader menu, and the way back after a jump ("‹ Back to p. 12").
//!
//! Bookmarks are stored like highlights (see [`crate::highlight`]): a CFI for where the page
//! starts plus a text quote of its first words, resolved again whenever a chapter is laid out,
//! so a bookmarked page stays marked after any change of font, size or margins.

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::anchor::{reanchor, resolve_cfi, Anchor, TextMap, TextQuote};
use zarathustra_engine::book::Chapter;
use zarathustra_engine::dom::{Dom, NodeId};
use zarathustra_engine::layout::flow::Block;
use zarathustra_engine::layout::page::Page;
use zarathustra_store::{Annotation, AnnotationData, BookId, Color, Kind, Store};

use crate::highlight::{page_start, TextPos};
use crate::i18n::t;
use crate::reader::MARGIN_BOTTOM;
use crate::toc::Entry;
use crate::ui::{rounded_rect, Label, INK, INK_SOFT, PAPER};
use crate::{Ctx, H, W};

/// The dog-ear: taps in this square at the top-right corner toggle the bookmark. It wins
/// over the top band that opens the menu.
pub const CORNER: i32 = 120;
/// The ribbon drawn on bookmarked pages, hanging from the top edge.
const RIBBON: Rect = Rect { x: W - 96, y: 0, w: 40, h: 88 };
/// Depth of the V cut into the ribbon's tail.
const NOTCH: i32 = 16;
/// Words of the page's opening kept as the bookmark's quote (shown in the list).
const QUOTE_WORDS: usize = 12;
const PILL_W: i32 = 440;
const PILL_CLOSE_W: i32 = 90;
const PILL_H: i32 = 72;
/// Inset of the scrubber's track from its touch area, so the knob fits at both ends.
const KNOB: i32 = 22;
/// Chapter ticks on the track, unless there are too many to tell apart.
const MAX_TICKS: usize = 80;

/// Whether a tap at `(x, y)` lands on the dog-ear corner.
pub fn in_corner((x, y): (i32, i32)) -> bool {
    x >= W - CORNER && y < CORNER
}

/// The screen area the ribbon may cover, to refresh after a toggle.
pub fn ribbon_area() -> Rect {
    Rect::new(W - CORNER, 0, CORNER, CORNER)
}

/// A black ribbon with a notched tail, hanging from the top edge.
pub fn draw_ribbon(c: &mut Canvas) {
    let r = RIBBON;
    c.fill(Rect::new(r.x, r.y, r.w, r.h - NOTCH), INK);
    for d in 0..NOTCH {
        // Both tails narrow towards the centre as the V deepens.
        let side = r.w / 2 - (d + 1) * (r.w / 2) / NOTCH;
        let y = r.bottom() - NOTCH + d;
        c.fill(Rect::new(r.x, y, side, 1), INK);
        c.fill(Rect::new(r.right() - side, y, side, 1), INK);
    }
}

/// Where the text position `pos` is shown: the last page starting at or before it.
pub fn page_of(chapter: &Chapter, pos: TextPos) -> usize {
    let blocks = &chapter.blocks;
    (chapter.pages.iter()).rposition(|p| page_start(p, blocks).is_some_and(|s| s <= pos)).unwrap_or(0)
}

/// Block position of a DOM position, searching every block's source map.
fn block_pos(blocks: &[Block], node: NodeId, offset: usize) -> Option<TextPos> {
    blocks.iter().enumerate().find_map(|(i, b)| b.position_of(node, offset).map(|o| TextPos::new(i, o)))
}

/// End of the first `words` words of `text` from `start` (leading whitespace skipped).
fn words_end(text: &str, start: usize, words: usize) -> usize {
    let mut seen = 0;
    let mut in_word = false;
    for (i, c) in text[start..].char_indices() {
        if c.is_whitespace() {
            if in_word {
                seen += 1;
                if seen == words {
                    return start + i;
                }
            }
            in_word = false;
        } else {
            in_word = true;
        }
    }
    text.len()
}

/// The text positions a page can be anchored at: its start, then its first line's start
/// (a page opening with an image has no DOM text position for the image).
fn page_positions(page: &Page, blocks: &[Block]) -> impl Iterator<Item = TextPos> {
    let line = page.lines.first().and_then(|l| {
        let g = l.glyphs.iter().find(|g| !g.inserted)?;
        let span = blocks.get(l.block as usize)?.spans.get(g.run as usize)?;
        Some(TextPos::new(l.block as usize, span.start + g.offset as usize))
    });
    page_start(page, blocks).into_iter().chain(line)
}

/// What the store keeps for a bookmark on page `page`: its start and its first words.
pub fn anchor_page(chapter: &Chapter, spine: u32, page: usize) -> Option<AnnotationData> {
    let p = chapter.pages.get(page)?;
    let blocks = &chapter.blocks;
    let (block, (node, offset)) = page_positions(p, blocks).find_map(|pos| {
        let block = blocks.get(pos.block as usize)?;
        Some((block, block.source_at(pos.offset as usize)?))
    })?;
    let map = TextMap::new(&chapter.dom);
    let text = map.text();
    let raw = map.to_text(node, offset)?;
    // Anchor on the first word itself, not on whitespace before it.
    let start = text[raw..].find(|c: char| !c.is_whitespace()).map_or(raw, |i| raw + i);
    let (node, offset) = map.to_dom(start)?;
    // The quote stays in the page's first paragraph.
    // ponytail: a <br> inside it joins the lines without a space in the quote shown.
    let block_end = block.source_at(block.text.len()).and_then(|(n, o)| map.to_text(n, o)).unwrap_or(text.len());
    let end = words_end(text, start, QUOTE_WORDS).min(block_end.max(start));
    Some(AnnotationData {
        kind: Kind::Bookmark,
        start: Anchor::new(&chapter.dom, spine, node, offset),
        end_cfi: None,
        quote: Some(TextQuote::new(text, start..end)),
        chapter: None,
        page_hint: u32::try_from(page + 1).ok(),
        note: None,
        color: Color::Gray,
    })
}

/// Where a stored bookmark lies in the laid-out chapter: its CFI when the words there still
/// match its quote, else wherever the quote is found again (the book was edited).
pub fn resolve(dom: &Dom, map: &TextMap, blocks: &[Block], data: &AnnotationData) -> Option<TextPos> {
    let quote = data.quote.as_ref();
    let by_cfi = || {
        let (node, offset) = resolve_cfi(dom, &data.start.cfi)?;
        let s = map.to_text(node, offset)?;
        let fits = quote.is_none_or(|q| {
            let mut here = map.text()[s..].split_whitespace();
            q.exact.split_whitespace().all(|w| here.next() == Some(w))
        });
        fits.then_some((node, offset))
    };
    let (node, offset) = by_cfi().or_else(|| map.to_dom(reanchor(map.text(), quote?)?.start))?;
    block_pos(blocks, node, offset)
}

/// The open book's bookmarks and where those of the laid-out chapter fall.
#[derive(Default)]
pub struct Bookmarks {
    /// In reading order.
    saved: Vec<Annotation>,
    /// Bookmarks found in the laid-out chapter: index into `saved`, position.
    here: Vec<(usize, TextPos)>,
    spine: u32,
}

impl Bookmarks {
    /// The book's bookmarks from the database (none without one: they last the session).
    pub fn load(store: Option<(&Store, BookId)>) -> Bookmarks {
        let saved = store.and_then(|(s, id)| s.annotations(id).map_err(|e| eprintln!("bookmarks: {e}")).ok());
        let saved = saved.unwrap_or_default().into_iter().filter(|a| a.data.kind == Kind::Bookmark).collect();
        Bookmarks { saved, here: Vec::new(), spine: 0 }
    }

    /// Finds this chapter's bookmarks in its new layout.
    pub fn chapter_loaded(&mut self, chapter: &Chapter, spine: u32) {
        self.spine = spine;
        let map = TextMap::new(&chapter.dom);
        self.here = (self.saved.iter().enumerate())
            .filter(|(_, a)| a.data.start.spine == spine)
            .filter_map(|(i, a)| Some((i, resolve(&chapter.dom, &map, &chapter.blocks, &a.data)?)))
            .collect();
    }

    pub fn list(&self) -> &[Annotation] {
        &self.saved
    }

    /// Indices (into [`list`](Self::list)) of the bookmarks shown on page `page`.
    fn on_page(&self, chapter: &Chapter, page: usize) -> Vec<usize> {
        self.here.iter().filter(|&&(_, pos)| page_of(chapter, pos) == page).map(|&(i, _)| i).collect()
    }

    /// Page `page` shows a bookmarked position.
    pub fn marked(&self, chapter: &Chapter, page: usize) -> bool {
        self.here.iter().any(|&(_, pos)| page_of(chapter, pos) == page)
    }

    /// Bookmarks page `page`, or removes its bookmarks when it has some. `title` is the
    /// table-of-contents entry being read, kept for the list. True when the page is now
    /// bookmarked.
    pub fn toggle(
        &mut self,
        store: Option<(&Store, BookId)>,
        chapter: &Chapter,
        page: usize,
        title: Option<String>,
    ) -> bool {
        let on = self.on_page(chapter, page);
        if !on.is_empty() {
            for i in on.into_iter().rev() {
                self.remove(store, i);
            }
            return false;
        }
        let Some(mut data) = anchor_page(chapter, self.spine, page) else {
            // ponytail: an image-only page has no text to anchor; bookmark the next text
            // position instead if such pages turn out to matter.
            eprintln!("bookmarks: page {page} has no text to anchor");
            return false;
        };
        data.chapter = title;
        let a = match store {
            Some((s, book)) => match s.add_annotation(book, &data) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("bookmarks: {e}");
                    return false;
                }
            },
            None => Annotation { id: String::new(), book: BookId(0), data, created_at: 0, updated_at: 0 },
        };
        let key = |a: &Annotation| (a.data.start.spine, a.data.start.pos);
        let at = self.saved.partition_point(|b| key(b) <= key(&a));
        self.saved.insert(at, a);
        self.chapter_loaded(chapter, self.spine);
        true
    }

    /// Deletes bookmark `i` of [`list`](Self::list).
    pub fn remove(&mut self, store: Option<(&Store, BookId)>, i: usize) {
        let Some(a) = self.saved.get(i) else { return };
        if let Some((s, _)) = store.filter(|_| !a.id.is_empty()) {
            if let Err(e) = s.delete_annotation(&a.id) {
                return eprintln!("bookmarks: {e}");
            }
        }
        self.saved.remove(i);
        self.here.retain(|&(j, _)| j != i);
        for (j, _) in &mut self.here {
            if *j > i {
                *j -= 1;
            }
        }
    }
}

/// A place in the book to come back to: chapter, where its page starts, that page's index in
/// the chapter, and how far through the book it is (the pill's label: page numbers restart in
/// every chapter, so they can't name it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    pub chapter: usize,
    pub pos: TextPos,
    pub page: usize,
    pub progress: f32,
}

/// Where the reader was before a jump (contents, bookmark, scrubber, highlight list), offered
/// until it is used, dismissed, or the reader turns back onto that page: reading on from a
/// highlight jumped to (a passage over several pages) keeps the way back.
#[derive(Debug, Default)]
pub struct Back(Option<Mark>);

impl Back {
    /// A jump went from `from` to `to`. Jumps in a row keep the place first left; jumping
    /// back onto it leaves nothing to return to.
    pub fn jumped(&mut self, from: Mark, to: Mark) {
        let same = |a: Mark, b: Mark| (a.chapter, a.page) == (b.chapter, b.page);
        match self.0 {
            Some(m) if same(m, to) => self.0 = None,
            Some(_) => {}
            None if same(from, to) => {}
            None => self.0 = Some(from),
        }
    }

    /// A page turn to `here`: turning back onto the place left makes the way back moot.
    pub fn turned(&mut self, here: Mark) {
        if self.0.is_some_and(|m| (m.chapter, m.page) == (here.chapter, here.page)) {
            self.0 = None;
        }
    }

    /// Forgets the way back (the pill's ×, a new layout whose pages it no longer names).
    pub fn clear(&mut self) {
        self.0 = None;
    }

    pub fn get(&self) -> Option<Mark> {
        self.0
    }

    pub fn take(&mut self) -> Option<Mark> {
        self.0.take()
    }
}

/// The "‹ Back to 37%  ×" pill, centred over the footer.
pub fn pill() -> Rect {
    Rect::new((W - PILL_W) / 2, H - MARGIN_BOTTOM + 24, PILL_W, PILL_H)
}

/// The pill with its paper halo: what drawing or removing it repaints.
pub fn pill_halo() -> Rect {
    let r = pill();
    Rect::new(r.x - 8, r.y - 8, r.w + 16, r.h + 16)
}

/// The pill's ×, which dismisses it.
pub fn pill_close() -> Rect {
    let r = pill();
    Rect::new(r.right() - PILL_CLOSE_W, r.y, PILL_CLOSE_W, r.h)
}

pub fn draw_pill(c: &mut Canvas, ctx: &mut Ctx, progress: f32) {
    let r = pill();
    // A paper halo keeps it apart from the progress bar under it.
    rounded_rect(c, pill_halo(), r.h / 2 + 8, PAPER);
    rounded_rect(c, r, r.h / 2, INK);
    let close = pill_close();
    c.fill(Rect::new(close.x, r.y + 18, 2, r.h - 36), 120);
    let Ctx { fonts, raster, .. } = ctx;
    let Some(face) = crate::sans(fonts, true) else { return };
    let percent = (progress * 100.0).round() as u32;
    let back = Rect::new(r.x, r.y, r.w - PILL_CLOSE_W, r.h);
    Label { face, size: 28.0, ink: PAPER }.centered(c, raster, &crate::i18n::lang().back_to_percent(percent), back);
    Label { face, size: 30.0, ink: PAPER }.centered(c, raster, "×", close);
}

/// The scrubber's track inside its touch area `r`.
fn track(r: Rect) -> Rect {
    Rect::new(r.x + KNOB, r.bottom() - KNOB - 8, r.w - 2 * KNOB, 4)
}

/// The book fraction (0–1) under `x` on the scrubber in `r`.
pub fn fraction(r: Rect, x: i32) -> f32 {
    let t = track(r);
    ((x - t.x) as f32 / t.w.max(1) as f32).clamp(0.0, 1.0)
}

/// The chapter at book fraction `f` of `chapters` equal parts (as [`Reader::progress`]
/// counts them), and how far into it, 0–1.
///
/// [`Reader::progress`]: crate::reader::Reader::progress
// ponytail: chapters weigh the same, so a long chapter moves as fast as a short one; weigh
// by chapter text length once the book exposes it without decoding every chapter.
pub fn target(f: f32, chapters: usize) -> (usize, f32) {
    let x = f.clamp(0.0, 1.0) * chapters.max(1) as f32;
    let chapter = (x.floor() as usize).min(chapters.saturating_sub(1));
    (chapter, (x - chapter as f32).clamp(0.0, 1.0))
}

/// The page `within` (0–1) of the way through a chapter of `pages` pages.
pub fn page_in(within: f32, pages: usize) -> usize {
    ((within * pages as f32) as usize).min(pages.saturating_sub(1))
}

/// The contents title of chapter `chapter`: its first entry, else the last one before it.
pub fn chapter_title(entries: &[Entry], chapter: usize) -> Option<&str> {
    let first = entries.iter().find(|e| e.chapter == chapter);
    first.or_else(|| entries.iter().rfind(|e| e.chapter < chapter)).map(|e| e.title.as_str())
}

/// Draws the scrubber in `r` at book fraction `f`: "Position" (or, while `dragging`, the
/// chapter there, `title`) and the percentage over a track ticked at each of `chapters`.
pub fn draw_scrubber(c: &mut Canvas, ctx: &mut Ctx, r: Rect, f: f32, dragging: bool, title: &str, chapters: usize) {
    c.fill(r, PAPER);
    let Ctx { fonts, raster, .. } = ctx;
    let (Some(sans), Some(sans_bold), Some(serif_bold)) =
        (crate::sans(fonts, false), crate::sans(fonts, true), crate::serif(fonts, true))
    else {
        return;
    };
    let baseline = r.y + 44;
    let percent = format!("{}%", (f * 100.0).round() as u32);
    let pct = Label { face: sans_bold, size: 32.0, ink: INK };
    let pct_w = pct.width(&percent);
    pct.draw(c, raster, &percent, (r.right() - KNOB) as f32 - pct_w, baseline);
    let room = r.w as f32 - pct_w - 2.0 * KNOB as f32 - 32.0;
    if dragging {
        let label = Label { face: serif_bold, size: 32.0, ink: INK };
        let text = label.fit(if title.is_empty() { t().position } else { title }, room);
        label.draw(c, raster, &text, (r.x + KNOB) as f32, baseline);
    } else {
        Label { face: sans, size: 30.0, ink: INK_SOFT }.draw(c, raster, t().position, (r.x + KNOB) as f32, baseline);
    }

    // Solid black only: the track is redrawn with the black-and-white waveform while dragging.
    let t = track(r);
    let mid = t.y + t.h / 2;
    c.fill(Rect::new(t.x, mid - 1, t.w, 2), INK);
    if chapters <= MAX_TICKS {
        for i in 1..chapters {
            let x = t.x + (t.w as f32 * i as f32 / chapters as f32).round() as i32;
            c.fill(Rect::new(x - 1, mid - 8, 2, 16), INK);
        }
    }
    let x = t.x + (t.w as f32 * f) as i32;
    rounded_rect(c, Rect::new(t.x, mid - 4, x - t.x, 8), 4, INK);
    rounded_rect(c, Rect::new(x - KNOB, mid - KNOB, 2 * KNOB, 2 * KNOB), KNOB, INK);
    rounded_rect(c, Rect::new(x - 7, mid - 7, 14, 14), 7, PAPER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use zarathustra_engine::layout::flow;
    use zarathustra_engine::layout::page::PageLine;
    use zarathustra_engine::layout::paragraph::Placed;
    use zarathustra_engine::library::{BookEntry, Format};
    use zarathustra_engine::style::cascade::{compute, Preferences, Stylesheets};

    const CHAPTER: &str = r#"<html><body>
      <h1>Capitolo primo</h1>
      <p>Nel mezzo del <i>cammin</i> di nostra vita mi ritrovai per una selva oscura,</p>
      <p>ché la diritta via era smarrita.</p>
      <p>Ahi quanto a dir qual era è cosa dura</p>
    </body></html>"#;

    /// One line per block, one 10 px glyph per byte; `per_page` blocks a page.
    fn chapter(src: &str, per_page: usize) -> Chapter {
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
        let lines: Vec<PageLine> = (blocks.iter().enumerate())
            .map(|(i, b)| PageLine {
                block: i as u32,
                x: 0.0,
                baseline: 40.0,
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
        let pages = lines.chunks(per_page).map(|l| Page { lines: l.to_vec(), images: Vec::new() }).collect();
        Chapter { dom, blocks, pages }
    }

    fn store() -> (Store, BookId) {
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
        (store, book)
    }

    #[test]
    fn bookmarks_toggle_persist_and_follow_relayout() {
        let (db, book) = store();
        let store = Some((&db, book));
        let two_a_page = chapter(CHAPTER, 2);
        assert_eq!(two_a_page.pages.len(), 2);
        let mut b = Bookmarks::load(store);
        b.chapter_loaded(&two_a_page, 3);
        assert!(!b.marked(&two_a_page, 1));

        assert!(b.toggle(store, &two_a_page, 1, Some("Primo".into())));
        assert!(b.marked(&two_a_page, 1) && !b.marked(&two_a_page, 0));
        let saved = db.annotations(book).expect("list");
        assert_eq!(saved.len(), 1);
        let data = &saved[0].data;
        assert_eq!((data.kind, data.chapter.as_deref(), data.start.spine), (Kind::Bookmark, Some("Primo"), 3));
        assert_eq!(data.quote.as_ref().map(|q| q.exact.as_str()), Some("ché la diritta via era smarrita."));

        // Reopened and laid out one block a page: the bookmark is on the third page now.
        let one_a_page = chapter(CHAPTER, 1);
        let mut b = Bookmarks::load(store);
        b.chapter_loaded(&one_a_page, 3);
        assert_eq!((0..4).map(|p| b.marked(&one_a_page, p)).collect::<Vec<_>>(), [false, false, true, false]);
        // Another chapter: not marked.
        b.chapter_loaded(&one_a_page, 4);
        assert!(!b.marked(&one_a_page, 2));

        // Toggling the marked page removes it, in the database too.
        b.chapter_loaded(&one_a_page, 3);
        assert!(!b.toggle(store, &one_a_page, 2, None));
        assert!(!b.marked(&one_a_page, 2) && b.list().is_empty());
        assert!(db.annotations(book).expect("list").is_empty());
    }

    #[test]
    fn bookmarks_reanchor_by_quote_after_an_edit() {
        let ch = chapter(CHAPTER, 2);
        let mut b = Bookmarks::default();
        b.chapter_loaded(&ch, 0);
        assert!(b.toggle(None, &ch, 1, None));
        let edited = CHAPTER.replace("<h1>", "<p>Premessa.</p><h1>");
        let ch2 = chapter(&edited, 1);
        b.chapter_loaded(&ch2, 0);
        // The CFI now points into another paragraph; the quote finds "ché la diritta" again.
        assert!(b.marked(&ch2, 3), "{:?}", b.here);
        assert_eq!(b.list().len(), 1);
        b.remove(None, 0);
        assert!(!b.marked(&ch2, 3) && b.list().is_empty());
    }

    #[test]
    fn quotes_keep_whole_words() {
        let text = "  uno due\n  tre quattro";
        assert_eq!(&text[2..words_end(text, 2, 3)], "uno due\n  tre");
        assert_eq!(words_end(text, 2, 9), text.len());
    }

    #[test]
    fn scrubber_maps_fractions_to_chapters_and_pages() {
        let r = Rect::new(64, 500, 1108, 130);
        assert_eq!(fraction(r, 0), 0.0);
        assert_eq!(fraction(r, 5000), 1.0);
        let mid = fraction(r, r.x + r.w / 2);
        assert!((mid - 0.5).abs() < 0.01);

        assert_eq!(target(0.0, 10), (0, 0.0));
        let (c, within) = target(0.35, 10);
        assert_eq!(c, 3);
        assert!((within - 0.5).abs() < 0.01);
        assert_eq!(target(1.0, 10), (9, 1.0), "the very end is the last chapter's end");
        assert_eq!(target(0.5, 0).0, 0);

        assert_eq!(page_in(0.0, 20), 0);
        assert_eq!(page_in(0.5, 20), 10);
        assert_eq!(page_in(1.0, 20), 19);
        assert_eq!(page_in(0.7, 0), 0);

        let e = |title: &str, chapter| Entry { title: title.into(), depth: 0, chapter, fragment: None };
        let entries = [e("Uno", 1), e("Uno bis", 1), e("Tre", 3)];
        assert_eq!(chapter_title(&entries, 0), None, "before the first entry");
        assert_eq!(chapter_title(&entries, 1), Some("Uno"));
        assert_eq!(chapter_title(&entries, 2), Some("Uno bis"));
        assert_eq!(chapter_title(&entries, 7), Some("Tre"));
    }

    #[test]
    fn back_offers_the_place_first_left_until_used_or_reached() {
        let at = |chapter, page| Mark { chapter, pos: TextPos::new(page, 0), page, progress: 0.0 };
        let mut back = Back::default();
        back.jumped(at(0, 4), at(0, 4));
        assert_eq!(back.get(), None, "a jump to the page shown");
        back.jumped(at(0, 4), at(5, 0));
        assert_eq!(back.get(), Some(at(0, 4)));
        back.jumped(at(5, 0), at(8, 2));
        assert_eq!(back.get(), Some(at(0, 4)), "the first place left is kept");
        back.jumped(at(8, 2), at(0, 4));
        assert_eq!(back.get(), None, "jumped back onto it");
        back.jumped(at(0, 4), at(2, 1));
        back.turned(at(2, 2));
        assert_eq!(back.get(), Some(at(0, 4)), "reading on keeps the way back");
        back.turned(at(0, 4));
        assert_eq!(back.get(), None, "turned back onto it");
        back.jumped(at(0, 4), at(2, 1));
        back.clear();
        assert_eq!(back.get(), None);
        back.jumped(at(0, 4), at(2, 1));
        assert_eq!(back.take(), Some(at(0, 4)));
        assert_eq!(back.get(), None);
    }

    #[test]
    fn corner_wins_over_the_menu_band() {
        assert!(in_corner((W - 10, 10)) && in_corner((W - CORNER, CORNER - 1)));
        assert!(!in_corner((W - CORNER - 1, 10)) && !in_corner((W - 10, CORNER)));
        const { assert!(CORNER <= crate::reader::TOP_BAND) };
    }
}
