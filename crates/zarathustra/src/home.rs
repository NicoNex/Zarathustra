//! Home: the book being read on top (first page only), filter chips, then the library as a
//! gallery of covers or as a list, a page at a time.

use std::cell::OnceCell;
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
use crate::extensions::{book_actions, Extensions};
use crate::i18n::t;
use crate::keyboard::{self, Key, Keyboard, Typed};
use crate::notebook::{self, Notebook};
use crate::plugin::Plugin;
use crate::prefs::Prefs;
use crate::reader::Position;
use crate::stats::{self, StatsScreen};
use crate::transfer::{self, Transfer};
use crate::ui::{clip_corners, rounded_outline, rounded_rect, Label, FILL, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, Landing, H, W};

const PAD: i32 = 72;
pub const HEADER: i32 = 210;
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
const TRANSFER: Rect = Rect { x: GEAR.x - 32 - 72, y: 82, w: 72, h: 72 };
/// The search field, over the header's title and buttons while searching.
const FIELD: Rect = Rect { x: PAD, y: 82, w: W - 2 * PAD, h: 72 };
const CLEAR: Rect = Rect { x: FIELD.x + FIELD.w - 96, y: FIELD.y, w: 96, h: FIELD.h };
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
    /// Covers at grid and list size. Decoding all of them before the first draw took ~5 s
    /// with 95 books, so a thread decodes the current view's after the home is up
    /// ([`Home::prefetch`]); one drawn before its turn is decoded on the spot.
    cover: OnceCell<Option<GrayImage>>,
    small: OnceCell<Option<GrayImage>>,
}

impl Entry {
    fn cover(&self) -> Option<&GrayImage> {
        self.cover.get_or_init(|| cover(self.book.cover_thumb.as_deref(), (CELL_W, COVER_H))).as_ref()
    }

    fn small(&self) -> Option<&GrayImage> {
        self.small.get_or_init(|| cover(self.book.cover_thumb.as_deref(), ROW_COVER)).as_ref()
    }

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
    /// The book's index, its menu, and the plugin actions the menu offers.
    Menu(usize, BookMenu, Vec<(Plugin, usize, String)>),
    Notebook(usize, Notebook),
    About(usize, Box<About>),
    Stats(StatsScreen),
    Prefs(Prefs),
    /// Suspend, exit, restart, power off.
    Power,
    Transfer(Transfer),
    /// Opened by a plugin action from the book menu, to show it running.
    Extensions(Box<Extensions>),
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
    /// Covers decoded in the background ([`Home::prefetch`]).
    prefetched: std::cell::RefCell<Option<std::sync::mpsc::Receiver<Prefetched>>>,
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
    /// Where the view (grid/list, sort) is saved.
    state_dir: PathBuf,
    /// A book's highlights over the library: index into `books`, the list.
    overlay: Option<Overlay>,
    /// Searching the library: the text typed (titles and authors containing it are shown).
    query: Option<String>,
    /// The keyboard, up while typing the query.
    keyboard: Option<Keyboard>,
    /// Where the books are, to scan again after a delete.
    dirs: Vec<PathBuf>,
    /// What fills the first page under the continue-reading card and its row of books.
    pub band: Band,
    /// Joined to a Wi-Fi network: a small fan after the title says so.
    pub wifi: bool,
    /// Reading totals for the band (today, last 7 days, streak).
    reading: stats::Summary,
    /// A highlight for the band: its words and its book's title (a new one every
    /// [`QUOTE_EVERY`], from the recent).
    quote: Option<(String, String)>,
    /// The quote slot `quote` was picked for.
    quote_slot: i64,
}

/// The band under the continue-reading card.
const BAND_H: i32 = 190;
/// Longest highlight the first page quotes, in characters: two lines of the band.
const QUOTE_MAX: usize = 130;
/// How long one highlight stays quoted before the next.
const QUOTE_EVERY: i64 = 15 * 60;

/// Which quote is due now: the index of the current [`QUOTE_EVERY`] slot.
fn quote_slot() -> i64 {
    crate::reader::now_secs().div_euclid(QUOTE_EVERY)
}

