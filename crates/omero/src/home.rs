//! Home: the book being read on top (first page only), filter chips, then the library as a
//! gallery of covers or as a list, a page at a time.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use omero_device::{Canvas, Rect};
use omero_engine::font::{Face, Rasterizer};
use omero_engine::image::{self, GrayImage};
use omero_engine::library::{BookEntry, Library, SortBy};

use crate::notebook::{self, Notebook};
use crate::reader::Position;
use crate::ui::{clip_corners, rounded_outline, rounded_rect, Label, FILL, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, H, W};

const PAD: i32 = 72;
const HEADER: i32 = 210;
const CARD_H: i32 = 330;
const CARD_COVER: (i32, i32) = (172, 258);
const CHIPS_H: i32 = 64;
const FOOTER: i32 = 110;
/// Grid: three columns of covers at the books' usual 2:3 ratio, title and author below.
const COLS: i32 = 3;
const GUTTER: i32 = 48;
const CELL_W: i32 = (W - 2 * PAD - (COLS - 1) * GUTTER) / COLS;
const COVER_H: i32 = CELL_W * 3 / 2;
const CELL_H: i32 = COVER_H + 150;
/// List: one book per row.
const ROW_H: i32 = 190;
const ROW_COVER: (i32, i32) = (100, 150);
const RADIUS: i32 = 10;
const EXIT: Rect = Rect { x: W - PAD - 150, y: 82, w: 150, h: 72 };
const VIEW: Rect = Rect { x: W - PAD - 150 - 32 - 192, y: 82, w: 192, h: 72 };
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
    /// Cover at grid size (the list scales it down at draw time).
    cover: Option<GrayImage>,
    small: Option<GrayImage>,
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
    /// The book in the "continue reading" card, if any, and its card-size cover.
    current: Option<(usize, Option<GrayImage>)>,
    filter: Filter,
    grid: bool,
    /// Indices of `books` shown under the current filter.
    shown: Vec<usize>,
    page: usize,
    view_file: PathBuf,
    /// A book's highlights over the library: index into `books`, the list.
    notebook: Option<(usize, Notebook)>,
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
                let thumb = book.cover_thumb.as_deref();
                let (cover, small) = (cover(thumb, (CELL_W, COVER_H)), cover(thumb, ROW_COVER));
                Entry { book, progress, cover, small }
            })
            .collect();
        let current = (books.iter().enumerate())
            .filter(|(_, b)| b.status() == Filter::Reading)
            .max_by_key(|(_, b)| b.progress.map(|p| p.1))
            .map(|(i, b)| (i, cover(b.book.cover_thumb.as_deref(), CARD_COVER)));
        let view_file = ctx.state_dir.join("home_view.txt");
        let grid = std::fs::read_to_string(&view_file).map_or(true, |v| v.trim() != "list");
        let mut home =
            Home { books, current, filter: Filter::All, grid, shown: Vec::new(), page: 0, view_file, notebook: None };
        home.set_filter(Filter::All);
        home
    }

    fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.shown =
            (0..self.books.len()).filter(|&i| filter == Filter::All || self.books[i].status() == filter).collect();
        self.page = 0;
    }

    /// The "continue reading" card shows on the first page only, leaving later pages to covers.
    fn card(&self, page: usize) -> bool {
        page == 0 && self.current.is_some()
    }

    fn chips_top(&self, page: usize) -> i32 {
        if self.card(page) {
            HEADER + CARD_H + 44
        } else {
            HEADER
        }
    }

    fn items_top(&self, page: usize) -> i32 {
        self.chips_top(page) + CHIPS_H + 36
    }

    /// Books that fit on `page`.
    fn capacity(&self, page: usize) -> usize {
        let height = H - FOOTER - self.items_top(page);
        if self.grid {
            ((height / CELL_H).max(1) * COLS) as usize
        } else {
            (height / ROW_H).max(1) as usize
        }
    }

    /// Index into `shown` of the first book on `page`.
    fn start(&self, page: usize) -> usize {
        (0..page).map(|p| self.capacity(p)).sum()
    }

    fn pages(&self) -> usize {
        let mut pages = 1;
        while self.start(pages) < self.shown.len() {
            pages += 1;
        }
        pages
    }

    /// Item rectangles on the current page: (index into `books`, rect).
    fn items(&self) -> Vec<(usize, Rect)> {
        let (start, top) = (self.start(self.page), self.items_top(self.page));
        let shown = self.shown.iter().skip(start).take(self.capacity(self.page));
        shown
            .enumerate()
            .map(|(n, &i)| {
                let n = n as i32;
                let r = if self.grid {
                    Rect::new(PAD + (n % COLS) * (CELL_W + GUTTER), top + (n / COLS) * CELL_H, CELL_W, CELL_H)
                } else {
                    Rect::new(PAD, top + n * ROW_H, W - 2 * PAD, ROW_H)
                };
                (i, r)
            })
            .collect()
    }

    /// Filter chips' rectangles, measured with the chip font.
    fn chips(&self, label: &Label) -> Vec<(Filter, &'static str, Rect)> {
        let mut x = PAD;
        (FILTERS.iter())
            .map(|&(f, name)| {
                let w = label.width(name) as i32 + 56;
                let r = Rect::new(x, self.chips_top(self.page), w, CHIPS_H);
                x += w + 16;
                (f, name, r)
            })
            .collect()
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some((_, nb)) = &self.notebook {
            return nb.render(c, ctx);
        }
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(serif), Some(serif_bold), Some(sans), Some(sans_bold)) = (
            crate::serif(fonts, false),
            crate::serif(fonts, true),
            crate::sans(fonts, false),
            crate::sans(fonts, true),
        ) else {
            return;
        };
        let faces = Faces { serif, serif_bold, sans, sans_bold };

        Label { face: serif_bold, size: 66.0, ink: INK }.draw(c, raster, "Library", PAD as f32, 150);
        self.view_toggle(c);
        rounded_outline(c, EXIT, EXIT.h / 2, 3, INK, PAPER);
        let exit = Label { face: sans, size: 30.0, ink: INK };
        exit.draw_centred(c, raster, "Exit", (EXIT.x + EXIT.w / 2) as f32, EXIT.y + 47);

        if let (true, Some((i, big))) = (self.card(self.page), &self.current) {
            self.continue_card(c, raster, &faces, &self.books[*i], big.as_ref());
        }

        let chip = Label { face: sans, size: 28.0, ink: INK };
        for (f, name, r) in self.chips(&chip) {
            let on = f == self.filter;
            if on {
                rounded_rect(c, r, r.h / 2, INK);
            } else {
                rounded_outline(c, r, r.h / 2, 2, LINE, PAPER);
            }
            Label { ink: if on { PAPER } else { INK }, ..chip }.draw(c, raster, name, (r.x + 28) as f32, r.y + 42);
        }

        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        if self.shown.is_empty() {
            soft.draw(c, raster, "No books here", PAD as f32, self.items_top(self.page) + 60);
        }
        for (i, r) in self.items() {
            if self.grid {
                grid_cell(c, raster, &faces, &self.books[i], r);
            } else {
                list_row(c, raster, &faces, &self.books[i], r);
            }
        }

        if self.pages() > 1 {
            let label = format!("‹   {} / {}   ›", self.page + 1, self.pages());
            soft.draw_centred(c, raster, &label, W as f32 / 2.0, H - FOOTER / 2 + 10);
        }
    }

    /// Two-segment control: grid (four squares) and list (three bars) icons.
    fn view_toggle(&self, c: &mut Canvas) {
        rounded_outline(c, VIEW, VIEW.h / 2, 2, LINE, PAPER);
        let half = VIEW.w / 2;
        let seg = |n: i32| Rect::new(VIEW.x + n * half, VIEW.y, half, VIEW.h);
        let on = seg(if self.grid { 0 } else { 1 });
        rounded_rect(c, Rect::new(on.x + 4, on.y + 4, on.w - 8, on.h - 8), (on.h - 8) / 2, INK);
        let ink = |active: bool| if active { PAPER } else { INK };
        let (g, l) = (seg(0), seg(1));
        let (gx, gy) = (g.x + g.w / 2 - 15, g.y + g.h / 2 - 15);
        for (dx, dy) in [(0, 0), (17, 0), (0, 17), (17, 17)] {
            rounded_rect(c, Rect::new(gx + dx, gy + dy, 13, 13), 3, ink(self.grid));
        }
        let (lx, ly) = (l.x + l.w / 2 - 17, l.y + l.h / 2 - 13);
        for dy in [0, 11, 22] {
            rounded_rect(c, Rect::new(lx, ly + dy, 34, 5), 2, ink(!self.grid));
        }
    }

    fn continue_card(&self, c: &mut Canvas, raster: &mut Rasterizer, f: &Faces, book: &Entry, big: Option<&GrayImage>) {
        let card = Rect::new(PAD, HEADER, W - 2 * PAD, CARD_H);
        rounded_rect(c, card, 28, FILL);
        let r = Rect::new(card.x + 36, card.y + 36, CARD_COVER.0, CARD_COVER.1);
        draw_cover(c, raster, f, r, big, book, FILL);
        let x = (r.right() + 44) as f32;
        let width = (card.right() - 40) as f32 - x;
        let small = Label { face: f.sans, size: 26.0, ink: INK_SOFT };
        small.draw(c, raster, "CONTINUE READING", x, card.y + 70);
        let title = Label { face: f.serif_bold, size: 46.0, ink: INK };
        title.draw(c, raster, &title.fit(&book.book.title, width), x, card.y + 140);
        let author = Label { face: f.sans, size: 30.0, ink: INK_SOFT };
        author.draw(c, raster, &author.fit(&book.author(), width), x, card.y + 190);
        let progress = book.progress.map_or(0.0, |p| p.0);
        let bar = Rect::new(x as i32, card.bottom() - 70, width as i32 - 110, 10);
        rounded_rect(c, bar, 5, LINE);
        rounded_rect(c, Rect { w: (bar.w as f32 * progress) as i32, ..bar }, 5, INK);
        small.draw(c, raster, &format!("{:.0}%", progress * 100.0), (bar.right() + 24) as f32, bar.y + 12);
    }

    fn set_grid(&mut self, grid: bool) {
        self.grid = grid;
        self.page = 0;
        crate::write_atomic(&self.view_file, if grid { "grid\n" } else { "list\n" }).ok();
    }

    /// The book under `(x, y)`: a cover, a list row or the "continue reading" card.
    fn book_at(&self, (x, y): (i32, i32)) -> Option<usize> {
        if let (true, Some((i, _))) = (self.card(self.page), &self.current) {
            if Rect::new(PAD, HEADER, W - 2 * PAD, CARD_H).contains(x, y) {
                return Some(*i);
            }
        }
        self.items().into_iter().find(|(_, r)| r.contains(x, y)).map(|(i, _)| i)
    }

    /// A long press: a book's highlights.
    pub fn long_press(&mut self, at: (i32, i32), ctx: &Ctx) -> Action {
        if self.notebook.is_some() {
            return Action::None;
        }
        let Some(i) = self.book_at(at) else { return Action::None };
        let b = &self.books[i];
        let highlights = notebook::load(&ctx.state_dir, &b.book.path);
        self.notebook = Some((i, Notebook::new(ctx, &b.book.title, &b.author(), highlights)));
        Action::Redraw { turn: None }
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, y)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), (x, y): (i32, i32), ctx: &Ctx) -> Action {
        if let Some((i, nb)) = &mut self.notebook {
            return match nb.tap((x0, y0), x) {
                notebook::Tap::None => Action::None,
                notebook::Tap::Redraw => Action::Redraw { turn: None },
                notebook::Tap::Back => {
                    self.notebook = None;
                    Action::Redraw { turn: None }
                }
                notebook::Tap::Open(n) => {
                    let path = self.books[*i].book.path.clone();
                    let data = nb.highlights[n].data.clone();
                    self.notebook = None;
                    Action::OpenAt(path, Box::new(data))
                }
            };
        }
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
        let inside = |r: Rect| r.contains(x0, y0);
        if inside(EXIT) {
            return Action::Quit;
        }
        if inside(VIEW) {
            let grid = x0 < VIEW.x + VIEW.w / 2;
            if grid == self.grid {
                return Action::None;
            }
            self.set_grid(grid);
            return Action::Redraw { turn: None };
        }
        if y0 > H - FOOTER {
            return turn(&mut self.page, x0 > W / 2);
        }
        if let Some(sans) = ctx.sans(false) {
            let hit = self.chips(&Label { face: sans, size: 28.0, ink: INK }).into_iter().find(|c| inside(c.2));
            if let Some((f, _, _)) = hit {
                if f == self.filter {
                    return Action::None;
                }
                self.set_filter(f);
                return Action::Redraw { turn: None };
            }
        }
        match self.book_at((x0, y0)) {
            Some(i) => Action::Open(self.books[i].book.path.clone()),
            None => Action::None,
        }
    }
}

