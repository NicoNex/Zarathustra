# Boot to Zarathustra Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Kindle powers on into Zarathustra with the unneeded Amazon services stopped, falls back to the stock Amazon boot (UI + KUAL) on failure, and starts SSH at every boot.

**Architecture:** One upstart job on the rootfs (`/etc/upstart/zarathustra.conf`, `start on started lab126_gui`) runs `boot.sh` from the KUAL extension folder once per boot. `boot.sh` decides: Zarathustra boot (stop `lab126_gui` as `run.sh` does today, stop the Amazon jobs, start plugin boot services, run the app in a restart loop) or stock boot (leave Amazon alone, start SSH, exit). The app learns it is a boot session from `ZARATHUSTRA_BOOT=1` (hides "Exit") and clears the failure counter once its library is on screen. The app loop (update restart, USB hand-over) moves from `run.sh` into a shared `lib.sh` both scripts source.

**Tech Stack:** upstart 0.6.6, BusyBox `/bin/sh`, Rust (crates/zarathustra), LIPC tools on the device.

**Spec:** `docs/superpowers/specs/2026-11-09-boot-to-zarathustra-design.md` (facts: `docs/superpowers/research/2026-11-09-amazon-services-inventory.md`).

## Global Constraints

- Upstart 0.6.6 syntax only (no `.override`, no `manual`). BusyBox `/bin/sh` (no bash-isms).
- The rootfs is read-only except during install/uninstall; the only file added there is `/etc/upstart/zarathustra.conf`. No Amazon or jailbreak file is modified.
- Stopped at a Zarathustra boot: `todo tmd archive rcm dynconfig phd perfd stackdumpd minerva_service minervad demd ace_messaging_service ace_eventmgr acedropboxd acsbtfd btmanagerd playermgr_limit playermgr`.
- Kept: upstart, udevd, dbus, lipcd, powerd, deviced, volumd, fsp, wifim, wmt, wifis, wifid, cmd, udhcpc, zram, tee-supplicant, mdpd, syslog, cron, battstressd.
- Fallback to stock boot when `/mnt/us/zarathustra/no-autostart` exists, or the failure counter is ≥ 2, or `/mnt/us/zarathustra/bin/zarathustra` is missing.
- SSH starts at every boot, in both branches.
- No device data (SSIDs, serials, MACs, IPs) in the repo. Commits never dated Mon–Fri 09:00–18:00 (set `GIT_AUTHOR_DATE`/`GIT_COMMITTER_DATE` outside those hours, after the parent's date).
- Before every commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features` (zero warnings), `cargo test --workspace`; for shell, `sh -n` on every script.

## Review Focus

1. A KUAL launch after a fallback boot (Amazon UI → KUAL → Zarathustra → Exit → `start lab126_gui`) must not be taken over by the boot job: it acts once per boot only (Task 3 test `once per boot`).
2. USB plugged during a boot session: the storage reaches the computer (brief white Amazon screen), and Zarathustra comes back after the cable is out (Task 5, step USB).
3. `make update` during a boot session restarts into the new binary without the Amazon UI (Task 2 keeps the update flag in `lib.sh`; Task 5 checks it).
4. A crash loop after the library is shown must not spin forever: 3 exits within 60 s stop the loop and print a message with `eips` (Task 3 test `crash loop`).
5. Restart and Power off from the power menu in a boot session still work, and Exit is not offered (Task 1 test; Task 5 check).

---

### Task 0: Safety prerequisites on the device (no code)

**Files:** none (results go into `docs/superpowers/research/2026-11-09-amazon-services-inventory.md`, new section "Phase A prerequisites").

- [ ] **Step 1: RescuePack.** Ask the user to install the jailbreak's RescuePack (the standard MKK/RP package for their jailbreak). Verify over SSH: `ls -la /var/local/rp/debrick /mnt/us/rp` → both present. Do not continue without it.
- [ ] **Step 2: USB with the database open.** With Zarathustra running from KUAL, the user plugs the cable: the storage must appear on the computer and Zarathustra must come back after unplugging. Check `/tmp/zarathustra.log` for `exit status 42` and a restart. If the storage does not appear, stop here and report: the boot design depends on it.
- [ ] **Step 3: OTA block.** Over SSH: `ls /usr/bin/otaupd* /usr/bin/otav3*` → only `*.bck`; `grep -n otacheck /etc/crontab/root` → commented; `ls /mnt/us/update*.bin 2>/dev/null` → none. Record the three results.
- [ ] **Step 4: Commit** the research section.

### Task 1: The app in a boot session

**Files:**
- Modify: `crates/zarathustra/src/lib.rs` (two items below)
- Modify: `crates/zarathustra/src/home.rs` (power menu rows from a list; around `POWER_MENU`, `power_menu`, `Overlay::Power` tap)
- Modify: `crates/zarathustra/src/bin/app.rs` (clear the counter after the first full draw; currently `eprintln!("home ready …")` at ~336, first refresh after it)
- Modify: `crates/zarathustra/src/plugin_page.rs:30` (`BOOT_HOOK` → `true`)

**Interfaces:**
- Produces: `pub const BOOT_ATTEMPTS: &str = "/var/local/zarathustra/boot-attempts";` and `pub fn boot_session() -> bool` (true when env `ZARATHUSTRA_BOOT` is `1`), in `lib.rs`.
- Produces: the app deletes `BOOT_ATTEMPTS` (ignoring "not found") right after the first screen has been drawn and refreshed, in every launch mode (KUAL too: that is how a fallback is cleared).

- [ ] **Step 1: Failing test** in `home.rs` tests: `fn the_power_menu_has_no_exit_in_a_boot_session()` — build the row list with `power_rows(true)` and assert it is `[Action::Sleep, Action::Reboot, Action::PowerOff]` (labels standby/restart/power off), and `power_rows(false)` has `Action::Quit` second (4 rows).
- [ ] **Step 2:** `cargo test -p zarathustra --lib -- power_menu` → FAIL (no `power_rows`).
- [ ] **Step 3: Implement** `fn power_rows(boot: bool) -> Vec<(&'static str, Action)>` in `home.rs`; `POWER_MENU`'s height follows the row count (make it a function of `n`), drawing and tapping both use `power_rows(crate::boot_session())`. `BOOT_HOOK = true` in `plugin_page.rs` (fix the `the_ssh_page_shows_its_rows…` assertion, it already compares with `BOOT_HOOK`).
- [ ] **Step 4:** app.rs: after the first draw, `std::fs::remove_file(zarathustra::BOOT_ATTEMPTS)` with `NotFound` ignored and any other error logged.
- [ ] **Step 5:** fmt, clippy, `cargo test --workspace` → pass.
- [ ] **Step 6: Commit** `feat(app): boot session: no Exit, failure counter cleared once drawn`.

### Task 2: Shared app loop (`lib.sh`), `run.sh` on top of it

**Files:**
- Create: `dist/kual/zarathustra/lib.sh`
- Modify: `dist/kual/zarathustra/run.sh`

**Interfaces:**
- Produces (sourced, POSIX sh functions):
  - `z_unload_savers` / `z_reload_savers` — the blanket screensaver unload/reload now in `run.sh` (sets `Z_SAVERS`).
  - `z_hide_amazon_ui` — `stop lab126_gui` with TERM ignored around it, the 1.25 s wait, savers unloaded, `lipc-send-event com.lab126.kaf bootSplashCleanup` (exactly today's sequence).
  - `z_run_app NAME [ARGS…]` — today's `while :` loop: copy the binary to `/tmp/NAME`, run with log `/tmp/NAME.log`, `/tmp/zarathustra-update` → run again, status 42 with a UDC state file → the USB hand-over (`start lab126_gui`, wait for the cable to go and the binary to be back, `z_hide_amazon_ui`), any other status → return it.
- `run.sh` keeps its self-copy to `/tmp`, also copies `lib.sh` to `/tmp/zarathustra-lib.sh`, sources it, and keeps `restore()` (reload savers, `preventScreenSaver 0`, keep the log, `start lab126_gui`). Behaviour from KUAL is unchanged.

- [ ] **Step 1:** Write `lib.sh` and rewrite `run.sh` on it. `sh -n` both → no output.
- [ ] **Step 2: Device check** (`make update`, then from KUAL): Open Zarathustra; `make update` again → restarts into the new binary without the Amazon UI; USB in/out → storage on the computer, then back in Zarathustra; Exit → Amazon UI back.
- [ ] **Step 3: Commit** `refactor(kual): the app loop in lib.sh, shared with the boot job`.

### Task 3: The boot job and its script

**Files:**
- Create: `dist/kual/zarathustra/zarathustra.conf` (the upstart job, installed by Task 4)
- Create: `dist/kual/zarathustra/boot.sh`
- Create: `scripts/test-boot.sh` (runs `boot.sh`'s decision with stubbed commands)

**Interfaces:**
- `zarathustra.conf`: `start on started lab126_gui`, no `respawn`; `script` runs `exec /bin/sh /mnt/us/extensions/zarathustra/boot.sh` only if that file exists (else `exit 0`). Comment header: what it is, how to remove it (Task 4's uninstall).
- `boot.sh` honours these overridable paths (env, for the test): `Z_US=/mnt/us`, `Z_VAR=/var/local/zarathustra`, `Z_TMP=/tmp`. Log: `$Z_TMP/zarathustra-boot.log`, one line per decision.
- `boot.sh` functions: `z_decide` prints `zarathustra` or `stock <reason>` (reasons: `once`, `no-autostart`, `attempts`, `no-binary`, `dry-run`); the main part acts on it.
- Order in `boot.sh`: copy itself and `lib.sh` to `$Z_TMP` and re-exec from there (as `run.sh`); `z_decide`; touch `$Z_TMP/zarathustra-boot-done` (once per boot) whatever the decision.
  - `stock …`: start SSH — `"$Z_US/zarathustra/bin/zarathustra" --start-services --recovery`, and if that exits non-zero, run the SSH plugin's runtime copy `$Z_VAR/plugins/ssh/run` detached with `ZARATHUSTRA_DATA_DIR=$Z_VAR/data/ssh ZARATHUSTRA_USER_DIR=$Z_US/zarathustra/plugins/ssh` (port 22) — then exit 0. Amazon goes on booting.
  - `dry-run` (file `$Z_US/zarathustra/boot-dry-run` exists): log what a Zarathustra boot would do, start SSH as in `stock`, exit 0.
  - `zarathustra`: increment `$Z_VAR/boot-attempts`; `z_hide_amazon_ui`; `stop` each job of the Global Constraints list (ignore errors), logging which were running; `zarathustra --start-services --recovery`; then `ZARATHUSTRA_BOOT=1 z_run_app zarathustra` in a loop: an exit restarts it, except 3 exits within 60 s → `eips` prints "Zarathustra stopped: see /tmp/zarathustra.log; hold the power button to restart" and the script waits forever (no Amazon UI).
- Fallback decision order: `$Z_TMP/zarathustra-boot-done` → `once`; `no-autostart` → `no-autostart`; attempts ≥ 2 → `attempts`; binary missing → `no-binary`; `boot-dry-run` → `dry-run`; else `zarathustra`.

- [ ] **Step 1: Failing test** `scripts/test-boot.sh`: temp dirs for `Z_US/Z_VAR/Z_TMP`, a `PATH` dir with stub `stop start lipc-set-prop lipc-get-prop lipc-send-event eips usleep` that append their argv to a file; source `boot.sh` with `Z_TEST=1` (only defines functions) and assert `z_decide` output for each case: fresh → `zarathustra`; `zarathustra-boot-done` present → `stock once`; `no-autostart` → `stock no-autostart`; `boot-attempts` = 2 → `stock attempts`; binary absent → `stock no-binary`; `boot-dry-run` → `stock dry-run`. Plus `crash loop`: a stub app that exits at once → after 3 runs `eips` was called and the loop stopped (bounded by a timeout in the test). Exit non-zero on any mismatch.
- [ ] **Step 2:** `sh scripts/test-boot.sh` → FAIL (no `boot.sh`).
- [ ] **Step 3:** Write `boot.sh` and `zarathustra.conf`. `sh -n` both.
- [ ] **Step 4:** `sh scripts/test-boot.sh` → `ok` and exit 0.
- [ ] **Step 5: Commit** `feat(boot): the boot job: Zarathustra or the stock boot, SSH always`.

### Task 4: Installer

**Files:**
- Create: `dist/kual/zarathustra/boot-install.sh` (`install`, `uninstall`, `status`)
- Modify: `dist/kual/zarathustra/menu.json` (items "Boot into Zarathustra: install" / "…: remove" running it)
- Modify: `Makefile` (`boot-install`, `boot-uninstall`, `boot-status` over SSH)

**Interfaces:**
- `install`: refuse unless `/var/local/rp/debrick` exists (RescuePack) and `/etc/upstart/zarathustra.conf` differs from ours; `mount -o remount,rw /`; copy the job; `sync`; `mount -o remount,ro /` (always, also on error, via `trap`); print the installed file's checksum. `uninstall`: the same remount around `rm -f /etc/upstart/zarathustra.conf`. `status`: installed or not, `no-autostart`/`boot-dry-run`/`boot-attempts` state, last 10 lines of `/tmp/zarathustra-boot.log`.
- Make targets: `boot-install: update` then `$(SSH) 'sh /mnt/us/extensions/zarathustra/boot-install.sh install'`; likewise the others.

- [ ] **Step 1:** Write the script; `sh -n`.
- [ ] **Step 2: Device, dry run:** `touch /mnt/us/zarathustra/boot-dry-run`, `make boot-install`, `make boot-status` → installed. Reboot 3 times (user, or `reboot` over SSH): each boot shows the normal Amazon UI with KUAL, `make boot-status` shows `stock dry-run` logged once per boot, SSH answers after each boot.
- [ ] **Step 3: Commit** `feat(boot): installer for the boot job (KUAL and make)`.

### Task 5: Real boot, tested on the device

**Files:** results recorded in `docs/superpowers/HANDOFF.md`.

- [ ] **Step 1:** `rm /mnt/us/zarathustra/boot-dry-run`; reboot → Zarathustra's library, no Amazon home; `make boot-status` → `zarathustra`; `initctl list` → none of the stopped jobs running; `netstat -ulnp` → no UDP 46776; SSH answers. Repeat 3 boots: never the customer-service screen.
- [ ] **Step 2:** Power menu: no Exit; Restart → boots into Zarathustra again; Standby → wakes into Zarathustra.
- [ ] **Step 3: USB** in a boot session: storage on the computer (white Amazon screen is fine), unplug → Zarathustra back.
- [ ] **Step 4:** `make update` in a boot session → new binary, no Amazon UI.
- [ ] **Step 5: Fallbacks:** `touch …/no-autostart`, reboot → Amazon UI + KUAL, SSH answers; KUAL → Open Zarathustra → Exit → Amazon UI stays (boot job does not take over); remove the file, reboot → Zarathustra. Then `echo 2 > /var/local/zarathustra/boot-attempts`, reboot → stock; open Zarathustra from KUAL (clears it), reboot → Zarathustra.
- [ ] **Step 6:** Wi-Fi join/toggle, clock, battery %, suspend/resume, a night of standby (battery % before/after).
- [ ] **Step 7: Commit** HANDOFF notes `docs: boot into Zarathustra, tested on the device`.

### Task 6: More services off, one at a time

**Files:** `dist/kual/zarathustra/boot.sh` (the stop list), research doc.

- [ ] **Step 1:** For `dpmd`, then `appmgrd`, then `x`: `stop <job>` over SSH in a boot session, then suspend/resume, power button, USB in/out, Wi-Fi toggle, low-battery screen if reachable. Record pass/fail per job.
- [ ] **Step 2:** Add the jobs that passed to the stop list (`x` last; if `x` stops, check the USB hand-over still shows something and works).
- [ ] **Step 3:** Reboot, re-run Task 5 steps 1, 3, 6 quickly.
- [ ] **Step 4: Commit** `feat(boot): also stop <jobs>`, with the results in the research doc.

### Task 7: Docs

**Files:** `README.md` (install the boot job, how to get the Amazon UI back: `no-autostart`, KUAL item, `make boot-uninstall`), `docs/plugins.md` ("start at power-on" is real now; SSH starts at every boot), `docs/superpowers/HANDOFF.md`.

- [ ] **Step 1:** Write the sections.
- [ ] **Step 2: Commit** `docs: booting into Zarathustra`.
