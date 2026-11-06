//! Home: the book being read on top (first page only), filter chips, then the library as a
//! gallery of covers or as a list, a page at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::font::{Face, Rasterizer};
use zarathustra_engine::image::{self, GrayImage};
use zarathustra_engine::library::{BookEntry, Format, Library, SortBy};
use zarathustra_engine::search::fold;

use crate::about::About;
use crate::bookmenu::{self, BookMenu, Item};
use crate::i18n::t;
use crate::keyboard::{self, Key, Keyboard};
use crate::notebook::{self, Notebook};
use crate::prefs::Prefs;
use crate::reader::Position;
use crate::stats::StatsScreen;
use crate::ui::{clip_corners, rounded_outline, rounded_rect, Label, FILL, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, Landing, H, W};

const PAD: i32 = 72;
const HEADER: i32 = 210;
const CARD_H: i32 = 330;
const CARD_COVER: (i32, i32) = (172, 258);
const CHIPS_H: i32 = 64;
const FOOTER: i32 = 110;
/// Grid: three columns of covers at the books' usual 2:3 ratio, title and author below.
const COLS: i32 = 3;
/// Cell height sized so pages without the "continue reading" card hold two rows.
const CELL_H: i32 = (H - FOOTER - HEADER - CHIPS_H - 36) / 2;
const COVER_H: i32 = CELL_H - 150;
const CELL_W: i32 = COVER_H * 2 / 3;
const GUTTER: i32 = (W - 2 * PAD - COLS * CELL_W) / (COLS - 1);
const _: () = assert!(GUTTER >= 48, "covers keep their breathing room");
/// List: one book per row.
const ROW_H: i32 = 190;
const ROW_COVER: (i32, i32) = (100, 150);
const RADIUS: i32 = 10;
const POWER: Rect = Rect { x: W - PAD - 72, y: 82, w: 72, h: 72 };
const VIEW: Rect = Rect { x: POWER.x - 32 - 192, y: 82, w: 192, h: 72 };
/// The power menu, hanging under its button.
const POWER_ROW_H: i32 = 100;
const POWER_MENU: Rect = Rect { x: W - PAD - 440, y: POWER.y + POWER.h + 16, w: 440, h: 4 * POWER_ROW_H + 16 };
const STATS: Rect = Rect { x: VIEW.x - 32 - 72, y: 82, w: 72, h: 72 };
const SEARCH: Rect = Rect { x: STATS.x - 32 - 72, y: 82, w: 72, h: 72 };
const GEAR: Rect = Rect { x: SEARCH.x - 32 - 72, y: 82, w: 72, h: 72 };
/// The search field, over the header's title and buttons while searching.
const FIELD: Rect = Rect { x: PAD, y: 82, w: W - 2 * PAD, h: 72 };
const CLEAR: Rect = Rect { x: FIELD.x + FIELD.w - 96, y: FIELD.y, w: 96, h: FIELD.h };
const MAX_QUERY: usize = 40;
/// Progress from which a book counts as read.
const FINISHED: f32 = 0.99;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Unread,
    Reading,
    Finished,
}

/// In the order of `Strings::filters`.
const FILTERS: [Filter; 4] = [Filter::All, Filter::Unread, Filter::Reading, Filter::Finished];

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

/// A screen or sheet over the library; `usize` is the book's index in `books`.
enum Overlay {
    Menu(usize, BookMenu),
    Notebook(usize, Notebook),
    About(usize, Box<About>),
    Stats(StatsScreen),
    Prefs(Prefs),
    /// Suspend, exit, restart, power off.
    Power,
}

/// Library order: title, author, or recently read first (then recently added).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Title,
    Author,
    Recent,
}

const SORTS: [Sort; 3] = [Sort::Title, Sort::Author, Sort::Recent];

