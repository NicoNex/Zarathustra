# Extensions screen, plugin actions, plugin translations, reading statistics — design (2026-11-09)

Three independent pieces, built in parallel where they touch different files. Builds on the
plugin system (`2026-11-08-plugins-design.md`); where the two disagree, this one wins.

## Why

The user wants plugins to *do* things from the Kindle: "send my highlights to my phone", "sync
with the server", "run this", each a button that runs an executable. When one fails (as SSH
did while we set it up) the full output must be one tap away, so debugging needs no SSH.
Reading statistics today lose time and count time that isn't reading.

Anti-goals: an action freezing the UI; an action's failure hidden; a plugin able to break the
reader (it stays a separate process, as before).

## 1. Extensions screen

**Where.** The settings screen loses its inline plugin list; in its place one row
**"Estensioni" / "Extensions"** (with the number installed) opens the Extensions screen. Back
returns to the settings.

**The Extensions screen** lists every installed plugin as a card, in folder-name order:
- name and description (translated, see §3);
- for a plugin with a service: a status dot + word (Attivo / Spento / Errore) and its on/off
  switch (the same control and behaviour as the plugin page's "Active" row);
- its actions as buttons, generated from the manifest, in manifest order;
- under the buttons, the **last-run line** of that plugin (see Results).
A tap on the card's name area opens the plugin's detail page — today's `PluginPage` (status
lines, service rows, settings, lists such as SSH keys), which keeps its action buttons too.
A plugin whose manifest is broken or too new shows its card with the reason and no buttons.
Phase 1: if the cards don't fit, the screen pages (‹ n / m ›) like the library; no scrolling.

**Look.** Same visual language as the settings and home (ui.rs helpers, rounded cards, 88 px
touch targets, sans labels, INK/PAPER/INK_SOFT). Use the frontend-design skill for the card
layout; e-ink constraints: no animation, black/white/two grays, the running state is shown by
text and an outline change, not motion.

## 2. Plugin actions

**Manifest** (`[[action]]`, additions in bold; `format_version` stays 1, new fields are
optional):
- `menu_label`, `executable`, `arguments` (as before);
- **`output = "quiet" | "show"`** (default `"quiet"`);
- **`shown_in = ["extensions", "book"]`** (default `["extensions"]`; the plugin's detail page
  always shows all actions). `"book"` also puts the action in the book menu (long press on a
  book on the home), with `{book}` / `ZARATHUSTRA_BOOK` = that book's path and
  `ZARATHUSTRA_POSITION` = its saved position text (empty if never opened). Elsewhere these
  are empty. `{selection}` stays unimplemented (selection popup: not now).

**Running.** A tap starts the executable in the background (own process group, as services;
stdin null; stdout and stderr captured to files under the plugin's data dir, so any amount of
output is fine). The UI never waits. **No timeout by default**; an action may set
**`timeout_seconds = N`** (optional) and is stopped like "Stop" when it runs longer, its result
reading "✗ <label>: tempo scaduto dopo N s" (timed out). While it runs:
- its button reads "In corso…" / "Running…" in the extensions screen, plugin page and book
  menu; a second tap on it does nothing;
- a **"Ferma" / "Stop"** button appears beside it: SIGTERM to the process group, SIGKILL after
  3 s (the service stop logic, reused).
