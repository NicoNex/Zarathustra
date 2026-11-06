//! Font library: every font file in a set of directories, indexed by family, weight and
//! style, loaded on first use so dozens of installed fonts cost nothing until a book asks.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use skrifa::attribute::Style;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

use crate::font::Face;

struct Entry {
    path: PathBuf,
    /// Lowercase family name.
    family: String,
    weight: u16,
    italic: bool,
    face: OnceCell<Option<Face>>,
}

pub struct FontLibrary {
    entries: Vec<Entry>,
    /// Lowercase family used for `serif` and whenever nothing else matches.
    default_family: String,
}

/// Families tried for the CSS generic `sans-serif`, in order.
const SANS: &[&str] = &["amazon ember", "helvetica lt", "helvetica", "futura"];

impl FontLibrary {
    pub fn new(default_family: &str) -> Self {
        FontLibrary { entries: Vec::new(), default_family: default_family.to_lowercase() }
    }

    /// Indexes the `.ttf`/`.otf` files in `dir`; unreadable files are skipped. Only the
    /// table directory and the `name`/`OS/2` tables are read (the Kindle has 128 MB of fonts).
    pub fn add_dir(&mut self, dir: &Path) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = read.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            if !matches!(ext.as_deref(), Some("ttf" | "otf")) {
                continue;
            }
            let Some(bytes) = read_sparse(&path, &[*b"name", *b"OS/2", *b"head"]) else { continue };
            let Ok(font) = FontRef::new(&bytes) else { continue };
            let name = |id| font.localized_strings(id).english_or_first().map(|s| s.to_string());
            let Some(family) = name(StringId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| name(StringId::FAMILY_NAME)) else {
                continue;
            };
            let attrs = font.attributes();
            self.entries.push(Entry {
                path,
                family: family.to_lowercase(),
                weight: attrs.weight.value().round() as u16,
                italic: attrs.style != Style::Normal,
                face: OnceCell::new(),
            });
        }
    }

    /// Family names as the fonts spell them (lowercase), deduplicated, in index order.
    pub fn families(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for e in &self.entries {
            if !out.contains(&e.family.as_str()) {
                out.push(&e.family);
            }
        }
        out
    }

    /// Families fit for body text: those with both an upright and an italic style (which
    /// leaves out symbol, CJK and single-style display fonts).
    pub fn reading_families(&self) -> Vec<&str> {
        let has = |f: &str, italic: bool| self.entries.iter().any(|e| e.family == f && e.italic == italic);
        self.families().into_iter().filter(|f| has(f, false) && has(f, true)).collect()
    }

    /// The loaded face with id `id` (ids are entry indices, so they are unique and stable).
    pub fn face(&self, id: u32) -> Option<&Face> {
        let e = self.entries.get(id as usize)?;
        e.face.get_or_init(|| Face::from_bytes(id, std::fs::read(&e.path).ok()?)).as_ref()
    }

    /// Best face for a CSS family list, weight and style: the first listed family that is
    /// installed, else the default; within it the matching style, then the closest weight.
    /// A face whose file fails to load gives way to the next best one.
    pub fn pick(&self, families: &[String], weight: u16, italic: bool) -> Option<&Face> {
        // Called per text run: no allocation on the hit path. Family names are matched
        // ignoring ASCII case only (CSS family names are ASCII in practice).
        let installed =
            |f: &str| self.entries.iter().find(|e| e.family.eq_ignore_ascii_case(f)).map(|e| e.family.as_str());
        let wanted = (families.iter().map(|f| f.trim()))
            .find_map(|f| {
                if f.eq_ignore_ascii_case("serif") {
                    Some(self.default_family.as_str())
                } else if f.eq_ignore_ascii_case("sans-serif") {
                    SANS.iter().find_map(|s| installed(s))
                } else {
                    installed(f)
                }
            })
            .unwrap_or(&self.default_family);
        let rank = |i: &usize| {
            let e = &self.entries[*i];
            (e.family != wanted, e.family != self.default_family, e.italic != italic, e.weight.abs_diff(weight))
        };
        let best = (0..self.entries.len()).min_by_key(rank)?;
        if let Some(face) = self.face(best as u32) {
            return Some(face);
        }
        let mut order: Vec<usize> = (0..self.entries.len()).collect();
        order.sort_by_key(rank);
        order.into_iter().find_map(|i| self.face(i as u32))
    }
}

/// The font file as a buffer of its full length where only the table directory and the
/// `tables` are read; the rest stays zero (and, being freshly allocated zeroes, mostly unmapped).
fn read_sparse(path: &Path, tables: &[[u8; 4]]) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > 256 << 20 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let header = buf.get_mut(..12)?;
    file.read_exact(header).ok()?;
    let tables_n = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let dir_end = 12 + 16 * tables_n;
    file.read_exact(buf.get_mut(12..dir_end)?).ok()?;
    for record in buf[12..dir_end].to_vec().chunks_exact(16) {
        if !tables.iter().any(|t| t[..] == record[..4]) {
            continue;
        }
        let offset = u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as usize;
        let length = u32::from_be_bytes([record[12], record[13], record[14], record[15]]) as usize;
        let dst = buf.get_mut(offset..offset.checked_add(length)?)?;
        file.seek(SeekFrom::Start(offset as u64)).ok()?;
        file.read_exact(dst).ok()?;
    }
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_family_style_and_weight() {
        let mut lib = FontLibrary::new("Bookerly");
        lib.add_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-fonts")));
        if !lib.families().contains(&"bookerly") {
            return; // dev fonts are local-only
        }
        let id = |fam: &[&str], w, i| {
            let fam: Vec<String> = fam.iter().map(|s| s.to_string()).collect();
            let face = lib.pick(&fam, w, i).expect("a face");
            lib.entries[face.id as usize].path.file_name().and_then(|n| n.to_str()).map(str::to_string)
        };
        assert_eq!(id(&["serif"], 400, false).as_deref(), Some("Bookerly-Regular.ttf"));
        assert_eq!(id(&["Nonexistent", "serif"], 700, true).as_deref(), Some("Bookerly-BoldItalic.ttf"));
        assert_eq!(id(&["Baskerville"], 400, false).as_deref(), Some("Baskerville-Regular.ttf"));
        assert_eq!(id(&["Baskerville"], 700, false).as_deref(), Some("Baskerville-Regular.ttf"), "closest weight");
        assert_eq!(id(&["unknown"], 400, true).as_deref(), Some("Bookerly-Italic.ttf"), "default family");
        assert_eq!(id(&["BASKERVILLE "], 400, false).as_deref(), Some("Baskerville-Regular.ttf"), "case, spaces");

        // A perfect match whose file is gone gives way to the next best face. (Fresh library:
        // faces loaded above carry their entry index as id.)
        let mut lib = FontLibrary::new("Bookerly");
        lib.add_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-fonts")));
        let broken = Entry {
            path: PathBuf::from("/nonexistent/Bookerly-Gone.ttf"),
            family: "bookerly".into(),
            weight: 400,
            italic: false,
            face: OnceCell::new(),
        };
        lib.entries.insert(0, broken);
        let face = lib.pick(&["serif".to_string()], 400, false).expect("a face");
        let name = lib.entries[face.id as usize].path.file_name().and_then(|n| n.to_str());
        assert_eq!(name, Some("Bookerly-Regular.ttf"));
    }
}
