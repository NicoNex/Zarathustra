# Boot to Zarathustra, Amazon services off (de-Amazon phase A)

Date: 2026-11-09. Device: Kindle Paperwhite 5, FW 5.17.1, WinterBreak + MKK jailbreak.
Facts behind this design: `docs/superpowers/research/2026-11-09-amazon-services-inventory.md`
(services, boot chain, jailbreak hooks) and a read-only check on the device the same day.

## Intent

The user wants the Kindle to be theirs: they control it, Amazon cannot change or break it
remotely, and it does not depend on the jailbreak community for every firmware twist. Phase A
is the first step on the Amazon system as it is: the Kindle powers on straight into
Zarathustra, and every Amazon service Zarathustra does not need stays off. Phase B (later,
separate design) replaces the Amazon userland with Alpine on the Amazon kernel.

## What the user asked for

- Power on with the usual button and land in Zarathustra. Amazon's boot splash may show for a
  moment while things load; anything else is fine too if keeping the splash would cost Amazon
  services that are otherwise useless.
- If anything goes wrong, the Kindle starts the normal Amazon UI with KUAL, jailbreak intact,
  so Zarathustra can be opened by hand.
- No on/off switch for this in Zarathustra's settings: booting into Zarathustra is how the
  device works from now on, not an option.
- SSH (the plugin) always starts at boot, as the way back in.
- Every Amazon service Zarathustra does not need is off. Nothing of Amazon is to be kept for
  use: the user wants none of it (store, sync, Send to Kindle, ads, metrics, remote commands).
- In this phase the services are switched off, not deleted (user's agreement with the
  reasoning below).

## Goals

1. Power on → Zarathustra's library, without KUAL, every time.
2. While Zarathustra runs at boot: Amazon's UI (framework, `lab126_gui`) never shows, and the
   unneeded Amazon services are stopped.
3. Recovery always possible: a failure falls back to the stock Amazon boot (UI + KUAL), and SSH
   is up at every boot, Zarathustra's or the fallback.
5. Amazon OTA updates stay blocked in both boots. The jailbreak's block (`otaupd`, `otav3`
   renamed `*.bck` on the rootfs, cron `otacheck` commented out) already holds for a stock
   boot; the plan verifies it on the device and records how, without changing Amazon files.
4. Nothing Amazon or jailbreak-owned is modified. The rootfs gains one file of ours.

## Non-goals (this phase)

- Deleting Amazon services or files (phase B removes the whole Amazon userland at once).
- Replacing `/sbin/init` or upstart (phase B: a broken init has no fallback on this system).
- Replacing what Amazon services still do for us: power management (powerd), Wi-Fi (wifid,
  cmd, wpa_supplicant), USB drive mode (volumd, fsp), LIPC (dbus, lipcd). They stay.
