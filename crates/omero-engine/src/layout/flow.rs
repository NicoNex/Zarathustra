//! Block building: the styled DOM becomes a flat list of text blocks (paragraphs, headings,
//! list items, cells) with collapsed vertical margins, CSS whitespace processing and the
//! inline style changes recorded as spans.

use crate::dom::{Dom, NodeId, NodeKind};
use crate::style::cascade::ComputedStyle;
use crate::style::css::{Display, ListStyle, TextTransform, WhiteSpace};

/// A run of a block's text styled by one element, from byte `start` to the next span.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: usize,
    /// Element whose computed style the text takes.
    pub node: NodeId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Element whose style sets alignment, indent and line height.
    pub node: NodeId,
    pub text: String,
    pub spans: Vec<Span>,
    pub lang: Option<String>,
    /// Collapsed margin above, device pixels.
    pub space_before: f32,
    /// Left inset and width of the text column.
    pub x: f32,
    pub width: f32,
    pub break_before: bool,
    /// Headings: never the last thing on a page.
    pub keep_with_next: bool,
    pub avoid_break_inside: bool,
}

impl Block {
    /// The text of span `i`.
    pub fn span_text(&self, i: usize) -> &str {
        let end = self.spans.get(i + 1).map_or(self.text.len(), |s| s.start);
        &self.text[self.spans[i].start..end]
    }
}

struct Builder<'a> {
    dom: &'a Dom,
    styles: &'a [Option<ComputedStyle>],
    blocks: Vec<Block>,
    /// The paragraph being filled.
    text: String,
    spans: Vec<Span>,
    /// The last character pushed was collapsible space (or the paragraph is empty).
    space: bool,
    /// Largest margin since the last block (adjacent margins collapse).
    margin: f32,
    break_pending: bool,
}

/// Blocks of a chapter body; `lang` is the book's language when no element sets one.
// ponytail: images, tables (cells become plain blocks), floats and inline-blocks are not laid out
// as such; their text flows as paragraphs. Add image blocks with the reader view.
pub fn blocks(dom: &Dom, styles: &[Option<ComputedStyle>], lang: Option<&str>) -> Vec<Block> {
    let mut b = Builder {
        dom,
        styles,
        blocks: Vec::new(),
        text: String::new(),
        spans: Vec::new(),
        space: true,
        margin: 0.0,
        break_pending: false,
    };
    let root = dom.children(Dom::ROOT).find(|&c| dom.name(c).is_some());
    if let Some(root) = root {
        let lang = lang_of(dom, root).or(lang);
        b.block(root, 0.0, lang);
    }
    b.blocks
}

fn lang_of(dom: &Dom, id: NodeId) -> Option<&str> {
    dom.attr(id, "xml:lang").or_else(|| dom.attr(id, "lang")).filter(|l| !l.is_empty())
}

fn is_block(display: Display) -> bool {
    !matches!(display, Display::Inline | Display::InlineBlock)
}