pub struct Home {
    books: Vec<Entry>,
    /// The book in the "continue reading" card, if any, and its card-size cover.
    current: Option<(usize, Option<GrayImage>)>,
    filter: Filter,
    grid: bool,
    sort: Sort,
    /// Position of each book (index into `books`, which is in title order) by author.
    author_rank: Vec<u32>,
    /// Indices of `books` shown under the current filter.
    shown: Vec<usize>,
    page: usize,
    view_file: PathBuf,
    /// A book's highlights over the library: index into `books`, the list.
    overlay: Option<Overlay>,
    /// Searching the library: the text typed (titles and authors containing it are shown).
    query: Option<String>,
    /// The keyboard, up while typing the query.
    keyboard: Option<Keyboard>,
    /// Where the books are, to scan again after a delete.
    dirs: Vec<PathBuf>,
}

/// A book's file time and its grid and list covers, kept across a refresh.
type Covers = (SystemTime, Option<GrayImage>, Option<GrayImage>);

fn library(dirs: &[PathBuf], ctx: &Ctx) -> Library {
    let paths: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    Library::scan(&paths, &ctx.state_dir.join("library"))
}

/// Reading progress of the book at `path` and when it was saved; `None` if never opened.
fn progress(ctx: &Ctx, path: &Path) -> Option<(f32, SystemTime)> {
    let state = ctx.state_file(path);
    Position::load(&state).and_then(|p| Some((p.progress, std::fs::metadata(&state).ok()?.modified().ok()?)))
}

/// The book for the "continue reading" card: the last one read, not finished.
fn current(books: &[Entry]) -> Option<(usize, Option<GrayImage>)> {
    (books.iter().enumerate())
        .filter(|(_, b)| b.status() == Filter::Reading)
        .max_by_key(|(_, b)| b.progress.map(|p| p.1))
        .map(|(i, b)| (i, cover(b.book.cover_thumb.as_deref(), CARD_COVER)))
}

fn cover(thumb: Option<&Path>, (w, h): (i32, i32)) -> Option<GrayImage> {
    let mut img = image::decode(&std::fs::read(thumb?).ok()?, w as u32, h as u32)?;
    image::quantize16(&mut img);
    Some(img)
}

impl Home {
    pub fn scan(dirs: &[PathBuf], ctx: &Ctx) -> Home {
        Self::build(library(dirs, ctx), dirs, ctx, HashMap::new())
    }

