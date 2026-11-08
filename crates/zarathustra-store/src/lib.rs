//! Zarathustra's database: books and annotations (highlights, underlines, notes, bookmarks) in
//! SQLite, with a change feed (`events`) for scripts and future sync.
//!
//! Every write to `books` and `annotations` appends to `events` from a trigger, so the
//! event is part of the same statement (and transaction) as the change, whoever writes it.
//! Deleting an annotation leaves a tombstone (`deleted_at`) so a sync can propagate it.

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};
use zarathustra_engine::anchor::{Anchor, TextQuote};
use zarathustra_engine::library::{BookEntry, Format};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema v{0} is newer than this build (v{LATEST})")]
    NewerSchema(u32),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Schema migrations; entry `i` takes the database from `user_version` `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[include_str!("v1.sql"), include_str!("v2.sql")];
const LATEST: u32 = MIGRATIONS.len() as u32;

/// Stable identity of a book across moves, renames and re-downloads: its EPUB
/// `dc:identifier` when it has one, else a hash of `fallback` (e.g. the OPF bytes).
// ponytail: FNV-1a 64 (collision odds ~1e-11 at 10k books); a cryptographic hash is only
// needed if keys ever become an untrusted sync identity.
pub fn book_key(identifier: Option<&str>, fallback: &[u8]) -> String {
    match identifier.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => format!("id:{id}"),
        None => {
            let hash = fallback
                .iter()
                .fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
            format!("fnv:{hash:016x}")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BookId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Highlight,
    Underline,
    Bookmark,
    Note,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Highlight => "highlight",
            Kind::Underline => "underline",
            Kind::Bookmark => "bookmark",
            Kind::Note => "note",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        [Kind::Highlight, Kind::Underline, Kind::Bookmark, Kind::Note].into_iter().find(|k| k.as_str() == s)
    }
}

/// Highlight colour: gray levels on e-ink, real colours on colour panels and in exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Gray,
    Yellow,
    Green,
    Blue,
    Pink,
}

impl Color {
    const ALL: [Color; 5] = [Color::Gray, Color::Yellow, Color::Green, Color::Blue, Color::Pink];

    fn from_db(n: i64) -> Color {
        usize::try_from(n).ok().and_then(|i| Color::ALL.get(i).copied()).unwrap_or_default()
    }
}

/// The user-editable part of an annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationData {
    pub kind: Kind,
    pub start: Anchor,
    /// End of the range (`None` for bookmarks).
    pub end_cfi: Option<String>,
    /// Selected text and context, for display and re-anchoring (`None` for bookmarks).
    pub quote: Option<TextQuote>,
    /// Chapter title, for grouping in lists and exports.
    pub chapter: Option<String>,
    pub page_hint: Option<u32>,
    pub note: Option<String>,
    pub color: Color,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// UUIDv7: unique across devices and sortable by creation time.
    pub id: String,
    pub book: BookId,
    pub data: AnnotationData,
    /// Unix seconds.
    pub created_at: i64,
    pub updated_at: i64,
}

/// One row of the change feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub seq: i64,
    pub at: i64,
    /// `book` or `annotation`.
    pub entity: String,
    pub entity_id: String,
    /// `insert`, `update` or `delete`.
    pub op: String,
}

/// Time and pages read on one local day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// Days since 1970-01-01, local time.
    pub day: i64,
    pub seconds: i64,
    pub pages: i64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it to the latest schema.
    pub fn open(path: &Path) -> Result<Store> {
        Store::init(Connection::open(path)?)
    }

    /// A private in-memory database, for tests and previews.
    pub fn open_in_memory() -> Result<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store> {
        // WAL: readers (scripts, a future sync) never block the UI's writes. NORMAL sync is
        // durable across app crashes and only loses the last commits on power loss.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    /// The preference group `name` as saved, `None` when it never was.
    pub fn pref(&self, name: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM prefs WHERE name = ?1", [name], |r| r.get(0)).optional()?)
    }

    /// Saves preference group `name`.
    pub fn set_pref(&self, name: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO prefs(name, value, updated_at) VALUES (?1, ?2, unixepoch())
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (name, value),
        )?;
        Ok(())
    }

    /// Inserts the book identified by `key` (see [`book_key`]) or refreshes its path and
    /// metadata; returns its id. Unchanged metadata writes nothing (and logs no event).
    pub fn ensure_book(&self, key: &str, entry: &BookEntry) -> Result<BookId> {
        let authors = entry.authors.join(" & ");
        self.conn.execute(
            "INSERT INTO books(hash, path, title, authors, series, series_index, language, format, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(hash) DO UPDATE SET path = excluded.path, title = excluded.title,
               authors = excluded.authors, series = excluded.series, series_index = excluded.series_index,
               language = excluded.language, format = excluded.format
             WHERE (path, title, authors, series, series_index, language, format)
               IS NOT (excluded.path, excluded.title, excluded.authors, excluded.series,
                       excluded.series_index, excluded.language, excluded.format)",
            params![
                key,
                entry.path.to_string_lossy(),
                entry.title,
                authors,
                entry.series,
                entry.series_index,
                entry.language,
                format_name(entry.format),
                now(),
            ],
        )?;
        Ok(BookId(self.conn.query_row("SELECT id FROM books WHERE hash = ?1", [key], |r| r.get(0))?))
    }

    /// The book registered at `path` (by the reader, when it was opened), if any.
    pub fn book_by_path(&self, path: &Path) -> Result<Option<BookId>> {
        let id = self.conn.query_row(
            "SELECT id FROM books WHERE path = ?1 ORDER BY id DESC LIMIT 1",
            [path.to_string_lossy()],
            |r| r.get(0),
        );
        Ok(id.optional()?.map(BookId))
    }

    pub fn add_annotation(&self, book: BookId, data: &AnnotationData) -> Result<Annotation> {
        let id = uuid::Uuid::now_v7().to_string();
        let at = now();
        let q = data.quote.as_ref();
        self.conn.execute(
            "INSERT INTO annotations(id, book_id, kind, spine, pos, start_cfi, end_cfi, exact, prefix, suffix,
               chapter, page_hint, note, color, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
            params![
                id,
                book.0,
                data.kind.as_str(),
                data.start.spine,
                data.start.pos,
                data.start.cfi,
                data.end_cfi,
                q.map(|q| &q.exact),
                q.map(|q| &q.prefix),
                q.map(|q| &q.suffix),
                data.chapter,
                data.page_hint,
                data.note,
                data.color as i64,
                at,
            ],
        )?;
        Ok(Annotation { id, book, data: data.clone(), created_at: at, updated_at: at })
    }

    /// Replaces the editable fields of annotation `id`. Returns false if it does not exist
    /// (or was deleted).
    pub fn update_annotation(&self, id: &str, data: &AnnotationData) -> Result<bool> {
        let q = data.quote.as_ref();
        let n = self.conn.execute(
            "UPDATE annotations SET kind = ?2, spine = ?3, pos = ?4, start_cfi = ?5, end_cfi = ?6,
               exact = ?7, prefix = ?8, suffix = ?9, chapter = ?10, page_hint = ?11, note = ?12,
               color = ?13, updated_at = ?14
             WHERE id = ?1 AND deleted_at IS NULL",
            params![
                id,
                data.kind.as_str(),
                data.start.spine,
                data.start.pos,
                data.start.cfi,
                data.end_cfi,
                q.map(|q| &q.exact),
                q.map(|q| &q.prefix),
                q.map(|q| &q.suffix),
                data.chapter,
                data.page_hint,
                data.note,
                data.color as i64,
                now(),
            ],
        )?;
        Ok(n > 0)
    }

    /// Deletes annotation `id`, leaving a tombstone. Returns false if it was already gone.
    pub fn delete_annotation(&self, id: &str) -> Result<bool> {
        let at = now();
        let n = self.conn.execute(
            "UPDATE annotations SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
            params![id, at],
        )?;
        Ok(n > 0)
    }

    /// A book's annotations (all kinds) in reading order.
    pub fn annotations(&self, book: BookId) -> Result<Vec<Annotation>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLUMNS} FROM annotations WHERE book_id = ?1 AND deleted_at IS NULL ORDER BY spine, pos, id"
        ))?;
        let rows = stmt.query_map([book.0], annotation)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The newest highlights, underlines and notes across all books, with the book's title.
    pub fn recent_annotations(&self, limit: u32) -> Result<Vec<(Annotation, Option<String>)>> {
        let cols = COLUMNS.split(", ").map(|c| format!("a.{c}")).collect::<Vec<_>>().join(", ");
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {cols}, b.title FROM annotations a JOIN books b ON b.id = a.book_id
             WHERE a.deleted_at IS NULL AND a.kind != 'bookmark'
             ORDER BY a.created_at DESC, a.id DESC LIMIT ?1"
        ))?;
        let rows = stmt.query_map([limit], |r| Ok((annotation(r)?, r.get(COLUMN_COUNT)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Number of highlights, underlines and notes per book (books without any are absent).
    pub fn annotation_counts(&self) -> Result<HashMap<BookId, u32>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT book_id, COUNT(*) FROM annotations
             WHERE deleted_at IS NULL AND kind != 'bookmark' GROUP BY book_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((BookId(r.get(0)?), r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A book's highlights, underlines and notes as Markdown, grouped by chapter.
    pub fn export_markdown(&self, book: BookId) -> Result<String> {
        let (title, authors): (Option<String>, Option<String>) =
            self.conn.query_row("SELECT title, authors FROM books WHERE id = ?1", [book.0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let mut md = format!("# {}\n", title.as_deref().unwrap_or("Untitled"));
        if let Some(authors) = authors.filter(|a| !a.is_empty()) {
            md.push_str(&format!("\n*{authors}*\n"));
        }
        let mut chapter = None;
        for a in self.annotations(book)?.into_iter().filter(|a| a.data.kind != Kind::Bookmark) {
            if a.data.chapter.is_some() && a.data.chapter != chapter {
                chapter = a.data.chapter.clone();
                md.push_str(&format!("\n## {}\n", chapter.as_deref().unwrap_or_default()));
            }
            if let Some(q) = a.data.quote.as_ref().filter(|q| !q.exact.trim().is_empty()) {
                md.push_str(&format!("\n> {}\n", q.exact.trim()));
            }
            if let Some(note) = a.data.note.as_deref().filter(|n| !n.trim().is_empty()) {
                md.push_str(&format!("\n{}\n", note.trim()));
            }
        }
        Ok(md)
    }

    /// Adds `seconds` of reading and `pages` turned to `book` on local day `day` (days since
    /// 1970-01-01).
    pub fn add_reading(&self, book: BookId, day: i64, seconds: u32, pages: u32) -> Result<()> {
        // Cached: it runs on every page turn.
        self.conn
            .prepare_cached(
                "INSERT INTO reading_stats(book_id, day, seconds, pages, words) VALUES (?1, ?2, ?3, ?4, 0)
                 ON CONFLICT(book_id, day) DO UPDATE SET seconds = seconds + excluded.seconds,
                   pages = pages + excluded.pages",
            )?
            .execute(params![book.0, day, seconds, pages])?;
        Ok(())
    }

    /// Reading per day, oldest first, for one book or all of them.
    pub fn reading_days(&self, book: Option<BookId>) -> Result<Vec<Reading>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT day, SUM(seconds), SUM(pages) FROM reading_stats
             WHERE ?1 IS NULL OR book_id = ?1 GROUP BY day ORDER BY day",
        )?;
        let rows = stmt
            .query_map([book.map(|b| b.0)], |r| Ok(Reading { day: r.get(0)?, seconds: r.get(1)?, pages: r.get(2)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Change-feed entries after `seq` (0 for all), oldest first.
    pub fn events_since(&self, seq: i64) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT seq, at, entity, entity_id, op FROM events WHERE seq > ?1 ORDER BY seq")?;
        let rows = stmt.query_map([seq], |r| {
            Ok(Event { seq: r.get(0)?, at: r.get(1)?, entity: r.get(2)?, entity_id: r.get(3)?, op: r.get(4)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > LATEST {
        return Err(Error::NewerSchema(version));
    }
    for (sql, next) in MIGRATIONS.iter().zip(1..).skip(version as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", next)?;
        tx.commit()?;
    }
    Ok(())
}

const COLUMNS: &str = "id, book_id, kind, spine, pos, start_cfi, end_cfi, exact, prefix, suffix, \
                       chapter, page_hint, note, color, created_at, updated_at";
const COLUMN_COUNT: usize = 16;

fn annotation(r: &Row) -> rusqlite::Result<Annotation> {
    let kind: String = r.get(2)?;
    let kind = Kind::parse(&kind).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, format!("kind {kind}").into())
    })?;
    let exact: Option<String> = r.get(7)?;
    let quote = exact.map(|exact| -> rusqlite::Result<_> {
        Ok(TextQuote {
            exact,
            prefix: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            suffix: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        })
    });
    Ok(Annotation {
        id: r.get(0)?,
        book: BookId(r.get(1)?),
        data: AnnotationData {
            kind,
            start: Anchor { cfi: r.get(5)?, spine: r.get(3)?, pos: r.get(4)? },
            end_cfi: r.get(6)?,
            quote: quote.transpose()?,
            chapter: r.get(10)?,
            page_hint: r.get(11)?,
            note: r.get(12)?,
            color: Color::from_db(r.get(13)?),
        },
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Epub => "epub",
        Format::Kepub => "kepub",
        Format::Azw3 => "azw3",
        Format::Mobi => "mobi",
        Format::Pdf => "pdf",
        Format::Txt => "txt",
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use zarathustra_engine::anchor::{cfi_for, reanchor, resolve_cfi, TextMap};
    use zarathustra_engine::dom::Dom;

    fn entry(title: &str) -> BookEntry {
        BookEntry {
            path: PathBuf::from(format!("/mnt/us/books/{title}.epub")),
            title: title.into(),
            authors: vec!["Dante Alighieri".into()],
            language: Some("it".into()),
            series: None,
            series_index: None,
            format: Format::Epub,
            size: 1,
            modified: UNIX_EPOCH,
            cover_thumb: None,
        }
    }

    fn highlight(spine: u32, pos: u32, exact: &str) -> AnnotationData {
        AnnotationData {
            kind: Kind::Highlight,
            start: Anchor { cfi: format!("epubcfi(/6/{}!/4/2/1:{pos})", 2 * (spine + 1)), spine, pos },
            end_cfi: Some(format!("epubcfi(/6/{}!/4/2/1:{})", 2 * (spine + 1), pos + 5)),
            quote: Some(TextQuote { exact: exact.into(), prefix: "before ".into(), suffix: " after".into() }),
            chapter: Some(format!("Canto {}", spine + 1)),
            page_hint: Some(pos / 100),
            note: None,
            color: Color::Yellow,
        }
    }

    fn events(store: &Store, after: i64) -> Vec<(String, String, String)> {
        store.events_since(after).expect("events").into_iter().map(|e| (e.entity, e.entity_id, e.op)).collect()
    }

    fn ev(entity: &str, id: &str, op: &str) -> (String, String, String) {
        (entity.into(), id.into(), op.into())
    }

    #[test]
    fn book_keys() {
        assert_eq!(book_key(Some(" urn:isbn:978 "), b"x"), "id:urn:isbn:978");
        assert_eq!(book_key(Some(""), b"opf"), book_key(None, b"opf"));
        assert_ne!(book_key(None, b"opf a"), book_key(None, b"opf b"));
        assert!(book_key(None, b"").starts_with("fnv:"));
    }

    #[test]
    fn migrates_from_empty_and_reopens() {
        let path = std::env::temp_dir().join(format!("zarathustra-store-{}-migrate.db", std::process::id()));
        let cleanup = || {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
        };
        cleanup();
        {
            let store = Store::open(&path).expect("open");
            let v: u32 = store.conn.pragma_query_value(None, "user_version", |r| r.get(0)).expect("version");
            assert_eq!(v, LATEST);
            let mode: String = store.conn.pragma_query_value(None, "journal_mode", |r| r.get(0)).expect("mode");
            assert_eq!(mode, "wal");
            let fk: bool = store.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0)).expect("fk");
            assert!(fk);
            store.ensure_book("id:a", &entry("Inferno")).expect("book");
        }
        let store = Store::open(&path).expect("reopen keeps data");
        assert_eq!(store.ensure_book("id:a", &entry("Inferno")).expect("book"), BookId(1));
        store.conn.pragma_update(None, "user_version", 99).expect("bump");
        drop(store);
        assert!(matches!(Store::open(&path), Err(Error::NewerSchema(99))));
        cleanup();
    }

    #[test]
    fn ensure_book_is_idempotent_and_quiet() {
        let store = Store::open_in_memory().expect("open");
        let a = store.ensure_book("id:a", &entry("Inferno")).expect("insert");
        assert_eq!(store.ensure_book("id:a", &entry("Inferno")).expect("same"), a);
        assert_eq!(events(&store, 0).len(), 1, "unchanged upsert logs nothing");
        let mut moved = entry("Inferno");
        moved.path = PathBuf::from("/mnt/us/elsewhere/inferno.epub");
        assert_eq!(store.ensure_book("id:a", &moved).expect("moved"), a);
        assert_eq!(store.book_by_path(&moved.path).expect("query"), Some(a));
        assert_eq!(store.book_by_path(&entry("Inferno").path).expect("query"), None, "follows the move");
        assert_eq!(events(&store, 0), [ev("book", "1", "insert"), ev("book", "1", "update")]);
        // Page turns do not hit the feed.
        store.conn.execute("UPDATE books SET progress = 0.5, position = 'x'", []).expect("progress");
        assert_eq!(events(&store, 0).len(), 2);
    }

    #[test]
    fn annotations_round_trip_in_reading_order() {
        let store = Store::open_in_memory().expect("open");
        let book = store.ensure_book("id:a", &entry("Inferno")).expect("book");
        let late = store.add_annotation(book, &highlight(2, 10, "selva oscura")).expect("add");
        let early = store.add_annotation(book, &highlight(0, 900, "nel mezzo")).expect("add");
        let mid = store.add_annotation(book, &highlight(0, 1200, "diritta via")).expect("add");
        let mut bookmark = highlight(1, 0, "");
        bookmark.kind = Kind::Bookmark;
        bookmark.quote = None;
        bookmark.end_cfi = None;
        let bm = store.add_annotation(book, &bookmark).expect("bookmark");

        let listed = store.annotations(book).expect("list");
        let ids: Vec<_> = listed.iter().map(|a| &a.id).collect();
        assert_eq!(ids, [&early.id, &mid.id, &bm.id, &late.id]);
        assert_eq!(listed[0], early, "every field survives the round trip");
        assert_eq!(listed[2].data.quote, None);

        let mut edit = mid.data.clone();
        edit.note = Some("Note: *important*".into());
        edit.color = Color::Blue;
        edit.kind = Kind::Underline;
        assert!(store.update_annotation(&mid.id, &edit).expect("update"));
        assert_eq!(store.annotations(book).expect("list")[1].data, edit);

        assert!(store.delete_annotation(&early.id).expect("delete"));
        assert!(!store.delete_annotation(&early.id).expect("already deleted"));
        assert!(!store.update_annotation(&early.id, &edit).expect("deleted is immutable"));
        assert_eq!(store.annotations(book).expect("list").len(), 3);

        let other = store.ensure_book("id:b", &entry("Purgatorio")).expect("book");
        let newest = store.add_annotation(other, &highlight(0, 1, "dolce color")).expect("add");
        let counts = store.annotation_counts().expect("counts");
        assert_eq!(counts, HashMap::from([(book, 2), (other, 1)]), "bookmarks and deleted not counted");

        let recent = store.recent_annotations(2).expect("recent");
        let recent: Vec<_> = recent.iter().map(|(a, t)| (a.id.as_str(), t.as_deref())).collect();
        assert_eq!(recent, [(newest.id.as_str(), Some("Purgatorio")), (mid.id.as_str(), Some("Inferno"))]);
    }

    #[test]
    fn reading_adds_up_per_day_and_book() {
        let store = Store::open_in_memory().expect("open");
        let a = store.ensure_book("id:a", &entry("Inferno")).expect("book");
        let b = store.ensure_book("id:b", &entry("Purgatorio")).expect("book");
        store.add_reading(a, 10, 30, 1).expect("add");
        store.add_reading(a, 10, 45, 1).expect("add");
        store.add_reading(b, 10, 20, 1).expect("add");
        store.add_reading(a, 12, 60, 2).expect("add");
        let day = |day, seconds, pages| Reading { day, seconds, pages };
        assert_eq!(store.reading_days(Some(a)).expect("a"), [day(10, 75, 2), day(12, 60, 2)]);
        assert_eq!(store.reading_days(None).expect("all"), [day(10, 95, 3), day(12, 60, 2)]);
    }

    #[test]
    fn every_mutation_logs_an_event_in_its_transaction() {
        let mut store = Store::open_in_memory().expect("open");
        let book = store.ensure_book("id:a", &entry("Inferno")).expect("book");
        let a = store.add_annotation(book, &highlight(0, 1, "x")).expect("add");
        store.update_annotation(&a.id, &highlight(0, 2, "y")).expect("update");
        store.delete_annotation(&a.id).expect("delete");
        let row = |op| ev("annotation", &a.id, op);
        assert_eq!(events(&store, 1), [row("insert"), row("update"), row("delete")]);
        let last = store.events_since(0).expect("events").last().map(|e| e.seq).expect("seq");

        // A failed write leaves no event behind.
        assert!(store.add_annotation(BookId(42), &highlight(0, 1, "x")).is_err(), "foreign key enforced");
        // A rolled-back write takes its event with it.
        let tx = store.conn.transaction().expect("tx");
        tx.execute(
            "INSERT INTO annotations(id, book_id, kind, spine, pos, start_cfi, created_at, updated_at)
             VALUES ('r', ?1, 'note', 0, 0, 'c', 0, 0)",
            [book.0],
        )
        .expect("insert");
        tx.rollback().expect("rollback");
        assert!(events(&store, last).is_empty());

        // Deleting the book cascades, without re-logging the tombstoned annotation.
        let b = store.add_annotation(book, &highlight(0, 3, "z")).expect("add");
        store.conn.execute("DELETE FROM books WHERE id = ?1", [book.0]).expect("delete book");
        let mut log = events(&store, last);
        log.sort();
        assert_eq!(
            log,
            [ev("annotation", &b.id, "delete"), ev("annotation", &b.id, "insert"), ev("book", "1", "delete")]
        );
    }

    #[test]
    fn exports_markdown_grouped_by_chapter() {
        let store = Store::open_in_memory().expect("open");
        let book = store.ensure_book("id:a", &entry("Inferno")).expect("book");
        let mut noted = highlight(0, 2, "che la diritta via era smarrita");
        noted.note = Some("Il tema del viaggio.".into());
        store.add_annotation(book, &noted).expect("add");
        store.add_annotation(book, &highlight(0, 1, "Nel mezzo del cammin")).expect("add");
        store.add_annotation(book, &highlight(1, 1, "Lasciate ogni speranza")).expect("add");
        let md = store.export_markdown(book).expect("export");
        assert_eq!(
            md,
            "# Inferno\n\n*Dante Alighieri*\n\n## Canto 1\n\n> Nel mezzo del cammin\n\n\
             > che la diritta via era smarrita\n\nIl tema del viaggio.\n\n## Canto 2\n\n> Lasciate ogni speranza\n"
        );
    }

    #[test]
    fn stores_engine_anchors_and_reanchors_them() {
        let src = "<html><body><h1>Canto I</h1><p>Nel mezzo del cammin di nostra vita</p></body></html>";
        let dom = Dom::parse_xml(src).expect("parses");
        let map = TextMap::new(&dom);
        let start = map.text().find("cammin").expect("in text");
        let end = start + "cammin".len();
        let (node, off) = map.to_dom(start).expect("start");
        let (end_node, end_off) = map.to_dom(end).expect("end");
        let data = AnnotationData {
            kind: Kind::Highlight,
            start: Anchor::new(&dom, 0, node, off),
            end_cfi: Some(cfi_for(&dom, 0, end_node, end_off)),
            quote: Some(TextQuote::new(map.text(), start..end)),
            chapter: Some("Canto I".into()),
            page_hint: None,
            note: None,
            color: Color::Gray,
        };
        let store = Store::open_in_memory().expect("open");
        let book = store.ensure_book("id:a", &entry("Inferno")).expect("book");
        store.add_annotation(book, &data).expect("add");
        let stored = store.annotations(book).expect("list").remove(0).data;
        assert_eq!(resolve_cfi(&dom, &stored.start.cfi), Some((node, off)));

        // The book was re-edited: the quote finds the text again.
        let edited = src.replace("<p>", "<p><b>Incipit.</b> ");
        let map2 = TextMap::new(&Dom::parse_xml(&edited).expect("parses"));
        let range = reanchor(map2.text(), stored.quote.as_ref().expect("quote")).expect("re-anchored");
        assert_eq!(&map2.text()[range], "cammin");
    }
}
