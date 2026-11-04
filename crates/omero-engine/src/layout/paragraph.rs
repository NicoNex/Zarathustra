//! Paragraph builder: styled runs → shaped text, break opportunities (UAX #14) and
//! hyphenation points → Knuth–Plass → positioned lines, with margin protrusion
//! (hanging punctuation) so justified edges look optically straight.

use std::collections::HashMap;

use icu_segmenter::{LineSegmenter, LineSegmenterBorrowed};

use super::hyphen::Hyphenators;
use super::knuth_plass::{self, Break, Item, Params, INFINITY};
use crate::font::{Face, ShapedGlyph};
use crate::style::cascade::ComputedStyle;
use crate::style::css::TextAlign;

/// A stretch of text in one style. Whitespace is already processed per `white-space`;
/// a `\n` left in the text is a forced line break.
pub struct Run<'a> {
    pub text: &'a str,
    pub style: &'a ComputedStyle,
    pub face: &'a Face,
    pub lang: Option<&'a str>,
}

/// A glyph placed on a line, x relative to the line's left edge, baseline at y = 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub run: u32,
    /// Byte offset in the run's text (the glyph's cluster); the anchor for selections.
    pub offset: u32,
    pub face: u32,
    pub gid: u32,
    pub size: f32,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    /// Added by layout (a hyphen at a hyphenated line end), not part of the source text.
    pub inserted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub glyphs: Vec<Placed>,
    /// Distance from the line box top to the baseline.
    pub baseline: f32,
    /// Line box height.
    pub height: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct LayoutParams {
    pub measure: f32,
    pub indent: f32,
    pub align: TextAlign,
}

/// Penalty for breaking at a hyphenation point (TeX's \hyphenpenalty).
const HYPHEN_PENALTY: f32 = 50.0;
const MIN_HYPHEN_WORD: usize = 5;

/// Long-lived typesetting state: the UAX #14 segmenter and loaded hyphenation dictionaries.
pub struct Typesetter {
    segmenter: LineSegmenterBorrowed<'static>,
    pub hyphenators: Hyphenators,
}

/// One shaped glyph in paragraph order; `advance` includes letter-spacing.
#[derive(Clone, Copy)]
struct G {
    run: u32,
    offset: u32,
    gid: u32,
    advance: f32,
    x_offset: f32,
    y_offset: f32,
}

/// What a Knuth–Plass item stands for, aligned with the item list.
#[derive(Clone, Copy)]
enum Piece {
    /// Glyphs `[start, end)` of the paragraph.
    Glyphs(usize, usize),
    /// Word space.
    Space,
    /// Hyphenation point: draws the run's hyphen if the line breaks here.
    Hyphen {
        run: u32,
    },
    Indent,
    /// Breaks without glyphs: break opportunities, paragraph end, forced breaks.
    Other,
}

struct Para<'a, 'r> {
    runs: &'a [Run<'r>],
    glyphs: Vec<G>,
    items: Vec<Item>,
    pieces: Vec<Piece>,
    /// Shaped hyphen per (face, size).
    hyphens: HashMap<(u32, u32), Vec<ShapedGlyph>>,
}

impl Para<'_, '_> {
    fn push(&mut self, item: Item, piece: Piece) {
        self.items.push(item);
        self.pieces.push(piece);
    }

    fn hyphen(&mut self, run: u32) -> &[ShapedGlyph] {
        let r = &self.runs[run as usize];
        self.hyphens.entry((r.face.id, r.style.font_size.to_bits())).or_insert_with(|| {
            let mut g = Vec::new();
            r.face.shape("-", r.style.font_size, r.lang, &mut g);
            g
        })
    }

    /// Glyphs within `run_glyphs` whose cluster lies in byte range `[a, b)` of their run.
    fn glyph_range(&self, run_glyphs: (usize, usize), a: usize, b: usize) -> (usize, usize) {
        let slice = &self.glyphs[run_glyphs.0..run_glyphs.1];
        let lo = slice.partition_point(|g| (g.offset as usize) < a);
        let hi = slice.partition_point(|g| (g.offset as usize) < b);
        (run_glyphs.0 + lo, run_glyphs.0 + hi)
    }
}

