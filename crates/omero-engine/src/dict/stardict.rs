//! StarDict dictionaries (`.ifo` + `.idx` + `.dict[.dz]`, optional `.syn`), read in place.
//!
//! The index is never loaded: a sparse table of every [`BLOCK`]th entry's file offset is built
//! once (one sequential pass, then cached on disk), and a lookup binary-searches it with a few
//! small positioned reads, then scans one block. Articles are read by offset from the
//! `.dict` file (see [`super::dictzip`]).

use std::cmp::Ordering;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Error, ErrorKind, Read};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use super::dictzip::{read_up_to, DictFile};

/// Entries per sparse-table slot: a lookup scans at most this many (~1–2 KiB).
const BLOCK: usize = 32;
/// Headwords are at most 255 bytes in StarDict.
const MAX_WORD: usize = 256;

/// One dictionary article: the headword it is filed under and its body, as HTML-ish markup or
/// plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    pub headword: String,
    pub body: String,
    /// The body is markup (`h`, `g`, `x` types) rather than plain text.
    pub html: bool,
}

pub struct StarDict {
    pub name: String,
    /// Language of the headwords (ISO 639-1), when the file names or title tell it.
    pub lang: Option<String>,
    /// Language of the definitions.
    pub target: Option<String>,
    idx: Table,
    syn: Option<Table>,
    dict: DictFile,
    /// `sametypesequence`: the field types every article has, or `None` when each article
    /// lists its own.
    types: Option<Vec<u8>>,
}

/// StarDict's collation: ASCII-case-insensitive, then bytewise (`stardict_strcmp`). Lookups
/// only need its first half ([`fold`]); tests sort with it.
#[cfg(test)]
fn collate(a: &[u8], b: &[u8]) -> Ordering {
    fold(a, b).then_with(|| a.cmp(b))
}

/// ASCII-case-insensitive comparison.
fn fold(a: &[u8], b: &[u8]) -> Ordering {
    a.iter().map(u8::to_ascii_lowercase).cmp(b.iter().map(u8::to_ascii_lowercase))
}

/// An entry found: its number, headword and tail.
type Hit = (usize, Vec<u8>, Vec<u8>);

/// A sorted file of `word\0` + fixed-size tail records (`.idx`: offset and size; `.syn`:
/// an entry number in the `.idx`).
struct Table {
    file: File,
    path: PathBuf,
    len: u64,
    tail: usize,
    cache_dir: Option<PathBuf>,
    /// File offset of every BLOCKth entry.
    sparse: OnceLock<Vec<u32>>,
    /// Held while `sparse` is built: a lookup during the background warm-up waits for it
    /// rather than reading the whole index a second time.
    building: Mutex<()>,
}

