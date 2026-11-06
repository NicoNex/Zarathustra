//! Drawing primitives for the shell's own UI: anti-aliased rounded rectangles and single
//! lines of text, in the grayscale palette of the design spec.

use omero_device::{Canvas, Rect};
use omero_engine::font::{draw_glyph, Face, Rasterizer, ShapedGlyph};
use omero_engine::fonts::FontLibrary;
use omero_engine::layout::page::Page;

/// Palette (0 = black).
pub const INK: u8 = 0;
pub const INK_SOFT: u8 = 90;
pub const LINE: u8 = 200;
pub const FILL: u8 = 235;
pub const PAPER: u8 = 255;

/// Blends `color` over the pixel at `(x, y)` with coverage `a` (0–255).
fn blend(c: &mut Canvas, x: i32, y: i32, color: u8, a: u32) {
    if x < 0 || y < 0 || x >= c.width || y >= c.height || a == 0 {
        return;
    }
    let p = &mut c.pixels[y as usize * c.stride + x as usize];
    *p = ((*p as u32 * (255 - a) + color as u32 * a) / 255) as u8;
}

/// Coverage of pixel `(px, py)` by a rounded rectangle, 4×4 supersampled at the corners.
fn coverage(r: Rect, radius: f32, px: i32, py: i32) -> u32 {
    let (x0, y0) = (r.x as f32, r.y as f32);
    let (x1, y1) = (r.right() as f32, r.bottom() as f32);
    let mut hits = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let x = px as f32 + (sx as f32 + 0.5) / 4.0;
            let y = py as f32 + (sy as f32 + 0.5) / 4.0;
            let cx = x.clamp(x0 + radius, x1 - radius);
            let cy = y.clamp(y0 + radius, y1 - radius);
            if (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius {
                hits += 1;
            }
        }
    }
    hits * 255 / 16
}

/// Fills `r` with corners of `radius` pixels, anti-aliased.
pub fn rounded_rect(c: &mut Canvas, r: Rect, radius: i32, color: u8) {
    let radius = radius.min(r.w / 2).min(r.h / 2).max(0);
    // Straight middle bands are solid; only the four corner squares need coverage.
    c.fill(Rect::new(r.x + radius, r.y, r.w - 2 * radius, r.h), color);
    c.fill(Rect::new(r.x, r.y + radius, radius, r.h - 2 * radius), color);
    c.fill(Rect::new(r.right() - radius, r.y + radius, radius, r.h - 2 * radius), color);
    for (cx, cy) in
        [(r.x, r.y), (r.right() - radius, r.y), (r.x, r.bottom() - radius), (r.right() - radius, r.bottom() - radius)]
    {
        for y in cy..cy + radius {
            for x in cx..cx + radius {
                blend(c, x, y, color, coverage(r, radius as f32, x, y));
            }
        }
    }
}

/// Rounds the corners of whatever is drawn in `r` by blending the outside of each arc
/// with what lies behind it: `bg` per corner (top-left, top-right, bottom-left, bottom-right).
/// For images, which are drawn square.
pub fn clip_corners(c: &mut Canvas, r: Rect, radius: i32, bg: [u8; 4]) {
    let radius = radius.min(r.w / 2).min(r.h / 2).max(0);
    let corners =
        [(r.x, r.y), (r.right() - radius, r.y), (r.x, r.bottom() - radius), (r.right() - radius, r.bottom() - radius)];
    for ((cx, cy), bg) in corners.into_iter().zip(bg) {
        for y in cy..cy + radius {
            for x in cx..cx + radius {
                blend(c, x, y, bg, 255 - coverage(r, radius as f32, x, y));
            }
        }
    }
}

/// A rounded outline `width` pixels thick.
pub fn rounded_outline(c: &mut Canvas, r: Rect, radius: i32, width: i32, color: u8, inside: u8) {
    rounded_rect(c, r, radius, color);
    let inner = Rect::new(r.x + width, r.y + width, r.w - 2 * width, r.h - 2 * width);
    rounded_rect(c, inner, (radius - width).max(0), inside);
}

/// Corner radius and border width of buttons and segmented controls.
pub const RADIUS: i32 = 20;
pub const BORDER: i32 = 2;

/// A button: a black outline, or solid black when `on`. Solid black edges stay crisp on
/// e-ink under fast waveforms, where thin gray lines ghost.
pub fn button(c: &mut Canvas, r: Rect, on: bool) {
    if on {
        rounded_rect(c, r, RADIUS, INK);
    } else {
        rounded_outline(c, r, RADIUS, BORDER, INK, PAPER);
    }
}

/// A row of `n` equal segments in one outline, `on` filled black. Returns the segment rects.
pub fn segmented(c: &mut Canvas, r: Rect, n: usize, on: Option<usize>) -> Vec<Rect> {
    let segs = segments(r, n);
    rounded_outline(c, r, RADIUS, BORDER, INK, PAPER);
    for (i, s) in segs.iter().enumerate() {
        if on == Some(i) {
            // Round only the outer corners: a rounded fill squared off on its inner sides.
            rounded_rect(c, *s, RADIUS, INK);
            if i > 0 {
                c.fill(Rect::new(s.x, s.y, RADIUS, s.h), INK);
            }
            if i + 1 < n {
                c.fill(Rect::new(s.right() - RADIUS, s.y, RADIUS, s.h), INK);
            }
        }
        if i > 0 {
            c.fill(Rect::new(s.x - BORDER / 2, r.y, BORDER, r.h), INK);
        }
    }
    segs
}

/// `r` split into `n` equal columns (the last takes the rounding remainder).
pub fn segments(r: Rect, n: usize) -> Vec<Rect> {
    let n = n.max(1) as i32;
    (0..n).map(|i| Rect::new(r.x + i * r.w / n, r.y, (i + 1) * r.w / n - i * r.w / n, r.h)).collect()
}

