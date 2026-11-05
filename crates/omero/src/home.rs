//! Home: the book being read on top, then the library filtered by reading status, a page of
//! rows at a time.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use omero_device::{Canvas, Rect};
use omero_engine::font::{Face, Rasterizer};
use omero_engine::image::{self, GrayImage};
use omero_engine::library::{BookEntry, Library, SortBy};

use crate::reader::Position;
use crate::ui::{rounded_outline, rounded_rect, Label, FILL, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, H, W};

const PAD: i32 = 72;
const CARD_TOP: i32 = 210;
const CARD_H: i32 = 330;
const CARD_COVER: (i32, i32) = (172, 258);
const ROW_H: i32 = 156;
const ROW_COVER: (i32, i32) = (80, 120);
const CHIPS_H: i32 = 64;
const FOOTER: i32 = 110;
const EXIT: Rect = Rect { x: W - PAD - 150, y: 82, w: 150, h: 72 };
/// Progress from which a book counts as read.
const FINISHED: f32 = 0.99;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Unread,
    Reading,
    Finished,
}

const FILTERS: [(Filter, &str); 4] =
    [(Filter::All, "All"), (Filter::Unread, "Unread"), (Filter::Reading, "Reading"), (Filter::Finished, "Read")];

struct Entry {
    book: BookEntry,
    /// Reading progress 0–1 and when it was saved, if the book was opened.
    progress: Option<(f32, SystemTime)>,
    cover: Option<GrayImage>,
}

impl Entry {
    fn status(&self) -> Filter {
        match self.progress {
            None => Filter::Unread,
            Some((p, _)) if p >= FINISHED => Filter::Finished,
            Some(_) => Filter::Reading,
        }
    }

    fn author(&self) -> String {
        self.book.authors.join(", ")
    }
}

pub struct Home {
    books: Vec<Entry>,
    /// The book in the "continue reading" card, if any, and its large cover.
    current: Option<(usize, Option<GrayImage>)>,
    filter: Filter,
    /// Indices of `books` shown under the current filter.
    shown: Vec<usize>,
    page: usize,
}

fn cover(thumb: Option<&Path>, (w, h): (i32, i32)) -> Option<GrayImage> {
    let mut img = image::decode(&std::fs::read(thumb?).ok()?, w as u32, h as u32)?;
    image::quantize16(&mut img);
    Some(img)
}

impl Home {
    pub fn scan(dirs: &[PathBuf], ctx: &Ctx) -> Home {
        let dirs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        let library = Library::scan(&dirs, &ctx.state_dir.join("library"));
        let order = library.sorted(SortBy::Title);
        let mut slots: Vec<Option<BookEntry>> = library.books.into_iter().map(Some).collect();
        let books: Vec<Entry> = (order.into_iter())
            .filter_map(|i| slots[i].take())
            .map(|book| {
                let state = ctx.state_file(&book.path);
                let progress = Position::load(&state)
                    .and_then(|p| Some((p.progress, std::fs::metadata(&state).ok()?.modified().ok()?)));
                let cover = cover(book.cover_thumb.as_deref(), ROW_COVER);
                Entry { book, progress, cover }
            })
            .collect();
        let current = (books.iter().enumerate())
            .filter(|(_, b)| b.status() == Filter::Reading)
            .max_by_key(|(_, b)| b.progress.map(|p| p.1))
            .map(|(i, b)| (i, cover(b.book.cover_thumb.as_deref(), CARD_COVER)));
        let mut home = Home { books, current, filter: Filter::All, shown: Vec::new(), page: 0 };
        home.set_filter(Filter::All);
        home
    }

    fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.shown =
            (0..self.books.len()).filter(|&i| filter == Filter::All || self.books[i].status() == filter).collect();
        self.page = 0;
    }

    fn chips_top(&self) -> i32 {
        if self.current.is_some() {
            CARD_TOP + CARD_H + 48
        } else {
            CARD_TOP
        }
    }

    fn list_top(&self) -> i32 {
        self.chips_top() + CHIPS_H + 24
    }

    fn rows_per_page(&self) -> usize {
        ((H - FOOTER - self.list_top()) / ROW_H).max(1) as usize
    }

    fn pages(&self) -> usize {
        self.shown.len().div_ceil(self.rows_per_page()).max(1)
    }

    /// Filter chips' rectangles, measured with the chip font.
    fn chips(&self, label: &Label) -> Vec<(Filter, &'static str, Rect)> {
        let mut x = PAD;
        (FILTERS.iter())
            .map(|&(f, name)| {
                let w = label.width(name) as i32 + 56;
                let r = Rect::new(x, self.chips_top(), w, CHIPS_H);
                x += w + 16;
                (f, name, r)
            })
            .collect()
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(serif), Some(serif_bold), Some(sans)) = (
            fonts.pick(&["serif".into()], 400, false),
            fonts.pick(&["serif".into()], 700, false),
            fonts.pick(&["sans-serif".into()], 400, false),
        ) else {
            return;
        };

        Label { face: serif_bold, size: 66.0, ink: INK }.draw(c, raster, "Library", PAD as f32, 150);
        rounded_outline(c, EXIT, EXIT.h / 2, 3, INK, PAPER);
        let exit = Label { face: sans, size: 30.0, ink: INK };
        let x = EXIT.x as f32 + (EXIT.w as f32 - exit.width("Exit")) / 2.0;
        exit.draw(c, raster, "Exit", x, EXIT.y + 47);

        if let Some((i, big)) = &self.current {
            let book = &self.books[*i];
            let card = Rect::new(PAD, CARD_TOP, W - 2 * PAD, CARD_H);
            rounded_rect(c, card, 28, FILL);
            let r = Rect::new(card.x + 36, card.y + 36, CARD_COVER.0, CARD_COVER.1);
            let r = draw_cover(c, raster, serif_bold, r, big.as_ref(), &book.book.title);
            let x = (r.right() + 44) as f32;
            let width = (card.right() - 40) as f32 - x;
            let small = Label { face: sans, size: 28.0, ink: INK_SOFT };
            small.draw(c, raster, "CONTINUE READING", x, card.y + 70);
            let title = Label { face: serif_bold, size: 46.0, ink: INK };
            title.draw(c, raster, &title.fit(&book.book.title, width), x, card.y + 140);
            let author = Label { face: sans, size: 30.0, ink: INK_SOFT };
            author.draw(c, raster, &author.fit(&book.author(), width), x, card.y + 190);
            let progress = book.progress.map_or(0.0, |p| p.0);
            let bar = Rect::new(x as i32, card.bottom() - 70, width as i32 - 110, 10);
            rounded_rect(c, bar, 5, LINE);
            rounded_rect(c, Rect { w: (bar.w as f32 * progress) as i32, ..bar }, 5, INK);
            small.draw(c, raster, &format!("{:.0}%", progress * 100.0), (bar.right() + 24) as f32, bar.y + 12);
        }

        let chip = Label { face: sans, size: 28.0, ink: INK };
        for (f, name, r) in self.chips(&chip) {
            let on = f == self.filter;
            if on {
                rounded_rect(c, r, r.h / 2, INK);
            } else {
                rounded_outline(c, r, r.h / 2, 2, LINE, PAPER);
            }
            let label = Label { ink: if on { PAPER } else { INK }, ..chip };
            label.draw(c, raster, name, (r.x + 28) as f32, r.y + 42);
        }

        let top = self.list_top();
        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        if self.shown.is_empty() {
            soft.draw(c, raster, "No books here", PAD as f32, top + 70);
        }
        let per = self.rows_per_page();
        for (row, &i) in self.shown.iter().skip(self.page * per).take(per).enumerate() {
            let book = &self.books[i];
            let y = top + row as i32 * ROW_H;
            let r = Rect::new(PAD, y + 18, ROW_COVER.0, ROW_COVER.1);
            draw_cover(c, raster, serif_bold, r, book.cover.as_ref(), &book.book.title);
            let x = (PAD + ROW_COVER.0 + 32) as f32;
            let right = (W - PAD) as f32;
            let status = match (book.status(), book.progress) {
                (Filter::Finished, _) => Some("Read".to_string()),
                (_, Some((p, _))) => Some(format!("{:.0}%", p * 100.0)),
                _ => None,
            };
            let status_w = status.as_deref().map_or(0.0, |s| soft.width(s) + 24.0);
            let title = Label { face: serif, size: 38.0, ink: INK };
            title.draw(c, raster, &title.fit(&book.book.title, right - x - status_w), x, y + 70);
            let author = Label { face: sans, size: 28.0, ink: INK_SOFT };
            author.draw(c, raster, &author.fit(&book.author(), right - x - status_w), x, y + 114);
            if let Some(s) = status {
                soft.draw(c, raster, &s, right - soft.width(&s), y + 70);
            }
            if row + 1 < per {
                c.fill(Rect::new(x as i32, y + ROW_H - 1, W - PAD - x as i32, 1), LINE);
            }
        }

        if self.pages() > 1 {
            let label = format!("‹   {} / {}   ›", self.page + 1, self.pages());
            let x = (W as f32 - soft.width(&label)) / 2.0;
            soft.draw(c, raster, &label, x, H - FOOTER / 2 + 10);
        }
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, y)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), (x, y): (i32, i32), ctx: &Ctx) -> Action {
        let (dx, dy) = (x - x0, y - y0);
        let pages = self.pages();
        let turn = |page: &mut usize, forward: bool| {
            let next = if forward { (*page + 1).min(pages - 1) } else { page.saturating_sub(1) };
            let moved = next != *page;
            *page = next;
            if moved {
                Action::Redraw { turn: Some(forward) }
            } else {
                Action::None
            }
        };
        if dx.abs() > 100 || dy.abs() > 100 {
            let forward = if dx.abs() > dy.abs() { dx < 0 } else { dy < 0 };
            return turn(&mut self.page, forward);
        }
        let inside = |r: Rect| x0 >= r.x && x0 < r.right() && y0 >= r.y && y0 < r.bottom();
        if inside(EXIT) {
            return Action::Quit;
        }
        if y0 > H - FOOTER {
            return turn(&mut self.page, x0 > W / 2);
        }
        if let Some((i, _)) = &self.current {
            if inside(Rect::new(PAD, CARD_TOP, W - 2 * PAD, CARD_H)) {
                return Action::Open(self.books[*i].book.path.clone());
            }
        }
        if let Some(sans) = ctx.sans(false) {
            let hit = self.chips(&Label { face: sans, size: 28.0, ink: INK }).into_iter().find(|c| inside(c.2));
            if let Some((f, _, _)) = hit {
                if f != self.filter {
                    self.set_filter(f);
                    return Action::Redraw { turn: None };
                }
                return Action::None;
            }
        }
        let top = self.list_top();
        if y0 >= top {
            let row = ((y0 - top) / ROW_H) as usize;
            if row < self.rows_per_page() {
                if let Some(&i) = self.shown.get(self.page * self.rows_per_page() + row) {
                    return Action::Open(self.books[i].book.path.clone());
                }
            }
        }
        Action::None
    }
}