/// The first page's band, under the row of books.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Stats,
    Quote,
}

/// Today's reading figures and a highlight that changes daily, for the first page.
/// The quote comes from `current` (the book being read: its path and title) when it has a
/// highlight that fits, else from the recent ones of any book.
fn band_data(ctx: &Ctx, current: Option<(&Path, &str)>) -> (stats::Summary, Option<(String, String)>) {
    let today = crate::clock::local_now().0;
    let reading = stats::summary(&stats::days(&ctx.state_dir, None), today);
    let slot = quote_slot();
    // Whole sentences that fit the band's two lines (about 65 characters each): a quote cut
    // short reads badly. None of them: the band shows the reading figures instead.
    let fits = |q: &String| q.split_whitespace().count() >= 6 && q.chars().count() <= QUOTE_MAX;
    let pick = |quotes: Vec<(String, String)>| {
        (!quotes.is_empty()).then(|| quotes[slot.rem_euclid(quotes.len() as i64) as usize].clone())
    };
    let quote = ctx.store().and_then(|s| {
        let own = current.and_then(|(path, title)| {
            let marks = s.book_by_path(path).ok()??;
            let quotes = (s.annotations(marks).ok()?.into_iter())
                .filter_map(|a| Some((a.data.quote?.exact, title.to_string())))
                .filter(|(q, _)| fits(q))
                .collect();
            pick(quotes)
        });
        own.or_else(|| {
            let recent = s.recent_annotations(50).ok()?;
            let quotes = (recent.into_iter())
                .filter_map(|(a, title)| Some((a.data.quote?.exact, title.unwrap_or_default())))
                .filter(|(q, _)| fits(q))
                .collect();
            pick(quotes)
        })
    });
    (reading, quote)
}

/// A cover decoded in the background: book index, grid size (true) or list size, image.
type Prefetched = (usize, bool, Option<GrayImage>);

/// A book's file time and its grid and list covers, kept across a refresh.
type Covers = (SystemTime, OnceCell<Option<GrayImage>>, OnceCell<Option<GrayImage>>);

fn library(dirs: &[PathBuf], ctx: &Ctx) -> Library {
    let paths: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    Library::scan(&paths, &ctx.state_dir.join("library"))
}

/// Reading progress of the book at `path` and when it was saved; `None` if never opened.
fn progress(ctx: &Ctx, path: &Path) -> Option<(f32, SystemTime)> {
    saved_progress(&ctx.store()?.position(&path.to_string_lossy()).ok()??)
}

