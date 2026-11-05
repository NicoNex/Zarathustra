//! The reader menu: a top bar (back to the library, book title) and a bottom sheet with the
//! typography settings, which persist across books and sessions.

use std::path::Path;

use omero_device::{Canvas, Rect};

use crate::ui::{rounded_outline, rounded_rect, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

pub const TOPBAR_H: i32 = 150;
const PAD: i32 = 72;
const ROW_H: i32 = 88;
const GAP: i32 = 28;
const LABEL_W: i32 = 250;
const FONT_COLS: usize = 3;
const MAX_FONTS: usize = 9;

/// Text sizes in device pixels (300 dpi: 40 px ≈ 9.6 pt).
pub const SIZES: [f32; 10] = [28.0, 32.0, 36.0, 40.0, 44.0, 48.0, 54.0, 60.0, 68.0, 76.0];
pub const SPACINGS: [(f32, &str); 3] = [(1.3, "Tight"), (1.45, "Normal"), (1.65, "Loose")];
pub const MARGINS: [(i32, &str); 3] = [(56, "Narrow"), (96, "Normal"), (150, "Wide")];

#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    /// Index into [`SIZES`].
    pub size: usize,
    /// Reading font family (lowercase); `None` keeps the publisher's fonts.
    pub family: Option<String>,
    pub spacing: usize,
    pub margin: usize,
    pub justify: bool,
}

impl Default for Typography {
    fn default() -> Self {
        Typography { size: 3, family: Some("bookerly".into()), spacing: 1, margin: 1, justify: true }
    }
}

impl Typography {
    fn path(state_dir: &Path) -> std::path::PathBuf {
        state_dir.join("typography.txt")
    }

    pub fn load(state_dir: &Path) -> Typography {
        let mut t = Typography::default();
        let Ok(text) = std::fs::read_to_string(Self::path(state_dir)) else { return t };
        for (k, v) in text.lines().filter_map(|l| l.split_once('=')) {
            let index = |n: usize| v.trim().parse::<usize>().ok().filter(|&i| i < n);
            match k.trim() {
                "size" => t.size = index(SIZES.len()).unwrap_or(t.size),
                "spacing" => t.spacing = index(SPACINGS.len()).unwrap_or(t.spacing),
                "margin" => t.margin = index(MARGINS.len()).unwrap_or(t.margin),
                "justify" => t.justify = v.trim() != "false",
                "family" => t.family = Some(v.trim().to_lowercase()).filter(|f| !f.is_empty()),
                _ => {}
            }
        }
        t
    }

    pub fn save(&self, state_dir: &Path) {
        let family = self.family.as_deref().unwrap_or("");
        let text = format!(
            "size={}\nfamily={family}\nspacing={}\nmargin={}\njustify={}\n",
            self.size, self.spacing, self.margin, self.justify
        );
        std::fs::write(Self::path(state_dir), text).ok();
    }

    pub fn font_size(&self) -> f32 {
        SIZES[self.size]
    }

    pub fn line_height(&self) -> f32 {
        SPACINGS[self.spacing].0
    }

    pub fn margin_x(&self) -> i32 {
        MARGINS[self.margin].0
    }
}

/// Something in the menu a tap can hit.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Library,
    Smaller,
    Larger,
    Family(Option<String>),
    Spacing(usize),
    Margin(usize),
    Justify(bool),
}

/// The fonts offered: the publisher's, then the installed reading families.
fn families(ctx: &Ctx) -> Vec<Option<String>> {
    let mut out = vec![None];
    out.extend(ctx.fonts.reading_families().into_iter().map(|f| Some(f.to_string())));
    out.truncate(MAX_FONTS);
    out
}

fn sheet_top(fonts: usize) -> i32 {
    let font_rows = fonts.div_ceil(FONT_COLS) as i32;
    H - (GAP * 2 + ROW_H + GAP + font_rows * (ROW_H + 16) + 3 * (ROW_H + GAP) + 20)
}

/// Every control with its rectangle, top to bottom.
pub fn controls(ctx: &Ctx) -> Vec<(Control, Rect)> {
    let fonts = families(ctx);
    let mut out = vec![(Control::Library, Rect::new(0, 0, 360, TOPBAR_H))];
    let x = PAD + LABEL_W;
    let width = W - PAD - x;
    let mut y = sheet_top(fonts.len()) + GAP * 2;

    out.push((Control::Smaller, Rect::new(x, y, 160, ROW_H)));
    out.push((Control::Larger, Rect::new(W - PAD - 160, y, 160, ROW_H)));
    y += ROW_H + GAP;

    let cell = (W - 2 * PAD - 16 * (FONT_COLS as i32 - 1)) / FONT_COLS as i32;
    for (i, f) in fonts.into_iter().enumerate() {
        let (row, col) = ((i / FONT_COLS) as i32, (i % FONT_COLS) as i32);
        out.push((Control::Family(f), Rect::new(PAD + col * (cell + 16), y + row * (ROW_H + 16), cell, ROW_H)));
    }
    y = out.last().map_or(y, |(_, r)| r.bottom()) + GAP;

    let segments =
        |n: usize, y: i32| (0..n).map(move |i| Rect::new(x + i as i32 * width / n as i32, y, width / n as i32, ROW_H));
    out.extend(segments(SPACINGS.len(), y).enumerate().map(|(i, r)| (Control::Spacing(i), r)));
    y += ROW_H + GAP;
    out.extend(segments(MARGINS.len(), y).enumerate().map(|(i, r)| (Control::Margin(i), r)));
    y += ROW_H + GAP;
    out.extend(segments(2, y).enumerate().map(|(i, r)| (Control::Justify(i == 0), r)));
    out
}

