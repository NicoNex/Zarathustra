//! The sleep screen, shown while the device is suspended (design spec §11b): the cover of
//! the book being read instead of Amazon's ads, or the clock when no book is open.

use omero_device::{Battery, Canvas, Rect};
use omero_engine::font::{Face, Rasterizer};
use omero_engine::image;

use crate::clock::{date_text, hhmm, local_now};
use crate::ui::{Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

/// Side and top margin around the cover.
const MARGIN: i32 = 48;
/// The strip under the cover with the title line and the hint.
const FOOTER: i32 = 150;

/// Draws the whole sleep screen. `cover` is the book's cover file (JPEG/PNG/GIF); an empty
/// `title` means no book is open, and shows the time it went to sleep instead.
// ponytail: decodes the cover at suspend time (~0.2 s on the PW5, well inside powerd's
// screensaver delay). Pre-render it when the book opens if that ever shows.
pub fn render_sleep(
    c: &mut Canvas,
    ctx: &mut Ctx,
    cover: Option<&[u8]>,
    title: &str,
    author: &str,
    battery: Option<Battery>,
) {
    c.fill(Rect::new(0, 0, W, H), PAPER);
    let Ctx { fonts, raster, .. } = ctx;
    let (Some(sans), Some(bold), Some(serif)) =
        (crate::sans(fonts, false), crate::sans(fonts, true), crate::serif(fonts, false))
    else {
        return;
    };
    let hint = Label { face: sans, size: 28.0, ink: INK_SOFT };
    hint.draw_centred(c, raster, crate::i18n::t().sleeping, W as f32 / 2.0, H - 48);

    if title.is_empty() {
        return clock(c, raster, sans, bold, battery);
    }
    let area = Rect::new(MARGIN, MARGIN, W - 2 * MARGIN, H - MARGIN - FOOTER);
    let img = cover.and_then(|b| image::decode(b, area.w as u32, area.h as u32));
    let Some(mut img) = img else {
        return title_page(c, raster, serif, area, title, author);
    };
    image::quantize16(&mut img);
    let (w, h) = (img.width as i32, img.height as i32);
    let at = Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h);
    image::draw(c, &img, at.x, at.y);
    // Hairline frame so white covers don't melt into the page.
    for side in [
        Rect { h: 1, ..at },
        Rect { y: at.bottom() - 1, h: 1, ..at },
        Rect { w: 1, ..at },
        Rect { x: at.right() - 1, w: 1, ..at },
    ] {
        c.fill(side, LINE);
    }
    let line = if author.is_empty() { title.to_string() } else { format!("{title} · {author}") };
    let label = Label { face: bold, size: 30.0, ink: INK };
    let line = label.fit(&line, (W - 2 * MARGIN) as f32);
    label.draw_centred(c, raster, &line, W as f32 / 2.0, H - 100);
}

/// No usable cover: the title typeset large in the book font, the author under it.
fn title_page(c: &mut Canvas, raster: &mut Rasterizer, serif: &Face, area: Rect, title: &str, author: &str) {
    let big = Label { face: serif, size: 88.0, ink: INK };
    let lines = big.wrap(title, (area.w - 2 * MARGIN) as f32, 4);
    let lead = 112;
    let mut y = area.y + area.h * 2 / 5 - (lines.len() as i32 - 1) * lead / 2;
    for l in &lines {
        big.draw_centred(c, raster, l, W as f32 / 2.0, y);
        y += lead;
    }
    if !author.is_empty() {
        let small = Label { face: serif, size: 44.0, ink: INK_SOFT };
        let a = small.fit(author, area.w as f32);
        small.draw_centred(c, raster, &a, W as f32 / 2.0, y + 24);
    }
}

/// No book: the time it went to sleep, the date and the battery.
fn clock(c: &mut Canvas, raster: &mut Rasterizer, sans: &Face, bold: &Face, battery: Option<Battery>) {
    let (days, minutes) = local_now();
    let time = hhmm(minutes);
    let center = |label: &Label, text: &str, y: i32, c: &mut Canvas, raster: &mut Rasterizer| {
        label.draw_centred(c, raster, text, W as f32 / 2.0, y);
    };
    center(&Label { face: bold, size: 200.0, ink: INK }, &time, H * 2 / 5, c, raster);
    center(
        &Label { face: sans, size: 44.0, ink: INK_SOFT },
        &date_text(crate::i18n::lang(), days),
        H * 2 / 5 + 96,
        c,
        raster,
    );
    if let Some(b) = battery {
        center(&Label { face: sans, size: 32.0, ink: INK_SOFT }, &format!("{}%", b.percent), H - 120, c, raster);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(cover: Option<&[u8]>, title: &str) -> Option<Vec<u8>> {
        let mut ctx = Ctx::new();
        ctx.sans(false)?;
        let mut px = vec![0u8; (W * H) as usize];
        let mut c = Canvas { width: W, height: H, stride: W as usize, pixels: &mut px };
        render_sleep(&mut c, &mut ctx, cover, title, "Author", Some(Battery { percent: 80, charging: false }));
        Some(px)
    }

    #[test]
    fn falls_back_to_title_page_and_clock() {
        // Skips silently without the dev fonts.
        let Some(page) = render(Some(b"not an image"), "A Rather Long Title for a Book") else { return };
        let Some(clock) = render(None, "") else { return };
        assert!(page.iter().any(|&p| p < 128), "title page has ink");
        assert!(clock.iter().any(|&p| p < 128), "clock has ink");
        assert_ne!(page, clock);
    }

    #[test]
    fn cover_is_drawn_in_16_grays() {
        let Some(path) = std::fs::read_dir(crate::first_existing(&["dev-books"])).ok().and_then(|d| {
            d.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "epub"))
        }) else {
            return;
        };
        let Ok(mut book) = omero_engine::book::Book::open(&path) else { return };
        let Some(cover) = book.cover() else { return };
        let Some(px) = render(Some(&cover), "Title") else { return };
        let mid = &px[(H / 2 * W) as usize..(H / 2 * W + W) as usize];
        assert!(mid.iter().all(|&p| p % 17 == 0 || p == LINE), "cover row quantized to 16 levels (plus the frame)");
        assert!(mid.iter().any(|&p| p != PAPER), "cover row has content");
    }
}
