//! Arena document tree for XHTML chapters and EPUB package files.
//!
//! Names are local (namespace prefixes stripped). Text nodes remember their byte
//! offset in the source file, the anchor for stable reading positions.

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::XmlVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

#[derive(Debug)]
pub enum NodeKind {
    Document,
    Element { name: Box<str>, attrs: Vec<(Box<str>, Box<str>)> },
    Text { text: Box<str>, offset: u32 },
}

#[derive(Debug)]
struct Node {
    kind: NodeKind,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    next_sibling: Option<NodeId>,
}

#[derive(Debug)]
pub struct Dom {
    nodes: Vec<Node>,
}

#[derive(Debug, thiserror::Error)]
#[error("XML error at byte {position}: {message}")]
pub struct ParseError {
    pub position: u64,
    pub message: String,
}

impl Dom {
    pub const ROOT: NodeId = NodeId(0);

    fn new() -> Dom {
        let root =
            Node { kind: NodeKind::Document, parent: None, first_child: None, last_child: None, next_sibling: None };
        Dom { nodes: vec![root] }
    }

    fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    fn append(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node { kind, parent: Some(parent), first_child: None, last_child: None, next_sibling: None });
        match self.node(parent).last_child {
            Some(last) => self.nodes[last.0 as usize].next_sibling = Some(id),
            None => self.nodes[parent.0 as usize].first_child = Some(id),
        }
        self.nodes[parent.0 as usize].last_child = Some(id);
        id
    }

    pub fn kind(&self, id: NodeId) -> &NodeKind {
        &self.node(id).kind
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).next_sibling
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.node(id).first_child, |&c| self.node(c).next_sibling)
    }

    /// All nodes below `id`, in document order.
    pub fn descendants(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        // Nodes are appended in document order, so a subtree is a contiguous index range.
        let end = self.subtree_end(id);
        (id.0 + 1..end).map(NodeId)
    }

    fn subtree_end(&self, mut id: NodeId) -> u32 {
        loop {
            if let Some(next) = self.node(id).next_sibling {
                return next.0;
            }
            match self.node(id).parent {
                Some(p) => id = p,
                None => return self.nodes.len() as u32,
            }
        }
    }

    pub fn name(&self, id: NodeId) -> Option<&str> {
        match &self.node(id).kind {
            NodeKind::Element { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn attr(&self, id: NodeId, key: &str) -> Option<&str> {
        match &self.node(id).kind {
            NodeKind::Element { attrs, .. } => attrs.iter().find(|(k, _)| &**k == key).map(|(_, v)| &**v),
            _ => None,
        }
    }

    /// First element named `name` in the subtree (including `id` itself).
    pub fn find(&self, id: NodeId, name: &str) -> Option<NodeId> {
        std::iter::once(id).chain(self.descendants(id)).find(|&n| self.name(n) == Some(name))
    }

    pub fn find_all<'a>(&'a self, id: NodeId, name: &'a str) -> impl Iterator<Item = NodeId> + 'a {
        self.descendants(id).filter(move |&n| self.name(n) == Some(name))
    }

    /// Concatenated text of the subtree, ASCII whitespace collapsed (no-break spaces kept).
    pub fn text(&self, id: NodeId) -> String {
        let mut s = String::new();
        for n in std::iter::once(id).chain(self.descendants(id)) {
            if let NodeKind::Text { text, .. } = &self.node(n).kind {
                s.push_str(text);
            }
        }
        s.split(|c: char| c.is_ascii_whitespace()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
    }

    /// Parses well-formed XML/XHTML. HTML named entities beyond XML's five
    /// (`&nbsp;`, `&mdash;`…) are resolved too, since EPUBs use them without a DTD.
    pub fn parse_xml(src: &str) -> Result<Dom, ParseError> {
        let mut dom = Dom::new();
        let mut reader = Reader::from_str(src);
        let mut stack = vec![Dom::ROOT];
        let err = |reader: &Reader<&[u8]>, e: &dyn std::fmt::Display| ParseError {
            position: reader.error_position(),
            message: e.to_string(),
        };
        loop {
            let offset = reader.buffer_position() as u32;
            let event = reader.read_event().map_err(|e| err(&reader, &e))?;
            let top = *stack.last().expect("root is never popped");
            match event {
                Event::Start(e) => {
                    let kind = element(&e).map_err(|e| err(&reader, &e))?;
                    stack.push(dom.append(top, kind));
                }
                Event::Empty(e) => {
                    let kind = element(&e).map_err(|e| err(&reader, &e))?;
                    dom.append(top, kind);
                }
                Event::End(_) => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                }
                Event::Text(t) => push_text(&mut dom, top, &t.xml_content(XmlVersion::Implicit1_0), offset),
                Event::CData(t) => push_text(&mut dom, top, &t.into_inner(), offset),
                Event::GeneralRef(r) => {
                    let name = r.xml_content(XmlVersion::Implicit1_0);
                    let resolved =
                        resolve_entity(&name).ok_or_else(|| err(&reader, &format!("unknown entity &{name};")))?;
                    push_text(&mut dom, top, &resolved, offset);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(dom)
    }
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

fn element(e: &BytesStart) -> Result<NodeKind, quick_xml::Error> {
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false) {
        let a = a.map_err(quick_xml::Error::from)?;
        let key = a.key.as_ref();
        // Keep epub:type and xml:lang distinguishable; drop other namespace declarations.
        let key = match key {
            "epub:type" | "xml:lang" => key,
            k if k.starts_with("xmlns") => continue,
            k => local(k),
        };
        attrs.push((key.into(), a.normalized_value_with(XmlVersion::Implicit1_0, 1, named_entity)?.into()));
    }
    Ok(NodeKind::Element { name: local(e.name().as_ref()).into(), attrs })
}

/// Appends text, merging with a preceding text sibling (entities split text into events).
fn push_text(dom: &mut Dom, parent: NodeId, text: &str, offset: u32) {
    if let Some(last) = dom.node(parent).last_child {
        if let NodeKind::Text { text: prev, .. } = &mut dom.nodes[last.0 as usize].kind {
            let mut merged = String::with_capacity(prev.len() + text.len());
            merged.push_str(prev);
            merged.push_str(text);
            *prev = merged.into();
            return;
        }
    }
    dom.append(parent, NodeKind::Text { text: text.into(), offset });
}

/// Numeric references plus [`named_entity`].
fn resolve_entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code).map(String::from);
    }
    named_entity(name).map(String::from)
}

/// XML's predefined entities and the HTML entities common in books.
// ponytail: common subset; add more names when a real book needs them.
fn named_entity(name: &str) -> Option<&'static str> {
    Some(match name {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => "\u{a0}",
        "shy" => "\u{ad}",
        "ensp" => "\u{2002}",
        "emsp" => "\u{2003}",
        "thinsp" => "\u{2009}",
        "zwnj" => "\u{200c}",
        "zwj" => "\u{200d}",
        "ndash" => "–",
        "mdash" => "—",
        "lsquo" => "‘",
        "rsquo" => "’",
        "sbquo" => "‚",
        "ldquo" => "“",
        "rdquo" => "”",
        "bdquo" => "„",
        "laquo" => "«",
        "raquo" => "»",
        "hellip" => "…",
        "bull" => "•",
        "middot" => "·",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "times" => "×",
        "divide" => "÷",
        "sect" => "§",
        "para" => "¶",
        "dagger" => "†",
        "Dagger" => "‡",
        "prime" => "′",
        "Prime" => "″",
        "euro" => "€",
        "pound" => "£",
        "agrave" => "à",
        "egrave" => "è",
        "eacute" => "é",
        "igrave" => "ì",
        "ograve" => "ò",
        "ugrave" => "ù",
        "Agrave" => "À",
        "Egrave" => "È",
        "Eacute" => "É",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xhtml_with_namespaces_entities_and_empty_tags() {
        let src = r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="it">
<body><p class="a" epub:type="x">Caff&egrave;&nbsp;&amp; <a id="n1"/>t&#233;<br/>fine</p></body></html>"#;
        let dom = Dom::parse_xml(src).expect("parses");
        let html = dom.find(Dom::ROOT, "html").expect("html");
        assert_eq!(dom.attr(html, "xml:lang"), Some("it"));
        assert_eq!(dom.attr(html, "xmlns"), None);
        let p = dom.find(Dom::ROOT, "p").expect("p");
        assert_eq!((dom.attr(p, "class"), dom.attr(p, "epub:type")), (Some("a"), Some("x")));
        // <a/> is empty: "té" stays a sibling, not a child of <a>.
        let kids: Vec<_> = dom.children(p).map(|c| dom.name(c).unwrap_or("#text")).collect();
        assert_eq!(kids, ["#text", "a", "#text", "br", "#text"]);
        assert_eq!(dom.text(p), "Caffè\u{a0}& téfine"); // <br/> adds no text
    }

    #[test]
    fn text_offsets_point_into_source() {
        let src = "<p>ab<i>cd</i></p>";
        let dom = Dom::parse_xml(src).expect("parses");
        let offsets: Vec<_> = dom
            .descendants(Dom::ROOT)
            .filter_map(|n| match dom.kind(n) {
                NodeKind::Text { text, offset } => Some((text.to_string(), *offset)),
                _ => None,
            })
            .collect();
        assert_eq!(offsets, [("ab".to_string(), 3), ("cd".to_string(), 8)]);
        assert_eq!(&src[3..5], "ab");
    }

    #[test]
    fn descendants_stay_inside_subtree() {
        let dom = Dom::parse_xml("<r><a><b/><c/></a><d/></r>").expect("parses");
        let a = dom.find(Dom::ROOT, "a").expect("a");
        let names: Vec<_> = dom.descendants(a).filter_map(|n| dom.name(n)).collect();
        assert_eq!(names, ["b", "c"]);
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(Dom::parse_xml("<p>&bogus;</p>").is_err());
        assert!(Dom::parse_xml("<p><b></p>").is_err());
    }
}
