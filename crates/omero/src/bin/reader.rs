//! The reading view: opens an EPUB and turns pages with taps or swipes.
//!
//!   omero-reader BOOK.epub [--no-swipe] [--pgm DIR [--chapter N] [--pages N]]
//!
//! Tap the right two thirds (or swipe left) for the next page, the left third (or swipe right)
//! for the previous one, the top band to quit. The position is saved per book.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use omero_device::{Canvas, Event, Phase, Rect, Refresh, Swipe, Waveform};
use omero_engine::book::{Book, Chapter};
use omero_engine::font::{draw_glyph, Rasterizer};
use omero_engine::fonts::FontLibrary;
use omero_engine::layout::hyphen::Hyphenators;
use omero_engine::layout::page::Page;
use omero_engine::layout::paragraph::Typesetter;
use omero_engine::style::cascade::Preferences;

const W: i32 = 1236;
const H: i32 = 1648;
const MARGIN_X: i32 = 96;
const MARGIN_TOP: i32 = 110;
const MARGIN_BOTTOM: i32 = 130;
/// Taps above this quit the reader (until the reader menu exists).
const TOP_BAND: i32 = 140;
/// A full flashing refresh every this many page turns clears e-ink ghosting.
const FLASH_EVERY: u32 = 8;

/// Device paths first, then the repo's dev assets on a desktop.
fn first_existing(candidates: &[&str]) -> PathBuf {
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    (candidates.iter())
        .map(|c| if c.starts_with('/') { PathBuf::from(c) } else { Path::new(repo).join(c) })
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(candidates[0]))
}

struct Reader {
    book: Book,
    fonts: FontLibrary,
    ts: Typesetter,
    raster: Rasterizer,
    prefs: Preferences,
    chapter: Chapter,
    chapter_index: usize,
    page: usize,
    state_file: PathBuf,
}

impl Reader {
    fn open(path: &Path) -> Result<Reader> {
        let book = Book::open(path).with_context(|| path.display().to_string())?;
        if book.chapters.is_empty() {
            bail!("no chapters");
        }
        let mut fonts = FontLibrary::new("Bookerly");
        for dir in ["/mnt/us/fonts", "/usr/java/lib/fonts"] {
            fonts.add_dir(Path::new(dir));
        }
        fonts.add_dir(&first_existing(&["dev-fonts"]));
        let ts = Typesetter::new(Hyphenators::new(first_existing(&["/mnt/us/omero/hyph", "assets/hyphenation"])));
        let prefs = Preferences {
            font_size: 40.0,
            font_family: None,
            line_height: Some(1.45),
            justify: Some(true),
            hyphenate: true,
            page_width: (W - 2 * MARGIN_X) as f32,
        };
        let state_dir = first_existing(&["/mnt/us/omero/state", "target/omero-state"]);
        std::fs::create_dir_all(&state_dir).ok();
        let stem = path.file_stem().map_or_else(|| "book".into(), |s| s.to_string_lossy().into_owned());
        let state_file = state_dir.join(format!("{stem}.pos"));
        let (chapter_index, block) = std::fs::read_to_string(&state_file)
            .ok()
            .and_then(|s| {
                let mut it = s.split_whitespace().map(|n| n.parse::<usize>().ok());
                Some((it.next()??, it.next()??))
            })
            .unwrap_or((0, 0));
        let mut r = Reader {
            book,
            fonts,
            ts,
            raster: Rasterizer::new(1.3),
            prefs,
            chapter: Chapter { blocks: Vec::new(), pages: Vec::new() },
            chapter_index,
            page: 0,
            state_file,
        };
        r.chapter_index = r.chapter_index.min(r.book.chapters.len() - 1);
        r.load(r.chapter_index)?;
        // The saved block's page: the last page starting at or before it.
        r.page = (r.chapter.pages.iter())
            .rposition(|p| p.lines.first().is_some_and(|l| l.block as usize <= block))
            .unwrap_or(0);
        if r.chapter.pages.is_empty() {
            r.turn(true)?; // cover-only or empty chapter: start at the first page with text
        }
        Ok(r)
    }

    fn load(&mut self, i: usize) -> Result<()> {
        let t = Instant::now();
        let height = (H - MARGIN_TOP - MARGIN_BOTTOM) as f32;
        self.chapter = self.book.chapter(i, &self.prefs, &self.fonts, &mut self.ts, height)?;
        self.chapter_index = i;
        eprintln!("chapter {i}: {} pages in {:?}", self.chapter.pages.len(), t.elapsed());
        Ok(())
    }

    /// Moves one page forward or back across chapters; false at either end of the book.
    fn turn(&mut self, forward: bool) -> Result<bool> {
        if forward && self.page + 1 < self.chapter.pages.len() {
            self.page += 1;
        } else if !forward && self.page > 0 {
            self.page -= 1;
        } else {
            // Next non-empty chapter in that direction.
            let mut i = self.chapter_index;
            loop {
                i = match (forward, i) {
                    (true, i) if i + 1 < self.book.chapters.len() => i + 1,
                    (false, i) if i > 0 => i - 1,
                    _ => return Ok(false),
                };
                self.load(i)?;
                if !self.chapter.pages.is_empty() {
                    break;
                }
            }
            self.page = if forward { 0 } else { self.chapter.pages.len() - 1 };
        }
        self.save();
        Ok(true)
    }

