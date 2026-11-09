//! The book menu: a card floating beside the book held on the home, with its title, author
//! and progress and what can be done with it. The book stays in view, framed.

use zarathustra_device::{Canvas, Rect};

use crate::i18n::t;
use crate::ui::{rounded_outline, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const CARD_W: i32 = 600;
/// Text inset inside the card.
const PAD: i32 = 40;
const HEAD_H: i32 = 150;
/// Touch targets: 88 px ≈ 7.5 mm at 300 dpi.
const ROW_H: i32 = 88;
/// Space under the last row.
const BOTTOM: i32 = 16;
/// Between the book and the card, and the card and the screen edge.
const GAP: i32 = 24;
const RADIUS: i32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Open,
    Contents,
    Bookmarks,
    Search,
    About,
    Highlights,
    /// Mark as read (true) or unread (false).
    MarkRead(bool),
    /// Delete the file: the first tap arms it, the second deletes.
    Delete,
    /// The `n`th plugin action offered for books.
    Plugin(usize),
}

/// What a tap on the open menu asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// Outside the sheet: close it.
    Close,
    /// On the sheet but no row.
    None,
    /// The sheet changed (Delete armed): draw it again.
    Redraw,
    Item(Item),
}

pub struct BookMenu {
    title: String,
    author: String,
    progress: Option<f32>,
    highlights: usize,
    /// Plugin actions' labels, for the `Plugin` rows.
    actions: Vec<String>,
    rows: Vec<(Item, Rect)>,
    card: Rect,
    /// The book held, framed while the card is up.
    anchor: Rect,
    /// Delete was tapped once: the row asks for a second tap.
    armed: bool,
}

impl BookMenu {
    /// `anchor` is the book's cover or row; `reflowable`: not a PDF, so it has bookmarks and
    /// search; `actions`: the plugin actions offered for books.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: &str,
        author: &str,
        progress: Option<f32>,
        finished: bool,
        highlights: usize,
        reflowable: bool,
        anchor: Rect,
        actions: Vec<String>,
    ) -> BookMenu {
        let mut items = vec![Item::Open, Item::Contents];
        if reflowable {
            items.extend([Item::Bookmarks, Item::Search]);
        }
        items.extend([Item::About, Item::Highlights]);
        if !finished {
            items.push(Item::MarkRead(true));
        }
        if progress.is_some() {
            // Unread starts over: nothing to undo on a book never opened.
            items.push(Item::MarkRead(false));
        }
        items.extend((0..actions.len()).map(Item::Plugin));
        items.push(Item::Delete);
        let card = place(anchor, HEAD_H + items.len() as i32 * ROW_H + BOTTOM);
        let rows = (items.iter().enumerate())
            .map(|(i, &it)| (it, Rect::new(card.x, card.y + HEAD_H + i as i32 * ROW_H, card.w, ROW_H)))
            .collect();
        BookMenu {
            title: title.into(),
            author: author.into(),
            progress,
            highlights,
            actions,
            rows,
            card,
            anchor,
            armed: false,
        }
    }

    pub fn rect(&self) -> Rect {
        self.card
    }

    /// `Item(Delete)` only on the second tap of the Delete row.
    pub fn tap(&mut self, (x, y): (i32, i32)) -> Tap {
        if !self.rect().contains(x, y) {
            return Tap::Close;
        }
        match self.rows.iter().find(|(_, r)| r.contains(x, y)).map(|&(it, _)| it) {
            Some(Item::Delete) if !self.armed => {
                self.armed = true;
                Tap::Redraw
            }
            Some(it) => Tap::Item(it),
            None => Tap::None,
        }
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let card = self.card;
        // The book held: a frame just outside it, so it reads as the card's subject.
        let ring = Rect::new(self.anchor.x - 10, self.anchor.y - 10, self.anchor.w + 20, self.anchor.h + 20);
        for edge in [
            Rect::new(ring.x, ring.y, ring.w, 4),
            Rect::new(ring.x, ring.bottom() - 4, ring.w, 4),
            Rect::new(ring.x, ring.y, 4, ring.h),
            Rect::new(ring.right() - 4, ring.y, 4, ring.h),
        ] {
            c.fill(edge, INK);
        }
        // A heavy outline instead of a shadow or a dimmed page: crisp on e-ink, one refresh.
        rounded_outline(c, card, RADIUS, 4, INK, PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        // Head: title, then author and progress.
        let (x, width) = (card.x + PAD, (card.w - 2 * PAD) as f32);
        let title = Label { face: serif_bold, size: 36.0, ink: INK };
        title.draw(c, raster, &title.fit(&self.title, width), x as f32, card.y + 68);
        let soft = Label { face: sans, size: 26.0, ink: INK_SOFT };
        let sub = match self.progress {
            Some(p) => format!("{}  ·  {:.0}%", self.author, p * 100.0),
            None => self.author.clone(),
        };
        soft.draw(c, raster, &soft.fit(&sub, width), x as f32, card.y + 114);

        let label = Label { face: serif, size: 32.0, ink: INK };
        for (it, r) in &self.rows {
            c.fill(Rect::new(x, r.y, card.w - 2 * PAD, 2), LINE);
            let text = match it {
                Item::Open if self.progress.is_some() => s.resume,
                Item::Open => s.start_reading,
                Item::Contents => s.contents,
                Item::Bookmarks => s.bookmarks,
                Item::Search => s.search_book,
                Item::About => s.about_book,
                Item::Highlights => s.highlights,
                Item::MarkRead(true) => s.mark_read,
                Item::MarkRead(false) => s.mark_unread,
                Item::Delete if self.armed => s.delete_confirm,
                Item::Delete => s.delete_book,
                Item::Plugin(k) => self.actions.get(*k).map_or("", String::as_str),
            };
            let base = r.y + r.h / 2 + 12;
            label.draw(c, raster, text, x as f32, base);
            let meta = match it {
                Item::Highlights if self.highlights > 0 => format!("{}   ›", self.highlights),
                Item::MarkRead(_) | Item::Delete => String::new(),
                _ => "›".into(),
            };
            soft.draw(c, raster, &meta, (card.right() - PAD) as f32 - soft.width(&meta), base);
        }
    }
}

