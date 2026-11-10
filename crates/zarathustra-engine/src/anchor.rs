//! Stable positions inside a book, for highlights, bookmarks and reading progress.
//!
//! A position is stored three ways, each covering the others' weak spot:
//! - an **EPUB CFI** (`epubcfi(/6/4!/4/2[p1]/1:10)`), the interoperable path through the
//!   spine and the chapter's element tree, with character offsets in UTF-16 code units;
//! - a **source offset** (spine index + byte offset in the chapter file), a cheap sort key;
//! - a **text-quote selector** (W3C `TextQuoteSelector`: exact text plus ~32 characters of
//!   context), to find the text again when the file or its markup changes.

use std::ops::Range;

use crate::dom::{Dom, NodeId, NodeKind};

/// Characters of context kept on each side of a quote.
pub const CONTEXT_CHARS: usize = 32;

/// A position in a book: the CFI plus a sort key that orders positions in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub cfi: String,
    pub spine: u32,
    /// Byte offset in the chapter's source file (see [`source_offset`]).
    pub pos: u32,
}

impl Anchor {
    /// Anchor for byte `offset` inside `node` (a text node, or an element for offset 0) of
    /// the chapter at `spine`.
    pub fn new(dom: &Dom, spine: u32, node: NodeId, offset: usize) -> Anchor {
        Anchor { cfi: cfi_for(dom, spine, node, offset), spine, pos: source_offset(dom, node, offset) }
    }
}

/// Byte offset of a DOM position in the chapter's source file, for ordering positions.
/// Elements map to their first text at or after them.
// ponytail: text offsets are in decoded text, so inside a node with entities the key drifts
// by a few bytes; it still orders positions correctly, which is all it is used for.
pub fn source_offset(dom: &Dom, node: NodeId, offset: usize) -> u32 {
    let next_text = dom.nodes_from(node).find_map(|n| match dom.kind(n) {
        NodeKind::Text { offset: start, .. } => Some((n == node, *start)),
        _ => None,
    });
    match next_text {
        Some((true, start)) => start.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX)),
        Some((false, start)) => start,
        None => u32::MAX,
    }
}

/// EPUB CFI for byte `offset` inside `node` of the chapter at spine position `spine`.
///
/// The package part assumes the conventional OPF layout (`<spine>` is the third child
/// element of `<package>`, hence `/6`). Element steps carry `[id]` assertions when the
/// element has an id, so [`resolve_cfi`] survives markup inserted before it.
pub fn cfi_for(dom: &Dom, spine: u32, node: NodeId, offset: usize) -> String {
    let mut steps = Vec::new();
    let mut cur = node;
    while let Some(parent) = dom.parent(cur) {
        if parent == Dom::ROOT {
            break; // `cur` is the root element: the path starts below it.
        }
        let elements_before = siblings_before(dom, cur).filter(|&s| dom.name(s).is_some()).count();
        let step = match dom.kind(cur) {
            NodeKind::Element { .. } => {
                let index = 2 * (elements_before + 1);
                match dom.attr(cur, "id") {
                    Some(id) => format!("/{index}[{}]", escape(id)),
                    None => format!("/{index}"),
                }
            }
            _ => format!("/{}", 2 * elements_before + 1),
        };
        steps.push(step);
        cur = parent;
    }
    let mut cfi = format!("epubcfi(/6/{}!", 2 * (u64::from(spine) + 1));
    for step in steps.iter().rev() {
        cfi.push_str(step);
    }
    if let NodeKind::Text { text, .. } = dom.kind(node) {
        let utf16: usize = text.char_indices().take_while(|&(i, _)| i < offset).map(|(_, c)| c.len_utf16()).sum();
        cfi.push_str(&format!(":{utf16}"));
    }
    cfi.push(')');
    cfi
}

/// Spine index named by a CFI's package step (`/6/N` → `N/2 - 1`).
pub fn cfi_spine(cfi: &str) -> Option<u32> {
    let rest = cfi.strip_prefix("epubcfi(/6/")?;
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let n: u32 = rest[..digits].parse().ok()?;
    (n >= 2 && n.is_multiple_of(2)).then(|| n / 2 - 1)
}

