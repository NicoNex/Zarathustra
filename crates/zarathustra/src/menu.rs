//! The reader menu: a top bar (back to the library, book title) and a low bottom sheet with
//! the table of contents entry, the position scrubber and the typography in two rows (size and
//! font; spacing, margins and alignment as icons), so most of the page stays in view and
//! shows each change. The settings persist across books and sessions.

use std::path::Path;

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::font::{Face, Rasterizer};
use zarathustra_engine::style::css::TextAlign;

use crate::i18n::t;
use crate::ui::{button, rounded_rect, segmented, segments, Label, INK, INK_SOFT, PAPER};
use crate::{Ctx, H, W};

pub const TOPBAR_H: i32 = 150;
pub const PAD: i32 = 64;
/// The search button, at the right end of the top bar.
const SEARCH: Rect = Rect { x: W - 180, y: 0, w: 180, h: TOPBAR_H };
/// Touch targets: 96 px ≈ 8 mm at 300 dpi.
pub const ROW_H: i32 = 96;
const ROW_GAP: i32 = 20;
/// Between controls side by side in a row.
const COL_GAP: i32 = 24;
const CONTENTS_H: i32 = 104;
/// The position scrubber under the contents row (drawn by [`crate::nav`]).
const SCRUB_H: i32 = 120;
const SHEET_PAD_TOP: i32 = 44;
const SHEET_PAD_BOTTOM: i32 = 52;
const MAX_FONTS: usize = 9;

/// Text sizes in device pixels (300 dpi: 40 px ≈ 9.6 pt).
pub const SIZES: [f32; 10] = [28.0, 32.0, 36.0, 40.0, 44.0, 48.0, 54.0, 60.0, 68.0, 76.0];
/// Line heights; the menu names them with `Strings::spacings`.
pub const SPACINGS: [f32; 3] = [1.3, 1.45, 1.65];
/// Side margins in pixels; named by `Strings::margin_names`.
pub const MARGINS: [i32; 3] = [56, 96, 150];

#[derive(Debug, Clone, PartialEq)]
pub struct Typography {
    /// Index into [`SIZES`].
    pub size: usize,
    /// Reading font family (lowercase); `None` keeps the publisher's fonts.
    pub family: Option<String>,
    pub spacing: usize,
    pub margin: usize,
    /// Index into [`ALIGNS`].
    pub align: usize,
}

/// Body text alignment choices, in menu order; named by `Strings::aligns`.
pub const ALIGNS: [TextAlign; 3] = [TextAlign::Justify, TextAlign::Start, TextAlign::Center];

impl Default for Typography {
    fn default() -> Self {
        Typography { size: 3, family: Some("bookerly".into()), spacing: 1, margin: 1, align: 0 }
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
                "align" => t.align = index(ALIGNS.len()).unwrap_or(t.align),
                // Before center alignment: justify=false meant left.
                "justify" => t.align = usize::from(v.trim() == "false"),
                "family" => t.family = Some(v.trim().to_lowercase()).filter(|f| !f.is_empty()),
                _ => {}
            }
        }
        t
    }

    pub fn save(&self, state_dir: &Path) {
        let family = self.family.as_deref().unwrap_or("");
        let text = format!(
            "size={}\nfamily={family}\nspacing={}\nmargin={}\nalign={}\n",
            self.size, self.spacing, self.margin, self.align
        );
        if let Err(e) = crate::write_atomic(&Self::path(state_dir), text) {
            eprintln!("typography: {e}");
        }
    }

    pub fn font_size(&self) -> f32 {
        SIZES[self.size]
    }

    pub fn text_align(&self) -> TextAlign {
        ALIGNS[self.align]
    }

    pub fn line_height(&self) -> f32 {
        SPACINGS[self.spacing]
    }

    pub fn margin_x(&self) -> i32 {
        MARGINS[self.margin]
    }
}

/// Something in the menu a tap can hit.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Library,
    Contents,
    Search,
    Smaller,
    Larger,
    /// The previous (false) or next (true) font.
    Font(bool),
    Spacing(usize),
    Margin(usize),
    Align(usize),
}

