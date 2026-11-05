//! The book library behind the home screen: scan folders, read metadata, make cover
//! thumbnails, and cache it all so an unchanged library rescans without opening any book.

use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::epub::{Epub, Metadata};
use crate::image;

/// Size the cover thumbnails are fitted into.
pub const THUMB_W: u32 = 340;
pub const THUMB_H: u32 = 510;

const CACHE_FILE: &str = "library.tsv";
// Bumped when entries gain data (2: AZW3/MOBI metadata and covers; 3: grid-size thumbs).
const CACHE_HEADER: &str = "omero-library 3";
const THUMB_DIR: &str = "thumbs";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Epub,
    Kepub,
    Azw3,
    Mobi,
    Pdf,
    Txt,
}

impl Format {
    /// Format by file extension (`.kepub.epub` counts as Kepub).
    pub fn from_path(path: &Path) -> Option<Format> {
        let name = path.file_name()?.to_str()?.to_ascii_lowercase();
        let ext = name.rsplit_once('.')?.1;
        Some(match ext {
            "epub" if name.ends_with(".kepub.epub") => Format::Kepub,
            "epub" => Format::Epub,
            "kepub" => Format::Kepub,
            "azw3" | "azw" => Format::Azw3,
            "mobi" | "prc" => Format::Mobi,
            "pdf" => Format::Pdf,
            "txt" => Format::Txt,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookEntry {
    pub path: PathBuf,
    pub title: String,
    pub authors: Vec<String>,
    pub language: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<f32>,
    pub format: Format,
    pub size: u64,
    pub modified: SystemTime,
    /// Grayscale PNG of the cover, at most [`THUMB_W`]×[`THUMB_H`], in the cache directory.
    pub cover_thumb: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Title,
    /// First author's "Last, First" key, then title.
    Author,
    /// Newest file modification first (calibre and USB copies set it to the time added).
    Recent,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Library {
    pub books: Vec<BookEntry>,
}

impl Library {
    /// Walks `dirs` recursively (skipping hidden folders and KOReader `*.sdr` sidecars) and
    /// returns every book found, sorted by path. Metadata and thumbnails come from the cache in
    /// `cache_dir` when the file's size and mtime are unchanged; the cache is rewritten after.
    /// The cache describes exactly the last scan, so pass all library roots in one call rather
    /// than scanning them separately into the same `cache_dir`.
    pub fn scan(dirs: &[&Path], cache_dir: &Path) -> Library {
        let cache = load_cache(cache_dir);
        let mut files = Vec::new();
        for dir in dirs {
            walk(dir, &mut files);
        }
        files.sort();
        files.dedup();
        let thumbs = cache_dir.join(THUMB_DIR);
        let (books, cacheable): (Vec<BookEntry>, Vec<bool>) = files
            .into_iter()
            .filter_map(|path| {
                let format = Format::from_path(&path)?;
                let meta = fs::metadata(&path).ok().filter(|m| m.is_file())?;
                let (size, modified) = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
                if let Some(hit) = cache.get(&path) {
                    let fresh = hit.size == size && hit.modified == modified;
                    if fresh && hit.cover_thumb.as_ref().is_none_or(|t| t.is_file()) {
                        return Some((hit.clone(), true));
                    }
                }
                Some(read_entry(path, format, size, modified, &thumbs))
            })
            .unzip();
        let library = Library { books };
        let cached: Vec<&BookEntry> = library.books.iter().zip(cacheable).filter_map(|(b, c)| c.then_some(b)).collect();
        // ponytail: a read-only or full cache dir just means slower rescans; nothing to report.
        let _ = save_cache(cache_dir, &cached);
        library
    }

    /// Indices of `books` in the given order.
    pub fn sorted(&self, by: SortBy) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.books.len()).collect();
        match by {
            SortBy::Title => {
                let keys: Vec<String> = self.books.iter().map(|b| fold(&b.title)).collect();
                idx.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
            }
            SortBy::Author => {
                let keys: Vec<(String, String)> = self
                    .books
                    .iter()
                    .map(|b| (b.authors.first().map(|a| fold(&author_sort_key(a))).unwrap_or_default(), fold(&b.title)))
                    .collect();
                // Books without an author go last.
                idx.sort_by(|&a, &b| (keys[a].0.is_empty(), &keys[a]).cmp(&(keys[b].0.is_empty(), &keys[b])));
            }
            SortBy::Recent => idx.sort_by(|&a, &b| self.books[b].modified.cmp(&self.books[a].modified)),
        }
        idx
    }

    /// Groups by author ("Last, First", so both spellings meet); a book with several authors
    /// appears under each. The label is empty for books without an author.
    pub fn group_by_author(&self) -> Vec<(String, Vec<usize>)> {
        self.group(|b| b.authors.iter().map(|a| author_sort_key(a)).collect())
    }

    /// Groups by series, members ordered by series index. Empty label: not in a series.
    pub fn group_by_series(&self) -> Vec<(String, Vec<usize>)> {
        let mut groups = self.group(|b| vec![b.series.clone().unwrap_or_default()]);
        for (_, members) in &mut groups {
            members.sort_by(|&a, &b| {
                let index = |i: usize| self.books[i].series_index.unwrap_or(f32::MAX);
                index(a).total_cmp(&index(b))
            });
        }
        groups
    }

    /// Groups by language tag. Empty label: unknown.
    pub fn group_by_language(&self) -> Vec<(String, Vec<usize>)> {
        self.group(|b| vec![b.language.clone().unwrap_or_default()])
    }

    /// Groups by containing folder (full path).
    pub fn group_by_folder(&self) -> Vec<(String, Vec<usize>)> {
        self.group(|b| vec![b.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()])
    }

    /// Groups sorted by folded label (empty label last); members in title order.
    /// Labels that differ only in case or accents merge under the first spelling seen.
    fn group(&self, labels: impl Fn(&BookEntry) -> Vec<String>) -> Vec<(String, Vec<usize>)> {
        let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
        let mut by_key: HashMap<String, usize> = HashMap::new();
        for i in self.sorted(SortBy::Title) {
            let mut labels = labels(&self.books[i]);
            if labels.is_empty() {
                labels.push(String::new());
            }
            for label in labels {
                let key = fold(&label);
                let g = *by_key.entry(key.clone()).or_insert_with(|| {
                    groups.push((key, label, Vec::new()));
                    groups.len() - 1
                });
                groups[g].2.push(i);
            }
        }
        groups.sort_by(|a, b| (a.0.is_empty(), &a.0).cmp(&(b.0.is_empty(), &b.0)));
        groups.into_iter().map(|(_, label, members)| (label, members)).collect()
    }

    /// Books whose title, authors or series contain every word of `query`, ignoring case and
    /// accents. In title order; an empty query matches everything.
    pub fn search(&self, query: &str) -> Vec<usize> {
        let words: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
        self.sorted(SortBy::Title)
            .into_iter()
            .filter(|&i| {
                let b = &self.books[i];
                let hay = fold(&format!("{} {} {}", b.title, b.authors.join(" "), b.series.as_deref().unwrap_or("")));
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }
}

/// "Last, First" from either "First Last" or "Last, First".
pub fn author_sort_key(name: &str) -> String {
    let name = name.trim();
    if name.contains(',') {
        return name.to_string();
    }
    match name.rsplit_once(char::is_whitespace) {
        Some((first, last)) => format!("{last}, {}", first.trim_end()),
        None => name.to_string(),
    }
}

/// Lowercases and strips Latin diacritics, for search and sorting.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            '\u{300}'..='\u{36f}' => {} // combining marks of decomposed text
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            'þ' => out.push_str("th"),
            _ => out.push(fold_char(c)),
        }
    }
    out
}

fn fold_char(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' | 'ð' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
        'ţ' | 'ť' | 'ŧ' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => c,
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `file_type` does not follow symlinks, so symlinked folders cannot loop the walk.
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if !name.ends_with(".sdr") {
                    walk(&path, out);
                }
            }
            Ok(_) if Format::from_path(&path).is_some() => out.push(path),
            _ => {}
        }
    }
}

