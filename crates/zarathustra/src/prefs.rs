//! The settings screen, opened from the gear in the home header: the language, how often a
//! page turn flashes the screen clean, and what the home's first page shows. Each is a row of
//! choices, all in view, one tap. Under them, the installed plugins, each opening its page. Light, warmth, clock and page animation stay in the quick
//! panel, which owns the settings: a change here goes back to it as [`Action::Settings`].

use zarathustra_device::{Canvas, Rect};

use crate::i18n::{t, Lang};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::plugin::{discover, Dirs, Plugin};
use crate::plugin_page::PluginPage;
use crate::settings::Settings;
use crate::ui::{segments, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Action, Ctx, H, W};

/// A row: its name, then the choices under it.
const ROW_H: i32 = 210;
const CONTROL_H: i32 = 96;
const BACK_W: i32 = 360;
/// The choices for a full refresh, in page turns.
const FLASHES: [u8; 4] = [4, 8, 16, 32];
const ROWS: usize = 3;
/// A plugin in the list: its name, its description under it.
const PLUGIN_H: i32 = 130;

/// Where the plugin list starts: its heading, then one row per plugin.
fn plugins_top() -> i32 {
    TOPBAR_H + 40 + ROWS as i32 * ROW_H + 10
}

fn plugin_row(i: usize) -> Rect {
    Rect::new(PAD, plugins_top() + 70 + i as i32 * PLUGIN_H, W - 2 * PAD, PLUGIN_H)
}

/// The choices of row `i`.
fn control(i: usize) -> Rect {
    Rect::new(PAD, TOPBAR_H + 40 + i as i32 * ROW_H + 74, W - 2 * PAD, CONTROL_H)
}

pub struct Prefs {
    settings: Settings,
    plugins: Vec<Plugin>,
    /// The plugin page open over the settings.
    page: Option<Box<PluginPage>>,
}

impl Prefs {
    pub fn new(settings: Settings, ctx: &Ctx) -> Prefs {
        Prefs { settings, plugins: discover(&Dirs::new(&ctx.state_dir)), page: None }
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
        if let Some(page) = &mut self.page {
            if !page.tap((x, y)) {
                self.page = None;
            }
            return Some(Action::Redraw { turn: None });
        }
        if y < TOPBAR_H {
            return (x >= BACK_W).then_some(Action::None);
        }
        if let Some(i) = (0..self.plugins.len()).find(|&i| plugin_row(i).contains(x, y)) {
            self.page = Some(Box::new(PluginPage::open(self.plugins[i].clone(), ctx)));
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

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(page) = &self.page {
            return page.render(c, ctx);
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
        for i in 0..ROWS {
            let (name, choices, on) = self.row(i);
            let r = control(i);
            label.draw(c, raster, name, PAD as f32, r.y - 26);
            crate::ui::segmented_labels(c, raster, sans, 28.0, r, &choices, Some(on));
            c.fill(Rect::new(PAD, r.bottom() + 40, W - 2 * PAD, 2), LINE);
        }
        if !self.plugins.is_empty() {
            label.draw(c, raster, t().plugins, PAD as f32, plugins_top() + 48);
        }
        let soft = Label { face: sans, size: 26.0, ink: INK_SOFT };
        let state = Label { face: sans, size: 28.0, ink: INK };
        for (i, p) in self.plugins.iter().enumerate() {
            let r = plugin_row(i);
            if r.bottom() > H - 100 {
                break; // ponytail: no scrolling; a dozen plugins fit
            }
            let status = match &p.manifest {
                Err(_) => t().plugin_broken,
                Ok(m) if m.service.is_some() && p.is_running() => t().service_on,
                Ok(m) if m.service.is_some() => t().service_off,
                Ok(_) => "",
            };
            let status = format!("{status}  ›");
            let w = state.width(&status);
            state.draw(c, raster, &status, r.right() as f32 - w, r.y + 52);
            let text_w = (r.w - w as i32 - 40) as f32;
            Label { face: serif, size: 34.0, ink: INK }.draw(
                c,
                raster,
                &label.fit(p.name(), text_w),
                r.x as f32,
                r.y + 52,
            );
            let about = p.manifest.as_ref().map_or_else(|e| e.clone(), |m| m.description.clone());
            soft.draw(c, raster, &soft.fit(&about, text_w), r.x as f32, r.y + 96);
            c.fill(Rect::new(r.x, r.bottom() - 2, r.w, 2), LINE);
        }
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
        // The bundled SSH plugin opens its page; its back link returns here.
        let ssh = p.plugins.iter().position(|pl| pl.id == "ssh").expect("ssh plugin listed");
        let r = plugin_row(ssh);
        assert!(p.tap((r.x + 10, r.y + 10), &ctx).is_some() && p.page.is_some());
        p.tap((100, 60), &ctx);
        assert!(p.page.is_none());
    }
}