struct Faces<'a> {
    serif: &'a Face,
    serif_bold: &'a Face,
    sans: &'a Face,
    sans_bold: &'a Face,
}

/// A grid cell: the cover with a soft offset shadow, then title (two lines) and author.
fn grid_cell(c: &mut Canvas, raster: &mut Rasterizer, f: &Faces, book: &Entry, r: Rect) {
    let cover = Rect::new(r.x, r.y, CELL_W, COVER_H);
    let drawn = draw_cover(c, raster, f, cover, book.cover.as_ref(), book, PAPER);
    let mut y = drawn.bottom() + 20;
    match (book.status(), book.progress) {
        (Filter::Finished, _) => {
            let badge = Label { face: f.sans_bold, size: 22.0, ink: PAPER };
            let w = badge.width("READ") as i32 + 28;
            let b = Rect::new(drawn.x + 12, drawn.bottom() - 52, w, 40);
            rounded_rect(c, b, 20, INK);
            badge.draw(c, raster, "READ", (b.x + 14) as f32, b.y + 29);
        }
        (_, Some((p, _))) => {
            let bar = Rect::new(drawn.x, y - 4, drawn.w, 6);
            rounded_rect(c, bar, 3, LINE);
            rounded_rect(c, Rect { w: ((bar.w as f32 * p) as i32).max(6), ..bar }, 3, INK);
            y += 14;
        }
        _ => {}
    }
    let title = Label { face: f.serif, size: 31.0, ink: INK };
    let lines = title.wrap(&book.book.title, CELL_W as f32, 2);
    for (n, line) in lines.iter().enumerate() {
        title.draw(c, raster, line, r.x as f32, y + 30 + n as i32 * 38);
    }
    let author = Label { face: f.sans, size: 24.0, ink: INK_SOFT };
    let ay = y + 30 + lines.len() as i32 * 38 + 4;
    author.draw(c, raster, &author.fit(&book.author(), CELL_W as f32), r.x as f32, ay);
}