    /// Picks up books added, removed or replaced on disk since the last look (a wireless
    /// transfer, a delete, a copy over USB); true if the library changed. Unchanged books keep
    /// their decoded covers, and the view (filter, search, page) stays. Nothing changed costs a
    /// walk of the library folders and a stat per file. Skipped while a sheet is open: it
    /// holds a book's index.
    pub fn refresh(&mut self, ctx: &Ctx) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        let library = library(&self.dirs, ctx);
        let known: HashMap<&Path, SystemTime> =
            self.books.iter().map(|b| (b.book.path.as_path(), b.book.modified)).collect();
        let same = library.books.len() == known.len()
            && library.books.iter().all(|b| known.get(b.path.as_path()) == Some(&b.modified));
        if same {
            return false;
        }
        let covers = (std::mem::take(&mut self.books).into_iter())
            .map(|e| (e.book.path, (e.book.modified, e.cover, e.small)))
            .collect();
        let old = std::mem::replace(self, Self::build(library, &self.dirs.clone(), ctx, covers));
        self.query = old.query;
        self.keyboard = old.keyboard;
        self.set_filter(old.filter);
        self.page = old.page.min(self.pages() - 1);
        true
    }

    /// The home for `library`; covers come from `covers` (path → file time, grid and list
    /// covers) when the file is unchanged, else are decoded.
    fn build(library: Library, dirs: &[PathBuf], ctx: &Ctx, mut covers: HashMap<PathBuf, Covers>) -> Home {
        let order = library.sorted(SortBy::Title);
        // Library index → title position, to carry the author order over to `books`.
        let mut title_pos = vec![0u32; order.len()];
        for (pos, &i) in order.iter().enumerate() {
            title_pos[i] = pos as u32;
        }
        let mut author_rank = vec![0u32; order.len()];
        for (rank, i) in library.sorted(SortBy::Author).into_iter().enumerate() {
            author_rank[title_pos[i] as usize] = rank as u32;
        }
        let mut slots: Vec<Option<BookEntry>> = library.books.into_iter().map(Some).collect();
        let books: Vec<Entry> = (order.into_iter())
            .filter_map(|i| slots[i].take())
            .map(|book| {
                let progress = progress(ctx, &book.path);
                let (cover, small) = match covers.remove(&book.path).filter(|c| c.0 == book.modified) {
                    Some((_, cover, small)) => (cover, small),
                    None => {
                        let thumb = book.cover_thumb.as_deref();
                        (cover(thumb, (CELL_W, COVER_H)), cover(thumb, ROW_COVER))
                    }
                };
                Entry { book, progress, cover, small }
            })
            .collect();
        let current = current(&books);
        let view_file = ctx.state_dir.join("home_view.txt");
        // "grid|list [title|author|recent]"
        let view = std::fs::read_to_string(&view_file).unwrap_or_default();
        let mut words = view.split_whitespace();
        let grid = words.next() != Some("list");
        let sort = match words.next() {
            Some("author") => Sort::Author,
            Some("recent") => Sort::Recent,
            _ => Sort::Title,
        };
        let mut home = Home {
            books,
            current,
            filter: Filter::All,
            grid,
            sort,
            author_rank,
            shown: Vec::new(),
            page: 0,
            view_file,
            overlay: None,
            query: None,
            keyboard: None,
            dirs: dirs.to_vec(),
        };
        home.set_filter(Filter::All);
        home
    }

    fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        let query = self.query.as_deref().map(|q| fold(q).trim().to_string()).filter(|q| !q.is_empty());
        let matches = |b: &Entry| {
            query.as_ref().is_none_or(|q| fold(&format!("{} {}", b.book.title, b.author())).contains(q.as_str()))
        };
        self.shown = (0..self.books.len())
            .filter(|&i| filter == Filter::All || self.books[i].status() == filter)
            .filter(|&i| matches(&self.books[i]))
            .collect();
        let books = &self.books;
        match self.sort {
            Sort::Title => {}
            Sort::Author => self.shown.sort_by_key(|&i| self.author_rank[i]),
            // Last read first, then the never opened by newest file.
            Sort::Recent => self.shown.sort_by(|&a, &b| {
                let key = |i: usize| (books[i].progress.map(|p| p.1), books[i].book.modified);
                key(b).cmp(&key(a))
            }),
        }
        self.page = 0;
    }

    /// The "continue reading" card shows on the first page only, leaving later pages to covers.
    fn card(&self, page: usize) -> bool {
        page == 0 && self.current.is_some() && self.query.is_none()
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
        let bottom = if self.keyboard.is_some() { H - keyboard::KEYBOARD_H } else { H - FOOTER };
        let height = bottom - self.items_top(page);
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
        (FILTERS.into_iter().zip(t().filters))
            .map(|(f, name)| {
                let w = label.width(name) as i32 + 56;
                let r = Rect::new(x, self.chips_top(self.page), w, CHIPS_H);
                x += w + 16;
                (f, name, r)
            })
            .collect()
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        match &self.overlay {
            None => self.render_library(c, ctx),
            Some(Overlay::Menu(_, menu)) => {
                self.render_library(c, ctx);
                menu.render(c, ctx);
            }
            Some(Overlay::Notebook(_, nb)) => nb.render(c, ctx),
            Some(Overlay::About(i, about)) => about.render(c, ctx, self.books[*i].cover.as_ref()),
            Some(Overlay::Stats(stats)) => stats.render(c, ctx),
            Some(Overlay::Prefs(p)) => p.render(c, ctx),
            Some(Overlay::Power) => {
                self.render_library(c, ctx);
                power_menu(c, ctx);
            }
        }
    }

    fn render_library(&self, c: &mut Canvas, ctx: &mut Ctx) {
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

        match &self.query {
            Some(q) => {
                rounded_outline(c, FIELD, FIELD.h / 2, 3, INK, PAPER);
                crate::search::icon(c, Rect::new(FIELD.x + 28, FIELD.y + 20, 32, 32), INK, PAPER);
                let x = (FIELD.x + 84) as f32;
                let width = (CLEAR.x - FIELD.x - 84) as f32;
                if q.is_empty() {
                    Label { face: sans, size: 30.0, ink: INK_SOFT }.draw(
                        c,
                        raster,
                        t().search_library,
                        x,
                        FIELD.y + 47,
                    );
                } else {
                    // The end of a long query stays in view.
                    let text = Label { face: sans, size: 32.0, ink: INK };
                    let skip = (0..q.len()).filter(|&i| q.is_char_boundary(i)).find(|&i| text.width(&q[i..]) <= width);
                    text.draw(c, raster, &q[skip.unwrap_or(0)..], x, FIELD.y + 48);
                }
                Label { face: sans, size: 40.0, ink: INK }.draw_centred(
                    c,
                    raster,
                    "×",
                    (CLEAR.x + CLEAR.w / 2) as f32,
                    CLEAR.y + 50,
                );
            }
            None => {
                Label { face: serif_bold, size: 66.0, ink: INK }.draw(c, raster, t().library, PAD as f32, 150);
                self.view_toggle(c);
                stats_button(c);
                gear_button(c);
                rounded_outline(c, SEARCH, SEARCH.h / 2, 2, LINE, PAPER);
                crate::search::icon(c, Rect::new(SEARCH.x + 22, SEARCH.y + 22, 28, 28), INK, PAPER);
                power_button(c);
            }
        }

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
        let (text, r) = self.sort_chip(&chip);
        rounded_outline(c, r, r.h / 2, 2, LINE, PAPER);
        chip.draw(c, raster, &text, (r.x + 28) as f32, r.y + 42);

        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        if self.shown.is_empty() {
            soft.draw(c, raster, t().no_books, PAD as f32, self.items_top(self.page) + 60);
        }
        for (i, r) in self.items() {
            if self.grid {
                grid_cell(c, raster, &faces, &self.books[i], r);
            } else {
                list_row(c, raster, &faces, &self.books[i], r);
            }
        }

        if let Some(kb) = &self.keyboard {
            kb.render(c, ctx);
        } else if self.pages() > 1 {
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
        small.draw(c, raster, t().continue_reading, x, card.y + 70);
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
        self.save_view();
    }

    fn save_view(&self) {
        let sort = match self.sort {
            Sort::Title => "title",
            Sort::Author => "author",
            Sort::Recent => "recent",
        };
        let view = format!("{} {sort}\n", if self.grid { "grid" } else { "list" });
        if let Err(e) = crate::write_atomic(&self.view_file, view) {
            eprintln!("home view: {e}");
        }
    }

    /// The sort chip at the right end of the filter row: "↕ Titolo".
    fn sort_chip(&self, label: &Label) -> (String, Rect) {
        let text = format!("↕  {}", t().sorts[SORTS.iter().position(|&s| s == self.sort).unwrap_or(0)]);
        let w = label.width(&text) as i32 + 56;
        (text, Rect::new(W - PAD - w, self.chips_top(self.page), w, CHIPS_H))
    }

    /// The book under `(x, y)`: a cover, a list row or the "continue reading" card.
    fn book_at(&self, at: (i32, i32)) -> Option<usize> {
        self.book_rect_at(at).map(|(i, _)| i)
    }

    /// The book at `(x, y)` and its cover (grid) or row (list, the continue-reading card).
    fn book_rect_at(&self, (x, y): (i32, i32)) -> Option<(usize, Rect)> {
        if let (true, Some((i, _))) = (self.card(self.page), &self.current) {
            let card = Rect::new(PAD, HEADER, W - 2 * PAD, CARD_H);
            if card.contains(x, y) {
                return Some((*i, card));
            }
        }
        let (i, r) = self.items().into_iter().find(|(_, r)| r.contains(x, y))?;
        // In the grid, the cover alone: the title under it stays readable beside the card.
        Some((i, if self.grid { Rect::new(r.x, r.y, r.w, COVER_H) } else { r }))
    }

    /// A long press on a book: its menu.
    pub fn long_press(&mut self, at: (i32, i32), ctx: &Ctx) -> Action {
        if self.overlay.is_some() {
            return Action::None;
        }
        if let Some(kb) = self.keyboard.as_mut().filter(|_| keyboard::area().contains(at.0, at.1)) {
            return kb.long_press(at.0, at.1).map_or(Action::None, Action::Region);
        }
        let Some((i, anchor)) = self.book_rect_at(at) else { return Action::None };
        let b = &self.books[i];
        let highlights = notebook::load(&ctx.state_dir, &b.book.path).len();
        let finished = b.status() == Filter::Finished;
        let reflowable = b.book.format != Format::Pdf;
        let menu = BookMenu::new(
            &b.book.title,
            &b.author(),
            b.progress.map(|p| p.0),
            finished,
            highlights,
            reflowable,
            anchor,
        );
        self.overlay = Some(Overlay::Menu(i, menu));
        Action::Redraw { turn: None }
    }

    /// Touches while a sheet or screen covers the library.
    fn overlay_tap(&mut self, start: (i32, i32), x: i32, ctx: &Ctx) -> Action {
        let redraw = Action::Redraw { turn: None };
        let Some(overlay) = &mut self.overlay else { return Action::None };
        match overlay {
            Overlay::Menu(i, menu) => {
                let i = *i;
                let b = &self.books[i];
                match menu.tap(start) {
                    bookmenu::Tap::None => return Action::None,
                    bookmenu::Tap::Redraw => {}
                    bookmenu::Tap::Close => self.overlay = None,
                    bookmenu::Tap::Item(it @ (Item::Open | Item::Contents | Item::Bookmarks | Item::Search)) => {
                        self.overlay = None;
                        let landing = match it {
                            Item::Contents => Landing::Contents,
                            Item::Bookmarks => Landing::Bookmarks,
                            Item::Search => Landing::Search,
                            _ => Landing::Resume,
                        };
                        return Action::Open(b.book.path.clone(), landing);
                    }
                    bookmenu::Tap::Item(Item::Delete) => {
                        self.overlay = None;
                        return self.delete(i, ctx);
                    }
                    bookmenu::Tap::Item(Item::About) => {
                        let about = About::new(&ctx.state_dir, &b.book.path, &b.book.title, &b.author());
                        self.overlay = Some(Overlay::About(i, Box::new(about)));
                    }
                    bookmenu::Tap::Item(Item::Highlights) => {
                        let list = notebook::load(&ctx.state_dir, &b.book.path);
                        self.overlay = Some(Overlay::Notebook(i, Notebook::new(ctx, &b.book.title, &b.author(), list)));
                    }
                    bookmenu::Tap::Item(Item::MarkRead(read)) => {
                        self.overlay = None;
                        self.mark_read(i, read, ctx);
                    }
                }
                redraw
            }
            Overlay::Notebook(i, nb) => match nb.tap(start, x) {
                notebook::Tap::None => Action::None,
                notebook::Tap::Redraw => redraw,
                notebook::Tap::Back => {
                    self.overlay = None;
                    redraw
                }
                notebook::Tap::Open(n) => {
                    let path = self.books[*i].book.path.clone();
                    let data = nb.highlights[n].data.clone();
                    self.overlay = None;
                    Action::Open(path, Landing::At(Box::new(data)))
                }
            },
            Overlay::About(_, about) if about.back(start) => {
                self.overlay = None;
                redraw
            }
            Overlay::Stats(stats) if stats.back(start) => {
                self.overlay = None;
                redraw
            }
            Overlay::Prefs(p) => p.tap(start).unwrap_or_else(|| {
                self.overlay = None;
                redraw
            }),
            Overlay::Power => {
                self.overlay = None;
                let (x0, y0) = start;
                let row = (y0 - POWER_MENU.y - 8).div_euclid(POWER_ROW_H);
                match (POWER_MENU.contains(x0, y0), row) {
                    (true, 0) => Action::Sleep,
                    (true, 1) => Action::Quit,
                    (true, 2) => Action::Reboot,
                    (true, 3) => Action::PowerOff,
                    _ => redraw,
                }
            }
            Overlay::About(..) | Overlay::Stats(_) => Action::None,
        }
    }

    /// Back from reading `path`: its progress (and so the card, filters and order) as saved,
    /// without scanning the library or decoding covers again.
    pub fn returned(&mut self, path: &Path, ctx: &Ctx) {
        if let Some(i) = self.books.iter().position(|b| b.book.path == path) {
            self.books[i].progress = progress(ctx, path);
            if self.current.as_ref().is_none_or(|(c, _)| *c != i) || self.books[i].status() != Filter::Reading {
                self.current = current(&self.books);
            }
        }
        let page = self.page;
        self.set_filter(self.filter);
        self.page = page.min(self.pages() - 1);
    }

    /// Deletes book `i`'s file and its reading position, then reloads the library.
    // ponytail: highlights and bookmarks stay in the database (they return if a book is added
    // again at the same path, even a different one); calibre's sidecars (cover.jpg,
    // metadata.opf) and the folder stay too; the library is scanned again, so filter and page reset.
    // Add a `Store::delete_book` and remove the entry in place when that shows up in use.
    fn delete(&mut self, i: usize, ctx: &Ctx) -> Action {
        let path = &self.books[i].book.path;
        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("delete {}: {e}", path.display());
            return Action::Redraw { turn: None };
        }
        let _ = std::fs::remove_file(ctx.state_file(path)); // a never-opened book has none
        self.refresh(ctx);
        Action::Redraw { turn: None }
    }

    /// Marks book `i` as read (progress 100%, the position kept) or unread (the saved
    /// position removed: the book starts over).
    fn mark_read(&mut self, i: usize, read: bool, ctx: &Ctx) {
        let state = ctx.state_file(&self.books[i].book.path);
        let done = if read {
            let p = Position::load(&state);
            let (chapter, block, offset) = p.map_or((0, 0, 0), |p| (p.chapter, p.block, p.offset));
            crate::write_atomic(&state, format!("{chapter} {block} 1.0000 {offset}\n"))
        } else {
            std::fs::remove_file(&state)
                .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
        };
        if let Err(e) = done {
            return eprintln!("mark read: {e}");
        }
        self.books[i].progress = read.then(|| (1.0, SystemTime::now()));
        if self.current.as_ref().is_some_and(|(c, _)| *c == i) {
            self.current = None;
        }
        let page = self.page;
        self.set_filter(self.filter);
        self.page = page.min(self.pages() - 1);
    }

    /// The finger went down: a key of the search keyboard shows pressed. Returns its area.
    pub fn press(&mut self, (x, y): (i32, i32)) -> Option<Rect> {
        self.keyboard.as_mut().filter(|_| self.overlay.is_none())?.press(x, y)
    }

    /// A tap while searching on the field, its clear button or the keyboard; `None` for the
    /// rest of the screen (chips, books, pages), handled as usual.
    fn search_tap(&mut self, (x0, y0): (i32, i32)) -> Option<Action> {
        let q = self.query.as_mut()?;
        if CLEAR.contains(x0, y0) {
            self.query = None;
            self.keyboard = None;
            self.set_filter(self.filter);
            return Some(Action::Redraw { turn: None });
        }
        if FIELD.contains(x0, y0) {
            return Some(match self.keyboard {
                Some(_) => Action::None,
                None => {
                    self.keyboard = Some(Keyboard::default());
                    Action::Redraw { turn: None }
                }
            });
        }
        let kb = self.keyboard.as_mut().filter(|_| keyboard::area().contains(x0, y0))?;
        let Some((key, rect)) = kb.tap(x0, y0) else { return Some(Action::None) };
        match key {
            Key::Char(c) if q.chars().count() < MAX_QUERY => q.push(c),
            Key::Space if !q.is_empty() && !q.ends_with(' ') => q.push(' '),
            Key::Backspace => {
                q.pop();
            }
            Key::Enter => {
                self.keyboard = None;
                return Some(Action::Redraw { turn: None });
            }
            // The layer switched, a full field or a second space: just the keyboard.
            _ => return Some(Action::Region(rect)),
        }
        self.set_filter(self.filter);
        // Results and field change above the keyboard: no flash while typing.
        Some(Action::Region(Rect::new(0, 0, W, H - keyboard::KEYBOARD_H).union(&rect)))
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, y)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), (x, y): (i32, i32), ctx: &Ctx) -> Action {
        if self.overlay.is_some() {
            return self.overlay_tap((x0, y0), x, ctx);
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
        if self.query.is_some() {
            if let Some(a) = self.search_tap((x0, y0)) {
                return a;
            }
        } else if inside(SEARCH) {
            self.query = Some(String::new());
            self.keyboard = Some(Keyboard::default());
            self.set_filter(self.filter);
            return Action::Redraw { turn: None };
        } else if inside(GEAR) {
            self.overlay = Some(Overlay::Prefs(Prefs::new(crate::settings::Settings::load(&ctx.state_dir))));
            return Action::Redraw { turn: None };
        } else if inside(STATS) {
            let finished = self.books.iter().filter(|b| b.status() == Filter::Finished).count();
            self.overlay = Some(Overlay::Stats(StatsScreen::new(&ctx.state_dir, finished)));
            return Action::Redraw { turn: None };
        }
        if inside(POWER) && self.query.is_none() {
            self.overlay = Some(Overlay::Power);
            return Action::Redraw { turn: None };
        }
        if inside(VIEW) && self.query.is_none() {
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
            let chip = Label { face: sans, size: 28.0, ink: INK };
            if inside(self.sort_chip(&chip).1) {
                let next = SORTS.iter().position(|&s| s == self.sort).map_or(0, |i| (i + 1) % SORTS.len());
                self.sort = SORTS[next];
                self.save_view();
                self.set_filter(self.filter);
                return Action::Redraw { turn: None };
            }
            let hit = self.chips(&chip).into_iter().find(|c| inside(c.2));
            if let Some((f, _, _)) = hit {
                if f == self.filter {
                    return Action::None;
                }
                self.set_filter(f);
                return Action::Redraw { turn: None };
            }
        }
        match self.book_at((x0, y0)) {
            Some(i) => Action::Open(self.books[i].book.path.clone(), Landing::Resume),
            None => Action::None,
        }
    }
}

