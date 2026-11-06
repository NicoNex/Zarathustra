//! Turns tag soup into well-formed XML that `Dom::parse_xml` accepts: stray `&`/`<` escaped,
//! attributes quoted and deduplicated, void elements self-closed, unmatched end tags dropped and
//! unclosed elements closed at the end. Comments, doctypes and processing instructions are dropped.
//!
//! Well-formed XHTML (KF8 parts) passes through with its structure unchanged.

use quick_xml::escape::{resolve_html5_entity, resolve_xml_entity};

/// Rewrites one attribute: given (name, value) returns a replacement (name, value), or `None`
/// to keep it.
pub type AttrMap<'a> = &'a dyn Fn(&str, &str) -> Option<(&'static str, String)>;

/// `html`: the input is HTML-ish (MOBI6): names are lowercased, an open `<p>` is closed by the
/// next block, and `html`/`head`/`body` and prefixed (`mbp:…`) tags are dropped, keeping their content.
pub fn fix(src: &str, html: bool, map: Option<AttrMap>) -> String {
    let mut f = Fixer { out: String::with_capacity(src.len() + src.len() / 16), stack: Vec::new(), html, map };
    let mut rest = src;
    while let Some(lt) = rest.find('<') {
        f.text(&rest[..lt]);
        rest = f.markup(&rest[lt..]);
    }
    f.text(rest);
    while let Some(name) = f.stack.pop() {
        f.close(&name);
    }
    f.out
}

struct Fixer<'a> {
    out: String,
    stack: Vec<String>,
    html: bool,
    map: Option<AttrMap<'a>>,
}

const VOID: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
/// Blocks that implicitly close an open `<p>` in HTML.
const BLOCKS: &[&str] = &[
    "address",
    "blockquote",
    "center",
    "div",
    "dl",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "ol",
    "p",
    "pre",
    "table",
    "ul",
];

impl Fixer<'_> {
    /// Handles the markup at the start of `s` (which begins with `<`); returns the rest.
    fn markup<'s>(&mut self, s: &'s str) -> &'s str {
        let after = |pat: &str, from: usize| s[from..].find(pat).map_or("", |i| &s[from + i + pat.len()..]);
        if s.starts_with("<!--") {
            return after("-->", 4);
        }
        if let Some(body) = s.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            self.out.push_str("<![CDATA[");
            self.out.push_str(&body[..end]);
            self.out.push_str("]]>");
            return body.get(end + 3..).unwrap_or("");
        }
        if s.starts_with("<!") || s.starts_with("<?") {
            return after(">", 2);
        }
        let closing = s.as_bytes().get(1) == Some(&b'/');
        let name_at = if closing { 2 } else { 1 };
        let starts_name = s.as_bytes().get(name_at).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_');
        let Some(end) = tag_end(s).filter(|_| starts_name) else {
            self.out.push_str("&lt;");
            return &s[1..];
        };
        let inner = &s[name_at..end];
        let name_len = inner.find(|c: char| c.is_ascii_whitespace() || c == '/').unwrap_or(inner.len());
        let raw_name = &inner[..name_len];
        if !is_name(raw_name) {
            self.out.push_str("&lt;");
            return &s[1..];
        }
        let name = if self.html { raw_name.to_ascii_lowercase() } else { raw_name.to_string() };
        let rest = &s[end + 1..];
        if self.html && (name.contains(':') || matches!(name.as_str(), "html" | "head" | "body")) {
            return rest;
        }
        if closing {
            if let Some(pos) = self.stack.iter().rposition(|n| n.eq_ignore_ascii_case(&name)) {
                for n in self.stack.split_off(pos).into_iter().rev() {
                    self.close(&n);
                }
            }
            return rest;
        }
        let attrs = inner[name_len..].trim_end();
        let (attrs, self_closing) = match attrs.strip_suffix('/') {
            Some(a) => (a, true),
            None => (attrs, false),
        };
        if self.html {
            let top = self.stack.last().map(String::as_str);
            if (top == Some("p") && BLOCKS.contains(&name.as_str())) || (top == Some("li") && name == "li") {
                if let Some(n) = self.stack.pop() {
                    self.close(&n);
                }
            }
        }
        self.out.push('<');
        self.out.push_str(&name);
        self.attrs(attrs);
        if self_closing || VOID.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
            self.out.push_str("/>");
        } else {
            self.out.push('>');
            self.stack.push(name);
        }
        rest
    }

    fn close(&mut self, name: &str) {
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
    }

    fn attrs(&mut self, mut s: &str) {
        let mut seen: Vec<String> = Vec::new();
        loop {
            s = s.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
            if s.is_empty() {
                return;
            }
            let name_len = s.find(|c: char| c.is_ascii_whitespace() || c == '=').unwrap_or(s.len());
            let name = &s[..name_len];
            s = s[name_len..].trim_start();
            let mut value = None;
            if let Some(v) = s.strip_prefix('=') {
                let v = v.trim_start();
                let (val, rest) = match v.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let body = &v[1..];
                        let end = body.find(q).unwrap_or(body.len());
                        (&body[..end], body.get(end + 1..).unwrap_or(""))
                    }
                    _ => {
                        let end = v.find(|c: char| c.is_ascii_whitespace()).unwrap_or(v.len());
                        (&v[..end], &v[end..])
                    }
                };
                value = Some(val);
                s = rest;
            }
            if name.is_empty() {
                // A lone `=` or similar junk: skip one character so the loop advances.
                s = s.get(1..).unwrap_or("");
                continue;
            }
            if !is_name(name) {
                continue;
            }
            let name = if self.html { name.to_ascii_lowercase() } else { name.to_string() };
            let value = value.unwrap_or(&name).to_string();
            let (name, value) = match self.map.and_then(|m| m(&name, &value)) {
                Some((n, v)) => (n.to_string(), v),
                None => (name, value),
            };
            if seen.contains(&name) {
                continue;
            }
            self.out.push(' ');
            self.out.push_str(&name);
            self.out.push_str("=\"");
            escape(&mut self.out, &value, true);
            self.out.push('"');
            seen.push(name);
        }
    }

    fn text(&mut self, s: &str) {
        escape(&mut self.out, s, false);
    }
}

