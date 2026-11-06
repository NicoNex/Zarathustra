//! A compact on-screen keyboard for e-ink: QWERTY with a row of Italian accented vowels, a
//! numbers-and-symbols layer, backspace, space and a search key. Keys are 100 px tall
//! (≈ 8.5 mm at 300 dpi); a press inverts only the key under the finger, so feedback is one
//! small black-and-white refresh.
// ponytail: no shift: search and library filtering ignore case. Add a shift key (and
// long-press accents) when the keyboard is used for notes.

use zarathustra_device::{Canvas, Rect};

use crate::i18n::t;
use crate::ui::{button, Label, INK, PAPER};
use crate::{Ctx, H, W};

/// Longest query the keyboard edits (library and book search alike).
pub const MAX_QUERY: usize = 60;

/// Height of the keyboard band at the bottom of the screen.
pub const KEYBOARD_H: i32 = 500;
const SIDE: i32 = 12;
const PAD_TOP: i32 = 32;
const KEY_H: i32 = 100;
const ROW_PITCH: i32 = 112;
const GAP: i32 = 10;
/// Columns are laid out in half-key units across the width.
const UNITS: i32 = 20;

const LETTERS: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const SYMBOLS: [&str; 3] = ["1234567890", "@#€&-/:;()", "?!'\"%*+"];

/// What a long press on a key offers: accents and close relatives, most used first.
fn variants(c: char) -> &'static str {
    match c {
        'a' => "àáâäãåæ",
        'e' => "èéêë€",
        'i' => "ìíîï",
        'o' => "òóôöõøœ",
        'u' => "ùúûü",
        'c' => "ç©",
        'n' => "ñ",
        's' => "ß§",
        'y' => "ýÿ",
        '\'' => "’‘«»",
        '"' => "“”«»",
        '-' => "–—_",
        '?' => "¿",
        '!' => "¡",
        '€' => "$£¥",
        _ => "",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Space,
    /// Switch between letters and numbers/symbols.
    Layer,
    Enter,
    /// A tap that only closes the accents strip.
    Dismiss,
}

#[derive(Debug, Default)]
pub struct Keyboard {
    symbols: bool,
    /// The key held down, drawn inverted.
    pressed: Option<Rect>,
    /// The accents strip opened by a long press: each variant with its key.
    strip: Vec<(char, Rect)>,
}

/// Left edge of half-key column `u`.
fn unit_x(u: i32) -> i32 {
    SIDE + u * (W - 2 * SIDE) / UNITS
}

/// The keyboard band.
pub fn area() -> Rect {
    Rect::new(0, H - KEYBOARD_H, W, KEYBOARD_H)
}

impl Keyboard {
    /// Every key with its rectangle, top row first.
    pub fn keys(&self) -> Vec<(Key, Rect)> {
        let top = H - KEYBOARD_H + PAD_TOP;
        let x = unit_x;
        let key =
            |row: i32, u0: i32, u1: i32| Rect::new(x(u0) + GAP / 2, top + row * ROW_PITCH, x(u1) - x(u0) - GAP, KEY_H);
        let rows = if self.symbols { SYMBOLS } else { LETTERS };
        let mut out = Vec::new();
        for (row, chars) in rows.into_iter().enumerate() {
            // Each row centred: n keys of two units each.
            let n = chars.chars().count() as i32;
            let start = (UNITS - 2 * n) / 2;
            for (i, c) in chars.chars().enumerate() {
                let u = start + 2 * i as i32;
                out.push((Key::Char(c), key(row as i32, u, u + 2)));
            }
        }
        out.push((Key::Backspace, key(2, 17, UNITS)));
        // Comma and full stop beside the space, on both layers.
        out.push((Key::Layer, key(3, 0, 3)));
        out.push((Key::Char(','), key(3, 3, 5)));
        out.push((Key::Space, key(3, 5, 13)));
        out.push((Key::Char('.'), key(3, 13, 15)));
        out.push((Key::Enter, key(3, 15, UNITS)));
        out
    }

    /// The key at `(x, y)`, if any.
    pub fn hit(&self, x: i32, y: i32) -> Option<(Key, Rect)> {
        self.keys().into_iter().find(|(_, r)| r.contains(x, y))
    }