/// Whether a tap at `(x, y)` lands on the menu's chrome (top bar or sheet).
pub fn covers(ctx: &Ctx, y: i32) -> bool {
    y < TOPBAR_H || y >= sheet_top(families(ctx).len())
}

pub fn render(c: &mut Canvas, ctx: &mut Ctx, typo: &Typography, title: &str) {
    let fonts_n = families(ctx).len();
    let controls = controls(ctx);
    let Ctx { fonts, raster, .. } = ctx;
    let (Some(sans), Some(sans_bold), Some(serif_bold)) = (
        fonts.pick(&["sans-serif".into()], 400, false),
        fonts.pick(&["sans-serif".into()], 700, false),
        fonts.pick(&["serif".into()], 700, false),
    ) else {
        return;
    };

    // Top bar.
    c.fill(Rect::new(0, 0, W, TOPBAR_H), PAPER);
    c.fill(Rect::new(0, TOPBAR_H - 2, W, 2), INK);
    let back = Label { face: sans, size: 32.0, ink: INK };
    back.draw(c, raster, "‹  Library", PAD as f32, 92);
    let t = Label { face: serif_bold, size: 34.0, ink: INK };
    let text = t.fit(title, (W - 2 * 380) as f32);
    t.draw(c, raster, &text, (W as f32 - t.width(&text)) / 2.0, 92);

    // Sheet with rounded top corners and a dark edge, like the quick panel.
    let top = sheet_top(fonts_n);
    rounded_rect(c, Rect::new(0, top - 3, W, H - top + 40), 32, INK);
    rounded_rect(c, Rect::new(0, top, W, H - top + 40), 30, PAPER);
    rounded_rect(c, Rect::new(W / 2 - 40, top + 18, 80, 8), 4, LINE);

    let label = Label { face: sans, size: 28.0, ink: INK_SOFT };
    let row_label = |c: &mut Canvas, raster: &mut _, text: &str, r: Rect| {
        label.draw(c, raster, text, PAD as f32, r.y + r.h / 2 + 10);
    };
    let mut labelled = [false; 5];
    for (ctl, r) in &controls {
        let on = match ctl {
            Control::Family(f) => *f == typo.family,
            Control::Spacing(i) => *i == typo.spacing,
            Control::Margin(i) => *i == typo.margin,
            Control::Justify(j) => *j == typo.justify,
            _ => false,
        };
        let (row, text): (usize, String) = match ctl {
            Control::Library => continue,
            Control::Smaller => (0, "A−".into()),
            Control::Larger => (0, "A+".into()),
            Control::Family(f) => (1, f.as_deref().map_or("Original".into(), title_case)),
            Control::Spacing(i) => (2, SPACINGS[*i].1.into()),
            Control::Margin(i) => (3, MARGINS[*i].1.into()),
            Control::Justify(j) => (4, if *j { "Justified" } else { "Left" }.into()),
        };
        if !labelled[row] && row != 1 {
            labelled[row] = true;
            row_label(c, raster, ["Size", "", "Spacing", "Margins", "Alignment"][row], *r);
        }
        if on {
            rounded_rect(c, *r, 20, INK);
        } else {
            rounded_outline(c, *r, 20, 2, LINE, PAPER);
        }
        let ink = if on { PAPER } else { INK };
        // Font names are set in their own face.
        let face = match ctl {
            Control::Family(Some(f)) => fonts.pick(std::slice::from_ref(f), 400, false).unwrap_or(sans),
            Control::Smaller | Control::Larger => sans_bold,
            _ => sans,
        };
        let l = Label { face, size: if matches!(ctl, Control::Family(_)) { 32.0 } else { 30.0 }, ink };
        let text = l.fit(&text, (r.w - 24) as f32);
        l.draw(c, raster, &text, r.x as f32 + (r.w as f32 - l.width(&text)) / 2.0, r.y + r.h / 2 + 11);
    }
    // The current size between the − and + buttons.
    if let (Some((_, minus)), Some((_, plus))) = (controls.get(1), controls.get(2)) {
        let v = Label { face: sans, size: 32.0, ink: INK };
        let text = format!("{}", typo.size + 1);
        let mid = (minus.right() + plus.x) as f32 / 2.0;
        v.draw(c, raster, &text, mid - v.width(&text) / 2.0, minus.y + minus.h / 2 + 11);
    }
}

fn title_case(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Applies a control to the typography; true if it changed.
pub fn apply(typo: &mut Typography, ctl: &Control) -> bool {
    let before = typo.clone();
    match ctl {
        Control::Smaller => typo.size = typo.size.saturating_sub(1),
        Control::Larger => typo.size = (typo.size + 1).min(SIZES.len() - 1),
        Control::Family(f) => typo.family = f.clone(),
        Control::Spacing(i) => typo.spacing = *i,
        Control::Margin(i) => typo.margin = *i,
        Control::Justify(j) => typo.justify = *j,
        Control::Library => {}
    }
    *typo != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typography_round_trips_and_clamps() {
        let dir = std::env::temp_dir().join(format!("omero-typo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let t = Typography { size: 5, family: None, spacing: 2, margin: 0, justify: false };
        t.save(&dir);
        assert_eq!(Typography::load(&dir), t);
        std::fs::write(dir.join("typography.txt"), "size=99\nfamily=Baskerville\n").expect("write");
        let t = Typography::load(&dir);
        assert_eq!((t.size, t.family.as_deref()), (3, Some("baskerville")));
        std::fs::remove_dir_all(dir).ok();

        let mut t = Typography { size: SIZES.len() - 1, ..Typography::default() };
        assert!(!apply(&mut t, &Control::Larger));
        assert!(apply(&mut t, &Control::Smaller));
        assert_eq!(title_case("amazon ember"), "Amazon Ember");
    }
}
