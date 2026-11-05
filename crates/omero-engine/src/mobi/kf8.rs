//! KF8 (AZW3) content: the decompressed text holds, per output file, a skeleton followed by
//! its fragments. The SKEL index locates skeletons, the FRAG index says where each fragment is
//! inserted. Flows (CSS, SVG) follow the text, delimited by the FDST record.

use std::cell::OnceCell;
use std::collections::VecDeque;
use std::ops::Range;

use super::index::Index;
use super::{attr_value, be_u32, find, part_path, MobiError};

pub struct Kf8 {
    text: Vec<u8>,
    flows: Vec<Range<usize>>,
    files: Vec<File>,
    frags: Vec<Frag>,
    /// Per file, its anchors once a link or TOC entry needed them (`None`: it did not assemble).
    anchors: Vec<OnceCell<Option<Anchors>>>,
}

struct File {
    /// Text offset of the skeleton; also the file's origin in insert-position space.
    start: usize,
    skel_len: usize,
    /// Assembled length: skeleton plus all its fragments.
    len: usize,
    frags: Range<usize>,
}

struct Frag {
    /// Where the fragment goes, in the assembled-text coordinates of its file.
    insert: usize,
    len: usize,
}

/// Ids of a part's elements as (offset of the start tag's `<`, id), in document order.
type Anchors = Vec<(usize, String)>;

