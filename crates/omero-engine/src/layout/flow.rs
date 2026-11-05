//! Block building: the styled DOM becomes a flat list of text blocks (paragraphs, headings,
//! list items, cells) and image blocks, with collapsed vertical margins, CSS whitespace
//! processing and the inline style changes recorded as spans.

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

/// Where a stretch of a block's text came from: from block byte `at` on, the text continues
/// text node `node` from its byte `offset`, until the next entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    pub at: u32,
    pub node: NodeId,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Element whose style sets alignment, indent and line height.
    pub node: NodeId,
    pub text: String,
    pub spans: Vec<Span>,
    /// Map from `text` back to the DOM text nodes, in block order. A new entry starts at
    /// every text node and wherever whitespace collapsing or a case change broke the
    /// byte-for-byte correspondence; generated text (list markers) has none.
    pub sources: Vec<Source>,
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
    /// An image block (its `text` is empty).
    pub image: Option<ImageRef>,
}

/// An image in the flow: `<img>`, or an SVG wrapper around one `<image>` (common for covers).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRef {
    /// `src`/`href` as written; [`crate::book::Book::chapter`] resolves it to an archive path.
    pub src: String,
    /// Requested display size in device pixels, from CSS `width`/`height` or percentage
    /// attributes (resolved against the column).
    // ponytail: absolute `width`/`height` attributes are ignored, the pixel size from the file
    // header is the natural size. Map them like CSS px if a book relies on them.
    pub width: Option<f32>,
    pub height: Option<f32>,
    /// Pixel size from the image header, when it is a decodable format; filled by the book.
    pub intrinsic: Option<(u32, u32)>,
}

impl Block {
    /// The text of span `i`.
    pub fn span_text(&self, i: usize) -> &str {
        let end = self.spans.get(i + 1).map_or(self.text.len(), |s| s.start);
        &self.text[self.spans[i].start..end]
    }

    /// DOM text node and byte offset of block byte `pos`. Generated text before the first
    /// source maps to its start.
    pub fn source_at(&self, pos: usize) -> Option<(NodeId, usize)> {
        let i = self.sources.partition_point(|s| s.at as usize <= pos);
        let s = self.sources.get(i.saturating_sub(1))?;
        Some((s.node, s.offset as usize + pos.saturating_sub(s.at as usize)))
    }

    /// Block byte of byte `offset` in text node `node`, if the node's text is in this block.
    /// An offset inside collapsed whitespace maps to where the collapsed run ends.
    pub fn position_of(&self, node: NodeId, offset: usize) -> Option<usize> {
        let first = self.sources.iter().position(|s| s.node == node)?;
        let i = (self.sources.iter().rposition(|s| s.node == node && s.offset as usize <= offset)).unwrap_or(first);
        let s = self.sources[i];
        let end = self.sources.get(i + 1).map_or(self.text.len(), |n| n.at as usize);
        Some((s.at as usize + offset.saturating_sub(s.offset as usize)).min(end))
    }
}

struct Builder<'a> {
    dom: &'a Dom,
    styles: &'a [Option<ComputedStyle>],
    blocks: Vec<Block>,
    /// The paragraph being filled.
    text: String,
    spans: Vec<Span>,
    sources: Vec<Source>,
    /// The last character pushed was collapsible space (or the paragraph is empty).
    space: bool,
    /// Largest margin since the last block (adjacent margins collapse).
    margin: f32,
    break_pending: bool,
    /// Images met before any text of the current paragraph; they become blocks ahead of it.
    images: Vec<ImageRef>,
}

/// Blocks of a chapter body; `lang` is the book's language when no element sets one.
// ponytail: tables (cells become plain blocks), floats and inline-blocks are not laid out as
// such; their text flows as paragraphs.
pub fn blocks(dom: &Dom, styles: &[Option<ComputedStyle>], lang: Option<&str>) -> Vec<Block> {
    let mut b = Builder {
        dom,
        styles,
        blocks: Vec::new(),
        text: String::new(),
        spans: Vec::new(),
        sources: Vec::new(),
        space: true,
        margin: 0.0,
        break_pending: false,
        images: Vec::new(),
    };
    let root = dom.children(Dom::ROOT).find(|&c| dom.name(c).is_some());
    if let Some(root) = root {
        let lang = lang_of(dom, root).or(lang);
        b.block(root, 0.0, lang);
    }
    b.blocks
}

