//! A compact on-screen keyboard for e-ink: QWERTY with accents on a long press, a
//! numbers-and-symbols layer, shift, backspace, space and an enter key. Keys are 100 px tall
//! (≈ 8.5 mm at 300 dpi).
//!
//! Typing has to keep up with the finger on a panel that needs a good tenth of a second per
//! refresh, so the band is drawn once per state (layer, shift) into two images, keys up and
//! keys down, and copied from them afterwards: no 33 rounded keys and shaped labels per touch.
//! A press shows by copying one key from the "down" image, refreshed alone in fast black and
//! white; what a touch changed comes back as a [`Typed`], keys and text refreshed apart.

use std::cell::RefCell;
use std::fmt;
use std::time::{Duration, Instant};

use zarathustra_device::{Canvas, Rect};

use crate::i18n::t;
use crate::ui::{button, Label, INK, PAPER};
use crate::{screen_h, screen_w, Ctx};

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
/// Two taps on shift closer than this lock the capitals.
const DOUBLE_TAP: Duration = Duration::from_millis(450);

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
    /// Capitals: the next letter, or every letter once locked.
    Shift,
    Enter,
    /// A tap that only closes the accents strip.
    Dismiss,
}

/// The shift key's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shift {
    #[default]
    Off,
    /// The next letter is a capital, then shift goes off.
    Once,
    /// Capitals until shift is tapped again (caps lock).
    Locked,
}

/// What a touch on the keyboard asks the screen to refresh once redrawn: `keys` in fast
/// black and white, `text` (the field, and whatever follows the text) in gray. Either may be
/// empty. They are refreshed apart, so a key at the bottom and a field at the top don't
/// become one tall rectangle in the slow gray waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Typed {
    pub keys: Rect,
    pub text: Rect,
}

impl Typed {
    /// Only keys changed.
    pub fn keys(keys: Rect) -> Typed {
        Typed { keys, text: Rect::default() }
    }

    /// The same, with `text` refreshed in gray too.
    pub fn and_text(self, text: Rect) -> Typed {
        Typed { text: self.text.union(&text), ..self }
    }
}

#[derive(Debug, Default)]
pub struct Keyboard {
    symbols: bool,
    shift: Shift,
    /// The last tap on shift, for telling a double tap.
    shift_at: Option<Instant>,
    /// The enter key's label; `None` is "Search".
    pub enter: Option<&'static str>,
    /// The key held down, drawn inverted.
    pressed: Option<Rect>,
    /// The accents strip opened by a long press: each variant with its key.
    strip: Vec<(char, Rect)>,
    /// The band as last drawn (see the module docs); filled by [`Keyboard::render`].
    images: RefCell<Option<Images>>,
}

/// What the band images show: layer, shift and the enter key's label.
type State = (bool, Shift, Option<&'static str>);

/// The keyboard band drawn for one [`State`], every key up and every key down.
struct Images {
    state: State,
    up: Vec<u8>,
    down: Vec<u8>,
}

impl fmt::Debug for Images {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Images").field("state", &self.state).finish_non_exhaustive()
    }
}

impl Keyboard {
    /// Opened on the numbers and symbols, with `enter` as the enter key's label.
    pub fn numbers(enter: &'static str) -> Keyboard {
        Keyboard { symbols: true, enter: Some(enter), ..Keyboard::default() }
    }
}

/// Left edge of half-key column `u`.
fn unit_x(u: i32) -> i32 {
    SIDE + u * (screen_w() - 2 * SIDE) / UNITS
}

/// The keyboard band.
pub fn area() -> Rect {
    Rect::new(0, screen_h() - KEYBOARD_H, screen_w(), KEYBOARD_H)
}

/// Copies `r` (screen coordinates, clipped to the band) from a band image into `c`.
fn blit(band: &[u8], c: &mut Canvas, r: Rect) {
    let r = r.intersect(&area()).intersect(&c.bounds());
    let top = area().y;
    for y in r.y..r.bottom() {
        let src = (y - top) as usize * screen_w() as usize + r.x as usize;
        let dst = y as usize * c.stride + r.x as usize;
        c.pixels[dst..dst + r.w as usize].copy_from_slice(&band[src..src + r.w as usize]);
    }
}

