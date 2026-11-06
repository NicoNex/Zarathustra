//! Footnotes: which links are note references, which element holds a note, and what of it
//! to show in a popup.
//!
//! Marked-up books say so (`epub:type="noteref"`, `role="doc-noteref"`, notes in `aside`s or
//! sections typed `footnote`/`endnote`/`rearnote`). Most books don't: there a note reference
//! is a link whose text is a short marker (`12`, `[3]`, `*`, `†`), usually in `<sup>`.

use crate::dom::{Dom, NodeId};
use crate::epub::resolve;
use crate::style::cascade::ComputedStyle;
use crate::style::css::Display;

/// Whitespace-separated tokens of `epub:type` and `role`.
fn semantics(dom: &Dom, n: NodeId) -> impl Iterator<Item = &str> {
    let types = dom.attr(n, "epub:type").into_iter().chain(dom.attr(n, "role"));
    types.flat_map(str::split_whitespace)
}

fn has(dom: &Dom, n: NodeId, tokens: &[&str]) -> bool {
    semantics(dom, n).any(|t| tokens.contains(&t.strip_prefix("doc-").unwrap_or(t)))
}

/// Text of a typical note marker: digits (plain or superscript) or reference symbols, in
/// optional brackets and with an optional trailing period.
pub fn is_marker(text: &str) -> bool {
    let t = text.trim().trim_matches(|c| matches!(c, '[' | ']' | '(' | ')' | '{' | '}')).trim_end_matches('.');
    let digit = |c: char| c.is_ascii_digit() || "⁰¹²³⁴⁵⁶⁷⁸⁹".contains(c);
    let symbol = |c: char| "*†‡§¶#‖※⁂".contains(c);
    !t.is_empty() && t.chars().count() <= 5 && (t.chars().all(digit) || t.chars().all(symbol))
}

/// The `<a href>` element at or around node `n` (a text node inside a link, say).
pub fn link_at(dom: &Dom, n: NodeId) -> Option<NodeId> {
    std::iter::successors(Some(n), |&p| dom.parent(p))
        .find(|&p| dom.name(p) == Some("a") && dom.attr(p, "href").is_some())
}

/// Whether link `a` looks like a note reference: marked as one, or an unmarked link into a
/// fragment whose text is a note marker or a few characters in superscript.
pub fn is_noteref(dom: &Dom, a: NodeId) -> bool {
    if has(dom, a, &["noteref"]) {
        return true;
    }
    if !dom.attr(a, "href").is_some_and(|h| h.contains('#')) {
        return false;
    }
    let text = dom.text(a);
    let sup = |n: NodeId| dom.name(n) == Some("sup");
    let in_sup =
        std::iter::successors(dom.parent(a), |&p| dom.parent(p)).take(2).any(sup) || dom.descendants(a).any(sup);
    is_marker(&text) || (in_sup && !text.trim().is_empty() && text.trim().chars().count() <= 6)
}

/// Whether element `n` or a container of it is typed as a note (`aside epub:type=footnote`,
/// an endnotes section, `role=doc-endnote`…).
pub fn is_marked_note(dom: &Dom, n: NodeId) -> bool {
    const NOTES: &[&str] = &["footnote", "footnotes", "endnote", "endnotes", "rearnote", "rearnotes", "note"];
    std::iter::successors(Some(n), |&p| dom.parent(p)).any(|p| has(dom, p, NOTES))
}

fn style(styles: &[Option<ComputedStyle>], n: NodeId) -> Option<&ComputedStyle> {
    styles.get(n.index()).and_then(Option::as_ref)
}

/// The element whose content is the note that link target `target` points at: the target
/// itself when it is a block (an `aside`, a list item, a paragraph), the block after an empty
/// anchor that precedes it, or the block an inline target (an anchor, a back-link) sits in.
/// `None` when that is a heading or the whole document: the link goes to a chapter or a
/// section, not a note.
pub fn note_root(dom: &Dom, styles: &[Option<ComputedStyle>], target: NodeId) -> Option<NodeId> {
    // Hidden elements count as blocks: books often hide footnote asides from the flow.
    let block =
        |n: NodeId| style(styles, n).is_some_and(|s| !matches!(s.display, Display::Inline | Display::InlineBlock));
    let next_element = std::iter::successors(dom.next_sibling(target), |&s| dom.next_sibling(s))
        .find(|&s| dom.name(s).is_some() || !dom.text(s).trim().is_empty());
    let root = if block(target) {
        target
    } else if let Some(next) = next_element.filter(|&s| block(s) && dom.text(target).trim().is_empty()) {
        next
    } else {
        std::iter::successors(dom.parent(target), |&p| dom.parent(p)).find(|&p| block(p))?
    };
    match dom.name(root)? {
        "html" | "body" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => None,
        _ => Some(root),
    }
}

