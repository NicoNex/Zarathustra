# Handoff — 2026-11-05

Read CLAUDE.md first (workflow, build/deploy, reviews at milestones, agents in worktrees for independent modules). Talk to the user in Italian.

## State (HEAD 8bd5200, all on master)

Usable app on the Kindle: `KINDLE_HOST=192.168.1.116 scripts/deploy.sh` (stops a running instance), then KUAL → Omero → Open Omero, or over SSH `/mnt/us/extensions/omero/run.sh omero` (log: `/mnt/us/omero/omero.log`). Desktop render: `cargo run --release -p omero --bin omero -- [BOOK] --pgm DIR` (home.pgm, page0-2.pgm, menu.pgm).

- Engine (`crates/omero-engine`): EPUB + AZW3/MOBI (`mobi/`, all 22 user AZW3 parse) behind `book::Book`; DOM, CSS cascade, Knuth–Plass + hyphenation + protrusion (`layout/paragraph.rs`), blocks (`layout/flow.rs`), pagination with widows/orphans and images (`layout/page.rs`), fonts (`fonts.rs`, sparse table read: startup 0.5 s with 128 MB of system fonts), images (`image.rs`, zune decoders, 16-gray dither, cache), library scan + metadata/cover cache (`library.rs`, header "omero-library 3").
- Shell (`crates/omero`): `bin/app.rs` (event loop, screens, refresh policy: page turns animate with MTK swipe, flash every 8), `home.rs` (cover grid / list toggle, continue-reading card on page 1, filters All/Unread/Reading/Read), `reader.rs` (taps: left quarter back, rest forward, swipes, top band → menu; position per book in state dir), `menu.rs` (typography sheet: size, font, spacing, margins, alignment; persisted `typography.txt`), `quick.rs` + `settings.rs` (quick panel: swipe down from top edge; brightness/warmth sliders, auto warmth by time, tiles), `ui.rs` (rounded rects, labels, clip_corners).

## Status since (4fbe885)

- Merged: highlights store (`omero-store`, `omero-engine::anchor`) and power/sleep (sleep screen wired into `bin/app.rs`).
- Fixed, NOT yet verified on device: slider lag/glitch (batch-coalesced panel redraw, B/W slider while dragging), brightness↔warmth (LEDs mixed in `SysfsLight`, quadratic curve). Deployed over USB.
- Menu redesign agent left no commits (worktree a36223 is clean): redo it.
- Stale agent worktrees in `.claude/worktrees/` can be removed.

## Next steps (user priorities)

1. Fix bugs 1–2, merge menu redesign, deploy, let the user test.
2. Merge highlights store → highlight UX (long press then drag, multi-page) → highlights sheet from the home (long press a book).
3. Merge power/sleep → integrate into app loop per its report.
4. Reader header: clock (setting exists: `settings.clock`), chapter time-left from reading speed (EWMA), TOC navigation (Mobi/EPUB `toc` already parsed).
5. Then: kepub/txt/pdf (MuPDF), i18n (Fluent, Italian), launcher for executables, boot without the Amazon framework.
6. Run `/code-review` + simplify on the diff since the last review (last reviewed: d9c7840 paragraph builder; everything after is unreviewed).

## Device facts

PW5, FW 5.17.1, MTK hwtcon, fb 8bpp rotate=3 handled by driver, touch `/dev/input/event1` no transform, SSH root@192.168.1.116:2222 (KOReader dropbear, key auth). Books: `/mnt/us/documents/<Author>/*.azw3` (calibre layout), future `/mnt/us/library`. Fonts: `/usr/java/lib/fonts` (+ `/mnt/us/fonts`). State: `/mnt/us/omero/state` (positions, settings.txt, typography.txt, home_view.txt, library cache). Local test assets (gitignored): `dev-fonts/`, `dev-books/` (+ `dev-books/azw3/`).
