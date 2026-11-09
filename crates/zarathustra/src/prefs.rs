//! The settings screen, opened from the gear in the home header: the device information row
//! at the top ([`DeviceInfo`]), then the language, how often a
//! page turn flashes the screen clean, and what the home's first page shows. Each is a row of
//! choices, all in view, one tap. Under them, the Extensions row opens the plugins' screen. Light, warmth, clock and page animation stay in the quick
//! panel, which owns the settings: a change here goes back to it as [`Action::Settings`].

use zarathustra_device::{Canvas, Rect};

use crate::device_info::DeviceInfo;
use crate::extensions::Extensions;
use crate::i18n::{t, Lang};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::settings::Settings;
use crate::toc::BACK_W;
use crate::ui::{segments, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, H, W};

/// A row: its name, then the choices under it.
const ROW_H: i32 = 210;
const CONTROL_H: i32 = 96;
/// The choices for a full refresh, in page turns.
const FLASHES: [u8; 4] = [4, 8, 16, 32];
const ROWS: usize = 3;
/// The device information row, at the top.
const DEVICE_ROW: Rect = Rect { x: PAD, y: TOPBAR_H + 10, w: W - 2 * PAD, h: 120 };
/// Where the rows of choices start, under it.
const ROWS_TOP: i32 = DEVICE_ROW.y + DEVICE_ROW.h + 30;

/// The Extensions row, under the choices.
fn extensions_row() -> Rect {
    Rect::new(PAD, ROWS_TOP + ROWS as i32 * ROW_H + 10, W - 2 * PAD, 120)
}

/// The choices of row `i`.
fn control(i: usize) -> Rect {
    Rect::new(PAD, ROWS_TOP + i as i32 * ROW_H + 74, W - 2 * PAD, CONTROL_H)
}

pub struct Prefs {
    settings: Settings,
    /// How many plugins are installed, shown on the Extensions row.
    plugins: usize,
    /// The Extensions screen, open over the settings.
    extensions: Option<Box<Extensions>>,
    /// The device information, open over the settings.
    device: Option<DeviceInfo>,
}

impl Prefs {
    pub fn new(settings: Settings, ctx: &Ctx) -> Prefs {
        Prefs { settings, plugins: Extensions::count(ctx), extensions: None, device: None }
    }

    /// Row `i`: its name, its choices and which one is set.
    fn row(&self, i: usize) -> (&'static str, Vec<String>, usize) {
        let (s, v) = (t(), &self.settings);
        match i {
            0 => (
                s.language,
                vec![
                    crate::i18n::strings(Lang::It).language_name.into(),
                    crate::i18n::strings(Lang::En).language_name.into(),
                ],
                usize::from(v.lang == Lang::En),
            ),
            1 => {
                let on = FLASHES.iter().position(|&f| f == v.flash_every).unwrap_or(1);
                (s.full_refresh, FLASHES.iter().map(|f| s.every_pages.replace("{}", &f.to_string())).collect(), on)
            }
            _ => (s.first_page, vec![s.quote.into(), s.reading_figures.into()], usize::from(!v.home_quotes)),
        }
    }

    /// `None` for the back button; otherwise the action (a changed setting, or nothing).
    pub fn tap(&mut self, (x, y): (i32, i32), ctx: &Ctx) -> Option<Action> {
        if let Some(ext) = &mut self.extensions {
            if !ext.tap((x, y), ctx) {
                self.extensions = None;
            }
            return Some(Action::Redraw { turn: None });
        }
        if let Some(d) = &self.device {
            if d.back((x, y)) {
                self.device = None;
                return Some(Action::Redraw { turn: None });
            }
            return Some(Action::None);
        }
        if y < TOPBAR_H {
            return (x >= BACK_W).then_some(Action::None);
        }
        if DEVICE_ROW.contains(x, y) {
            self.device = Some(DeviceInfo::new(ctx));
            return Some(Action::Redraw { turn: None });
        }
        if extensions_row().contains(x, y) {
            self.extensions = Some(Box::new(Extensions::new(ctx, t().back_settings)));
            return Some(Action::Redraw { turn: None });
        }
        let hit = (0..ROWS).find_map(|i| {
            let n = self.row(i).1.len();
            segments(control(i), n).iter().position(|r| r.contains(x, y)).map(|c| (i, c))
        });
        let s = &mut self.settings;
        match hit {
            Some((0, c)) => s.lang = if c == 0 { Lang::It } else { Lang::En },
            Some((1, c)) => s.flash_every = FLASHES[c],
            Some((_, c)) => s.home_quotes = c == 0,
            None => return Some(Action::None),
        }
        Some(Action::Settings(self.settings))
    }

