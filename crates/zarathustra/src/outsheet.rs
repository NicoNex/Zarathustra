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
use crate::{screen_h, screen_w, Ctx};

const LINE_H: i32 = 42;

/// One line of the sheet as drawn.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Field(&'static str, String),
    Heading(&'static str),
    Text(String),
    Gap,
}

/// `text` in lines no wider than `width`, given its glyphs' (byte offset, advance) from one
/// shaping: broken at a space when one fits, else mid-word, so paths, URLs and hashes show
/// whole; leading spaces stay.
fn hard_wrap(text: &str, width: f32, glyphs: &[(usize, f32)]) -> Vec<String> {
    let mut lines = Vec::new();
    let (mut start, mut pen, mut k) = (0, 0.0, 0);
    while let Some(&(at, adv)) = glyphs.get(k) {
        if at > start && pen + adv > width {
            let line = &text[start..at];
            let indent = line.len() - line.trim_start().len();
            // At a space, or back to the last one (not the indentation's), or mid-word.
            let end = if text[at..].starts_with(' ') {
                at
            } else {
                line[indent..].rfind(' ').map_or(at, |sp| start + indent + sp + 1)
            };
            lines.push(text[start..end].trim_end().to_string());
            start = if end == at && text[at..].starts_with(' ') { end + 1 } else { end };
            k = glyphs.partition_point(|g| g.0 < start);
            pen = 0.0;
            continue;
        }
        pen += adv;
        k += 1;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(text[start..].to_string());
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
        ((screen_h() - TOPBAR_H - 40 - FOOTER_H) / LINE_H).max(1) as usize
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
        let width = (screen_w() - 2 * PAD) as f32;
        if self.rows.borrow().is_empty() {
            let wrapped = (self.lines.iter()).flat_map(|line| match line {
                Line::Text(t) => {
                    // In text order (right-to-left runs shape with descending offsets).
                    let mut glyphs = text.advances(t);
                    glyphs.sort_by_key(|g| g.0);
                    hard_wrap(t, width, &glyphs).into_iter().map(Line::Text).collect()
                }
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
                    c.fill(Rect::new(PAD, y + 12, screen_w() - 2 * PAD, 2), LINE);
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
        // One unit per character.
        let wrap = |t: &str, w| hard_wrap(t, w, &t.char_indices().map(|(i, _)| (i, 1.0)).collect::<Vec<_>>());
        assert_eq!(wrap("ab cd ef", 5.0), ["ab cd", "ef"]);
        assert_eq!(wrap("ab cdef", 5.0), ["ab", "cdef"]);
        assert_eq!(wrap("/mnt/us/very/long", 6.0), ["/mnt/u", "s/very", "/long"]);
        assert_eq!(wrap("    at main.rs:12", 40.0), ["    at main.rs:12"]);
        assert_eq!(wrap("    x y", 5.0), ["    x", "y"]);
    }
}
