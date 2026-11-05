//! Omero: home and reader on the device.
//!
//!   omero [BOOK] [--no-swipe] [--pgm DIR]
//!
//! With `--pgm`, renders the home screen and the first pages of BOOK into DIR instead.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
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
    let mut events = Vec::new();
    loop {
        events.clear();
        // Wake every few minutes so automatic warmth follows the clock.
        dev.wait_events(Some(Duration::from_secs(300)), &mut events)?;
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
        }
        // Panel changes from the whole batch, drawn once: a drag queues many moves per frame.
        let mut panel: Option<(Rect, Waveform)> = quick.tick(dev.as_mut()).map(|r| (r, Waveform::Text));
        let mut closed = false;
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
                    continue;
                }
                Phase::Move => continue,
                Phase::Up => {}
            }
            let Some(start) = down.take() else { continue };
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
