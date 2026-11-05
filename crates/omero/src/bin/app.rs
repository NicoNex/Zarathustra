//! Omero: home and reader on the device.
//!
//!   omero [BOOK] [--no-swipe] [--pgm DIR]
//!
//! With `--pgm`, renders the home screen and the first pages of BOOK into DIR instead.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use omero::highlight::{Button, LONG_PRESS, SLOP};
use omero::home::Home;
use omero::quick::{QuickPanel, QuickResult, PANEL};
use omero::reader::Reader;
use omero::sleep::render_sleep;
use omero::{first_existing, Action, Ctx, H, W};
use omero_device::{Canvas, Event, Phase, PowerEvent, Rect, Refresh, Swipe, Waveform};

/// A full flashing refresh every this many page turns clears e-ink ghosting.
const FLASH_EVERY: u32 = 8;

enum Screen {
    Home(Home),
    Reader(Box<Reader>),
}

impl Screen {
    fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        match self {
            Screen::Home(h) => h.render(c, ctx),
            Screen::Reader(r) => r.render(c, ctx),
        }
    }
}

/// The touch in progress, for long-press detection.
#[derive(Default)]
struct Press {
    /// When the finger went down, while it may still become a long press.
    at: Option<Instant>,
    /// The long press happened (the lift is not a tap).
    fired: bool,
    /// It selected a word: moves extend the selection.
    selecting: bool,
}

impl Press {
    fn pending(&self) -> Option<Instant> {
        self.at.filter(|_| !self.fired)
    }

    /// Turns the press into a long press at `start`: selects the word there, if any.
    fn fire(&mut self, r: &mut Reader, start: (i32, i32), selection: &mut Option<Rect>) {
        self.fired = true;
        if let Some(rect) = r.long_press(start) {
            self.selecting = true;
            *selection = Some(selection.map_or(rect, |s| s.union(&rect)));
        }
    }
}

fn library_dirs() -> Vec<PathBuf> {
    ["/mnt/us/library", "/mnt/us/documents", "dev-books"].iter().map(|d| first_existing(&[d])).collect()
}