/// The fonts offered: the publisher's, then the installed reading families.
fn families(ctx: &Ctx) -> Vec<Option<String>> {
    let mut out = vec![None];
    out.extend(ctx.fonts.reading_families().into_iter().map(|f| Some(f.to_string())));
    out.truncate(MAX_FONTS);
    out
}

/// Where everything in the sheet goes.
struct Layout {
    top: i32,
    contents: Rect,
    scrubber: Rect,
    size: Rect,
    font: Rect,
    spacing: Rect,
    margin: Rect,
    align: Rect,
}

impl Layout {
    fn new() -> Layout {
        let height = SHEET_PAD_TOP + CONTENTS_H + ROW_GAP + SCRUB_H + ROW_GAP + 2 * ROW_H + ROW_GAP + SHEET_PAD_BOTTOM;
        let top = H - height;
        let w = W - 2 * PAD;
        // Columns of a row: `n` side by side across the sheet.
        let cols = |y: i32, n: i32| {
            let cw = (w - (n - 1) * COL_GAP) / n;
            (0..n).map(move |i| Rect::new(PAD + i * (cw + COL_GAP), y, cw, ROW_H))
        };
        let mut y = top + SHEET_PAD_TOP;
        let contents = Rect::new(PAD, y, w, CONTENTS_H);
        y += CONTENTS_H + ROW_GAP;
        // Wider than the rows by the knob's radius, so the track lines up with them.
        let scrubber = Rect::new(PAD - 22, y, W - 2 * (PAD - 22), SCRUB_H);
        y += SCRUB_H + ROW_GAP;
        let mut text = cols(y, 2);
        let (size, font) = (text.next().unwrap_or_default(), text.next().unwrap_or_default());
        let mut page = cols(y + ROW_H + ROW_GAP, 3);
        let (spacing, margin, align) =
            (page.next().unwrap_or_default(), page.next().unwrap_or_default(), page.next().unwrap_or_default());
        Layout { top, contents, scrubber, size, font, spacing, margin, align }
    }
}

/// Every control with its rectangle, top to bottom.
pub fn controls() -> Vec<(Control, Rect)> {
    let l = Layout::new();
    let (size, font) = (segments(l.size, 3), segments(l.font, 3));
    let mut out = vec![
        (Control::Library, Rect::new(0, 0, 360, TOPBAR_H)),
        (Control::Search, SEARCH),
        (Control::Contents, l.contents),
        (Control::Smaller, size[0]),
        (Control::Larger, size[2]),
        (Control::Font(false), font[0]),
        // The name steps forward too: the bigger target for the common move.
        (Control::Font(true), font[1].union(&font[2])),
    ];
    out.extend(segments(l.spacing, SPACINGS.len()).into_iter().enumerate().map(|(i, r)| (Control::Spacing(i), r)));
    out.extend(segments(l.margin, MARGINS.len()).into_iter().enumerate().map(|(i, r)| (Control::Margin(i), r)));
    out.extend(segments(l.align, ALIGNS.len()).into_iter().enumerate().map(|(i, r)| (Control::Align(i), r)));
    out
}

/// Where the position scrubber goes in the sheet.
pub fn scrubber() -> Rect {
    Layout::new().scrubber
}

/// Whether a tap at height `y` lands on the menu's chrome (top bar or sheet).
pub fn covers(y: i32) -> bool {
    y < TOPBAR_H || y >= Layout::new().top
}