/// Index of the `>` closing the tag at the start of `s`, skipping quoted attribute values.
/// A stray `<` first means this was no tag; a quote never closed falls back to the first `>`.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, b) in s.bytes().enumerate().skip(1) {
        match (quote, b) {
            (None, b'"' | b'\'') => quote = Some(b),
            (Some(q), _) if b == q => quote = None,
            (None, b'>') => return Some(i),
            (None, b'<') => return None,
            _ => {}
        }
    }
    let end = s[1..].find(['<', '>'])? + 1;
    (s.as_bytes()[end] == b'>').then_some(end)
}

fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// Appends `s` with `<` (and in attributes `"`) escaped, `&` kept only when it starts a
/// reference the XML parser will resolve, and XML-illegal control characters dropped.
fn escape(out: &mut String, s: &str, attr: bool) {
    for (i, c) in s.char_indices() {
        match c {
            '&' if valid_reference(&s[i..]) => out.push('&'),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' if attr => out.push_str("&quot;"),
            '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' => {}
            c => out.push(c),
        }
    }
}

/// Whether `s` (starting at `&`) begins with a character or entity reference that resolves.
fn valid_reference(s: &str) -> bool {
    // Bytes, not `str::get`: 40 bytes in may fall inside a multi-byte character. `;` is ASCII,
    // so `end` is a char boundary.
    let Some(end) = s.as_bytes().iter().take(40).position(|&b| b == b';') else { return false };
    let body = &s[1..end];
    if let Some(num) = body.strip_prefix('#') {
        let (digits, radix) = match num.strip_prefix(['x', 'X']) {
            Some(hex) => (hex, 16),
            None => (num, 10),
        };
        let code = digits.chars().all(|c| c.is_digit(radix)).then(|| u32::from_str_radix(digits, radix).ok()).flatten();
        return code
            .is_some_and(|c| matches!(c, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff));
    }
    is_name(body) && (resolve_xml_entity(body).is_some() || resolve_html5_entity(body).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;

    fn html(s: &str) -> String {
        let out = fix(s, true, None);
        Dom::parse_xml(&out).unwrap_or_else(|e| panic!("{e:?} in {out}"));
        out
    }

    #[test]
    fn well_formed_xhtml_is_unchanged() {
        let src = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p class="a">x &amp; y&nbsp;&#233;<br/><svg viewBox="0 0 1 1"><linearGradient/></svg></p></body></html>"#;
        assert_eq!(fix(src, false, None), src);
        assert_eq!(fix("<?xml version='1.0'?><!DOCTYPE html><!-- c --><a/>", false, None), "<a/>");
        assert_eq!(fix("<style><![CDATA[a>b]]></style>", false, None), "<style><![CDATA[a>b]]></style>");
    }

    #[test]
    fn tag_soup_becomes_well_formed() {
        assert_eq!(html("<P ALIGN=center>a & b <i>c</P><p>d"), r#"<p align="center">a &amp; b <i>c</i></p><p>d</p>"#);
        assert_eq!(html("<p>one<p>two<div>x</div>"), "<p>one</p><p>two</p><div>x</div>");
        assert_eq!(html("<ul><li>a<li>b</ul>"), "<ul><li>a</li><li>b</li></ul>");
        assert_eq!(html("a</b>c<br>d<hr size=1>"), r#"ac<br/>d<hr size="1"/>"#);
        assert_eq!(html("x < y && z &bogus; &#0; &#65;"), "x &lt; y &amp;&amp; z &amp;bogus; &amp;#0; &#65;");
        assert_eq!(
            html(r#"<img src=a.jpg alt='say "hi"' ismap alt=dup>"#),
            r#"<img src="a.jpg" alt="say &quot;hi&quot;" ismap="ismap"/>"#
        );
        assert_eq!(html("<html><body><mbp:pagebreak/>t</body></html>"), "t");
        assert_eq!(html("<a href=\"x\">dangling <b"), "<a href=\"x\">dangling &lt;b</a>");
        assert_eq!(html("<1>"), "&lt;1>");
        // Byte 40 after the `&` falls inside a multi-byte character: the entity is still kept.
        let accented = format!("&amp;{}", "é".repeat(30));
        assert_eq!(html(&accented), accented);
        assert_eq!(html("a <b <i>c</i>"), "a &lt;b <i>c</i>");
        assert_eq!(html(r#"<p title="open>x</p>"#), r#"<p title="open">x</p>"#);
    }

    #[test]
    fn attributes_can_be_rewritten() {
        let map = |name: &str, value: &str| {
            (name == "filepos").then(|| ("href", format!("#f{}", value.trim_start_matches('0'))))
        };
        assert_eq!(fix("<a filepos=000123>x</a>", true, Some(&map)), r##"<a href="#f123">x</a>"##);
    }
}