/// The settings button: a cog (eight square teeth round a ring, a hole in the middle) in a
/// ring, sampled 4×4 per pixel so its curves come out smooth in gray.
fn gear_button(c: &mut Canvas) {
    rounded_outline(c, GEAR, GEAR.h / 2, 2, LINE, PAPER);
    let (cx, cy) = ((GEAR.x + GEAR.w / 2) as f32, (GEAR.y + GEAR.h / 2) as f32);
    let (body, tip, hole, teeth) = (12.5f32, 18.0f32, 5.5f32, 8.0f32);
    let inside = |x: f32, y: f32| {
        let (r, a) = (x.hypot(y), y.atan2(x));
        // Distance from the nearest tooth's centre line, along the arc at the tip.
        let sector = std::f32::consts::TAU / teeth;
        let off = (a.rem_euclid(sector) - sector / 2.0).abs() * tip;
        let rim = if off < 4.0 { tip } else { body };
        r <= rim && r >= hole
    };
    let span = tip.ceil() as i32 + 1;
    for py in -span..=span {
        for px in -span..=span {
            let hits = (0..16)
                .filter(|k| {
                    let (sx, sy) = (px as f32 + (k % 4) as f32 / 4.0 + 0.125, py as f32 + (k / 4) as f32 / 4.0 + 0.125);
                    inside(sx - 0.5, sy - 0.5)
                })
                .count() as i32;
            if hits > 0 {
                let ink = PAPER as i32 - (PAPER as i32 - INK as i32) * hits / 16;
                c.fill(Rect::new(cx as i32 + px, cy as i32 + py, 1, 1), ink as u8);
            }
        }
    }
}

