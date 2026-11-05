//! MOBI and AZW3 (KF8) books, DRM-free: PalmDB container, PalmDOC/MOBI/EXTH headers, text
//! decompression (PalmDOC LZ77, HUFF/CDIC), KF8 part reconstruction and MOBI6 fallback.
//!
//! Everything a book refers to gets a synthetic flat path, so callers treat it like an EPUB
//! archive: chapters are `part0000.xhtml`, stylesheets `flow0001.css`, images and fonts
//! `res00012.jpg` (0-based resource index). [`Mobi::read`] serves any of them.

mod compress;
mod index;
mod kf8;
mod mobi6;
mod xhtml;

use std::io::Read;
use std::ops::Range;
use std::path::Path;

use crate::epub::{Metadata, TocEntry};

#[derive(Debug, thiserror::Error)]
pub enum MobiError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a MOBI/AZW3 file")]
    NotMobi,
    #[error("the book is DRM-protected")]
    Drm,
    #[error("unsupported compression type {0}")]
    Compression(u16),
    #[error("malformed book: {0}")]
    Malformed(&'static str),
    #[error("not in the book: {0}")]
    NotFound(String),
}

pub struct Mobi {
    pub metadata: Metadata,
    pub publisher: Option<String>,
    pub asin: Option<String>,
    /// Whether the content is KF8 (AZW3) rather than old MOBI6.
    pub kf8: bool,
    /// Reading order: one path per part.
    pub chapters: Vec<String>,
    pub toc: Vec<TocEntry>,
    data: Vec<u8>,
    records: Vec<Range<usize>>,
    /// Absolute record index of resource 0.
    first_resource: Option<usize>,
    /// Resource index of the cover image.
    cover: Option<usize>,
    content: Content,
}

enum Content {
    Kf8(kf8::Kf8),
    Mobi6(mobi6::Mobi6),
}

const NULL: u32 = u32::MAX;
/// Largest book we read into memory.
const MAX_FILE: u64 = 256 << 20;
/// Largest decompressed font.
const MAX_FONT: u64 = 32 << 20;
/// Largest decompressed text: a crafted header must not make us allocate without bound.
const MAX_TEXT: usize = 64 << 20;