/// Resolves the in-chapter part of a CFI produced by [`cfi_for`] (or by another reader
/// using plain element/text steps) to a node and a byte offset in its text.
///
/// An `[id]` assertion that disagrees with the indexed child wins: the element is looked up
/// by id. Text-location assertions and side bias are ignored. Ranges (`,`), spatial (`@`)
/// and temporal (`~`) offsets are not supported.
pub fn resolve_cfi(dom: &Dom, cfi: &str) -> Option<(NodeId, usize)> {
    let inner = cfi.strip_prefix("epubcfi(")?.strip_suffix(')')?;
    let (_, mut path) = inner.split_once('!')?;
    let mut cur = dom.children(Dom::ROOT).find(|&c| dom.name(c).is_some())?;
    let mut char_offset = None;
    while !path.is_empty() {
        if let Some(rest) = path.strip_prefix(':') {
            let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            char_offset = Some(rest[..digits].parse::<usize>().ok()?);
            // Whatever follows (text assertion, side bias) is advisory.
            break;
        }
        let rest = path.strip_prefix('/')?;
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let index: usize = rest[..digits].parse().ok()?;
        path = &rest[digits..];
        let mut id = None;
        if path.starts_with('[') {
            let (assertion, rest) = split_assertion(path)?;
            id = assertion.split(';').next().filter(|s| !s.is_empty()).map(str::to_owned);
            path = rest;
        }
        dom.name(cur)?; // A text node has no children.
        cur = child_at(dom, cur, index, id.as_deref())?;
    }
    match dom.kind(cur) {
        NodeKind::Text { text, .. } => Some((cur, utf16_to_byte(text, char_offset.unwrap_or(0))?)),
        _ => Some((cur, 0)),
    }
}

/// Child of `parent` at CFI step `index` (even: element, odd: text between elements).
fn child_at(dom: &Dom, parent: NodeId, index: usize, id: Option<&str>) -> Option<NodeId> {
    if index == 0 {
        return None;
    }
    let mut elements = 0;
    for child in dom.children(parent) {
        let is_element = dom.name(child).is_some();
        if is_element {
            elements += 1;
            if index == 2 * elements {
                return match id {
                    Some(id) if dom.attr(child, "id") != Some(id) => element_by_id(dom, id),
                    _ => Some(child),
                };
            }
        } else if index == 2 * elements + 1 {
            return Some(child);
        }
    }
    id.and_then(|id| element_by_id(dom, id))
}

/// The element whose `id` attribute is `id`.
pub fn element_by_id(dom: &Dom, id: &str) -> Option<NodeId> {
    dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id))
}

fn siblings_before(dom: &Dom, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    std::iter::successors(dom.prev_sibling(node), |&s| dom.prev_sibling(s))
}

/// Splits `[assertion]rest`, unescaping `^`-escaped characters inside the brackets.
fn split_assertion(s: &str) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '^' => out.push(chars.next()?.1),
            ']' => return Some((out, &s[i + 1..])),
            _ => out.push(c),
        }
    }
    None
}

/// Escapes the CFI special characters inside an assertion.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '^' | '[' | ']' | '(' | ')' | ',' | ';' | '=') {
            out.push('^');
        }
        out.push(c);
    }
    out
}

/// Byte offset of UTF-16 offset `units` in `text`; `None` past the end. An offset inside a
/// surrogate pair snaps to the start of its character.
fn utf16_to_byte(text: &str, units: usize) -> Option<usize> {
    let mut seen = 0;
    for (i, c) in text.char_indices() {
        seen += c.len_utf16();
        if seen > units {
            return Some(i);
        }
    }
    (seen == units).then_some(text.len())
}

/// The chapter's text as one string (every text node under `<body>`, verbatim, in document
/// order), with the mapping between string offsets and DOM positions.
#[derive(Debug, Clone)]
pub struct TextMap {
    text: String,
    /// Start of each text node in `text`, in document order.
    starts: Vec<(usize, NodeId)>,
}

