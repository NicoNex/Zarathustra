//! Highlights on PDF pages. A long press selects the word under the finger in the page's
//! text (MuPDF's, with a box per character), a drag extends the selection word by word, and
//! the action bar saves it. Stored like the reflowed books' highlights, with the page as the
//! spine, byte offsets in the page text as positions (`pdf:<page>:<byte>` CFIs) and the
//! quote, which finds the text again if MuPDF ever extracts the page differently.

use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::anchor::{reanchor, Anchor, TextQuote};
use zarathustra_engine::library::{BookEntry, Format};
use zarathustra_engine::pdf::{Frame, PageText};
use zarathustra_store::{book_key, Annotation, AnnotationData, BookId, Color, Kind, Store};

use crate::highlight::{load, mark, word_at, Button, Popup, HIGHLIGHT_GRAY};
use crate::Ctx;

/// How far (screen pixels) from a line of text a press still lands on it.
const REACH: f32 = 40.0;

/// Where a page image is on screen: its top-left corner and how it maps the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub x: i32,
    pub y: i32,
    pub frame: Frame,
}

impl Place {
    fn to_page(self, (x, y): (i32, i32)) -> (f32, f32) {
        let f = self.frame;
        ((x - self.x) as f32 / f.scale + f.x0, (y - self.y) as f32 / f.scale + f.y0)
    }

    /// A box in page points (left, top, right, bottom) on screen, rounded outwards.
    fn to_screen(self, r: [f32; 4]) -> Rect {
        let f = self.frame;
        let (x0, y0) = ((r[0] - f.x0) * f.scale, (r[1] - f.y0) * f.scale);
        let (x1, y1) = ((r[2] - f.x0) * f.scale, (r[3] - f.y0) * f.scale);
        let (x0, y0) = (self.x + x0.floor() as i32, self.y + y0.floor() as i32);
        Rect::new(x0, y0, self.x + x1.ceil() as i32 - x0, self.y + y1.ceil() as i32 - y0)
    }
}

/// The byte offset of the character nearest `at`, if a line of text is within reach (and,
/// with `near`, the character itself: a tap in the margin beside a line is not on it).
fn hit(text: &PageText, place: &Place, at: (i32, i32), near: bool) -> Option<usize> {
    let (x, y) = place.to_page(at);
    let off = |lo: f32, hi: f32, v: f32| if v < lo { lo - v } else { (v - hi).max(0.0) };
    let d = |r: &[f32; 4]| (off(r[1], r[3], y), off(r[0], r[2], x));
    let best = text.chars.iter().min_by(|a, b| {
        let (da, db) = (d(&a.rect), d(&b.rect));
        da.0.total_cmp(&db.0).then(da.1.total_cmp(&db.1))
    })?;
    let (dy, dx) = d(&best.rect);
    (dy * place.frame.scale <= REACH && (!near || dx * place.frame.scale <= REACH)).then_some(best.at)
}

/// The parts of byte `range` on each line, on screen.
pub fn bands(text: &PageText, place: &Place, range: &Range<usize>) -> Vec<Rect> {
    let mut lines: Vec<(u32, [f32; 4])> = Vec::new();
    for c in text.chars.iter().filter(|c| range.contains(&c.at)) {
        match lines.last_mut() {
            Some((line, r)) if *line == c.line => {
                *r = [r[0].min(c.rect[0]), r[1].min(c.rect[1]), r[2].max(c.rect[2]), r[3].max(c.rect[3])];
            }
            _ => lines.push((c.line, c.rect)),
        }
    }
    lines.into_iter().map(|(_, r)| place.to_screen(r)).collect()
}

fn union(rects: &[Rect]) -> Rect {
    rects.iter().fold(Rect::default(), |u, r| u.union(r))
}

/// The document's highlights and the selection in progress on the page shown.
#[derive(Default)]
pub struct PdfMarks {
    store: Option<(Rc<Store>, BookId)>,
    saved: Vec<Annotation>,
    /// Highlights on the page shown: index into `saved`, byte range in its text.
    resolved: Vec<(usize, Range<usize>)>,
    page: usize,
    /// The word first pressed and the whole selection.
    selection: Option<(Range<usize>, Range<usize>)>,
    popup: Option<Popup>,
}

impl PdfMarks {
    /// Loads the document's highlights; without a database they last the session.
    pub fn open(store: Option<Rc<Store>>, path: &Path, title: &str, author: Option<&str>) -> PdfMarks {
        let entry = BookEntry {
            path: path.to_path_buf(),
            title: title.to_string(),
            authors: author.map(str::to_string).into_iter().collect(),
            language: None,
            series: None,
            series_index: None,
            format: Format::Pdf,
            size: 0,
            modified: std::time::SystemTime::UNIX_EPOCH,
            cover_thumb: None,
        };
        let key = book_key(None, path.as_os_str().as_encoded_bytes());
        let Some(store) = store else { return PdfMarks::default() };
        match load(&store, &key, &entry) {
            Ok((id, saved)) => PdfMarks { store: Some((store, id)), saved, ..PdfMarks::default() },
            Err(e) => {
                eprintln!("pdf highlights: {e}");
                PdfMarks::default()
            }
        }
    }

