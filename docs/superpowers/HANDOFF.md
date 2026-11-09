# Handoff — 2026-11-09

## 2026-11-09: Extensions, plugin actions, Fluent plugin strings, reading clock

- Spec `specs/2026-11-09-extensions-stats-design.md`, plan `plans/2026-11-09-extensions-stats.md`.
- Settings → **Extensions** (`extensions.rs`): per plugin its service switch, action buttons, last-run line (Error chip on failure; tap → `outsheet.rs` full stdout/stderr, paged). Same block on the plugin page.
- Actions (`plugin/action.rs`) run detached in their own process group via an `sh` wrapper writing the exit status; files `data/<id>/action.*`; Stop (TERM, KILL after 3 s); optional `timeout_seconds`; `output = quiet|show`; `shown_in = ["book"]` puts one in the book menu (`ZARATHUSTRA_BOOK`, `ZARATHUSTRA_POSITION`, `{book}`). Polled every 1 s while running (home `transferring()`).
- Plugin strings via Fluent (`plugin_i18n.rs`, `locales/<lang>.ftl`, ids `display-name`, `description`, `action-<id|exe>`, `setting-<id>[-empty|-option-<v>]`); `ZARATHUSTRA_LANGUAGE` for executables; SSH manifest now English with it/en .ftl.
- Reading time like KOReader (`pace.rs`): skim < 5 s ignored, page capped at 120 s, clock paused in overlays, selection, quick panel, sleep; flushed on any turn, close (Drop), sleep, USB, update restart.
- Home covers decoded on a low-priority thread right after the home shows (`Home::prefetch`).
- PDFs (2026-11-09): menu without the typography controls; search (`Search::step_with` over pages, MuPDF page text with a box per character, `pdf::PageText`) with the hit marked on the page; highlights (`pdfmarks.rs`: long press, drag word by word, Highlight/Delete bar; stored with the page as spine and `pdf:<page>:<byte>` CFIs plus the quote). Shortcuts: a selection stays on one page, no end knobs. Not yet tried on the device.
- Settings → "Informazioni sul dispositivo" (`device_info.rs`): model and dpi (from the device, kept in `Ctx`), system version (/etc/prettyversion.txt), app version, battery (first power_supply of type Battery), storage (statvfs /mnt/us), memory, IP (UDP route, nothing sent), MAC, screen, uptime. No serial number on purpose. Not yet tried on the device.
- PDF parked by the user (not a main feature): text in reading order by geometry (`pdf::reading_order`), line-first hit testing, a 24 px shake allowance after the long press. User still finds PDF selection unsteady; resume only if asked.
- Reader/PDF clock: the app wakes as the minute turns while the clock shows, and refreshes the header.
- Home footer: « and » jump to the first and last page (3ca1154).
- User confirmed 2026-11-09: device information, SSH port typed on the number pad. MuPDF base-14 fonts on (`base14-fonts`, +0.6 MB): PDFs with non-embedded Helvetica/Times now show and find text.
- Review + simplify of ca53b07..HEAD done (b70d74e); **last reviewed: b70d74e**. Skipped there: generic store layer for both highlight kinds, shared text-entry widget with the Wi-Fi password, dehyphenated PDF text, boxes-free text for search.
- User confirmed on the device 2026-11-09: highlights across pages with knobs, reading figures, footnotes, light mix, center alignment, SSH port typed on the keyboard. Still to check: Wi-Fi join with '#'/'!' passwords.
- SSH plugin: "Connection info" action (manifest only: runs `status` with `output = "show"`), because the plugin page (port, address, host key) is reached only by tapping the plugin's name in Extensions and nothing hints at it. Then: `›` after each plugin's name in Extensions; `text` settings typed on the keyboard (`PluginPage::edit_tap`, full redraw per key: no pressed key or accents there), SSH port is now `text` (run/status fall back to 22 on anything but 1-65535). Confirmed on the device.
- Simplify skips, worth doing when touching this again: an `ActionBlock` struct owning run/sheet/poll (Extensions and PluginPage duplicate the glue); a `Live` trait for screens the app polls (home → prefs → extensions → page, by hand today); an app-level "away" state instead of `screen.away()` at each exit path; one plugin list cached across screens (each screen calls `discover`).
- Amazon services inventory for de-Amazon phase A: `research/2026-11-09-amazon-services-inventory.md` (when the agent finishes).

## 2026-11-08 (evening): state — SSH plugin works on the device

