//! Old MOBI (MOBI6) content: one HTML-ish text stream. Links are `<a filepos=N>` byte offsets
//! into it and images are `<img recindex=N>`. We insert `<a id="fileposN"/>` anchors at every
//! target, split at `<mbp:pagebreak/>`, and fix each piece up into XHTML on demand.

use std::collections::HashMap;
use std::ops::Range;

use memchr::{memchr, memrchr};

use super::xhtml;
use super::{decode, find_ci, part_path, rfind_ci, MobiError};

pub struct Mobi6 {
    /// Body text with the anchors inserted.
    text: Vec<u8>,
    chunks: Vec<Range<usize>>,
    /// filepos → chunk holding its anchor.
    anchors: HashMap<u32, usize>,
    encoding: u32,
}

impl Mobi6 {
    /// `extra`: further filepos targets (from the NCX).
    pub fn new(raw: &[u8], extra: &[u32], encoding: u32) -> Mobi6 {
        let mut targets: Vec<u32> = filepos_links(raw).chain(extra.iter().copied()).collect();
        targets.retain(|&t| (t as usize) <= raw.len());
        targets.sort_unstable();
        targets.dedup();

        let mut text = Vec::with_capacity(raw.len() + targets.len() * 24);
        let mut at = Vec::with_capacity(targets.len());
        let mut copied = 0;
        for &t in &targets {
            let mut pos = t as usize;
            // Inside a tag: anchor before it instead.
            if let Some(lt) = memrchr(b'<', &raw[..pos]) {
                if !raw[lt..pos].contains(&b'>') {
                    pos = lt;
                }
            }
            let pos = pos.max(copied);
            text.extend_from_slice(&raw[copied..pos]);
            copied = pos;
            at.push((t, text.len()));
            text.extend_from_slice(format!("<a id=\"filepos{t}\"></a>").as_bytes());
        }
        text.extend_from_slice(&raw[copied..]);

        // ponytail: everything outside <body> (head, guide) is dropped; MOBI6 rarely has CSS there.
        let body = find_ci(&text, b"<body", 0).and_then(|b| memchr(b'>', &text[b..]).map(|g| b + g + 1)).unwrap_or(0);
        let end = rfind_ci(&text, b"</body").filter(|&e| e >= body).unwrap_or(text.len());
        let mut chunks = Vec::new();
        let mut cur = body;
        while let Some(p) = find_ci(&text[..end], b"<mbp:pagebreak", cur) {
            chunks.push(cur..p);
            cur = memchr(b'>', &text[p..end]).map_or(end, |g| p + g + 1);
        }
        chunks.push(cur..end);
        chunks.retain(|r| has_content(&text[r.clone()]));
        if chunks.is_empty() {
            chunks.push(body..end);
        }

        let anchors = at
            .into_iter()
            // Chunks are in text order, so their ends are sorted.
            .map(|(t, pos)| (t, chunks.partition_point(|r| r.end <= pos).min(chunks.len() - 1)))
            .collect();
        Mobi6 { text, chunks, anchors, encoding }
    }

    pub fn parts(&self) -> usize {
        self.chunks.len()
    }

    /// Link target for a filepos.
    pub fn target(&self, filepos: u32) -> String {
        match self.anchors.get(&filepos) {
            Some(&c) => format!("{}#filepos{filepos}", part_path(c)),
            None => String::new(),
        }
    }

    pub fn part(&self, i: usize, res: &dyn Fn(usize) -> String) -> Result<String, MobiError> {
        let r = self.chunks.get(i).ok_or_else(|| MobiError::NotFound(part_path(i)))?;
        let src = decode(&self.text[r.clone()], self.encoding);
        let map = |name: &str, value: &str| -> Option<(&'static str, String)> {
            let n: u32 = value.trim_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
            match name {
                "filepos" => Some(("href", self.target(n))),
                "recindex" => Some(("src", res(n.checked_sub(1)? as usize))),
                _ => None,
            }
        };
        let body = xhtml::fix(&src, true, Some(&map));
        Ok(format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title></title></head><body>{body}</body></html>"
        ))
    }
}

/// Values of every `filepos=` attribute.
fn filepos_links(raw: &[u8]) -> impl Iterator<Item = u32> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || loop {
        let at = find_ci(raw, b"filepos=", i)? + 8;
        i = at;
        let digits: Vec<u8> = raw[at..]
            .iter()
            .skip_while(|&&b| b == b'"' || b == b'\'')
            .take_while(|b| b.is_ascii_digit())
            .copied()
            .collect();
        if let Some(n) = std::str::from_utf8(&digits).ok().and_then(|s| s.parse().ok()) {
            return Some(n);
        }
    })
}

/// Whether a chunk has visible text or an image (pagebreaks often leave tag-only scraps).
fn has_content(chunk: &[u8]) -> bool {
    let mut in_tag = false;
    for (i, &b) in chunk.iter().enumerate() {
        match b {
            b'<' => {
                in_tag = true;
                if chunk[i + 1..].get(..3).is_some_and(|n| n.eq_ignore_ascii_case(b"img")) {
                    return true;
                }
            }
            b'>' => in_tag = false,
            b if !in_tag && !b.is_ascii_whitespace() => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;

    #[test]
    fn splits_at_pagebreaks_and_resolves_filepos_and_images() {
        let head: &[u8] = b"<html><head><guide></guide></head><body><p>Contents: <a filepos=";
        let mid: &[u8] = b">ch2</a><mbp:pagebreak/><mbp:pagebreak/>";
        let target = (head.len() + 10 + mid.len()) as u32;
        let tail: &[u8] = b"<h1>Two</h1><P>caf\xe9 & <img recindex=\"00002\"><mbp:pagebreak/></body></html>";
        let raw = [head, format!("{target:010}").as_bytes(), mid, tail].concat();
        let m = Mobi6::new(&raw, &[], 1252);
        assert_eq!(m.parts(), 2, "empty chunks between pagebreaks are dropped");
        let res = |i: usize| format!("res{i:05}.jpg");
        let one = m.part(0, &res).expect("part 0");
        assert!(one.contains(r#"<a href="part0001.xhtml#filepos"#), "{one}");
        let two = m.part(1, &res).expect("part 1");
        assert!(two.contains(&format!(r#"<a id="filepos{target}"></a><h1>Two</h1>"#)), "{two}");
        assert!(two.contains(r#"<p>café &amp; <img src="res00001.jpg"/></p>"#), "{two}");
        for p in [one, two] {
            Dom::parse_xml(&p).expect("well-formed");
        }
        assert!(m.part(2, &res).is_err());
    }

    #[test]
    fn anchor_inside_a_tag_moves_before_it() {
        let m = Mobi6::new(b"ab<p class=x>cd</p>", &[5], 65001);
        assert_eq!(&m.text, br#"ab<a id="filepos5"></a><p class=x>cd</p>"#);
        assert_eq!(m.target(5), "part0000.xhtml#filepos5");
        assert_eq!(m.target(6), "");
    }
}