fn write_pgm(path: PathBuf, px: &[u8]) -> Result<()> {
    let mut out = format!("P5\n{W} {H}\n255\n").into_bytes();
    out.extend_from_slice(px);
    Ok(std::fs::write(path, out)?)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut book, mut swipe, mut pgm) = (None, true, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-swipe" => swipe = false,
            "--pgm" => pgm = Some(PathBuf::from(args.next().context("--pgm DIR")?)),
            path => book = Some(PathBuf::from(path)),
        }
    }
    let t = Instant::now();
    let mut ctx = Ctx::new();
    let home = |ctx: &Ctx| Home::scan(&library_dirs(), ctx);
    let mut screen = Screen::Home(home(&ctx));
    eprintln!("home ready in {:?}", t.elapsed());
    if let Some(path) = &book {
        screen = Screen::Reader(Box::new(Reader::open(path, &mut ctx)?));
    }

    if let Some(dir) = pgm {
        std::fs::create_dir_all(&dir)?;
        let mut px = vec![0u8; (W * H) as usize];
        let home = home(&ctx);
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("home.pgm"), &px)?;
        if let Screen::Reader(r) = &mut screen {
            for n in 0..3 {
                r.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
                write_pgm(dir.join(format!("page{n}.pgm")), &px)?;
                r.turn(true, &mut ctx)?;
            }
            // The highlight flow on the page shown: select, save, tap it, delete it again.
            let mut snap = |r: &mut Reader, ctx: &mut Ctx, name: &str| {
                r.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, ctx);
                write_pgm(dir.join(name), &px)
            };
            let centre = |b: Rect| (b.x + b.w / 2, b.y + b.h / 2);
            let (x, y) = (W / 3, H / 2);
            r.long_press((x, y));
            r.drag((x + 350, y + 120));
            r.release();
            snap(r, &mut ctx, "select.pgm")?;
            if let Some(b) = r.popup_button(Button::Highlight) {
                r.tap(centre(b), centre(b).0, &mut ctx)?;
            }
            snap(r, &mut ctx, "highlight.pgm")?;
            r.tap((x, y), x, &mut ctx)?;
            snap(r, &mut ctx, "delete.pgm")?;
            if let Some(b) = r.popup_button(Button::Delete) {
                r.tap(centre(b), centre(b).0, &mut ctx)?;
            }
            r.menu = true;
            r.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_pgm(dir.join("menu.pgm"), &px)?;
        }
        return Ok(());
    }

    let mut dev = omero_device::open_default()?;
    let mut quick = QuickPanel::new(&ctx.state_dir, dev.as_mut());
    let full = Rect::new(0, 0, W, H);
    screen.render(&mut dev.canvas(), &mut ctx);
    let mut marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
    let mut turns = 0u32;
    let mut down = None;
    let mut press = Press::default();
    let mut events = Vec::new();
    loop {
        events.clear();
        // Wake every few minutes so automatic warmth follows the clock, or when a finger held
        // still on a page becomes a long press.
        let timeout = match (&screen, press.pending()) {
            (Screen::Reader(_), Some(at)) if !quick.is_open() => LONG_PRESS.saturating_sub(at.elapsed()),
            _ => Duration::from_secs(300),
        };
        dev.wait_events(Some(timeout), &mut events)?;
        while let Some(ev) = dev.power_event() {
            match ev {
                PowerEvent::Sleep => {
                    quick.close();
                    let battery = dev.battery();
                    match &mut screen {
                        Screen::Reader(r) => r.render_sleep(&mut dev.canvas(), &mut ctx, battery),
                        Screen::Home(_) => render_sleep(&mut dev.canvas(), &mut ctx, None, "", "", battery),
                    }
                }
                PowerEvent::Wake => screen.render(&mut dev.canvas(), &mut ctx),
                PowerEvent::Suspend { .. } | PowerEvent::Resumed => continue,
            }
            dev.wait_refresh(marker)?;
            marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
            down = None;
            press = Press::default();
        }
        // Panel changes from the whole batch, drawn once: a drag queues many moves per frame.
        let mut panel: Option<(Rect, Waveform)> = quick.tick(dev.as_mut()).map(|r| (r, Waveform::Text));
        let mut closed = false;
        // Selection changes from the whole batch, refreshed once like the panel.
        let mut selection: Option<Rect> = None;
        for e in &events {
            let t = match e {
                Event::Touch(t) => t,
                Event::Quit => return Ok(()),
            };
            if quick.is_open() {
                match quick.touch(t, dev.as_mut()) {
                    QuickResult::None => {}
                    QuickResult::Redraw { rect, waveform } => {
                        panel = Some(match panel {
                            // Any settled (non-drag) change in the batch needs the gray waveform.
                            Some((r, w)) => (r.union(&rect), if w == Waveform::Fast { waveform } else { w }),
                            None => (rect, waveform),
                        });
                    }
                    QuickResult::Close => closed = true,
                    QuickResult::Sleep => {
                        closed = true;
                        dev.sleep()?;
                    }
                }
                down = None;
                continue;
            }
            match t.phase {
                Phase::Down => {
                    down = Some((t.x, t.y));
                    press = Press { at: Some(Instant::now()), ..Press::default() };
                    continue;
                }
                Phase::Move => {
                    let (Screen::Reader(r), Some(start)) = (&mut screen, down) else { continue };
                    if press.selecting {
                        if let Some(rect) = r.drag((t.x, t.y)) {
                            selection = Some(selection.map_or(rect, |s| s.union(&rect)));
                        }
                    } else if (t.x - start.0).abs() > SLOP || (t.y - start.1).abs() > SLOP {
                        press.at = None; // moving: a swipe or a tap, not a long press
                    } else if press.pending().is_some_and(|at| at.elapsed() >= LONG_PRESS) {
                        press.fire(r, start, &mut selection);
                    }
                    continue;
                }
                Phase::Up => {}
            }
            let Some(start) = down.take() else { continue };
            if std::mem::take(&mut press).fired {
                // The end of a long press: show the actions for the selection, if any.
                if let Screen::Reader(r) = &mut screen {
                    if let Some(rect) = r.release() {
                        // Drag frames of this batch are not drawn yet: cover them too.
                        let rect = selection.map_or(rect, |s| s.union(&rect));
                        r.render(&mut dev.canvas(), &mut ctx);
                        dev.wait_refresh(marker)?;
                        marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                    }
                }
                selection = None;
                continue;
            }
            let t0 = Instant::now();
            // Swipe down from the top edge: the quick-settings panel, on every screen.
            if QuickPanel::opens(start, (t.x, t.y)) {
                let rect = quick.open(dev.as_ref());
                quick.render(&mut dev.canvas(), &mut ctx);
                dev.wait_refresh(marker)?;
                marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                continue;
            }
            let action = match &mut screen {
                Screen::Home(h) => h.tap(start, (t.x, t.y), &ctx),
                Screen::Reader(r) => r.tap(start, t.x, &mut ctx)?,
            };
            // Screen changes flash (clean slate); page turns animate and flash now and then.
            let (turn, flash) = match action {
                Action::None => continue,
                Action::Quit => return Ok(()),
                Action::Region(rect) => {
                    screen.render(&mut dev.canvas(), &mut ctx);
                    dev.wait_refresh(marker)?;
                    marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                    continue;
                }
                Action::Redraw { turn } => {
                    turns += 1;
                    (turn, turns.is_multiple_of(FLASH_EVERY))
                }
                Action::Open(path) => match Reader::open(&path, &mut ctx) {
                    Ok(r) => {
                        screen = Screen::Reader(Box::new(r));
                        (None, true)
                    }
                    Err(e) => {
                        eprintln!("{e:#}");
                        continue;
                    }
                },
                Action::Home => {
                    screen = Screen::Home(home(&ctx));
                    (None, true)
                }
            };
            screen.render(&mut dev.canvas(), &mut ctx);
            let drawn = t0.elapsed();
            dev.wait_refresh(marker)?;
            marker = dev.refresh(&Refresh {
                rect: full,
                waveform: if flash { Waveform::Full } else { Waveform::Text },
                flash,
                swipe: turn.filter(|_| swipe && quick.settings().swipe_animation && !flash).map(|f| {
                    if f {
                        Swipe::Left
                    } else {
                        Swipe::Right
                    }
                }),
            })?;
            eprintln!("drawn in {drawn:?}");
        }
        // A finger held still past the long-press delay (the timeout above woke us).
        if let (Screen::Reader(r), Some(start)) = (&mut screen, down) {
            if !quick.is_open() && press.pending().is_some_and(|at| at.elapsed() >= LONG_PRESS) {
                press.fire(r, start, &mut selection);
            }
        }
        if let Some(rect) = selection.filter(|r| !r.is_empty()) {
            screen.render(&mut dev.canvas(), &mut ctx);
            // Black-and-white while the finger moves; the gray settles on release.
            dev.wait_refresh(marker)?;
            marker = dev.refresh(&Refresh { rect, waveform: Waveform::Fast, flash: false, swipe: None })?;
        }
        if closed {
            screen.render(&mut dev.canvas(), &mut ctx);
            dev.wait_refresh(marker)?;
            marker = dev.refresh(&Refresh { rect: PANEL, waveform: Waveform::Text, flash: false, swipe: None })?;
        } else if let Some((rect, waveform)) = panel.filter(|_| quick.is_open()) {
            quick.render(&mut dev.canvas(), &mut ctx);
            // Wait even between drag frames: overlapping DU updates collide on the MTK panel
            // (offset gray blocks). Batching already keeps the drag from lagging behind.
            dev.wait_refresh(marker)?;
            marker = dev.refresh(&Refresh { rect, waveform, flash: false, swipe: None })?;
        }
    }
}
