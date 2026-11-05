//! INDX records: a header record (with the TAGX tag table), entry records (IDXT offsets to
//! entries, each a label plus tagged varint values), and CNCX string records.

use super::{be_u16, be_u32, MobiError};

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub label: Vec<u8>,
    pub tags: Vec<(u8, Vec<u32>)>,
}

impl Entry {
    pub fn tag(&self, tag: u8) -> Option<&[u32]> {
        self.tags.iter().find(|(t, _)| *t == tag).map(|(_, v)| v.as_slice())
    }

    pub fn first(&self, tag: u8) -> Option<u32> {
        self.tag(tag)?.first().copied()
    }
}

#[derive(Debug, Default)]
pub struct Index {
    pub entries: Vec<Entry>,
    cncx: Vec<Vec<u8>>,
}

impl Index {
    /// String at a CNCX offset: each CNCX record covers 64 KiB of offset space; a string is a
    /// varint length followed by its bytes.
    pub fn cncx(&self, offset: u32) -> Option<&[u8]> {
        let rec = self.cncx.get((offset >> 16) as usize)?;
        let pos = (offset & 0xffff) as usize;
        let (len, used) = varint(rec.get(pos..)?).ok()?;
        rec.get(pos + used..pos + used + len as usize)
    }
}

#[derive(Debug, Clone, Copy)]
struct TagX {
    tag: u8,
    values: u8,
    mask: u8,
    end: bool,
}

/// Reads the index whose header is record `idx`. `rec` fetches records by index.
pub fn read<'a>(rec: impl Fn(usize) -> Option<&'a [u8]>, idx: usize) -> Result<Index, MobiError> {
    let bad = |what| MobiError::Malformed(what);
    let header = rec(idx).ok_or(bad("index record"))?;
    if !header.starts_with(b"INDX") {
        return Err(bad("INDX magic"));
    }
    let count = be_u32(header, 24)? as usize;
    let ncncx = be_u32(header, 52)? as usize;
    let tagx_at = be_u32(header, 180)? as usize;
    let tagx = header.get(tagx_at..).ok_or(bad("TAGX offset"))?;
    if !tagx.starts_with(b"TAGX") {
        return Err(bad("TAGX magic"));
    }
    let tagx_len = be_u32(tagx, 4)? as usize;
    let control_bytes = be_u32(tagx, 8)? as usize;
    let table: Vec<TagX> = tagx
        .get(12..tagx_len)
        .ok_or(bad("TAGX length"))?
        .chunks_exact(4)
        .map(|c| TagX { tag: c[0], values: c[1], mask: c[2], end: c[3] & 1 != 0 })
        .collect();

    let mut index = Index::default();
    for i in 0..ncncx {
        index.cncx.push(rec(idx.saturating_add(count).saturating_add(1 + i)).ok_or(bad("CNCX record"))?.to_vec());
    }
    for i in 0..count {
        let data = rec(idx.saturating_add(1 + i)).ok_or(bad("index entry record"))?;
        if !data.starts_with(b"INDX") {
            return Err(bad("INDX magic"));
        }
        let idxt = be_u32(data, 20)? as usize;
        let n = be_u32(data, 24)? as usize;
        if data.get(idxt..idxt.saturating_add(4)) != Some(b"IDXT") {
            return Err(bad("IDXT magic"));
        }
        let mut starts = Vec::with_capacity(n.min(data.len()) + 1);
        for j in 0..n {
            starts.push(usize::from(be_u16(data, idxt.saturating_add(4 + 2 * j))?));
        }
        // The last entry ends where the IDXT begins.
        starts.push(idxt);
        for w in starts.windows(2) {
            let raw = data.get(w[0]..w[1]).ok_or(bad("index entry bounds"))?;
            index.entries.push(entry(raw, control_bytes, &table)?);
        }
    }
    Ok(index)
}

