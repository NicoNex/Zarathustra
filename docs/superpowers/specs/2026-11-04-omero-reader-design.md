# Omero — e-reader shell for de-amazonized e-ink devices

Status: approved design, 2026-11-04. License: GPL-3.0-or-later (files derived from Plato stay AGPL-3.0, compatible via GPLv3 §13).
Mockups: https://claude.ai/artifact/S86BnPCFdz6rXdp2ceCpgf (50% scale, grayscale).

## 1. Goal

Replace the Amazon UI on a jailbroken Kindle with a minimal, fast, beautiful reader written in Rust: a home (library) and a reading view, nothing else by default. No store, no recommendations, no accounts.

Non-goals (for now): sync services, dictionaries, Wi-Fi features, a different Linux distro (planned later; the design must not block it).

## 2. Target device and portability

First target: **Kindle Paperwhite 5 (11th gen, non-Signature)**, FW 5.17.1, WinterBreak jailbreak, KOReader + KUAL installed.

| Fact | Value | Source |
|---|---|---|
| SoC | MediaTek, dual core, armhf userspace (FW ≥ 5.16.3) | KOReader `isMTK` |
| Display | 1236×1648, 300 dpi, MTK hwtcon EPDC, hardware swipe animation (`MTK_EPDC_FLAG_ENABLE_SWIPE`) | `koreader/ffi/framebuffer_mxcfb.lua`, `mxcfb_kindle_h.lua` |
| Frontlight | `/sys/class/backlight/fp9966-bl1/brightness` | KOReader `KindlePaperWhite5:init` |
| Warm light | `/sys/class/backlight/fp9966-bl0/brightness` | same |
| Battery | `/sys/class/power_supply/bd71827_bat/{capacity,charging,status}` | same |
| Light sensor | none → "automatic" light means time-based schedule | KOReader model table |
| Quirk | enable MTK "fast mode" so the driver doesn't promote refreshes to REAGL | same |

The reader must not be Kindle-specific. All hardware access goes through one `Device` trait (§4.1) with two implementations from day one: `kindle_mtk` and `emulator` (desktop window). A Kobo backend (mxcfb, portable from Plato) or another distro is a new implementation plus a model profile, with no changes to the UI or engine.

## 3. Sub-projects and order

1. **Base**: cross-build toolchain, `Device` trait + Kindle MTK + emulator backends, launch via KUAL, then an upstart job that replaces the framework at boot. Deliverable: a probe binary that draws, reads touch, sets light and warmth on the device.
2. **Reader core (EPUB)**: engine, reading view, page-turn animation, fonts, progress, reading-speed estimate.
3. **Home / library**: scanning `library/`, metadata, covers, grouping, search, long-press book menu.
4. **Highlights**: selection UX, storage, highlights screen, export.
5. **More formats**: kepub, txt, pdf (MuPDF), mobi/azw3.
6. **Quick settings + light automation** (swipe down from top).
7. **Actions launcher**: user executables (NickelMenu replacement, no config file).
8. **i18n**: Italian translation (the infrastructure lands in 2).

Each sub-project gets its own plan. This document is the umbrella design.

## 4. Architecture

A Cargo workspace:

| crate | job | depends on |
|---|---|---|
| `omero-device` | `Device` trait, Kindle MTK backend, emulator backend, model profiles | `nix`/`libc`, `evdev` |
| `omero-engine` | documents → paginated display lists; EPUB/TXT layout; MuPDF adapter | text stack (§5) |
| `omero-store` | SQLite: library, positions, highlights, reading stats | `rusqlite` (bundled) |
| `omero` (bin) | UI, views, i18n, settings, lifecycle | all above |

### 4.1 Device trait

```rust
pub trait Device {
    fn profile(&self) -> &Profile;                       // name, dpi, size, capabilities
    fn framebuffer(&mut self) -> &mut dyn Framebuffer;   // gray8 buffer + refresh(rect, mode, flags)
    fn input(&mut self) -> &mut dyn InputSource;         // raw fd for epoll + decoded events
    fn light(&mut self) -> Option<&mut dyn Frontlight>;  // brightness + optional warmth
    fn battery(&self) -> Battery;
    fn suspend(&mut self) -> io::Result<()>;
}
```