impl Mobi {
    pub fn open(path: &Path) -> Result<Mobi, MobiError> {
        let mut data = Vec::new();
        std::fs::File::open(path)?.take(MAX_FILE).read_to_end(&mut data)?;
        Mobi::from_bytes(data)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Mobi, MobiError> {
        if data.get(60..68) != Some(b"BOOKMOBI") {
            return Err(MobiError::NotMobi);
        }
        let records = palmdb_records(&data)?;
        let rec = |i: usize| records.get(i).map(|r| &data[r.clone()]);

        let r0 = rec(0).ok_or(MobiError::Malformed("no records"))?;
        let h0 = Header::parse(r0)?;
        if h0.encryption != 0 {
            return Err(MobiError::Drm);
        }
        let exth = exth(r0, &h0);
        let exth_str =
            |t: u32| exth.iter().find(|(k, _)| *k == t).map(|(_, v)| decode(v, h0.encoding).trim().to_string());
        let exth_u32 = |t: u32| exth.iter().find(|(k, _)| *k == t).and_then(|(_, v)| be_u32(v, 0).ok());

        // Combination files carry a MOBI6 book and, after a BOUNDARY record, a KF8 one whose
        // record indices are relative to its own header record.
        let (base, h) = match exth_u32(121).filter(|_| h0.version < 8).map(|k| k as usize) {
            Some(k) => match rec(k).map(Header::parse) {
                Some(Ok(h)) if h.version >= 8 => (k, h),
                _ => (0, h0.clone()),
            },
            None => (0, h0.clone()),
        };
        if h.encryption != 0 {
            return Err(MobiError::Drm);
        }
        let text = text(&rec, base, &h)?;

        let full_name =
            r0.get(h0.full_name.0..h0.full_name.0.saturating_add(h0.full_name.1)).map(|n| decode(n, h0.encoding));
        let palm_name = || decode(data[..32].split(|&b| b == 0).next().unwrap_or_default(), h0.encoding);
        let metadata = Metadata {
            title: exth_str(503).or(full_name).or_else(|| Some(palm_name())).filter(|t| !t.is_empty()),
            authors: exth
                .iter()
                .filter(|(k, _)| *k == 100)
                .map(|(_, v)| decode(v, h0.encoding).trim().to_string())
                .collect(),
            language: exth_str(524),
            identifier: exth_str(104).or_else(|| exth_str(113)),
            series: None,
            series_index: None,
            description: exth_str(103),
        };

        // ponytail: in combination files the KF8 header's first resource may be absolute
        // (resources are shared and live before the boundary); we take whichever index lands on
        // a plausible resource. Ceiling: a combination file where both do. Upgrade: confirm on
        // a real combination file.
        let resource_like = |i: usize| rec(i).is_some_and(|r| !r.is_empty() && !is_meta_record(r));
        let first_resource = (h.first_resource != NULL).then(|| {
            let rel = base.saturating_add(h.first_resource as usize);
            if !resource_like(rel) && resource_like(h.first_resource as usize) {
                h.first_resource as usize
            } else {
                rel
            }
        });
        let cover = exth_u32(201)
            .or_else(|| exth_u32(202))
            .filter(|&c| c != NULL)
            .map(|c| c as usize)
            .filter(|&c| first_resource.is_some_and(|f| resource_like(f.saturating_add(c))));

        let ncx = (h.ncx != NULL).then(|| index::read(rec, base.saturating_add(h.ncx as usize)).ok()).flatten();
        let (content, toc) = if h.version >= 8 {
            let idx = |i: u32, what| {
                if i == NULL {
                    return Err(MobiError::Malformed(what));
                }
                index::read(rec, base.saturating_add(i as usize))
            };
            let skel = idx(h.skel, "no SKEL index")?;
            let frag = idx(h.frag, "no FRAG index")?;
            let fdst = (h.fdst != NULL).then(|| rec(base.saturating_add(h.fdst as usize))).flatten();
            let kf8 = kf8::Kf8::new(text, fdst, &skel, &frag)?;
            let toc = ncx.map(|n| {
                toc(&n, h.encoding, |e| {
                    let fid_off = e.tag(6).and_then(|v| Some((*v.first()?, *v.get(1)?)));
                    kf8.toc_target(fid_off, e.first(1))
                })
            });
            (Content::Kf8(kf8), toc)
        } else {
            let filepos: Vec<u32> = ncx.iter().flat_map(|n| n.entries.iter().filter_map(|e| e.first(1))).collect();
            let m6 = mobi6::Mobi6::new(&text, &filepos, h.encoding);
            let toc = ncx.map(|n| toc(&n, h.encoding, |e| e.first(1).map(|p| m6.target(p)).unwrap_or_default()));
            (Content::Mobi6(m6), toc)
        };
        let parts = match &content {
            Content::Kf8(k) => k.parts(),
            Content::Mobi6(m) => m.parts(),
        };

        Ok(Mobi {
            metadata,
            publisher: exth_str(101),
            asin: exth_str(113),
            kf8: h.version >= 8,
            chapters: (0..parts).map(part_path).collect(),
            toc: toc.unwrap_or_default(),
            first_resource,
            cover,
            content,
            data,
            records,
        })
    }

    /// Part `i` as well-formed XHTML, links and references rewritten to the synthetic paths.
    pub fn read_part(&self, i: usize) -> Result<String, MobiError> {
        let res = |i| self.resource_path(i);
        match &self.content {
            // ponytail: KF8 parts are XHTML already; the fixer guards against the odd broken one.
            Content::Kf8(k) => Ok(xhtml::fix(&String::from_utf8_lossy(&k.part(i, &res)?), false, None)),
            Content::Mobi6(m) => m.part(i, &res),
        }
    }

    /// Flow `i` (KF8 only): stylesheets, SVG. Flow 0 is the raw text.
    pub fn flow(&self, i: usize) -> Result<String, MobiError> {
        match &self.content {
            Content::Kf8(k) => Ok(String::from_utf8_lossy(&k.flow(i, &|r| self.resource_path(r))?).into_owned()),
            Content::Mobi6(_) => Err(MobiError::NotFound(format!("flow {i}"))),
        }
    }

    /// Resource `i` (images, fonts), 0-based as in `kindle:embed` minus one.
    pub fn resource(&self, i: usize) -> Result<Vec<u8>, MobiError> {
        let r = self.resource_record(i).ok_or_else(|| MobiError::NotFound(format!("resource {i}")))?;
        if r.starts_with(b"FONT") {
            font(r)
        } else {
            Ok(r.to_vec())
        }
    }

    pub fn cover(&self) -> Option<Vec<u8>> {
        self.resource(self.cover?).ok()
    }

    /// Bytes behind a synthetic path (`#fragment` ignored): a part, a flow or a resource.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, MobiError> {
        let name = path.split('#').next().unwrap_or_default();
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
        let num = |prefix: &str| stem.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok());
        if let Some(n) = num("part") {
            Ok(self.read_part(n)?.into_bytes())
        } else if let Some(n) = num("flow") {
            Ok(self.flow(n)?.into_bytes())
        } else if let Some(n) = num("res") {
            self.resource(n)
        } else {
            Err(MobiError::NotFound(path.to_string()))
        }
    }

    fn resource_record(&self, i: usize) -> Option<&[u8]> {
        let r = self.records.get(self.first_resource?.checked_add(i)?)?;
        Some(&self.data[r.clone()])
    }

    fn resource_path(&self, i: usize) -> String {
        let ext = match self.resource_record(i) {
            Some([0xff, 0xd8, 0xff, ..]) => "jpg",
            Some([0x89, b'P', b'N', b'G', ..]) => "png",
            Some([b'G', b'I', b'F', b'8', ..]) => "gif",
            Some([b'B', b'M', ..]) => "bmp",
            Some([b'F', b'O', b'N', b'T', ..]) => "ttf",
            _ => "bin",
        };
        format!("res{i:05}.{ext}")
    }
}

