//! Text record codecs: PalmDOC LZ77, HUFF/CDIC Huffman, and trailing-entry stripping.

use super::{be_u16, be_u32, MobiError};

/// Bytes at the end of a text record that are not text, per the MOBI header's extra-data flags.
/// Bit 0 is the multibyte-overlap entry (innermost); bits 1..15 are entries (TBS indexing and
/// friends) whose sizes are backward-encoded varints, each including its own size bytes.
pub fn trailing_size(rec: &[u8], flags: u16) -> usize {
    let mut size = 0usize;
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            size = size.saturating_add(backward_varint(rec, rec.len().saturating_sub(size)));
        }
        f >>= 1;
    }
    if flags & 1 != 0 {
        if let Some(&b) = rec.len().checked_sub(size + 1).and_then(|i| rec.get(i)) {
            size += usize::from(b & 3) + 1;
        }
    }
    size.min(rec.len())
}

/// Variable-width integer stored backwards ending at `end`: 7 bits per byte, the byte with the
/// high bit set (read last, i.e. lowest address) terminates it.
fn backward_varint(rec: &[u8], end: usize) -> usize {
    let (mut value, mut shift) = (0usize, 0u32);
    for &b in rec[..end.min(rec.len())].iter().rev() {
        value |= usize::from(b & 0x7f) << shift;
        shift += 7;
        if b & 0x80 != 0 || shift >= 28 {
            break;
        }
    }
    value
}

/// Decompresses one PalmDOC (LZ77) record, appending to `out`. Back-references stay inside the
/// record's own output: records are compressed independently.
pub fn palmdoc(input: &[u8], out: &mut Vec<u8>) -> Result<(), MobiError> {
    let start = out.len();
    let mut i = 0;
    while let Some(&c) = input.get(i) {
        i += 1;
        match c {
            1..=8 => {
                let lit = input.get(i..i + usize::from(c)).ok_or(MobiError::Malformed("LZ77 literal run"))?;
                out.extend_from_slice(lit);
                i += usize::from(c);
            }
            0x80..=0xbf => {
                let &next = input.get(i).ok_or(MobiError::Malformed("LZ77 pair"))?;
                i += 1;
                let pair = u16::from(c) << 8 | u16::from(next);
                let dist = usize::from((pair >> 3) & 0x7ff);
                let len = usize::from(pair & 7) + 3;
                if dist == 0 || dist > out.len() - start {
                    return Err(MobiError::Malformed("LZ77 distance"));
                }
                // Byte by byte: the source may overlap the bytes being written.
                for _ in 0..len {
                    out.push(out[out.len() - dist]);
                }
            }
            0xc0..=0xff => out.extend_from_slice(&[b' ', c ^ 0x80]),
            _ => out.push(c),
        }
    }
    Ok(())
}

/// HUFF/CDIC decoder. Phrases may themselves be compressed and are expanded on first use.
pub struct Huff {
    /// Indexed by the code's top byte: (code length, terminal, max code left-aligned in 32 bits).
    dict1: Vec<(u32, bool, u64)>,
    mincode: [u64; 33],
    maxcode: [u64; 33],
    phrases: Vec<Phrase>,
}

enum Phrase {
    Packed(Vec<u8>),
    Plain(Vec<u8>),
    /// Being expanded right now: seeing it again means a cycle.
    Busy,
}

/// Recursion bound for phrases that reference other compressed phrases.
const MAX_DEPTH: u32 = 32;
/// Output bound per record (records hold 4 KiB of text): stops decompression bombs.
const MAX_OUT: usize = 1 << 20;

