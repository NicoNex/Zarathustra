//! Omero: home and reader on the device.
//!
//!   omero [BOOK] [--no-swipe] [--pgm DIR]
//!
//! With `--pgm`, renders the home screen and the first pages of BOOK into DIR instead.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use omero::home::Home;
use omero::reader::Reader;
use omero::{first_existing, Action, Ctx, H, W};
use omero_device::{Canvas, Event, Phase, Rect, Refresh, Swipe, Waveform};

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
        }
        return Ok(());
    }

    let mut dev = omero_device::open_default()?;
    let full = Rect::new(0, 0, W, H);
    screen.render(&mut dev.canvas(), &mut ctx);
    let mut marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
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
                Phase::Move => continue,
                Phase::Up => {}
            }
            let Some(start) = down.take() else { continue };
            let t0 = Instant::now();
            eprintln!("touch {start:?} → ({}, {})", t.x, t.y);
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
                swipe: turn.filter(|_| swipe && !flash).map(|f| if f { Swipe::Left } else { Swipe::Right }),
            })?;
            eprintln!("drawn in {drawn:?}");
        }
    }
}