    /// Finds page `page`'s highlights in its `text`; drops any selection.
    pub fn page_loaded(&mut self, page: usize, text: &PageText) {
        self.page = page;
        self.dismiss();
        self.resolved = (self.saved.iter().enumerate())
            .filter(|(_, a)| a.data.start.spine as usize == page)
            .filter_map(|(i, a)| Some((i, resolve(&text.text, &a.data)?)))
            .collect();
    }

    /// A selection or its action bar is showing: every tap is for them.
    pub fn is_active(&self) -> bool {
        self.selection.is_some() || self.popup.is_some()
    }

    /// Drops the selection and the action bar (the menu opened).
    pub fn dismiss(&mut self) {
        self.selection = None;
        self.popup = None;
    }

    /// Long press at `at`: selects the word there. The screen area to redraw.
    pub fn long_press(&mut self, text: &PageText, place: &Place, at: (i32, i32)) -> Option<Rect> {
        if self.popup.as_ref().is_some_and(|p| p.rect.contains(at.0, at.1)) {
            return None;
        }
        let pos = hit(text, place, at, false)?;
        let mut damage = self.popup.take().map_or(Rect::default(), |p| p.rect);
        if let Some((_, old)) = self.selection.take() {
            damage = damage.union(&union(&bands(text, place, &old)));
        }
        let word = word_at(&text.text, pos);
        damage = damage.union(&union(&bands(text, place, &word)));
        self.selection = Some((word.clone(), word));
        Some(damage)
    }

    /// The finger moved while selecting: the free end follows it, word by word.
    pub fn drag(&mut self, text: &PageText, place: &Place, at: (i32, i32)) -> Option<Rect> {
        let pos = hit(text, place, at, false)?;
        let (anchor, range) = self.selection.as_mut()?;
        let word = word_at(&text.text, pos);
        let new =
            if word.start >= anchor.start { anchor.start..word.end.max(anchor.end) } else { word.start..anchor.end };
        if new == *range {
            return None;
        }
        let damage = union(&bands(text, place, range)).union(&union(&bands(text, place, &new)));
        *range = new;
        Some(damage)
    }

    /// The finger lifted after selecting: the action bar.
    pub fn release(&mut self, text: &PageText, place: &Place) -> Option<Rect> {
        let (_, range) = self.selection.as_ref()?;
        let b = bands(text, place, range);
        let popup = Popup::new(None, &[Button::Highlight, Button::Cancel], &b);
        let damage = popup.rect.union(&union(&b));
        self.popup = Some(popup);
        Some(damage)
    }

    /// A tap: a button of the bar, a tap elsewhere (cancels), or a tap on a highlight (its bar).
    /// `None` when the tap is not for the highlights (a page turn, the menu).
    pub fn tap(&mut self, text: &PageText, place: &Place, at: (i32, i32), chapter: Option<&str>) -> Option<Rect> {
        if let Some(popup) = self.popup.take() {
            let mut damage = popup.rect;
            let button = popup.button_at(at);
            if let Some((_, range)) = self.selection.take() {
                damage = damage.union(&union(&bands(text, place, &range)));
                if button == Some(Button::Highlight) {
                    self.save(&text.text, range, chapter);
                }
            } else if let Some(i) = popup.target {
                if let Some((_, r)) = self.resolved.iter().find(|(j, _)| *j == i) {
                    damage = damage.union(&union(&bands(text, place, r)));
                }
                if button == Some(Button::Delete) {
                    self.delete(i);
                }
            }
            return Some(damage);
        }
        if let Some((_, range)) = self.selection.take() {
            return Some(union(&bands(text, place, &range)));
        }
        let pos = hit(text, place, at, true)?;
        let (i, range) = self.resolved.iter().find(|(_, r)| r.contains(&pos)).cloned()?;
        let popup = Popup::new(Some(i), &[Button::Delete, Button::Cancel], &bands(text, place, &range));
        let damage = popup.rect;
        self.popup = Some(popup);
        Some(damage)
    }