impl Typesetter {
    pub fn new(hyphenators: Hyphenators) -> Self {
        Typesetter { segmenter: LineSegmenter::new_auto(Default::default()), hyphenators }
    }

    pub fn layout(&mut self, runs: &[Run], p: &LayoutParams) -> Vec<Line> {
        if runs.iter().all(|r| r.text.trim().is_empty()) {
            return Vec::new();
        }
        let mut text = String::new();
        let mut starts = Vec::with_capacity(runs.len());
        for r in runs {
            starts.push(text.len());
            text.push_str(r.text);
        }
        let run_at = |pos: usize| starts.partition_point(|&s| s <= pos).saturating_sub(1);

        // Shape each run once: kerning and ligatures across words and punctuation stay intact.
        let mut para =
            Para { runs, glyphs: Vec::new(), items: Vec::new(), pieces: Vec::new(), hyphens: HashMap::new() };
        let mut run_glyphs = Vec::with_capacity(runs.len());
        let mut shaped = Vec::new();
        for (ri, r) in runs.iter().enumerate() {
            shaped.clear();
            r.face.shape(r.text, r.style.font_size, r.lang, &mut shaped);
            let first = para.glyphs.len();
            para.glyphs.extend(shaped.iter().map(|g| G {
                run: ri as u32,
                offset: g.cluster,
                gid: g.gid,
                advance: g.x_advance + r.style.letter_spacing,
                x_offset: g.x_offset,
                y_offset: g.y_offset,
            }));
            run_glyphs.push((first, para.glyphs.len()));
        }

        if p.indent > 0.0 {
            para.push(Item::Box { width: p.indent }, Piece::Indent);
        }
        let opportunities: Vec<usize> = self.segmenter.segment_str(&text).collect();
        for w in opportunities.windows(2) {
            let seg = &text[w[0]..w[1]];
            let word_end = w[0] + seg.trim_end_matches([' ', '\t', '\n']).len();

            let mut pos = w[0];
            while pos < word_end {
                let ri = run_at(pos);
                let end = (starts[ri] + runs[ri].text.len()).min(word_end);
                let (a, b) = (pos - starts[ri], end - starts[ri]);
                let whole_word = pos == w[0] && end == word_end;
                self.word(&mut para, ri, run_glyphs[ri], a, b, whole_word);
                pos = end;
            }

            if w[1] == text.len() {
                continue;
            }
            let trailing = &text[word_end..w[1]];
            if trailing.contains('\n') {
                para.push(Item::Glue { width: 0.0, stretch: INFINITY, shrink: 0.0 }, Piece::Other);
                para.push(Item::Penalty { width: 0.0, cost: -INFINITY, flagged: false }, Piece::Other);
            } else if !trailing.is_empty() {
                // Inter-word space at TeX proportions: stretch ½, shrink ⅓.
                let ri = run_at(word_end);
                let (lo, hi) = para.glyph_range(run_glyphs[ri], word_end - starts[ri], w[1] - starts[ri]);
                let sw: f32 = para.glyphs[lo..hi].iter().map(|g| g.advance).sum();
                para.push(Item::Glue { width: sw, stretch: sw / 2.0, shrink: sw / 3.0 }, Piece::Space);
            } else {
                // A break opportunity without a space (after an em dash, between CJK ideographs).
                para.push(Item::Penalty { width: 0.0, cost: 0.0, flagged: false }, Piece::Other);
            }
        }
        para.push(Item::Glue { width: 0.0, stretch: INFINITY, shrink: 0.0 }, Piece::Other);
        para.push(Item::Penalty { width: 0.0, cost: -INFINITY, flagged: false }, Piece::Other);

        // Strict first; then looser; then with emergency stretch so single-word lines are feasible.
        let passes = [(2.0, 0.0), (4.0, 0.0), (10.0, p.measure * 0.25)];
        let breaks = passes
            .iter()
            .find_map(|&(tolerance, emergency_stretch)| {
                let params = Params { tolerance, emergency_stretch, ..Params::default() };
                knuth_plass::breaks(&para.items, &[p.measure], &params)
            })
            .unwrap_or_else(|| greedy(&para.items, p.measure));

        let mut lines = Vec::with_capacity(breaks.len());
        let mut start = 0;
        for (n, b) in breaks.iter().enumerate() {
            let justify = p.align == TextAlign::Justify && n + 1 < breaks.len();
            lines.push(set_line(&mut para, start, b, p, justify));
            start = b.next;
        }
        lines
    }

