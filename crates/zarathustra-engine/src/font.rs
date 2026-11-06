//! Font faces: shaping (harfrust), hinted outlines (skrifa) and gray8 glyph rasterization (zeno).

use std::collections::HashMap;
use std::sync::Arc;

use harfrust::{Buffer, Direction, Language, ShapeOptions, ShaperFont};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, SmoothMode, Target};
use skrifa::{FontRef, MetadataProvider};
use zarathustra_device::{Canvas, Rect};
use zeno::{Command, Format, Mask, PathBuilder};

/// Horizontal subpixel positions cached per glyph (quarter pixels).
const SUBPIXEL_STEPS: u32 = 4;

/// One font file at its default instance.
pub struct Face {
    /// Unique per font file for as long as layout caches live (they key on it).
    pub id: u32,
    bytes: Arc<Vec<u8>>,
    shaper: harfrust::Font,
    upem: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    pub gid: u32,
    /// Byte offset of the source text this glyph came from.
    pub cluster: u32,
    pub x_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
}

impl Face {
    pub fn from_bytes(id: u32, bytes: Vec<u8>) -> Option<Face> {
        let bytes = Arc::new(bytes);
        let upem = FontRef::new(&bytes).ok()?.metrics(Size::unscaled(), LocationRef::default()).units_per_em as f32;
        // Shares the same allocation with the shaper: no second copy of the font in RAM.
        let shared: Arc<dyn AsRef<[u8]> + Send + Sync> = bytes.clone();
        let shaper = harfrust::Font::new(shared, 0)?;
        Some(Face { id, bytes, shaper, upem })
    }

    fn font_ref(&self) -> FontRef<'_> {
        FontRef::new(&self.bytes).expect("validated in from_bytes")
    }

    /// Shapes one run of text (single script and direction) at `size` pixels per em.
    // ponytail: fresh harfrust Buffer per call; fine at one call per paragraph run, pool it if profiling says so.
    pub fn shape(&self, text: &str, size: f32, language: Option<&str>, out: &mut Vec<ShapedGlyph>) {
        let mut buffer = Buffer::new();
        buffer.push_str(text);
        buffer.set_direction(Direction::LeftToRight);
        buffer.set_language(language.and_then(|l| l.parse::<Language>().ok()));
        buffer.guess_segment_properties();
        let font = ShaperFont::new(&self.shaper);
        if harfrust::shape(&font, &mut buffer, ShapeOptions::new()).is_err() {
            return;
        }
        let scale = size / self.upem;
        out.extend(buffer.glyph_infos().iter().zip(buffer.glyph_positions()).map(|(info, pos)| ShapedGlyph {
            gid: info.glyph_id,
            cluster: info.cluster,
            x_advance: pos.x_advance as f32 * scale,
            x_offset: pos.x_offset as f32 * scale,
            y_offset: pos.y_offset as f32 * scale,
        }));
    }

    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        let m = self.font_ref().metrics(Size::new(size), LocationRef::default());
        LineMetrics { ascent: m.ascent, descent: -m.descent, line_gap: m.leading }
    }
}

/// A rasterized glyph: coverage (0–255, already contrast-adjusted) placed relative to the pen position.
pub struct Glyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
struct GlyphKey {
    face: u32,
    gid: u32,
    /// Size in 1/64 pixel.
    size: u32,
    subpixel: u32,
}

/// Collects skrifa outline commands into a zeno path, flipping y to raster space.
struct ZenoPen<'a> {
    path: &'a mut Vec<Command>,
    dx: f32,
}

impl OutlinePen for ZenoPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to([x + self.dx, -y]);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to([x + self.dx, -y]);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.quad_to([cx + self.dx, -cy], [x + self.dx, -y]);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.curve_to([cx0 + self.dx, -cy0], [cx1 + self.dx, -cy1], [x + self.dx, -y]);
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// Rasterizes and caches glyphs. Hinting is light (vertical only) with linear advances,
/// so layout never depends on hinting.
pub struct Rasterizer {
    glyphs: HashMap<GlyphKey, Glyph>,
    /// `None` when the font can't be hinted; such glyphs are drawn unhinted.
    hinting: HashMap<(u32, u32), Option<HintingInstance>>,
    path: Vec<Command>,
    /// Coverage curve: values > 1 darken stems, which thin e-ink text needs.
    lut: [u8; 256],
}

impl Rasterizer {
    pub fn new(contrast: f32) -> Self {
        let mut lut = [0u8; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = (255.0 * (i as f32 / 255.0).powf(1.0 / contrast.max(0.1))).round() as u8;
        }
        // ponytail: unbounded cache; add LRU eviction when long sessions show memory growth.
        Rasterizer { glyphs: HashMap::new(), hinting: HashMap::new(), path: Vec::new(), lut }
    }

