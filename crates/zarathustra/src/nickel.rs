//! Once, on a Kobo: what Nickel (the Kobo's own reader) knows about the books that are also in
//! Zarathustra's library — reading state and place, highlights, notes, dog-ears, reading time —
//! taken from its database into ours. Books are matched by file name (the library holds copies
//! of the books Nickel read in the storage's root).

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use zarathustra_engine::anchor::{cfi_for, reanchor, Anchor, TextMap, TextQuote};
use zarathustra_engine::book::Book;
use zarathustra_engine::dom::{Dom, NodeId};
use zarathustra_store::{AnnotationData, Color, Kind, Store};

use crate::reader::Position;

/// Nickel's database on a Kobo.
pub const DATABASE: &str = "/mnt/onboard/.kobo/KoboReader.sqlite";
/// The preference that records the import, so it runs once.
const DONE: &str = "nickel-import";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub books: u32,
    pub positions: u32,
    pub annotations: u32,
    /// Highlights whose text wasn't found in our copy of the book.
    pub lost: u32,
    pub seconds: u64,
}

/// Imports from `nickel` into `store` unless done before; `None` when there was nothing to do.
pub fn import_once(nickel: &Path, store: &Store, library: &Path) -> Option<Report> {
    if !nickel.exists() || store.pref(DONE).ok().flatten().is_some() {
        return None;
    }
    let report = import(nickel, store, library).map_err(|e| eprintln!("nickel import: {e:#}")).ok()?;
    store.set_pref(DONE, &format!("{report:?}")).map_err(|e| eprintln!("nickel import: {e}")).ok();
    Some(report)
}

struct Volume {
    id: String,
    status: i64,
    percent: f64,
    chapter: Option<String>,
    last_read: Option<String>,
    seconds: i64,
}

struct Mark {
    chapter: String,
    span: Option<String>,
    text: String,
    note: String,
    kind: String,
    created: Option<String>,
}

