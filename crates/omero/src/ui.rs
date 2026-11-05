//! Drawing primitives for the shell's own UI: anti-aliased rounded rectangles and single
//! lines of text, in the grayscale palette of the design spec.

use omero_device::{Canvas, Rect};
use omero_engine::font::{draw_glyph, Face, Rasterizer};

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

/// A rounded outline `width` pixels thick.
pub fn rounded_outline(c: &mut Canvas, r: Rect, radius: i32, width: i32, color: u8, inside: u8) {
    rounded_rect(c, r, radius, color);
    let inner = Rect::new(r.x + width, r.y + width, r.w - 2 * width, r.h - 2 * width);
    rounded_rect(c, inner, (radius - width).max(0), inside);
}

/// Single-line text in one face and size.
pub struct Label<'a> {
    pub face: &'a Face,
    pub size: f32,
    pub ink: u8,
}

impl Label<'_> {
    pub fn width(&self, text: &str) -> f32 {
        let mut shaped = Vec::new();
        self.face.shape(text, self.size, None, &mut shaped);
        shaped.iter().map(|g| g.x_advance).sum()
    }

    /// `text` cut with an ellipsis so it fits in `max_width`.
    pub fn fit(&self, text: &str, max_width: f32) -> String {
        if self.width(text) <= max_width {
            return text.to_string();
        }
        let mut cut: Vec<char> = text.chars().collect();
        while !cut.is_empty() {
            cut.pop();
            let s: String = cut.iter().collect::<String>().trim_end().to_string() + "…";
            if self.width(&s) <= max_width {
                return s;
            }
        }
        "…".into()
    }

    /// Draws `text` with its baseline at `y`; returns the advance.
    pub fn draw(&self, c: &mut Canvas, raster: &mut Rasterizer, text: &str, x: f32, y: i32) -> f32 {
        let mut shaped = Vec::new();
        self.face.shape(text, self.size, None, &mut shaped);
        let mut pen = x;
        for g in &shaped {
            let (gx, glyph) = raster.glyph(self.face, g.gid, self.size, pen + g.x_offset);
            draw_glyph(c, glyph, gx, y - g.y_offset.round() as i32, self.ink);
            pen += g.x_advance;
        }
        pen - x
    }
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
