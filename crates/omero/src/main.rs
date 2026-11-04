//! Hardware probe: verifies drawing, refresh modes, swipe animation, touch and light on a device.
//!
//! Layout (device pixels): top band = warmth −/+, bottom band = brightness −/+,
//! top-left corner = exit, middle = draw with your finger; a tap in the
//! middle's left/right third turns the "page" with the hardware swipe.

use std::time::Instant;

use anyhow::Result;
use omero_device::{Canvas, Device, Event, Phase, Rect, Refresh, Swipe, Waveform};

const BAND: i32 = 220;
const EXIT: i32 = 160;
const DOT: i32 = 24;
const TAP_SLOP: i32 = 30;

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
        let g = (i * 17) as u8;
        let g = if inverted { 255 - g } else { g };
        c.fill(Rect::new(ramp.x + i * step, ramp.y, step, ramp.h), g);
    }
}

fn adjust(dev: &mut dyn Device, warmth: bool, delta: i16) -> Result<()> {
    let Some(mut l) = dev.light() else { return Ok(()) };
    let step = |v: u8| (v as i16 + delta).clamp(0, 100) as u8;
    if warmth {
        l.warmth = l.warmth.map(step);
    } else {
        l.brightness = step(l.brightness);
    }
    dev.set_light(l)?;
    eprintln!("light: {l:?}");
    Ok(())
}

fn run(dev: &mut dyn Device) -> Result<()> {
    let (w, h) = {
        let c = dev.canvas();
        (c.width, c.height)
    };
    eprintln!("probe: {} {w}x{h} battery={:?} light={:?}", dev.name(), dev.battery(), dev.light());

    let mut inverted = false;
    draw_page(&mut dev.canvas(), inverted);
    let m = dev.refresh(&Refresh { rect: Rect::new(0, 0, w, h), waveform: Waveform::Full, flash: true, swipe: None })?;
    dev.wait_refresh(m)?;

    let mut events = Vec::new();
    let mut start = (0, 0);
    loop {
        events.clear();
        dev.wait_events(None, &mut events)?;
        let t0 = Instant::now();
        // Coalesce: only the latest Move matters on a slow panel.
        let last_move = events.iter().rposition(|e| matches!(e, Event::Touch(t) if t.phase == Phase::Move));
        for (i, ev) in events.iter().enumerate() {
            let t = match ev {
                Event::Quit => return Ok(()),
                Event::Touch(t) => *t,
            };
            if t.phase == Phase::Move && Some(i) != last_move {
                continue;
            }
            let in_middle = t.y >= BAND && t.y < h - BAND;
            match t.phase {
                Phase::Down => {
                    start = (t.x, t.y);
                    if t.x < EXIT && t.y < EXIT {
                        return Ok(());
                    }
                    if !in_middle {
                        adjust(dev, t.y < BAND, if t.x < w / 2 { -10 } else { 10 })?;
                        continue;
                    }
                }
                Phase::Up => {
                    let tap = (t.x - start.0).abs() < TAP_SLOP && (t.y - start.1).abs() < TAP_SLOP;
                    let side = if t.x < w / 3 { Some(Swipe::Right) } else if t.x > 2 * w / 3 { Some(Swipe::Left) } else { None };
                    if in_middle && tap && side.is_some() {
                        inverted = !inverted;
                        draw_page(&mut dev.canvas(), inverted);
                        let r = Refresh { rect: Rect::new(0, 0, w, h), waveform: Waveform::Text, flash: false, swipe: side };
                        dev.refresh(&r)?;
                        eprintln!("page turn submitted in {:?}", t0.elapsed());
                    }
                    continue;
                }
                Phase::Move if !in_middle => continue,
                Phase::Move => {}
            }
            let dot = Rect::new(t.x - DOT / 2, t.y - DOT / 2, DOT, DOT);
            dev.canvas().fill(dot, 0);
            dev.refresh(&Refresh { rect: dot, waveform: Waveform::Fast, flash: false, swipe: None })?;
            eprintln!("dot submitted in {:?}", t0.elapsed());
        }
    }
}

fn main() -> Result<()> {
    #[cfg(feature = "emulator")]
    let mut dev = omero_device::emulator::Emulator::new()?;
    #[cfg(not(feature = "emulator"))]
    let mut dev = omero_device::kindle::Kindle::open()?;
    run(&mut dev)
}