/// Single-line text in one face and size.
pub struct Label<'a> {
    pub face: &'a Face,
    pub size: f32,
    pub ink: u8,
}

impl Label<'_> {
    fn shape(&self, text: &str) -> Vec<ShapedGlyph> {
        let mut shaped = Vec::new();
        self.face.shape(text, self.size, None, &mut shaped);
        shaped
    }

    pub fn width(&self, text: &str) -> f32 {
        advance(&self.shape(text))
    }

    /// `text` cut with an ellipsis so it fits in `max_width`.
    pub fn fit(&self, text: &str, max_width: f32) -> String {
        self.cut(text, &self.shape(text), max_width).unwrap_or_else(|| text.to_string())
    }

    /// `text` (shaped as `shaped`) cut with an ellipsis to fit `max_width`, `None` when it
    /// fits whole: one shaping, cut at the first cluster that would cross the ellipsis.
    // ponytail: ignores kerning across the cut, a pixel or two at UI sizes.
    fn cut(&self, text: &str, shaped: &[ShapedGlyph], max_width: f32) -> Option<String> {
        if advance(shaped) <= max_width {
            return None;
        }
        let budget = max_width - self.width("…");
        let mut pen = 0.0;
        let over = shaped.iter().find(|g| {
            pen += g.x_advance;
            pen > budget
        });
        let at = over.map_or(text.len(), |g| g.cluster as usize);
        Some(format!("{}…", text.get(..at).unwrap_or("").trim_end()))
    }

    /// Greedy word wrap into at most `max` lines, the last one ellipsized when text is left.
    pub fn wrap(&self, text: &str, width: f32, max: usize) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut words = text.split_whitespace().peekable();
        while let Some(w) = words.next() {
            let mut line = w.to_string();
            while let Some(next) = words.peek() {
                let candidate = format!("{line} {next}");
                if self.width(&candidate) > width {
                    break;
                }
                line = candidate;
                words.next();
            }
            if lines.len() + 1 >= max {
                let rest: Vec<&str> = words.collect();
                let full = if rest.is_empty() { line } else { format!("{line} {}", rest.join(" ")) };
                lines.push(self.fit(&full, width));
                break;
            }
            lines.push(self.fit(&line, width));
        }
        lines
    }

    /// Draws `text` centred in `r` (fitted to its width), with the baseline set for a
    /// capital height of about 0.7 em.
    pub fn centered(&self, c: &mut Canvas, raster: &mut Rasterizer, text: &str, r: Rect) {
        let mut shaped = self.shape(text);
        if let Some(cut) = self.cut(text, &shaped, (r.w - 24) as f32) {
            shaped = self.shape(&cut);
        }
        let x = r.x as f32 + (r.w as f32 - advance(&shaped)) / 2.0;
        self.draw_glyphs(c, raster, &shaped, x, self.baseline(r));
    }

    /// Draws `text` centred on `cx`, with its baseline at `y`.
    pub fn draw_centred(&self, c: &mut Canvas, raster: &mut Rasterizer, text: &str, cx: f32, y: i32) {
        let shaped = self.shape(text);
        self.draw_glyphs(c, raster, &shaped, cx - advance(&shaped) / 2.0, y);
    }

    /// Baseline that centres capitals vertically in `r`.
    pub fn baseline(&self, r: Rect) -> i32 {
        r.y + r.h / 2 + (self.size * 0.35).round() as i32
    }

    /// Draws `text` with its baseline at `y`; returns the advance.
    pub fn draw(&self, c: &mut Canvas, raster: &mut Rasterizer, text: &str, x: f32, y: i32) -> f32 {
        self.draw_glyphs(c, raster, &self.shape(text), x, y)
    }

    fn draw_glyphs(&self, c: &mut Canvas, raster: &mut Rasterizer, shaped: &[ShapedGlyph], x: f32, y: i32) -> f32 {
        let mut pen = x;
        for g in shaped {
            let (gx, glyph) = raster.glyph(self.face, g.gid, self.size, pen + g.x_offset);
            draw_glyph(c, glyph, gx, y - g.y_offset.round() as i32, self.ink);
            pen += g.x_advance;
        }
        pen - x
    }
}

/// Draws the text lines of a laid-out page with the text area's top-left corner at `origin`.
pub fn draw_lines(c: &mut Canvas, fonts: &FontLibrary, raster: &mut Rasterizer, page: &Page, (ox, oy): (f32, f32)) {
    for line in &page.lines {
        for g in &line.glyphs {
            let Some(face) = fonts.face(g.face) else { continue };
            let (gx, glyph) = raster.glyph(face, g.gid, g.size, ox + line.x + g.x);
            draw_glyph(c, glyph, gx, (oy + line.baseline + g.y).round() as i32, INK);
        }
    }
}

fn advance(shaped: &[ShapedGlyph]) -> f32 {
    shaped.iter().map(|g| g.x_advance).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_is_solid_inside_and_soft_at_corners() {
        let mut px = vec![PAPER; 100 * 100];
        let mut c = Canvas { width: 100, height: 100, stride: 100, pixels: &mut px };
        rounded_rect(&mut c, Rect::new(10, 10, 80, 60), 16, INK);
        let at = |x: usize, y: usize| px[y * 100 + x];
        assert_eq!(at(50, 40), INK);
        assert_eq!(at(10, 10), PAPER, "corner pixel is outside the arc");
        assert_eq!(at(9, 40), PAPER);
        let edge = at(12, 16);
        assert!(edge > INK && edge < PAPER, "anti-aliased arc: {edge}");
    }
}