/// The power button: the standby symbol, a ring open at the top crossed by a bar.
fn power_button(c: &mut Canvas) {
    rounded_outline(c, POWER, POWER.h / 2, 2, LINE, PAPER);
    let (cx, cy) = ((POWER.x + POWER.w / 2) as f32, (POWER.y + POWER.h / 2 + 2) as f32);
    let inside = |x: f32, y: f32| {
        let r = x.hypot(y);
        // The ring, with a gap of ±40° round the top for the bar.
        let ring = (11.5..=15.5).contains(&r) && (y > 0.0 || x.abs() > -y * 0.84);
        let bar = x.abs() <= 2.0 && (-19.0..=-3.0).contains(&y);
        ring || bar
    };
    for py in -21..=17 {
        for px in -17..=17 {
            let hits = (0..16)
                .filter(|k| inside(px as f32 + (k % 4) as f32 / 4.0 - 0.375, py as f32 + (k / 4) as f32 / 4.0 - 0.375))
                .count() as i32;
            if hits > 0 {
                let ink = PAPER as i32 - (PAPER as i32 - INK as i32) * hits / 16;
                c.fill(Rect::new(cx as i32 + px, cy as i32 + py, 1, 1), ink as u8);
            }
        }
    }
}

/// The power menu: four rows under the button, each one tap.
fn power_menu(c: &mut Canvas, ctx: &mut Ctx) {
    rounded_outline(c, POWER_MENU, 24, 4, INK, PAPER);
    let Ctx { fonts, raster, .. } = ctx;
    let Some(serif) = crate::serif(fonts, false) else { return };
    let label = Label { face: serif, size: 34.0, ink: INK };
    let s = t();
    for (i, text) in [s.standby, s.exit, s.restart, s.power_off].into_iter().enumerate() {
        let r = Rect::new(POWER_MENU.x, POWER_MENU.y + 8 + i as i32 * POWER_ROW_H, POWER_MENU.w, POWER_ROW_H);
        if i > 0 {
            c.fill(Rect::new(r.x + 32, r.y, r.w - 64, 2), LINE);
        }
        label.draw(c, raster, text, (r.x + 40) as f32, label.baseline(r));
    }
}

