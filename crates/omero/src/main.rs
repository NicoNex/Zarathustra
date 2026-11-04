//! Hardware probe: verifies drawing, refresh modes, swipe animation, touch and light on a device.
//!
//! Layout (device pixels): top band = warmth −/+, bottom band = brightness −/+,
//! top-left corner = exit, middle = draw with your finger; a tap in the
//! middle's left/right third turns the "page" with the hardware swipe.

use std::time::{Duration, Instant};

use anyhow::Result;
use omero_device::{Canvas, Device, Event, Phase, Rect, Refresh, Swipe, Waveform};

const BAND: i32 = 220;
const EXIT: i32 = 160;
const DOT: i32 = 24;
const TAP_SLOP: i32 = 30;
/// Quit after this long without input, so nobody gets stuck in the probe.
const IDLE_EXIT: Duration = Duration::from_secs(60);

fn draw_page(c: &mut Canvas, inverted: bool) {
    let (w, h) = (c.width, c.height);
    c.fill(c.bounds(), 255);
    // Bands with −/+ glyphs made of bars.
    for y in [0, h - BAND] {
        c.fill(Rect::new(0, y, w, BAND), 230);
        let cy = y + BAND / 2;
        c.fill(Rect::new(w / 4 - 50, cy - 6, 100, 12), 0);
        c.fill(Rect::new(3 * w / 4 - 50, cy - 6, 100, 12), 0);
        c.fill(Rect::new(3 * w / 4 - 6, cy - 50, 12, 100), 0);
    }
    // Exit corner: a solid square.
    c.fill(Rect::new(0, 0, EXIT, EXIT), 0);
    // 16-step gray ramp, reversed on the alternate page.
    let ramp = Rect::new(60, h / 2 - 120, w - 120, 240);
    let step = ramp.w / 16;
    for i in 0..16 {
        let g = if inverted { 255 - i * 17 } else { i * 17 } as u8;
        c.fill(Rect::new(ramp.x + i * step, ramp.y, step, ramp.h), g);
    }
}

fn run(dev: &mut dyn Device) -> Result<()> {
    let (w, h) = dev.size();
    let screen = Rect::new(0, 0, w, h);
    eprintln!("probe: {} {w}x{h} battery={:?} light={:?}", dev.name(), dev.battery(), dev.light());

    let mut inverted = false;
    draw_page(&mut dev.canvas(), inverted);
    // The last submitted refresh; the framebuffer isn't rewritten under an in-flight full-screen update.
    let mut marker = dev.refresh(&Refresh { rect: screen, waveform: Waveform::Full, flash: true, swipe: None })?;

    let mut events = Vec::new();
    let mut start = (0, 0);
    loop {
        events.clear();
        dev.wait_events(Some(IDLE_EXIT), &mut events)?;
        if events.is_empty() {
            eprintln!("idle for {IDLE_EXIT:?}, exiting");
            return Ok(());
        }
        let t0 = Instant::now();
        // Everything drawn in this batch goes out as one refresh: the panel is slower than the touch rate.
        let mut dirty = Rect::default();
        for ev in &events {
            let t = match ev {
                Event::Quit => return Ok(()),
                Event::Touch(t) => *t,
            };
            let in_middle = t.y >= BAND && t.y < h - BAND;
            match t.phase {
                Phase::Down if t.x < EXIT && t.y < EXIT => {
                    eprintln!("exit corner");
                    return Ok(());
                }
                Phase::Down if !in_middle => {
                    if let Some(l) = dev.light() {
                        let l = l.step(t.y < BAND, if t.x < w / 2 { -10 } else { 10 });
                        dev.set_light(l)?;
                        eprintln!("light: {l:?}");
                    }
                }
                Phase::Up => {
                    let tap = (t.x - start.0).abs() < TAP_SLOP && (t.y - start.1).abs() < TAP_SLOP;
                    let side = match t.x {
                        x if x < w / 3 => Some(Swipe::Right),
                        x if x > 2 * w / 3 => Some(Swipe::Left),
                        _ => None,
                    };
                    if in_middle && tap && side.is_some() {
                        dev.wait_refresh(marker)?;
                        inverted = !inverted;
                        draw_page(&mut dev.canvas(), inverted);
                        marker = dev.refresh(&Refresh {
                            rect: screen,
                            waveform: Waveform::Text,
                            flash: false,
                            swipe: side,
                        })?;
                        dirty = Rect::default();
                        eprintln!("page turn submitted in {:?}", t0.elapsed());
                    }
                }
                Phase::Down | Phase::Move if in_middle => {
                    if t.phase == Phase::Down {
                        start = (t.x, t.y);
                    }
                    let dot = Rect::new(t.x - DOT / 2, t.y - DOT / 2, DOT, DOT);
                    dev.canvas().fill(dot, 0);
                    dirty = dirty.union(&dot);
                }
                Phase::Down | Phase::Move => {}
            }
        }
        if !dirty.is_empty() {
            dev.wait_refresh(marker)?;
            marker = dev.refresh(&Refresh { rect: dirty, waveform: Waveform::Fast, flash: false, swipe: None })?;
        }
    }
}

fn main() -> Result<()> {
    run(omero_device::open_default()?.as_mut())
}