/// A list row: cover, title, author, status on the right.
fn list_row(c: &mut Canvas, raster: &mut Rasterizer, f: &Faces, book: &Entry, r: Rect) {
    let cover = Rect::new(r.x, r.y + 20, ROW_COVER.0, ROW_COVER.1);
    draw_cover(c, raster, f, cover, book.small.as_ref(), book, PAPER);
    let x = (cover.right() + 36) as f32;
    let right = r.right() as f32;
    let soft = Label { face: f.sans, size: 28.0, ink: INK_SOFT };
    let status = match (book.status(), book.progress) {
        (Filter::Finished, _) => Some("Read".to_string()),
        (_, Some((p, _))) => Some(format!("{:.0}%", p * 100.0)),
        _ => None,
    };
    let status_w = status.as_deref().map_or(0.0, |s| soft.width(s) + 24.0);
    let title = Label { face: f.serif, size: 40.0, ink: INK };
    title.draw(c, raster, &title.fit(&book.book.title, right - x - status_w), x, r.y + 84);
    let author = Label { face: f.sans, size: 28.0, ink: INK_SOFT };
    author.draw(c, raster, &author.fit(&book.author(), right - x - status_w), x, r.y + 130);
    if let Some(s) = status {
        soft.draw(c, raster, &s, right - soft.width(&s), r.y + 84);
    }
    c.fill(Rect::new(x as i32, r.bottom() - 1, r.right() - x as i32, 1), LINE);
}