/// The statistics button: three rising bars in a ring.
fn stats_button(c: &mut Canvas) {
    rounded_outline(c, STATS, STATS.h / 2, 2, LINE, PAPER);
    let (x, base) = (STATS.x + STATS.w / 2 - 17, STATS.y + STATS.h / 2 + 14);
    for (n, h) in [14, 22, 30].into_iter().enumerate() {
        rounded_rect(c, Rect::new(x + n as i32 * 12, base - h, 8, h), 2, INK);
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
            let w = badge.width(t().finished_badge) as i32 + 28;
            let b = Rect::new(drawn.x + 12, drawn.bottom() - 52, w, 40);
            rounded_rect(c, b, 20, INK);
            badge.draw(c, raster, t().finished_badge, (b.x + 14) as f32, b.y + 29);
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
        (Filter::Finished, _) => Some(t().finished.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_picks_up_new_and_removed_books() {
        let book = Path::new("../../dev-books/etica.epub");
        if !book.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("zarathustra-refresh-{}", std::process::id()));
        let (books, state) = (dir.join("books"), dir.join("state"));
        std::fs::create_dir_all(&books).expect("dirs");
        std::fs::copy(book, books.join("a.epub")).expect("copy");
        let mut ctx = Ctx::new();
        ctx.state_dir = state;
        let mut home = Home::scan(std::slice::from_ref(&books), &ctx);
        assert_eq!(home.books.len(), 1);
        assert!(!home.refresh(&ctx), "nothing changed");
        std::fs::copy(book, books.join("b.epub")).expect("copy");
        assert!(home.refresh(&ctx));
        assert_eq!(home.books.len(), 2);
        std::fs::remove_file(books.join("a.epub")).expect("remove");
        assert!(home.refresh(&ctx));
        assert_eq!(home.books.len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
