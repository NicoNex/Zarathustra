//! Text rendering probe: typesets a passage in each given font, on the device or into a PGM file.
//!
//!   omero-textprobe [--pgm out.pgm] [--size PX] [--contrast C] FONT.ttf...

use std::time::Instant;

use anyhow::{bail, Context, Result};
use omero_device::{Canvas, Rect, Refresh, Waveform};
use omero_engine::dom::Dom;
use omero_engine::font::{draw_glyph, Face, Rasterizer};
use omero_engine::layout::hyphen::Hyphenators;
use omero_engine::layout::paragraph::{LayoutParams, Run, Typesetter};
use omero_engine::style::cascade::{compute, Preferences, Stylesheets};
use omero_engine::style::css::TextAlign;

const TEXT: &str = "Call me Ishmael. Some years ago—never mind how long precisely—having little or no money \
in my purse, and nothing particular to interest me on shore, I thought I would sail about a little and see the \
watery part of the world. It is a way I have of driving off the spleen and regulating the circulation. \
“Office” · ﬁne · fluffy · 1234567890 · àèéìòù";

const MARGIN: i32 = 90;

/// One justified paragraph through the real engine: Knuth–Plass, hyphenation, protrusion.
fn paragraph(c: &mut Canvas, r: &mut Rasterizer, face: &Face, size: f32, top: i32, ts: &mut Typesetter) -> i32 {
    let dom = Dom::parse_xml("<p>x</p>").expect("static markup");
    let prefs = Preferences {
        font_size: size,
        font_family: None,
        line_height: Some(1.5),
        justify: Some(true),
        hyphenate: true,
        page_width: (c.width - 2 * MARGIN) as f32,
    };
    let style = compute(&dom, &Stylesheets::default(), &prefs).into_iter().flatten().last().expect("p styled");
    let runs = [Run { text: TEXT, style: &style, face, lang: Some("en") }];
    let params = LayoutParams { measure: prefs.page_width, indent: size * 1.5, align: TextAlign::Justify };
    let mut y = top as f32;
    for line in ts.layout(&runs, &params) {
        let baseline = y + line.baseline;
        for g in &line.glyphs {
            let (gx, glyph) = r.glyph(face, g.gid, g.size, MARGIN as f32 + g.x);
            draw_glyph(c, glyph, gx, (baseline + g.y).round() as i32, 0);
        }
        y += line.height;
    }
    (y + size) as i32
}

fn render(c: &mut Canvas, fonts: &[Face], size: f32, contrast: f32) {
    c.fill(c.bounds(), 255);
    let mut r = Rasterizer::new(contrast);
    let mut ts = Typesetter::new(Hyphenators::new(hyph_dir()));
    let mut y = MARGIN;
    let t = Instant::now();
    for face in fonts {
        y = paragraph(c, &mut r, face, size, y, &mut ts);
    }
    eprintln!("typeset {} fonts in {:?}", fonts.len(), t.elapsed());
}

/// Hyphenation dictionaries: next to the binary on the device, the repo's assets on a dev machine.
fn hyph_dir() -> std::path::PathBuf {
    let device = std::path::Path::new("/mnt/us/omero/hyph");
    if device.exists() {
        device.into()
    } else {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hyphenation").into()
    }
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