/// The block a link target `node` lands in: the block of the element itself, the block an
/// inline element sits in, or the first block after a container that makes no block of its own.
pub fn block_of(dom: &Dom, blocks: &[Block], node: NodeId) -> Option<usize> {
    let contains = |b: &Block| {
        let mut n = Some(node);
        while let Some(id) = n {
            if id == b.node {
                return true;
            }
            n = dom.parent(id);
        }
        false
    };
    match blocks.iter().position(|b| b.node.index() >= node.index()) {
        Some(i) if blocks[i].node == node => Some(i),
        Some(i) if i > 0 && contains(&blocks[i - 1]) => Some(i - 1),
        Some(i) => Some(i),
        None => blocks.iter().rposition(contains).or(blocks.len().checked_sub(1)),
    }
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
                    self.push_text(c, text, s);
                }
                NodeKind::Element { .. } => {
                    let Some(cs) = self.style(c) else { continue };
                    if cs.display == Display::None {
                    } else if matches!(self.dom.name(c), Some("img" | "svg")) {
                        if is_block(cs.display) {
                            self.flush(id, x, lang);
                        }
                        self.image(c);
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
        if matches!(self.dom.name(id), Some("img" | "svg")) {
            return self.image(id);
        }
        if self.dom.name(id) == Some("br") {
            if !self.images.is_empty() && self.text.trim().is_empty() {
                return; // the queued image becomes its own block: no line to break
            }
            self.push_span(id);
            self.text.push('\n');
            self.space = true;
            return;
        }
        for c in self.dom.children(id) {
            match self.dom.kind(c) {
                NodeKind::Text { text, .. } => {
                    self.push_span(id);
                    self.push_text(c, text, s);
                }
                NodeKind::Element { .. } if self.style(c).is_some_and(|cs| cs.display != Display::None) => {
                    self.inline(c)
                }
                _ => {}
            }
        }
    }

    /// An `<img>`, or an `<svg>` wrapping an `<image>`, queued as a block image when no text
    /// precedes it in the paragraph. Other SVG content is dropped.
    // ponytail: images inside running text (glyph-like ornaments, inline formulas) are skipped;
    // they need inline boxes in the line breaker.
    fn image(&mut self, id: NodeId) {
        if !self.text.trim().is_empty() {
            return;
        }
        let img = match self.dom.name(id) {
            Some("svg") => match self.dom.find(id, "image") {
                Some(i) => i,
                None => return,
            },
            _ => id,
        };
        // `xlink:href` is stored by its local name.
        let Some(src) = self.dom.attr(img, "src").or_else(|| self.dom.attr(img, "href")) else { return };
        let column = self.style(id).map_or(0.0, |s| s.content_width);
        let size = |key: &str| {
            let css = |n: NodeId| self.style(n).and_then(|s| if key == "width" { s.width } else { s.height });
            let pct = |n: NodeId| {
                let v = self.dom.attr(n, key)?.trim().strip_suffix('%')?.trim().parse::<f32>().ok()?;
                // Percentage heights have no definite containing height in reflowable text.
                (key == "width").then_some(v / 100.0 * column)
            };
            css(img).or_else(|| pct(img)).or_else(|| css(id)).or_else(|| pct(id))
        };
        self.images.push(ImageRef {
            src: src.trim().to_string(),
            width: size("width"),
            height: size("height"),
            intrinsic: None,
        });
    }

    fn push_span(&mut self, node: NodeId) {
        match self.spans.last_mut() {
            Some(last) if last.node == node => {}
            Some(last) if last.start == self.text.len() => last.node = node,
            _ => self.spans.push(Span { start: self.text.len(), node }),
        }
    }

    /// Appends text node `node`'s `text` per `white-space` and `text-transform`.
    fn push_text(&mut self, node: NodeId, text: &str, s: &ComputedStyle) {
        let upper = s.text_transform == TextTransform::Uppercase;
        let lower = s.text_transform == TextTransform::Lowercase;
        for (i, c) in text.char_indices() {
            self.map_source(node, i);
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

    /// Starts a source entry for byte `offset` of `node` unless the text about to be pushed
    /// continues the last entry byte for byte.
    fn map_source(&mut self, node: NodeId, offset: usize) {
        let at = self.text.len();
        let continues =
            self.sources.last().is_some_and(|s| s.node == node && s.offset as usize + (at - s.at as usize) == offset);
        if continues {
            return;
        }
        let (at, offset) = (u32::try_from(at).unwrap_or(u32::MAX), u32::try_from(offset).unwrap_or(u32::MAX));
        match self.sources.last_mut() {
            // Nothing was pushed since the last entry (its text collapsed away): replace it.
            Some(last) if last.at == at => *last = Source { at, node, offset },
            _ => self.sources.push(Source { at, node, offset }),
        }
    }

    /// Ends the current paragraph (if it has content) as a block styled by `node`.
    fn flush(&mut self, node: NodeId, x: f32, lang: Option<&str>) {
        let trimmed = self.text.trim_end_matches(' ').len();
        self.text.truncate(trimmed);
        self.spans.retain(|s| s.start < trimmed);
        self.sources.retain(|s| (s.at as usize) < trimmed);
        let s = self.style(node);
        let width = s.map_or(0.0, |s| s.content_width);
        let avoid_break_inside = s.is_some_and(|s| s.break_inside_avoid);
        let images = std::mem::take(&mut self.images);
        // A line break or stray space between an image and its paragraph's end is no text.
        if !images.is_empty() && self.text.trim().is_empty() {
            self.text.clear();
        }
        for image in images {
            self.blocks.push(Block {
                node,
                text: String::new(),
                spans: Vec::new(),
                sources: Vec::new(),
                lang: None,
                space_before: std::mem::take(&mut self.margin),
                x,
                width,
                break_before: std::mem::take(&mut self.break_pending),
                keep_with_next: false,
                avoid_break_inside: true,
                image: Some(image),
            });
        }
        if !self.text.is_empty() {
            let name = self.dom.name(node).unwrap_or_default();
            let heading = matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
            self.blocks.push(Block {
                node,
                text: std::mem::take(&mut self.text),
                spans: std::mem::take(&mut self.spans),
                sources: std::mem::take(&mut self.sources),
                lang: lang.map(str::to_string),
                space_before: std::mem::take(&mut self.margin),
                x,
                width,
                break_before: std::mem::take(&mut self.break_pending),
                keep_with_next: heading,
                avoid_break_inside,
                image: None,
            });
        }
        self.text.clear();
        self.spans.clear();
        self.sources.clear();
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
            align: None,
            hyphenate: true,
            page_width: 500.0,
        };
        let styles = compute(&dom, &Stylesheets::default(), &prefs);
        let blocks = blocks(&dom, &styles, Some("en"));
        (dom, blocks)
    }

    #[test]
    fn link_targets_find_their_block() {
        let (dom, b) = build(
            "<html><body><p id='a'>one</p><section id='s'><div>x <span id='i'>y</span><p id='p'>z</p>w \
             <em id='e'>v</em></div></section><p>last</p></body></html>",
        );
        let texts: Vec<&str> = b.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["one", "x y", "z", "w v", "last"]);
        let at = |id: &str| {
            let node = dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id)).expect("id");
            block_of(&dom, &b, node)
        };
        assert_eq!([at("a"), at("s"), at("i"), at("p"), at("e")], [0, 1, 1, 2, 3].map(Some));
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
    fn block_text_maps_back_to_the_dom() {
        let (dom, b) = build(
            "<html><body><p>\n   Some <em>very\n  emphatic</em>   text. </p>\
             <ol><li>one  two</li></ol><p style='text-transform: uppercase'>ﬁne x</p></body></html>",
        );
        let node_text = |n: NodeId| match dom.kind(n) {
            NodeKind::Text { text, .. } => text.as_str(),
            _ => "",
        };
        // Every non-space character comes from the same character in its text node, and the
        // DOM position maps back to it.
        for block in &b[..2] {
            for (pos, c) in block.text.char_indices().filter(|&(_, c)| c != ' ') {
                let (node, off) = block.source_at(pos).expect("mapped");
                if block.text.starts_with('1') && pos < "1.\u{a0}".len() {
                    continue; // the list marker is generated
                }
                assert_eq!(node_text(node)[off..].chars().next(), Some(c), "{:?} at {pos}", block.text);
                assert_eq!(block.position_of(node, off), Some(pos));
            }
        }
        // A space collapsed away maps to the end of its run.
        let (node, _) = b[0].source_at(0).expect("mapped");
        assert_eq!(b[0].position_of(node, 1), Some(0));
        let em = b[0].text.find("emphatic").expect("x");
        let (node, off) = b[0].source_at(em).expect("mapped");
        assert_eq!(&node_text(node)[off..], "emphatic");
        // Uppercase changes lengths ("ﬁ" → "FI"): the text after it is mapped again.
        assert_eq!(b[2].text, "FINE X");
        let (node, off) = b[2].source_at(b[2].text.find('X').expect("x")).expect("mapped");
        assert_eq!(&node_text(node)[off..], "x");
    }

    #[test]
    fn anonymous_text_around_blocks_and_hidden_content() {
        let (_, b) = build("<body><div>before<p>inside</p>after<script>x()</script></div></body>");
        let texts: Vec<&str> = b.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["before", "inside", "after"]);
    }

    #[test]
    fn images_become_blocks() {
        let (dom, b) = build(
            "<body><p>Text</p><p><span><img src='a.jpg'/></span></p>\
             <div><img src='b.png' width='50%'/><br/>Caption</div>\
             <p>Inline <img src='glyph.png'/> image</p>\
             <img src='c.gif' style='display:block; width:20%'/>\
             <svg width='100%' height='100%' viewBox='0 0 4 6'><title>t</title><image href='cover.jpg'/></svg></body>",
        );
        let srcs: Vec<Option<&str>> = b.iter().map(|b| b.image.as_ref().map(|i| i.src.as_str())).collect();
        let texts: Vec<&str> = b.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(srcs, [None, Some("a.jpg"), Some("b.png"), None, None, Some("c.gif"), Some("cover.jpg")]);
        assert_eq!(texts, ["Text", "", "", "Caption", "Inline image", "", ""]);
        let img = |i: usize| b[i].image.clone().expect("image");
        assert_eq!((img(1).width, img(1).height), (None, None));
        assert_eq!(img(2).width, Some(250.0), "50% of the 500 px column");
        assert_eq!(img(5).width, Some(100.0), "CSS 20%");
        assert_eq!(img(6).width, Some(500.0), "the svg's 100%");
        assert_eq!(dom.name(b[1].node), Some("p"));
        assert!(b[1].space_before > 0.0, "the paragraph's margin goes to its image");
    }
}
