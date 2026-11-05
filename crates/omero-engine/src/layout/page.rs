//! Pagination: blocks → lines → pages, with widow and orphan control, headings kept with
//! the text that follows, forced breaks and blocks that must not split.

use super::flow::Block;
use super::paragraph::{LayoutParams, Line, Placed, Run, Typesetter};
use crate::fonts::FontLibrary;
use crate::style::cascade::ComputedStyle;
use crate::style::css::TextAlign;

/// Fewest lines of a paragraph left at a page bottom (orphans) or carried to the next top (widows).
const MIN_LINES: usize = 2;

/// A line on a page; `glyphs[].run` indexes the block's spans.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLine {
    pub block: u32,
    /// Left edge and baseline, relative to the text area.
    pub x: f32,
    pub baseline: f32,
    pub glyphs: Vec<Placed>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub lines: Vec<PageLine>,
}

/// What pagination needs to know about a block besides its lines.
#[derive(Debug, Clone, Copy, Default)]
pub struct Flow {
    pub space_before: f32,
    pub x: f32,
    pub break_before: bool,
    pub keep_with_next: bool,
    pub avoid_break_inside: bool,
}

impl From<&Block> for Flow {
    fn from(b: &Block) -> Self {
        Flow {
            space_before: b.space_before,
            x: b.x,
            break_before: b.break_before,
            keep_with_next: b.keep_with_next,
            avoid_break_inside: b.avoid_break_inside,
        }
    }
}

/// Typesets and paginates a chapter's blocks into pages `height` pixels tall.
pub fn layout(
    blocks: &[Block],
    styles: &[Option<ComputedStyle>],
    fonts: &FontLibrary,
    ts: &mut Typesetter,
    height: f32,
) -> Vec<Page> {
    let laid = blocks.iter().map(|b| {
        let style = |n: crate::dom::NodeId| styles.get(n.index()).and_then(Option::as_ref);
        let runs: Option<Vec<Run>> = (0..b.spans.len())
            .map(|i| {
                let s = style(b.spans[i].node)?;
                let face = fonts.pick(&s.font_family, s.font_weight, s.italic)?;
                Some(Run { text: b.span_text(i), style: s, face, lang: b.lang.as_deref() })
            })
            .collect();
        let lines = match (runs, style(b.node)) {
            (Some(runs), Some(s)) => {
                // Headings are never stretched to the measure, whatever the justify preference.
                let align = match s.text_align {
                    TextAlign::Justify if b.keep_with_next => TextAlign::Start,
                    a => a,
                };
                ts.layout(&runs, &LayoutParams { measure: b.width, indent: s.text_indent, align })
            }
            _ => Vec::new(),
        };
        (Flow::from(b), lines)
    });
    paginate(laid, height)
}

