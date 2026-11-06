//! Footnotes in the reading view: a tap on a note reference opens the note in a card over the
//! page, laid out by the engine in the reading font. Long notes page inside the card (tap or
//! swipe in it, dots show where you are); a tap outside or on the cross closes it, and the
//! page below never moved.

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::book::Chapter;
use zarathustra_engine::dom::{Dom, NodeId};
use zarathustra_engine::layout::flow::Block;
use zarathustra_engine::layout::page::Page;
use zarathustra_engine::note;

use crate::highlight::glyph_pos;
use crate::ui::{draw_lines, rounded_outline, rounded_rect, Label, INK, INK_SOFT, PAPER};
use crate::{Ctx, H, W};

/// How far (pixels) a tap may land from a reference's glyphs: superscript digits are smaller
/// than a fingertip.
const LINK_SLOP: f32 = 44.0;
/// A reference wins over ordinary text this much closer to the tap.
const LINK_BIAS: f32 = 16.0;

const CARD_X: i32 = 44;
const CARD_W: i32 = W - 2 * CARD_X;
const PAD: i32 = 60;
const HEADER_H: i32 = 104;
const FOOTER_H: i32 = 84;
const RADIUS: i32 = 28;
const BORDER: i32 = 3;
/// A hard offset shadow under the card: crisp on e-ink, where soft gray blurs ghost.
const SHADOW: i32 = 8;
/// Width and height of the note's text area.
pub const TEXT_W: f32 = (CARD_W - 2 * PAD) as f32;
pub const BODY_H: f32 = 620.0;
/// The card keeps clear of the reader's header and footer.
const TOP: i32 = 120;
const BOTTOM: i32 = H - 120;
/// Horizontal travel that makes a touch a swipe.
const SWIPE: i32 = 100;
/// Up to this many pages show as dots, beyond as "3 / 12".
const MAX_DOTS: usize = 8;

/// The link `<a>` under a tap at `(x, y)` (text-area coordinates) on `page`: the reference
/// whose glyphs are within reach, unless ordinary text is clearly nearer.
pub fn link_at(page: &Page, blocks: &[Block], dom: &Dom, x: f32, y: f32) -> Option<NodeId> {
    let dist = |v: f32, lo: f32, hi: f32| if v < lo { lo - v } else { (v - hi).max(0.0) };
    let (mut link, mut text) = (None::<(f32, NodeId)>, f32::MAX);
    for line in &page.lines {
        for g in line.glyphs.iter().filter(|g| !g.inserted) {
            let base = line.baseline + g.y;
            let d =
                dist(x, line.x + g.x, line.x + g.x + g.advance).max(dist(y, base - 0.8 * g.size, base + 0.2 * g.size));
            if d > LINK_SLOP {
                continue;
            }
            let pos = glyph_pos(blocks, line, g.run, g.offset);
            let block = blocks.get(pos.block as usize);
            let node = block.and_then(|b| b.source_at(pos.offset as usize)).map(|(n, _)| n);
            match node.and_then(|n| note::link_at(dom, n)) {
                Some(a) if link.is_none_or(|(best, _)| d < best) => link = Some((d, a)),
                Some(_) => {}
                None => text = text.min(d),
            }
        }
    }
    link.filter(|&(d, _)| d <= text + LINK_BIAS).map(|(_, a)| a)
}

/// Height of a page's ink: the last line's descent or the bottom of the lowest image.
fn content_height(page: &Page) -> f32 {
    let lines = page.lines.iter().map(|l| l.baseline + 0.3 * l.glyphs.iter().map(|g| g.size).fold(0.0, f32::max));
    let images = page.images.iter().map(|i| i.y + i.height);
    lines.chain(images).fold(0.0, f32::max)
}

pub enum Tap {
    None,
    Redraw,
    Close,
}

/// An open note.
pub struct Popup {
    note: Chapter,
    /// The reference's marker ("12", "*"), shown in the title; empty for a worded link.
    marker: String,
    pub page: usize,
    /// The card, without its shadow.
    card: Rect,
}

impl Popup {
    /// A card for `note`, referenced by a link reading `link_text` tapped at height `ref_y`:
    /// below the reference when it is in the top half of the screen, above it otherwise.
    pub fn new(note: Chapter, link_text: &str, ref_y: i32) -> Popup {
        let paged = note.pages.len() > 1;
        let body = if paged { BODY_H } else { note.pages.first().map_or(0.0, content_height) };
        let h = HEADER_H + body.ceil() as i32 + if paged { FOOTER_H } else { PAD * 3 / 4 };
        let y = if ref_y < H / 2 { BOTTOM - h } else { TOP };
        let marker = link_text.trim().trim_matches(|c| matches!(c, '[' | ']' | '(' | ')')).trim_end_matches('.');
        let marker = if note::is_marker(marker) { marker.to_string() } else { String::new() };
        Popup { note, marker, page: 0, card: Rect::new(CARD_X, y, CARD_W, h) }
    }