impl TextMap {
    pub fn new(dom: &Dom) -> TextMap {
        let body = dom.find(Dom::ROOT, "body").unwrap_or(Dom::ROOT);
        let mut map = TextMap { text: String::new(), starts: Vec::new() };
        for n in dom.descendants(body) {
            if let NodeKind::Text { text, .. } = dom.kind(n) {
                map.starts.push((map.text.len(), n));
                map.text.push_str(text);
            }
        }
        map
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Offset in [`text`](Self::text) of byte `offset` inside text node `node`.
    pub fn to_text(&self, node: NodeId, offset: usize) -> Option<usize> {
        let i = self.starts.binary_search_by_key(&node, |&(_, n)| n).ok()?;
        let start = self.starts[i].0;
        let end = self.starts.get(i + 1).map_or(self.text.len(), |&(s, _)| s);
        (offset <= end - start).then_some(start + offset)
    }

    /// Text node and byte offset of `pos` in [`text`](Self::text). A position between two
    /// nodes maps to the start of the later one.
    pub fn to_dom(&self, pos: usize) -> Option<(NodeId, usize)> {
        if pos > self.text.len() {
            return None;
        }
        let i = self.starts.partition_point(|&(s, _)| s <= pos).checked_sub(1)?;
        let (start, node) = self.starts[i];
        Some((node, pos - start))
    }

    /// Like [`to_dom`](Self::to_dom), but a position between two nodes maps to the end of the
    /// earlier one: the end of a range that stops at a paragraph's end stays in that
    /// paragraph instead of landing in the whitespace between blocks.
    pub fn to_dom_end(&self, pos: usize) -> Option<(NodeId, usize)> {
        if pos > self.text.len() {
            return None;
        }
        match self.starts.partition_point(|&(s, _)| s < pos).checked_sub(1) {
            Some(i) => Some((self.starts[i].1, pos - self.starts[i].0)),
            None => self.to_dom(pos),
        }
    }
}

/// W3C text-quote selector: the selected text and its context, whitespace-collapsed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextQuote {
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
}

impl TextQuote {
    /// Quote for byte `range` of `text`, with up to [`CONTEXT_CHARS`] of context per side.
    pub fn new(text: &str, range: Range<usize>) -> TextQuote {
        // Normalise a window around the range: enough for the context even when the markup
        // indentation adds long whitespace runs.
        const WINDOW: usize = 1024;
        let lo = text.floor_char_boundary(range.start.saturating_sub(WINDOW));
        let hi = text.ceil_char_boundary(range.end.saturating_add(WINDOW).min(text.len()));
        let n = Normalized::new(&text[lo..hi]);
        let start = n.to_normalized(range.start - lo);
        let end = n.to_normalized(range.end.min(hi).saturating_sub(lo)).max(start);
        let before = &n.text[..start];
        let prefix_at = before.char_indices().rev().nth(CONTEXT_CHARS - 1).map_or(0, |(i, _)| i);
        let after = &n.text[end..];
        let suffix_end = after.char_indices().nth(CONTEXT_CHARS).map_or(after.len(), |(i, _)| i);
        TextQuote {
            exact: n.text[start..end].to_owned(),
            prefix: before[prefix_at..].to_owned(),
            suffix: after[..suffix_end].to_owned(),
        }
    }
}

/// Finds `quote` in `text` again (typically a [`TextMap`] text after the book changed) and
/// returns its byte range in `text`.
///
/// The exact text is tried first, ties broken by how much context matches. When the exact
/// text itself was edited, the range between a matching prefix and suffix is used, provided
/// its length is plausible; shorter context is tried when the context was edited too.
// ponytail: substring search, not Bitap/Myers. Survives edits around or inside the quote,
// but not edits that touch the quote *and* both context halves; upgrade to a bounded
// edit-distance search when orphaned highlights show up in practice.
pub fn reanchor(text: &str, quote: &TextQuote) -> Option<Range<usize>> {
    let n = Normalized::new(text);
    let hay = n.text.as_str();
    let found = if quote.exact.is_empty() { None } else { best_exact(hay, quote) };
    let found = found.or_else(|| {
        [CONTEXT_CHARS, CONTEXT_CHARS / 2].into_iter().find_map(|chars| {
            let prefix = tail_chars(&quote.prefix, chars);
            let suffix = head_chars(&quote.suffix, chars);
            between_context(hay, prefix, suffix, quote.exact.len())
        })
    })?;
    Some(n.map[found.start]..n.map[found.end])
}