fn entry(raw: &[u8], control_bytes: usize, table: &[TagX]) -> Result<Entry, MobiError> {
    let bad = MobiError::Malformed("index entry");
    let &len = raw.first().ok_or(bad)?;
    let label = raw.get(1..1 + usize::from(len)).ok_or(MobiError::Malformed("index label"))?.to_vec();
    let rest = &raw[1 + usize::from(len)..];
    let controls = rest.get(..control_bytes).ok_or(MobiError::Malformed("index control bytes"))?;
    let mut data = &rest[control_bytes..];

    // First pass: which tags are present, and how many values (or value bytes) each carries.
    enum Count {
        Values(usize),
        Bytes(usize),
    }
    let mut present = Vec::new();
    let mut ci = 0;
    for t in table {
        if t.end {
            ci += 1;
            continue;
        }
        let Some(&c) = controls.get(ci) else { break };
        let value = c & t.mask;
        if value == 0 {
            continue;
        }
        let count = if value == t.mask && t.mask.count_ones() > 1 {
            // All mask bits set on a multi-bit mask: a varint says how many *bytes* of values follow.
            let (n, used) = varint(data)?;
            data = &data[used..];
            Count::Bytes(n as usize)
        } else if value == t.mask {
            Count::Values(1)
        } else {
            Count::Values(usize::from(value >> t.mask.trailing_zeros()))
        };
        present.push((t, count));
    }

    let mut tags = Vec::with_capacity(present.len());
    for (t, count) in present {
        let mut values = Vec::new();
        match count {
            Count::Values(n) => {
                for _ in 0..n * usize::from(t.values) {
                    let (v, used) = varint(data)?;
                    data = &data[used..];
                    values.push(v);
                }
            }
            Count::Bytes(n) => {
                let mut consumed = 0;
                while consumed < n {
                    let (v, used) = varint(data)?;
                    data = &data[used..];
                    consumed += used;
                    values.push(v);
                }
            }
        }
        tags.push((t.tag, values));
    }
    Ok(Entry { label, tags })
}