impl Huff {
    pub fn new(huff: &[u8], cdics: &[&[u8]]) -> Result<Huff, MobiError> {
        let bad = MobiError::Malformed("HUFF record");
        if huff.get(..8) != Some(b"HUFF\0\0\0\x18") {
            return Err(bad);
        }
        let off1 = be_u32(huff, 8)? as usize;
        let off2 = be_u32(huff, 12)? as usize;
        let mut dict1 = Vec::with_capacity(256);
        for i in 0..256 {
            let v = be_u32(huff, off1.saturating_add(4 * i))?;
            let codelen = v & 0x1f;
            if codelen == 0 {
                return Err(bad);
            }
            let maxcode = ((u64::from(v >> 8) + 1) << (32 - codelen)) - 1;
            dict1.push((codelen, v & 0x80 != 0, maxcode));
        }
        let (mut mincode, mut maxcode) = ([0u64; 33], [0u64; 33]);
        for len in 1..=32u32 {
            let i = off2.saturating_add(8 * (len as usize - 1));
            mincode[len as usize] = u64::from(be_u32(huff, i)?) << (32 - len);
            maxcode[len as usize] = ((u64::from(be_u32(huff, i + 4)?) + 1) << (32 - len)) - 1;
        }
        let mut phrases = Vec::new();
        for cdic in cdics {
            if cdic.get(..8) != Some(b"CDIC\0\0\0\x10") {
                return Err(MobiError::Malformed("CDIC record"));
            }
            let total = be_u32(cdic, 8)? as usize;
            let bits = be_u32(cdic, 12)?.min(16);
            let n = (1usize << bits).min(total.saturating_sub(phrases.len()));
            for j in 0..n {
                let off = 16 + usize::from(be_u16(cdic, 16 + 2 * j)?);
                let blen = be_u16(cdic, off)?;
                let bytes = cdic
                    .get(off + 2..off + 2 + usize::from(blen & 0x7fff))
                    .ok_or(MobiError::Malformed("CDIC phrase"))?
                    .to_vec();
                phrases.push(if blen & 0x8000 != 0 { Phrase::Plain(bytes) } else { Phrase::Packed(bytes) });
            }
        }
        Ok(Huff { dict1, mincode, maxcode, phrases })
    }

