# Extensions, plugin actions, plugin translations, statistics — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an Extensions screen with background plugin actions (stop, optional timeout, output
sheet), book-menu actions, Fluent translations for plugins, and KOReader-style reading time.

**Architecture:** three tracks. A (stats) and B (plugin Fluent) run as worktree agents; C
(actions + Extensions screen) in the main session, which merges A and B. Plugins stay
separate processes; an action is a detached process group whose exit code a tiny `sh`
wrapper writes to a file, so results survive the app restarting.

**Tech Stack:** Rust 2021, `fluent-bundle` 0.16 + `unic-langid` (new, Apache-2.0/MIT),
`nix` (killpg), existing `Canvas`/`ui.rs` drawing.

**Spec:** `docs/superpowers/specs/2026-11-09-extensions-stats-design.md`

## Global Constraints

- CLAUDE.md: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features` zero
  warnings, `cargo test --workspace` before every commit; no `unwrap()` outside tests;
  `// ponytail:` on shortcuts; comments/commits in English; Conventional Commits with why.
- Every new user-visible string in `i18n.rs` (`Strings`, both `IT` and `EN`).
- Stats: skim < 5 s adds nothing; page time capped at 120 s; pace keeps SKIM 2 s / AWAY 600 s.
- Actions: no timeout unless `timeout_seconds`; Stop = SIGTERM to the group, SIGKILL after 3 s.
- Status and list runs keep the 2 s timeout.

## Review Focus

- An action printing megabytes (or binary bytes) → the line and sheet still render (lossy
  UTF-8, sheet reads ≤ 256 KB). Test in Task C1.
- The app restarting while an action runs → shows Running, then its real result. Test C1.
- A pid file left from a dead run whose pid was reused → not "running" (cmdline check). C1.
- A broken `.ftl` file or a missing message → manifest string, no panic. Test B1.
- Suspend with the book open, then resume and turn → the asleep time is not counted. Test A1.

---

### Task A1: paused page clock and KOReader-style logging (agent A, worktree)

**Files:** Modify `crates/zarathustra/src/pace.rs`, `crates/zarathustra/src/reader.rs`,
`crates/zarathustra/src/bin/app.rs`.

**Interfaces — Produces** (in `pace.rs`):
- `pub const STAT_MIN: Duration = 5 s; pub const STAT_MAX: Duration = 120 s;`
- `Pace::pause(&mut self, now: Instant)`, `Pace::resume(&mut self, now: Instant)` (idempotent;
  the clock only accumulates while running).
- `Pace::leave(&mut self, now: Instant, forward: bool) -> Option<Duration>`: ends the page's
  visit; returns the time to log for stats (`None` under `STAT_MIN`, else `min(t, STAT_MAX)`);
  a speed sample only when `forward` and within SKIM..=AWAY (current rules); restarts the clock
  for the next page.
- `Reader::flush_reading(&mut self)`: logs the current page's time (as `leave(now, false)`,
  pages count 0) — called on close, suspend, exit, USB.
- `Highlights::log_page(secs, pages)` gains the `pages` count (1 forward, 0 otherwise).

- [ ] Tests in `pace.rs`: `paused_time_not_counted` (resume 0 s, pause at 30 s, resume at
  300 s, leave at 310 s → 40 s); `skim_counts_nothing` (4 s → None); `long_page_capped`
  (900 s → 120 s); `backward_leave_logs_but_no_speed_sample` (samples unchanged).
- [ ] Run `cargo test -p zarathustra pace` → fail; implement; pass.
- [ ] Reader: every turn (forward or back, jumps, goto) calls `leave`; overlay state changes
  (`overlay_open()`, extended with `self.hl.popup` open) pause/resume — do it in one place,
  e.g. after each handled event compare `overlay_open()` before/after.
- [ ] app.rs: on `PowerEvent::Suspend`, before exit and on USB → `flush_reading()` + pause;
  on resume → resume; quick panel open/close → pause/resume; leaving the reader → flush.
- [ ] fmt, clippy, test, commit `feat(stats): count reading time like KOReader, paused in menus and sleep`.

### Task B1: Fluent translations for plugins (agent B, worktree)

**Files:** Create `crates/zarathustra/src/plugin_i18n.rs`, `dist/plugins/ssh/locales/it.ftl`,
`dist/plugins/ssh/locales/en.ftl`; Modify `plugin.rs` (Action `id`, env
`ZARATHUSTRA_LANGUAGE`), `plugin_page.rs` and `prefs.rs` (shown names via the API),
`crates/zarathustra/Cargo.toml`, `lib.rs`.