pub fn import(nickel: &Path, store: &Store, library: &Path) -> Result<Report> {
    let db = Connection::open_with_flags(nickel, OpenFlags::SQLITE_OPEN_READ_ONLY).context("Nickel database")?;
    let volumes: Vec<Volume> = db
        .prepare(
            "SELECT ContentID, ReadStatus, ___PercentRead, ChapterIDBookmarked, DateLastRead, TimeSpentReading
             FROM content WHERE ContentType = 6 AND ContentID LIKE 'file:///mnt/onboard/%'",
        )?
        .query_map([], |r| {
            Ok(Volume {
                id: r.get(0)?,
                status: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                percent: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                chapter: r.get(3)?,
                last_read: r.get(4)?,
                seconds: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut marks_of = db.prepare(
        "SELECT ContentID, StartContainerPath, Text, Annotation, Type, DateCreated FROM Bookmark
         WHERE VolumeID = ?1 AND (Hidden IS NULL OR Hidden != 'true')",
    )?;
    let mut report = Report::default();
    for v in volumes {
        let Some(name) = v.id.rsplit('/').next() else { continue };
        let path = library.join(name);
        if !path.exists() {
            continue;
        }
        let marks: Vec<Mark> = marks_of
            .query_map(params![v.id], |r| {
                let content: String = r.get(0)?;
                Ok(Mark {
                    chapter: chapter_path(&content),
                    span: r.get::<_, Option<String>>(1)?.and_then(|p| span_id(&p)),
                    text: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    note: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    kind: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    created: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let reading = v.status > 0 || v.percent > 0.0;
        if !reading && marks.is_empty() && v.seconds == 0 {
            continue;
        }
        match import_book(&v, &marks, &path, store, &mut report) {
            Ok(()) => report.books += 1,
            Err(e) => eprintln!("nickel import: {}: {e:#}", path.display()),
        }
    }
    Ok(report)
}

fn import_book(v: &Volume, marks: &[Mark], path: &Path, store: &Store, report: &mut Report) -> Result<()> {
    let book = Book::open(path)?;
    let key = path.to_string_lossy();
    let finished = v.status == 2 || v.percent >= 100.0;
    if (finished || v.status == 1 || v.percent > 0.0) && store.position(&key)?.is_none() {
        let last = book.chapters.len().saturating_sub(1);
        let chapter =
            if finished { last } else { v.chapter.as_deref().and_then(|c| chapter_index(&book, c)).unwrap_or(0) };
        let progress = if finished { 1.0 } else { (v.percent / 100.0) as f32 };
        // shortcut: the chapter's start, not the exact page (Nickel's span would need the
        // chapter laid out); fine for picking a book up again.
        let pos = Position { chapter, block: 0, progress, offset: 0 };
        store.set_position(&key, &pos.to_text(), v.last_read.as_deref().and_then(unix_time).unwrap_or(0))?;
        report.positions += 1;
    }
    let (book_key, entry) = crate::highlight::book_record(path, &book);
    let id = store.ensure_book(&book_key, &entry)?;
    if v.seconds > 0 {
        let day = v.last_read.as_deref().and_then(unix_time).unwrap_or(0).div_euclid(86_400);
        store.add_reading(id, day, u32::try_from(v.seconds).unwrap_or(u32::MAX), 0)?;
        report.seconds += v.seconds as u64;
    }
    for m in marks {
        let Some(i) = chapter_index(&book, &m.chapter) else {
            report.lost += 1;
            continue;
        };
        let dom = book.chapter_dom(i)?;
        let spine = u32::try_from(book.spine.get(i).copied().unwrap_or(i)).unwrap_or(u32::MAX);
        match annotation(&dom, spine, m) {
            Some(data) => {
                let at = m.created.as_deref().and_then(unix_time).unwrap_or(0);
                store.add_annotation_at(id, &data, at)?;
                report.annotations += 1;
            }
            None => report.lost += 1,
        }
    }
    Ok(())
}

/// The chapter whose archive path is `href` (Nickel's paths may lack the folder part, and
/// may carry a `#fragment`).
fn chapter_index(book: &Book, href: &str) -> Option<usize> {
    let href = href.split('#').next()?.trim_start_matches('/');
    if href.is_empty() {
        return None;
    }
    let same = |c: &str| c == href || c.ends_with(&format!("/{href}")) || href.ends_with(&format!("/{c}"));
    book.chapters.iter().position(|c| same(c))
}

/// The chapter in Nickel's content id `<book>!<OPF folder>!<path>`: `OEBPS/Text/c01.html` from
/// `/mnt/onboard/x.kepub.epub!OEBPS!Text/c01.html`, `c01.html` from `…epub!!c01.html`.
fn chapter_path(content: &str) -> String {
    let rest = content.find(".epub!").map_or("", |i| &content[i + ".epub".len()..]);
    rest.replace('!', "/").trim_start_matches('/').to_string()
}

/// `span#kobo\.109\.1` → `kobo.109.1`.
fn span_id(path: &str) -> Option<String> {
    let id = path.strip_prefix("span#")?.replace('\\', "");
    (!id.is_empty()).then_some(id)
}

fn by_id(dom: &Dom, id: &str) -> Option<NodeId> {
    dom.descendants(Dom::ROOT).find(|&n| dom.attr(n, "id") == Some(id))
}

/// Nickel's mark as one of ours: a dog-ear becomes a bookmark at its span, a highlight (with
/// its note, if any) is found again by its text, from its span on.
fn annotation(dom: &Dom, spine: u32, m: &Mark) -> Option<AnnotationData> {
    let map = TextMap::new(dom);
    let text = map.text();
    let at_span = m.span.as_deref().and_then(|id| by_id(dom, id)).and_then(|n| map.to_text(n, 0));
    if m.kind == "dogear" {
        let start = at_span.unwrap_or(0);
        let end = text[start..].char_indices().nth(60).map_or(text.len(), |(i, _)| start + i);
        let (node, offset) = map.to_dom(start)?;
        return Some(AnnotationData {
            kind: Kind::Bookmark,
            start: Anchor::new(dom, spine, node, offset),
            end_cfi: None,
            quote: Some(TextQuote::new(text, start..end)),
            chapter: None,
            page_hint: None,
            note: None,
            color: Color::Gray,
        });
    }
    let exact = m.text.trim();
    if exact.is_empty() {
        return None;
    }
    // From a little before the span: Nickel's span is where the selection began.
    let from = at_span.map_or(0, |s| {
        let mut b = s.saturating_sub(64);
        while !text.is_char_boundary(b) {
            b -= 1;
        }
        b
    });
    let range = locate(text, from, exact).or_else(|| locate(text, 0, exact))?;
    let (start_node, start_off) = map.to_dom(range.start)?;
    let (end_node, end_off) = map.to_dom_end(range.end)?;
    let note = m.note.trim();
    Some(AnnotationData {
        kind: Kind::Highlight,
        start: Anchor::new(dom, spine, start_node, start_off),
        end_cfi: Some(cfi_for(dom, spine, end_node, end_off)),
        quote: Some(TextQuote::new(text, range)),
        chapter: None,
        page_hint: None,
        note: (!note.is_empty()).then(|| note.to_string()),
        color: Color::Gray,
    })
}

/// Where `exact` lies in `text`, searching from byte `from`: the whole quote, or else its
/// first and last words (Nickel joins the paragraphs of a long highlight its own way).
fn locate(text: &str, from: usize, exact: &str) -> Option<std::ops::Range<usize>> {
    let find = |at: usize, s: &str| {
        let q = TextQuote { exact: s.to_string(), prefix: String::new(), suffix: String::new() };
        reanchor(&text[at..], &q).map(|r| r.start + at..r.end + at)
    };
    if let Some(r) = find(from, exact) {
        return Some(r);
    }
    const ENDS: usize = 40;
    let chars: Vec<(usize, char)> = exact.char_indices().collect();
    if chars.len() < 3 * ENDS {
        return None;
    }
    let head = &exact[..chars[ENDS].0];
    let tail = &exact[chars[chars.len() - ENDS].0..];
    let start = find(from, head)?;
    let end = find(start.end, tail)?;
    // The same passage, not two far-apart matches.
    (end.end - start.start <= 2 * exact.len()).then_some(start.start..end.end)
}

/// Nickel's `2024-01-11T01:21:15.425` or `2026-09-16T07:57:40Z` (read as UTC) in Unix seconds.
fn unix_time(s: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, min, sec) = (n(11..13).unwrap_or(0), n(14..16).unwrap_or(0), n(17..19).unwrap_or(0));
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + sec)
}

/// On a Kobo, the first time: the import on its own thread with its own connection (about a
/// second per 40 annotated books on a desktop, many more on the device), so the home shows at
/// once; it picks the positions up at its next look.
pub fn import_in_background(state_dir: &Path) {
    use zarathustra_device::{platform, Kind};
    if platform().kind != Kind::Kobo {
        return;
    }
    let (db, library) = (crate::db_path(state_dir), platform().library.clone());
    std::thread::spawn(move || {
        let store = match Store::open(&db) {
            Ok(s) => s,
            Err(e) => return eprintln!("nickel import: {e}"),
        };
        let t = std::time::Instant::now();
        if let Some(report) = import_once(Path::new(DATABASE), &store, &library) {
            eprintln!("nickel import: {report:?} in {:?}", t.elapsed());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nickel_dates() {
        assert_eq!(unix_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_time("2024-01-11T01:21:15.425"), Some(1_704_936_075));
        assert_eq!(unix_time("2026-09-16T07:57:40Z"), Some(1_789_545_460));
        assert_eq!(unix_time("garbage"), None);
    }

    #[test]
    fn reads_chapter_paths() {
        assert_eq!(chapter_path("/mnt/onboard/a b.kepub.epub!OEBPS!Text/c01.html"), "OEBPS/Text/c01.html");
        assert_eq!(chapter_path("/mnt/onboard/x.kepub.epub!!index_split_010.html"), "index_split_010.html");
        assert_eq!(chapter_path("/mnt/onboard/x.kepub.epub"), "");
    }

    #[test]
    fn reads_span_paths() {
        assert_eq!(span_id(r"span#kobo\.109\.1").as_deref(), Some("kobo.109.1"));
        assert_eq!(span_id("p#x"), None);
    }

    #[test]
    fn imports_a_nickel_database_once() {
        let Some(epub) = std::fs::read_dir(crate::first_existing(&["dev-books"]))
            .ok()
            .and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "epub")))
        else {
            return; // dev books are local-only
        };
        let dir = tempfile::tempdir().expect("dir");
        let library = dir.path().join("library");
        std::fs::create_dir_all(&library).expect("mkdir");
        let copy = library.join("libro.kepub.epub");
        std::fs::copy(&epub, &copy).expect("copy");
        let book = Book::open(&copy).expect("open");
        // A highlight of real text from the book's second chapter (or first).
        let i = 1.min(book.chapters.len() - 1);
        let dom = book.chapter_dom(i).expect("dom");
        let text = TextMap::new(&dom).text().to_string();
        let words: String = text.split_whitespace().skip(20).take(6).collect::<Vec<_>>().join(" ");
        let nickel = dir.path().join("KoboReader.sqlite");
        let db = Connection::open(&nickel).expect("db");
        db.execute_batch(
            "CREATE TABLE content(ContentID TEXT, ContentType TEXT, ReadStatus INT, ___PercentRead INT,
               ChapterIDBookmarked TEXT, DateLastRead TEXT, TimeSpentReading INT);
             CREATE TABLE Bookmark(VolumeID TEXT, ContentID TEXT, StartContainerPath TEXT, Text TEXT,
               Annotation TEXT, Type TEXT, DateCreated TEXT, Hidden TEXT);",
        )
        .expect("schema");
        let volume = "file:///mnt/onboard/libro.kepub.epub";
        db.execute(
            "INSERT INTO content VALUES (?1, '6', 1, 40, ?2, '2026-09-16T07:57:40Z', 3600)",
            params![volume, format!("{}#kobo.1.1", book.chapters[i])],
        )
        .expect("content");
        db.execute(
            "INSERT INTO Bookmark VALUES (?1, ?2, NULL, ?3, 'una nota', 'note', '2024-01-11T01:21:15.425', 'false')",
            params![volume, format!("/mnt/onboard/libro.kepub.epub!!{}", book.chapters[i]), words],
        )
        .expect("bookmark");
        drop(db);
        let store = Store::open_in_memory().expect("store");
        let report = import_once(&nickel, &store, &library).expect("imported");
        assert_eq!(report, Report { books: 1, positions: 1, annotations: 1, lost: 0, seconds: 3600 });
        let (value, _) = store.position(&copy.to_string_lossy()).expect("read").expect("position");
        assert!(value.starts_with(&format!("{i} 0 0.4000")), "{value}");
        let id = store.book_by_path(&copy).expect("read").expect("book");
        let notes = store.annotations(id).expect("annotations");
        assert_eq!(notes[0].data.note.as_deref(), Some("una nota"));
        assert_eq!(notes[0].created_at, 1_704_936_075);
        assert!(import_once(&nickel, &store, &library).is_none(), "only once");
    }

    /// A dry run on a real Kobo's data: `NICKEL_DB=… NICKEL_LIBRARY=… cargo test -p zarathustra
    /// --lib nickel::tests::dry_run -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dry_run() {
        let (Ok(db), Ok(library)) = (std::env::var("NICKEL_DB"), std::env::var("NICKEL_LIBRARY")) else { return };
        let store = Store::open_in_memory().expect("store");
        let t = std::time::Instant::now();
        let report = import(Path::new(&db), &store, Path::new(&library)).expect("import");
        eprintln!("{report:?} in {:?}", t.elapsed());
    }
}
