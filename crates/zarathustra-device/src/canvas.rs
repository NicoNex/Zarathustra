use crate::geom::Rect;

/// 8-bit grayscale view over a framebuffer (0 = black, 255 = white).
/// `stride` may exceed `width` (hardware line padding).
pub struct Canvas<'a> {
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub pixels: &'a mut [u8],
}

impl Canvas<'_> {
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Multiplies `rect` by `gray`/255: white turns `gray`, black stays black, antialiased
    /// text keeps its shape. A tint laid over what is already drawn.
    pub fn darken(&mut self, rect: Rect, gray: u8) {
        let r = rect.intersect(&self.bounds());
        for y in r.y..r.bottom() {
            let start = y as usize * self.stride + r.x as usize;
            for p in &mut self.pixels[start..start + r.w as usize] {
                *p = (u16::from(*p) * u16::from(gray) / 255) as u8;
            }
        }
    }

    /// The visible pixels, row by row (the stride's padding left out).
    pub fn snapshot(&self) -> Vec<u8> {
        let w = self.width as usize;
        (0..self.height as usize).flat_map(|y| &self.pixels[y * self.stride..y * self.stride + w]).copied().collect()
    }

    /// Puts back what [`Canvas::snapshot`] took; a snapshot of another size is ignored.
    pub fn restore(&mut self, snapshot: &[u8]) {
        let w = self.width as usize;
        if snapshot.len() != w * self.height as usize {
            return;
        }
        for (y, row) in snapshot.chunks_exact(w).enumerate() {
            self.pixels[y * self.stride..y * self.stride + w].copy_from_slice(row);
        }
    }

    pub fn fill(&mut self, rect: Rect, gray: u8) {
        let r = rect.intersect(&self.bounds());
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let start = y as usize * self.stride + r.x as usize;
            self.pixels[start..start + r.w as usize].fill(gray);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_clips_and_respects_stride() {
        let mut buf = vec![9u8; 6 * 3]; // 4 visible columns + 2 padding
        let mut c = Canvas { width: 4, height: 3, stride: 6, pixels: &mut buf };
        c.fill(Rect::new(2, 1, 10, 10), 0);
        assert_eq!(&buf[0..6], &[9, 9, 9, 9, 9, 9]);
        assert_eq!(&buf[6..12], &[9, 9, 0, 0, 9, 9]); // padding untouched
        assert_eq!(&buf[12..18], &[9, 9, 0, 0, 9, 9]);
    }

    #[test]
    fn snapshots_skip_the_padding_and_darken_multiplies() {
        // 2x2 visible, stride 3 (one padding byte per row).
        let mut buf = vec![255, 0, 7, 128, 255, 7];
        let mut c = Canvas { width: 2, height: 2, stride: 3, pixels: &mut buf };
        let shot = c.snapshot();
        assert_eq!(shot, [255, 0, 128, 255]);
        c.darken(Rect::new(0, 0, 2, 2), 215);
        assert_eq!(c.pixels, &[215, 0, 7, 107, 215, 7], "white tints, black stays, padding untouched");
        c.restore(&shot);
        assert_eq!(c.pixels, &[255, 0, 7, 128, 255, 7]);
        c.restore(&[1, 2, 3]); // wrong size: ignored
        assert_eq!(c.pixels[0], 255);
    }
}