/// The card, `h` tall, beside `anchor`: right of it when there is room, else left, else on
/// the roomier side over the neighbouring book. A list row (wider than half the screen) gets
/// it below, or above when it would not fit. Always kept on screen.
fn place(anchor: Rect, h: i32) -> Rect {
    let h = h.min(H - 2 * GAP);
    let clamp_y = |y: i32| y.clamp(GAP, H - GAP - h);
    if anchor.w > W / 2 {
        let below = anchor.bottom() + GAP;
        let y = if below + h <= H - GAP { below } else { anchor.y - GAP - h };
        return Rect::new(W - GAP - CARD_W, clamp_y(y), CARD_W, h);
    }
    let (right, left) = (anchor.right() + GAP, anchor.x - GAP - CARD_W);
    let x = match () {
        _ if right + CARD_W <= W - GAP => right,
        _ if left >= GAP => left,
        _ if W - anchor.right() >= anchor.x => W - GAP - CARD_W,
        _ => GAP,
    };
    Rect::new(x, clamp_y(anchor.y), CARD_W, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cover in the grid's left column.
    const BOOK: Rect = Rect { x: 72, y: 400, w: 290, h: 435 };

    #[test]
    fn rows_hit_and_outside_closes() {
        let mut m = BookMenu::new("Inferno", "Dante", Some(0.4), false, 3, true, BOOK, vec!["Send".into()]);
        let r = m.rect();
        assert!(r.x > BOOK.right() && r.right() <= W && r.y >= 0 && r.bottom() <= H, "beside the book, on screen");
        let rows = m.rows.clone();
        let row = |i: usize| rows[i].1;
        let mid = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        assert_eq!(m.tap(mid(row(0))), Tap::Item(Item::Open));
        assert_eq!(m.tap(mid(row(2))), Tap::Item(Item::Bookmarks));
        assert_eq!(m.tap(mid(row(3))), Tap::Item(Item::Search));
        assert_eq!(m.tap(mid(row(6))), Tap::Item(Item::MarkRead(true)));
        assert_eq!(m.tap(mid(row(7))), Tap::Item(Item::MarkRead(false)), "in progress: both");
        assert_eq!(m.tap(mid(row(8))), Tap::Item(Item::Plugin(0)), "plugin actions before Delete");
        assert_eq!(m.tap(mid(row(9))), Tap::Redraw, "the first tap arms");
        assert_eq!(m.tap(mid(row(9))), Tap::Item(Item::Delete));
        let pdf = BookMenu::new("Inferno", "Dante", None, false, 0, false, BOOK, Vec::new());
        assert!(!pdf.rows.iter().any(|&(it, _)| it == Item::MarkRead(false)), "never opened");
        assert!(pdf.rows.iter().all(|&(it, _)| !matches!(it, Item::Bookmarks | Item::Search)));
        assert_eq!(m.tap((r.x + r.w / 2, r.y + 20)), Tap::None, "the head");
        assert_eq!(m.tap((BOOK.x + 10, BOOK.y + 10)), Tap::Close);
        // A right-column cover gets the card on its left; a list row, below it.
        assert!(place(Rect::new(W - 72 - 290, 400, 290, 435), 900).right() < W - 72 - 290);
        let row = Rect::new(72, 500, W - 144, 190);
        assert!(place(row, 900).y > row.bottom());
    }
}
