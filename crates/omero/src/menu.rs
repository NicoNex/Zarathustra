//! The reader menu: a top bar (back to the library, book title) and a bottom sheet with the
//! table of contents entry, the position scrubber and the typography settings, grouped as Text (size, font) and
//! Layout (spacing, margins, alignment). The settings persist across books and sessions.

use std::path::Path;

use omero_device::{Canvas, Rect};
use omero_engine::font::{Face, Rasterizer};
use omero_engine::style::css::TextAlign;

use crate::i18n::t;
use crate::ui::{button, rounded_rect, segmented, segments, Label, INK, INK_SOFT, PAPER};
use crate::{Ctx, H, W};

pub const TOPBAR_H: i32 = 150;
pub const PAD: i32 = 64;
/// The search button, at the right end of the top bar.
const SEARCH: Rect = Rect { x: W - 180, y: 0, w: 180, h: TOPBAR_H };
/// Width of the row-label column left of the controls.
const LABEL_W: i32 = 230;
/// Touch targets: 96 px ≈ 8 mm at 300 dpi.
pub const ROW_H: i32 = 96;
const ROW_GAP: i32 = 20;
const CHIP_GAP: i32 = 16;
const SECTION_GAP: i32 = 40;
const HEADER_H: i32 = 60;
const CONTENTS_H: i32 = 104;
/// The position scrubber under the contents row (drawn by [`crate::nav`]).
const SCRUB_H: i32 = 120;
const SHEET_PAD_TOP: i32 = 44;
const SHEET_PAD_BOTTOM: i32 = 52;
const FONT_COLS: usize = 3;
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
    Family(Option<String>),
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
    /// Section headers: text and the band they sit in.
    headers: [(&'static str, Rect); 2],
    /// Row labels with the row they name.
    labels: [(&'static str, Rect); 5],
    size: Rect,
    fonts: Vec<(Option<String>, Rect)>,
    spacing: Rect,
    margin: Rect,
    align: Rect,
}

impl Layout {
    fn new(fonts: Vec<Option<String>>) -> Layout {
        let font_rows = fonts.len().div_ceil(FONT_COLS).max(1) as i32;
        let fonts_h = font_rows * ROW_H + (font_rows - 1) * CHIP_GAP;
        let height = SHEET_PAD_TOP
            + CONTENTS_H
            + ROW_GAP
            + SCRUB_H
            + 2 * (SECTION_GAP + HEADER_H)
            + ROW_H
            + ROW_GAP
            + fonts_h
            + 3 * ROW_H
            + 2 * ROW_GAP
            + SHEET_PAD_BOTTOM;
        let top = H - height;
        let (x, w) = (PAD + LABEL_W, W - 2 * PAD - LABEL_W);
        let row = |y: i32| Rect::new(x, y, w, ROW_H);

        let mut y = top + SHEET_PAD_TOP;
        let contents = Rect::new(PAD, y, W - 2 * PAD, CONTENTS_H);
        y += CONTENTS_H + ROW_GAP;
        // Wider than the rows by the knob's radius, so the track lines up with them.
        let scrubber = Rect::new(PAD - 22, y, W - 2 * (PAD - 22), SCRUB_H);
        y += SCRUB_H + SECTION_GAP;
        let text_header = Rect::new(PAD, y, W - 2 * PAD, HEADER_H);
        y += HEADER_H;
        let size = row(y);
        y += ROW_H + ROW_GAP;
        let cell = (w - CHIP_GAP * (FONT_COLS as i32 - 1)) / FONT_COLS as i32;
        let font_top = y;
        let fonts = (fonts.into_iter().enumerate())
            .map(|(i, f)| {
                let (r, c) = ((i / FONT_COLS) as i32, (i % FONT_COLS) as i32);
                (f, Rect::new(x + c * (cell + CHIP_GAP), y + r * (ROW_H + CHIP_GAP), cell, ROW_H))
            })
            .collect();
        y += fonts_h + SECTION_GAP;
        let layout_header = Rect::new(PAD, y, W - 2 * PAD, HEADER_H);
        y += HEADER_H;
        let spacing = row(y);
        let margin = row(y + ROW_H + ROW_GAP);
        let align = row(y + 2 * (ROW_H + ROW_GAP));
        Layout {
            top,
            contents,
            scrubber,
            headers: [(t().text, text_header), (t().layout, layout_header)],
            labels: [
                (t().size, size),
                (t().font, Rect::new(x, font_top, w, ROW_H)),
                (t().spacing, spacing),
                (t().margins, margin),
                (t().alignment, align),
            ],
            size,
            fonts,
            spacing,
            margin,
            align,
        }
    }
}

/// Every control with its rectangle, top to bottom.
pub fn controls(ctx: &Ctx) -> Vec<(Control, Rect)> {
    let l = Layout::new(families(ctx));
    let size = segments(l.size, 3);
    let mut out = vec![
        (Control::Library, Rect::new(0, 0, 360, TOPBAR_H)),
        (Control::Search, SEARCH),
        (Control::Contents, l.contents),
        (Control::Smaller, size[0]),
        (Control::Larger, size[2]),
    ];
    out.extend(l.fonts.into_iter().map(|(f, r)| (Control::Family(f), r)));
    out.extend(segments(l.spacing, SPACINGS.len()).into_iter().enumerate().map(|(i, r)| (Control::Spacing(i), r)));
    out.extend(segments(l.margin, MARGINS.len()).into_iter().enumerate().map(|(i, r)| (Control::Margin(i), r)));
    out.extend(segments(l.align, ALIGNS.len()).into_iter().enumerate().map(|(i, r)| (Control::Align(i), r)));
    out
}

/// Where the position scrubber goes in the sheet.
pub fn scrubber(ctx: &Ctx) -> Rect {
    Layout::new(families(ctx)).scrubber
}

/// Whether a tap at height `y` lands on the menu's chrome (top bar or sheet).
pub fn covers(ctx: &Ctx, y: i32) -> bool {
    y < TOPBAR_H || y >= Layout::new(families(ctx)).top
}

/// Draws the menu over the page. `chapter` is the current table-of-contents entry, `None`
/// when the book has no table of contents.
pub fn render(c: &mut Canvas, ctx: &mut Ctx, typo: &Typography, title: &str, chapter: Option<&str>) {
    let l = Layout::new(families(ctx));
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

    let header = Label { face: sans_bold, size: 30.0, ink: INK };
    for (text, r) in l.headers {
        header.draw(c, raster, text, r.x as f32, r.y + 40);
    }
    let label = Label { face: sans, size: 30.0, ink: INK_SOFT };
    for (text, r) in l.labels {
        label.draw(c, raster, text, PAD as f32, label.baseline(r));
    }

    // Size: a stepper, the current step between a small and a large "A".
    let segs = segmented(c, l.size, 3, None);
    Label { face: sans_bold, size: 30.0, ink: INK }.centered(c, raster, "A", segs[0]);
    Label { face: sans_bold, size: 46.0, ink: INK }.centered(c, raster, "A", segs[2]);
    let value = format!("{} / {}", typo.size + 1, SIZES.len());
    Label { face: sans, size: 32.0, ink: INK }.centered(c, raster, &value, segs[1]);

    // Fonts: chips, each name set in its own face.
    for (f, r) in &l.fonts {
        let on = *f == typo.family;
        button(c, *r, on);
        let face = f.as_ref().and_then(|f| fonts.pick(std::slice::from_ref(f), 400, false)).unwrap_or(sans);
        let name = f.as_deref().map_or(t().original_font.into(), title_case);
        Label { face, size: 32.0, ink: if on { PAPER } else { INK } }.centered(c, raster, &name, *r);
    }

    let mut choice = |c: &mut Canvas, r: Rect, names: &[&str], on: usize| {
        let segs = segmented(c, r, names.len(), Some(on));
        for (i, (name, s)) in names.iter().zip(segs).enumerate() {
            Label { face: sans, size: 30.0, ink: if i == on { PAPER } else { INK } }.centered(c, raster, name, s);
        }
    };
    choice(c, l.spacing, &t().spacings, typo.spacing);
    choice(c, l.margin, &t().margin_names, typo.margin);
    choice(c, l.align, &t().aligns, typo.align);
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
pub fn apply(typo: &mut Typography, ctl: &Control) -> bool {
    let before = typo.clone();
    match ctl {
        Control::Smaller => typo.size = typo.size.saturating_sub(1),
        Control::Larger => typo.size = (typo.size + 1).min(SIZES.len() - 1),
        Control::Family(f) => typo.family = f.clone(),
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
        let dir = std::env::temp_dir().join(format!("omero-typo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let t = Typography { size: 5, family: None, spacing: 2, margin: 0, align: 2 };
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
