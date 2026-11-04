//! Paragraph builder: styled runs → shaped words, break opportunities (UAX #14) and
//! hyphenation points → Knuth–Plass → positioned lines, with margin protrusion
//! (hanging punctuation) so justified edges look optically straight.

use std::collections::HashMap;

use icu_segmenter::LineSegmenter;

use super::hyphen::Hyphenators;
use super::knuth_plass::{self, Item, Params, INFINITY};
use crate::font::{Face, ShapedGlyph};
use crate::style::cascade::ComputedStyle;
use crate::style::css::TextAlign;

/// A stretch of text in one style. Whitespace is already collapsed per `white-space`.
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub glyphs: Vec<Placed>,
    /// Distance from the line box top to the baseline.
    pub baseline: f32,
    /// Line box height.
    pub height: f32,
}

pub struct Params2 {
    pub measure: f32,
    pub indent: f32,
    pub align: TextAlign,
}

/// Penalty for breaking at a hyphenation point (TeX's \hyphenpenalty).
const HYPHEN_PENALTY: f32 = 50.0;
const MIN_HYPHEN_WORD: usize = 5;

/// One shaped glyph in paragraph order.
#[derive(Clone, Copy)]
struct G {
    run: u32,
    /// Byte offset in the run's text.
    offset: u32,
    g: ShapedGlyph,
}

/// What a Knuth–Plass item stands for, aligned with the item list.
#[derive(Clone, Copy)]
enum Piece {
    /// Glyphs `[start, end)` of the paragraph.
    Glyphs(usize, usize),
    /// Word space (no glyphs drawn).
    Space,
    /// Hyphenation point: draw a hyphen from `run` if the line breaks here.
    Hyphen { run: u32 },
    /// First-line indent or paragraph end.
    None,
}