pub(crate) fn part_path(i: usize) -> String {
    format!("part{i:04}.xhtml")
}

/// Record byte ranges from the PalmDB header: 78 bytes, then 8 bytes per record.
fn palmdb_records(data: &[u8]) -> Result<Vec<Range<usize>>, MobiError> {
    let n = usize::from(be_u16(data, 76)?);
    let offsets = (0..n).map(|i| be_u32(data, 78 + 8 * i).map(|o| o as usize)).collect::<Result<Vec<_>, _>>()?;
    let mut records = Vec::with_capacity(n);
    for (i, &start) in offsets.iter().enumerate() {
        let end = offsets.get(i + 1).copied().unwrap_or(data.len());
        if start > end || end > data.len() {
            return Err(MobiError::Malformed("record offsets"));
        }
        records.push(start..end);
    }
    Ok(records)
}

/// Records that sit among the resources but are not images or fonts.
fn is_meta_record(r: &[u8]) -> bool {
    matches!(
        r.get(..4),
        Some(b"FLIS" | b"FCIS" | b"FDST" | b"DATP" | b"SRCS" | b"BOUN" | b"INDX" | b"RESC" | b"\xe9\x8e\r\n")
    )
}

/// The fields of record 0 (PalmDOC header + MOBI header) we use. Offsets are from the record start.
#[derive(Debug, Clone)]
struct Header {
    compression: u16,
    text_length: usize,
    text_records: usize,
    encryption: u16,
    encoding: u32,
    version: u32,
    full_name: (usize, usize),
    first_resource: u32,
    huff: (u32, u32),
    exth_flags: u32,
    mobi_len: usize,
    extra_flags: u16,
    ncx: u32,
    frag: u32,
    skel: u32,
    fdst: u32,
}