impl Keyboard {
    /// Every key with its rectangle, top row first.
    pub fn keys(&self) -> Vec<(Key, Rect)> {
        let top = screen_h() - KEYBOARD_H + PAD_TOP;
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
                out.push((Key::Char(self.case(c)), key(row as i32, u, u + 2)));
            }
        }
        if !self.symbols {
            out.push((Key::Shift, key(2, 0, 3)));
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

    pub fn shift(&self) -> Shift {
        self.shift
    }

    /// `c` in capitals while shift is on.
    fn case(&self, c: char) -> char {
        if self.shift == Shift::Off {
            c
        } else {
            c.to_uppercase().next().unwrap_or(c)
        }
    }

    /// Shift tapped at `now`: off → once; once → locked on a double tap, else off; locked → off.
    fn tap_shift(&mut self, now: Instant) {
        let double = self.shift_at.is_some_and(|t| now.saturating_duration_since(t) < DOUBLE_TAP);
        self.shift = match self.shift {
            Shift::Off => Shift::Once,
            Shift::Once if double => Shift::Locked,
            Shift::Once | Shift::Locked => Shift::Off,
        };
        self.shift_at = Some(now);
    }

    /// `key` was typed: a one-shot shift is used up. Whether the letters changed with it.
    fn typed(&mut self, key: Key) -> bool {
        // A digit or symbol in between leaves the capital for the next letter.
        let used = matches!(key, Key::Char(c) if c.is_alphabetic()) && self.shift == Shift::Once;
        if used {
            self.shift = Shift::Off;
        }
        used && !self.symbols
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

    /// The finger went down at `(x, y)`: the key there shows pressed. Returns its rectangle,
    /// for [`Keyboard::draw_key`].
    pub fn press(&mut self, x: i32, y: i32) -> Option<Rect> {
        self.pressed = self.hit(x, y).map(|(_, r)| r).filter(|_| self.strip.is_empty());
        self.pressed
    }

    /// A finger held on a key with variants: their strip opens above it, to be tapped next.
    /// Returns the area to refresh (keys only: black and white).
    pub fn long_press(&mut self, x: i32, y: i32) -> Option<Rect> {
        let (Key::Char(c), key) = self.hit(x, y)? else { return None };
        let chars = variants(c.to_lowercase().next().unwrap_or(c));
        if chars.is_empty() {
            return None;
        }
        let n = chars.chars().count() as i32;
        let w = unit_x(2) - unit_x(0);
        let x0 = (key.x + key.w / 2 - n * w / 2).clamp(SIDE, screen_w() - SIDE - n * w);
        let y = key.y - ROW_PITCH - 8;
        self.strip =
            chars.chars().enumerate().map(|(i, v)| (self.case(v), Rect::new(x0 + i as i32 * w, y, w, KEY_H))).collect();
        self.pressed = None;
        Some(self.strip_area().union(&key))
    }

    /// The accents strip with its frame.
    fn strip_area(&self) -> Rect {
        let r = self.strip.iter().fold(Rect::default(), |a, (_, r)| a.union(r));
        Rect::new(r.x - 12, r.y - 12, r.w + 24, r.h + 24)
    }

    /// The finger lifted after going down at `(x, y)`: the key typed, and what to refresh.
    /// With the accents strip open, a variant tapped is typed and anything else just closes it.
    pub fn tap(&mut self, x: i32, y: i32) -> Option<(Key, Typed)> {
        self.tap_at(x, y, Instant::now())
    }

    fn tap_at(&mut self, x: i32, y: i32, now: Instant) -> Option<(Key, Typed)> {
        if !self.strip.is_empty() {
            let strip = self.strip_area();
            let key = self.strip.iter().find(|(_, r)| r.contains(x, y)).map_or(Key::Dismiss, |&(c, _)| Key::Char(c));
            self.strip.clear();
            self.pressed = None;
            self.typed(key);
            // Above the band, the strip covered the screen's own (gray) drawing.
            let above = strip.intersect(&Rect::new(0, 0, screen_w(), area().y));
            return Some((key, Typed { keys: area(), text: above }));
        }
        let pressed = self.pressed.take();
        let (key, rect) = self.hit(x, y)?;
        let changed = match key {
            Key::Layer => {
                self.symbols = !self.symbols;
                true
            }
            Key::Shift => {
                self.tap_shift(now);
                true
            }
            // A capital typed with shift once: the letters go back to lower case.
            _ => self.typed(key),
        };
        Some((key, Typed::keys(if changed { area() } else { pressed.map_or(rect, |p| p.union(&rect)) })))
    }

    fn state(&self) -> State {
        (self.symbols, self.shift, self.enter)
    }

    /// Draws the band (from its images, painting them first when the state changed), the key
    /// held down and the accents strip.
    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        {
            let mut images = self.images.borrow_mut();
            if images.as_ref().is_none_or(|i| i.state != self.state()) {
                *images = self.paint(ctx);
            }
        }
        self.draw_key(c, area());
        if let Some(r) = self.pressed {
            self.draw_key(c, r);
        }
        if self.strip.is_empty() {
            return;
        }
        let Ctx { fonts, raster, .. } = ctx;
        let Some(serif) = crate::serif(fonts, false) else { return };
        // A raised strip: a heavy frame, then keys like the rest.
        crate::ui::rounded_outline(c, self.strip_area(), 16, 4, INK, PAPER);
        let mut utf8 = [0; 4];
        for &(ch, r) in &self.strip {
            button(c, r, false);
            Label { face: serif, size: 46.0, ink: INK }.centered(c, raster, ch.encode_utf8(&mut utf8), r);
        }
    }

    /// Copies `r` of the band as it stands (the key held down shows pressed) from the images
    /// of the last [`Keyboard::render`]: no fonts, no shaping. False when they are missing or
    /// out of date, so render instead.
    pub fn draw_key(&self, c: &mut Canvas, r: Rect) -> bool {
        let images = self.images.borrow();
        let Some(i) = images.as_ref().filter(|i| i.state == self.state()) else { return false };
        blit(if self.pressed == Some(r) { &i.down } else { &i.up }, c, r);
        true
    }

    /// Both images of the band for the current state; `None` without fonts.
    fn paint(&self, ctx: &mut Ctx) -> Option<Images> {
        let len = (screen_w() * KEYBOARD_H) as usize;
        let (mut up, mut down) = (vec![PAPER; len], vec![PAPER; len]);
        for (band, pressed) in [(&mut up, false), (&mut down, true)] {
            let mut c = Canvas { width: screen_w(), height: KEYBOARD_H, stride: screen_w() as usize, pixels: band };
            self.draw_band(&mut c, ctx, pressed)?;
        }
        Some(Images { state: self.state(), up, down })
    }

    /// The band into `c`, a canvas the band's size: every key up, or every key `pressed`
    /// (drawn the other way round).
    fn draw_band(&self, c: &mut Canvas, ctx: &mut Ctx, pressed: bool) -> Option<()> {
        let Ctx { fonts, raster, .. } = ctx;
        let (serif, sans, sans_bold) =
            (crate::serif(fonts, false)?, crate::sans(fonts, false)?, crate::sans(fonts, true)?);
        c.fill(Rect::new(0, 0, screen_w(), 3), INK);
        let top = area().y;
        for (key, r) in self.keys() {
            let r = Rect::new(r.x, r.y - top, r.w, r.h);
            // The enter key is solid black, the rest outlined.
            let on = (key == Key::Enter) != pressed;
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
                Key::Enter => {
                    Label { face: sans_bold, size: 32.0, ink }.centered(c, raster, self.enter.unwrap_or(t().search), r)
                }
                Key::Backspace => backspace(c, r, ink),
                Key::Shift => shift(c, r, ink, self.shift),
                Key::Dismiss => {}
            }
        }
        Some(())
    }
}

