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
}