impl Header {
    fn parse(r: &[u8]) -> Result<Header, MobiError> {
        if r.get(16..20) != Some(b"MOBI") {
            return Err(MobiError::Malformed("MOBI header"));
        }
        // Clamped so offsets derived from it cannot overflow.
        let mobi_len = (be_u32(r, 20)? as usize).min(r.len());
        // Fields past the declared header length are absent (older, shorter headers).
        let field = |off: usize| if off + 4 <= 16 + mobi_len { be_u32(r, off).ok() } else { None };
        let version = field(0x24).unwrap_or(0);
        Ok(Header {
            compression: be_u16(r, 0)?,
            text_length: be_u32(r, 4)? as usize,
            text_records: usize::from(be_u16(r, 8)?),
            encryption: be_u16(r, 12)?,
            encoding: field(0x1c).unwrap_or(1252),
            version,
            full_name: (field(0x54).unwrap_or(0) as usize, field(0x58).unwrap_or(0) as usize),
            first_resource: field(0x6c).unwrap_or(NULL),
            huff: (field(0x70).unwrap_or(0), field(0x74).unwrap_or(0)),
            exth_flags: field(0x80).unwrap_or(0),
            mobi_len,
            extra_flags: if mobi_len >= 0xe4 && version >= 5 { be_u16(r, 0xf2).unwrap_or(0) } else { 0 },
            ncx: field(0xf4).unwrap_or(NULL),
            frag: field(0xf8).unwrap_or(NULL),
            skel: field(0xfc).unwrap_or(NULL),
            fdst: if version >= 8 { field(0xc0).unwrap_or(NULL) } else { NULL },
        })
    }
}

/// EXTH records (type, data), right after the MOBI header. Tolerant: stops at the first bad record.
fn exth<'a>(r0: &'a [u8], h: &Header) -> Vec<(u32, &'a [u8])> {
    let mut out = Vec::new();
    let at = 16 + h.mobi_len;
    if h.exth_flags & 0x40 == 0 || r0.get(at..at + 4) != Some(b"EXTH") {
        return out;
    }
    let Ok(count) = be_u32(r0, at + 8) else { return out };
    let mut pos = at + 12;
    for _ in 0..count {
        let (Ok(kind), Ok(len)) = (be_u32(r0, pos), be_u32(r0, pos + 4)) else { break };
        let len = len as usize;
        let Some(data) = r0.get(pos + 8..pos.saturating_add(len)).filter(|_| len >= 8) else { break };
        out.push((kind, data));
        pos += len;
    }
    out
}

/// Decompressed text of records 1..=text_records after `base`, trailing entries stripped.
fn text<'a>(rec: &impl Fn(usize) -> Option<&'a [u8]>, base: usize, h: &Header) -> Result<Vec<u8>, MobiError> {
    let mut huff = if h.compression == 17480 {
        let first = base.saturating_add(h.huff.0 as usize);
        let huff = rec(first).ok_or(MobiError::Malformed("HUFF record"))?;
        let cdics = (1..h.huff.1 as usize)
            .map(|i| rec(first.saturating_add(i)).ok_or(MobiError::Malformed("CDIC record")))
            .collect::<Result<Vec<_>, _>>()?;
        Some(compress::Huff::new(huff, &cdics)?)
    } else {
        None
    };
    if h.text_length > MAX_TEXT {
        return Err(MobiError::Malformed("text too large"));
    }
    let mut text = Vec::with_capacity(h.text_length);
    for i in 1..=h.text_records {
        // Stopping at the declared length bounds the output whatever the records hold (HUFF
        // records are bounded on their own).
        let budget = h.text_length - text.len().min(h.text_length);
        if budget == 0 {
            break;
        }
        let r = rec(base + i).ok_or(MobiError::Malformed("text record"))?;
        let r = &r[..r.len() - compress::trailing_size(r, h.extra_flags)];
        match (h.compression, &mut huff) {
            (1, _) => text.extend_from_slice(&r[..r.len().min(budget)]),
            (2, _) => compress::palmdoc(r, &mut text, budget)?,
            (17480, Some(huff)) => huff.unpack(r, &mut text)?,
            (c, _) => return Err(MobiError::Compression(c)),
        }
    }
    text.truncate(h.text_length);
    Ok(text)
}