One action per plugin at a time is enough (other plugins' actions may run concurrently).
Leaving the screen doesn't stop it; the app exiting doesn't either (it is detached like a
service); on the next start a still-running action is recognised from its pid file, a finished
one shows its stored result. The app checks running actions about every 500 ms while any runs
(not when none does: no idle wake-ups) and redraws what shows them when one ends.

**Results.** Each plugin keeps its **last run** (persisted in its data dir: label, start time,
duration, exit status or signal, stdout, stderr). The last-run line under the buttons:
- success + `quiet`: "✓ <label> — fatto" (Done);
- success + `show`: the first line of stdout (or "✓ … fatto" if empty);
- failure (non-zero exit, killed, spawn error): **always shown**, "✗ <label>: " + last
  non-empty line of stderr (or the exit status). Drawn so it stands out (inverted chip).
- stopped by the user: "■ <label> — fermata".
**Tapping the line opens the output sheet**, full screen: label, when, duration, exit status,
then stdout and stderr (headed, monospace if a mono face is available else sans), paged with
‹ › when long. Back closes it. This is the debug tool.

## 3. Plugin translations

`plugin.toml` may carry `[translations.<lang>]` tables (`it`, `en`; any code the app knows)
overriding user-visible strings: `display_name`, `description`, `actions` (keyed by the
action's untranslated `menu_label`, gettext-style: no collisions, no extra ids) and `settings`
(keyed by setting id, translating its `label`; and `empty_label` for lists via
`settings_empty`). Choice options, status and list output are not translated. Shape:

```toml
display_name = "SSH"
description = "Remote access with keys"

[translations.it]
description = "Accesso remoto con chiavi"
actions = { "Save diagnostics" = "Salva diagnostica" }   # keyed by menu_label
settings = { port = "Porta", authorized_keys = "Chiavi autorizzate" }
```

Lookup: the app's current language table, else the untranslated manifest string. The SSH
plugin ships `it` and `en`. Status lines printed by plugin executables are not translated
(the executable can read `ZARATHUSTRA_LANGUAGE`, a new env var with the app's language code).
All new built-in strings (screen title, Running, Stop, Done, Stopped, Error, output sheet
headings) go in `i18n.rs` for both languages.

## 4. Reading statistics

State of the art (KOReader's statistics plugin, Kobo): time is measured per page shown;
- a page shown less than **5 s** is a skim and adds nothing;
- a page's time is **capped at 120 s** instead of being thrown away (today >600 s pages are
  dropped and <600 s ones count in full);
- the clock **pauses** while the reader isn't reading: any overlay or menu open over the page
  (book menu, typography, contents, bookmarks, search, dictionary card, footnote, highlight
  popup, quick panel), the device asleep, the app on another screen; it resumes when the page
  is back in front;
- the time of the page in front is **flushed** when it stops being in front for good: turning
  (forward *or* back), closing the book, suspend, app exit/USB. Today only forward turns log.
A page counts once per visit; turning back logs the time too. `pages` in the stats keeps
counting forward turns only (pages read). The reading-speed estimate (`pace.rs`) keeps its own
rules (skims and away pages are not speed samples) but uses the same paused clock.
Constants live in one place (`stats.rs` or `pace.rs`), the cap as a `// ponytail:`-free plain
constant (KOReader makes it a setting; not now).

## Builder's calls (settled after the builder check)

- The paused clock lives in `pace.rs` (`Pace` owns "page in front since", with pause/resume);
  stats and the speed estimate both read it. The reader drives it from its one choke point,
  `Reader::overlay_open()` (extended with the highlight popup), plus app-level hooks: suspend,
  resume, quick panel, leaving the reader. Stats skim threshold 5 s and pace's SKIM 2 s stay
  different on purpose.
- Times are wall time; on resume from suspend the app re-checks running actions at once.
  `timeout_seconds` counts wall time (a suspended process may get killed on wake: fine).
- A running action = its pid file names a live process whose `/proc/<pid>/cmdline` is the
  action's executable (the service check, reused). Captured output: one stdout/stderr file
  pair per plugin, overwritten by each run, no cap; the last-run line reads the head of stdout
  and the tail of stderr; the sheet reads at most the first 256 KB of each.
- The synchronous `Plugin::run_action` (2 s timeout, toast) is replaced; `plugin_page.rs` and
  its test move to the background runner. The 2 s timeout stays for status and list runs.
- Card heights vary with the number of action buttons; buttons wrap two per row; the screen
  pages by accumulated height; a card taller than a page is clipped (`// ponytail:`).
- Book-menu actions show for every book (position empty if never opened). Arguments are
  substituted per argv element, never through a shell (as today), so any characters are safe.
- The Running/Stop/Done/Error strings, the "Active" row and every other built-in label are
  app-side i18n.

## Parallel split

- Agent A (worktree): §4 statistics — `pace.rs`, `reader.rs` timing hooks, `bin/app.rs`
  suspend/exit flush, `stats.rs` constants, tests.
- Agent B (worktree): §3 translations — `plugin.rs` manifest parsing + lookup, SSH
  `plugin.toml`, `ZARATHUSTRA_LANGUAGE`, tests. Touches `plugin.rs` only in the manifest and
  env parts.
- Main session: §1 + §2 — new `extensions.rs` screen, `plugin.rs` background runs, `prefs.rs`
  row, `bookmenu.rs` actions, `plugin_page.rs`, output sheet. Merges A and B.
Then code-review + simplify on the whole diff, commit, `make update`, verify on the device
(screenshots of the extensions screen, a running action, a failed action's sheet).

## Not now

Actions in the selection popup (`{selection}`), PDF work, Fluent, scrolling, a plugin store,
an action queue or concurrency limits, action progress reporting.