    /// Boxes for one word piece of run `ri` (bytes `[a, b)`), split at hyphenation points
    /// that fall on glyph cluster boundaries (never inside a ligature).
    fn word(&mut self, para: &mut Para, ri: usize, run_glyphs: (usize, usize), a: usize, b: usize, whole: bool) {
        let run = &para.runs[ri];
        let (first, last) = para.glyph_range(run_glyphs, a, b);
        let mut cuts = Vec::new();
        if let (true, true, Some(lang)) = (whole, run.style.hyphens, run.lang) {
            let word = &run.text[a..b];
            if word.chars().filter(|c| c.is_alphabetic()).nth(MIN_HYPHEN_WORD - 1).is_some() {
                for off in self.hyphenators.breaks(lang, word) {
                    let target = (a + off) as u32;
                    let i = first + para.glyphs[first..last].partition_point(|g| g.offset < target);
                    if i < last && para.glyphs[i].offset == target {
                        cuts.push(i);
                    }
                }
            }
        }
        let mut from = first;
        for &cut in cuts.iter().chain(std::iter::once(&last)) {
            let width = para.glyphs[from..cut].iter().map(|g| g.advance).sum();
            para.push(Item::Box { width }, Piece::Glyphs(from, cut));
            if cut != last {
                let hw = para.hyphen(ri as u32).iter().map(|g| g.x_advance).sum();
                let item = Item::Penalty { width: hw, cost: HYPHEN_PENALTY, flagged: true };
                para.push(item, Piece::Hyphen { run: ri as u32 });
            }
            from = cut;
        }
    }
}

/// Fallback when even emergency stretch fails (an overlong word in a narrow column):
/// fill greedily; an overlong box gets its own, overfull line.
fn greedy(items: &[Item], measure: f32) -> Vec<Break> {
    let mut out = Vec::new();
    let (mut width, mut last_break) = (0.0f32, None);
    for (i, it) in items.iter().enumerate() {
        match *it {
            Item::Box { width: w } => {
                if width + w > measure {
                    if let Some(g) = last_break.take() {
                        out.push(Break { index: g, next: g + 1, ratio: 0.0 });
                        width = items[g + 1..i].iter().map(Item::width).sum();
                    }
                }
                width += w;
            }
            Item::Glue { width: w, .. } => {
                last_break = Some(i);
                width += w;
            }
            Item::Penalty { cost, .. } if cost <= -INFINITY => {
                out.push(Break { index: i, next: i + 1, ratio: 0.0 });
                width = 0.0;
                last_break = None;
            }
            Item::Penalty { cost, .. } if cost < INFINITY => last_break = Some(i),
            Item::Penalty { .. } => {}
        }
    }
    out
}

/// How far a glyph may hang into the margin, as a fraction of its advance.
fn protrusion(c: char, right: bool) -> f32 {
    match (c, right) {
        ('.' | ',', true) => 0.6,
        ('-' | '\u{2010}' | '\u{00ad}', true) => 0.7,
        ('\u{2013}', true) => 0.3,
        ('\u{2014}', true) => 0.2,
        ('\'' | '"' | '\u{2019}' | '\u{201d}', true) => 0.6,
        (':' | ';', true) => 0.3,
        ('!' | '?', true) => 0.2,
        ('\u{201c}' | '\u{2018}' | '"' | '\'', false) => 0.6,
        ('\u{00ab}' | '\u{2039}', false) => 0.4,
        ('(' | '[', false) => 0.2,
        _ => 0.0,
    }
}

fn char_at(runs: &[Run], g: &Placed) -> char {
    if g.inserted {
        return '-';
    }
    runs[g.run as usize].text[g.offset as usize..].chars().next().unwrap_or(' ')
}