/// NCX index → nested TOC. Tag 3 is the label (CNCX offset), tag 21 the parent entry.
fn toc(ncx: &index::Index, encoding: u32, mut target: impl FnMut(&index::Entry) -> String) -> Vec<TocEntry> {
    let n = ncx.entries.len();
    let mut nodes: Vec<Option<TocEntry>> = Vec::with_capacity(n);
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut roots = Vec::new();
    for (i, e) in ncx.entries.iter().enumerate() {
        let title = e.first(3).and_then(|o| ncx.cncx(o)).map(|l| decode(l, encoding)).unwrap_or_default();
        nodes.push(Some(TocEntry { title, target: target(e), children: Vec::new() }));
        // Parents come first; anything else is treated as a root rather than risking a cycle.
        match e.first(21).map(|p| p as usize).filter(|&p| p < i) {
            Some(p) => children[p].push(i),
            None => roots.push(i),
        }
    }
    // Children always have larger indices, so building back to front completes them first.
    for i in (0..n).rev() {
        let kids: Vec<TocEntry> = children[i].iter().filter_map(|&c| nodes[c].take()).collect();
        if let Some(node) = &mut nodes[i] {
            node.children = kids;
        }
    }
    roots.into_iter().filter_map(|i| nodes[i].take()).collect()
}

/// FONT record: header, then data that may be XOR-obfuscated (first 1040 bytes) and zlib-compressed.
fn font(r: &[u8]) -> Result<Vec<u8>, MobiError> {
    let flags = be_u32(r, 8)?;
    let start = be_u32(r, 12)? as usize;
    let (key_len, key_at) = (be_u32(r, 16)? as usize, be_u32(r, 20)? as usize);
    let mut data = r.get(start..).ok_or(MobiError::Malformed("FONT data"))?.to_vec();
    if flags & 2 != 0 && key_len > 0 {
        let key = r.get(key_at..key_at.saturating_add(key_len)).ok_or(MobiError::Malformed("FONT key"))?;
        for (i, b) in data.iter_mut().take(1040).enumerate() {
            *b ^= key[i % key.len()];
        }
    }
    if flags & 1 != 0 {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(data.as_slice()).take(MAX_FONT).read_to_end(&mut out)?;
        data = out;
    }
    Ok(data)
}

pub(crate) fn be_u16(b: &[u8], at: usize) -> Result<u16, MobiError> {
    let s = b.get(at..at.saturating_add(2)).ok_or(MobiError::Malformed("truncated"))?;
    Ok(u16::from_be_bytes([s[0], s[1]]))
}

pub(crate) fn be_u32(b: &[u8], at: usize) -> Result<u32, MobiError> {
    let s = b.get(at..at.saturating_add(4)).ok_or(MobiError::Malformed("truncated"))?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub(crate) fn find_ci(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle)).map(|p| from + p)
}

/// Value of attribute `name` inside a tag's bytes (between `<` and `>`), quoted or not.
pub(crate) fn attr_value<'a>(tag: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut from = 0;
    while let Some(p) = find(&tag[from..], name) {
        let at = from + p;
        from = at + 1;
        if at == 0 || !tag[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = tag[at + name.len()..].trim_ascii_start();
        let Some(rest) = rest.strip_prefix(b"=") else { continue };
        let rest = rest.trim_ascii_start();
        return Some(match rest.first() {
            Some(&q @ (b'"' | b'\'')) => {
                let body = &rest[1..];
                &body[..body.iter().position(|&b| b == q).unwrap_or(body.len())]
            }
            _ => &rest[..rest.iter().position(|b| b.is_ascii_whitespace() || *b == b'/').unwrap_or(rest.len())],
        });
    }
    None
}