/// Progress and save time of a saved position (value, Unix seconds).
fn saved_progress((value, at): &(String, i64)) -> Option<(f32, SystemTime)> {
    let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(u64::try_from(*at).ok()?);
    Position::parse(value).map(|p| (p.progress, when))
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
        let t = std::time::Instant::now();
        let library = library(dirs, ctx);
        let scanned = t.elapsed();
        let home = Self::build(library, dirs, ctx, HashMap::new());
        eprintln!("library: scanned in {scanned:?}, home built (positions, covers) in {:?}", t.elapsed() - scanned);
        home
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
        self.wifi = old.wifi;
        self.refilter(old.filter, old.page);
        true
    }

    /// The home for `library`; covers come from `covers` (path → file time, grid and list
    /// covers) when the file is unchanged, else are decoded.
    fn build(library: Library, dirs: &[PathBuf], ctx: &Ctx, mut covers: HashMap<PathBuf, Covers>) -> Home {
        let paths: Vec<&Path> = library.books.iter().map(|b| b.path.as_path()).collect();
        let store = ctx.store();
        if let Some(store) = &store {
            ctx.adopt_orphan_positions(store, &paths);
        }
        let saved = store.and_then(|s| s.positions().ok()).unwrap_or_default();
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
                let progress = saved.get(book.path.to_string_lossy().as_ref()).and_then(saved_progress);
                let (cover, small) = match covers.remove(&book.path).filter(|c| c.0 == book.modified) {
                    Some((_, cover, small)) => (cover, small),
                    None => Default::default(),
                };
                Entry { book, progress, cover, small }
            })
            .collect();
        let current = current(&books);
        let card_book = current.as_ref().map(|&(i, _)| (books[i].book.path.as_path(), books[i].book.title.as_str()));
        let (reading, quote) = band_data(ctx, card_book);
        let band = if crate::settings::Settings::load(&ctx.state_dir).home_quotes { Band::Quote } else { Band::Stats };
        // "grid|list [title|author|recent]"
        let view = crate::load_pref(&ctx.state_dir, "home_view").unwrap_or_default();
        let mut words = view.split_whitespace();
        let grid = words.next() != Some("list");
        let sort = match words.next() {
            Some("author") => Sort::Author,
            Some("recent") => Sort::Recent,
            _ => Sort::Title,
        };
        let mut home = Home {
            books,
            prefetched: Default::default(),
            current,
            filter: Filter::All,
            grid,
            sort,
            author_rank,
            shown: Vec::new(),
            page: 0,
            state_dir: ctx.state_dir.clone(),
            overlay: None,
            query: None,
            keyboard: None,
            dirs: dirs.to_vec(),
            band,
            wifi: false,
            reading,
            quote,
            quote_slot: quote_slot(),
        };
        home.set_filter(Filter::All);
        home.prefetch();
        home
    }

    /// Decodes, on a background thread, every cover the current view will show that isn't
    /// decoded yet, in the order the pages show them; [`Home::take_prefetched`] files them.
    /// A new home (a refresh) drops the receiver, which stops the old thread.
    fn prefetch(&mut self) {
        let grid = self.grid;
        let cells = |b: &Entry| if grid { b.cover.get().is_none() } else { b.small.get().is_none() };
        let jobs: Vec<(usize, PathBuf)> = (self.shown.iter().copied().chain(0..self.books.len()))
            .filter(|&i| cells(&self.books[i]))
            .filter_map(|i| Some((i, self.books[i].book.cover_thumb.clone()?)))
            .collect();
        let mut seen = std::collections::HashSet::new();
        let jobs: Vec<_> = jobs.into_iter().filter(|(i, _)| seen.insert(*i)).collect();
        if jobs.is_empty() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let size = if grid { (CELL_W, COVER_H) } else { ROW_COVER };
        std::thread::spawn(move || {
            // SAFETY: setpriority on the calling thread (who = 0 is this thread on Linux); a
            // nicer thread leaves the CPU to page turns and taps.
            unsafe { nix::libc::setpriority(nix::libc::PRIO_PROCESS, 0, 10) };
            for (i, thumb) in jobs {
                if tx.send((i, grid, cover(Some(&thumb), size))).is_err() {
                    return;
                }
            }
        });
        *self.prefetched.get_mut() = Some(rx);
    }

    /// Files the covers decoded so far into their books.
    fn take_prefetched(&self) {
        let rx = self.prefetched.borrow();
        let Some(rx) = rx.as_ref() else { return };
        for (i, grid, img) in rx.try_iter() {
            if let Some(b) = self.books.get(i) {
                let _ = if grid { b.cover.set(img) } else { b.small.set(img) };
            }
        }
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
            HEADER + CARD_H + 24 + BAND_H + 24
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
        self.take_prefetched();
        match &self.overlay {
            None => self.render_library(c, ctx),
            Some(Overlay::Menu(_, menu, _)) => {
                self.render_library(c, ctx);
                menu.render(c, ctx);
            }
            Some(Overlay::Notebook(_, nb)) => nb.render(c, ctx),
            Some(Overlay::About(i, about)) => about.render(c, ctx, self.books[*i].cover()),
            Some(Overlay::Stats(stats)) => stats.render(c, ctx),
            Some(Overlay::Prefs(p)) => p.render(c, ctx),
            Some(Overlay::Power) => {
                self.render_library(c, ctx);
                power_menu(c, ctx);
            }
            Some(Overlay::Transfer(tr)) => tr.render(c, ctx),
            Some(Overlay::Extensions(e)) => e.render(c, ctx),
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
                    text.draw(c, raster, text.fit_tail(q, width), x, FIELD.y + 48);
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
                let title = Label { face: serif_bold, size: 66.0, ink: INK };
                title.draw(c, raster, t().library, PAD as f32, 150);
                if self.wifi {
                    wifi_mark(c, (PAD + title.width(t().library) as i32 + 44, 140));
                }
                self.view_toggle(c);
                stats_button(c);
                gear_button(c);
                transfer_button(c);
                rounded_outline(c, SEARCH, SEARCH.h / 2, 2, LINE, PAPER);
                crate::search::icon(c, Rect::new(SEARCH.x + 22, SEARCH.y + 22, 28, 28), INK, PAPER);
                power_button(c);
            }
        }

        if let (true, Some((i, big))) = (self.card(self.page), &self.current) {
            self.continue_card(c, raster, &faces, &self.books[*i], big.as_ref());
            // Under the book being read, before the library: a quote or the reading figures.
            self.render_band(c, raster, &faces, Rect::new(PAD, HEADER + CARD_H + 24, W - 2 * PAD, BAND_H));
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
        let items = self.items();
        for &(i, r) in &items {
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

    /// Sample reading figures and quote, for `--pgm` reviews on a desktop with no history.
    pub fn sample_band(&mut self) {
        self.reading = stats::Summary { today: 25 * 60, week: 3 * 3600 + 40 * 60, streak: 6, ..Default::default() };
        let quote = "Il puritano voleva essere un professionista; noi dobbiamo esserlo.";
        self.quote = Some((quote.into(), "L'Etica protestante e lo spirito del capitalismo".into()));
    }

    fn render_band(&self, c: &mut Canvas, raster: &mut Rasterizer, f: &Faces, r: Rect) {
        let s = t();
        let caption = Label { face: f.sans, size: 24.0, ink: INK_SOFT };
        match (self.band, &self.quote) {
            (Band::Quote, Some((quote, title))) => {
                caption.draw(c, raster, s.from_highlights, r.x as f32, r.y + 34);
                let text = Label { face: f.serif, size: 34.0, ink: INK };
                let lines = text.wrap(&format!("«{quote}»"), r.w as f32, 2);
                for (i, line) in lines.iter().enumerate() {
                    text.draw(c, raster, line, r.x as f32, r.y + 86 + i as i32 * 46);
                }
                let by = caption.fit(&format!("— {title}"), r.w as f32);
                caption.draw(c, raster, &by, r.x as f32, r.y + 92 + lines.len() as i32 * 46);
            }
            _ => {
                caption.draw(c, raster, s.your_reading, r.x as f32, r.y + 34);
                let sum = &self.reading;
                let items = [
                    (stats::duration(sum.today), s.today),
                    (stats::duration(sum.week), s.this_week),
                    (sum.streak.to_string(), s.streak),
                ];
                stats::figures(c, raster, (f.serif_bold, f.sans), Rect::new(r.x, r.y + 44, r.w, 130), &items);
            }
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
        crate::save_pref(&self.state_dir, "home_view", &view);
    }

    /// The sort chip at the right end of the filter row: "↕ Titolo".
    fn sort_chip(&self, label: &Label) -> (String, Rect) {
        let text = format!("↕  {}", t().sorts[SORTS.iter().position(|&s| s == self.sort).unwrap_or(0)]);
        let w = label.width(&text) as i32 + 56;
        (text, Rect::new(W - PAD - w, self.chips_top(self.page), w, CHIPS_H))
    }

    /// The book under `(x, y)`: a cover, a list row or the "continue reading" card.
    /// Applies `filter` again after the books changed, staying on `page` where it still exists.
    fn refilter(&mut self, filter: Filter, page: usize) {
        self.set_filter(filter);
        self.page = page.min(self.pages() - 1);
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
        if let Some(kb) = self.keyboard.as_mut().filter(|kb| kb.holds(at.0, at.1)) {
            return kb.long_press(at.0, at.1).map_or(Action::None, |r| Action::Typed(Typed::keys(r)));
        }
        let Some((i, anchor)) = self.book_rect_at(at) else { return Action::None };
        let b = &self.books[i];
        let highlights = notebook::load(&ctx.state_dir, &b.book.path).len();
        let finished = b.status() == Filter::Finished;
        let reflowable = b.book.format != Format::Pdf;
        let actions = book_actions(ctx);
        let menu = BookMenu::new(
            &b.book.title,
            &b.author(),
            b.progress.map(|p| p.0),
            finished,
            highlights,
            reflowable,
            anchor,
            actions.iter().map(|(_, _, label)| label.clone()).collect(),
        );
        self.overlay = Some(Overlay::Menu(i, menu, actions));
        Action::Redraw { turn: None }
    }

    /// Touches while a sheet or screen covers the library.
    fn overlay_tap(&mut self, start: (i32, i32), x: i32, ctx: &Ctx) -> Action {
        let redraw = Action::Redraw { turn: None };
        let Some(overlay) = &mut self.overlay else { return Action::None };
        match overlay {
            Overlay::Menu(i, menu, actions) => {
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
                    bookmenu::Tap::Item(Item::Plugin(k)) => {
                        // The Extensions screen then shows it running and how it ended.
                        if let Some((p, a, _)) = actions.get(k) {
                            let pos = ctx.store().and_then(|s| s.position(&b.book.path.to_string_lossy()).ok()?);
                            let pos = pos.map(|(text, _)| text).unwrap_or_default();
                            if let Err(e) = p.run(&ctx.state_dir, *a, Some((&b.book.path, &pos))) {
                                eprintln!("plugin {}: {e}", p.id);
                            }
                        }
                        self.overlay = Some(Overlay::Extensions(Box::new(Extensions::new(ctx))));
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
            Overlay::Prefs(p) => p.tap(start, ctx).unwrap_or_else(|| {
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
            Overlay::Transfer(tr) => match tr.tap(start) {
                transfer::Tap::None => Action::None,
                transfer::Tap::Redraw => redraw,
                transfer::Tap::Back => {
                    let received = tr.received();
                    self.overlay = None;
                    if received {
                        self.refresh(ctx);
                    }
                    redraw
                }
            },
            Overlay::Extensions(e) => {
                if !e.tap(start, ctx) {
                    self.overlay = None;
                }
                redraw
            }
            Overlay::About(..) | Overlay::Stats(_) => Action::None,
        }
    }

    /// The band's quote and figures for the book on the card.
    fn reload_band(&mut self, ctx: &Ctx) {
        let card = self.current.as_ref().map(|&(i, _)| &self.books[i].book);
        (self.reading, self.quote) = band_data(ctx, card.map(|b| (b.path.as_path(), b.title.as_str())));
        self.quote_slot = quote_slot();
    }

    /// The next quote is due (every [`QUOTE_EVERY`]): picks it; true when the band changed and
    /// the home needs a redraw.
    pub fn rotate_quote(&mut self, ctx: &Ctx) -> bool {
        // Not under a screen opened over the home (settings, a book's menu) or while searching.
        if self.quote_slot == quote_slot() || self.band != Band::Quote || self.overlay.is_some() || self.query.is_some()
        {
            return false;
        }
        let before = self.quote.clone();
        self.reload_band(ctx);
        self.quote != before
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
        // Highlights made and minutes read meanwhile: the band's quote and figures.
        self.reload_band(ctx);
        self.refilter(self.filter, self.page);
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
        if let Some(Err(e)) = ctx.store().map(|s| s.delete_position(&path.to_string_lossy())) {
            eprintln!("delete position: {e}");
        }
        self.refresh(ctx);
        Action::Redraw { turn: None }
    }

    /// Marks book `i` as read (progress 100%, the position kept) or unread (the saved
    /// position removed: the book starts over).
    fn mark_read(&mut self, i: usize, read: bool, ctx: &Ctx) {
        let Some(store) = ctx.store() else { return };
        let path = &self.books[i].book.path;
        let done = if read {
            let p = Position::load(&store, &ctx.state_dir, path);
            let (chapter, block, offset) = p.map_or((0, 0, 0), |p| (p.chapter, p.block, p.offset));
            let p = Position { chapter, block, progress: 1.0, offset };
            store.set_position(&path.to_string_lossy(), &p.to_text(), crate::reader::now_secs())
        } else {
            store.delete_position(&path.to_string_lossy())
        };
        if let Err(e) = done {
            return eprintln!("mark read: {e}");
        }
        self.books[i].progress = read.then(|| (1.0, SystemTime::now()));
        // The card falls back to the book read before it, not to nothing.
        if self.current.as_ref().is_some_and(|(c, _)| *c == i) {
            self.current = current(&self.books);
            self.reload_band(ctx);
        }
        self.refilter(self.filter, self.page);
    }

    /// Opens the send-books screen (from the header, or the quick panel's tile).
    pub fn open_transfer(&mut self) {
        self.overlay = Some(Overlay::Transfer(Transfer::open()));
    }

    /// Whether something on screen changes by itself (the send-books screen, a plugin action
    /// running): the app then polls often.
    pub fn transferring(&self) -> bool {
        match &self.overlay {
            Some(Overlay::Transfer(_)) => true,
            Some(Overlay::Prefs(p)) => p.actions_running(),
            Some(Overlay::Extensions(e)) => e.running(),
            _ => false,
        }
    }

    /// Takes in arriving books, looks at running actions; true if it needs a redraw.
    pub fn poll_transfer(&mut self) -> bool {
        match &mut self.overlay {
            Some(Overlay::Transfer(tr)) => tr.poll(),
            Some(Overlay::Prefs(p)) => p.poll(),
            Some(Overlay::Extensions(e)) => e.poll(),
            _ => false,
        }
    }

    /// The finger went down: a key of the search keyboard shows pressed. Returns its area.
    pub fn press(&mut self, (x, y): (i32, i32)) -> Option<Rect> {
        self.keyboard.as_mut().filter(|_| self.overlay.is_none())?.press(x, y)
    }

    /// The search keyboard, while it is up.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.keyboard.as_ref().filter(|_| self.overlay.is_none())
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
        // The accents strip may stand above the band: its taps are the keyboard's too.
        let kb = self.keyboard.as_mut().filter(|kb| kb.holds(x0, y0))?;
        let Some((key, typed)) = kb.tap(x0, y0) else { return Some(Action::None) };
        match key {
            Key::Char(c) if q.chars().count() < keyboard::MAX_QUERY => q.push(c),
            Key::Space if !q.is_empty() && !q.ends_with(' ') => q.push(' '),
            Key::Backspace => {
                q.pop();
            }
            Key::Enter => {
                self.keyboard = None;
                return Some(Action::Redraw { turn: None });
            }
            // The layer switched, a full field or a second space: just the keyboard.
            _ => return Some(Action::Typed(typed)),
        }
        self.set_filter(self.filter);
        // Results and field change above the keyboard: no flash while typing.
        Some(Action::Typed(typed.and_text(Rect::new(0, 0, W, H - keyboard::KEYBOARD_H))))
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
        } else if inside(TRANSFER) {
            self.open_transfer();
            return Action::Redraw { turn: None };
        } else if inside(GEAR) {
            self.overlay = Some(Overlay::Prefs(Prefs::new(crate::settings::Settings::load(&ctx.state_dir), ctx)));
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
        match self.book_rect_at((x0, y0)).map(|(i, _)| i) {
            Some(i) => Action::Open(self.books[i].book.path.clone(), Landing::Resume),
            None => Action::None,
        }
    }
}

/// Fills the shape `inside` (in pixels from `(cx, cy)`) over a box `w` and `h` either side,
/// ink by coverage of 4×4 samples per pixel, so its curves come out smooth in gray.
// ponytail: sampled on every home render (a few thousand pixels per icon); cache the masks if
// the header ever shows up in render timings on the device.
fn shape(c: &mut Canvas, (cx, cy): (i32, i32), w: i32, h: i32, inside: impl Fn(f32, f32) -> bool) {
    for py in -h..=h {
        for px in -w..=w {
            let hits = (0..16)
                .filter(|k| inside(px as f32 + (k % 4) as f32 / 4.0 - 0.375, py as f32 + (k / 4) as f32 / 4.0 - 0.375))
                .count() as i32;
            if hits > 0 {
                let ink = PAPER as i32 - (PAPER as i32 - INK as i32) * hits / 16;
                c.fill(Rect::new(cx + px, cy + py, 1, 1), ink as u8);
            }
        }
    }
}

/// The settings button: a cog (eight square teeth round a ring, a hole in the middle) in a
/// ring.
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
    shape(c, (cx as i32, cy as i32), span, span, inside);
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
    shape(c, (cx as i32, cy as i32), 17, 21, inside);
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

/// The send-books button: an arrow down into a tray.
fn transfer_button(c: &mut Canvas) {
    rounded_outline(c, TRANSFER, TRANSFER.h / 2, 2, LINE, PAPER);
    let (cx, cy) = (TRANSFER.x + TRANSFER.w / 2, TRANSFER.y + TRANSFER.h / 2);
    c.fill(Rect::new(cx - 3, cy - 18, 6, 20), INK);
    for n in 0..9 {
        c.fill(Rect::new(cx - 11 + n, cy - 2 + n, 22 - 2 * n, 1), INK);
    }
    c.fill(Rect::new(cx - 17, cy + 12, 34, 5), INK);
    c.fill(Rect::new(cx - 17, cy + 4, 5, 10), INK);
    c.fill(Rect::new(cx + 12, cy + 4, 5, 10), INK);
}

/// Connected to Wi-Fi: a fan of two arcs over a dot centred at `(cx, cy)`.
fn wifi_mark(c: &mut Canvas, (cx, cy): (i32, i32)) {
    shape(c, (cx, cy), 32, 32, |x, y| {
        let r = x.hypot(y);
        // Within 45° of straight up.
        let fan = y < 0.0 && x.abs() < -y;
        r < 5.5 || (fan && ((13.0..19.0).contains(&r) || (25.0..31.0).contains(&r)))
    });
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
    let drawn = draw_cover(c, raster, f, cover, book.cover(), book, PAPER);
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
    draw_cover(c, raster, f, cover, book.small(), book, PAPER);
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

    #[test]
    fn unmarking_the_book_on_the_card_shows_the_one_read_before() {
        let book = Path::new("../../dev-books/etica.epub");
        if !book.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("zarathustra-unmark-{}", std::process::id()));
        let (books, state) = (dir.join("books"), dir.join("state"));
        std::fs::create_dir_all(&books).expect("dirs");
        std::fs::create_dir_all(&state).expect("dirs");
        let mut ctx = Ctx::new();
        ctx.state_dir = state;
        let store = ctx.store().expect("db");
        for (name, age) in [("a.epub", 3600), ("b.epub", 0)] {
            std::fs::copy(book, books.join(name)).expect("copy");
            let key = books.join(name).to_string_lossy().into_owned();
            store.set_position(&key, "1 0 0.3000 0", crate::reader::now_secs() - age).expect("set");
        }
        let mut home = Home::scan(std::slice::from_ref(&books), &ctx);
        let on_card = |h: &Home| h.current.as_ref().map(|&(i, _)| h.books[i].book.path.clone());
        assert_eq!(on_card(&home), Some(books.join("b.epub")));
        let b = home.books.iter().position(|e| e.book.path == books.join("b.epub")).expect("b");
        home.mark_read(b, false, &ctx);
        assert_eq!(on_card(&home), Some(books.join("a.epub")), "the book still being read");
        std::fs::remove_dir_all(dir).ok();
    }
}