Refresh modes are abstract (`Full`, `Text`, `Fast`, `Mono`, and the `Swipe(direction)` flag); each backend maps them to its waveforms. Capabilities that are missing (warmth, swipe, light sensor) are `Option`s or flags, and the UI hides what isn't there.

### 4.2 Threads and power

- **UI thread**: blocks on `epoll` (input fd, timerfd, eventfd from workers). No polling. Idle = CPU sleeps.
- **Render worker** (second core): lays out and rasterizes the next and previous pages ahead of time. A page turn blits a ready buffer and refreshes.
- **Background layout**: low priority (`nice`), runs only when settings or font change, to rebuild pagination and the page count.
- Refresh pipeline: one in-flight update. Newer requests for the same region replace queued ones (latest wins), and we wait on the update marker before submitting dependent updates.
- **Responsiveness budgets** (measured on device, regressions are bugs):
  - Touch-down → visible feedback < 50 ms: the pressed control is inverted with a small-region fast mono refresh before any work starts.
  - Page turn (pre-rendered) → refresh submitted < 30 ms. Uncached page < 150 ms.
  - Menus and sheets open < 100 ms, drawn from pre-built layouts; only their region is refreshed.
  - Opening a book with a cached layout < 300 ms, showing the saved page first and paginating the rest in the background.
- The UI thread never does blocking I/O, layout of whole chapters, cover decoding or SQLite writes larger than one row. Those go to workers and post results back via an eventfd.
- Only damaged rectangles are refreshed. The waveform is picked per change: mono/fast for UI feedback and drags, text waveform for page content, full only when ghosting accumulates.
- Full GC16 refresh every N page turns (default 8, configurable, 0 = never), plus on chapter start if chosen.

## 5. Document engine

### 5.1 EPUB pipeline

`zip` → OPF/NCX/nav parse (`quick-xml`) → per spine item: `html5ever` DOM → style cascade (`cssparser` + `selectors`, a CSS subset for books: fonts, margins, text-indent, alignment, lists, tables (simple), floats (images only), page-break-*) → box tree → **inline layout** → pagination → display list (positioned glyph runs, images, rules) carrying source positions.