    fn save(&self) {
        let block = self.current().and_then(|p| p.lines.first()).map_or(0, |l| l.block);
        std::fs::write(&self.state_file, format!("{} {block}\n", self.chapter_index)).ok();
    }

    fn current(&self) -> Option<&Page> {
        self.chapter.pages.get(self.page)
    }

    fn render(&mut self, c: &mut Canvas) {
        c.fill(c.bounds(), 255);
        if let Some(page) = self.chapter.pages.get(self.page) {
            for line in &page.lines {
                for g in &line.glyphs {
                    let Some(face) = self.fonts.face(g.face) else { continue };
                    let x = MARGIN_X as f32 + line.x + g.x;
                    let (gx, glyph) = self.raster.glyph(face, g.gid, g.size, x);
                    let y = (MARGIN_TOP as f32 + line.baseline + g.y).round() as i32;
                    draw_glyph(c, glyph, gx, y, 0);
                }
            }
        }
        self.footer(c);
    }

    /// Page count in the chapter and a thin progress bar through the book.
    fn footer(&mut self, c: &mut Canvas) {
        let chapters = self.book.chapters.len() as f32;
        let pages = self.chapter.pages.len().max(1) as f32;
        let progress = (self.chapter_index as f32 + (self.page as f32 + 1.0) / pages) / chapters;
        let y = H - MARGIN_BOTTOM / 2;
        let width = W - 2 * MARGIN_X;
        c.fill(Rect::new(MARGIN_X, y, width, 2), 200);
        c.fill(Rect::new(MARGIN_X, y - 1, (width as f32 * progress) as i32, 4), 60);

        let label = format!("{} / {}", self.page + 1, self.chapter.pages.len());
        let Some(face) = self.fonts.pick(&["serif".into()], 400, false) else { return };
        let size = 26.0;
        let mut shaped = Vec::new();
        face.shape(&label, size, None, &mut shaped);
        let advance: f32 = shaped.iter().map(|g| g.x_advance).sum();
        let mut x = (W - MARGIN_X) as f32 - advance;
        for g in &shaped {
            let (gx, glyph) = self.raster.glyph(face, g.gid, size, x + g.x_offset);
            draw_glyph(c, glyph, gx, y + 44, 90);
            x += g.x_advance;
        }
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut book, mut swipe, mut pgm, mut chapter, mut pages) = (None, true, None, 0, 3);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-swipe" => swipe = false,
            "--pgm" => pgm = args.next().map(PathBuf::from),
            "--chapter" => chapter = args.next().context("--chapter N")?.parse()?,
            "--pages" => pages = args.next().context("--pages N")?.parse()?,
            path => book = Some(PathBuf::from(path)),
        }
    }
    let Some(path) = book else { bail!("usage: omero-reader BOOK.epub [--no-swipe] [--pgm DIR]") };
    let t = Instant::now();
    let mut reader = Reader::open(&path)?;
    eprintln!("opened in {:?}", t.elapsed());

    if let Some(dir) = pgm {
        std::fs::create_dir_all(&dir)?;
        reader.load(chapter)?;
        reader.page = 0;
        let mut px = vec![0u8; (W * H) as usize];
        for n in 0..pages.min(reader.chapter.pages.len()) {
            reader.page = n;
            reader.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px });
            let mut out = format!("P5\n{W} {H}\n255\n").into_bytes();
            out.extend_from_slice(&px);
            std::fs::write(dir.join(format!("page{n}.pgm")), out)?;
        }
        return Ok(());
    }

    let mut dev = omero_device::open_default()?;
    let screen = Rect::new(0, 0, W, H);
    reader.render(&mut dev.canvas());
    let mut marker = dev.refresh(&Refresh { rect: screen, waveform: Waveform::Full, flash: true, swipe: None })?;
    let mut turns = 0u32;
    let mut down = None;
    let mut events = Vec::new();
    loop {
        events.clear();
        dev.wait_events(None, &mut events)?;
        for e in &events {
            let t = match e {
                Event::Touch(t) => t,
                Event::Quit => return Ok(()),
            };
            match t.phase {
                Phase::Down => down = Some((t.x, t.y)),
                Phase::Move => {}
                Phase::Up => {
                    let Some((x0, y0)) = down.take() else { continue };
                    let dx = t.x - x0;
                    let forward = match dx {
                        dx if dx < -100 => true,
                        dx if dx > 100 => false,
                        _ if y0 < TOP_BAND => {
                            reader.save();
                            return Ok(());
                        }
                        _ => x0 >= W / 3,
                    };
                    let t0 = Instant::now();
                    if !reader.turn(forward)? {
                        continue;
                    }
                    reader.render(&mut dev.canvas());
                    let drawn = t0.elapsed();
                    turns += 1;
                    let flash = turns.is_multiple_of(FLASH_EVERY);
                    dev.wait_refresh(marker)?;
                    marker = dev.refresh(&Refresh {
                        rect: screen,
                        waveform: if flash { Waveform::Full } else { Waveform::Text },
                        flash,
                        swipe: (swipe && !flash).then_some(if forward { Swipe::Left } else { Swipe::Right }),
                    })?;
                    eprintln!("turn: drawn in {drawn:?}");
                }
            }
        }
    }
}