/// The shift glyph, drawn: an arrow up (a triangle on a stem), hollow while off, solid for
/// the next letter, solid over a bar when locked.
fn shift(c: &mut Canvas, r: Rect, ink: u8, state: Shift) {
    let bg = if ink == INK { PAPER } else { INK };
    let (cx, top) = (r.x + r.w / 2, r.y + r.h / 2 - 30);
    for dy in 0..22 {
        c.fill(Rect::new(cx - dy - 1, top + dy, 2 * dy + 2, 1), ink);
    }
    c.fill(Rect::new(cx - 9, top + 22, 18, 22), ink);
    match state {
        // An outline about 3 px thick: the inside of the head and of the stem cut out.
        Shift::Off => {
            for dy in 6..19 {
                c.fill(Rect::new(cx - dy + 4, top + dy, 2 * (dy - 4), 1), bg);
            }
            c.fill(Rect::new(cx - 6, top + 19, 12, 22), bg);
        }
        Shift::Once => {}
        Shift::Locked => c.fill(Rect::new(cx - 14, top + 50, 28, 6), ink),
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

    fn find(kb: &Keyboard, k: Key) -> Option<Rect> {
        kb.keys().into_iter().find(|&(key, _)| key == k).map(|(_, r)| r)
    }

    /// Taps `k` at `now`; the key typed.
    fn tap(kb: &mut Keyboard, k: Key, now: Instant) -> Option<Key> {
        let (x, y) = centre(find(kb, k).expect("key on the keyboard"));
        kb.press(x, y);
        kb.tap_at(x, y, now).map(|t| t.0)
    }

    #[test]
    fn keys_fit_the_band_without_overlapping() {
        for symbols in [false, true] {
            let kb = Keyboard { symbols, ..Keyboard::default() };
            let keys = kb.keys();
            // Letters: 26 and shift; symbols: 27. Then six function keys on both.
            assert_eq!(keys.len(), 27 + 6);
            for (i, (_, a)) in keys.iter().enumerate() {
                assert!(a.h >= 96 && a.w >= 96, "touch target {a:?}");
                assert!(
                    a.x >= 0 && a.right() <= screen_w() && a.y >= screen_h() - KEYBOARD_H && a.bottom() <= screen_h(),
                    "{a:?}"
                );
                for (_, b) in &keys[i + 1..] {
                    assert!(a.intersect(b).is_empty(), "{a:?} overlaps {b:?}");
                }
            }
        }
    }

    #[test]
    fn hits_letters_accents_and_function_keys() {
        let mut kb = Keyboard::default();
        let q = find(&kb, Key::Char('q')).expect("q");
        let p = find(&kb, Key::Char('p')).expect("p");
        assert!(q.x < 30 && p.right() > screen_w() - 30, "the top letter row spans the width");
        // A long press on e offers è, é…; tapping one types it, tapping elsewhere just closes.
        let e = find(&kb, Key::Char('e')).expect("e");
        assert!(kb.long_press(centre(e).0, centre(e).1).is_some());
        let grave = kb.strip[0].1;
        let (key, typed) = kb.tap(centre(grave).0, centre(grave).1).expect("typed");
        assert_eq!(key, Key::Char('è'));
        assert_eq!(typed.keys, area());
        assert!(!typed.text.is_empty() && typed.text.bottom() <= area().y, "the screen under the strip, in gray");
        assert!(kb.long_press(centre(e).0, centre(e).1).is_some());
        assert_eq!(kb.tap(centre(q).0, centre(q).1).map(|t| t.0), Some(Key::Dismiss));
        assert_eq!(kb.long_press(centre(q).0, centre(q).1), None, "q has no variants");
        assert!(find(&kb, Key::Char(',')).is_some(), "comma on the letters");
        // A plain key: only it is refreshed, black and white.
        let bs = find(&kb, Key::Backspace).expect("backspace");
        assert_eq!(kb.press(centre(bs).0, centre(bs).1), Some(bs));
        assert_eq!(kb.tap(centre(bs).0, centre(bs).1), Some((Key::Backspace, Typed::keys(bs))));
        // Between keys and above the keyboard: nothing.
        assert_eq!(kb.hit(q.right() + GAP / 2, centre(q).1), None);
        assert_eq!(kb.hit(600, screen_h() - KEYBOARD_H - 10), None);
        // The layer key swaps letters for digits in place.
        let layer = find(&kb, Key::Layer).expect("layer");
        assert_eq!(kb.tap(centre(layer).0, centre(layer).1), Some((Key::Layer, Typed::keys(area()))));
        assert_eq!(kb.hit(centre(q).0, centre(q).1).map(|h| h.0), Some(Key::Char('1')));
        assert!(find(&kb, Key::Char('€')).is_some());
    }

    #[test]
    fn shift_once_types_one_capital_then_lower_case() {
        let mut kb = Keyboard::default();
        let t0 = Instant::now();
        tap(&mut kb, Key::Shift, t0);
        assert_eq!(kb.shift(), Shift::Once);
        assert_eq!(tap(&mut kb, Key::Char('A'), t0 + Duration::from_secs(1)), Some(Key::Char('A')));
        assert_eq!(kb.shift(), Shift::Off, "used up by the letter");
        assert_eq!(tap(&mut kb, Key::Char('a'), t0 + Duration::from_secs(2)), Some(Key::Char('a')));
        // Tapped again slowly, a one-shot shift goes off instead of locking.
        tap(&mut kb, Key::Shift, t0 + Duration::from_secs(3));
        tap(&mut kb, Key::Shift, t0 + Duration::from_secs(4));
        assert_eq!(kb.shift(), Shift::Off);
        // Space and backspace leave it waiting for its letter.
        tap(&mut kb, Key::Shift, t0 + Duration::from_secs(5));
        tap(&mut kb, Key::Space, t0 + Duration::from_secs(6));
        tap(&mut kb, Key::Backspace, t0 + Duration::from_secs(7));
        assert_eq!(kb.shift(), Shift::Once);
    }

    #[test]
    fn double_tapped_shift_locks_until_tapped_again() {
        let mut kb = Keyboard::default();
        let t0 = Instant::now();
        tap(&mut kb, Key::Shift, t0);
        tap(&mut kb, Key::Shift, t0 + Duration::from_millis(200));
        assert_eq!(kb.shift(), Shift::Locked);
        for (i, ch) in "ABC".chars().enumerate() {
            let at = t0 + Duration::from_secs(1 + i as u64);
            assert_eq!(tap(&mut kb, Key::Char(ch), at), Some(Key::Char(ch)), "caps stays on");
        }
        tap(&mut kb, Key::Shift, t0 + Duration::from_secs(9));
        assert_eq!(kb.shift(), Shift::Off);
        assert!(find(&kb, Key::Char('a')).is_some(), "back to lower case");
    }

    #[test]
    fn accents_follow_the_case() {
        let mut kb = Keyboard::default();
        let t0 = Instant::now();
        tap(&mut kb, Key::Shift, t0);
        let a = find(&kb, Key::Char('A')).expect("capital A");
        assert!(kb.long_press(centre(a).0, centre(a).1).is_some());
        assert_eq!(kb.strip[0].0, 'À');
        let grave = kb.strip[0].1;
        assert_eq!(kb.tap_at(centre(grave).0, centre(grave).1, t0).map(|t| t.0), Some(Key::Char('À')));
        assert_eq!(kb.shift(), Shift::Off, "the accented capital used up shift");
        assert!(kb.long_press(centre(a).0, centre(a).1).is_some());
        assert_eq!(kb.strip[0].0, 'à');
    }
}