Inline layout:
- Shaping with `harfrust` (official Rust port of HarfBuzz). Fonts parsed and outlined with `skrifa` (fontations), autohinted, rasterized with an anti-aliased scanline rasterizer into gray8. Glyph cache LRU keyed by (font, size, glyph, subpixel x bucket).
- Line break opportunities: `icu_segmenter` (UAX #14). Hyphenation: `hyphenation` crate (Liang/TeX patterns, embedded en + it, language from `xml:lang`/OPF).
- Justification: **Knuth–Plass** total-fit line breaking per paragraph, with a greedy fallback for pathological paragraphs (> N items) to bound latency.
- Pagination: widows/orphans control (2/2 default), keep headings with the next block, images scaled to fit the page.

Positions: an engine position is `(spine_index, dom_path, char_offset)`, serialized as an **EPUB CFI**. Pagination maps page ↔ position range. Layout is cached on disk per (book hash, layout-settings hash) so reopening a book is instant.

### 5.2 Other formats

- **kepub**: EPUB with Kobo `koboSpan` wrappers → same pipeline, spans ignored.
- **txt**: encoding detection (`chardetng` + `encoding_rs`), paragraphs → same inline layout.
- **pdf / cbz**: MuPDF via a thin C wrapper (reuse Plato's `mupdf_wrapper` approach), rendered as page images, text extracted for highlights.
- **mobi / azw3**: our own Rust MOBI/KF8 parser that produces resources for the EPUB pipeline (MuPDF has no KF8 support). DRM-free only.

## 6. Reading view

- Tap zones: left 1/3 = back, right 2/3 = forward, center = menu. Swipe horizontal = turn page, swipe down from the top edge = quick settings. Hardware page buttons, if a device has them, are mapped in its profile.
- **Page-turn animation**: hardware swipe on MTK, toggle in Aa menu (default on). Backends without it fall back to a plain refresh.
- **Header** (each field toggleable): clock (optional), chapter title, battery.
- **Footer** (each field toggleable): book progress bar with chapter tick marks; "time left in chapter" (or in book); page `n / total` and percent.
- **Reading speed**: words per page are known from layout. For each page turn, `dt` is accepted if 2 s ≤ dt ≤ 10 min, after the menu closes, and while not suspended. WPM = words/dt, smoothed by an exponentially weighted mean (α = 0.1) per book, starting from a global per-user prior (initial 230 wpm). Time left = remaining words in chapter / WPM, rounded to minutes. Stored in `reading_stats`.
- **Menu (center tap)**: top bar (back to library, title, contents, search in book, bookmark) and bottom sheet (Aa). Aa: font (3 recent + "All fonts"), size (10 steps), line spacing (3), margins (3), justify, hyphenation, page-turn animation, progress slider.

## 7. Home

- "Continue reading" hero card (cover, title, author, chapter, progress, time left).
- Grouping chips: **All, Authors, Series, Folders** (subfolders of `library/`), **Language, Reading, Unread, Finished**.
- Sort: recently read (default), recently added, title, author, progress.
- **Search**: incremental, diacritics- and case-insensitive, over title, author, series. SQLite FTS5 with the `trigram` tokenizer (substring matching) plus `remove_diacritics`. Match highlighted.
- **Long-press a book** → bottom sheet: cover + metadata, **Highlights & notes (count)**, Bookmarks, Book details, Mark as finished/unread, Export highlights, Delete book (confirm).
- Bottom nav: Library · Highlights · Settings.
- Library scan: `inotify` on `library/` while running, plus a full rescan after a USB unplug. Books are identified by content hash (partial MD5 like KOReader's, so moving or renaming keeps state). Covers extracted once and cached as thumbnails.

## 8. Highlights

### 8.1 Selection UX (e-ink aware)

1. **Long-press and drag, as on stock Kindle**: hold on a word (~500 ms) and the word is selected. Without lifting the finger, dragging extends the selection live. On release, the handles stay and the action bar appears.
2. Handles can be dragged afterwards to adjust either end.
3. Snapping: start and end snap to word boundaries (`icu_segmenter` word break, handles apostrophes and hyphens). A double tap selects the sentence.
4. **Across pages**:
   - Drag the end handle (or keep dragging during the initial long-press) into the bottom-right edge zone and dwell ~600 ms → the page turns and the selection continues. Same backwards via the top-left zone.
   - Or with a pending selection, turn pages normally (any number) and tap the end word. A banner shows "Selection continues from page N".
5. **Hit testing**: per page, lines sorted by y, then binary search on the line and binary search on glyph x-advances → O(log n).
6. **Rendering on slow e-ink**: touch events are coalesced (only the latest position is processed when the panel is ready). The damage is the symmetric difference between old and new selection rectangles (merged per line). It is refreshed with the fast mono waveform while dragging, then the union area gets a gray-level clean-up refresh on release.
7. Actions: Highlight, Underline, Note, Copy, Cancel. Tapping an existing highlight opens it (change style, note, delete). Overlapping highlights merge on request, never silently.

### 8.2 Storage (`omero-store`)

SQLite at `/mnt/us/.omero/omero.db` (portable path from the profile), WAL mode, `foreign_keys=ON`, schema versioned with `PRAGMA user_version` migrations.

```sql
CREATE TABLE books(
  id INTEGER PRIMARY KEY, hash TEXT UNIQUE NOT NULL, path TEXT NOT NULL,
  title TEXT, authors TEXT, series TEXT, series_index REAL, language TEXT,
  format TEXT NOT NULL, added_at INTEGER NOT NULL, opened_at INTEGER,
  finished_at INTEGER, progress REAL, position TEXT  -- CFI
);
CREATE TABLE annotations(
  id TEXT PRIMARY KEY,               -- UUIDv7
  book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('highlight','underline','bookmark','note')),
  start_cfi TEXT NOT NULL, end_cfi TEXT,
  exact TEXT, prefix TEXT, suffix TEXT,   -- W3C TextQuoteSelector, 32-char context
  chapter TEXT, page_hint INTEGER,
  note TEXT, color INTEGER,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, deleted_at INTEGER  -- tombstone
);
CREATE TABLE events(                  -- change feed for scripts/plugins
  seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL,
  entity TEXT NOT NULL, entity_id TEXT NOT NULL, op TEXT NOT NULL
);
-- triggers on books/annotations insert/update append to events
CREATE TABLE reading_stats(book_id INTEGER REFERENCES books(id) ON DELETE CASCADE,
  day INTEGER, seconds INTEGER, pages INTEGER, words INTEGER, PRIMARY KEY(book_id, day));
```

Re-anchoring: resolve `start_cfi`/`end_cfi`. If the text there doesn't match `exact`, search the chapter (then the book) for `prefix+exact+suffix` with a Bitap/Myers fuzzy match and repair the CFIs. If it is unresolvable, keep the annotation, flagged "orphaned", still visible in lists.

Export: Markdown and JSON per book or all books (`/mnt/us/omero-exports/`).

## 9. Fonts

- Sources: `/mnt/us/fonts` (user, USB-visible) + system fonts from the profile (Bookerly on Kindle). Default: Bookerly.
- Scan on start and on USB unplug, indexed by `skrifa` name table: typographic family (name ID 16/1), style, weight, italic, variable axes. Classified serif/sans via OS/2 `sFamilyClass`/PANOSE.
- **Fonts screen**: 2-column grid of family cards, each rendered in its own face ("Aa" + name), with style count / "Variable" badge, filter chips (All, Serif, Sans, Publisher), and a "Publisher font" card (use embedded book fonts). Weight fine-tune: the `wght` axis on variable fonts, synthetic emboldening otherwise. This matters on e-ink.

## 10. Light

- Brightness and warmth sliders (large pill sliders, quick settings).
- **Automatic warmth**: a schedule, either sunset→sunrise (NOAA solar position algorithm from a configured lat/lon, no network) or custom hours. 30 min linear fade in small steps on a timerfd. A manual change overrides until the next transition.
- Brightness is not automatic (no sensor on PW5). Profiles with a sensor can enable it later.

## 11. Quick settings (swipe down from top edge)

Rounded panel: date/time, battery + estimated days left. Tiles (4×2): Wi-Fi, Auto warmth, Dark mode (inverted rendering), Rotate, Refresh screen, Screenshot, Sleep timer, Settings. Two 64 px pill sliders: Brightness, Warmth (with AUTO badge). A day bar shows the warm schedule.

## 12. Visual language

Grayscale only. Rounded corners everywhere (sheets 22–28 px, cards 14–16 px, buttons 12 px, chips fully rounded). Touch targets ≥ 44 px at 50% scale (≥ 88 device px). UI sans: IBM Plex Sans (bundled, OFL). Book text: the user's font. Black for active/selected, 1 px #ccc dividers, no shadows (they dither badly on e-ink).

## 13. i18n

Project Fluent (`fluent-bundle`): `en-US` base, `it` added later. Strings are never hard-coded in views. Plurals and number/date formatting via Fluent + `icu`.

## 14. Settings and actions

- Settings: `/mnt/us/.omero/settings.toml` (serde), written atomically (write temp + rename).
- **Actions** (sub-project 7): any executable in `/mnt/us/omero/actions/` shows up in a menu, named after its filename (no config file). It runs with env `OMERO_BOOK`, `OMERO_POSITION`, `OMERO_DB`, `OMERO_SELECTION`. Its stdout is shown as a toast.

## 15. Deployment

- Build: `cargo zigbuild --target armv7-unknown-linux-gnueabihf.<kindle glibc>` (glibc version read from the device), release profile with LTO.
- Install: copy `omero/` to the USB root. A KUAL extension launches it (stops `lab126_gui`, as KOReader's `--framework_stop`, and restores it on exit). Later an opt-in upstart job starts Omero at boot. USB mass storage keeps working as now.
- Folders: `library/`, `fonts/` at the USB root, created on first run.

## 16. Errors and robustness

- The reading position is saved on every page turn (single upsert, WAL) and on suspend: never lose a position.
- Panic hook: log to `/mnt/us/.omero/crash.log`, restore the framework if we stopped it, exit non-zero (the KUAL/upstart wrapper restarts once).
- A corrupt book shows an error card and never crashes the library scan.

## 17. Testing

- Emulator backend: same app in a desktop window (`minifb`/`softbuffer`), the main dev loop.
- Golden-image tests for layout (render fixtures → compare gray8 PNG).
- Unit tests: Knuth–Plass, hyphenation hookup, CFI round-trip, re-anchoring (edited text), reading-speed filter, selection damage diff, sunrise/sunset times.
- On-device smoke test script (over SSH/USB-net when available).
