-- Schema v1: books, annotations, the change feed and reading stats (design spec §8.2).

CREATE TABLE books(
  id INTEGER PRIMARY KEY,
  hash TEXT UNIQUE NOT NULL,          -- book_key(): dc:identifier or content hash
  path TEXT NOT NULL,
  title TEXT, authors TEXT, series TEXT, series_index REAL, language TEXT,
  format TEXT NOT NULL,
  added_at INTEGER NOT NULL, opened_at INTEGER, finished_at INTEGER,
  progress REAL, position TEXT        -- CFI
);

CREATE TABLE annotations(
  id TEXT PRIMARY KEY,                -- UUIDv7
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('highlight','underline','bookmark','note')),
  spine INTEGER NOT NULL, pos INTEGER NOT NULL,  -- start: spine index + source byte offset (sort key)
  start_cfi TEXT NOT NULL, end_cfi TEXT,
  exact TEXT, prefix TEXT, suffix TEXT,          -- W3C TextQuoteSelector, 32-char context
  chapter TEXT, page_hint INTEGER,
  note TEXT, color INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  deleted_at INTEGER                             -- tombstone, kept for sync
);
CREATE INDEX annotations_by_book ON annotations(book_id, spine, pos) WHERE deleted_at IS NULL;
CREATE INDEX annotations_by_time ON annotations(created_at) WHERE deleted_at IS NULL;

-- Change feed for scripts, plugins and sync. Written by triggers, so every change and its
-- event commit (or roll back) together.
CREATE TABLE events(
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  entity TEXT NOT NULL, entity_id TEXT NOT NULL, op TEXT NOT NULL
);

CREATE TRIGGER books_insert AFTER INSERT ON books BEGIN
  INSERT INTO events(at, entity, entity_id, op) VALUES (unixepoch(), 'book', NEW.id, 'insert');
END;
-- Metadata and finished state only: progress/position/opened_at change on every page turn
-- and would flood the feed (and the flash).
CREATE TRIGGER books_update
AFTER UPDATE OF hash, path, title, authors, series, series_index, language, format, finished_at ON books BEGIN
  INSERT INTO events(at, entity, entity_id, op) VALUES (unixepoch(), 'book', NEW.id, 'update');
END;
CREATE TRIGGER books_delete AFTER DELETE ON books BEGIN
  INSERT INTO events(at, entity, entity_id, op) VALUES (unixepoch(), 'book', OLD.id, 'delete');
END;

CREATE TRIGGER annotations_insert AFTER INSERT ON annotations BEGIN
  INSERT INTO events(at, entity, entity_id, op) VALUES (unixepoch(), 'annotation', NEW.id, 'insert');
END;
CREATE TRIGGER annotations_update AFTER UPDATE ON annotations BEGIN
  INSERT INTO events(at, entity, entity_id, op) VALUES (unixepoch(), 'annotation', NEW.id,
    CASE WHEN NEW.deleted_at IS NOT NULL AND OLD.deleted_at IS NULL THEN 'delete' ELSE 'update' END);
END;
-- Hard deletes (a book removed with its annotations) after the tombstone was already logged
-- are not repeated.
CREATE TRIGGER annotations_delete AFTER DELETE ON annotations WHEN OLD.deleted_at IS NULL BEGIN
  INSERT INTO events(at, entity, entity_id, op) VALUES (unixepoch(), 'annotation', OLD.id, 'delete');
END;

CREATE TABLE reading_stats(
  book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
  day INTEGER, seconds INTEGER, pages INTEGER, words INTEGER,
  PRIMARY KEY(book_id, day)
);
