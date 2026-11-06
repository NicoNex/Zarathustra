# Handoff — 2026-11-05

Read CLAUDE.md first (workflow, build/deploy, reviews at milestones, agents in worktrees for independent modules). Talk to the user in Italian.

## State (all on master)

Usable app on the Kindle: `KINDLE_HOST=192.168.1.113 scripts/deploy.sh` (stops a running instance), then KUAL → Omero → Open Omero, or over SSH `/mnt/us/extensions/omero/run.sh omero` (log: `/mnt/us/omero/omero.log`). Desktop render: `cargo run --release -p omero --bin omero -- [BOOK] --pgm DIR` (home.pgm, page0-2.pgm, menu.pgm, footnote*.pgm).

Footnotes (`omero-engine/src/note.rs`, `omero/src/footnote.rs`): tap a note reference (epub:type/role noteref, or an unmarked marker link like `12`/`*`/`[3]`, often in `<sup>`) → the note in a card over the page, laid out by the engine; long notes page inside the card (dots), tap outside or × closes. Not yet tested on the device.

- Engine (`crates/omero-engine`): EPUB + AZW3/MOBI (`mobi/`, all 22 user AZW3 parse) behind `book::Book`; DOM, CSS cascade, Knuth–Plass + hyphenation + protrusion (`layout/paragraph.rs`), blocks (`layout/flow.rs`), pagination with widows/orphans and images (`layout/page.rs`), fonts (`fonts.rs`, sparse table read: startup 0.5 s with 128 MB of system fonts), images (`image.rs`, zune decoders, 16-gray dither, cache), library scan + metadata/cover cache (`library.rs`, header "omero-library 3").
- Shell (`crates/omero`): `bin/app.rs` (event loop, screens, refresh policy: page turns animate with MTK swipe, flash every 8), `home.rs` (cover grid / list toggle, continue-reading card on page 1, filters All/Unread/Reading/Read), `reader.rs` (taps: left quarter back, rest forward, swipes, top band → menu; position per book in state dir), `menu.rs` (typography sheet: size, font, spacing, margins, alignment; persisted `typography.txt`), `quick.rs` + `settings.rs` (quick panel: swipe down from top edge; brightness/warmth sliders, auto warmth by time, tiles), `ui.rs` (rounded rects, labels, clip_corners).

## Status (2026-11-05 evening, master after "vendor-measured frontlight mix")

Merged and deployed: highlights store + UX (long press, drag, popup; `highlight.rs`), power/sleep screen, menu redesign + TOC (`toc.rs`), reader clock + "min left in chapter" (`pace.rs`), center alignment (`menu::ALIGNS`, `Preferences.align`), frontlight driven by powerd's measured table (`omero-device/src/kindle/pw5-light.txt`, `sysfs::Mix`). Sliders: user confirmed no more gray blocks (fix: wait_refresh BEFORE drawing into the fb). Light mix + center: deployed, NOT yet confirmed by the user.

Review/simplify of d9c7840..HEAD DONE and merged (engine: 9942ffb, 234c71f; shell: b985cbc + shared UI helpers/clock.rs/lazy covers). **Last reviewed: 2de82c5** (review of 08ca9a6..2de82c5 merged as 9ea4565^; f23799f home sort and later not yet). Merged fixes NOT yet deployed (device was offline): deploy and let the user test light mix, center alignment, broken-chapter skip, exact positions, highlight edges.

## In flight (2026-11-06) — check `git branch | grep worktree-agent` and `git log master..BRANCH`

Merged (not yet on the device): review fixes (2de82c5 reviewed), library sort, bookmarks + scrubber + back pill (`nav.rs`), footnote popup (`footnote.rs`, engine `note.rs`), dictionaries (StarDict, engine `dict/`, `dict.rs` card on long press of a word; Kindle .azw dicts are DRM → free Wiktionary dicts via `scripts/fetch-dicts.sh [push]` to /mnt/us/dictionaries), in-book search (`keyboard.rs`, `search.rs`, engine `search.rs`). Slow all-books note sweep test is `#[ignore]`.

Power-button branch merged (2026-11-06). Book menu (`bookmenu.rs`, long press on a book): open, contents, bookmarks, search (open the book with that reader overlay via `Action::OpenOn`), about, highlights, mark read/unread, delete (second tap confirms). Device offline at last try: `scripts/fetch-dicts.sh push`, deploy, let the user test everything.

Known gaps: PDF menu shows typography/search controls that do nothing; stats miss the last page's time before leaving a book and count time in menus; Wikipedia skipped (ureq+rustls +1.3 MB, plan in dict.rs ponytail).
Review rule: next review+simplify from e6f60ec when `git diff --shortstat e6f60ec..HEAD -- crates` passes ~2000 added lines.

## Next steps (user priorities)

1. Fix bugs 1–2, merge menu redesign, deploy, let the user test.
2. Highlights: DONE (store, long press + drag, popup; home long press on a book → `notebook.rs` list, tap opens the book there via `Reader::goto`). Deployed f77d4ec, user to test.
3. Merge power/sleep → integrate into app loop per its report.
4. Reader header: clock (setting exists: `settings.clock`), chapter time-left from reading speed (EWMA), TOC navigation (Mobi/EPUB `toc` already parsed).
5. Then: kepub/txt/pdf (MuPDF), i18n (Fluent, Italian), launcher for executables, boot without the Amazon framework.

## Device facts

PW5, FW 5.17.1, MTK hwtcon, fb 8bpp rotate=3 handled by driver, touch `/dev/input/event1` no transform, SSH root@192.168.1.113:2222 (DHCP: the address moves; was .116 — scan port 2222 if unreachable, same ED25519 key SHA256:W/z7vTC…) (KOReader dropbear, key auth). Books: `/mnt/us/documents/<Author>/*.azw3` (calibre layout), future `/mnt/us/library`. Fonts: `/usr/java/lib/fonts` (+ `/mnt/us/fonts`). State: `/mnt/us/omero/state` (positions, settings.txt, typography.txt, home_view.txt, library cache). Local test assets (gitignored): `dev-fonts/`, `dev-books/` (+ `dev-books/azw3/`).