/// The entry for a book not in the cache, and whether it may be cached: not when it has a
/// cover whose thumbnail could not be written (a full disk), or it would stay coverless.
fn read_entry(path: PathBuf, format: Format, size: u64, modified: SystemTime, thumbs: &Path) -> (BookEntry, bool) {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut entry = BookEntry {
        title: stem.strip_suffix(".kepub").unwrap_or(&stem).to_string(),
        path,
        authors: Vec::new(),
        language: None,
        series: None,
        series_index: None,
        format,
        size,
        modified,
        cover_thumb: None,
    };
    let Some((m, cover)) = read_book(&entry.path, format) else { return (entry, true) };
    if let Some(title) = m.title.filter(|t| !t.trim().is_empty()) {
        entry.title = title;
    }
    entry.authors = m.authors;
    entry.language = m.language;
    entry.series = m.series;
    entry.series_index = m.series_index;
    // An undecodable cover (WebP, corrupt) is final; a failed write is not.
    let Some(png) = cover.as_deref().and_then(thumb_png) else { return (entry, true) };
    let dest = thumb_path(thumbs, &entry);
    entry.cover_thumb = write_atomic(&dest, &png).ok().map(|()| dest);
    let cacheable = entry.cover_thumb.is_some();
    (entry, cacheable)
}

