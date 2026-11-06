//! The book menu: a sheet rising from the bottom of the home on a long press, with the book
//! (small cover, title, author, progress) and what can be done with it.

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::image::{self, GrayImage};

use crate::i18n::t;
use crate::ui::{clip_corners, rounded_outline, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const PAD: i32 = 72;
const HEAD_H: i32 = 250;
const ROW_H: i32 = 118;
/// Space under the last row.
const BOTTOM: i32 = 40;
const RADIUS: i32 = 36;

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
    rows: Vec<(Item, Rect)>,
    /// Delete was tapped once: the row asks for a second tap.
    armed: bool,
}

impl BookMenu {
    /// `reflowable`: not a PDF, so it has bookmarks and search.
    pub fn new(
        title: &str,
        author: &str,
        progress: Option<f32>,
        finished: bool,
        highlights: usize,
        reflowable: bool,
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
        items.push(Item::Delete);
        // Rows shrink to keep the sheet within the screen's lower three quarters.
        let n = items.len() as i32;
        let row_h = ROW_H.min((H - H / 4 - HEAD_H - BOTTOM) / n);
        let top = H - HEAD_H - n * row_h - BOTTOM;
        let rows = (items.iter().enumerate())
            .map(|(i, &it)| (it, Rect::new(0, top + HEAD_H + i as i32 * row_h, W, row_h)))
            .collect();
        BookMenu { title: title.into(), author: author.into(), progress, highlights, rows, armed: false }
    }

    /// The sheet, its bottom corners below the screen.
    pub fn rect(&self) -> Rect {
        let top = self.rows.first().map_or(H, |(_, r)| r.y - HEAD_H);
        Rect::new(0, top, W, H - top + RADIUS)
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

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx, cover: Option<&GrayImage>) {
        let sheet = self.rect();
        // A heavy outline instead of a shadow or a dimmed page: crisp on e-ink, one refresh.
        rounded_outline(c, sheet, RADIUS, 4, INK, PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        // Head: cover, title, author, progress.
        let cover_r = Rect::new(PAD, sheet.y + 56, 100, 150);
        let x = match cover {
            Some(img) => {
                let at =
                    Rect::new(cover_r.x, cover_r.bottom() - img.height as i32, img.width as i32, img.height as i32);
                image::draw(c, img, at.x, at.y);
                clip_corners(c, at, 8, [PAPER; 4]);
                at.right() + 40
            }
            None => PAD,
        };
        let width = (W - PAD - x) as f32;
        let title = Label { face: serif_bold, size: 42.0, ink: INK };
        title.draw(c, raster, &title.fit(&self.title, width), x as f32, sheet.y + 116);
        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        soft.draw(c, raster, &soft.fit(&self.author, width), x as f32, sheet.y + 166);
        if let Some(p) = self.progress {
            soft.draw(c, raster, &format!("{:.0}%", p * 100.0), x as f32, sheet.y + 210);
        }

        let label = Label { face: serif, size: 38.0, ink: INK };
        for (it, r) in &self.rows {
            c.fill(Rect::new(PAD, r.y, W - 2 * PAD, 2), LINE);
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
            };
            let base = r.y + r.h / 2 + 14;
            label.draw(c, raster, text, PAD as f32, base);
            let meta = match it {
                Item::Highlights if self.highlights > 0 => format!("{}   ›", self.highlights),
                Item::MarkRead(_) | Item::Delete => String::new(),
                _ => "›".into(),
            };
            soft.draw(c, raster, &meta, W as f32 - PAD as f32 - soft.width(&meta), base);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_hit_and_outside_closes() {
        let mut m = BookMenu::new("Inferno", "Dante", Some(0.4), false, 3, true);
        let r = m.rect();
        assert!(r.y > H / 4 && r.bottom() > H, "a bottom sheet");
        let rows = m.rows.clone();
        let row = |i: usize| rows[i].1;
        let mid = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        assert_eq!(m.tap(mid(row(0))), Tap::Item(Item::Open));
        assert_eq!(m.tap(mid(row(2))), Tap::Item(Item::Bookmarks));
        assert_eq!(m.tap(mid(row(3))), Tap::Item(Item::Search));
        assert_eq!(m.tap(mid(row(6))), Tap::Item(Item::MarkRead(true)));
        assert_eq!(m.tap(mid(row(7))), Tap::Item(Item::MarkRead(false)), "in progress: both");
        assert_eq!(m.tap(mid(row(8))), Tap::Redraw, "the first tap arms");
        assert_eq!(m.tap(mid(row(8))), Tap::Item(Item::Delete));
        let pdf = BookMenu::new("Inferno", "Dante", None, false, 0, false);
        assert!(!pdf.rows.iter().any(|&(it, _)| it == Item::MarkRead(false)), "never opened");
        assert!(pdf.rows.iter().all(|&(it, _)| !matches!(it, Item::Bookmarks | Item::Search)));
        assert_eq!(m.tap((W / 2, r.y + 20)), Tap::None, "the head");
        assert_eq!(m.tap((W / 2, r.y - 20)), Tap::Close);
    }
}