/// Draws the menu over the page. `chapter` is the current table-of-contents entry, `None`
/// when the book has no table of contents.
pub fn render(c: &mut Canvas, ctx: &mut Ctx, typo: &Typography, title: &str, chapter: Option<&str>) {
    let l = Layout::new();
    let Ctx { fonts, raster, .. } = ctx;
    let (Some(sans), Some(sans_bold), Some(serif_bold)) =
        (crate::sans(fonts, false), crate::sans(fonts, true), crate::serif(fonts, true))
    else {
        return;
    };

    topbar(c, raster, sans, serif_bold, t().back_library, title);
    // ponytail: also drawn over PDFs, where search is not wired; MuPDF can search page text.
    let icon = Rect::new(SEARCH.x + (SEARCH.w - 52) / 2, SEARCH.y + (SEARCH.h - 52) / 2 + 4, 52, 52);
    crate::search::icon(c, icon, INK, PAPER);

    // Sheet: rounded top corners over a black edge, like the quick panel.
    rounded_rect(c, Rect::new(0, l.top - 3, W, H - l.top + 40), 32, INK);
    rounded_rect(c, Rect::new(0, l.top, W, H - l.top + 40), 30, PAPER);

    // Contents: a full-width row naming the current chapter.
    let r = l.contents;
    button(c, r, false);
    let ink = if chapter.is_some() { INK } else { INK_SOFT };
    let head = Label { face: sans_bold, size: 32.0, ink };
    let mut x = r.x as f32 + 36.0;
    x += head.draw(c, raster, t().contents, x, head.baseline(r)) + 32.0;
    let chevron = Label { face: sans_bold, size: 44.0, ink };
    let chevron_x = (r.right() - 36) as f32 - chevron.width("›");
    chevron.draw(c, raster, "›", chevron_x, chevron.baseline(r));
    let sub = Label { face: sans, size: 30.0, ink: INK_SOFT };
    let text = sub.fit(chapter.unwrap_or(t().no_contents), chevron_x - 32.0 - x);
    sub.draw(c, raster, &text, x, sub.baseline(r));

    // Size: a stepper, the current step between a small and a large "A".
    let segs = segmented(c, l.size, 3, None);
    // A small "A−" and a large "A+": the sign says which way, the size shows it. The signs
    // are drawn, not set, so they match whatever the sans font has.
    for (r, size, plus) in [(segs[0], 30.0, false), (segs[2], 46.0, true)] {
        let a = Label { face: sans_bold, size, ink: INK };
        let (arm, gap) = ((size * 0.36) as i32, 10);
        let x = r.x as f32 + (r.w as f32 - a.width("A") - (gap + 2 * arm) as f32) / 2.0;
        let sign_x = (x + a.draw(c, raster, "A", x, a.baseline(r))) as i32 + gap;
        let cy = r.y + r.h / 2;
        c.fill(Rect::new(sign_x, cy - 2, 2 * arm, 4), INK);
        if plus {
            c.fill(Rect::new(sign_x + arm - 2, cy - arm, 4, 2 * arm), INK);
        }
    }
    let value = format!("{} / {}", typo.size + 1, SIZES.len());
    Label { face: sans, size: 32.0, ink: INK }.centered(c, raster, &value, segs[1]);

    // Font: ‹ name ›, the name set in its own face.
    let segs = segmented(c, l.font, 3, None);
    let chevron = Label { face: sans_bold, size: 40.0, ink: INK };
    chevron.centered(c, raster, "‹", segs[0]);
    chevron.centered(c, raster, "›", segs[2]);
    let f = &typo.family;
    let face = f.as_ref().and_then(|f| fonts.pick(std::slice::from_ref(f), 400, false)).unwrap_or(sans);
    let name = f.as_deref().map_or(t().original_font.into(), title_case);
    Label { face, size: 30.0, ink: INK }.centered(c, raster, &name, segs[1]);

    // Layout choices as pictures of the page: what changes is visible without reading.
    for (kind, r, on) in [
        (Icon::Spacing, l.spacing, typo.spacing),
        (Icon::Margin, l.margin, typo.margin),
        (Icon::Align, l.align, typo.align),
    ] {
        for (i, s) in segmented(c, r, 3, Some(on)).into_iter().enumerate() {
            layout_icon(c, kind, i, s, if i == on { PAPER } else { INK });
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Spacing,
    Margin,
    Align,
}

/// Choice `i` (narrow to wide, or justified/left/centred) of `kind`, centred in `r`.
fn layout_icon(c: &mut Canvas, kind: Icon, i: usize, r: Rect, ink: u8) {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    let i = i as i32;
    let bar = |c: &mut Canvas, x: i32, y: i32, w: i32| c.fill(Rect::new(x, y - 2, w, 4), ink);
    match kind {
        // Three lines, further apart.
        Icon::Spacing => {
            let gap = 10 + 5 * i;
            for k in -1..=1 {
                bar(c, cx - 20, cy + k * gap, 40);
            }
        }
        // A page outline, its text column narrower.
        Icon::Margin => {
            let (w, h) = (46, 50);
            let page = Rect::new(cx - w / 2, cy - h / 2, w, h);
            for edge in [
                Rect::new(page.x, page.y, w, 3),
                Rect::new(page.x, page.bottom() - 3, w, 3),
                Rect::new(page.x, page.y, 3, h),
                Rect::new(page.right() - 3, page.y, 3, h),
            ] {
                c.fill(edge, ink);
            }
            let inset = 8 + 5 * i;
            for k in -1..=1 {
                bar(c, page.x + inset, cy + k * 11, w - 2 * inset);
            }
        }
        // Four lines: all full, ragged right, or centred.
        Icon::Align => {
            for (k, w) in [40, 28, 36, 22].into_iter().enumerate() {
                let y = cy - 15 + k as i32 * 10;
                match i {
                    0 => bar(c, cx - 20, y, 40),
                    1 => bar(c, cx - 20, y, w),
                    _ => bar(c, cx - w / 2, y, w),
                }
            }
        }
    }
}

/// The menu's top bar: a back link on the left and a centred title, over a black rule.
pub fn topbar(c: &mut Canvas, raster: &mut Rasterizer, sans: &Face, serif_bold: &Face, back: &str, title: &str) {
    c.fill(Rect::new(0, 0, W, TOPBAR_H), PAPER);
    c.fill(Rect::new(0, TOPBAR_H - 3, W, 3), INK);
    let b = Label { face: sans, size: 32.0, ink: INK };
    b.draw(c, raster, back, PAD as f32, 92);
    let t = Label { face: serif_bold, size: 34.0, ink: INK };
    let text = t.fit(title, (W - 2 * 380) as f32);
    t.draw_centred(c, raster, &text, W as f32 / 2.0, 92);
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
pub fn apply(typo: &mut Typography, ctl: &Control, ctx: &Ctx) -> bool {
    let before = typo.clone();
    match ctl {
        Control::Font(next) => {
            let all = families(ctx);
            let i = all.iter().position(|f| *f == typo.family).unwrap_or(0);
            let step = if *next { 1 } else { all.len().saturating_sub(1) };
            typo.family = all.get((i + step) % all.len().max(1)).cloned().flatten();
        }
        Control::Smaller => typo.size = typo.size.saturating_sub(1),
        Control::Larger => typo.size = (typo.size + 1).min(SIZES.len() - 1),
        Control::Spacing(i) => typo.spacing = *i,
        Control::Margin(i) => typo.margin = *i,
        Control::Align(i) => typo.align = *i,
        Control::Library | Control::Contents | Control::Search => {}
    }
    *typo != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typography_round_trips_and_clamps() {
        let dir = std::env::temp_dir().join(format!("zarathustra-typo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let t = Typography { size: 5, family: None, spacing: 2, margin: 0, align: 2 };
        t.save(&dir);
        assert_eq!(Typography::load(&dir), t);
        std::fs::write(dir.join("typography.txt"), "size=99\nfamily=Baskerville\n").expect("write");
        let t = Typography::load(&dir);
        assert_eq!((t.size, t.family.as_deref()), (3, Some("baskerville")));
        std::fs::remove_dir_all(dir).ok();

        let ctx = Ctx::new();
        let mut t = Typography { size: SIZES.len() - 1, ..Typography::default() };
        assert!(!apply(&mut t, &Control::Larger, &ctx));
        assert!(apply(&mut t, &Control::Smaller, &ctx));
        // The font steps round the list both ways, back to where it started.
        let start = t.family.clone();
        if families(&ctx).len() > 1 {
            assert!(apply(&mut t, &Control::Font(false), &ctx), "wraps to the last");
            assert!(apply(&mut t, &Control::Font(true), &ctx));
            assert_eq!(t.family, start);
        }
        assert_eq!(title_case("amazon ember"), "Amazon Ember");
    }
}