**Interfaces — Produces:**
- `Action { pub id: Option<String>, .. }` and `Action::key(&self) -> String` (id, else the
  executable's file name).
- `Plugin::text(&self, message_id: &str, fallback: &str) -> String` — current language's
  bundle (`crate::i18n::lang()`), else `fallback`. Bundles loaded lazily once per plugin
  (`OnceCell`), from `user_dir()/locales/<code>.ftl`; parse errors logged, partial resource used.
- Convenience: `Plugin::display_name()`, `Plugin::description()`,
  `Plugin::action_label(&Action)`, `Plugin::setting_label(&Setting)`,
  `Plugin::setting_empty(&Setting) -> Option<String>`,
  `Plugin::option_label(&Setting, value: &str) -> String`.
- Message ids exactly as the spec: `display-name`, `description`, `action-<key>`,
  `setting-<id>`, `setting-<id>-empty`, `setting-<id>-option-<value>`.

- [ ] Tests: `uses_the_language_file` (it.ftl → Italian label), `falls_back_to_manifest`
  (no file / missing id / `en` without file), `broken_ftl_falls_back` (garbage line + one valid
  message: the valid one used), `action_key_defaults_to_executable`, `language_env_set`.
  Use `FluentBundle::set_use_isolating(false)` (no Unicode isolation marks on e-ink).
- [ ] SSH `it.ftl` and `en.ftl` covering every string of its manifest.
- [ ] `plugin_page.rs`, `prefs.rs` draw names/labels through the API.
- [ ] fmt, clippy, test, commit `feat(plugins): Fluent translations for plugin strings`.

### Task C1: background action runs (main session)

**Files:** Modify `crates/zarathustra/src/plugin.rs`.

**Interfaces — Produces:**
- `Action { output: Output (Quiet|Show, default Quiet), shown_in: Vec<Place> (Extensions|Book,
  default [Extensions]), timeout_seconds: Option<u64> }`.
- `pub struct LastRun { label, started: i64, duration: Option<Duration>, outcome: Outcome,
  stdout_head: String, stderr_tail: String }`,
  `enum Outcome { Running, Exited(i32), Stopped, TimedOut, SpawnFailed(String) }`.
- `Plugin::run(&self, state_dir, i: usize, book: Option<(&Path, &str)>) -> Result<()>`: one at
  a time per plugin; files in `data_dir()/action.{pid,label,started,stdout,stderr,status}`;
  spawned as `sh -c '"$@"; echo $? > "$STATUS"' sh <exe> <args…>` in its own process group.
- `Plugin::last_run(&self) -> Option<LastRun>`; `Plugin::stop_action(&self)`;
  `Plugin::poll_action(&self) -> bool` (true when a run just ended or was timed out by it).
- `Plugin::output(&self) -> (String, String)`: full stdout/stderr, ≤ 256 KB each, lossy UTF-8.
- Removes `run_action`.

- [ ] Tests (desktop, real `sh` scripts in a temp plugin): `quiet_success`, `failure_keeps_stderr_tail`,
  `stop_kills_group` (script `sleep 30`), `timeout_stops` (`timeout_seconds = 1`),
  `survives_restart` (new `Plugin` value reads Running then the result),
  `reused_pid_is_not_running`, `huge_binary_output_is_lossy_and_capped`, `book_env_set`.
- [ ] Implement; pass; commit `feat(plugins): actions run in the background, with stop and optional timeout`.

### Task C2: Extensions screen, output sheet, settings row (main session)

**Files:** Create `crates/zarathustra/src/extensions.rs`, `crates/zarathustra/src/outsheet.rs`;
Modify `prefs.rs` (one row instead of the list), `plugin_page.rs` (actions via C1, Running/Stop,
last-run line opening the sheet), `i18n.rs`, `bin/app.rs` (500 ms tick while any action runs).

- [ ] Layout per spec §1 (frontend-design skill); laid-out rows shared by render and tap, as
  `plugin_page.rs` does; tests: `cards_page_by_height`, `tap_runs_action_and_shows_running`,
  `failed_line_opens_sheet`.
- [ ] Device check: screenshots of the list, a running action, a failed action's sheet.
- [ ] Commit `feat(ui): Extensions screen with action buttons and output sheet`.

### Task C3: book-menu actions

**Files:** Modify `bookmenu.rs` (plugin action items after the built-ins, Running state),
`home.rs` (passes the book path and saved position text).
- [ ] Test: an action with `shown_in = ["book"]` appears in the menu and gets `ZARATHUSTRA_BOOK`.
- [ ] Commit `feat(plugins): plugin actions in the book menu`.

### Task D: merge, review, deploy

- [ ] Merge A and B; resolve; full checks.
- [ ] `/code-review` + simplify angles on the diff since 7b5105e; apply; commit.
- [ ] `make update`; screenshots; update HANDOFF; commit.
