//! Random access into StarDict `.dict` files: plain, or dictzip (`.dict.dz`), a gzip file whose
//! `RA` extra field lists the compressed size of each fixed-size chunk. Every chunk is flushed
//! on its own, so reading an article inflates only the one or two chunks under it.

use std::fs::File;
use std::io::{self, Error, ErrorKind};
use std::os::unix::fs::FileExt;
use std::path::Path;

use flate2::{Decompress, FlushDecompress};

pub enum DictFile {
    Plain(File),
    Zip(Zip),
}

pub struct Zip {
    file: File,
    /// Uncompressed size of every chunk but possibly the last.
    chunk_len: usize,
    /// File offset where each chunk's compressed data starts, plus the end of the last.
    starts: Vec<u64>,
}

fn bad(msg: &'static str) -> Error {
    Error::new(ErrorKind::InvalidData, msg)
}

impl DictFile {
    pub fn open(path: &Path) -> io::Result<DictFile> {
        let file = File::open(path)?;
        if path.extension().is_some_and(|e| e == "dz") {
            Zip::new(file).map(DictFile::Zip)
        } else {
            Ok(DictFile::Plain(file))
        }
    }

    /// The `len` bytes at uncompressed offset `at`.
    pub fn read(&self, at: u64, len: usize) -> io::Result<Vec<u8>> {
        match self {
            DictFile::Plain(f) => {
                let mut buf = vec![0; len];
                f.read_exact_at(&mut buf, at)?;
                Ok(buf)
            }
            DictFile::Zip(z) => z.read(at, len),
        }
    }
}

impl Zip {
    fn new(file: File) -> io::Result<Zip> {
        // Fixed header (10 bytes) plus the extra field: XLEN is at most 64 KiB.
        let mut head = vec![0; 10 + 2 + 65535];
        let n = read_up_to(&file, &mut head, 0)?;
        head.truncate(n);
        if head.len() < 12 || head[..3] != [0x1f, 0x8b, 8] {
            return Err(bad("not gzip"));
        }
        let flags = head[3];
        if flags & 4 == 0 {
            return Err(bad("gzip without dictzip index"));
        }
        let xlen = u16::from_le_bytes([head[10], head[11]]) as usize;
        let extra = head.get(12..12 + xlen).ok_or_else(|| bad("short extra field"))?;
        let mut sizes = None;
        let mut chunk_len = 0;
        let mut rest = extra;
        while rest.len() >= 4 {
            let len = u16::from_le_bytes([rest[2], rest[3]]) as usize;
            let data = rest.get(4..4 + len).ok_or_else(|| bad("short subfield"))?;
            if rest[..2] == *b"RA" && data.len() >= 6 {
                chunk_len = u16::from_le_bytes([data[2], data[3]]) as usize;
                let count = u16::from_le_bytes([data[4], data[5]]) as usize;
                let table = data.get(6..6 + 2 * count).ok_or_else(|| bad("short chunk table"))?;
                sizes =
                    Some(table.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u64).collect::<Vec<_>>());
            }
            rest = &rest[4 + len..];
        }
        let sizes = sizes.filter(|_| chunk_len > 0).ok_or_else(|| bad("gzip without dictzip index"))?;
        // Then the optional name, comment and header CRC.
        let mut at = 12 + xlen;
        for flag in [8u8, 16] {
            if flags & flag != 0 {
                let nul = head.get(at..).and_then(|h| h.iter().position(|&b| b == 0)).ok_or_else(|| bad("header"))?;
                at += nul + 1;
            }
        }
        if flags & 2 != 0 {
            at += 2;
        }
        let mut starts = Vec::with_capacity(sizes.len() + 1);
        let mut offset = at as u64;
        starts.push(offset);
        for s in sizes {
            offset += s;
            starts.push(offset);
        }
        Ok(Zip { file, chunk_len, starts })
    }

    fn read(&self, at: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let first = (at / self.chunk_len as u64) as usize;
        let mut skip = (at % self.chunk_len as u64) as usize;
        let mut chunk = first;
        let mut compressed = Vec::new();
        let mut plain = Vec::with_capacity(self.chunk_len);
        while out.len() < len {
            let (Some(&start), Some(&end)) = (self.starts.get(chunk), self.starts.get(chunk + 1)) else {
                return Err(bad("read past the end"));
            };
            compressed.resize((end - start) as usize, 0);
            self.file.read_exact_at(&mut compressed, start)?;
            plain.clear();
            // Each chunk ends in a full flush: a fresh raw inflater decodes it alone.
            Decompress::new(false)
                .decompress_vec(&compressed, &mut plain, FlushDecompress::Sync)
                .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
            let part = plain.get(skip..).ok_or_else(|| bad("short chunk"))?;
            out.extend_from_slice(&part[..part.len().min(len - out.len())]);
            skip = 0;
            chunk += 1;
        }
        Ok(out)
    }
}

/// Reads into `buf` from `at` until it is full or the file ends; the bytes read.
pub fn read_up_to(f: &File, buf: &mut [u8], at: u64) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read_at(&mut buf[n..], at + n as u64)? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    /// Writes `data` as a dictzip file with `chunk_len`-byte chunks, as `dictzip` does.
    pub(crate) fn dictzip(data: &[u8], chunk_len: usize) -> Vec<u8> {
        let mut c = Compress::new(Compression::default(), false);
        let mut chunks = Vec::new();
        for (i, part) in data.chunks(chunk_len).enumerate() {
            let last = (i + 1) * chunk_len >= data.len();
            let mut out = Vec::with_capacity(part.len() + 64);
            let flush = if last { FlushCompress::Finish } else { FlushCompress::Full };
            c.compress_vec(part, &mut out, flush).expect("deflate");
            chunks.push(out);
        }
        let mut extra = b"RA".to_vec();
        extra.extend_from_slice(&(6 + 2 * chunks.len() as u16).to_le_bytes());
        extra.extend_from_slice(&1u16.to_le_bytes());
        extra.extend_from_slice(&(chunk_len as u16).to_le_bytes());
        extra.extend_from_slice(&(chunks.len() as u16).to_le_bytes());
        for ch in &chunks {
            extra.extend_from_slice(&(ch.len() as u16).to_le_bytes());
        }
        // FEXTRA | FNAME, to cover the optional fields.
        let mut file = vec![0x1f, 0x8b, 8, 4 | 8, 0, 0, 0, 0, 0, 3];
        file.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        file.extend_from_slice(&extra);
        file.extend_from_slice(b"test.dict\0");
        for ch in chunks {
            file.extend_from_slice(&ch);
        }
        file.extend_from_slice(&[0; 8]); // CRC and size: not checked
        file
    }

    #[test]
    fn reads_across_chunks() {
        let data: Vec<u8> = (0..20_000u32).flat_map(|i| format!("{i} ").into_bytes()).collect();
        let dir = std::env::temp_dir().join(format!("zarathustra-dz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("t.dict.dz");
        std::fs::write(&path, dictzip(&data, 4096)).expect("write");
        let f = DictFile::open(&path).expect("open");
        for (at, len) in [(0, 10), (4090, 20), (5000, 9000), (data.len() - 7, 7)] {
            assert_eq!(f.read(at as u64, len).expect("read"), &data[at..at + len], "{at}+{len}");
        }
        assert!(f.read(data.len() as u64 - 2, 10).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
