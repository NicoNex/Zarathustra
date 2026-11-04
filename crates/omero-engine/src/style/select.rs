//! Glue between our [`Dom`] and Servo's `selectors` crate: full CSS selector grammar
//! with correct specificity, matched against book content.

use std::borrow::Borrow;
use std::fmt;
use std::hash::{Hash, Hasher};

use cssparser::{serialize_identifier, ParseError, ToCss};
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::MatchingContext;
use selectors::matching::ElementSelectorFlags;
use selectors::parser::{NonTSPseudoClass, SelectorParseErrorKind};
use selectors::{OpaqueElement, SelectorImpl};

use crate::dom::{Dom, NodeId, NodeKind};

/// Interned-ish string; hash precomputed once at parse time.
#[derive(Clone, Debug, Default, Eq)]
pub struct Atom {
    s: Box<str>,
    hash: u32,
}

impl PartialEq for Atom {
    fn eq(&self, o: &Self) -> bool {
        self.s == o.s
    }
}

impl Hash for Atom {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.s.hash(h)
    }
}

impl From<&str> for Atom {
    fn from(s: &str) -> Atom {
        // FNV-1a: cheap and good enough for the selector bloom filter.
        let hash = s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
        Atom { s: s.into(), hash }
    }
}

impl AsRef<str> for Atom {
    fn as_ref(&self) -> &str {
        &self.s
    }
}

impl Borrow<str> for Atom {
    fn borrow(&self) -> &str {
        &self.s
    }
}

impl ToCss for Atom {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        serialize_identifier(&self.s, dest)
    }
}

impl precomputed_hash::PrecomputedHash for Atom {
    fn precomputed_hash(&self) -> u32 {
        self.hash
    }
}

/// Pseudo-classes and -elements a static book can never be in (`:hover`, `::before`…)
/// parse as these so the rule is kept but never matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Never;

impl ToCss for Never {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(":never")
    }
}

impl NonTSPseudoClass for Never {
    fn is_active_or_hover(&self) -> bool {
        false
    }
    fn is_user_action_state(&self) -> bool {
        false
    }
}

impl selectors::parser::PseudoElement for Never {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookSelectors;

impl SelectorImpl for BookSelectors {
    type ExtraMatchingData<'a> = std::marker::PhantomData<&'a ()>;
    type AttrValue = Atom;
    type Identifier = Atom;
    type LocalName = Atom;
    type NamespaceUrl = Atom;
    type NamespacePrefix = Atom;
    type BorrowedLocalName = str;
    type BorrowedNamespaceUrl = str;
    type NonTSPseudoClass = Never;
    type PseudoElement = Never;
}

pub type SelectorList = selectors::SelectorList<BookSelectors>;

pub struct BookParser;

impl<'i> selectors::parser::Parser<'i> for BookParser {
    type Impl = BookSelectors;
    type Error = SelectorParseErrorKind;

    fn parse_non_ts_pseudo_class(&self, _name: &str) -> Result<Never, ParseError<Self::Error>> {
        Ok(Never)
    }

    fn parse_pseudo_element(&self, _name: &str) -> Result<Never, ParseError<Self::Error>> {
        Ok(Never)
    }
}

/// An element of a [`Dom`] as seen by the selector matcher.
#[derive(Clone, Copy, Debug)]
pub struct El<'a> {
    pub dom: &'a Dom,
    pub id: NodeId,
}

impl<'a> El<'a> {
    fn wrap(&self, id: Option<NodeId>) -> Option<Self> {
        id.map(|id| El { dom: self.dom, id })
    }

    fn element_sibling(&self, mut next: impl FnMut(NodeId) -> Option<NodeId>) -> Option<Self> {
        let mut cur = next(self.id);
        while let Some(n) = cur {
            if self.dom.name(n).is_some() {
                return self.wrap(Some(n));
            }
            cur = next(n);
        }
        None
    }

    fn attr(&self, name: &str) -> Option<&'a str> {
        self.dom.attr(self.id, name)
    }
}

