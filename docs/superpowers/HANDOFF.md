# Handoff — 2026-11-05

Read CLAUDE.md first (workflow, build/deploy, reviews at milestones, agents in worktrees for independent modules). Talk to the user in Italian.

## State (HEAD 8bd5200, all on master)

Usable app on the Kindle: `KINDLE_HOST=192.168.1.116 scripts/deploy.sh` (stops a running instance), then KUAL → Omero → Open Omero, or over SSH `/mnt/us/extensions/omero/run.sh omero` (log: `/mnt/us/omero/omero.log`). Desktop render: `cargo run --release -p omero --bin omero -- [BOOK] --pgm DIR` (home.pgm, page0-2.pgm, menu.pgm).

- Engine (`crates/omero-engine`): EPUB + AZW3/MOBI (`mobi/`, all 22 user AZW3 parse) behind `book::Book`; DOM, CSS cascade, Knuth–Plass + hyphenation + protrusion (`layout/paragraph.rs`), blocks (`layout/flow.rs`), pagination with widows/orphans and images (`layout/page.rs`), fonts (`fonts.rs`, sparse table read: startup 0.5 s with 128 MB of system fonts), images (`image.rs`, zune decoders, 16-gray dither, cache), library scan + metadata/cover cache (`library.rs`, header "omero-library 3").
- Shell (`crates/omero`): `bin/app.rs` (event loop, screens, refresh policy: page turns animate with MTK swipe, flash every 8), `home.rs` (cover grid / list toggle, continue-reading card on page 1, filters All/Unread/Reading/Read), `reader.rs` (taps: left quarter back, rest forward, swipes, top band → menu; position per book in state dir), `menu.rs` (typography sheet: size, font, spacing, margins, alignment; persisted `typography.txt`), `quick.rs` + `settings.rs` (quick panel: swipe down from top edge; brightness/warmth sliders, auto warmth by time, tiles), `ui.rs` (rounded rects, labels, clip_corners).

## Open bugs (user-reported, not fixed yet)

1. **Quick-panel sliders lag and glitch**: every Move triggers a full panel render + `wait_refresh` + DU refresh in `bin/app.rs` → backlog. Fix: coalesce touch moves per event batch (render/refresh once per batch, latest value), don't `wait_refresh` before DU updates during a drag; render the slider in pure black/white while dragging (DU on grays shows the "big rectangles"), final GL16 (`Waveform::Text`) on release.
2. **Brightness changes warmth**: `omero-device/src/sysfs.rs` `SysfsLight::set` writes brightness to the white LED (fp9966-bl1) and warmth to the warm LED (bl0) independently. KOReader drives powerd (`lipc flIntensity`, `currentAmberLevel` 0–24) which mixes both LEDs. Fix: mix in `SysfsLight` — total = perceptual curve of brightness (e.g. quadratic), cool = total·(1−w), warm = total·w — or call powerd via `lipc-set-prop com.lab126.powerd flIntensity/currentAmberLevel` (coarser, process per call: throttle).
3. Reader menu "not comfortable, needs order": a redesign agent was running (branch `worktree-agent-a36223eceddd4e6d3`, touches `menu.rs`, menu parts of `reader.rs`, `ui.rs`). Merge it if committed; reader.rs changed since (images, first_block) → expect small conflicts.

## Agents possibly still running / branches to check

- `worktree-agent-a87ed920ec7d87ea7` — highlights SQLite store (rusqlite bundled, CFI + text-quote anchors, events). Merge, then build the long-press-drag highlight UX on top (spec §highlights) and the home long-press sheet.
- `worktree-agent-aee5e86814ff55bbb` — power button + sleep screen with cover (`kindle/power.rs`, `sleep.rs`, notes in `docs/superpowers/notes/kindle-power.md`). It was told to WAIT before suspend tests on the device while the user tests; tell it when the device is free (SendMessage won't survive a new session: check the branch and re-run the device test yourself if needed).
- `git branch | grep worktree-agent` lists them; `git log master..BRANCH` shows unmerged work; worktrees in `.claude/worktrees/`.

## Next steps (user priorities)

1. Fix bugs 1–2, merge menu redesign, deploy, let the user test.
2. Merge highlights store → highlight UX (long press then drag, multi-page) → highlights sheet from the home (long press a book).
3. Merge power/sleep → integrate into app loop per its report.
4. Reader header: clock (setting exists: `settings.clock`), chapter time-left from reading speed (EWMA), TOC navigation (Mobi/EPUB `toc` already parsed).
5. Then: kepub/txt/pdf (MuPDF), i18n (Fluent, Italian), launcher for executables, boot without the Amazon framework.
6. Run `/code-review` + simplify on the diff since the last review (last reviewed: d9c7840 paragraph builder; everything after is unreviewed).

## Device facts

PW5, FW 5.17.1, MTK hwtcon, fb 8bpp rotate=3 handled by driver, touch `/dev/input/event1` no transform, SSH root@192.168.1.116:2222 (KOReader dropbear, key auth). Books: `/mnt/us/documents/<Author>/*.azw3` (calibre layout), future `/mnt/us/library`. Fonts: `/usr/java/lib/fonts` (+ `/mnt/us/fonts`). State: `/mnt/us/omero/state` (positions, settings.txt, typography.txt, home_view.txt, library cache). Local test assets (gitignored): `dev-fonts/`, `dev-books/` (+ `dev-books/azw3/`).
