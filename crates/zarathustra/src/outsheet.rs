//! An action's full output, opened from its last-run line: how it ended, when, how long it
//! took, then everything it printed and its error messages, a page at a time. The tool for
//! finding out why a plugin failed without a computer.

use std::cell::Cell;

use zarathustra_device::{Canvas, Rect};

use crate::extensions::{duration_text, run_line};
use crate::i18n::{fill, t};
use crate::menu::{topbar, BACK_W, PAD, TOPBAR_H};
use crate::plugin::{LastRun, Outcome, Plugin};
use crate::ui::{Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const LINE_H: i32 = 42;
const PAGER_H: i32 = 100;

/// One line of the sheet as drawn.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Field(&'static str, String),
    Heading(&'static str),
    Text(String),
    Gap,
}

pub struct OutputSheet {
    title: String,
    lines: Vec<Line>,
    page: usize,
    /// Pages, known once laid out at the last render.
    pages: Cell<usize>,
}

impl OutputSheet {
    pub fn new(plugin: &Plugin, run: &LastRun) -> OutputSheet {
        let s = t();
        let outcome = match run.outcome {
            Outcome::Exited(0) => s.succeeded.to_string(),
            _ => run_line(plugin, run).1,
        };
        let (out, err) = plugin.action_output();
        let mut lines = vec![
            Line::Field(s.outcome, outcome),
            Line::Field(s.started, fill(s.ago, &[&duration_text(run.started.elapsed().unwrap_or_default())])),
        ];
        if let Some(d) = run.duration {
            lines.push(Line::Field(s.duration, duration_text(d)));
        }
        for (heading, text) in [(s.output_heading, out), (s.errors_heading, err)] {
            lines.extend([Line::Gap, Line::Heading(heading)]);
            if text.trim().is_empty() {
                lines.push(Line::Text(s.no_output.into()));
            }
            lines.extend(text.lines().map(|l| Line::Text(l.replace('\t', "    "))));
        }
        OutputSheet { title: run.label.clone(), lines, page: 0, pages: Cell::new(1) }
    }

    /// A tap: `false` for the back link (close the sheet), else `true` (redraw).
    pub fn tap(&mut self, (x, y): (i32, i32)) -> bool {
        if y < TOPBAR_H {
            return x >= BACK_W;
        }
        if y >= H - PAGER_H {
            if x < W / 3 {
                self.page = self.page.saturating_sub(1);
            } else if x > W * 2 / 3 {
                self.page = (self.page + 1).min(self.pages.get().saturating_sub(1));
            }
        }
        true
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(sans_bold), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::sans(fonts, true), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_extensions, &self.title);
        let text = Label { face: sans, size: 28.0, ink: INK };
        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let heading = Label { face: sans_bold, size: 30.0, ink: INK };
        let width = (W - 2 * PAD) as f32;
        // Wrapped into screen lines, then cut into pages.
        let mut rows: Vec<(Line, String)> = Vec::new();
        for line in &self.lines {
            match line {
                Line::Text(t) if t.is_empty() => rows.push((Line::Gap, String::new())),
                Line::Text(t) => rows.extend(text.wrap(t, width, usize::MAX).into_iter().map(|w| (line.clone(), w))),
                _ => rows.push((line.clone(), String::new())),
            }
        }
        let per_page = ((H - TOPBAR_H - 40 - PAGER_H) / LINE_H).max(1) as usize;
        let pages = rows.len().div_ceil(per_page).max(1);
        self.pages.set(pages);
        let page = self.page.min(pages - 1);
        let mut y = TOPBAR_H + 40 + LINE_H;
        for (line, wrapped) in rows.iter().skip(page * per_page).take(per_page) {
            match line {
                Line::Field(k, v) => {
                    soft.draw(c, raster, k, PAD as f32, y);
                    text.draw(c, raster, &text.fit(v, width - 260.0), (PAD + 260) as f32, y);
                }
                Line::Heading(h) => {
                    heading.draw(c, raster, h, PAD as f32, y);
                    c.fill(Rect::new(PAD, y + 12, W - 2 * PAD, 2), LINE);
                }
                Line::Text(_) => drop(text.draw(c, raster, wrapped, PAD as f32, y)),
                Line::Gap => {}
            }
            y += LINE_H;
        }
        if pages > 1 {
            let label = Label { face: sans, size: 32.0, ink: INK };
            label.draw_centred(
                c,
                raster,
                &format!("‹   {} / {pages}   ›", page + 1),
                W as f32 / 2.0,
                H - PAGER_H / 2 + 12,
            );
        }
    }
}