    /// Decompresses one record, appending to `out`.
    pub fn unpack(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<(), MobiError> {
        self.unpack_at(data, out, 0)
    }

    fn unpack_at(&mut self, data: &[u8], out: &mut Vec<u8>, depth: u32) -> Result<(), MobiError> {
        if depth > MAX_DEPTH {
            return Err(MobiError::Malformed("HUFF nesting"));
        }
        let bad = || MobiError::Malformed("HUFF code");
        let mut padded = Vec::with_capacity(data.len() + 8);
        padded.extend_from_slice(data);
        padded.extend_from_slice(&[0; 8]);
        let window = |pos: usize| -> u64 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&padded[pos..pos + 8]);
            u64::from_be_bytes(b)
        };
        let start = out.len();
        let mut bits_left = data.len() as i64 * 8;
        let mut pos = 0;
        let mut x = window(pos);
        // Bits of `x` not yet consumed below the current 32-bit code window.
        let mut n: i32 = 32;
        loop {
            if n <= 0 {
                pos += 4;
                x = window(pos);
                n += 32;
            }
            let code = (x >> n) & 0xffff_ffff;
            let (mut codelen, term, mut maxcode) = self.dict1[(code >> 24) as usize];
            if !term {
                while code < self.mincode[codelen as usize] {
                    codelen += 1;
                    if codelen > 32 {
                        return Err(bad());
                    }
                }
                maxcode = self.maxcode[codelen as usize];
            }
            n -= codelen as i32;
            bits_left -= i64::from(codelen);
            if bits_left < 0 {
                return Ok(());
            }
            let r = (maxcode.checked_sub(code).ok_or_else(bad)? >> (32 - codelen)) as usize;
            let phrase = self.phrases.get_mut(r).ok_or_else(bad)?;
            match std::mem::replace(phrase, Phrase::Busy) {
                Phrase::Plain(p) => {
                    out.extend_from_slice(&p);
                    self.phrases[r] = Phrase::Plain(p);
                }
                Phrase::Packed(p) => {
                    let mut plain = Vec::new();
                    self.unpack_at(&p, &mut plain, depth + 1)?;
                    out.extend_from_slice(&plain);
                    self.phrases[r] = Phrase::Plain(plain);
                }
                Phrase::Busy => return Err(MobiError::Malformed("HUFF phrase cycle")),
            }
            if out.len() - start > MAX_OUT {
                return Err(MobiError::Malformed("HUFF output too large"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lz77(input: &[u8]) -> Result<Vec<u8>, MobiError> {
        let mut out = Vec::new();
        palmdoc(input, &mut out).map(|()| out)
    }

    #[test]
    fn palmdoc_literals_runs_spaces_and_backrefs() {
        // 'a', run of 2 raw bytes (0x00 0xc5), space+'E' (0xc5), then copy 3 bytes from distance 4.
        let pair: u16 = 0x8000 | (4 << 3); // len = 0 + 3
        let input = [b'a', 2, 0x00, 0xc5, 0xc5, (pair >> 8) as u8, pair as u8];
        assert_eq!(lz77(&input).expect("ok"), [b'a', 0x00, 0xc5, b' ', b'E', 0x00, 0xc5, b' ']);
    }

    #[test]
    fn palmdoc_overlapping_copy_repeats() {
        let pair: u16 = 0x8000 | (1 << 3) | 4; // dist 1, len 7
        assert_eq!(lz77(&[b'z', (pair >> 8) as u8, pair as u8]).expect("ok"), b"zzzzzzzz");
    }

    #[test]
    fn palmdoc_rejects_bad_input() {
        assert!(lz77(&[0x80, 0x08]).is_err(), "distance past the start");
        assert!(lz77(&[5, b'a']).is_err(), "truncated literal run");
        assert!(lz77(&[0x80]).is_err(), "truncated pair");
    }

    #[test]
    fn trailing_entries_are_stripped() {
        // text "hello", multibyte entry (1 extra byte + count byte 0x01), TBS entry of 3 bytes
        // whose last byte encodes its size (0x83 = 3 | terminator).
        let rec = [b'h', b'e', b'l', b'l', b'o', 0xaa, 0x01, 0x10, 0x20, 0x83];
        assert_eq!(trailing_size(&rec, 0b11), 5);
        assert_eq!(trailing_size(&rec[..7], 0b01), 2);
        assert_eq!(trailing_size(&rec, 0), 0);
        assert_eq!(trailing_size(&[0xff], 0xffff), 1, "never more than the record");
    }

    /// Builds HUFF/CDIC records for a fixed 2-bit code: code `i` (0..4) maps to `phrases[i]`.
    fn huff_tables(phrases: &[(&[u8], bool)]) -> (Vec<u8>, Vec<u8>) {
        let mut huff = b"HUFF\0\0\0\x18".to_vec();
        huff.extend_from_slice(&24u32.to_be_bytes());
        huff.extend_from_slice(&(24 + 1024u32).to_be_bytes());
        huff.extend_from_slice(&[0; 8]);
        // dict1: every top byte is a terminal 2-bit code with max code 3, and codes decode as
        // r = maxcode - code: top bits 11 → phrase 0 … 00 → phrase 3.
        for _ in 0..256 {
            huff.extend_from_slice(&((3u32 << 8) | 0x80 | 2).to_be_bytes());
        }
        for _ in 0..32 {
            huff.extend_from_slice(&0u32.to_be_bytes());
            huff.extend_from_slice(&u32::MAX.to_be_bytes());
        }
        let mut cdic = b"CDIC\0\0\0\x10".to_vec();
        cdic.extend_from_slice(&(phrases.len() as u32).to_be_bytes());
        cdic.extend_from_slice(&2u32.to_be_bytes());
        let mut offsets = Vec::new();
        let mut bodies = Vec::new();
        let base = 2 * phrases.len();
        for (p, plain) in phrases {
            offsets.push((base + bodies.len()) as u16);
            let flag = if *plain { 0x8000 } else { 0 };
            bodies.extend_from_slice(&(p.len() as u16 | flag).to_be_bytes());
            bodies.extend_from_slice(p);
        }
        for o in offsets {
            cdic.extend_from_slice(&o.to_be_bytes());
        }
        cdic.extend_from_slice(&bodies);
        (huff, cdic)
    }

    #[test]
    fn huff_decodes_plain_and_nested_phrases() {
        // Code bits 11 → phrase 0, 10 → 1, 01 → 2, 00 → 3. Phrase 3 is itself compressed.
        let (huff, cdic) = huff_tables(&[(b"a", true), (b"b", true), (b" ", true), (&[0b1110_0101], false)]);
        let mut h = Huff::new(&huff, &[&cdic]).expect("tables");
        let mut out = Vec::new();
        // 11 01 10 00 → "a", " ", "b", phrase 3 (11 10 01 01 → "ab  ").
        h.unpack(&[0b1101_1000], &mut out).expect("unpacks");
        assert_eq!(out, b"a bab  ");
        // Expanded phrases are cached: a second record reuses them.
        out.clear();
        h.unpack(&[0b0000_0000], &mut out).expect("unpacks");
        assert_eq!(out, b"ab  ab  ab  ab  ");
    }

    #[test]
    fn huff_rejects_garbage() {
        assert!(Huff::new(b"HUFF", &[]).is_err());
        let (huff, _) = huff_tables(&[(b"a", true)]);
        assert!(Huff::new(&huff, &[b"nope"]).is_err());
        let mut h = Huff::new(&huff, &[]).expect("no phrases");
        assert!(h.unpack(&[0], &mut Vec::new()).is_err(), "code past the dictionary");
        // A phrase that decodes to itself is a cycle, not a stack overflow.
        let (huff, cdic) = huff_tables(&[(&[0xff], false)]);
        let mut h = Huff::new(&huff, &[&cdic]).expect("tables");
        assert!(h.unpack(&[0xff], &mut Vec::new()).is_err());
    }
}