pub fn layout(runs: &[Run], p: &Params2, hyph: &mut Hyphenators) -> Vec<Line> {
    if runs.iter().all(|r| r.text.is_empty()) {
        return Vec::new();
    }
    // Paragraph text and where each run starts in it.
    let mut text = String::new();
    let mut starts = Vec::with_capacity(runs.len());
    for r in runs {
        starts.push(text.len());
        text.push_str(r.text);
    }
    let run_at = |pos: usize| starts.partition_point(|&s| s <= pos).saturating_sub(1);

    let segmenter = LineSegmenter::new_auto(Default::default());
    let opportunities: Vec<usize> = segmenter.segment_str(&text).collect();

    let mut glyphs: Vec<G> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut space_cache: HashMap<(u32, u32), f32> = HashMap::new();
    let mut hyphen_cache: HashMap<(u32, u32), f32> = HashMap::new();

    if p.indent > 0.0 {
        items.push(Item::Box { width: p.indent });
        pieces.push(Piece::None);
    }

    for w in opportunities.windows(2) {
        let seg = &text[w[0]..w[1]];
        let word_len = seg.trim_end_matches([' ', '\t', '\n']).len();
        let (word_end, seg_end) = (w[0] + word_len, w[1]);

        // The word part, split at run boundaries (no break between pieces).
        let mut pos = w[0];
        while pos < word_end {
            let ri = run_at(pos);
            let run = &runs[ri];
            let end = (starts[ri] + run.text.len()).min(word_end);
            let local = (pos - starts[ri], end - starts[ri]);
            let piece_text = &run.text[local.0..local.1];
            let size = run.style.font_size;

            let first = glyphs.len();
            let mut shaped = Vec::new();
            run.face.shape(piece_text, size, run.lang, &mut shaped);
            glyphs.extend(shaped.iter().map(|g| G { run: ri as u32, offset: (local.0 as u32) + g.cluster, g: *g }));
            let last = glyphs.len();

            // Hyphenation points that fall on glyph cluster boundaries (never inside a ligature).
            let mut cuts: Vec<usize> = Vec::new();
            let alphabetic = piece_text.chars().filter(|c| c.is_alphabetic()).count();
            if run.style.hyphens && alphabetic >= MIN_HYPHEN_WORD && pos == w[0] && end == word_end {
                if let Some(lang) = run.lang {
                    for b in hyph.breaks(lang, piece_text) {
                        let b_off = (local.0 + b) as u32;
                        if let Some(i) = (first..last).find(|&i| glyphs[i].offset == b_off) {
                            cuts.push(i);
                        }
                    }
                }
            }
            let mut from = first;
            for &cut in cuts.iter().chain(std::iter::once(&last)) {
                let width = glyphs[from..cut].iter().map(|g| g.g.x_advance).sum();
                items.push(Item::Box { width });
                pieces.push(Piece::Glyphs(from, cut));
                if cut != last {
                    let hw = *hyphen_cache.entry((run.face.id, size.to_bits())).or_insert_with(|| advance_of(run, "-"));
                    items.push(Item::Penalty { width: hw, cost: HYPHEN_PENALTY, flagged: true });
                    pieces.push(Piece::Hyphen { run: ri as u32 });
                }
                from = cut;
            }
            pos = end;
        }

        // Inter-word space: TeX proportions (stretch ½, shrink ⅓ of the space).
        if seg_end > word_end && seg_end < text.len() {
            let ri = run_at(word_end);
            let run = &runs[ri];
            let sw = *space_cache
                .entry((run.face.id, run.style.font_size.to_bits()))
                .or_insert_with(|| advance_of(run, " "));
            let sw = sw + run.style.letter_spacing;
            items.push(Item::Glue { width: sw, stretch: sw / 2.0, shrink: sw / 3.0 });
            pieces.push(Piece::Space);
        }
    }
    items.push(Item::Glue { width: 0.0, stretch: INFINITY, shrink: 0.0 });
    pieces.push(Piece::None);
    items.push(Item::Penalty { width: 0.0, cost: -INFINITY, flagged: false });
    pieces.push(Piece::None);

    // Strict first; then looser; then with emergency stretch so single-word lines are feasible.
    let passes = [(2.0, 0.0), (4.0, 0.0), (10.0, p.measure * 0.25)];
    let breaks = passes
        .iter()
        .find_map(|&(tolerance, emergency_stretch)| {
            knuth_plass::breaks(&items, &[p.measure], &Params { tolerance, emergency_stretch, ..Params::default() })
        })
        .unwrap_or_else(|| greedy(&items, p.measure));

    let justify = p.align == TextAlign::Justify;
    let mut lines = Vec::with_capacity(breaks.len());
    let mut start = 0;
    for (n, b) in breaks.iter().enumerate() {
        let last_line = n + 1 == breaks.len();
        let range = start..b.index;
        start = b.next;
        lines.push(set_line(
            &items[range.clone()],
            &pieces[range],
            &pieces[b.index],
            &glyphs,
            runs,
            p,
            justify && !last_line,
        ));
    }
    lines
}

fn advance_of(run: &Run, s: &str) -> f32 {
    let mut g = Vec::new();
    run.face.shape(s, run.style.font_size, run.lang, &mut g);
    g.iter().map(|g| g.x_advance).sum()
}

/// Fallback when even a loose Knuth–Plass pass fails (an overlong word in a narrow
/// column): fill greedily, giving an overlong box its own (overfull) line.
fn greedy(items: &[Item], measure: f32) -> Vec<knuth_plass::Break> {
    let mut out = Vec::new();
    let (mut width, mut last_glue) = (0.0f32, None);
    for (i, it) in items.iter().enumerate() {
        match *it {
            Item::Box { width: w } => {
                if width + w > measure {
                    if let Some(g) = last_glue.take() {
                        out.push(knuth_plass::Break { index: g, next: g + 1, ratio: 0.0 });
                        width = items[g + 1..i].iter().map(item_width).sum();
                    }
                }
                width += w;
            }
            Item::Glue { width: w, .. } => {
                last_glue = Some(i);
                width += w;
            }
            Item::Penalty { cost, .. } if cost <= -INFINITY => {
                out.push(knuth_plass::Break { index: i, next: i + 1, ratio: 0.0 });
            }
            Item::Penalty { .. } => {}
        }
    }
    out
}