    pub fn pages(&self) -> usize {
        self.note.pages.len()
    }

    /// What the card covers on screen, shadow included: the area to refresh.
    pub fn bounds(&self) -> Rect {
        let c = self.card;
        Rect::new(c.x, c.y, c.w + SHADOW, c.h + SHADOW)
    }

    fn close_button(&self) -> Rect {
        Rect::new(self.card.right() - HEADER_H - 8, self.card.y, HEADER_H + 8, HEADER_H)
    }

    /// A completed touch from `(x0, y0)` to `x`.
    pub fn tap(&mut self, (x0, y0): (i32, i32), x: i32) -> Tap {
        if !self.card.contains(x0, y0) || self.close_button().contains(x0, y0) {
            return Tap::Close;
        }
        let forward = match x - x0 {
            dx if dx < -SWIPE => true,
            dx if dx > SWIPE => false,
            _ => x0 >= self.card.x + self.card.w / 3,
        };
        let page = if forward { (self.page + 1).min(self.pages() - 1) } else { self.page.saturating_sub(1) };
        if page == self.page {
            return Tap::None;
        }
        self.page = page;
        Tap::Redraw
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let r = self.card;
        rounded_rect(c, Rect::new(r.x + SHADOW, r.y + SHADOW, r.w, r.h), RADIUS, INK);
        rounded_outline(c, r, RADIUS, BORDER, INK, PAPER);

        let Ctx { fonts, raster, .. } = ctx;
        if let Some(face) = crate::sans(fonts, true) {
            let label = Label { face, size: 24.0, ink: INK };
            let title = crate::i18n::t().note.to_uppercase();
            let title = if self.marker.is_empty() { title } else { format!("{title}  {}", self.marker) };
            label.draw(c, raster, &title, (r.x + PAD) as f32, r.y + 62);
        }
        // An editorial rule under the title, short and solid (thin gray lines ghost).
        c.fill(Rect::new(r.x + PAD, r.y + 80, 40, BORDER), INK);
        cross(c, self.close_button(), 26, 3.0);

        if let Some(page) = self.note.pages.get(self.page) {
            draw_lines(c, fonts, raster, page, ((r.x + PAD) as f32, (r.y + HEADER_H) as f32), 0);
        }
        let n = self.pages();
        if n < 2 {
            return;
        }
        let cy = r.bottom() - FOOTER_H / 2;
        if n <= MAX_DOTS {
            let (d, gap) = (14, 34);
            let x0 = r.x + r.w / 2 - ((n as i32 - 1) * gap + d) / 2;
            for i in 0..n {
                let dot = Rect::new(x0 + i as i32 * gap, cy - d / 2, d, d);
                if i == self.page {
                    rounded_rect(c, dot, d / 2, INK);
                } else {
                    rounded_outline(c, dot, d / 2, 2, INK, PAPER);
                }
            }
        } else if let Some(face) = crate::sans(fonts, false) {
            let label = Label { face, size: 24.0, ink: INK_SOFT };
            let text = format!("{} / {n}", self.page + 1);
            label.draw_centred(c, raster, &text, (r.x + r.w / 2) as f32, cy + 9);
        }
    }
}

/// An anti-aliased "×" `size` pixels across, centred in `r`, strokes `width` thick.
fn cross(c: &mut Canvas, r: Rect, size: i32, width: f32) {
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let half = size as f32 / 2.0;
    for y in (cy - half - 2.0) as i32..=(cy + half + 2.0) as i32 {
        for x in (cx - half - 2.0) as i32..=(cx + half + 2.0) as i32 {
            if x < 0 || y < 0 || x >= c.width || y >= c.height {
                continue;
            }
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            // Distance to either diagonal, and how far past the stroke ends.
            let d = (dx - dy).abs().min((dx + dy).abs()) / std::f32::consts::SQRT_2;
            let along = dx.abs().max(dy.abs()) - half;
            let a = (width / 2.0 + 0.5 - d).clamp(0.0, 1.0) * (0.5 - along).clamp(0.0, 1.0);
            let p = &mut c.pixels[y as usize * c.stride + x as usize];
            *p = (*p as f32 * (1.0 - a)) as u8;
        }
    }
}