/// Draws the cover bottom-aligned and centred in `r` with rounded corners and a flat offset
/// shadow; without an image, a typographic cover (title and author on dark gray).
/// Returns the rectangle the cover occupies.
fn draw_cover(
    c: &mut Canvas,
    raster: &mut Rasterizer,
    f: &Faces,
    r: Rect,
    img: Option<&GrayImage>,
    book: &Entry,
    bg: u8,
) -> Rect {
    let at = match img {
        Some(img) => {
            let (w, h) = (img.width as i32, img.height as i32);
            Rect::new(r.x + (r.w - w) / 2, r.y + (r.h - h), w, h)
        }
        None => r,
    };
    let shadow = Rect::new(at.x + 5, at.y + 7, at.w, at.h);
    rounded_rect(c, shadow, RADIUS, if bg == PAPER { 215 } else { 200 });
    match img {
        Some(img) => {
            image::draw(c, img, at.x, at.y);
            // The bottom-right corner sits on the shadow, the others on the page.
            let tone = if bg == PAPER { 215 } else { 200 };
            clip_corners(c, at, RADIUS, [bg, bg, bg, tone]);
        }
        None => {
            rounded_rect(c, at, RADIUS, 60);
            let size = (at.w as f32 * 0.13).clamp(16.0, 40.0);
            let title = Label { face: f.serif_bold, size, ink: PAPER };
            let lines = title.wrap(&book.book.title, at.w as f32 * 0.8, 4);
            let line_h = (size * 1.25) as i32;
            let cx = (at.x + at.w / 2) as f32;
            let top = at.y + at.h / 3 - lines.len() as i32 * line_h / 2;
            for (n, line) in lines.iter().enumerate() {
                title.draw_centred(c, raster, line, cx, top + n as i32 * line_h);
            }
            let author = Label { face: f.sans, size: size * 0.6, ink: 210 };
            let a = author.fit(&book.author(), at.w as f32 * 0.8);
            author.draw_centred(c, raster, &a, cx, at.bottom() - at.h / 8);
        }
    }
    at
}