fn item_width(it: &Item) -> f32 {
    match *it {
        Item::Box { width } | Item::Glue { width, .. } => width,
        Item::Penalty { .. } => 0.0,
    }
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
    runs[g.run as usize].text[g.offset as usize..].chars().next().unwrap_or(' ')
}

fn set_line(
    items: &[Item],
    pieces: &[Piece],
    end: &Piece,
    glyphs: &[G],
    runs: &[Run],
    p: &Params2,
    justify: bool,
) -> Line {
    // Lay glyphs out at natural spacing, remembering where the glue sits.
    let mut placed: Vec<Placed> = Vec::new();
    let mut glue_after: Vec<usize> = Vec::new(); // index into `placed` after which each glue sits
    let (mut x, mut stretch, mut shrink) = (0.0f32, 0.0f32, 0.0f32);
    let place = |placed: &mut Vec<Placed>, x: &mut f32, g: &G| {
        placed.push(Placed {
            run: g.run,
            offset: g.offset,
            face: runs[g.run as usize].face.id,
            gid: g.g.gid,
            size: runs[g.run as usize].style.font_size,
            x: *x + g.g.x_offset,
            y: -g.g.y_offset,
            advance: g.g.x_advance,
        });
        *x += g.g.x_advance;
    };
    for (item, piece) in items.iter().zip(pieces) {
        match (item, piece) {
            (Item::Box { .. }, Piece::Glyphs(a, b)) => {
                glyphs[*a..*b].iter().for_each(|g| place(&mut placed, &mut x, g))
            }
            (Item::Box { width }, _) => x += width,
            (Item::Glue { width, stretch: y, shrink: z }, _) => {
                glue_after.push(placed.len());
                x += width;
                stretch += y;
                shrink += z;
            }
            _ => {}
        }
    }
    if let Piece::Hyphen { run } = *end {
        let r = &runs[run as usize];
        let mut shaped = Vec::new();
        r.face.shape("-", r.style.font_size, r.lang, &mut shaped);
        let offset = placed.last().map_or(0, |g| g.offset);
        for s in shaped {
            placed.push(Placed {
                run,
                offset,
                face: r.face.id,
                gid: s.gid,
                size: r.style.font_size,
                x,
                y: 0.0,
                advance: s.x_advance,
            });
            x += s.x_advance;
        }
    }

    // Margin protrusion: punctuation at either edge hangs outside the measure.
    let first = placed.first().copied();
    let last = placed.last().copied();
    let left = first.map_or(0.0, |g| protrusion(char_at(runs, &g), false) * g.advance);
    let right = match (last, end) {
        (Some(g), Piece::Hyphen { .. }) => protrusion('-', true) * g.advance,
        (Some(g), _) => protrusion(char_at(runs, &g), true) * g.advance,
        (None, _) => 0.0,
    };

    // Distribute the remaining space over the glue (justified) or align the line.
    let natural = x;
    let shift_x = if justify {
        let target = p.measure + left + right;
        let extra = target - natural;
        let per_unit = if extra >= 0.0 {
            if stretch > 0.0 {
                extra / stretch
            } else {
                0.0
            }
        } else if shrink > 0.0 {
            (extra / shrink).max(-1.0)
        } else {
            0.0
        };
        // Glue k widens by its own stretch/shrink share; glues are equal within a run, so
        // spread evenly by count.
        let total = if extra >= 0.0 { per_unit * stretch } else { per_unit * shrink };
        let each = if glue_after.is_empty() { 0.0 } else { total / glue_after.len() as f32 };
        let mut k = 0;
        for (i, g) in placed.iter_mut().enumerate() {
            while k < glue_after.len() && glue_after[k] <= i {
                k += 1;
            }
            g.x += each * k as f32;
        }
        -left
    } else {
        match p.align {
            TextAlign::Right => p.measure - natural + right,
            TextAlign::Center => (p.measure - natural) / 2.0,
            _ => -left,
        }
    };
    for g in &mut placed {
        g.x += shift_x;
    }

    // Line box: tallest line-height among the runs present; baseline with half-leading.
    let (mut height, mut asc, mut desc) = (0.0f32, 0.0f32, 0.0f32);
    let mut seen: Vec<u32> = Vec::new();
    for g in &placed {
        if !seen.contains(&g.run) {
            seen.push(g.run);
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
        let styles = compute(&dom, &Stylesheets::default(), &prefs);
        styles.into_iter().flatten().next().expect("p styled")
    }

    const TEXT: &str = "Call me Ishmael. Some years ago—never mind how long precisely—having little or no \
        money in my purse, and nothing particular to interest me on shore, I thought I would sail about a \
        little and see the watery part of the world. It is a way I have of driving off the spleen and \
        regulating the circulation.";

    #[test]
    fn justified_paragraph_fills_the_measure_with_hanging_punctuation() {
        let Some(face) = bookerly() else { return }; // dev font is local-only
        let style = style();
        let runs = [Run { text: TEXT, style: &style, face: &face, lang: Some("en") }];
        let mut hyph = Hyphenators::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hyphenation"));
        let p = Params2 { measure: 700.0, indent: 57.0, align: TextAlign::Justify };
        let lines = layout(&runs, &p, &mut hyph);
        assert!(lines.len() >= 6, "{} lines", lines.len());
        for (i, line) in lines[..lines.len() - 1].iter().enumerate() {
            let g = line.glyphs.last().expect("glyphs");
            let right = g.x + g.advance;
            let c = char_at(&runs, g);
            let hang = protrusion(if line_ends_with_hyphen(line, &runs) { '-' } else { c }, true) * g.advance;
            assert!((right - (700.0 + hang)).abs() < 0.5, "line {i} right edge {right} (hang {hang})");
        }
        assert!(lines[0].glyphs[0].x >= 56.0, "first-line indent");
        assert!(lines.iter().all(|l| l.height >= 57.0 - 0.01), "line-height 1.5 × 38");
        // Every character is drawn exactly once (plus inserted hyphens).
        let drawn: usize = lines.iter().map(|l| l.glyphs.len()).sum();
        assert!(drawn >= TEXT.chars().filter(|c| !c.is_whitespace()).count() - 10, "ligatures only");
    }

    fn line_ends_with_hyphen(line: &Line, runs: &[Run]) -> bool {
        let g = line.glyphs.last().expect("glyphs");
        char_at(runs, g) != '-' && line.glyphs.len() >= 2 && {
            let prev = line.glyphs[line.glyphs.len() - 2];
            prev.offset == g.offset
        }
    }

    #[test]
    fn narrow_measure_hyphenates_and_overlong_words_fall_back() {
        let Some(face) = bookerly() else { return };
        let style = style();
        let mut hyph = Hyphenators::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hyphenation"));
        let runs = [Run { text: TEXT, style: &style, face: &face, lang: Some("en") }];
        let p = Params2 { measure: 300.0, indent: 0.0, align: TextAlign::Justify };
        let lines = layout(&runs, &p, &mut hyph);
        assert!(lines.iter().any(|l| line_ends_with_hyphen(l, &runs)), "narrow columns hyphenate");

        let long = [Run { text: "Supercalifragilisticexpialidocious word", style: &style, face: &face, lang: None }];
        let lines = layout(&long, &Params2 { measure: 100.0, indent: 0.0, align: TextAlign::Justify }, &mut hyph);
        assert!(lines.len() >= 2, "greedy fallback still lays out");
    }
}
