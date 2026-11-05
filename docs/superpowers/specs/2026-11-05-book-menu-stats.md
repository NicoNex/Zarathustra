# Book menu, book details, reading statistics

User request (2026-11-05): a long press on a book opens a context menu with useful actions
(the blurb, the highlights, …); a reading statistics screen, also per book. Priority: EPUB
usability, home and UI first.

## Book menu (long press on a cover, list row or the continue-reading card)

A card over the home, centred, with the book's small cover, title and author, then rows:

- **Read** (or **Continue reading**) — opens the book.
- **About this book** — the details screen.
- **Highlights (N)** — the existing highlights list (`notebook.rs`).
- **Mark as read** / **Mark as unread** — read: progress 1.0, position kept; unread: the
  saved position is removed (the book starts over, as on the Kindle).

A tap outside the card closes it.

## About this book (full screen)

Top bar "‹ Library" / "About". Cover, title, author, series; the description (the blurb:
`dc:description` for EPUB, EXTH 103 for MOBI, HTML stripped) wrapped to the room left;
facts: format, file size, language. Then **Your reading** for this book: time spent,
pages read, days read, last read, estimated time left (pages left × the reading pace).

Metadata is read on demand (`library::details`), not cached: the scan cache stays as is.

## Reading statistics (full screen, from a "Stats" button in the home header)

Today, this week, current streak (consecutive days with reading); a bar per day for the
last 14 days; totals: time read, pages read, books finished.

## Recording

`reading_stats(book_id, day, seconds, pages)` already exists in the schema. Every forward
page turn adds the time spent on the page left (the same skim/away filter as the pace:
2 s – 10 min) and one page, for the local day. Written through the reader's store
connection (WAL, one upsert per turn).

## Not now

Words read, sessions, per-hour charts, goals, sync.
