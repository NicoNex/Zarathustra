//! Throwaway probe for the power button and sleep: shows a page, then follows powerd's
//! sleep/wake cycle with the sleep screen, logging every step with timestamps.
//!
//!   zarathustra-powerprobe [--auto] [BOOK]     on the device, via `run.sh zarathustra-powerprobe`
//!   zarathustra-powerprobe --pgm DIR [BOOK]    renders the sleep screens to PGM files instead
//!
//! `--auto` runs one cycle unattended: it presses the power button through powerd, sets an
//! RTC alarm when powerd is about to suspend, and presses it again on resume. The probe
//! always exits after one wake or two minutes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use zarathustra::reader::Reader;
use zarathustra::sleep::render_sleep;
use zarathustra::{first_existing, Ctx, H, W};
use zarathustra_device::{Canvas, PowerEvent, Rect, Refresh, Waveform};

const DEADLINE: Duration = Duration::from_secs(120);
/// RTC alarm set on `readyToSuspend` in `--auto` mode.
const RTC_WAKE_SECS: u32 = 15;

fn lipc(args: &[&str]) -> String {
    match Command::new(args[0]).args(&args[1..]).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string() + &String::from_utf8_lossy(&o.stderr),
        Err(e) => e.to_string(),
    }
}

fn powerd_state() -> String {
    lipc(&["lipc-get-prop", "com.lab126.powerd", "state"])
}

fn first_book() -> Option<PathBuf> {
    ["/mnt/us/library", "/mnt/us/documents", "dev-books"]
        .iter()
        .filter_map(|d| std::fs::read_dir(first_existing(&[d])).ok())
        .flat_map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|e| e == "epub"))
}

fn write_pgm(path: &Path, px: &[u8]) -> Result<()> {
    let mut out = format!("P5\n{W} {H}\n255\n").into_bytes();
    out.extend_from_slice(px);
    Ok(std::fs::write(path, out)?)
}

fn canvas(px: &mut [u8]) -> Canvas<'_> {
    Canvas { width: W, height: H, stride: W as usize, pixels: px }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut auto, mut pgm, mut book) = (false, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--auto" => auto = true,
            "--pgm" => pgm = Some(PathBuf::from(args.next().context("--pgm DIR")?)),
            path => book = Some(PathBuf::from(path)),
        }
    }
    let mut ctx = Ctx::new();
    let mut reader = match book.or_else(first_book) {
        Some(p) => Some(Reader::open(&p, &mut ctx)?),
        None => None,
    };

    if let Some(dir) = pgm {
        std::fs::create_dir_all(&dir)?;
        let mut px = vec![0u8; (W * H) as usize];
        let battery = Some(zarathustra_device::Battery { percent: 87, charging: false });
        if let Some(r) = &mut reader {
            r.render_sleep(&mut canvas(&mut px), &mut ctx, battery);
            write_pgm(&dir.join("sleep-cover.pgm"), &px)?;
            let title = r.title().unwrap_or("Untitled").to_string();
            render_sleep(&mut canvas(&mut px), &mut ctx, None, &title, "Some Author", battery);
            write_pgm(&dir.join("sleep-title.pgm"), &px)?;
        }
        render_sleep(&mut canvas(&mut px), &mut ctx, None, "", "", battery);
        return write_pgm(&dir.join("sleep-clock.pgm"), &px);
    }

    let t0 = Instant::now();
    let log = |msg: &str| eprintln!("[{:7.3}] {msg}", t0.elapsed().as_secs_f64());
    let mut dev = zarathustra_device::open_default()?;
    let full = Rect::new(0, 0, W, H);
    let flash = Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None };
    let draw_page = |dev: &mut Box<dyn zarathustra_device::Device>, ctx: &mut Ctx, reader: &Option<Reader>| {
        let mut c = dev.canvas();
        match reader {
            Some(r) => r.render(&mut c, ctx),
            None => c.fill(full, 255),
        }
    };
    log(&format!("start: state={} light={:?}", powerd_state(), dev.light()));
    draw_page(&mut dev, &mut ctx, &reader);
    let mut marker = dev.refresh(&flash)?;

    let mut pressed = false;
    let mut asleep = false;
    let mut events = Vec::new();
    while t0.elapsed() < DEADLINE {
        if auto && !pressed && t0.elapsed() > Duration::from_secs(3) {
            pressed = true;
            log(&format!("auto: power button -> {:?}", dev.sleep()));
        }
        events.clear();
        dev.wait_events(Some(Duration::from_millis(500)), &mut events)?;
        for e in &events {
            log(&format!("input while awake={}: {e:?}", !asleep));
        }
        while let Some(ev) = dev.power_event() {
            log(&format!("power {ev:?}: state={} light={:?}", powerd_state(), dev.light()));
            match ev {
                PowerEvent::Sleep => {
                    asleep = true;
                    let t = Instant::now();
                    let battery = dev.battery();
                    let mut c = dev.canvas();
                    match &mut reader {
                        Some(r) => r.render_sleep(&mut c, &mut ctx, battery),
                        None => render_sleep(&mut c, &mut ctx, None, "", "", battery),
                    }
                    let drawn = t.elapsed();
                    dev.wait_refresh(marker)?;
                    marker = dev.refresh(&flash)?;
                    dev.wait_refresh(marker)?;
                    log(&format!("sleep screen: drawn {drawn:?}, shown {:?}", t.elapsed()));
                }
                PowerEvent::Suspend { delay } => {
                    if auto {
                        let out =
                            lipc(&["lipc-set-prop", "com.lab126.powerd", "rtcWakeup", &RTC_WAKE_SECS.to_string()]);
                        log(&format!("auto: rtcWakeup {RTC_WAKE_SECS} (delay {delay}) {out}"));
                    }
                }
                PowerEvent::Resumed => {
                    if auto {
                        log(&format!("auto: power button -> {:?}", dev.sleep()));
                    }
                }
                PowerEvent::Wake => {
                    let t = Instant::now();
                    draw_page(&mut dev, &mut ctx, &reader);
                    dev.wait_refresh(marker)?;
                    marker = dev.refresh(&flash)?;
                    dev.wait_refresh(marker)?;
                    log(&format!("woke, page back in {:?}; light={:?}", t.elapsed(), dev.light()));
                    return Ok(());
                }
            }
        }
    }
    log(&format!("deadline: state={} asleep={asleep}", powerd_state()));
    // Never leave powerd in the screen saver behind us: press the button to leave it.
    if asleep || powerd_state() != "active" {
        log(&format!("leaving screen saver -> {:?}", dev.sleep()));
    }
    Ok(())
}