/// Metadata and cover image bytes, or `None` when the book cannot be read.
fn read_book(path: &Path, format: Format) -> Option<(Metadata, Option<Vec<u8>>)> {
    match format {
        Format::Epub | Format::Kepub => {
            let mut epub = Epub::open(BufReader::new(fs::File::open(path).ok()?)).ok()?;
            let cover = epub.cover.clone().and_then(|c| epub.read(&c).ok());
            Some((std::mem::take(&mut epub.metadata), cover))
        }
        Format::Azw3 | Format::Mobi => {
            let mut mobi = crate::mobi::Mobi::open(path).ok()?;
            let cover = mobi.cover();
            Some((std::mem::take(&mut mobi.metadata), cover))
        }
        // ponytail: PDF and TXT keep the file stem as title until their readers land.
        Format::Pdf | Format::Txt => None,
    }
}

/// Thumbnail file name: the cache key (path, size, mtime) hashed, so a changed book gets a new one.
fn thumb_path(thumbs: &Path, entry: &BookEntry) -> PathBuf {
    // FNV-1a: stable across Rust releases, unlike `DefaultHasher`.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let key = format!("{}\0{}\0{}", entry.path.to_string_lossy(), entry.size, nanos(entry.modified));
    for b in key.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    thumbs.join(format!("{h:016x}.png"))
}

/// The cover fitted, dithered to 16 grays and encoded as PNG; `None` if it does not decode.
fn thumb_png(cover: &[u8]) -> Option<Vec<u8>> {
    let mut img = image::decode(cover, THUMB_W, THUMB_H)?;
    image::quantize16(&mut img);
    let opts = zune_core::options::EncoderOptions::new(
        img.width as usize,
        img.height as usize,
        zune_core::colorspace::ColorSpace::Luma,
        zune_core::bit_depth::BitDepth::Eight,
    );
    let mut png = Vec::new();
    zune_png::PngEncoder::new(&img.pixels, opts).encode(&mut png).ok()?;
    Some(png)
}

/// Writes through a temporary file and a rename, so a crash or a full disk never leaves a
/// truncated file under `dest` (a leftover temporary is cleaned up with the stale thumbnails).
fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)
}

fn nanos(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos().min(u128::from(u64::MAX)) as u64)
}

// Cache format: a header line, then one tab-separated line per book:
// path, size, mtime (ns), title, authors (\x1f-separated), language, series, series index, thumb.
// Fields are escaped (`\\`, `\t`, `\n`, `\u` for \x1f); an empty optional field means none.

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\x1f' => out.push_str("\\u"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('u') => out.push('\x1f'),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

/// Writes `books` as the cache and deletes the thumbnails none of them uses.
fn save_cache(cache_dir: &Path, books: &[&BookEntry]) -> std::io::Result<()> {
    let mut text = format!("{CACHE_HEADER}\n");
    for b in books {
        let authors: Vec<String> = b.authors.iter().map(|a| escape(a)).collect();
        let fields = [
            escape(&b.path.to_string_lossy()),
            b.size.to_string(),
            nanos(b.modified).to_string(),
            escape(&b.title),
            authors.join("\x1f"),
            escape(b.language.as_deref().unwrap_or("")),
            escape(b.series.as_deref().unwrap_or("")),
            b.series_index.map(|i| i.to_string()).unwrap_or_default(),
            b.cover_thumb.as_ref().map(|t| escape(&t.to_string_lossy())).unwrap_or_default(),
        ];
        text.push_str(&fields.join("\t"));
        text.push('\n');
    }
    write_atomic(&cache_dir.join(CACHE_FILE), text.as_bytes())?;
    remove_stale_thumbs(cache_dir, books);
    Ok(())
}