fn best_exact(hay: &str, quote: &TextQuote) -> Option<Range<usize>> {
    let score = |start: usize| {
        let end = start + quote.exact.len();
        common_suffix(&hay[..start], &quote.prefix) + common_prefix(&hay[end..], &quote.suffix)
    };
    let mut best: Option<(usize, usize)> = None;
    for (start, _) in hay.match_indices(quote.exact.as_str()) {
        let s = score(start);
        if best.is_none_or(|(_, b)| s > b) {
            best = Some((start, s));
        }
    }
    best.map(|(start, _)| start..start + quote.exact.len())
}

/// Range between an occurrence of `prefix` and the nearest following `suffix`, closest in
/// length to `expected` and at most about twice as long.
fn between_context(hay: &str, prefix: &str, suffix: &str, expected: usize) -> Option<Range<usize>> {
    if prefix.is_empty() && suffix.is_empty() {
        return None;
    }
    let starts: Vec<usize> =
        if prefix.is_empty() { vec![0] } else { hay.match_indices(prefix).map(|(i, _)| i + prefix.len()).collect() };
    let ends: Vec<usize> =
        if suffix.is_empty() { vec![hay.len()] } else { hay.match_indices(suffix).map(|(i, _)| i).collect() };
    let max_len = 2 * expected + CONTEXT_CHARS;
    starts
        .iter()
        .filter_map(|&s| {
            let e = *ends.get(ends.partition_point(|&e| e < s))?;
            (e - s <= max_len).then_some(s..e)
        })
        .min_by_key(|r| r.len().abs_diff(expected))
}