    fn save(&mut self, text: &str, range: Range<usize>, chapter: Option<&str>) {
        let page = u32::try_from(self.page).unwrap_or(u32::MAX);
        let start = u32::try_from(range.start).unwrap_or(u32::MAX);
        let data = AnnotationData {
            kind: Kind::Highlight,
            start: Anchor { cfi: format!("pdf:{page}:{}", range.start), spine: page, pos: start },
            end_cfi: Some(format!("pdf:{page}:{}", range.end)),
            quote: Some(TextQuote::new(text, range.clone())),
            chapter: chapter.map(str::to_string),
            page_hint: Some(page.saturating_add(1)),
            note: None,
            color: Color::Gray,
        };
        let annotation = match &self.store {
            Some((store, book)) => match store.add_annotation(*book, &data) {
                Ok(a) => a,
                Err(e) => return eprintln!("pdf highlights: {e}"),
            },
            None => Annotation { id: String::new(), book: BookId(0), data, created_at: 0, updated_at: 0 },
        };
        self.saved.push(annotation);
        self.resolved.push((self.saved.len() - 1, range));
    }

    fn delete(&mut self, i: usize) {
        let Some(a) = self.saved.get(i) else { return };
        if let Some((store, _)) = &self.store {
            if let Err(e) = store.delete_annotation(&a.id) {
                return eprintln!("pdf highlights: {e}");
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

    /// Draws the highlights and the selection over the drawn page, then the action bar.
    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx, text: &PageText, place: &Place) {
        for (_, range) in &self.resolved {
            for r in bands(text, place, range) {
                c.darken(r, HIGHLIGHT_GRAY);
            }
        }
        if let Some((_, range)) = &self.selection {
            bands(text, place, range).into_iter().for_each(|r| mark(c, r));
        }
        if let Some(popup) = &self.popup {
            popup.render(c, ctx);
        }
    }
}

/// Where a stored highlight is in the page's text: at its offsets while the text there is
/// still its quote, else wherever the quote is found again.
fn resolve(text: &str, data: &AnnotationData) -> Option<Range<usize>> {
    let start = data.start.pos as usize;
    let end: usize = data.end_cfi.as_deref()?.rsplit(':').next()?.parse().ok()?;
    let fits = |q: &TextQuote| start < end && end <= text.len() && TextQuote::new(text, start..end).exact == q.exact;
    match &data.quote {
        Some(q) if fits(q) => Some(start..end),
        Some(q) => reanchor(text, q),
        None => (start < end && end <= text.len()).then_some(start..end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zarathustra_engine::pdf::CharBox;

    /// "ab cd\nef": two lines of 10 × 10 pt characters, the second 20 pt lower.
    fn page() -> PageText {
        let mut chars = Vec::new();
        for (i, at) in [0usize, 1, 2, 3, 4].into_iter().enumerate() {
            chars.push(CharBox { at, line: 0, rect: [i as f32 * 10.0, 0.0, i as f32 * 10.0 + 10.0, 10.0] });
        }
        for (i, at) in [6usize, 7].into_iter().enumerate() {
            chars.push(CharBox { at, line: 1, rect: [i as f32 * 10.0, 20.0, i as f32 * 10.0 + 10.0, 30.0] });
        }
        PageText { text: "ab cd\nef".into(), chars }
    }

    const PLACE: Place = Place { x: 100, y: 200, frame: Frame { x0: 0.0, y0: 0.0, scale: 2.0 } };

    #[test]
    fn selects_highlights_and_finds_them_again() {
        let text = page();
        let mut m = PdfMarks::default();
        // Screen (135, 210) is page (17.5, 5): the "b" of "ab".
        assert!(m.long_press(&text, &PLACE, (135, 210)).is_some());
        assert_eq!(m.selection.as_ref().map(|s| s.1.clone()), Some(0..2));
        // Down to "ef" on the next line.
        m.drag(&text, &PLACE, (105, 250));
        assert_eq!(m.selection.as_ref().map(|s| s.1.clone()), Some(0..8));
        assert_eq!(bands(&text, &PLACE, &(0..8)), [Rect::new(100, 200, 100, 20), Rect::new(100, 240, 40, 20)]);
        m.release(&text, &PLACE);
        let save = m.popup.as_ref().and_then(|p| p.buttons.iter().find(|b| b.0 == Button::Highlight)).map(|b| b.1);
        let save = save.expect("highlight button");
        m.tap(&text, &PLACE, (save.x + 5, save.y + 5), Some("One"));
        assert_eq!(m.resolved, [(0, 0..8)]);
        assert_eq!(m.saved[0].data.end_cfi.as_deref(), Some("pdf:0:8"));
        // Laid out again, and found again after the text moved.
        m.page_loaded(0, &text);
        assert_eq!(m.resolved, [(0, 0..8)]);
        assert_eq!(resolve("xx ab cd\nef", &m.saved[0].data), Some(3..11));
        // Far below the text: nothing to select.
        assert!(m.long_press(&text, &PLACE, (105, 400)).is_none());
    }
}