impl<'a> Builder<'a> {
    fn style(&self, id: NodeId) -> Option<&'a ComputedStyle> {
        self.styles.get(id.index()).and_then(Option::as_ref)
    }

    /// A block container at left inset `x`: its inline content becomes paragraphs, its block
    /// children recurse.
    fn block(&mut self, id: NodeId, x: f32, lang: Option<&'a str>) {
        let Some(s) = self.style(id) else { return };
        let [mt, _, mb, ml] = s.margin.map(|m| m.unwrap_or(0.0));
        self.margin = self.margin.max(mt + s.padding[0]);
        self.break_pending |= s.break_before;
        let x = x + ml + s.padding[3];

        if s.display == Display::ListItem {
            if let Some(marker) = self.marker(id, s.list_style) {
                self.push_span(id);
                self.text.push_str(&marker);
                self.space = true;
            }
        }
        for c in self.dom.children(id) {
            match self.dom.kind(c) {
                NodeKind::Text { text, .. } => {
                    self.push_span(id);
                    self.push_text(text, s);
                }
                NodeKind::Element { .. } => {
                    let Some(cs) = self.style(c) else { continue };
                    if cs.display == Display::None {
                    } else if is_block(cs.display) {
                        self.flush(id, x, lang);
                        self.block(c, x, lang_of(self.dom, c).or(lang));
                    } else {
                        self.inline(c);
                    }
                }
                NodeKind::Document => {}
            }
        }
        self.flush(id, x, lang);
        self.margin = self.margin.max(mb + s.padding[2]);
        self.break_pending |= s.break_after;
    }

    /// Inline content of `id`. Block descendants of inline elements
    /// (invalid but common) are flattened into the paragraph.
    fn inline(&mut self, id: NodeId) {
        let Some(s) = self.style(id) else { return };
        if self.dom.name(id) == Some("br") {
            self.push_span(id);
            self.text.push('\n');
            self.space = true;
            return;
        }
        for c in self.dom.children(id) {
            match self.dom.kind(c) {
                NodeKind::Text { text, .. } => {
                    self.push_span(id);
                    self.push_text(text, s);
                }
                NodeKind::Element { .. } if self.style(c).is_some_and(|cs| cs.display != Display::None) => {
                    self.inline(c)
                }
                _ => {}
            }
        }
    }

    fn push_span(&mut self, node: NodeId) {
        match self.spans.last_mut() {
            Some(last) if last.node == node => {}
            Some(last) if last.start == self.text.len() => last.node = node,
            _ => self.spans.push(Span { start: self.text.len(), node }),
        }
    }

    /// Appends text per `white-space` and `text-transform`.
    fn push_text(&mut self, text: &str, s: &ComputedStyle) {
        let upper = s.text_transform == TextTransform::Uppercase;
        let lower = s.text_transform == TextTransform::Lowercase;
        for c in text.chars() {
            if s.white_space == WhiteSpace::Pre {
                if c != '\r' {
                    self.text.push(c);
                }
                self.space = c == '\n';
            } else if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                if !self.space {
                    self.text.push(' ');
                    self.space = true;
                }
            } else {
                if upper {
                    self.text.extend(c.to_uppercase());
                } else if lower {
                    self.text.extend(c.to_lowercase());
                } else {
                    self.text.push(c);
                }
                self.space = false;
            }
        }
    }

    /// Ends the current paragraph (if it has content) as a block styled by `node`.
    fn flush(&mut self, node: NodeId, x: f32, lang: Option<&str>) {
        let trimmed = self.text.trim_end_matches(' ').len();
        self.text.truncate(trimmed);
        self.spans.retain(|s| s.start < trimmed);
        if !self.text.is_empty() {
            let s = self.style(node);
            let name = self.dom.name(node).unwrap_or_default();
            let heading = matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
            self.blocks.push(Block {
                node,
                text: std::mem::take(&mut self.text),
                spans: std::mem::take(&mut self.spans),
                lang: lang.map(str::to_string),
                space_before: std::mem::take(&mut self.margin),
                x,
                width: s.map_or(0.0, |s| s.content_width),
                break_before: std::mem::take(&mut self.break_pending),
                keep_with_next: heading,
                avoid_break_inside: s.is_some_and(|s| s.break_inside_avoid),
            });
        }
        self.text.clear();
        self.spans.clear();
        self.space = true;
    }

    /// List marker text for list item `id`, counted among its `li` siblings.
    fn marker(&self, id: NodeId, style: ListStyle) -> Option<String> {
        let n = std::iter::successors(self.dom.prev_sibling(id), |&p| self.dom.prev_sibling(p))
            .filter(|&p| self.dom.name(p) == Some("li"))
            .count()
            + 1;
        let letter = |base: u8| char::from(base + ((n - 1) % 26) as u8);
        Some(match style {
            ListStyle::None => return None,
            ListStyle::Disc => "•\u{a0}".into(),
            ListStyle::Circle => "◦\u{a0}".into(),
            ListStyle::Square => "▪\u{a0}".into(),
            ListStyle::Decimal => format!("{n}.\u{a0}"),
            ListStyle::LowerAlpha => format!("{}.\u{a0}", letter(b'a')),
            ListStyle::UpperAlpha => format!("{}.\u{a0}", letter(b'A')),
            ListStyle::LowerRoman => format!("{}.\u{a0}", roman(n).to_lowercase()),
            ListStyle::UpperRoman => format!("{}.\u{a0}", roman(n)),
        })
    }
}

fn roman(mut n: usize) -> String {
    const DIGITS: [(usize, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for &(v, s) in &DIGITS {
        while n >= v {
            out.push_str(s);
            n -= v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::cascade::{compute, Preferences, Stylesheets};

    fn build(html: &str) -> (Dom, Vec<Block>) {
        let dom = Dom::parse_xml(html).expect("parses");
        let prefs = Preferences {
            font_size: 20.0,
            font_family: None,
            line_height: None,
            justify: None,
            hyphenate: true,
            page_width: 500.0,
        };
        let styles = compute(&dom, &Stylesheets::default(), &prefs);
        let blocks = blocks(&dom, &styles, Some("en"));
        (dom, blocks)
    }

    #[test]
    fn paragraphs_spans_whitespace_and_margins() {
        let (dom, b) = build(
            "<html><body><h1>Title</h1><p>  Some <em>very\n  emphatic</em> text. </p>\
             <p xml:lang='it'>Due<br/>righe</p><ol><li>one</li><li>two</li></ol></body></html>",
        );
        let texts: Vec<&str> = b.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["Title", "Some very emphatic text.", "Due\nrighe", "1.\u{a0}one", "2.\u{a0}two"]);
        assert!(b[0].keep_with_next && !b[1].keep_with_next);
        assert_eq!(b[1].spans.len(), 3);
        assert_eq!(dom.name(b[1].spans[1].node), Some("em"));
        assert_eq!(b[1].span_text(1), "very emphatic");
        assert_eq!(b[2].lang.as_deref(), Some("it"));
        assert_eq!(b[1].lang.as_deref(), Some("en"));
        // h1 bottom margin (.67 × 40) and p top margin (20) collapse to the larger.
        assert!((b[1].space_before - 26.8).abs() < 0.01, "{}", b[1].space_before);
        assert!(b[3].x > 0.0, "list padding indents");
        assert_eq!(roman(1994), "MCMXCIV");
    }

    #[test]
    fn anonymous_text_around_blocks_and_hidden_content() {
        let (_, b) = build("<body><div>before<p>inside</p>after<script>x()</script></div></body>");
        let texts: Vec<&str> = b.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["before", "inside", "after"]);
    }
}