/// Deletes thumbnails no book refers to any more (removed or changed books, failed writes).
fn remove_stale_thumbs(cache_dir: &Path, books: &[&BookEntry]) {
    let Ok(entries) = fs::read_dir(cache_dir.join(THUMB_DIR)) else { return };
    let live: std::collections::HashSet<&Path> = books.iter().filter_map(|b| b.cover_thumb.as_deref()).collect();
    for path in entries.flatten().map(|e| e.path()) {
        if !live.contains(path.as_path()) {
            let _ = fs::remove_file(path);
        }
    }
}

fn load_cache(cache_dir: &Path) -> HashMap<PathBuf, BookEntry> {
    let Ok(text) = fs::read_to_string(cache_dir.join(CACHE_FILE)) else { return HashMap::new() };
    let mut lines = text.lines();
    if lines.next() != Some(CACHE_HEADER) {
        return HashMap::new();
    }
    lines.filter_map(parse_line).map(|b| (b.path.clone(), b)).collect()
}

fn parse_line(line: &str) -> Option<BookEntry> {
    let f: Vec<&str> = line.split('\t').collect();
    let [path, size, mtime, title, authors, language, series, index, thumb] = f.as_slice() else { return None };
    let opt = |s: &str| (!s.is_empty()).then(|| unescape(s));
    let path = PathBuf::from(unescape(path));
    Some(BookEntry {
        format: Format::from_path(&path)?,
        path,
        title: unescape(title),
        authors: if authors.is_empty() { Vec::new() } else { authors.split('\x1f').map(unescape).collect() },
        language: opt(language),
        series: opt(series),
        series_index: if index.is_empty() { None } else { Some(index.parse().ok()?) },
        size: size.parse().ok()?,
        modified: SystemTime::UNIX_EPOCH + Duration::from_nanos(mtime.parse().ok()?),
        cover_thumb: opt(thumb).map(PathBuf::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    /// A fresh, empty temp directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let nanos = nanos(SystemTime::now());
            let dir = std::env::temp_dir().join(format!("omero-{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&dir).expect("temp dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn cover_png() -> Vec<u8> {
        let (w, h) = (200usize, 300usize);
        let data: Vec<u8> = (0..w * h).map(|i| (i % w) as u8).collect();
        let opts = zune_core::options::EncoderOptions::new(
            w,
            h,
            zune_core::colorspace::ColorSpace::Luma,
            zune_core::bit_depth::BitDepth::Eight,
        );
        let mut out = Vec::new();
        zune_png::PngEncoder::new(&data, opts).encode(&mut out).expect("png");
        out
    }

    fn write_epub(path: &Path, title: &str, author: &str, series: Option<(&str, u32)>) {
        let series = series.map_or(String::new(), |(s, i)| {
            format!(r#"<meta name="calibre:series" content="{s}"/><meta name="calibre:series_index" content="{i}"/>"#)
        });
        let opf = format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
            <dc:title>{title}</dc:title><dc:creator>{author}</dc:creator><dc:language>it</dc:language>{series}
            <meta name="cover" content="cov"/></metadata>
            <manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
            <item id="cov" href="cover.png" media-type="image/png"/></manifest>
            <spine><itemref idref="c1"/></spine></package>"#
        );
        let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles>
            <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in [
            ("META-INF/container.xml", container.as_bytes().to_vec()),
            ("content.opf", opf.into_bytes()),
            ("c1.xhtml", b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>x</p></body></html>".to_vec()),
            ("cover.png", cover_png()),
        ] {
            zip.start_file(name, opts).expect("entry");
            zip.write_all(&bytes).expect("write");
        }
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, zip.finish().expect("finish").into_inner()).expect("save epub");
    }

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(path, b"x").expect("touch");
    }

    #[test]
    fn formats_by_extension() {
        let f = |p: &str| Format::from_path(Path::new(p));
        assert_eq!(f("a/B.EPUB"), Some(Format::Epub));
        assert_eq!(f("b.kepub.epub"), Some(Format::Kepub));
        assert_eq!(f("c.azw3"), Some(Format::Azw3));
        assert_eq!(f("d.mobi"), Some(Format::Mobi));
        assert_eq!(f("e.pdf"), Some(Format::Pdf));
        assert_eq!(f("f.txt"), Some(Format::Txt));
        assert_eq!(f("g.jpg"), None);
        assert_eq!(f("epub"), None);
    }

    #[test]
    fn scans_reads_metadata_and_caches() {
        let root = TempDir::new("lib");
        let docs = root.0.join("documents");
        let cache = root.0.join("cache");
        write_epub(&docs.join("Calvino/Le città invisibili.epub"), "Le città invisibili", "Italo Calvino", None);
        write_epub(&docs.join("Eco/Rosa.epub"), "Il nome della rosa", "Eco, Umberto", Some(("Gialli", 2)));
        touch(&docs.join("Eco/Baudolino.azw3"));
        touch(&docs.join("notes.txt"));
        touch(&docs.join(".hidden/secret.epub"));
        touch(&docs.join("Eco/Rosa.sdr/metadata.epub.lua"));
        touch(&docs.join("Eco/Rosa.sdr/fake.pdf"));
        touch(&docs.join("cover.jpg"));

        let lib = Library::scan(&[&docs], &cache);
        let names: Vec<String> = lib.books.iter().map(|b| b.title.clone()).collect();
        assert_eq!(names, ["Le città invisibili", "Baudolino", "Il nome della rosa", "notes"]);
        let rosa = &lib.books[2];
        assert_eq!(rosa.authors, ["Eco, Umberto"]);
        assert_eq!((rosa.series.as_deref(), rosa.series_index), (Some("Gialli"), Some(2.0)));
        assert_eq!(rosa.language.as_deref(), Some("it"));
        let thumb = rosa.cover_thumb.as_ref().expect("thumb");
        let img = image::decode(&fs::read(thumb).expect("read thumb"), THUMB_W, THUMB_H).expect("png");
        assert_eq!((img.width, img.height), (THUMB_W, THUMB_H));
        assert!(img.pixels.iter().all(|p| p % 17 == 0), "quantized to 16 levels");

        // Same size and mtime: the cache answers without reopening the book. Prove it by
        // corrupting the file in place and restoring its mtime.
        let path = rosa.path.clone();
        let mtime = fs::metadata(&path).and_then(|m| m.modified()).expect("mtime");
        let len = fs::metadata(&path).expect("meta").len() as usize;
        fs::write(&path, vec![0u8; len]).expect("corrupt");
        fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(mtime)).expect("set mtime");
        let again = Library::scan(&[&docs], &cache);
        assert_eq!(again, lib);

        // A changed file is re-read (now unreadable, so it falls back to the stem) and its old
        // thumbnail is cleaned up.
        fs::write(&path, b"garbage").expect("rewrite");
        let changed = Library::scan(&[&docs], &cache);
        let rosa = &changed.books[2];
        assert_eq!((rosa.title.as_str(), rosa.cover_thumb.as_ref()), ("Rosa", None));
        assert!(!thumb.exists());
        assert!(changed.books[0].cover_thumb.as_ref().is_some_and(|t| t.is_file()));
    }

    #[test]
    fn failed_thumbnail_write_is_retried() {
        let root = TempDir::new("thumbfail");
        let docs = root.0.join("documents");
        let cache = root.0.join("cache");
        write_epub(&docs.join("a.epub"), "A", "Anon", None);
        // A file where the thumbnail folder should be: every thumbnail write fails.
        touch(&cache.join(THUMB_DIR));
        let lib = Library::scan(&[&docs], &cache);
        assert_eq!(lib.books[0].cover_thumb, None);
        assert!(load_cache(&cache).is_empty(), "not cached as coverless");
        fs::remove_file(cache.join(THUMB_DIR)).expect("unblock");
        let lib = Library::scan(&[&docs], &cache);
        assert!(lib.books[0].cover_thumb.as_ref().is_some_and(|t| t.is_file()));
    }

    #[test]
    fn cache_line_roundtrip_with_awkward_text() {
        let b = BookEntry {
            path: PathBuf::from("/mnt/us/documents/a\tb/c.epub"),
            title: "Line\nbreak \\ tab\t".into(),
            authors: vec!["A\x1fB".into(), "Ç".into()],
            language: None,
            series: Some("S".into()),
            series_index: Some(1.5),
            format: Format::Epub,
            size: 42,
            modified: SystemTime::UNIX_EPOCH + Duration::from_nanos(1_700_000_000_123_456_789),
            cover_thumb: None,
        };
        let root = TempDir::new("cache");
        save_cache(&root.0, &[&b]).expect("save");
        let loaded = load_cache(&root.0);
        assert_eq!(loaded.get(&b.path), Some(&b));
        assert!(parse_line("too\tfew").is_none());
    }

    fn entry(title: &str, authors: &[&str], series: Option<(&str, f32)>, secs: u64, folder: &str) -> BookEntry {
        BookEntry {
            path: PathBuf::from(format!("/docs/{folder}/{title}.epub")),
            title: title.into(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
            language: Some(if title.starts_with('T') { "en" } else { "it" }.into()),
            series: series.map(|s| s.0.into()),
            series_index: series.map(|s| s.1),
            format: Format::Epub,
            size: 1,
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
            cover_thumb: None,
        }
    }

    fn sample() -> Library {
        Library {
            books: vec![
                entry("Il pendolo di Foucault", &["Umberto Eco"], None, 30, "Eco"),
                entry("Èrcole", &["Eco, Umberto"], Some(("Miti", 2.0)), 10, "Eco"),
                entry("The Hobbit", &["J. R. R. Tolkien"], None, 20, "Tolkien"),
                entry("anonimo", &[], Some(("Miti", 1.0)), 40, "Misc"),
                entry("Divina Commedia", &["Dante Alighieri", "Gustave Doré"], None, 5, "Misc"),
            ],
        }
    }

    #[test]
    fn author_keys_from_either_spelling() {
        assert_eq!(author_sort_key("Umberto Eco"), "Eco, Umberto");
        assert_eq!(author_sort_key("Eco, Umberto"), "Eco, Umberto");
        assert_eq!(author_sort_key("J. R. R. Tolkien"), "Tolkien, J. R. R.");
        assert_eq!(author_sort_key("Omero"), "Omero");
    }

    #[test]
    fn sorts() {
        let lib = sample();
        assert_eq!(lib.sorted(SortBy::Title), [3, 4, 1, 0, 2]); // "Èrcole" sorts as "ercole"
        assert_eq!(lib.sorted(SortBy::Author), [4, 1, 0, 2, 3]); // Alighieri, Eco×2, Tolkien, none
        assert_eq!(lib.sorted(SortBy::Recent), [3, 0, 2, 1, 4]);
    }

    #[test]
    fn groups() {
        let lib = sample();
        let authors = lib.group_by_author();
        let labels: Vec<&str> = authors.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Alighieri, Dante", "Doré, Gustave", "Eco, Umberto", "Tolkien, J. R. R.", ""]);
        assert_eq!(authors[2].1, [1, 0]);
        let series = lib.group_by_series();
        assert_eq!(series[0], ("Miti".to_string(), vec![3, 1]));
        assert_eq!(series[1].0, "");
        let langs = lib.group_by_language();
        assert_eq!(langs.iter().map(|(l, m)| (l.as_str(), m.len())).collect::<Vec<_>>(), [("en", 1), ("it", 4)]);
        let folders = lib.group_by_folder();
        assert_eq!(folders[0], ("/docs/Eco".to_string(), vec![1, 0]));
    }

    #[test]
    fn search_folds_case_and_accents() {
        let lib = sample();
        assert_eq!(lib.search("ercole"), [1]);
        assert_eq!(lib.search("ECO"), [1, 0]);
        assert_eq!(lib.search("dore commedia"), [4]);
        assert_eq!(lib.search("miti"), [3, 1]);
        assert_eq!(lib.search("  ").len(), 5);
        assert!(lib.search("xyz").is_empty());
        assert_eq!(fold("Straße Æsir Œuvre Łódź"), "strasse aesir oeuvre lodz");
        assert_eq!(fold("e\u{301}"), "e");
    }

    /// Scans the local `dev-books/` (not in the repo) twice: the second pass must hit the cache.
    #[test]
    fn dev_books_scan() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../dev-books"));
        if !dir.is_dir() {
            return;
        }
        let cache = TempDir::new("devlib");
        let t = std::time::Instant::now();
        let first = Library::scan(&[dir], &cache.0);
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let second = Library::scan(&[dir], &cache.0);
        let warm = t.elapsed();
        eprintln!("{} books: cold {cold:?}, warm {warm:?}", first.books.len());
        for b in &first.books {
            eprintln!("  {:?} by {:?} [{:?}] thumb {:?}", b.title, b.authors, b.format, b.cover_thumb.is_some());
        }
        assert_eq!(first, second);
        assert!(first.books.iter().filter(|b| b.format == Format::Epub).all(|b| b.cover_thumb.is_some()));
    }
}