    /// The glyph for pen position `x`, and the integer x to draw it at: `x` is rounded to the
    /// nearest quarter pixel, whose fraction is baked into the cached bitmap.
    pub fn glyph(&mut self, face: &Face, gid: u32, size: f32, x: f32) -> (i32, &Glyph) {
        let q = (x * SUBPIXEL_STEPS as f32).round() as i32;
        let (ix, subpixel) = (q.div_euclid(SUBPIXEL_STEPS as i32), q.rem_euclid(SUBPIXEL_STEPS as i32) as u32);
        let key = GlyphKey { face: face.id, gid, size: (size * 64.0).round() as u32, subpixel };
        if !self.glyphs.contains_key(&key) {
            let glyph = self.render(face, key, size);
            self.glyphs.insert(key, glyph);
        }
        (ix, &self.glyphs[&key])
    }

    fn render(&mut self, face: &Face, key: GlyphKey, size: f32) -> Glyph {
        let empty = Glyph { left: 0, top: 0, width: 0, height: 0, coverage: Vec::new() };
        let font = face.font_ref();
        let outlines = font.outline_glyphs();
        let Some(outline) = outlines.get(key.gid.into()) else { return empty };
        let hinting = self.hinting.entry((key.face, key.size)).or_insert_with(|| {
            let options = HintingOptions {
                engine: Engine::AutoFallback,
                target: Target::Smooth {
                    mode: SmoothMode::Light,
                    symmetric_rendering: true,
                    preserve_linear_metrics: true,
                },
            };
            HintingInstance::new(&outlines, Size::new(size), LocationRef::default(), options).ok()
        });
        self.path.clear();
        let mut pen = ZenoPen { path: &mut self.path, dx: key.subpixel as f32 / SUBPIXEL_STEPS as f32 };
        let hinted = hinting.as_ref().is_some_and(|h| outline.draw(DrawSettings::hinted(h, false), &mut pen).is_ok());
        let drawn = hinted || {
            pen.path.clear(); // drop whatever a failed hinted draw emitted
            outline.draw(DrawSettings::unhinted(Size::new(size), LocationRef::default()), &mut pen).is_ok()
        };
        if !drawn || self.path.is_empty() {
            return empty;
        }
        let (mut coverage, p) = Mask::new(&self.path).format(Format::Alpha).render();
        for c in &mut coverage {
            *c = self.lut[*c as usize];
        }
        Glyph { left: p.left, top: p.top, width: p.width, height: p.height, coverage }
    }
}

/// Blends a glyph onto the canvas with its origin (pen position, baseline) at `(x, y)`.
/// `ink` is the text gray (0 = black).
pub fn draw_glyph(canvas: &mut Canvas, glyph: &Glyph, x: i32, y: i32, ink: u8) -> Rect {
    let r = Rect::new(x + glyph.left, y + glyph.top, glyph.width as i32, glyph.height as i32);
    let clip = r.intersect(&canvas.bounds());
    for row in clip.y..clip.bottom() {
        let src = &glyph.coverage[((row - r.y) as u32 * glyph.width) as usize..][..glyph.width as usize];
        let dst = &mut canvas.pixels[row as usize * canvas.stride..];
        for col in clip.x..clip.right() {
            let a = src[(col - r.x) as usize] as u32;
            if a == 0 {
                continue;
            }
            let d = &mut dst[col as usize];
            *d = ((*d as u32 * (255 - a) + ink as u32 * a + 127) / 255) as u8;
        }
    }
    clip
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bookerly() -> Option<Face> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-fonts/Bookerly-Regular.ttf");
        Face::from_bytes(0, std::fs::read(path).ok()?)
    }

    #[test]
    fn shapes_and_rasterizes_text() {
        let Some(face) = bookerly() else { return }; // dev font is not in the repo
        let mut glyphs = Vec::new();
        face.shape("office", 32.0, Some("en"), &mut glyphs);
        // "ffi" ligature: fewer glyphs than chars, clusters point back to byte offsets.
        assert!(glyphs.len() < 6, "expected a ligature, got {}", glyphs.len());
        assert_eq!(glyphs[0].cluster, 0);
        let width: f32 = glyphs.iter().map(|g| g.x_advance).sum();
        assert!(width > 60.0 && width < 120.0, "width {width}");

        let mut r = Rasterizer::new(1.0);
        let (x, g) = r.glyph(&face, glyphs[0].gid, 32.0, 9.9);
        assert_eq!(x, 10, "9.9 rounds to the next whole pixel");
        assert!(g.width > 5 && g.height > 5 && g.top < 0, "glyph above baseline");
        assert!(g.coverage.contains(&255));
    }

    #[test]
    fn draw_glyph_blends_and_clips() {
        let glyph = Glyph { left: -1, top: -1, width: 2, height: 2, coverage: vec![255, 128, 0, 255] };
        let mut px = vec![255u8; 9];
        let mut c = Canvas { width: 3, height: 3, stride: 3, pixels: &mut px };
        let dirty = draw_glyph(&mut c, &glyph, 0, 0, 0); // half off-canvas
        assert_eq!(dirty, Rect::new(0, 0, 1, 1));
        assert_eq!(px[0], 0); // coverage 255 at (0,0)
    }

    #[test]
    fn contrast_lut_darkens_midtones_only() {
        let r = Rasterizer::new(1.5);
        assert_eq!((r.lut[0], r.lut[255]), (0, 255));
        assert!(r.lut[128] > 128);
    }
}
