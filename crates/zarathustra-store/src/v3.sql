-- Schema v3: reading positions, keyed by book path (a book is read before it has a row in
-- `books`), each the app's "chapter block progress offset" text.
CREATE TABLE positions(
  path TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