- The white Amazon USB screen (blanket's `usb` module): separate item, though stopping `x` may
  remove it (see "To test").

## Anti-goals (a failure even if it boots)

- A Kindle that does not boot, boot-loops, or stops on Amazon's "contact customer service"
  screen.
- A lost jailbreak (MKK, gandalf, developer keys) or a lost way back in.
- Changes to Amazon's or the jailbreak's own files on the rootfs.
- Amazon's remote command channel (`todo`) or "phone home" (`phd`, UDP 46776) left running
  while Zarathustra is in front.

## Why "switched off" and not deleted, and why one upstart job

- The rootfs is ext3, read-only, 94 % full. Deleting means writing it repeatedly; a mistake
  there stops the boot. Stopping a service is undone by a reboot.
- The emergency path is the stock Amazon boot, which needs nearly every Amazon job, KUAL
  included. Services that stay installed make "emergency" simply "the factory boot".
- Upstart is 0.6.6: no `.override` files, no `manual` stanza. A job cannot be kept from
  starting without editing its `.conf`. And the services we need (powerd, volumd, wifid, cmd…)
  and the ones to switch off (todo, tmd, phd…) all start on `started lab126`. So: Amazon's
  jobs start as today, and our job stops the unneeded ones right after. It is less clean than
  never starting them (they live for a few seconds), and it is what keeps the fallback trivial.
- The jailbreak's own hook (`/etc/upstart/bridge.conf`) only runs at `framework_ready`, which
  never comes once the framework stays off. So a hook of ours is needed anyway, and it makes us
  independent of the jailbreak's.
- Replacing the init system now was considered and rejected by the user after discussion: a
  broken `/sbin/init` has no fallback (serial console only), and it would mean re-doing ~120
  intertwined Amazon jobs inside a system phase B throws away.

## The design

### One upstart job of ours

`/etc/upstart/zarathustra.conf`, added once by an installer (remount `/` rw, write the file,
remount ro). It is the only change to the rootfs. Uninstall = delete it the same way.

It starts once the services Zarathustra needs are up (power, Wi-Fi, USB, LIPC: at least
`powerd`, `volumd`, `wifid`, `lipcd`, and the userstore mounted) and decides between:

- **Zarathustra boot (normal):** keep Amazon's UI from showing, stop the unneeded services,
  start the plugins' boot services (SSH always), run Zarathustra.
- **Stock boot (fallback):** leave Amazon's boot alone (UI and KUAL as today), and still start
  SSH: the user wants SSH at every boot as the way back in. Since the fallback may be there
  because Zarathustra's binary is what fails, SSH must not depend on it here: builder's call
  how (e.g. the job runs the SSH plugin's runtime copy directly in the shell).

### When it falls back to the stock boot

- `/mnt/us/zarathustra/no-autostart` exists (the user creates it over USB, SSH or from a
  computer; it is on the userstore so it can always be reached). Builder's call: the exact name.
- Zarathustra failed to reach its library on the last 2 boots in a row: a crash, or a hang the
  user ended by force-restarting. Counted by the job (attempt recorded before running the app)
  and cleared by the app once the library is shown. After a fallback boot, the next normal boot
  tries Zarathustra again only once the count is cleared: builder's call how (e.g. the count is
  reset when the stock boot completes, or when Zarathustra is started by hand from KUAL and
  reaches its library).
- Builder's call: whether the job also falls back when the app binary is missing or not
  executable (it should: that is a certain failure).

### Keeping Amazon's UI from showing

The research found `framework.conf` honours `/mnt/us/DONT_START_FRAMEWORK` (the framework
stops itself) and that `lab126_gui` counts failed starts (3 restarts → reboot, 2 reboots →
customer-service screen and halt). The mechanism is the builder's call, under one hard rule:
**whatever is used must be proven on the device not to count as a failed boot**, across at
least 3 consecutive boots, before the job is enabled for real. Candidates: the
`DONT_START_FRAMEWORK` flag (set only for a Zarathustra boot, removed for a fallback one),
stopping `lab126_gui` as `run.sh` does today, or stopping it as it starts.

Builder's call: whether `x` (Xorg, awesome, blanket) is stopped in the boot or kept, see "To
test". Amazon's splash staying on screen until Zarathustra draws is acceptable.

### Services stopped at a Zarathustra boot

From the research's list (b), stopped by the job once they are up:
`todo tmd archive rcm dynconfig phd perfd stackdumpd minerva_service minervad demd
ace_messaging_service ace_eventmgr acedropboxd acsbtfd btmanagerd playermgr_limit playermgr`.

Kept: upstart, udevd, dbus, lipcd, powerd, deviced, volumd, fsp, wifim, wmt, wifis
(wpa_supplicant), wifid, cmd, udhcpc, zram, tee-supplicant, mdpd, syslog, cron, battstressd
(may protect the battery: kept until its role is known).

To test one at a time before adding to the list: `dpmd`, `appmgrd`, `x` (see below).

### When Zarathustra ends in a boot session

- "Exit" disappears from the power menu when Zarathustra was started at boot (user's choice);
  it stays when started from KUAL, where it brings the Amazon UI back as today.
- If the app ends anyway after reaching its library (a crash), the job starts it again; the
  Amazon UI does not come back for the rest of the session. A crash before the library counts
  toward the 2-boot fallback, as above. Builder's call: a limit on quick restarts within one
  session (e.g. 3 crashes in a minute → stop retrying and leave the screen with a message),
  so a crash loop does not spin forever.
- How the app knows it was started at boot (to hide "Exit"): builder's call (an argument or
  an environment variable set by the job).

### USB in a boot session

A brief Amazon UI with its white USB screen while a computer has the storage is fine (user's
choice), so the existing USB-exit loop (status 42 → `start lab126_gui`, wait for the cable to
go, stop it again) is reused. Constraint: whatever keeps the UI from showing at boot (e.g.
`DONT_START_FRAMEWORK`) must not stop that hand-over: the plan tests USB in/out in a boot
session, files reachable from the computer, and back to Zarathustra after.

### What Zarathustra itself needs at boot

Everything `run.sh` does today around the app, done by the job for a boot start: unload
blanket's screensaver modules, send `com.lab126.kaf bootSplashCleanup`, run the binary from
`/tmp` with the log in `/tmp`, restart into an update (`/tmp/zarathustra-update`), and the
USB-exit loop (status 42). Builder's call: share this with `run.sh` (one script both use)
rather than copy it. `run.sh` from KUAL keeps working as today for the fallback case.

The plugins' boot services: `zarathustra --start-services --recovery` (exists) or its
equivalent, with SSH started at every boot even if its switch is off in the plugin page.
Builder's call how the "always" is expressed (the manifest already has `start_in_recovery`).
The "start at power-on" row in the plugin page (`plugin_page::BOOT_HOOK`) shows once the hook
exists.

### Safety before the first boot change

1. Install the jailbreak's RescuePack (debrick); the research found none installed, so
   `bridge` has nothing to restore from.
2. Confirm USB drive mode still works with Zarathustra's SQLite database open on the userstore
   (open question in the research); fix it if not, since USB is how the user reaches
   `no-autostart` without SSH.
3. Dry run: the job installed but only logging what it would do, for a few boots.
4. Then the real job, with the fallback tested on the device: `no-autostart` honoured; 2 forced
   failures → stock boot with KUAL; a normal boot after clearing → Zarathustra again.

## To test on the device (the builder plans these as steps)

- The UI-blocking mechanism does not trip `lab126_gui`'s failed-boot counting (3+ boots).
- Stopping `x`: suspend/resume, power button, USB in/out, low battery still work. If they do,
  stop it too (saves memory and CPU, and likely removes the white USB screen).
- `dpmd`, `appmgrd`: stopped one at a time, then the same checks.
- With the list (b) stopped: Wi-Fi join/toggle, clock, battery %, a night of standby drain.
- The fallback boot gives the full Amazon UI and KUAL, and the jailbreak's `bridge` runs.

## Constraints

- Upstart 0.6.6 syntax only. Busybox `/bin/sh`.
- The rootfs stays read-only except during install/uninstall; 28 MiB free.
- No device data (SSIDs, serials, MACs, IPs) in the repo.
- Commits never dated Monday–Friday 09:00–18:00 (user's rule).

## Left to the builder

The job's exact start condition and file layout, the failure counter's storage (`/var/local`
is fine), the installer's form (a script run over SSH or a KUAL entry, or both), how the job
and `run.sh` share code, and the order of the tests above. Calls made in this document by the
designer are marked "builder's call".
