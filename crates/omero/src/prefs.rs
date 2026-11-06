//! The settings screen, opened from the gear in the home header: the language and how often
//! a page turn flashes the screen clean. Light, warmth, clock and page animation stay in the
//! quick panel, which owns the settings: a change here goes back to it as
//! [`Action::Settings`].

use omero_device::{Canvas, Rect};

use crate::i18n::{t, Lang};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::settings::Settings;
use crate::ui::{Label, INK, INK_SOFT, LINE};
use crate::{Action, Ctx, W};

const ROW_H: i32 = 140;
const BACK_W: i32 = 360;
/// The choices for a full refresh, in page turns.
const FLASHES: [u8; 4] = [4, 8, 16, 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Lang,
    Flash,
}

const ROWS: [Row; 2] = [Row::Lang, Row::Flash];

fn row_rect(i: usize) -> Rect {
    Rect::new(0, TOPBAR_H + 40 + i as i32 * ROW_H, W, ROW_H)
}

pub struct Prefs {
    settings: Settings,
}

impl Prefs {
    pub fn new(settings: Settings) -> Prefs {
        Prefs { settings }
    }

    /// `None` for the back button; otherwise the action (a changed setting, or nothing).
    pub fn tap(&mut self, (x, y): (i32, i32)) -> Option<Action> {
        if y < TOPBAR_H {
            return (x >= BACK_W).then_some(Action::None);
        }
        let s = &mut self.settings;
        match (0..ROWS.len()).find(|&i| row_rect(i).contains(x, y)).map(|i| ROWS[i]) {
            Some(Row::Lang) => s.lang = if s.lang == Lang::It { Lang::En } else { Lang::It },
            Some(Row::Flash) => {
                let next = FLASHES.iter().position(|&f| f == s.flash_every).map_or(1, |i| (i + 1) % FLASHES.len());
                s.flash_every = FLASHES[next];
            }
            None => return Some(Action::None),
        }
        Some(Action::Settings(self.settings))
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), crate::ui::PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_library, s.settings);
        let label = Label { face: serif, size: 38.0, ink: INK };
        let value = Label { face: sans, size: 30.0, ink: INK_SOFT };
        for (i, row) in ROWS.iter().enumerate() {
            let r = row_rect(i);
            c.fill(Rect::new(PAD, r.bottom() - 2, W - 2 * PAD, 2), LINE);
            let (name, v) = match row {
                Row::Lang => (s.language, s.language_name.to_string()),
                Row::Flash => (s.full_refresh, s.every_pages.replace("{}", &self.settings.flash_every.to_string())),
            };
            let base = r.y + r.h / 2 + 14;
            label.draw(c, raster, name, PAD as f32, base);
            value.draw(c, raster, &v, W as f32 - PAD as f32 - value.width(&v), base);
        }
        let version = format!("Omero {}", env!("CARGO_PKG_VERSION"));
        value.draw(c, raster, &version, PAD as f32, row_rect(ROWS.len()).y + 80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cycle_their_values() {
        let mut p = Prefs::new(Settings::default());
        let mid = |i: usize| (W / 2, row_rect(i).y + ROW_H / 2);
        assert!(matches!(p.tap(mid(0)), Some(Action::Settings(s)) if s.lang == Lang::En));
        assert!(matches!(p.tap(mid(1)), Some(Action::Settings(s)) if s.flash_every == 16));
        p.tap(mid(1));
        assert!(matches!(p.tap(mid(1)), Some(Action::Settings(s)) if s.flash_every == 4), "wraps");
        assert!(p.tap((100, 60)).is_none(), "back");
    }
}
