//! "About this book": cover, title, author and series, the reader's own figures for the book,
//! then the description (the blurb) in the reading font.

use std::path::Path;

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::image::{self, GrayImage};
use zarathustra_engine::library;

use crate::i18n::{lang, t};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::stats::{self, figures, Summary};
use crate::ui::{clip_corners, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{screen_h, screen_w, Ctx};

const COVER_W: i32 = 300;
const BLURB_SIZE: f32 = 34.0;
const BLURB_LINE: i32 = 50;

pub struct About {
    title: String,
    author: String,
    series: Option<String>,
    description: Option<String>,
    facts: String,
    reading: Summary,
}

impl About {
    /// Reads the book's metadata and statistics now (a few ms; the home is idle meanwhile).
    pub fn new(state_dir: &Path, path: &Path, title: &str, author: &str) -> About {
        let meta = library::details(path);
        let series = meta.as_ref().and_then(|m| {
            let name = m.series.clone()?;
            Some(match m.series_index {
                Some(i) => format!("{name} · {i}"),
                None => name,
            })
        });
        let format = path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        let size = std::fs::metadata(path).map_or(0, |m| m.len());
        let mut facts = vec![format, format!("{:.1} MB", size as f64 / 1_048_576.0)];
        facts.extend(meta.as_ref().and_then(|m| m.language.clone()).map(|l| l.to_uppercase()));
        let days = stats::days(state_dir, Some(path));
        About {
            title: title.into(),
            author: author.into(),
            series,
            description: meta.and_then(|m| m.description),
            facts: facts.join("  ·  "),
            reading: stats::summary(&days, crate::clock::local_now().0),
        }
    }

    /// Only the back button does anything.
    pub fn back(&self, (x, y): (i32, i32)) -> bool {
        y < TOPBAR_H && x < 400
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx, cover: Option<&GrayImage>) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_library, s.about);

        // Cover left, the book's identity right of it.
        let top = TOPBAR_H + 56;
        let mut x = PAD;
        let mut head_bottom = top + 200;
        if let Some(img) = cover {
            let (w, h) = (img.width as i32, img.height as i32);
            let at = Rect::new(PAD + (COVER_W - w).max(0) / 2, top, w, h);
            image::draw(c, img, at.x, at.y);
            clip_corners(c, at, 10, [PAPER; 4]);
            x = PAD + COVER_W.max(w) + 48;
            head_bottom = at.bottom();
        }
        let width = (screen_w() - PAD - x) as f32;
        let title = Label { face: serif_bold, size: 50.0, ink: INK };
        let mut y = top + 50;
        for line in title.wrap(&self.title, width, 4) {
            title.draw(c, raster, &line, x as f32, y);
            y += 62;
        }
        let soft = Label { face: sans, size: 30.0, ink: INK_SOFT };
        for text in
            [Some(self.author.as_str()), self.series.as_deref(), Some(self.facts.as_str())].into_iter().flatten()
        {
            y += 10;
            soft.draw(c, raster, &soft.fit(text, width), x as f32, y);
            y += 34;
        }

        // The reader's figures for this book.
        let caption = Label { face: sans, size: 24.0, ink: INK_SOFT };
        let mut y = head_bottom.max(y) + 70;
        caption.draw(c, raster, s.your_reading, PAD as f32, y);
        let r = &self.reading;
        if r.days == 0 {
            soft.draw(c, raster, s.not_read_yet, PAD as f32, y + 56);
            y += 90;
        } else {
            let last = r.last_day.map_or_else(String::new, |d| crate::clock::date_text(lang(), d));
            let row = Rect::new(PAD, y + 10, screen_w() - 2 * PAD, 130);
            figures(
                c,
                raster,
                (serif_bold, sans),
                row,
                &[
                    (stats::duration(r.seconds), s.time_read),
                    (r.pages.to_string(), s.pages_read),
                    (r.days.to_string(), s.days_read),
                ],
            );
            soft.draw(c, raster, &format!("{}: {last}", s.last_read), PAD as f32, row.bottom() + 30);
            y = row.bottom() + 60;
        }

        // The blurb, as much as fits.
        y += 50;
        c.fill(Rect::new(PAD, y - 40, screen_w() - 2 * PAD, 2), LINE);
        caption.draw(c, raster, s.description, PAD as f32, y);
        y += 30;
        let blurb = Label { face: serif, size: BLURB_SIZE, ink: INK };
        let Some(text) = &self.description else {
            soft.draw(c, raster, s.no_description, PAD as f32, y + 40);
            return;
        };
        // ponytail: a long blurb is cut with an ellipsis; page through it if users miss the end.
        let mut room = ((screen_h() - 60 - y) / BLURB_LINE).max(0) as usize;
        for para in text.lines() {
            if room == 0 {
                break;
            }
            let lines = blurb.wrap(para, (screen_w() - 2 * PAD) as f32, room);
            for line in &lines {
                y += BLURB_LINE;
                blurb.draw(c, raster, line, PAD as f32, y);
            }
            room = room.saturating_sub(lines.len() + 1);
            y += BLURB_LINE / 3;
        }
    }
}