impl selectors::Element for El<'_> {
    type Impl = BookSelectors;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.dom.kind(self.id))
    }

    fn parent_element(&self) -> Option<Self> {
        self.wrap(self.dom.parent(self.id).filter(|&p| self.dom.name(p).is_some()))
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.element_sibling(|n| self.dom.prev_sibling(n))
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.element_sibling(|n| self.dom.next_sibling(n))
    }

    fn first_element_child(&self) -> Option<Self> {
        self.wrap(self.dom.children(self.id).find(|&c| self.dom.name(c).is_some()))
    }

    fn is_html_element_in_html_document(&self) -> bool {
        // Book XHTML: type selectors match ASCII case-insensitively, like HTML.
        true
    }

    fn has_local_name(&self, name: &str) -> bool {
        self.dom.name(self.id).is_some_and(|n| n.eq_ignore_ascii_case(name))
    }

    fn has_namespace(&self, _ns: &str) -> bool {
        // ponytail: namespaces are stripped from the DOM; `svg|rect` matches any `rect`.
        true
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.dom.name(self.id) == other.dom.name(other.id)
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&Atom>,
        local_name: &Atom,
        operation: &AttrSelectorOperation<&Atom>,
    ) -> bool {
        self.attr(local_name.as_ref()).is_some_and(|v| operation.eval_str(v))
    }

    fn match_non_ts_pseudo_class(&self, _pc: &Never, _context: &mut MatchingContext<BookSelectors>) -> bool {
        false
    }

    fn match_pseudo_element(&self, _pe: &Never, _context: &mut MatchingContext<BookSelectors>) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        self.dom.name(self.id) == Some("a") && self.attr("href").is_some()
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &Atom, case: CaseSensitivity) -> bool {
        self.attr("id").is_some_and(|v| case.eq(v.as_bytes(), id.as_ref().as_bytes()))
    }

    fn has_class(&self, name: &Atom, case: CaseSensitivity) -> bool {
        self.attr("class")
            .is_some_and(|v| v.split_ascii_whitespace().any(|c| case.eq(c.as_bytes(), name.as_ref().as_bytes())))
    }

    fn has_custom_state(&self, _name: &Atom) -> bool {
        false
    }

    fn imported_part(&self, _name: &Atom) -> Option<Atom> {
        None
    }

    fn is_part(&self, _name: &Atom) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.dom.children(self.id).all(|c| match self.dom.kind(c) {
            NodeKind::Text { text, .. } => text.is_empty(),
            _ => false,
        })
    }

    fn is_root(&self) -> bool {
        self.dom.parent(self.id).is_some_and(|p| self.dom.name(p).is_none())
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}

/// Parses a selector list such as `p.first, blockquote > p`.
pub fn parse(css: &str) -> Option<SelectorList> {
    let mut parser = cssparser::Parser::new(css);
    SelectorList::parse(&BookParser, &mut parser, selectors::parser::ParseRelative::No).ok()
}

/// Specificity of each selector in the list, in the order `matches` reports.
pub fn matches(list: &SelectorList, el: El, caches: &mut selectors::context::SelectorCaches) -> Option<u32> {
    let mut ctx = MatchingContext::new(
        selectors::matching::MatchingMode::Normal,
        None,
        caches,
        selectors::context::QuirksMode::NoQuirks,
        selectors::matching::NeedsSelectorFlags::No,
        selectors::matching::MatchingForInvalidation::No,
    );
    list.slice()
        .iter()
        .filter(|s| selectors::matching::matches_selector(s, 0, None, el, &mut ctx))
        .map(|s| s.specificity())
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el_matching(dom: &Dom, sel: &str) -> Vec<String> {
        let list = parse(sel).expect("valid selector");
        let mut caches = Default::default();
        dom.descendants(Dom::ROOT)
            .filter(|&n| dom.name(n).is_some())
            .filter(|&n| matches(&list, El { dom, id: n }, &mut caches).is_some())
            .map(|n| dom.attr(n, "id").unwrap_or("?").to_string())
            .collect()
    }

    #[test]
    fn matches_full_selector_grammar() {
        let dom = Dom::parse_xml(
            r#"<html><body>
            <div id="d" class="chapter"><p id="p1" class="first big">a</p><p id="p2">b</p>
              <blockquote id="q"><p id="p3" lang="it">c</p></blockquote></div>
            <p id="p4" data-x="note-12">d</p></body></html>"#,
        )
        .expect("parses");
        assert_eq!(el_matching(&dom, "p"), ["p1", "p2", "p3", "p4"]);
        assert_eq!(el_matching(&dom, ".chapter > p"), ["p1", "p2"]);
        assert_eq!(el_matching(&dom, "div p"), ["p1", "p2", "p3"]);
        assert_eq!(el_matching(&dom, "p.first.big + p"), ["p2"]);
        assert_eq!(el_matching(&dom, "p:first-child"), ["p1", "p3"]);
        assert_eq!(el_matching(&dom, "p:not(.first):last-child"), ["p3", "p4"]);
        assert_eq!(el_matching(&dom, "[data-x^=note]"), ["p4"]);
        assert_eq!(el_matching(&dom, "P#P2"), Vec::<String>::new(), "ids are case-sensitive");
        assert_eq!(el_matching(&dom, "P"), ["p1", "p2", "p3", "p4"], "type selectors are not");
        assert_eq!(el_matching(&dom, "p:hover, a::before"), Vec::<String>::new());
    }

    #[test]
    fn reports_highest_specificity() {
        let dom = Dom::parse_xml(r#"<p id="x" class="c">t</p>"#).expect("parses");
        let p = dom.find(Dom::ROOT, "p").expect("p");
        let mut caches = Default::default();
        let mut spec = |s| matches(&parse(s).expect("valid"), El { dom: &dom, id: p }, &mut caches);
        assert!(spec("#x").expect("matches") > spec(".c").expect("matches"));
        assert!(spec(".c").expect("matches") > spec("p").expect("matches"));
        assert_eq!(spec("p, #x"), spec("#x"));
    }
}
