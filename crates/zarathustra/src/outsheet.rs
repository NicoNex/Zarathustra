//! An action's full output, opened from its last-run line: how it ended, when, how long it
//! took, then everything it printed and its error messages, a page at a time. The tool for
//! finding out why a plugin failed without a computer.

use std::cell::RefCell;

use zarathustra_device::{Canvas, Rect};

use crate::extensions::{duration_text, run_line};
use crate::i18n::{fill, t};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::plugin::{LastRun, Outcome, Plugin};
use crate::toc::{next_button, pager, prev_button, BACK_W, FOOTER_H};
use crate::ui::{Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const LINE_H: i32 = 42;

/// One line of the sheet as drawn.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Field(&'static str, String),
    Heading(&'static str),
    Text(String),
    Gap,
}

/// `text` in lines no wider than `width` by `measure`: broken at a space when one fits,
/// else mid-word, so paths, URLs and hashes show whole; leading spaces stay.
fn hard_wrap(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if measure(rest) <= width {
            lines.push(rest.to_string());
            break;
        }
        // Ends of the prefixes, one char at a time; the longest that fits (at least one char).
        let ends: Vec<usize> = rest.char_indices().skip(1).map(|(i, _)| i).chain([rest.len()]).collect();
        let fit = ends.partition_point(|&end| measure(&rest[..end]) <= width);
        let mut end = ends[fit.saturating_sub(1)];
        // Mid-word: back to the last space, unless it is part of the indentation.
        let mid_word = !rest[end..].starts_with(' ');
        if let Some(space) = rest[..end].rfind(' ').filter(|&sp| mid_word && !rest[..sp].trim().is_empty()) {
            end = space + 1;
        }
        lines.push(rest[..end].trim_end().to_string());
        rest = &rest[end..];
        if !mid_word {
            rest = rest.strip_prefix(' ').unwrap_or(rest);
        }
    }
    lines
}

pub struct OutputSheet {
    title: String,
    lines: Vec<Line>,
    page: usize,
    /// `lines` wrapped to the screen's width, at the first render (wrapping needs the font).
    rows: RefCell<Vec<Line>>,
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
            lines.extend(text.lines().map(|l| {
                if l.trim().is_empty() {
                    Line::Gap
                } else {
                    Line::Text(l.replace('\t', "    "))
                }
            }));
        }
        OutputSheet { title: run.label.clone(), lines, page: 0, rows: RefCell::default() }
    }

    /// Screen lines per page.
    fn per_page() -> usize {
        ((H - TOPBAR_H - 40 - FOOTER_H) / LINE_H).max(1) as usize
    }

    fn pages(&self) -> usize {
        self.rows.borrow().len().div_ceil(Self::per_page()).max(1)
    }

    /// A tap: `false` for the back link (close the sheet), else `true` (redraw).
    pub fn tap(&mut self, (x, y): (i32, i32)) -> bool {
        if y < TOPBAR_H {
            return x >= BACK_W;
        }
        if prev_button().contains(x, y) {
            self.page = self.page.saturating_sub(1);
        } else if next_button().contains(x, y) {
            self.page = (self.page + 1).min(self.pages() - 1);
        }
        true
    }

    /// Sends a tap to the sheet open in `sheet`, closing it on its back link; `false` when no
    /// sheet is open (the tap is the caller's).
    pub fn forward(sheet: &mut Option<OutputSheet>, at: (i32, i32)) -> bool {
        let Some(open) = sheet else { return false };
        if !open.tap(at) {
            *sheet = None;
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
        if self.rows.borrow().is_empty() {
            let wrapped = (self.lines.iter()).flat_map(|line| match line {
                Line::Text(t) => hard_wrap(t, width, |l| text.width(l)).into_iter().map(Line::Text).collect(),
                other => vec![other.clone()],
            });
            *self.rows.borrow_mut() = wrapped.collect();
        }
        let (pages, per_page) = (self.pages(), Self::per_page());
        let page = self.page.min(pages - 1);
        let mut y = TOPBAR_H + 40 + LINE_H;
        for line in self.rows.borrow().iter().skip(page * per_page).take(per_page) {
            match line {
                Line::Field(k, v) => {
                    soft.draw(c, raster, k, PAD as f32, y);
                    text.draw(c, raster, &text.fit(v, width - 260.0), (PAD + 260) as f32, y);
                }
                Line::Heading(h) => {
                    heading.draw(c, raster, h, PAD as f32, y);
                    c.fill(Rect::new(PAD, y + 12, W - 2 * PAD, 2), LINE);
                }
                Line::Text(t) => drop(text.draw(c, raster, t, PAD as f32, y)),
                Line::Gap => {}
            }
            y += LINE_H;
        }
        pager(c, raster, sans, page, pages);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_words_break_and_indentation_stays() {
        let chars = |s: &str| s.chars().count() as f32;
        assert_eq!(hard_wrap("ab cd ef", 5.0, chars), ["ab cd", "ef"]);
        assert_eq!(hard_wrap("/mnt/us/very/long", 6.0, chars), ["/mnt/u", "s/very", "/long"]);
        assert_eq!(hard_wrap("    at main.rs:12", 40.0, chars), ["    at main.rs:12"]);
        assert_eq!(hard_wrap("    x y", 5.0, chars), ["    x", "y"]);
    }
}
