//! Home: the book being read on top, then every book in the library, a page of rows at a time.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use omero_device::{Canvas, Rect};
use omero_engine::epub::Epub;

use crate::reader::Position;
use crate::ui::{rounded_outline, rounded_rect, Label, FILL, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, H, W};

const PAD: i32 = 72;
const CARD_TOP: i32 = 210;
const CARD_H: i32 = 330;
const ROW_H: i32 = 156;
const FOOTER: i32 = 110;
const EXIT: Rect = Rect { x: W - PAD - 150, y: 82, w: 150, h: 72 };

pub struct Entry {
    pub path: PathBuf,
    pub title: String,
    pub author: String,
    /// Reading progress 0–1 and when it was saved, if the book was opened.
    pub progress: Option<(f32, SystemTime)>,
}

pub struct Home {
    books: Vec<Entry>,
    page: usize,
}

impl Home {
    /// Scans the library folders for books.
    // ponytail: EPUB only, metadata read on every scan; the cached Library (covers, AZW3,
    // grouping and search) replaces this.
    pub fn scan(dirs: &[PathBuf], ctx: &Ctx) -> Home {
        let mut paths = Vec::new();
        for d in dirs {
            walk(d, &mut paths);
        }
        let mut books: Vec<Entry> = (paths.into_iter())
            .filter_map(|path| {
                let epub = Epub::open(std::io::BufReader::new(std::fs::File::open(&path).ok()?)).ok()?;
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                let state = ctx.state_file(&path);
                let progress = Position::load(&state)
                    .and_then(|p| Some((p.progress, std::fs::metadata(&state).ok()?.modified().ok()?)));
                Some(Entry {
                    title: epub.metadata.title.clone().unwrap_or(stem),
                    author: epub.metadata.authors.join(", "),
                    path,
                    progress,
                })
            })
            .collect();
        books.sort_by_key(|b| b.title.to_lowercase());
        Home { books, page: 0 }
    }

    /// The most recently read book.
    fn current(&self) -> Option<&Entry> {
        self.books.iter().filter(|b| b.progress.is_some()).max_by_key(|b| b.progress.map(|p| p.1))
    }

    fn list_top(&self) -> i32 {
        if self.current().is_some() {
            CARD_TOP + CARD_H + 90
        } else {
            CARD_TOP
        }
    }

    fn rows_per_page(&self) -> usize {
        ((H - FOOTER - self.list_top()) / ROW_H).max(1) as usize
    }

    fn pages(&self) -> usize {
        self.books.len().div_ceil(self.rows_per_page()).max(1)
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

        if let Some(book) = self.current() {
            let card = Rect::new(PAD, CARD_TOP, W - 2 * PAD, CARD_H);
            rounded_rect(c, card, 28, FILL);
            let cover = Rect::new(card.x + 36, card.y + 36, 172, CARD_H - 72);
            placeholder_cover(c, raster, serif_bold, cover, &book.title);
            let x = (cover.right() + 44) as f32;
            let width = (card.right() - 40) as f32 - x;
            let small = Label { face: sans, size: 28.0, ink: INK_SOFT };
            small.draw(c, raster, "CONTINUE READING", x, card.y + 70);
            let title = Label { face: serif_bold, size: 46.0, ink: INK };
            title.draw(c, raster, &title.fit(&book.title, width), x, card.y + 140);
            let author = Label { face: sans, size: 30.0, ink: INK_SOFT };
            author.draw(c, raster, &author.fit(&book.author, width), x, card.y + 190);
            let progress = book.progress.map_or(0.0, |p| p.0);
            let bar = Rect::new(x as i32, card.bottom() - 70, width as i32 - 110, 10);
            rounded_rect(c, bar, 5, LINE);
            rounded_rect(c, Rect { w: (bar.w as f32 * progress) as i32, ..bar }, 5, INK);
            small.draw(c, raster, &format!("{:.0}%", progress * 100.0), (bar.right() + 24) as f32, bar.y + 12);
        }

        let top = self.list_top();
        let section = Label { face: sans, size: 28.0, ink: INK_SOFT };
        section.draw(c, raster, &format!("ALL BOOKS · {}", self.books.len()), PAD as f32, top - 30);
        let per = self.rows_per_page();
        for (i, book) in self.books.iter().skip(self.page * per).take(per).enumerate() {
            let y = top + i as i32 * ROW_H;
            let cover = Rect::new(PAD, y + 18, 80, ROW_H - 36);
            placeholder_cover(c, raster, serif_bold, cover, &book.title);
            let x = (cover.right() + 32) as f32;
            let right = (W - PAD) as f32;
            let pct = book.progress.map(|p| format!("{:.0}%", p.0 * 100.0));
            let pct_w = pct.as_deref().map_or(0.0, |p| section.width(p) + 24.0);
            let title = Label { face: serif, size: 38.0, ink: INK };
            title.draw(c, raster, &title.fit(&book.title, right - x - pct_w), x, y + 70);
            let author = Label { face: sans, size: 28.0, ink: INK_SOFT };
            author.draw(c, raster, &author.fit(&book.author, right - x - pct_w), x, y + 114);
            if let Some(p) = pct {
                section.draw(c, raster, &p, right - section.width(&p), y + 70);
            }
            if i + 1 < per {
                c.fill(Rect::new(x as i32, y + ROW_H - 1, W - PAD - x as i32, 1), LINE);
            }
        }

        if self.pages() > 1 {
            let label = format!("‹   {} / {}   ›", self.page + 1, self.pages());
            let x = (W as f32 - section.width(&label)) / 2.0;
            section.draw(c, raster, &label, x, H - FOOTER / 2 + 10);
        }
    }

    /// Handles a completed touch from `(x0, y0)` to `(x, y)`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), (x, y): (i32, i32)) -> Action {
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
        if let Some(book) = self.current() {
            if inside(Rect::new(PAD, CARD_TOP, W - 2 * PAD, CARD_H)) {
                return Action::Open(book.path.clone());
            }
        }
        let top = self.list_top();
        if y0 >= top {
            let i = self.page * self.rows_per_page() + ((y0 - top) / ROW_H) as usize;
            if let Some(book) = self.books.get(i) {
                return Action::Open(book.path.clone());
            }
        }
        Action::None
    }
}

/// A cover stand-in: a dark rounded card with the title's initial.
fn placeholder_cover(
    c: &mut Canvas,
    raster: &mut omero_engine::font::Rasterizer,
    face: &omero_engine::font::Face,
    r: Rect,
    title: &str,
) {
    rounded_rect(c, r, 12, 70);
    let initial: String = title.chars().find(|c| c.is_alphanumeric()).into_iter().collect();
    let label = Label { face, size: r.w as f32 * 0.55, ink: PAPER };
    let x = r.x as f32 + (r.w as f32 - label.width(&initial)) / 2.0;
    label.draw(c, raster, &initial, x, r.y + r.h / 2 + (label.size * 0.35) as i32);
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.ends_with(".sdr") {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("epub")) {
            out.push(path);
        }
    }
}