/// Draws the cover image centered in `r` (or a placeholder with the title's initial);
/// returns the rectangle actually covered.
fn draw_cover(
    c: &mut Canvas,
    raster: &mut Rasterizer,
    face: &Face,
    r: Rect,
    img: Option<&GrayImage>,
    title: &str,
) -> Rect {
    if let Some(img) = img {
        let (w, h) = (img.width as i32, img.height as i32);
        let at = Rect::new(r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, w, h);
        image::draw(c, img, at.x, at.y);
        // Hairline frame so white covers don't melt into the page.
        for side in [
            Rect { h: 1, ..at },
            Rect { y: at.bottom() - 1, h: 1, ..at },
            Rect { w: 1, ..at },
            Rect { x: at.right() - 1, w: 1, ..at },
        ] {
            c.fill(side, LINE);
        }
        return r;
    }
    rounded_rect(c, r, 12, 70);
    let initial: String = title.chars().find(|c| c.is_alphanumeric()).into_iter().collect();
    let label = Label { face, size: r.w as f32 * 0.55, ink: PAPER };
    let x = r.x as f32 + (r.w as f32 - label.width(&initial)) / 2.0;
    label.draw(c, raster, &initial, x, r.y + r.h / 2 + (label.size * 0.35) as i32);
    r
}