impl Kf8 {
    pub fn new(text: Vec<u8>, fdst: Option<&[u8]>, skel: &Index, frag: &Index) -> Result<Kf8, MobiError> {
        let flows = fdst.and_then(|f| flows(f, text.len())).unwrap_or_else(|| std::iter::once(0..text.len()).collect());
        let frags = frag
            .entries
            .iter()
            .map(|e| {
                // The label is the insert position in decimal; tag 6 is (text offset, length).
                let insert = std::str::from_utf8(&e.label).ok().and_then(|s| s.parse().ok());
                let len = e.tag(6).and_then(|v| v.get(1)).copied();
                match (insert, len) {
                    (Some(insert), Some(len)) => Ok(Frag { insert, len: len as usize }),
                    _ => Err(MobiError::Malformed("FRAG entry")),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut files = Vec::with_capacity(skel.entries.len());
        let mut next_frag = 0usize;
        for e in &skel.entries {
            let count = e.first(1).ok_or(MobiError::Malformed("SKEL fragment count"))? as usize;
            let (start, skel_len) = match e.tag(6) {
                Some(&[start, len, ..]) => (start as usize, len as usize),
                _ => return Err(MobiError::Malformed("SKEL position")),
            };
            let range = next_frag
                ..next_frag
                    .checked_add(count)
                    .filter(|&e| e <= frags.len())
                    .ok_or(MobiError::Malformed("SKEL fragment count"))?;
            let len = frags[range.clone()].iter().try_fold(skel_len, |acc, f| acc.checked_add(f.len));
            let len = len.filter(|&l| start.checked_add(l).is_some_and(|end| end <= text.len()));
            files.push(File {
                start,
                skel_len,
                len: len.ok_or(MobiError::Malformed("SKEL bounds"))?,
                frags: range.clone(),
            });
            next_frag = range.end;
        }
        let anchors = files.iter().map(|_| OnceCell::new()).collect();
        Ok(Kf8 { text, flows, files, frags, anchors })
    }

    pub fn parts(&self) -> usize {
        self.files.len()
    }

    /// Raw XHTML of part `i`: the skeleton with its fragments spliced in.
    fn assemble(&self, i: usize) -> Result<Vec<u8>, MobiError> {
        let f = self.files.get(i).ok_or_else(|| MobiError::NotFound(part_path(i)))?;
        let bad = || MobiError::Malformed("fragment insert position");
        // Bounds were checked in `new`. Insert positions normally only move forward, so the
        // part is built in one pass: `part` is final, `pending` holds (in order) the text
        // ranges still to come, and each fragment is queued at the front of `pending` once
        // everything before its position has been emitted.
        let mut part = Vec::with_capacity(f.len);
        let mut pending = VecDeque::new();
        pending.push_back(f.start..f.start + f.skel_len);
        let mut at = f.start + f.skel_len;
        for fr in &self.frags[f.frags.clone()] {
            let src = at..at + fr.len;
            at += fr.len;
            let pos = fr.insert.checked_sub(f.start).ok_or_else(bad)?;
            if pos < part.len() {
                // Backwards: flush and splice (quadratic, but no known book does this).
                for r in pending.drain(..) {
                    part.extend_from_slice(&self.text[r]);
                }
                part.splice(pos..pos, self.text[src].iter().copied());
                continue;
            }
            while part.len() < pos {
                let r = pending.front_mut().ok_or_else(bad)?;
                let take = r.len().min(pos - part.len());
                part.extend_from_slice(&self.text[r.start..r.start + take]);
                r.start += take;
                if r.start == r.end {
                    pending.pop_front();
                }
            }
            pending.push_front(src);
        }
        for r in pending {
            part.extend_from_slice(&self.text[r]);
        }
        Ok(part)
    }

    /// Part `i` with `kindle:` references rewritten to `partNNNN.xhtml#id`, `flowNNNN.css`
    /// and resource paths (from `res`, given the 0-based resource index).
    pub fn part(&self, i: usize, res: &dyn Fn(usize) -> String) -> Result<Vec<u8>, MobiError> {
        Ok(self.rewrite(&self.assemble(i)?, res))
    }

    /// Flow `i` (flow 0 is the text itself) with resource references rewritten.
    pub fn flow(&self, i: usize, res: &dyn Fn(usize) -> String) -> Result<Vec<u8>, MobiError> {
        let r = self.flows.get(i).ok_or_else(|| MobiError::NotFound(format!("flow {i}")))?;
        Ok(self.rewrite(&self.text[r.clone()], res))
    }

    fn rewrite(&self, src: &[u8], res: &dyn Fn(usize) -> String) -> Vec<u8> {
        let mut out = Vec::with_capacity(src.len());
        let mut i = 0;
        while let Some(k) = find(&src[i..], b"kindle:") {
            let at = i + k;
            out.extend_from_slice(&src[i..at]);
            let end = src[at..]
                .iter()
                .position(|&b| matches!(b, b'"' | b'\'' | b'(' | b')' | b'<' | b'>') || b.is_ascii_whitespace())
                .map_or(src.len(), |p| at + p);
            let token = &src[at..end];
            match std::str::from_utf8(token).ok().and_then(|t| self.resolve(t, res)) {
                Some(s) => out.extend_from_slice(s.as_bytes()),
                None => out.extend_from_slice(token),
            }
            i = end;
        }
        out.extend_from_slice(&src[i..]);
        out
    }

    fn resolve(&self, token: &str, res: &dyn Fn(usize) -> String) -> Option<String> {
        if let Some(r) = token.strip_prefix("kindle:pos:fid:") {
            let (fid, off) = r.split_once(":off:")?;
            let off = base32(off.split(['?', '#']).next()?)?;
            let pos = self.frags.get(base32(fid)?)?.insert.checked_add(off)?;
            return self.target(pos);
        }
        if let Some(r) = token.strip_prefix("kindle:embed:") {
            return Some(res(base32(r.split(['?', '#']).next()?)?.checked_sub(1)?));
        }
        let r = token.strip_prefix("kindle:flow:")?;
        let (n, query) = r.split_once('?').unwrap_or((r, ""));
        let ext = match query.strip_prefix("mime=") {
            Some("text/css") => "css",
            Some("image/svg+xml") => "svg",
            _ => "txt",
        };
        Some(format!("flow{:04}.{ext}", base32(n)?))
    }

    /// Link target for a text position: the part containing it plus the nearest preceding id.
    fn target(&self, pos: usize) -> Option<String> {
        let fi = self.files.iter().position(|f| pos >= f.start && pos < f.start + f.len)?;
        let ids = self.anchors[fi].get_or_init(|| self.assemble(fi).ok().map(|p| anchors(&p))).as_ref()?;
        Some(match anchor_at(ids, pos - self.files[fi].start) {
            Some(id) => format!("{}#{id}", part_path(fi)),
            None => part_path(fi),
        })
    }

    /// Target of an NCX entry: (fragment id, offset) when given, else the plain text offset.
    pub fn toc_target(&self, fid_off: Option<(u32, u32)>, pos: Option<u32>) -> String {
        let pos = match fid_off {
            Some((fid, off)) => self.frags.get(fid as usize).and_then(|f| f.insert.checked_add(off as usize)),
            None => pos.map(|p| p as usize),
        };
        pos.and_then(|p| self.target(p)).unwrap_or_default()
    }
}

/// FDST: "FDST", data offset, count, then (start, end) pairs.
fn flows(fdst: &[u8], text_len: usize) -> Option<Vec<Range<usize>>> {
    if !fdst.starts_with(b"FDST") {
        return None;
    }
    let count = be_u32(fdst, 8).ok()? as usize;
    (0..count.min(fdst.len() / 8))
        .map(|i| {
            let start = be_u32(fdst, 12 + 8 * i).ok()? as usize;
            let end = (be_u32(fdst, 16 + 8 * i).ok()? as usize).min(text_len);
            (start <= end).then_some(start..end)
        })
        .collect()
}

fn anchors(part: &[u8]) -> Anchors {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(l) = find(&part[i..], b"<") {
        let start = i + l;
        let Some(g) = find(&part[start..], b">") else { break };
        let tag = &part[start + 1..start + g];
        if !tag.starts_with(b"/") {
            // `name` anchors only on <a>: <meta name=…> is not a link target.
            let is_a = tag.first().is_some_and(|b| b.eq_ignore_ascii_case(&b'a'))
                && tag.get(1).is_some_and(u8::is_ascii_whitespace);
            if let Some(v) = attr_value(tag, b"id").or_else(|| attr_value(tag, b"name").filter(|_| is_a)) {
                out.push((start, String::from_utf8_lossy(v).into_owned()));
            }
        }
        i = start + g + 1;
    }
    out
}

/// The id of the last element whose start tag begins at or before `pos` (so a position inside
/// a tag counts that tag).
fn anchor_at(anchors: &Anchors, pos: usize) -> Option<&str> {
    let n = anchors.partition_point(|(start, _)| *start <= pos);
    Some(anchors.get(n.checked_sub(1)?)?.1.as_str())
}

/// Kindle base-32 numbers: digits `0-9A-V`.
fn base32(s: &str) -> Option<usize> {
    if s.is_empty() || s.len() > 12 {
        return None;
    }
    s.chars().try_fold(0usize, |acc, c| acc.checked_mul(32)?.checked_add(c.to_digit(32)? as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobi::index::read;
    use crate::mobi::index::tests::{build_index, enc};

    const SKEL: &[u8] = br#"<html><body><p id="a">!</p></body></html>"#;
    const FRAG1: &[u8] = b"Hello ";
    const FRAG2: &[u8] = br#"<a href="kindle:pos:fid:0000:off:0000000006">x</a>"#;
    const CSS: &[u8] = b"p{background:url(kindle:embed:0002?mime=image/png)}";

    /// One file: `SKEL` with `FRAG1` inserted after `<p id="a">` (offset 22) and `FRAG2` after it.
    fn sample() -> Kf8 {
        build(SKEL, &[(b"22", FRAG1), (b"28", FRAG2)], CSS)
    }

    /// One file: `skel`, then `frags` as (insert position label, bytes), then a CSS flow.
    fn build(skel_text: &[u8], frags: &[(&[u8], &[u8])], css: &[u8]) -> Kf8 {
        let mut text = skel_text.to_vec();
        let s = skel_text.len() as u32;
        let skel_tagx = [[1, 1, 0x01, 0], [6, 2, 0x02, 0], [0, 0, 0, 1]];
        let mut e = vec![0x03];
        for v in [frags.len() as u32, 0, s] {
            e.extend(enc(v));
        }
        let skel = build_index(&skel_tagx, 1, &[(b"SKEL0000000000", e)], None);
        let frag_tagx = [[6, 2, 0x01, 0], [0, 0, 0, 1]];
        let entries: Vec<(&[u8], Vec<u8>)> = frags
            .iter()
            .map(|&(label, bytes)| {
                let mut e = vec![0x01];
                e.extend(enc(text.len() as u32));
                e.extend(enc(bytes.len() as u32));
                text.extend_from_slice(bytes);
                (label, e)
            })
            .collect();
        let frag = build_index(&frag_tagx, 1, &entries, None);
        let skel = read(|i| skel.get(i).map(Vec::as_slice), 0).expect("skel");
        let frag = read(|i| frag.get(i).map(Vec::as_slice), 0).expect("frag");
        let mut fdst = b"FDST".to_vec();
        let flow0 = text.len() as u32;
        text.extend_from_slice(css);
        for v in [12u32, 2, 0, flow0, flow0, text.len() as u32] {
            fdst.extend_from_slice(&v.to_be_bytes());
        }
        Kf8::new(text, Some(&fdst), &skel, &frag).expect("kf8")
    }

    #[test]
    fn fragments_insert_forwards_backwards_and_inside_each_other() {
        let kf8 = build(b"<p>ab</p>", &[(b"5", b"[1]"), (b"6", b"<2>"), (b"3", b"Y")], b"");
        let part = kf8.assemble(0).expect("assembles");
        assert_eq!(String::from_utf8(part).expect("utf8"), "<p>Yab[<2>1]</p>");
        assert!(build(b"<p></p>", &[(b"99", b"x")], b"").assemble(0).is_err(), "past the end");
    }

    #[test]
    fn assembles_parts_and_rewrites_references() {
        let kf8 = sample();
        assert_eq!(kf8.parts(), 1);
        let res = |i: usize| format!("res{i:05}.png");
        let part = String::from_utf8(kf8.part(0, &res).expect("part")).expect("utf8");
        assert_eq!(part, r#"<html><body><p id="a">Hello <a href="part0000.xhtml#a">x</a>!</p></body></html>"#);
        let css = String::from_utf8(kf8.flow(1, &res).expect("flow")).expect("utf8");
        assert_eq!(css, "p{background:url(res00001.png)}");
        assert!(kf8.flow(2, &res).is_err());
        assert!(kf8.part(1, &res).is_err());
    }

    #[test]
    fn base32_and_anchors() {
        assert_eq!(base32("0000"), Some(0));
        assert_eq!(base32("000V"), Some(31));
        assert_eq!(base32("0010"), Some(32));
        assert_eq!(base32("00W0"), None);
        let html = br#"<div id="x"><a name='y'>text</a><meta name="no"/></div>"#;
        let a = anchors(html);
        assert_eq!(anchor_at(&a, 0), Some("x"), "at the first tag");
        assert_eq!(anchor_at(&a, 14), Some("y"), "inside the second tag");
        assert_eq!(a.len(), 2, "meta name is not an anchor");
        assert_eq!(anchor_at(&a, html.len()), Some("y"));
        assert_eq!(anchor_at(&anchors(b"plain"), 3), None);
    }

    #[test]
    fn bad_indexes_are_errors() {
        let skel_tagx = [[1, 1, 0x01, 0], [6, 2, 0x02, 0], [0, 0, 0, 1]];
        let mut e = vec![0x03];
        for v in [5, 0, 42] {
            e.extend(enc(v));
        }
        let skel = build_index(&skel_tagx, 1, &[(b"S", e)], None);
        let skel = read(|i| skel.get(i).map(Vec::as_slice), 0).expect("skel");
        assert!(Kf8::new(SKEL.to_vec(), None, &skel, &Index::default()).is_err(), "more fragments than exist");
    }
}
