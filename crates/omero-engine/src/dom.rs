//! Arena document tree for XHTML chapters and EPUB package files.
//!
//! Names are local (namespace prefixes stripped). Text nodes remember their byte
//! offset in the source file, the anchor for stable reading positions.

use quick_xml::escape::resolve_html5_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::XmlVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    /// Dense arena index, for side tables indexed by node (e.g. computed styles).
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug)]
pub enum NodeKind {
    Document,
    Element {
        name: Box<str>,
        attrs: Vec<(Box<str>, Box<str>)>,
    },
    /// `String` so entity-split runs grow in place while parsing.
    Text {
        text: String,
        offset: u32,
    },
}

#[derive(Debug)]
struct Node {
    kind: NodeKind,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
}

impl Node {
    fn new(kind: NodeKind, parent: Option<NodeId>, prev_sibling: Option<NodeId>) -> Node {
        Node { kind, parent, first_child: None, last_child: None, prev_sibling, next_sibling: None }
    }
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
        Dom { nodes: vec![Node::new(NodeKind::Document, None, None)] }
    }

    /// Number of nodes, document node included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the document node has no children (`len()` still counts the document node).
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    fn append(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        let prev = self.node(parent).last_child;
        self.nodes.push(Node::new(kind, Some(parent), prev));
        match prev {
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

    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).prev_sibling
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
        let mut out = String::new();
        let mut pending_space = false;
        for n in std::iter::once(id).chain(self.descendants(id)) {
            let NodeKind::Text { text, .. } = &self.node(n).kind else { continue };
            for (i, word) in text.split(|c: char| c.is_ascii_whitespace()).enumerate() {
                pending_space |= i > 0;
                if word.is_empty() {
                    continue;
                }
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push_str(word);
            }
        }
        out
    }

    /// Parses well-formed XML/XHTML. All HTML5 named entities (`&nbsp;`, `&mdash;`…) are
    /// resolved, since EPUBs use them without a DTD. A leading BOM is skipped; text
    /// offsets still count from the start of `src`.
    pub fn parse_xml(src: &str) -> Result<Dom, ParseError> {
        let mut dom = Dom::new();
        let body = src.trim_start_matches('\u{feff}');
        let bom = (src.len() - body.len()) as u32;
        let mut reader = Reader::from_str(body);
        let mut cur = Dom::ROOT;
        let err = |reader: &Reader<&[u8]>, e: &dyn std::fmt::Display| ParseError {
            position: reader.error_position(),
            message: e.to_string(),
        };
        loop {
            let offset = bom + reader.buffer_position() as u32;
            let event = reader.read_event().map_err(|e| err(&reader, &e))?;
            match event {
                Event::Start(e) => {
                    let kind = element(&e).map_err(|e| err(&reader, &e))?;
                    cur = dom.append(cur, kind);
                }
                Event::Empty(e) => {
                    let kind = element(&e).map_err(|e| err(&reader, &e))?;
                    dom.append(cur, kind);
                }
                Event::End(_) => cur = dom.parent(cur).unwrap_or(Dom::ROOT),
                Event::Text(t) => push_text(&mut dom, cur, &t.xml_content(XmlVersion::Implicit1_0), offset),
                Event::CData(t) => push_text(&mut dom, cur, &t.into_inner(), offset),
                Event::GeneralRef(r) => {
                    let mut utf8 = [0; 4];
                    let text = match r.resolve_char_ref().map_err(|e| err(&reader, &e))? {
                        Some(c) => &*c.encode_utf8(&mut utf8),
                        None => {
                            let name = r.xml_content(XmlVersion::Implicit1_0);
                            resolve_html5_entity(&name)
                                .ok_or_else(|| err(&reader, &format!("unknown entity &{name};")))?
                        }
                    };
                    push_text(&mut dom, cur, text, offset);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(dom)
    }
}

fn element(e: &BytesStart) -> Result<NodeKind, quick_xml::Error> {
    let mut attrs = Vec::new();
    for a in e.attributes().with_checks(false) {
        let a = a.map_err(quick_xml::Error::from)?;
        // Keep epub:type and xml:lang distinguishable; drop namespace declarations.
        // ponytail: matches the conventional `epub:` prefix only, not other prefixes bound to the OPS namespace.
        let key = match a.key.as_ref() {
            k @ ("epub:type" | "xml:lang") => k,
            k if k.starts_with("xmlns") => continue,
            _ => a.key.local_name().into_inner(),
        };
        attrs.push((key.into(), a.normalized_value_with(XmlVersion::Implicit1_0, 1, resolve_html5_entity)?.into()));
    }
    Ok(NodeKind::Element { name: e.name().local_name().into_inner().into(), attrs })
}

/// Appends text, merging with a preceding text sibling (entities split text into events).
fn push_text(dom: &mut Dom, parent: NodeId, text: &str, offset: u32) {
    if let Some(last) = dom.node(parent).last_child {
        if let NodeKind::Text { text: prev, .. } = &mut dom.nodes[last.0 as usize].kind {
            prev.push_str(text);
            return;
        }
    }
    dom.append(parent, NodeKind::Text { text: text.into(), offset });
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
        assert!(Dom::parse_xml("<p>&ouml;&rarr;</p>").is_ok(), "full HTML5 entity table");
        assert!(Dom::parse_xml("<p><b></p>").is_err());
    }
}
