//! Text rendering probe: typesets a passage in each given font, on the device or into a PGM file.
//!
//!   omero-textprobe [--pgm out.pgm] [--size PX] [--contrast C] FONT.ttf...

use std::time::Instant;

use anyhow::{bail, Context, Result};
use omero_device::{Canvas, Rect, Refresh, Waveform};
use omero_engine::font::{draw_glyph, Face, Rasterizer, ShapedGlyph};

const TEXT: &str = "Call me Ishmael. Some years ago—never mind how long precisely—having little or no money \
in my purse, and nothing particular to interest me on shore, I thought I would sail about a little and see the \
watery part of the world. It is a way I have of driving off the spleen and regulating the circulation. \
“Office” · ﬁne · fluffy · 1234567890 · àèéìòù";

const MARGIN: i32 = 90;

/// Greedy, justified paragraph; enough to judge glyph quality (real line breaking comes later).
fn paragraph(c: &mut Canvas, r: &mut Rasterizer, face: &Face, size: f32, top: i32) -> i32 {
    let m = face.line_metrics(size);
    let line_h = (size * 1.5).round() as i32;
    let width = (c.width - 2 * MARGIN) as f32;
    let mut space = Vec::new();
    face.shape(" ", size, Some("en"), &mut space);
    let space_w = space.first().map_or(size / 4.0, |g| g.x_advance);

    let words: Vec<Vec<ShapedGlyph>> = TEXT
        .split(' ')
        .map(|w| {
            let mut g = Vec::new();
            face.shape(w, size, Some("en"), &mut g);
            g
        })
        .collect();
    let adv = |w: &[ShapedGlyph]| w.iter().map(|g| g.x_advance).sum::<f32>();

    let mut y = top + m.ascent.round() as i32;
    let mut i = 0;
    while i < words.len() {
        let (mut j, mut used) = (i, 0.0);
        while j < words.len() && used + adv(&words[j]) + if j > i { space_w } else { 0.0 } <= width {
            used += adv(&words[j]) + if j > i { space_w } else { 0.0 };
            j += 1;
        }
        let j = j.max(i + 1);
        let last = j == words.len();
        let gap = if last || j - i < 2 { space_w } else { space_w + (width - used) / (j - i - 1) as f32 };
        let mut x = MARGIN as f32;
        for w in &words[i..j] {
            for g in w {
                let (gx, glyph) = r.glyph(face, g.gid, size, x + g.x_offset);
                draw_glyph(c, glyph, gx, y - g.y_offset.round() as i32, 0);
                x += g.x_advance;
            }
            x += gap;
        }
        y += line_h;
        i = j;
    }
    y - m.ascent.round() as i32 + line_h / 2
}

fn render(c: &mut Canvas, fonts: &[Face], size: f32, contrast: f32) {
    c.fill(c.bounds(), 255);
    let mut r = Rasterizer::new(contrast);
    let mut y = MARGIN;
    let t = Instant::now();
    for face in fonts {
        y = paragraph(c, &mut r, face, size, y);
    }
    eprintln!("typeset {} fonts in {:?}", fonts.len(), t.elapsed());
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut pgm, mut size, mut contrast, mut fonts) = (None, 38.0, 1.3, Vec::new());
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pgm" => pgm = args.next(),
            "--size" => size = args.next().context("--size PX")?.parse()?,
            "--contrast" => contrast = args.next().context("--contrast C")?.parse()?,
            path => {
                let bytes = std::fs::read(path).with_context(|| path.to_string())?;
                fonts.push(Face::from_bytes(fonts.len() as u32, bytes).context("not a font")?);
            }
        }
    }
    if fonts.is_empty() {
        bail!("usage: omero-textprobe [--pgm out.pgm] [--size PX] [--contrast C] FONT.ttf...");
    }

    if let Some(path) = pgm {
        let (w, h) = (1236, 1648);
        let mut px = vec![0u8; (w * h) as usize];
        render(&mut Canvas { width: w, height: h, stride: w as usize, pixels: &mut px }, &fonts, size, contrast);
        let mut out = format!("P5\n{w} {h}\n255\n").into_bytes();
        out.extend_from_slice(&px);
        std::fs::write(path, out)?;
        return Ok(());
    }

    let mut dev = omero_device::open_default()?;
    let (w, h) = dev.size();
    render(&mut dev.canvas(), &fonts, size, contrast);
    let m =
        dev.refresh(&Refresh { rect: Rect::new(0, 0, w, h), waveform: Waveform::Full, flash: true, swipe: None })?;
    dev.wait_refresh(m)?;
    // Keep the page up (run.sh restarts the Amazon UI as soon as we exit) until a tap or a minute passes.
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    let mut events = Vec::new();
    while Instant::now() < deadline && events.is_empty() {
        dev.wait_events(Some(deadline - Instant::now()), &mut events)?;
    }
    Ok(())
}