    /// A touch at `(x, y)` is the keyboard's: on its band, or anywhere while the accents strip
    /// is open (a tap on it types, elsewhere closes it, and nothing under it is hit).
    pub fn holds(&self, x: i32, y: i32) -> bool {
        area().contains(x, y) || !self.strip.is_empty()
    }

    /// The finger went down at `(x, y)`: the key there shows pressed. Returns its rectangle.
    pub fn press(&mut self, x: i32, y: i32) -> Option<Rect> {
        self.pressed = self.hit(x, y).map(|(_, r)| r);
        self.pressed
    }

    /// A finger held on a key with variants: their strip opens above it, to be tapped next.
    /// Returns the area to redraw.
    pub fn long_press(&mut self, x: i32, y: i32) -> Option<Rect> {
        let (Key::Char(c), key) = self.hit(x, y)? else { return None };
        let chars = variants(c);
        if chars.is_empty() {
            return None;
        }
        let n = chars.chars().count() as i32;
        let w = unit_x(2) - unit_x(0);
        let x0 = (key.x + key.w / 2 - n * w / 2).clamp(SIDE, W - SIDE - n * w);
        let y = key.y - ROW_PITCH - 8;
        self.strip = chars.chars().enumerate().map(|(i, v)| (v, Rect::new(x0 + i as i32 * w, y, w, KEY_H))).collect();
        self.pressed = None;
        Some(self.strip_area().union(&key))
    }

    /// The accents strip with its frame.
    fn strip_area(&self) -> Rect {
        let r = self.strip.iter().fold(Rect::default(), |a, (_, r)| a.union(r));
        Rect::new(r.x - 12, r.y - 12, r.w + 24, r.h + 24)
    }

    /// The finger lifted after going down at `(x, y)`: the key typed, and the area to redraw
    /// (the key, or the whole keyboard when the layer changed). With the accents strip open,
    /// a variant tapped is typed and anything else just closes it.
    pub fn tap(&mut self, x: i32, y: i32) -> Option<(Key, Rect)> {
        if !self.strip.is_empty() {
            let strip = self.strip_area();
            let hit = self.strip.iter().find(|(_, r)| r.contains(x, y)).map(|&(c, _)| c);
            self.strip.clear();
            self.pressed = None;
            return Some((hit.map_or(Key::Dismiss, Key::Char), strip.union(&area())));
        }
        let pressed = self.pressed.take();
        let (key, rect) = self.hit(x, y)?;
        if key == Key::Layer {
            self.symbols = !self.symbols;
            return Some((key, area()));
        }
        Some((key, pressed.map_or(rect, |p| p.union(&rect))))
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        let band = area();
        c.fill(band, PAPER);
        c.fill(Rect::new(0, band.y, W, 3), INK);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(serif), Some(sans), Some(sans_bold)) =
            (crate::serif(fonts, false), crate::sans(fonts, false), crate::sans(fonts, true))
        else {
            return;
        };
        for (key, r) in self.keys() {
            // Pressed keys and the search key are solid black; the rest outlined.
            let on = self.pressed == Some(r) || key == Key::Enter;
            let ink = if on { PAPER } else { INK };
            button(c, r, on);
            match key {
                // Letters in the reading face, as they will appear in the book.
                Key::Char(ch) => {
                    let mut utf8 = [0; 4];
                    Label { face: serif, size: 46.0, ink }.centered(c, raster, ch.encode_utf8(&mut utf8), r);
                }
                Key::Space => Label { face: sans, size: 30.0, ink }.centered(c, raster, t().space, r),
                Key::Layer => {
                    let text = if self.symbols { "abc" } else { "?123" };
                    Label { face: sans_bold, size: 32.0, ink }.centered(c, raster, text, r);
                }
                Key::Enter => Label { face: sans_bold, size: 32.0, ink }.centered(c, raster, t().search, r),
                Key::Backspace => backspace(c, r, ink),
                Key::Dismiss => {}
            }
        }
        if !self.strip.is_empty() {
            // A raised strip: a heavy frame, then keys like the rest.
            let frame = self.strip_area();
            crate::ui::rounded_outline(c, frame, 16, 4, INK, PAPER);
            let mut utf8 = [0; 4];
            for &(ch, r) in &self.strip {
                button(c, r, false);
                Label { face: serif, size: 46.0, ink: INK }.centered(c, raster, ch.encode_utf8(&mut utf8), r);
            }
        }
    }
}