/// Stacks blocks' lines into pages.
pub fn paginate(blocks: impl IntoIterator<Item = (Flow, Vec<Line>)>, height: f32) -> Vec<Page> {
    let blocks: Vec<(Flow, Vec<Line>)> = blocks.into_iter().collect();
    let mut pages = Vec::new();
    let mut page = Page::default();
    let mut y = 0.0f32;
    let mut carried_space = 0.0f32;
    let mut carried_break = false;

    for (bi, (flow, lines)) in blocks.iter().enumerate() {
        if lines.is_empty() {
            carried_space = carried_space.max(flow.space_before);
            carried_break |= flow.break_before;
            continue;
        }
        let space = std::mem::take(&mut carried_space).max(flow.space_before);
        let forced = std::mem::take(&mut carried_break) || flow.break_before;
        let new_page = |page: &mut Page, pages: &mut Vec<Page>, y: &mut f32| {
            if !page.lines.is_empty() {
                pages.push(std::mem::take(page));
            }
            *y = 0.0;
        };
        if forced {
            new_page(&mut page, &mut pages, &mut y);
        }
        if !page.lines.is_empty() {
            y += space;
        }
        // A heading moves on when it and the start of what follows don't fit together.
        if flow.keep_with_next && !page.lines.is_empty() {
            let own: f32 = lines.iter().map(|l| l.height).sum();
            let next = blocks[bi + 1..].iter().find(|(_, l)| !l.is_empty());
            let next_start =
                next.map_or(0.0, |(f, l)| f.space_before + l.iter().take(MIN_LINES).map(|l| l.height).sum::<f32>());
            if y + own + next_start > height {
                new_page(&mut page, &mut pages, &mut y);
            }
        }

        let n = lines.len();
        let mut i = 0;
        while i < n {
            let mut fit = 0;
            let mut h = y;
            while i + fit < n && h + lines[i + fit].height <= height + 0.01 {
                h += lines[i + fit].height;
                fit += 1;
            }
            let mut take = fit;
            if i + fit < n {
                // Splitting: no orphans at the bottom, no widows at the top of the next page.
                let rest = n - i - take;
                if rest < MIN_LINES {
                    take = take.saturating_sub(MIN_LINES - rest);
                }
                if i == 0 && (take < MIN_LINES || flow.avoid_break_inside) {
                    take = 0;
                }
                if take == 0 && page.lines.is_empty() {
                    take = fit.max(1); // nothing fits better on a fresh page: make progress
                }
            }
            for line in &lines[i..i + take] {
                page.lines.push(PageLine {
                    block: bi as u32,
                    x: flow.x,
                    baseline: y + line.baseline,
                    glyphs: line.glyphs.clone(),
                });
                y += line.height;
            }
            i += take;
            if i < n {
                new_page(&mut page, &mut pages, &mut y);
            }
        }
    }
    if !page.lines.is_empty() {
        pages.push(page);
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<Line> {
        vec![Line { glyphs: Vec::new(), baseline: 8.0, height: 10.0 }; n]
    }

    fn shape(pages: &[Page]) -> Vec<Vec<u32>> {
        pages.iter().map(|p| p.lines.iter().map(|l| l.block).collect()).collect()
    }

    #[test]
    fn fills_pages_and_controls_widows_and_orphans() {
        let f = Flow::default();
        // 10 lines per page. Block 0 takes 9; block 1 (5 lines) would leave 1 orphan: moves whole.
        let p = paginate([(f, lines(9)), (f, lines(5))], 100.0);
        assert_eq!(shape(&p), [vec![0; 9], vec![1; 5]]);

        // Block 1 (11 lines) after 3: 7 fit, 4 carried; fine.
        let p = paginate([(f, lines(3)), (f, lines(11))], 100.0);
        assert_eq!(p[0].lines.len(), 10);
        assert_eq!(p[1].lines.len(), 4);

        // Block 1 (8 lines) after 3: 7 fit, 1 widow → carry 2.
        let p = paginate([(f, lines(3)), (f, lines(8))], 100.0);
        assert_eq!((p[0].lines.len(), p[1].lines.len()), (9, 2));
    }

    #[test]
    fn headings_breaks_and_spacing() {
        let f = Flow::default();
        let heading = Flow { keep_with_next: true, ..f };
        // Heading at the page bottom with no room for two lines after it moves on.
        let p = paginate([(f, lines(8)), (heading, lines(1)), (f, lines(4))], 100.0);
        assert_eq!(shape(&p), [vec![0; 8], vec![1, 2, 2, 2, 2]]);

        let chapter = Flow { break_before: true, ..f };
        let p = paginate([(f, lines(2)), (chapter, lines(2))], 100.0);
        assert_eq!(p.len(), 2);

        // Space before is dropped at a page top and kept elsewhere.
        let spaced = Flow { space_before: 15.0, ..f };
        let p = paginate([(spaced, lines(1)), (spaced, lines(1))], 100.0);
        assert_eq!(p[0].lines[0].baseline, 8.0);
        assert_eq!(p[0].lines[1].baseline, 10.0 + 15.0 + 8.0);

        // A block taller than a page still makes progress.
        let p = paginate([(f, lines(25))], 100.0);
        assert_eq!(p.iter().map(|p| p.lines.len()).collect::<Vec<_>>(), [10, 10, 5]);
    }
}