/// A glue on a line: glyphs from index `after` on shift by its share of the adjustment.
struct LineGlue {
    after: usize,
    stretch: f32,
    shrink: f32,
}

fn set_line(para: &mut Para, start: usize, b: &Break, p: &LayoutParams, justify: bool) -> Line {
    let runs = para.runs;
    let mut placed: Vec<Placed> = Vec::new();
    let mut glues: Vec<LineGlue> = Vec::new();
    let mut x = 0.0f32;
    // (gid, advance, x offset, y offset)
    let place = |placed: &mut Vec<Placed>, x: &mut f32, run: u32, offset: u32, g: (u32, f32, f32, f32), inserted| {
        let r = &runs[run as usize];
        let (gid, advance, dx, dy) = g;
        let size = r.style.font_size;
        placed.push(Placed { run, offset, face: r.face.id, gid, size, x: *x + dx, y: -dy, advance, inserted });
        *x += advance;
    };
    for k in start..b.index {
        match (para.items[k], para.pieces[k]) {
            (_, Piece::Glyphs(lo, hi)) => {
                for g in &para.glyphs[lo..hi] {
                    place(&mut placed, &mut x, g.run, g.offset, (g.gid, g.advance, g.x_offset, g.y_offset), false);
                }
            }
            (Item::Glue { width, stretch, shrink }, _) => {
                glues.push(LineGlue { after: placed.len(), stretch, shrink });
                x += width;
            }
            (item, _) => x += item.width(),
        }
    }
    if let Piece::Hyphen { run } = para.pieces[b.index] {
        let offset = placed.last().map_or(0, |g| g.offset);
        for s in para.hyphen(run).to_vec() {
            place(&mut placed, &mut x, run, offset, (s.gid, s.x_advance, s.x_offset, s.y_offset), true);
        }
    }

    // Margin protrusion: edge punctuation hangs outside the measure (not into an indent).
    let indented = matches!(para.pieces.get(start), Some(Piece::Indent));
    let left = match placed.first() {
        Some(g) if !indented => protrusion(char_at(runs, g), false) * g.advance,
        _ => 0.0,
    };
    let right = placed.last().map_or(0.0, |g| protrusion(char_at(runs, g), true) * g.advance);

    let shift = if justify {
        // Each glue takes its own share: ratio × its stretch (or shrink), as Knuth–Plass assumed.
        let extra = p.measure + left + right - x;
        let (y, z): (f32, f32) = glues.iter().fold((0.0, 0.0), |(y, z), g| (y + g.stretch, z + g.shrink));
        let ratio = match extra {
            e if e >= 0.0 && y > 0.0 => e / y,
            e if e < 0.0 && z > 0.0 => (e / z).max(-1.0),
            _ => 0.0,
        };
        let mut offset = 0.0;
        let mut gi = 0;
        for (i, g) in placed.iter_mut().enumerate() {
            while gi < glues.len() && glues[gi].after <= i {
                offset += ratio * if ratio >= 0.0 { glues[gi].stretch } else { glues[gi].shrink };
                gi += 1;
            }
            g.x += offset;
        }
        -left
    } else {
        match p.align {
            TextAlign::Right => p.measure - x + right,
            TextAlign::Center => (p.measure - x) / 2.0,
            _ => -left,
        }
    };
    for g in &mut placed {
        g.x += shift;
    }

    // Line box: tallest line-height among the runs present; baseline with half-leading.
    let (mut height, mut asc, mut desc) = (0.0f32, 0.0f32, 0.0f32);
    let mut prev_run = None;
    for g in &placed {
        if prev_run != Some(g.run) {
            prev_run = Some(g.run);
            let r = &runs[g.run as usize];
            let m = r.face.line_metrics(r.style.font_size);
            height = height.max(r.style.line_height);
            asc = asc.max(m.ascent);
            desc = desc.max(m.descent);
        }
    }
    let baseline = asc + (height - (asc + desc)).max(0.0) / 2.0;
    Line { glyphs: placed, baseline, height: height.max(asc + desc) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;
    use crate::style::cascade::{compute, Preferences, Stylesheets};

    fn bookerly() -> Option<Face> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-fonts/Bookerly-Regular.ttf");
        Face::from_bytes(0, std::fs::read(path).ok()?)
    }

    fn style() -> ComputedStyle {
        let dom = Dom::parse_xml("<p>x</p>").expect("parses");
        let prefs = Preferences {
            font_size: 38.0,
            font_family: None,
            line_height: Some(1.5),
            justify: Some(true),
            hyphenate: true,
            page_width: 1000.0,
        };
        compute(&dom, &Stylesheets::default(), &prefs).into_iter().flatten().last().expect("p styled")
    }

    fn typesetter() -> Typesetter {
        Typesetter::new(Hyphenators::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hyphenation")))
    }

    const TEXT: &str = "Call me Ishmael. Some years ago—never mind how long precisely—having little or no \
        money in my purse, and nothing particular to interest me on shore, I thought I would sail about a \
        little and see the watery part of the world. It is a way I have of driving off the spleen and \
        regulating the circulation.";

    fn right_edge(line: &Line, runs: &[Run]) -> (f32, f32) {
        let g = line.glyphs.last().expect("glyphs");
        (g.x + g.advance, protrusion(char_at(runs, g), true) * g.advance)
    }

    #[test]
    fn justified_paragraph_fills_the_measure_with_hanging_punctuation() {
        let Some(face) = bookerly() else { return }; // dev font is local-only
        let style = style();
        let runs = [Run { text: TEXT, style: &style, face: &face, lang: Some("en") }];
        let p = LayoutParams { measure: 700.0, indent: 57.0, align: TextAlign::Justify };
        let lines = typesetter().layout(&runs, &p);
        assert!(lines.len() >= 6, "{} lines", lines.len());
        for (i, line) in lines[..lines.len() - 1].iter().enumerate() {
            let (right, hang) = right_edge(line, &runs);
            assert!((right - (700.0 + hang)).abs() < 0.5, "line {i} right edge {right} (hang {hang})");
        }
        assert!((lines[0].glyphs[0].x - 57.0).abs() < 0.01, "indent is exact even before a quote");
        assert!(lines.iter().all(|l| l.height >= 57.0 - 0.01), "line-height 1.5 × 38");
    }

    #[test]
    fn breaks_after_dashes_hyphenates_narrow_columns_and_honours_newlines() {
        let Some(face) = bookerly() else { return };
        let style = style();
        let mut ts = typesetter();
        let narrow = LayoutParams { measure: 300.0, indent: 0.0, align: TextAlign::Justify };
        let runs = [Run { text: TEXT, style: &style, face: &face, lang: Some("en") }];
        let lines = ts.layout(&runs, &narrow);
        assert!(lines.iter().any(|l| l.glyphs.last().is_some_and(|g| g.inserted)), "narrow columns hyphenate");

        let dash = [Run { text: "aaaaaaaaaaaa—bbbbbbbbbbbb", style: &style, face: &face, lang: None }];
        assert_eq!(ts.layout(&dash, &narrow).len(), 2, "breaks at the em dash");

        let wide = LayoutParams { measure: 900.0, indent: 0.0, align: TextAlign::Justify };
        let poem = [Run { text: "One line\nand another", style: &style, face: &face, lang: None }];
        assert_eq!(ts.layout(&poem, &wide).len(), 2, "newline forces a break");

        let blank = [Run { text: "  ", style: &style, face: &face, lang: None }];
        assert!(ts.layout(&blank, &wide).is_empty());
    }

    #[test]
    fn letter_spacing_widens_glyphs() {
        let Some(face) = bookerly() else { return };
        let plain = style();
        let mut spaced = style();
        spaced.letter_spacing = 5.0;
        let mut ts = typesetter();
        let p = LayoutParams { measure: 2000.0, indent: 0.0, align: TextAlign::Start };
        let mut width = |s: &ComputedStyle| {
            let runs = [Run { text: "letters", style: s, face: &face, lang: None }];
            let g = ts.layout(&runs, &p)[0].glyphs.last().copied().expect("glyph");
            g.x + g.advance
        };
        let (a, b) = (width(&plain), width(&spaced));
        assert!((b - a - 7.0 * 5.0).abs() < 0.5, "{a} → {b}");
    }
}