/// Hides the links in note `root` that lead back to the text (`↩`, the note's own number
/// linking to its reference) so the popup shows the note alone. `ref_id` is the reference's
/// `id`, `ref_path` the chapter it is in, `note_path` the document of the note.
pub fn hide_backlinks(
    dom: &Dom,
    styles: &mut [Option<ComputedStyle>],
    root: NodeId,
    ref_id: Option<&str>,
    ref_path: &str,
    note_path: &str,
) {
    for a in dom.descendants(root).filter(|&n| dom.name(n) == Some("a")) {
        let Some(href) = dom.attr(a, "href") else { continue };
        let text = dom.text(a);
        let target = resolve(note_path, href);
        let (file, fragment) = target.split_once('#').unwrap_or((&target, ""));
        let back = has(dom, a, &["backlink"])
            || matches!(text.trim(), "↩" | "↩\u{fe0e}" | "↑" | "^" | "⏎")
            || (ref_id.is_some_and(|id| id == fragment))
            || (file == ref_path && is_marker(&text));
        if let Some(s) = styles.get_mut(a.index()).and_then(Option::as_mut).filter(|_| back) {
            s.display = Display::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::cascade::{compute, Preferences, Stylesheets};

    fn styled(src: &str) -> (Dom, Vec<Option<ComputedStyle>>) {
        let dom = Dom::parse_xml(src).expect("parses");
        let prefs = Preferences {
            font_size: 20.0,
            font_family: None,
            line_height: None,
            align: None,
            hyphenate: false,
            page_width: 500.0,
        };
        let styles = compute(&dom, &Stylesheets::default(), &prefs);
        (dom, styles)
    }

    fn by_id(dom: &Dom, id: &str) -> NodeId {
        dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id)).expect("id")
    }

    #[test]
    fn markers() {
        for m in ["1", " 12 ", "[3]", "(4)", "5.", "*", "**", "†", "‡", "²"] {
            assert!(is_marker(m), "{m}");
        }
        for m in ["", "Chapter 1", "x", "123456", "1a"] {
            assert!(!is_marker(m), "{m}");
        }
    }

    #[test]
    fn noterefs_marked_and_unmarked() {
        let (dom, _) = styled(
            "<html xmlns:epub='http://www.idpf.org/2007/ops'><body><p>\
             <a id='a' epub:type='noteref' href='#n1'>see note</a> <a id='b' href='n.xhtml#n2'>2</a> \
             <sup><a id='c' href='#n3'>iv</a></sup> <a id='d' href='ch2.xhtml#start'>Chapter 2</a> \
             <a id='e' href='ch2.xhtml'>3</a> <a id='f' role='doc-noteref' href='x'>x</a></p></body></html>",
        );
        let refs: Vec<bool> =
            ["a", "b", "c", "d", "e", "f"].iter().map(|id| is_noteref(&dom, by_id(&dom, id))).collect();
        assert_eq!(refs, [true, true, true, false, false, true]);
        let text = dom.descendants(by_id(&dom, "b")).next().expect("text");
        assert_eq!(link_at(&dom, text), Some(by_id(&dom, "b")));
    }

    #[test]
    fn note_roots() {
        let (dom, styles) = styled(
            "<html xmlns:epub='http://www.idpf.org/2007/ops'><body><h2 id='h'>Notes</h2>\
             <aside epub:type='footnote' id='n1' style='display:none'><p>One.</p></aside>\
             <ol epub:type='endnotes'><li id='n2'>Two.</li></ol>\
             <div class='footnote'><a id='n3' href='c.xhtml#r3'>3</a><p>Three.</p></div>\
             <a id='n4'/><p id='p4'>Four.</p></body></html>",
        );
        let root = |id: &str| note_root(&dom, &styles, by_id(&dom, id)).and_then(|n| dom.name(n));
        assert_eq!(
            [root("n1"), root("n2"), root("n3"), root("n4"), root("h")],
            [Some("aside"), Some("li"), Some("div"), Some("p"), None]
        );
        assert!(is_marked_note(&dom, by_id(&dom, "n1")) && is_marked_note(&dom, by_id(&dom, "n2")));
        assert!(!is_marked_note(&dom, by_id(&dom, "n3")));
    }

    #[test]
    fn backlinks_hide() {
        let (dom, mut styles) = styled(
            "<html xmlns:epub='http://www.idpf.org/2007/ops'><body><p>Text<a id='r1' href='#n1'>1</a>.</p>\
             <aside epub:type='footnote' id='n1'><p><a href='#r1'>1</a> Note <a href='http://x.org'>link</a> \
             <a href='#elsewhere'>↩</a></p></aside></body></html>",
        );
        let display = |s: &[Option<ComputedStyle>], n: NodeId| s[n.index()].as_ref().map(|s| s.display);
        let n1 = by_id(&dom, "n1");
        hide_backlinks(&dom, &mut styles, n1, Some("r1"), "c.xhtml", "c.xhtml");
        let links: Vec<_> = dom.find_all(n1, "a").map(|a| display(&styles, a) == Some(Display::None)).collect();
        assert_eq!(links, [true, false, true]);
    }
}