/// Forward varint: 7 bits per byte, big-endian, the byte with the high bit set is the last.
/// Returns (value, bytes consumed).
pub fn varint(data: &[u8]) -> Result<(u32, usize), MobiError> {
    let mut value: u32 = 0;
    for (i, &b) in data.iter().enumerate().take(5) {
        value = value.checked_mul(128).ok_or(MobiError::Malformed("varint overflow"))? | u32::from(b & 0x7f);
        if b & 0x80 != 0 {
            return Ok((value, i + 1));
        }
    }
    Err(MobiError::Malformed("varint"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Encodes a forward varint.
    pub(crate) fn enc(mut v: u32) -> Vec<u8> {
        let mut out = vec![(v & 0x7f) as u8 | 0x80];
        v >>= 7;
        while v > 0 {
            out.insert(0, (v & 0x7f) as u8);
            v >>= 7;
        }
        out
    }

    /// Builds an INDX header record + one entry record + optional CNCX record.
    /// `tagx` rows are (tag, values per entry, mask, end flag); `entries` are (label, control
    /// byte(s), value bytes).
    pub(crate) fn build_index(
        tagx: &[[u8; 4]],
        control_bytes: u32,
        entries: &[(&[u8], Vec<u8>)],
        cncx: Option<Vec<u8>>,
    ) -> Vec<Vec<u8>> {
        let mut header = vec![0u8; 192];
        header[..4].copy_from_slice(b"INDX");
        header[24..28].copy_from_slice(&1u32.to_be_bytes());
        header[52..56].copy_from_slice(&u32::from(cncx.is_some()).to_be_bytes());
        header[180..184].copy_from_slice(&192u32.to_be_bytes());
        header.extend_from_slice(b"TAGX");
        header.extend_from_slice(&(12 + 4 * tagx.len() as u32).to_be_bytes());
        header.extend_from_slice(&control_bytes.to_be_bytes());
        for row in tagx {
            header.extend_from_slice(row);
        }

        let mut rec = vec![0u8; 192];
        rec[..4].copy_from_slice(b"INDX");
        let mut offsets = Vec::new();
        for (label, body) in entries {
            offsets.push(rec.len() as u16);
            rec.push(label.len() as u8);
            rec.extend_from_slice(label);
            rec.extend_from_slice(body);
        }
        let idxt = rec.len() as u32;
        rec[20..24].copy_from_slice(&idxt.to_be_bytes());
        rec[24..28].copy_from_slice(&(entries.len() as u32).to_be_bytes());
        rec.extend_from_slice(b"IDXT");
        for o in offsets {
            rec.extend_from_slice(&o.to_be_bytes());
        }
        let mut out = vec![header, rec];
        out.extend(cncx);
        out
    }

    #[test]
    fn varints_round_trip() {
        for v in [0, 1, 127, 128, 300, 1 << 20, u32::MAX] {
            assert_eq!(varint(&enc(v)).expect("ok"), (v, enc(v).len()));
        }
        assert!(varint(&[0x01, 0x02]).is_err(), "unterminated");
        assert!(varint(&[0x7f; 6]).is_err(), "too long");
    }

    #[test]
    fn reads_tags_counts_and_cncx() {
        // Tag 1: one value, mask 0x01. Tag 6: two values per entry, mask 0x02.
        // Tag 3: one value, mask 0x0c (multi-bit: value count in the masked bits).
        let tagx = [[1, 1, 0x01, 0], [6, 2, 0x02, 0], [3, 1, 0x0c, 0], [0, 0, 0, 1]];
        let mut e0 = vec![0x01 | 0x02 | 0x08]; // tag 1, tag 6, tag 3 with count 2
        for v in [7, 100, 200, 0, 5] {
            e0.extend(enc(v));
        }
        let e1 = {
            let mut e = vec![0x0c]; // tag 3 with all mask bits set: byte count follows
            e.extend(enc(3));
            e.extend(enc(1));
            e.extend(enc(300)); // 2 bytes
            e
        };
        let mut cncx = Vec::new();
        cncx.extend(enc(5));
        cncx.extend_from_slice(b"Hello");
        let recs = build_index(&tagx, 1, &[(b"0001", e0), (b"0002", e1)], Some(cncx));
        let index = read(|i| recs.get(i).map(Vec::as_slice), 0).expect("parses");
        assert_eq!(index.entries.len(), 2);
        let a = &index.entries[0];
        assert_eq!(a.label, b"0001");
        assert_eq!(a.tag(1), Some(&[7][..]));
        assert_eq!(a.tag(6), Some(&[100, 200][..]));
        assert_eq!(a.tag(3), Some(&[0, 5][..]));
        assert_eq!(index.entries[1].tag(3), Some(&[1, 300][..]));
        assert_eq!(index.entries[1].tag(1), None);
        assert_eq!(index.cncx(a.first(3).expect("tag 3")), Some(&b"Hello"[..]));
        assert_eq!(index.cncx(0x10000), None, "second CNCX record missing");
    }

    #[test]
    fn truncated_index_is_an_error() {
        let tagx = [[1, 1, 0x01, 0], [0, 0, 0, 1]];
        let recs = build_index(&tagx, 1, &[(b"x", vec![0x01, 0x05])], None);
        assert!(read(|i| recs.get(i).map(Vec::as_slice), 0).is_err(), "unterminated varint");
        assert!(read(|i| recs.get(i).map(Vec::as_slice), 1).is_err(), "not a header");
        let one = &recs[..1];
        assert!(read(|i| one.get(i).map(Vec::as_slice), 0).is_err(), "missing entry record");
        let mut cut = recs.clone();
        cut[0].truncate(200);
        assert!(read(|i| cut.get(i).map(Vec::as_slice), 0).is_err(), "truncated TAGX");
    }
}