    /// Some plugin action shown is running: the app polls every half second.
    pub fn actions_running(&self) -> bool {
        self.extensions.as_ref().is_some_and(|e| e.running())
    }

    /// Looks again at running actions; `true` when the screen must be redrawn.
    pub fn poll(&mut self) -> bool {
        self.extensions.as_mut().is_some_and(|e| e.poll())
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(ext) = &self.extensions {
            return ext.render(c, ctx);
        }
        if let Some(d) = &self.device {
            return d.render(c, ctx);
        }
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        topbar(c, raster, sans, serif_bold, t().back_library, t().settings);
        let label = Label { face: serif, size: 36.0, ink: INK };
        let r = DEVICE_ROW;
        label.draw(c, raster, t().device_info, PAD as f32, r.y + r.h / 2 + 12);
        let chevron = Label { face: sans, size: 28.0, ink: INK_SOFT };
        chevron.draw(c, raster, "›", r.right() as f32 - chevron.width("›"), r.y + r.h / 2 + 12);
        c.fill(Rect::new(PAD, r.bottom(), W - 2 * PAD, 2), LINE);
        for i in 0..ROWS {
            let (name, choices, on) = self.row(i);
            let r = control(i);
            label.draw(c, raster, name, PAD as f32, r.y - 26);
            crate::ui::segmented_labels(c, raster, sans, 28.0, r, &choices, Some(on));
            c.fill(Rect::new(PAD, r.bottom() + 40, W - 2 * PAD, 2), LINE);
        }
        let r = extensions_row();
        let base = r.y + r.h / 2 + 12;
        label.draw(c, raster, t().extensions, PAD as f32, base);
        let count = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let n = format!("{}   ›", crate::i18n::fill(t().extensions_installed, &[&self.plugins.to_string()]));
        count.draw(c, raster, &n, r.right() as f32 - count.width(&n), base);
        c.fill(Rect::new(PAD, r.bottom(), W - 2 * PAD, 2), LINE);
        let version = format!("Zarathustra {}", env!("CARGO_PKG_VERSION"));
        Label { face: sans, size: 28.0, ink: INK_SOFT }.draw(c, raster, &version, PAD as f32, H - 60);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_picks_that_choice() {
        let ctx = Ctx::new();
        let mut p = Prefs::new(Settings::default(), &ctx);
        let pick = |p: &mut Prefs, row: usize, k: usize, n: usize| {
            let s = segments(control(row), n)[k];
            p.tap((s.x + s.w / 2, s.y + s.h / 2), &ctx)
        };
        assert!(matches!(pick(&mut p, 0, 1, 2), Some(Action::Settings(s)) if s.lang == Lang::En));
        assert!(matches!(pick(&mut p, 1, 3, 4), Some(Action::Settings(s)) if s.flash_every == 32));
        assert!(matches!(pick(&mut p, 2, 1, 2), Some(Action::Settings(s)) if !s.home_quotes));
        assert!(p.tap((100, 60), &ctx).is_none(), "back");
        // The Extensions row opens its screen; its back link returns here.
        let r = extensions_row();
        assert!(p.tap((r.x + 10, r.y + 10), &ctx).is_some() && p.extensions.is_some());
        p.tap((100, 60), &ctx);
        assert!(p.extensions.is_none());
        // So does the device information row, at the top.
        let r = DEVICE_ROW;
        assert!(p.tap((r.x + 10, r.y + 10), &ctx).is_some() && p.device.is_some());
        p.tap((100, 60), &ctx);
        assert!(p.device.is_none());
    }
}