fn common_suffix(a: &str, b: &str) -> usize {
    a.chars().rev().zip(b.chars().rev()).take_while(|(x, y)| x == y).count()
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn tail_chars(s: &str, n: usize) -> &str {
    s.char_indices().rev().nth(n.saturating_sub(1)).map_or(s, |(i, _)| &s[i..])
}

fn head_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// Text with every ASCII whitespace run collapsed to one space, mapped back to the source.
struct Normalized {
    text: String,
    /// Source byte offset of each byte of `text`, plus one entry for the end.
    map: Vec<usize>,
}

impl Normalized {
    fn new(src: &str) -> Normalized {
        let mut text = String::with_capacity(src.len());
        let mut map = Vec::with_capacity(src.len() + 1);
        let mut in_space = false;
        for (i, c) in src.char_indices() {
            if c.is_ascii_whitespace() {
                if !in_space {
                    text.push(' ');
                    map.push(i);
                }
                in_space = true;
                continue;
            }
            in_space = false;
            text.push(c);
            map.extend(i..i + c.len_utf8());
        }
        map.push(src.len());
        Normalized { text, map }
    }

    /// Normalised offset of source offset `src` (the first normalised byte at or after it).
    fn to_normalized(&self, src: usize) -> usize {
        self.map.partition_point(|&m| m < src).min(self.text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAPTER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Uno</title></head>
<body>
  <h1 id="c1">Capitolo primo</h1>
  <p>Nel mezzo del <i>cammin</i> di nostra vita</p>
  <p id="p2">mi ritrovai per una selva oscura, 😀 ché la diritta via era smarrita.</p>
</body>
</html>"#;

    fn text_node(dom: &Dom, containing: &str) -> NodeId {
        dom.descendants(Dom::ROOT)
            .find(|&n| matches!(dom.kind(n), NodeKind::Text { text, .. } if text.contains(containing)))
            .expect("text node")
    }

    #[test]
    fn cfi_shape_follows_the_spec() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        // body is html's 2nd element (/4); the 2nd <p> is body's 3rd element (/6); its text
        // is the chunk before its first element child (/1).
        let node = text_node(&dom, "selva");
        let offset = "mi ritrovai ".len();
        assert_eq!(cfi_for(&dom, 2, node, offset), "epubcfi(/6/6!/4/6[p2]/1:12)");
        // "di nostra vita" follows <i>: odd step 3.
        let after_i = text_node(&dom, "di nostra");
        assert_eq!(cfi_for(&dom, 0, after_i, 0), "epubcfi(/6/2!/4/4/3:0)");
        let h1 = dom.find(Dom::ROOT, "h1").expect("h1");
        assert_eq!(cfi_for(&dom, 0, h1, 0), "epubcfi(/6/2!/4/2[c1])");
        assert_eq!(cfi_spine("epubcfi(/6/6!/4/6[p2]/1:12)"), Some(2));
        assert_eq!(cfi_spine("epubcfi(/6/14[ch3]!/4)"), Some(6));
    }

    #[test]
    fn cfi_round_trips_with_utf16_offsets() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        for (needle, at) in [("selva", "selva"), ("selva", "ché"), ("cammin", "mm"), ("Nel", "mezzo"), ("Capitolo", "")]
        {
            let node = text_node(&dom, needle);
            let NodeKind::Text { text, .. } = dom.kind(node) else { unreachable!() };
            let offset = if at.is_empty() { text.len() } else { text.find(at).expect("in text") };
            let cfi = cfi_for(&dom, 3, node, offset);
            assert_eq!(resolve_cfi(&dom, &cfi), Some((node, offset)), "{cfi}");
        }
        // The emoji is two UTF-16 units: "ché" sits two units past the emoji's start.
        let node = text_node(&dom, "selva");
        let NodeKind::Text { text, .. } = dom.kind(node) else { unreachable!() };
        let emoji = text.find('😀').expect("emoji");
        let units = text[..emoji].encode_utf16().count();
        let cfi = cfi_for(&dom, 0, node, text.find("ché").expect("ché"));
        assert!(cfi.ends_with(&format!(":{})", units + 3)), "{cfi}");
    }

    #[test]
    fn id_assertion_survives_inserted_markup() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        let cfi = cfi_for(&dom, 0, text_node(&dom, "selva"), 3);
        let edited = CHAPTER.replace("<h1 id=\"c1\">", "<div>new</div><h1 id=\"c1\">");
        let dom2 = Dom::parse_xml(&edited).expect("parses");
        assert_eq!(resolve_cfi(&dom2, &cfi), Some((text_node(&dom2, "selva"), 3)));
    }

    #[test]
    fn foreign_and_bad_cfis() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        // Text assertion and side bias after the offset are ignored.
        let node = text_node(&dom, "selva");
        assert_eq!(resolve_cfi(&dom, "epubcfi(/6/4[ch1]!/4/6/1:3[rit,ro];s=a)"), Some((node, 3)));
        assert_eq!(resolve_cfi(&dom, "epubcfi(/6/4!/4/6/1:999)"), None);
        assert_eq!(resolve_cfi(&dom, "epubcfi(/6/4!/4/40)"), None);
        assert_eq!(resolve_cfi(&dom, "/4/6/1:3"), None);
        assert_eq!(resolve_cfi(&dom, "epubcfi(/6/4!/4/6/1/2)"), None);
        let escaped = Dom::parse_xml(r#"<html><body><p id="a[1]">x</p></body></html>"#).expect("parses");
        let p = escaped.find(Dom::ROOT, "p").expect("p");
        let cfi = cfi_for(&escaped, 0, p, 0);
        assert_eq!(cfi, "epubcfi(/6/2!/2/2[a^[1^]])");
        assert_eq!(resolve_cfi(&escaped, &cfi), Some((p, 0)));
    }

    #[test]
    fn anchors_sort_in_reading_order() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        let a = Anchor::new(&dom, 1, text_node(&dom, "Nel"), 4);
        let b = Anchor::new(&dom, 1, text_node(&dom, "cammin"), 0);
        let c = Anchor::new(&dom, 1, dom.find(Dom::ROOT, "p").and_then(|p| dom.next_sibling(p)).expect("x"), 0);
        assert!(a.pos < b.pos && b.pos < c.pos, "{a:?} {b:?} {c:?}");
        assert_eq!(&CHAPTER[a.pos as usize..][..5], "mezzo");
    }

    #[test]
    fn text_map_round_trips() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        let map = TextMap::new(&dom);
        assert!(!map.text().contains("Uno"), "head text is not chapter text");
        let pos = map.text().find("cammin").expect("in text");
        let (node, off) = map.to_dom(pos).expect("maps");
        assert_eq!((node, off), (text_node(&dom, "cammin"), 0));
        assert_eq!(map.to_text(node, off), Some(pos));
        assert_eq!(map.to_dom(map.text().len() + 1), None);
        // At a node boundary the end mapping stays in the earlier node.
        let (prev, len) = map.to_dom(pos - 1).map(|(n, o)| (n, o + 1)).expect("maps");
        assert_ne!(prev, node);
        assert_eq!(map.to_dom_end(pos), Some((prev, len)));
        assert_eq!(map.to_dom_end(0), map.to_dom(0));
    }

    fn quote_of(text: &str, exact: &str) -> TextQuote {
        let start = text.find(exact).expect("in text");
        TextQuote::new(text, start..start + exact.len())
    }

    #[test]
    fn quote_collapses_whitespace_and_keeps_context() {
        let text = "alpha\n   beta gamma   delta epsilon zeta eta theta iota kappa lambda";
        let q = quote_of(text, "gamma   delta");
        assert_eq!(q.exact, "gamma delta");
        assert_eq!(q.prefix, "alpha beta ");
        assert_eq!(q.suffix, " epsilon zeta eta theta iota kap");
        assert_eq!(q.suffix.chars().count(), CONTEXT_CHARS);
    }

    #[test]
    fn reanchors_after_insertions() {
        let dom = Dom::parse_xml(CHAPTER).expect("parses");
        let text = TextMap::new(&dom).text().to_owned();
        let q = quote_of(&text, "selva oscura");

        // Text inserted before: same words, shifted.
        let moved = text.replace("Capitolo primo", "Capitolo primo, con una lunga premessa");
        let r = reanchor(&moved, &q).expect("found");
        assert_eq!(&moved[r], "selva oscura");

        // Reflowed markup: whitespace differences do not matter.
        let reflowed = text.replace("selva oscura", "selva\n      oscura");
        let r = reanchor(&reflowed, &q).expect("found");
        assert_eq!(&reflowed[r], "selva\n      oscura");

        // The quoted text itself was edited: the context brackets the new text.
        let edited = text.replace("selva oscura", "selva molto oscura");
        let r = reanchor(&edited, &q).expect("found");
        assert_eq!(&edited[r], "selva molto oscura");

        // Edited inside the quote and in the far half of each context.
        let both = edited.replace("mi ritrovai", "io mi ritrovai").replace("smarrita", "perduta");
        let r = reanchor(&both, &q).expect("found");
        assert_eq!(&both[r], "selva molto oscura");

        // Gone entirely.
        assert_eq!(reanchor("tutt'altro testo", &q), None);
    }

    #[test]
    fn repeated_text_is_disambiguated_by_context() {
        let text = "la rosa è rossa. Poi la rosa appassì. Infine la rosa cadde.";
        let start = text.find("Poi la rosa").expect("x") + "Poi ".len();
        let q = TextQuote::new(text, start..start + "la rosa".len());
        let inserted = format!("Prima la rosa nacque. {text}");
        let r = reanchor(&inserted, &q).expect("found");
        assert_eq!(r.start, inserted.find("Poi la rosa").expect("x") + "Poi ".len());
    }
}