- **Last reviewed: HEAD after "fix: code review of 7b5105e..HEAD"** (simplify + code-review applied). Next review from there.
- On the device and verified: prefs + positions in the DB (schema v3), plugin system, SSH plugin (ssh, scp, sftp; keys served from /var/zarathustra-ssh because /var/local belongs to Amazon's `framework` user and Dropbear checks folder owners), `make update` (app restarts into the new build in seconds, Amazon UI stays stopped), restart after a USB install on unplug, back pill with % and ×, drop knobs, no double edge turn, quote every 15 min, light after USB.
- a03e9e3 itself is NOT on the device yet (Kindle asleep): `make update` when it's awake.
- Deploy: `make update` / `make install-ssh` find the Kindle with `scripts/find-kindle.sh` (DHCP moves it: .31/.32/.33 in one afternoon); port 22 = our SSH plugin. `scripts/build-ssh.sh` must be run by the user (`! scripts/build-ssh.sh`): the auto-mode classifier blocks building downloaded code.
- Next: (1) DONE: the drag cache stays (measured 10.9 vs 24.6 ms per frame, note on `Reader::drag_base`); (2) DONE 2026-11-09: every screen checked with a temporary back-buffer-vs-screen compare on the device; only the PDF selection bar (outside its refresh) and the header clock (refreshed every 5 min) were stale, both fixed; (3) the white Amazon USB screen after "eject" (stays until the cable is pulled); (4) boot hook for `--start-services` (de-Amazon phase A), then show the "start at power-on" row (`plugin_page::BOOT_HOOK`); (5) positions stay keyed by path, like Kobo (user's decision 2026-11-09; if ever changed: book_key falling back to KOReader's partial MD5 instead of the path, v4 migration, enables KOReader progress sync); (6) plugin actions in the selection popup (`{selection}`).
- Perf 2026-11-09: startup 5.5 s → 0.1 s ("home ready"; covers decoded when first drawn). The MTK framebuffer mapping is uncached (reading it ~0.3 µs/byte, writing fast), so `MtkFb` now draws into a RAM back buffer and `refresh()` copies the rectangle out: a selection frame went 60–130 ms → 16 ms. A page-long selection's popup now goes mid-screen.
- Flaky hyphenation test still open (see below).

## Highlights across pages: DONE 2026-11-08, confirmed on the device 2026-11-09 (knobs too)

- While selecting, holding the finger 600 ms below the last line or above the first line turns the page (same chapter only; `highlight::EDGE_DWELL`, `Reader::edge_turn`); the selection stretches to that page's last/first word and keeps following the finger.
- A selection ending on a word takes the closing punctuation after it (`highlight::with_punct`).
- With the actions showing, two knobs mark the ends; a finger down on one moves that end (`Highlights::grab`, wired on `Phase::Down` in app.rs).
- Reviewed (code-review + simplify, fixes in 9bb77c9; **last reviewed: 9bb77c9**) and installed over USB 2026-11-08; user to try all three.

**Flaky test, not yet found**: `layout::hyphen::tests::hyphenates_from_data_files_with_tag_fallbacks` failed 5 times in a row in `cargo test --workspace` right after rebuilds (`breaks("en", "hyphenation")` returned `[]`), then never again (80+ runs, alone and full, also with `ulimit -n 64`). Not caused by the highlight changes (engine untouched). User parked it for now; resume by catching it with an `eprintln!` of the `Standard::from_path` error in `Hyphenators::load`.

Then: verify the Wi-Fi join with '#'/'!' passwords (wpa_cli fallback), the USB screen, the Kobo groundwork (see below).

Read CLAUDE.md first (workflow, build/deploy, reviews at milestones, agents in worktrees for independent modules). Talk to the user in Italian.

## State (all on master)

Usable app on the Kindle: `make install-ssh` (stops a running instance; `make install-usb` over USB), then KUAL → Zarathustra → Open Zarathustra, or over SSH `/mnt/us/extensions/zarathustra/run.sh zarathustra` (log: `/tmp/zarathustra.log`, i.e. /var/tmp). Desktop render: `cargo run --release -p zarathustra --bin zarathustra -- [BOOK] --pgm DIR` (home.pgm, page0-2.pgm, menu.pgm, footnote*.pgm).

Footnotes (`zarathustra-engine/src/note.rs`, `zarathustra/src/footnote.rs`): tap a note reference (epub:type/role noteref, or an unmarked marker link like `12`/`*`/`[3]`, often in `<sup>`) → the note in a card over the page, laid out by the engine; long notes page inside the card (dots), tap outside or × closes. Not yet tested on the device.

- Engine (`crates/zarathustra-engine`): EPUB + AZW3/MOBI (`mobi/`, all 22 user AZW3 parse) behind `book::Book`; DOM, CSS cascade, Knuth–Plass + hyphenation + protrusion (`layout/paragraph.rs`), blocks (`layout/flow.rs`), pagination with widows/orphans and images (`layout/page.rs`), fonts (`fonts.rs`, sparse table read: startup 0.5 s with 128 MB of system fonts), images (`image.rs`, zune decoders, 16-gray dither, cache), library scan + metadata/cover cache (`library.rs`, header "zarathustra-library 3").
- Shell (`crates/zarathustra`): `bin/app.rs` (event loop, screens, refresh policy: page turns animate with MTK swipe, flash every 8), `home.rs` (cover grid / list toggle, continue-reading card on page 1, filters All/Unread/Reading/Read), `reader.rs` (taps: left quarter back, rest forward, swipes, top band → menu; position per book in state dir), `menu.rs` (typography sheet: size, font, spacing, margins, alignment; persisted `typography.txt`), `quick.rs` + `settings.rs` (quick panel: swipe down from top edge; brightness/warmth sliders, auto warmth by time, tiles), `ui.rs` (rounded rects, labels, clip_corners).

## Status (2026-11-05 evening, master after "vendor-measured frontlight mix")

Merged and deployed: highlights store + UX (long press, drag, popup; `highlight.rs`), power/sleep screen, menu redesign + TOC (`toc.rs`), reader clock + "min left in chapter" (`pace.rs`), center alignment (`menu::ALIGNS`, `Preferences.align`), frontlight driven by powerd's measured table (`zarathustra-device/src/kindle/pw5-light.txt`, `sysfs::Mix`). Sliders: user confirmed no more gray blocks (fix: wait_refresh BEFORE drawing into the fb). Light mix + center: deployed, NOT yet confirmed by the user.

Review/simplify of d9c7840..HEAD DONE and merged (engine: 9942ffb, 234c71f; shell: b985cbc + shared UI helpers/clock.rs/lazy covers). **Last reviewed: 2de82c5** (review of 08ca9a6..2de82c5 merged as 9ea4565^; f23799f home sort and later not yet). Merged fixes NOT yet deployed (device was offline): deploy and let the user test light mix, center alignment, broken-chapter skip, exact positions, highlight edges.

## In flight (2026-11-06) — check `git branch | grep worktree-agent` and `git log master..BRANCH`

Merged (not yet on the device): review fixes (2de82c5 reviewed), library sort, bookmarks + scrubber + back pill (`nav.rs`), footnote popup (`footnote.rs`, engine `note.rs`), dictionaries (StarDict, engine `dict/`, `dict.rs` card on long press of a word; Kindle .azw dicts are DRM → free Wiktionary dicts via `scripts/fetch-dicts.sh [push]` to /mnt/us/dictionaries), in-book search (`keyboard.rs`, `search.rs`, engine `search.rs`). Slow all-books note sweep test is `#[ignore]`.

Power-button branch merged (2026-11-06). Book menu (`bookmenu.rs`, long press on a book): open, contents, bookmarks, search (open the book with that reader overlay via `Action::OpenOn`), about, highlights, mark read/unread, delete (second tap confirms). Device offline at last try: `scripts/fetch-dicts.sh push`, deploy, let the user test everything.

Known gaps: stats miss the last page's time before leaving a book and count time in menus; Wikipedia skipped (ureq+rustls +1.3 MB, plan in dict.rs ponytail).
Review rule: next review+simplify from 14c975b when `git diff --shortstat 14c975b..HEAD -- crates` passes ~2000 added lines.

## 2026-11-06 evening: state

Renamed Omero → Zarathustra (crates, binaries, KUAL, /mnt/us/zarathustra; the one-off /mnt/us/omero migration is gone). `make install-ssh [KINDLE_HOST=ip]` or `make install-usb`. Books: only /mnt/us/library/ is shown (deploy creates it empty; documents/ ignored by request).
Done and deployed (not yet tried by the user on the device): compact typography sheet (size + weight row, font ‹ › row, icon row), synthetic text weight (engine `font::WEIGHTS`), floating book menu beside the cover, home kept in memory while reading + `Home::refresh` (30 s on the home, on return, after delete/transfer), library search, settings screen (language, full-refresh interval), keyboard accents on long press + comma/period, power button menu (suspend/exit/restart/power off), Wi-Fi transfer (`transfer.rs`: QR + URL, PUT uploads into library/, iptables port opened on the device while open; untested on the device: Wi-Fi with the framework stopped, firewall).
Open: collections (design proposed, not agreed), landscape/Kobo (W/H are consts), opening speed measured only on the device.

## Next steps (user priorities)

1. Fix bugs 1–2, merge menu redesign, deploy, let the user test.
2. Highlights: DONE (store, long press + drag, popup; home long press on a book → `notebook.rs` list, tap opens the book there via `Reader::goto`). Deployed f77d4ec, user to test.
3. Merge power/sleep → integrate into app loop per its report.
4. Reader header: clock (setting exists: `settings.clock`), chapter time-left from reading speed (EWMA), TOC navigation (Mobi/EPUB `toc` already parsed).
5. Then: kepub/txt/pdf (MuPDF), i18n (Fluent, Italian), launcher for executables, boot without the Amazon framework.

## Device facts

PW5, FW 5.17.1, MTK hwtcon, fb 8bpp rotate=3 handled by driver, touch `/dev/input/event1` no transform, SSH root@192.168.1.113:2222 (DHCP: the address moves; was .116 — scan port 2222 if unreachable, same ED25519 key SHA256:W/z7vTC…) (KOReader dropbear, key auth). Books: `/mnt/us/documents/<Author>/*.azw3` (calibre layout), future `/mnt/us/library`. Fonts: `/usr/java/lib/fonts` (+ `/mnt/us/fonts`). State: `/mnt/us/zarathustra/state` (positions, settings.txt, typography.txt, home_view.txt, library cache). Local test assets (gitignored): `dev-fonts/`, `dev-books/` (+ `dev-books/azw3/`).

## State at 2026-11-06 (end of session)

- Verified on device: Wi-Fi (status, list, toggle; creating a new profile still unverified), USB plug detection (prompt appears).
- **USB, diagnosed 2026-11-06**: since run.sh and the binary run from /tmp nothing holds the userstore, so volumd enters drive mode **by itself** on plug (no framework needed): the storage reaches the computer, a white/Amazon USB screen covers ours, and Zarathustra (still running, exit status 0 in the log) shows again after unplug once the storage is remounted (slow). So our prompt and run.sh's exit-42 loop are moot. Next: replace the prompt with our own "connected" screen drawn after volumd's (or ask volumd not to draw/to defer: `userstoreDeferUnmountRequest`), find what draws the white screen (blanket module?), rescan the library on unplug, and offer "just charge" through volumd's defer request if it works.
- Wi-Fi: joining a **new** network fails at `lipc-hash-prop com.lab126.wifid createProfile`. Log of the 4 syntaxes (`hash_records`): bare keys with commas → "unexpected KEY, expecting STRING or INTEGER or HEX"; bare keys without commas → "unexpected KEY, expecting EBRACE or COMMA". So `{ key = "value", … }` with bare keys and commas is the grammar (the first pair parses); a later **value** lexes as KEY, most likely the password's characters (special characters or spaces inside the quoted string). Next: ask which kinds of characters the password has, test with a plain password (hotspot), check `lipc-hash-prop` escaping, or pass the PSK another way (HEX token? wpa_cli fallback). Keep only syntax 0 afterwards.
- Verified: OTA updates already blocked by the jailbreak; light after wake fixed.
- KUAL: "Wi-Fi probe" (dumps wifid incl. passwords — delete its output after reading), "System probe" (read-only inventory, output on the device at zarathustra/system-probe.txt).
- De-Amazon plan: phase A (clean the Amazon system: USB, battery, clock, boot to Zarathustra with a safety file, services off; OTA may already be blocked by the jailbreak — check), phase B (Alpine with the Amazon kernel and nothing else Amazon: our own Wi-Fi bring-up and power management).
- Never commit device data (SSIDs, passwords, MACs, IPs).

## Next device: Kobo (soon; the user owns one). Decided 2026-11-06: one binary, drivers as modules

- Keep a single static binary; device backends are modules/crates in this repo behind `zarathustra_device::Device` (+ `Wifi`), picked at startup by detecting the model. No dynamic `.so` drivers (no stable Rust ABI; would lose the single static binary).
- Before the Kobo backend:
  1. **Resolution-independent UI**: everything is laid out for 1236×1648 (`W`/`H` consts in `zarathustra/src/lib.rs`, absolute rects across home/menu/quick/wifi/keyboard…). Make layout derive from the device size (scale factor from dpi/size), then drop the consts.
  2. **Platform bits into the driver**: `/mnt/us` paths (`on_device`, `library_dir`, state dir, fonts, dictionaries, hyphenation), lipc, KUAL/`run.sh` launch and exit, USB handling. A small platform part of the backend says where books/state live and how to launch/quit.
  3. **"Add a device" guide**: trait to implement, testing with the emulator and `--pgm`, model detection.
- Then `kobo/`: framebuffer (mxcfb or MTK per model), touch, light (sysfs), power (no powerd: own suspend), Wi-Fi (wpa_cli; Kobo scripts), launch via NickelMenu/KFMon.