/// The backspace glyph, drawn: a solid tag pointing left with a cross cut out of it.
fn backspace(c: &mut Canvas, r: Rect, ink: u8) {
    let (h, body) = (40, 52);
    let tip = h / 2;
    let x0 = r.x + (r.w - body - tip) / 2;
    let y0 = r.y + (r.h - h) / 2;
    for dy in 0..h {
        // The point: rows narrow towards the top and bottom edges.
        let inset = (dy - tip).abs();
        c.fill(Rect::new(x0 + inset, y0 + dy, tip - inset + body, 1), ink);
    }
    // The cross, in the key's background colour, two strokes 4 px thick.
    let bg = if ink == INK { PAPER } else { INK };
    let (cx, cy, arm) = (x0 + tip + body / 2 - 2, y0 + h / 2, 10);
    for d in -arm..=arm {
        c.fill(Rect::new(cx + d - 2, cy + d - 2, 4, 4), bg);
        c.fill(Rect::new(cx + d - 2, cy - d - 2, 4, 4), bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centre(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    #[test]
    fn keys_fit_the_band_without_overlapping() {
        for symbols in [false, true] {
            let kb = Keyboard { symbols, ..Keyboard::default() };
            let keys = kb.keys();
            assert_eq!(keys.len(), if symbols { 10 + 10 + 7 } else { 10 + 9 + 7 } + 6);
            for (i, (_, a)) in keys.iter().enumerate() {
                assert!(a.h >= 96 && a.w >= 96, "touch target {a:?}");
                assert!(a.x >= 0 && a.right() <= W && a.y >= H - KEYBOARD_H && a.bottom() <= H, "{a:?}");
                for (_, b) in &keys[i + 1..] {
                    assert!(a.intersect(b).is_empty(), "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn hits_letters_accents_and_function_keys() {
        let mut kb = Keyboard::default();
        let find = |kb: &Keyboard, k: Key| kb.keys().into_iter().find(|&(key, _)| key == k).map(|(_, r)| r);
        let q = find(&kb, Key::Char('q')).expect("q");
        let p = find(&kb, Key::Char('p')).expect("p");
        assert!(q.x < 30 && p.right() > W - 30, "the top letter row spans the width");
        // A long press on e offers è, é…; tapping one types it, tapping elsewhere just closes.
        let e = find(&kb, Key::Char('e')).expect("e");
        assert!(kb.long_press(centre(e).0, centre(e).1).is_some());
        let grave = kb.strip[0].1;
        assert_eq!(kb.tap(centre(grave).0, centre(grave).1).map(|t| t.0), Some(Key::Char('è')));
        assert!(kb.long_press(centre(e).0, centre(e).1).is_some());
        assert_eq!(kb.tap(centre(q).0, centre(q).1).map(|t| t.0), Some(Key::Dismiss));
        assert_eq!(kb.long_press(centre(q).0, centre(q).1), None, "q has no variants");
        assert!(find(&kb, Key::Char(',')).is_some(), "comma on the letters");
        let bs = find(&kb, Key::Backspace).expect("backspace");
        assert_eq!(kb.press(centre(bs).0, centre(bs).1), Some(bs));
        assert_eq!(kb.tap(centre(bs).0, centre(bs).1), Some((Key::Backspace, bs)));
        // Between keys and above the keyboard: nothing.
        assert_eq!(kb.hit(q.right() + GAP / 2, centre(q).1), None);
        assert_eq!(kb.hit(600, H - KEYBOARD_H - 10), None);
        // The layer key swaps letters for digits in place.
        let layer = find(&kb, Key::Layer).expect("layer");
        assert_eq!(kb.tap(centre(layer).0, centre(layer).1), Some((Key::Layer, area())));
        assert_eq!(kb.hit(centre(q).0, centre(q).1).map(|h| h.0), Some(Key::Char('1')));
        assert!(find(&kb, Key::Char('€')).is_some());
    }
}