/// Text in the book's encoding: UTF-8 (65001) or Windows-1252.
pub(crate) fn decode(b: &[u8], encoding: u32) -> String {
    if encoding == 65001 {
        return String::from_utf8_lossy(b).into_owned();
    }
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    b.iter().map(|&c| if (0x80..0xa0).contains(&c) { HIGH[usize::from(c - 0x80)] } else { char::from(c) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Dom;

    /// PalmDB file from records.
    fn palmdb(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0u8; 78];
        out[..4].copy_from_slice(b"Test");
        out[60..68].copy_from_slice(b"BOOKMOBI");
        out[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
        let mut offset = 78 + 8 * records.len() + 2;
        for r in records {
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&[0; 4]);
            offset += r.len();
        }
        out.extend_from_slice(&[0, 0]);
        for r in records {
            out.extend_from_slice(r);
        }
        out
    }

    /// Record 0 for an uncompressed MOBI6 book with an EXTH block.
    fn record0(text_len: usize, text_records: u16, encryption: u16, exth: &[(u32, &[u8])]) -> Vec<u8> {
        let mut r = vec![0u8; 16 + 0xe8];
        r[0..2].copy_from_slice(&1u16.to_be_bytes());
        r[4..8].copy_from_slice(&(text_len as u32).to_be_bytes());
        r[8..10].copy_from_slice(&text_records.to_be_bytes());
        r[12..14].copy_from_slice(&encryption.to_be_bytes());
        r[16..20].copy_from_slice(b"MOBI");
        r[20..24].copy_from_slice(&0xe8u32.to_be_bytes());
        r[0x1c..0x20].copy_from_slice(&65001u32.to_be_bytes());
        r[0x24..0x28].copy_from_slice(&6u32.to_be_bytes());
        r[0x6c..0x70].copy_from_slice(&(u32::from(text_records) + 1).to_be_bytes());
        r[0x80..0x84].copy_from_slice(&0x40u32.to_be_bytes());
        r[0xf4..0xf8].copy_from_slice(&NULL.to_be_bytes());
        r.extend_from_slice(b"EXTH");
        let body: usize = exth.iter().map(|(_, d)| 8 + d.len()).sum();
        r.extend_from_slice(&((12 + body) as u32).to_be_bytes());
        r.extend_from_slice(&(exth.len() as u32).to_be_bytes());
        for (k, d) in exth {
            r.extend_from_slice(&k.to_be_bytes());
            r.extend_from_slice(&((8 + d.len()) as u32).to_be_bytes());
            r.extend_from_slice(d);
        }
        r
    }

    #[test]
    fn opens_a_mobi6_book() {
        let text = b"<html><body><p>One<mbp:pagebreak/><p>Two <img recindex=\"00001\"></body></html>".to_vec();
        let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
        let exth: &[(u32, &[u8])] =
            &[(100, b"Ann"), (100, b"Bob"), (503, b"Title"), (524, b"it"), (201, &0u32.to_be_bytes()), (113, b"B00X")];
        let file = palmdb(&[record0(text.len(), 1, 0, exth), text, jpeg.clone()]);
        let book = Mobi::from_bytes(file).expect("opens");
        assert!(!book.kf8);
        assert_eq!(book.metadata.title.as_deref(), Some("Title"));
        assert_eq!(book.metadata.authors, ["Ann", "Bob"]);
        assert_eq!(book.metadata.language.as_deref(), Some("it"));
        assert_eq!(book.asin.as_deref(), Some("B00X"));
        assert_eq!(book.chapters, ["part0000.xhtml", "part0001.xhtml"]);
        assert_eq!(book.cover(), Some(jpeg.clone()));
        let two = book.read_part(1).expect("part");
        assert!(two.contains(r#"<img src="res00000.jpg"/>"#), "{two}");
        Dom::parse_xml(&two).expect("well-formed");
        assert_eq!(book.read("res00000.jpg").expect("resource"), jpeg);
        assert!(book.read("nonsense").is_err());
    }

    #[test]
    fn drm_and_garbage_are_errors() {
        let file = palmdb(&[record0(4, 1, 2, &[]), b"text".to_vec()]);
        assert!(matches!(Mobi::from_bytes(file), Err(MobiError::Drm)));
        assert!(matches!(Mobi::from_bytes(b"PK\x03\x04".to_vec()), Err(MobiError::NotMobi)));
        let huge = palmdb(&[record0(MAX_TEXT + 1, 1, 0, &[]), b"text".to_vec()]);
        assert!(matches!(Mobi::from_bytes(huge), Err(MobiError::Malformed(_))), "text length is capped");
        // Records past the declared length are not decoded.
        let short = palmdb(&[record0(4, 2, 0, &[]), b"text".to_vec(), b"more".to_vec()]);
        assert_eq!(
            Mobi::from_bytes(short).expect("opens").read_part(0).expect("part"),
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title></title></head><body>text</body></html>"
        );
        let mut file = palmdb(&[record0(4, 1, 0, &[]), b"text".to_vec()]);
        // Every truncation is an error or a book, never a panic.
        while file.len() > 60 {
            file.pop();
            let _ = Mobi::from_bytes(file.clone());
        }
    }

    #[test]
    fn font_records_are_deobfuscated_and_inflated() {
        use std::io::Write;
        let font_bytes = b"OTTO font data ".repeat(100);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&font_bytes).expect("compress");
        let mut data = z.finish().expect("finish");
        let key = [0x5a, 0xa5, 0x0f];
        for (i, b) in data.iter_mut().take(1040).enumerate() {
            *b ^= key[i % key.len()];
        }
        let mut r = b"FONT".to_vec();
        for v in [font_bytes.len() as u32, 3, 27, 3, 24] {
            r.extend_from_slice(&v.to_be_bytes());
        }
        r.extend_from_slice(&key);
        r.extend_from_slice(&data);
        assert_eq!(font(&r).expect("font"), font_bytes);
        assert!(font(&r[..20]).is_err());
    }

    #[test]
    fn cp1252_and_attr_values() {
        assert_eq!(decode(b"caf\xe9 \x93q\x94 \x80", 1252), "café “q” €");
        assert_eq!(attr_value(br#"a href="x y" id=z/"#, b"id"), Some(&b"z"[..]));
        assert_eq!(attr_value(br#"a xid="1" id = '2'"#, b"id"), Some(&b"2"[..]));
        assert_eq!(attr_value(b"a", b"id"), None);
    }

    /// Opens every AZW3/MOBI in `dev-books/` (local only, not in the repo): metadata is read and
    /// every part parses as XML.
    #[test]
    fn local_books_parse() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let dirs = ["dev-books", "dev-books/azw3"].map(|d| root.join(d));
        for entry in dirs.iter().filter_map(|d| std::fs::read_dir(d).ok()).flatten().flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
            if !matches!(ext.as_str(), "azw3" | "mobi" | "azw") {
                continue;
            }
            let name = path.display();
            let book = match Mobi::open(&path) {
                Err(MobiError::Drm) => continue,
                r => r.unwrap_or_else(|e| panic!("{name}: {e}")),
            };
            assert!(book.metadata.title.is_some(), "{name}: no title");
            assert!(!book.chapters.is_empty(), "{name}: no chapters");
            for (i, _) in book.chapters.iter().enumerate() {
                let xhtml = book.read_part(i).unwrap_or_else(|e| panic!("{name} part {i}: {e}"));
                Dom::parse_xml(&xhtml).unwrap_or_else(|e| panic!("{name} part {i}: {e:?}"));
            }
            if book.kf8 {
                assert!(!book.toc.is_empty(), "{name}: no TOC");
            }
        }
    }
}
