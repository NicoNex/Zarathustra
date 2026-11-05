//! Reading statistics: figures computed from the per-day rows in the store, the global
//! statistics screen, and the figure row the book's details screen shares.

use std::path::Path;

use omero_device::{Canvas, Rect};
use omero_engine::font::{Face, Rasterizer};
use omero_store::{BookId, Reading, Store};

use crate::i18n::t;
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::ui::{rounded_rect, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

/// Days in the bar chart.
const CHART_DAYS: i64 = 14;
const FIGURE_H: i32 = 150;

/// Per-day reading of one book (`book`) or all of them (`None`), oldest first. Empty when
/// the database can't be read.
pub fn days(state_dir: &Path, book: Option<&Path>) -> Vec<Reading> {
    let found = Store::open(&state_dir.join("omero.db")).and_then(|s| {
        let id: Option<BookId> = match book {
            Some(path) => match s.book_by_path(path)? {
                Some(id) => Some(id),
                None => return Ok(Vec::new()),
            },
            None => None,
        };
        s.reading_days(id)
    });
    found.map_err(|e| eprintln!("stats: {e}")).unwrap_or_default()
}

/// Totals over `days`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub seconds: i64,
    pub pages: i64,
    pub days: usize,
    /// Seconds on `today` and over the last seven days (today included).
    pub today: i64,
    pub week: i64,
    /// Consecutive days with reading ending today, or yesterday when today has none yet.
    pub streak: u32,
    pub last_day: Option<i64>,
}

pub fn summary(days: &[Reading], today: i64) -> Summary {
    let on = |d: i64| days.iter().find(|r| r.day == d).map_or(0, |r| r.seconds);
    let mut streak = 0;
    let mut d = if on(today) > 0 { today } else { today - 1 };
    while on(d) > 0 {
        streak += 1;
        d -= 1;
    }
    Summary {
        seconds: days.iter().map(|r| r.seconds).sum(),
        pages: days.iter().map(|r| r.pages).sum(),
        days: days.iter().filter(|r| r.seconds > 0).count(),
        today: on(today),
        week: (today - 6..=today).map(on).sum(),
        streak,
        last_day: days.last().map(|r| r.day),
    }
}

/// "2 h 05 min", "45 min", "< 1 min".
pub fn duration(seconds: i64) -> String {
    let minutes = seconds / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) if seconds > 0 => "< 1 min".into(),
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} h {m:02} min"),
    }
}

/// A row of big figures with small labels under them, evenly spread over `r`.
pub fn figures(c: &mut Canvas, raster: &mut Rasterizer, faces: (&Face, &Face), r: Rect, items: &[(String, &str)]) {
    let (serif_bold, sans) = faces;
    let n = items.len().max(1) as i32;
    let w = r.w / n;
    let value = Label { face: serif_bold, size: 56.0, ink: INK };
    let label = Label { face: sans, size: 26.0, ink: INK_SOFT };
    for (i, (v, l)) in items.iter().enumerate() {
        let x = (r.x + i as i32 * w) as f32;
        value.draw(c, raster, &value.fit(v, (w - 24) as f32), x, r.y + 64);
        label.draw(c, raster, &label.fit(l, (w - 24) as f32), x, r.y + 110);
        if i > 0 {
            c.fill(Rect::new(r.x + i as i32 * w - 28, r.y + 12, 2, 106), LINE);
        }
    }
}

/// The global statistics screen.
pub struct StatsScreen {
    days: Vec<Reading>,
    finished: usize,
}

impl StatsScreen {
    pub fn new(state_dir: &Path, finished: usize) -> StatsScreen {
        StatsScreen { days: days(state_dir, None), finished }
    }

    /// Only the back button does anything.
    pub fn back(&self, (x, y): (i32, i32)) -> bool {
        y < TOPBAR_H && x < 400
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif_bold)) = (crate::sans(fonts, false), crate::serif(fonts, true)) else { return };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_library, s.stats);
        let today = crate::clock::local_now().0;
        let sum = summary(&self.days, today);
        let wide = Rect::new(PAD, TOPBAR_H + 40, W - 2 * PAD, FIGURE_H);
        let streak = sum.streak.to_string();
        figures(
            c,
            raster,
            (serif_bold, sans),
            wide,
            &[(duration(sum.today), s.today), (duration(sum.week), s.this_week), (streak, s.streak)],
        );

        // Bars for the last two weeks, today on the right.
        let caption = Label { face: sans, size: 24.0, ink: INK_SOFT };
        let top = wide.bottom() + 70;
        caption.draw(c, raster, s.last_days, PAD as f32, top);
        let chart = Rect::new(PAD, top + 40, W - 2 * PAD, 520);
        let secs: Vec<i64> = (today - CHART_DAYS + 1..=today)
            .map(|d| self.days.iter().find(|r| r.day == d).map_or(0, |r| r.seconds))
            .collect();
        let max = secs.iter().copied().max().unwrap_or(0).max(15 * 60);
        let slot = chart.w / CHART_DAYS as i32;
        let base = chart.bottom() - 50;
        c.fill(Rect::new(chart.x, base, chart.w, 2), INK);
        let day_label = Label { face: sans, size: 22.0, ink: INK_SOFT };
        for (i, &v) in secs.iter().enumerate() {
            let x = chart.x + i as i32 * slot;
            let h = ((base - chart.y - 40) as i64 * v / max) as i32;
            if h > 0 {
                let bar = Rect::new(x + slot / 4, base - h, slot / 2, h + 1);
                rounded_rect(c, bar, 6, if i + 1 == secs.len() { INK } else { INK_SOFT });
            }
            let day = today - CHART_DAYS + 1 + i as i64;
            // 1970-01-01 was a Thursday: Monday-first index.
            let wd = (day + 3).rem_euclid(7) as usize;
            day_label.draw_centred(c, raster, s.weekday_initials[wd], (x + slot / 2) as f32, base + 40);
        }
        // The top of the scale, so the bars read as time.
        caption.draw(c, raster, &duration(max), chart.x as f32, chart.y + 20);

        let totals = Rect::new(PAD, chart.bottom() + 60, W - 2 * PAD, FIGURE_H);
        c.fill(Rect::new(PAD, totals.y - 20, W - 2 * PAD, 2), LINE);
        figures(
            c,
            raster,
            (serif_bold, sans),
            totals,
            &[
                (duration(sum.seconds), s.time_read),
                (sum.pages.to_string(), s.pages_read),
                (self.finished.to_string(), s.books_finished),
            ],
        );
        if self.days.is_empty() {
            caption.draw(c, raster, s.not_read_yet, PAD as f32, H - 120);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_weeks_and_streaks() {
        let r = |day, seconds| Reading { day, seconds, pages: 1 };
        let days = [r(1, 600), r(8, 60), r(9, 120), r(10, 30)];
        let s = summary(&days, 10);
        assert_eq!((s.seconds, s.pages, s.days), (810, 4, 4));
        assert_eq!((s.today, s.week, s.streak, s.last_day), (30, 210, 3, Some(10)));
        assert_eq!(summary(&days, 11).streak, 3, "today not read yet keeps yesterday's streak");
        assert_eq!(summary(&days, 12).streak, 0);
        assert_eq!(duration(0), "0 min");
        assert_eq!(duration(20), "< 1 min");
        assert_eq!(duration(3900), "1 h 05 min");
    }
}
