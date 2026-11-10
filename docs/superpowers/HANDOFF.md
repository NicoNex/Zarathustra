# Handoff — 2026-11-10

Read CLAUDE.md first. Talk to the user in Italian. Finished work lives in git history and the specs;
this file holds only the current state and what is open.

**Last reviewed: 265d141** (five-agent review of 5cf138e..HEAD). Skipped items are in the fix
commits' bodies.

## Now: Kobo Clara 2E (the Kindle is parked)

The app runs on the Kobo and the user uses it: reading, USB, SSH, light and warmth, sleep cover,
suspend, quick panel. Design and Clara 2E facts: `specs/2026-11-09-kobo-port-design.md`. Adding a
device: `docs/porting.md`.

**Next: Bluetooth.** Ask the user what it is for (page-turn remote, audio, keyboard), then
brainstorm → spec → plan. The user is undecided whether a Bluetooth tile should replace
"Cover magnetica" in the quick panel.

Still to check on the Kobo (needs the user):
- Nickel import: positions and highlights taken in once at the first start (`nickel.rs`; dry run:
  `NICKEL_DB=… NICKEL_LIBRARY=… cargo test -p zarathustra --lib nickel::tests::dry_run -- --ignored --nocapture`).
- Wi-Fi join of a new network, also with '#'/'!' in the password.
- Typing and night mode since drawing stopped waiting for the panel (`Device::wait_before_draw`).

Working on the Kobo:
- Deploy: `make update-kobo` (address: `DEVICE=kobo scripts/find-kindle.sh`; was 192.168.1.15).
  Log: `/tmp/zarathustra.log` (appends across restarts). SSH plugin keys: `/var/zarathustra/data`.
- Library `/mnt/onboard/library/`; app files `/mnt/onboard/.adds/zarathustra/`.
- Screenshot: `scripts/screenshot.sh` is Kindle-only. On the Kobo read `/dev/fb0`: 8 bpp, stride
  1088, visible 1072×1448 (crop the first 1088×1448 bytes).
- Scripted touch for latency tests: write 16-byte `input_event`s to `/dev/input/event1`
  (raw x = screen y, raw y = 1071 − screen x). It turns real pages: avoid the user's books or put
  the positions back.
- Waveforms (ioctl takes the first number of sysfs `code:index`), full screen: A2 238 ms, DU 287,
  GL16 332, REAGL 346, GC16 426. Drawing costs 20–110 ms (more when a chapter is laid out).
- The first start after a fresh install scans the library slowly (20 s for 94 books, no cache).

## Open, any device

- Flaky test `layout::hyphen::tests::hyphenates_from_data_files_with_tag_fallbacks`: failed 5 times
  right after rebuilds (`breaks("en", "hyphenation")` → `[]`), never since. Resume by logging the
  `Standard::from_path` error in `Hyphenators::load`.
- Plugin actions in the selection popup (`{selection}`).
- Collections: a design was proposed, not agreed.
- Positions stay keyed by path (user's decision). If ever changed: `book_key` from KOReader's
  partial MD5, v4 migration, enables KOReader progress sync.
- Simplify skips worth doing when touching the code: an `ActionBlock` owning run/sheet/poll
  (Extensions and PluginPage duplicate it); a `Live` trait for polled screens; an app-level "away"
  state; one cached plugin list; a `Caps` struct so `Night` can't drop a capability.
- Wikipedia lookup skipped (ureq+rustls +1.3 MB; plan in the `dict.rs` shortcut comment).
- PDF (parked by the user; resume only if asked): selection still feels unsteady. Known: a tall
  part grows a line's box; the x sort reverses RTL and can move combining marks; the gap-space rule
  hits letter-spaced headings; a press in a tall line can go to a neighbour; highlights saved before
  the reading-order change may not re-anchor; dragging past a short last line jumps up a line.

## Kindle PW5 (parked)

- Boot into Zarathustra (de-Amazon phase A): spec `specs/2026-11-09-boot-to-zarathustra-design.md`,
  plan `plans/2026-11-09-boot-to-zarathustra.md`, not started. Task 0 needs the user (install the
  jailbreak's RescuePack; USB test with the app open). Amazon services inventory:
  `research/2026-11-09-amazon-services-inventory.md`. Then show the "start at power-on" row
  (`plugin_page::BOOT_HOOK`).
- USB: volumd enters drive mode by itself on plug; a white Amazon screen stays after "eject" until
  the cable is pulled. Find what draws it, or defer via `userstoreDeferUnmountRequest`.
- Wi-Fi: a new profile via `lipc-hash-prop com.lab126.wifid createProfile` failed on the password's
  characters; the wpa_cli fallback is untested with '#'/'!'.
- Never tried on the Kindle: PDF search and highlights, device information.
- With the RAM back buffer, MTK may no longer need `wait_before_draw` either: measure gray blocks
  before turning `draws_over_refresh` on there.

Facts: FW 5.17.1, MTK hwtcon, fb 8 bpp rotate=3 (driver), touch `/dev/input/event1`, no
transform. The MTK framebuffer mapping is uncached (reads ~0.3 µs/byte), hence the back buffer.
Paths under `/mnt/us` (library, `zarathustra/state`), fonts `/usr/java/lib/fonts`. Deploy:
`make update` / `make install-ssh` / `make install-usb`. Notes: `notes/kindle-power.md`,
`notes/kindle-wifi.md`.

Never commit device data (SSIDs, passwords, MACs, IPs, serials).