impl Table {
    fn open(path: &Path, tail: usize, cache_dir: Option<&Path>) -> io::Result<Table> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len > u32::MAX as u64 {
            return Err(Error::new(ErrorKind::InvalidData, "index over 4 GiB"));
        }
        Ok(Table {
            file,
            path: path.into(),
            len,
            tail,
            cache_dir: cache_dir.map(Into::into),
            sparse: OnceLock::new(),
            building: Mutex::new(()),
        })
    }

    fn sparse(&self) -> io::Result<&[u32]> {
        if let Some(s) = self.sparse.get() {
            return Ok(s);
        }
        let _building = self.building.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(s) = self.sparse.get() {
            return Ok(s);
        }
        let cache = self.cache_file();
        let table = match cache.as_deref().and_then(|c| self.load_cache(c)) {
            Some(t) => t,
            None => {
                let t = self.build()?;
                if let Some(c) = &cache {
                    if let Err(e) = self.save_cache(c, &t) {
                        eprintln!("dict cache {}: {e}", c.display());
                    }
                }
                t
            }
        };
        Ok(self.sparse.get_or_init(|| table))
    }

    /// One pass over the file, noting where every BLOCKth entry starts.
    fn build(&self) -> io::Result<Vec<u32>> {
        let mut r = BufReader::with_capacity(1 << 16, &self.file);
        let mut out = Vec::new();
        let (mut at, mut n, mut word, mut tail) = (0u64, 0usize, Vec::new(), [0u8; 12]);
        while at < self.len {
            if n % BLOCK == 0 {
                out.push(at as u32);
            }
            word.clear();
            let w = r.read_until(0, &mut word)?;
            r.read_exact(&mut tail[..self.tail])?;
            at += (w + self.tail) as u64;
            n += 1;
        }
        Ok(out)
    }

    /// `<cache_dir>/<hash of path>.oft`, keyed also by size and mtime inside.
    fn cache_file(&self) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        let hash = (self.path.as_os_str().as_encoded_bytes().iter())
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        Some(dir.join(format!("{hash:016x}.oft")))
    }

    fn stamp(&self) -> [u8; 24] {
        let mtime = (self.file.metadata().and_then(|m| m.modified()).ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let mut s = [0u8; 24];
        s[..8].copy_from_slice(b"omeroft1");
        s[8..16].copy_from_slice(&self.len.to_le_bytes());
        s[16..].copy_from_slice(&mtime.to_le_bytes());
        s
    }

    fn load_cache(&self, path: &Path) -> Option<Vec<u32>> {
        let data = std::fs::read(path).ok()?;
        let body = data.strip_prefix(&self.stamp()[..])?;
        let table: Vec<u32> = body.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        (!table.is_empty() && body.len() % 4 == 0).then_some(table)
    }

    fn save_cache(&self, path: &Path, table: &[u32]) -> io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let mut data = self.stamp().to_vec();
        data.extend(table.iter().flat_map(|v| v.to_le_bytes()));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, path)
    }

    /// The first headword of block `b`.
    fn first_word(&self, starts: &[u32], b: usize) -> io::Result<Vec<u8>> {
        let mut buf = [0u8; MAX_WORD];
        let n = read_up_to(&self.file, &mut buf, starts[b] as u64)?;
        let end = buf[..n].iter().position(|&c| c == 0).unwrap_or(n);
        Ok(buf[..end].to_vec())
    }

    /// The raw bytes of block `b`.
    fn block(&self, starts: &[u32], b: usize) -> io::Result<Vec<u8>> {
        let start = starts[b] as u64;
        let end = starts.get(b + 1).map_or(self.len, |&e| e as u64);
        let mut buf = vec![0; (end - start) as usize];
        self.file.read_exact_at(&mut buf, start)?;
        Ok(buf)
    }

    /// Entries of a block: (headword, tail).
    fn entries(&self, block: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut out = Vec::with_capacity(BLOCK);
        let mut rest = block;
        while let Some(nul) = rest.iter().position(|&c| c == 0) {
            let Some(tail) = rest.get(nul + 1..nul + 1 + self.tail) else { break };
            out.push((rest[..nul].to_vec(), tail.to_vec()));
            rest = &rest[nul + 1 + self.tail..];
        }
        out
    }

    /// Every entry whose headword equals `word` ignoring ASCII case, in file order, with its
    /// entry number.
    fn find(&self, word: &[u8]) -> io::Result<Vec<Hit>> {
        let starts = self.sparse()?;
        // The first block starting at or after `word`; matches may begin in the one before.
        let (mut lo, mut hi) = (0, starts.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if fold(&self.first_word(starts, mid)?, word) == Ordering::Less {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let mut out = Vec::new();
        for b in lo.saturating_sub(1)..starts.len() {
            for (i, (w, tail)) in self.entries(&self.block(starts, b)?).into_iter().enumerate() {
                match fold(&w, word) {
                    Ordering::Less => {}
                    Ordering::Equal => out.push((b * BLOCK + i, w, tail)),
                    Ordering::Greater => return Ok(out),
                }
            }
        }
        Ok(out)
    }

    /// Entry number `n`.
    fn nth(&self, n: usize) -> io::Result<Option<(Vec<u8>, Vec<u8>)>> {
        let starts = self.sparse()?;
        if n / BLOCK >= starts.len() {
            return Ok(None);
        }
        Ok(self.entries(&self.block(starts, n / BLOCK)?).into_iter().nth(n % BLOCK))
    }
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl StarDict {
    /// Opens the dictionary described by `ifo`. `cache_dir` keeps the sparse index tables.
    pub fn open(ifo: &Path, cache_dir: Option<&Path>) -> io::Result<StarDict> {
        let info = std::fs::read_to_string(ifo)?;
        if !info.starts_with("StarDict's dict ifo file") {
            return Err(Error::new(ErrorKind::InvalidData, "not a StarDict .ifo"));
        }
        let field =
            |key: &str| info.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim().to_string());
        let offset_bits = field("idxoffsetbits").and_then(|b| b.parse().ok()).unwrap_or(32);
        let tail = if offset_bits == 64 { 12 } else { 8 };
        let base = ifo.with_extension("");
        let with = |ext: &str| PathBuf::from(format!("{}.{ext}", base.display()));
        if with("idx.gz").exists() && !with("idx").exists() {
            // ponytail: compressed indexes are rare (KOReader's own packs ship plain .idx);
            // inflate them into the cache dir if users bring some.
            return Err(Error::new(ErrorKind::Unsupported, ".idx.gz: gunzip the index"));
        }
        let idx = Table::open(&with("idx"), tail, cache_dir)?;
        let syn = Table::open(&with("syn"), 4, cache_dir).ok();
        let dict = DictFile::open(&with("dict.dz")).or_else(|_| DictFile::open(&with("dict")))?;
        let name = field("bookname").unwrap_or_else(|| base.file_name().unwrap_or_default().to_string_lossy().into());
        let stem = base.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let (lang, target) = super::languages(&stem, &name);
        Ok(StarDict {
            lang,
            target,
            name,
            idx,
            syn,
            dict,
            types: field("sametypesequence").map(String::into_bytes).filter(|t| !t.is_empty()),
        })
    }

    /// Builds (or loads) the sparse index tables.
    pub fn warm(&self) -> io::Result<()> {
        self.idx.sparse()?;
        self.syn.as_ref().map_or(Ok(()), |s| s.sparse().map(|_| ()))
    }

    /// Articles filed under `word` (ignoring ASCII case; exact-case ones first), then those
    /// of the headwords `word` is a synonym or inflected form of.
    pub fn lookup(&self, word: &str) -> io::Result<Vec<Article>> {
        let key = word.as_bytes();
        if key.is_empty() || key.len() >= MAX_WORD || key.contains(&0) {
            return Ok(Vec::new());
        }
        let mut hits: Vec<(Vec<u8>, Vec<u8>)> = self.idx.find(key)?.into_iter().map(|(_, w, t)| (w, t)).collect();
        if let Some(syn) = &self.syn {
            for (_, _, tail) in syn.find(key)? {
                if let Some(hit) = self.idx.nth(be32(&tail) as usize)? {
                    hits.push(hit);
                }
            }
        }
        // Exact case first, then the rest in order; the same article once.
        hits.sort_by_key(|(w, _)| w.as_slice() != key);
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for (w, tail) in hits {
            if seen.contains(&tail) {
                continue;
            }
            let (off, size) = match tail.len() {
                12 => (u64::from_be_bytes(tail[..8].try_into().unwrap_or_default()), be32(&tail[8..])),
                _ => (be32(&tail) as u64, be32(&tail[4..])),
            };
            let data = self.dict.read(off, size as usize)?;
            seen.push(tail);
            if let Some((body, html)) = self.body(&data) {
                out.push(Article { headword: String::from_utf8_lossy(&w).into_owned(), body, html });
            }
        }
        Ok(out)
    }

    /// The text fields of an article, joined; markup when any is.
    fn body(&self, data: &[u8]) -> Option<(String, bool)> {
        let mut parts: Vec<(u8, &[u8])> = Vec::new();
        let mut rest = data;
        let mut i = 0;
        loop {
            let ty = match &self.types {
                Some(t) => match t.get(i) {
                    Some(&c) => c,
                    None => break,
                },
                None => match rest.split_first() {
                    Some((&c, r)) => {
                        rest = r;
                        c
                    }
                    None => break,
                },
            };
            let last = self.types.as_ref().is_some_and(|t| i + 1 == t.len());
            let field = if last {
                std::mem::take(&mut rest)
            } else if ty.is_ascii_lowercase() {
                let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
                let (f, r) = rest.split_at(end);
                rest = r.get(1..).unwrap_or_default();
                f
            } else {
                let size = rest.get(..4).map_or(0, be32) as usize;
                let f = rest.get(4..4 + size).unwrap_or_default();
                rest = rest.get(4 + size..).unwrap_or_default();
                f
            };
            parts.push((ty, field));
            i += 1;
        }
        let text: Vec<_> = parts.iter().filter(|(t, _)| b"mltyghx".contains(t)).collect();
        if text.is_empty() {
            return None;
        }
        let html = text.iter().any(|(t, _)| b"ghx".contains(t));
        let body = text.iter().map(|(_, f)| String::from_utf8_lossy(f)).collect::<Vec<_>>().join("\n");
        Some((body, html))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Writes a StarDict named `name` into `dir`: `words` sorted as StarDict sorts them,
    /// articles of type `h`, dictzipped; `syn` maps forms to headwords.
    pub(crate) fn write_dict(dir: &Path, name: &str, words: &[(&str, &str)], syn: &[(&str, &str)]) -> PathBuf {
        let mut words = words.to_vec();
        words.sort_by(|a, b| collate(a.0.as_bytes(), b.0.as_bytes()));
        let (mut idx, mut dict) = (Vec::new(), Vec::new());
        for (w, def) in &words {
            idx.extend_from_slice(w.as_bytes());
            idx.push(0);
            idx.extend_from_slice(&(dict.len() as u32).to_be_bytes());
            idx.extend_from_slice(&(def.len() as u32).to_be_bytes());
            dict.extend_from_slice(def.as_bytes());
        }
        let mut syn = syn.to_vec();
        syn.sort_by(|a, b| collate(a.0.as_bytes(), b.0.as_bytes()));
        let mut synf = Vec::new();
        for (form, head) in &syn {
            let n = words.iter().position(|(w, _)| w == head).expect("syn target") as u32;
            synf.extend_from_slice(form.as_bytes());
            synf.push(0);
            synf.extend_from_slice(&n.to_be_bytes());
        }
        std::fs::create_dir_all(dir).expect("dir");
        let ifo = dir.join(format!("{name}.ifo"));
        std::fs::write(
            &ifo,
            format!(
                "StarDict's dict ifo file\nversion=3.0.0\nbookname=Test {name}\nwordcount={}\n\
                 idxfilesize={}\nsametypesequence=h\n",
                words.len(),
                idx.len()
            ),
        )
        .expect("ifo");
        std::fs::write(dir.join(format!("{name}.idx")), idx).expect("idx");
        if !syn.is_empty() {
            std::fs::write(dir.join(format!("{name}.syn")), synf).expect("syn");
        }
        let dz = crate::dict::dictzip::tests::dictzip(&dict, 512);
        std::fs::write(dir.join(format!("{name}.dict.dz")), dz).expect("dict");
        ifo
    }

    pub(crate) fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omero-dict-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        d
    }

    #[test]
    fn looks_up_words_case_folded_and_through_synonyms() {
        let dir = temp("sd");
        // Enough words for many blocks, and articles spanning dictzip chunks.
        let filler: Vec<(String, String)> =
            (0..500).map(|i| (format!("w{i:03}"), format!("<p>article {i} {}</p>", "x".repeat(i % 90)))).collect();
        let mut words: Vec<(&str, &str)> = filler.iter().map(|(w, d)| (w.as_str(), d.as_str())).collect();
        words.extend([
            ("casa", "<p>edificio</p>"),
            ("Casa", "<p>dinastia</p>"),
            ("andare", "<p>muoversi</p>"),
            ("zeta", "<p>ultima</p>"),
        ]);
        let ifo = write_dict(&dir, "it-it", &words, &[("andai", "andare"), ("vado", "andare")]);
        let d = StarDict::open(&ifo, Some(&dir.join("cache"))).expect("open");
        assert_eq!(d.lang.as_deref(), Some("it"));
        let heads = |w: &str| d.lookup(w).expect("lookup").into_iter().map(|a| a.headword).collect::<Vec<_>>();
        assert_eq!(heads("Casa"), ["Casa", "casa"]);
        assert_eq!(heads("casa"), ["casa", "Casa"]);
        assert_eq!(heads("CASA").len(), 2);
        assert_eq!(heads("vado"), ["andare"]);
        assert_eq!(heads("zeta"), ["zeta"]);
        assert!(heads("zz").is_empty() && heads("").is_empty() && heads("a").is_empty());
        let a = d.lookup("w321").expect("lookup");
        assert_eq!(a[0].body, format!("<p>article 321 {}</p>", "x".repeat(321 % 90)));
        assert!(a[0].html);
        for (w, def) in &words {
            assert!(d.lookup(w).expect("lookup").iter().any(|a| a.body == *def), "{w}");
        }
        // The sparse table was cached; a second open reads it back.
        assert_eq!(std::fs::read_dir(dir.join("cache")).expect("cache").count(), 2);
        let again = StarDict::open(&ifo, Some(&dir.join("cache"))).expect("open");
        assert_eq!(again.lookup("andai").expect("lookup")[0].headword, "andare");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn articles_without_a_type_sequence_list_their_fields() {
        let dir = temp("types");
        let ifo = write_dict(&dir, "en", &[("x", "")], &[]);
        let mut d = StarDict::open(&ifo, None).expect("open");
        d.types = None;
        let mut data = b"mplain\0".to_vec();
        data.extend_from_slice(b"W");
        data.extend_from_slice(&3u32.to_be_bytes());
        data.extend_from_slice(b"wav");
        data.extend_from_slice(b"h<b>bold</b>\0");
        assert_eq!(d.body(&data), Some(("plain\n<b>bold</b>".into(), true)));
        d.types = Some(b"m".to_vec());
        assert_eq!(d.body(b"just text"), Some(("just text".into(), false)));
        std::fs::remove_dir_all(dir).ok();
    }
}
