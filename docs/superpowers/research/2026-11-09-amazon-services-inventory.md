# Amazon services inventory (PW5, FW 5.17.1, MTK) — 2026-11-09

Read-only snapshot taken over SSH while Zarathustra was running with `lab126_gui` stopped
(uptime 2 d 14 h). Purpose: decide what to keep and what to switch off for de-Amazon phase A,
and what Zarathustra must replace in phase B (Alpine). No device identifiers recorded here.

## Headline numbers

| | |
|---|---|
| RAM | 474 MiB total, ~347 MiB available (MemAvailable), 124 MiB used without buffers/cache |
| Swap | zram 128 MiB, 27 MiB used (`zram_setup` job) |
| Userland RSS (sum, shared libs counted repeatedly) | ~162 MiB; Zarathustra itself 15 MiB |
| CPU | 97 % idle; top CPU over 2.6 days: tmd 183 s, syslog-ng 144 s, fsp 94 s, battstressd 26 s, rcm 25 s, Xorg 25 s, powerd 24 s, blanket 21 s, todo 15 s |
| rootfs `/` | ext3, mounted **ro**, 94 % full (28 MiB free) |
| `/var/local` | ext3 rw, 196 MiB free |
| `/mnt/us` | FUSE `fsp` over `/mnt/base-us` (vfat on `/dev/loop/0`), 5.2 GiB free |
| `/var` | tmpfs 64 MiB (logs: `/var/log` 5.6 MiB; `/var/local/log` 10 MiB on flash) |

## 1. Running processes / jobs

RSS in KiB, CPU = cumulative seconds since boot. "Wrapper" = the upstart `/bin/sh` script is
alive but its daemon has **no child process** (the `su framework -c exec …` daemon is gone; likely
exited when the framework stopped). They cost ~1 MiB each and do nothing.

### Base system

| Job / process | RSS | CPU | What it does | Depends on it |
|---|---|---|---|---|
| `init.exe` (upstart) | 2588 | 10 | init | everything |
| `udevd` (job `system`) | 1236 | 2 | device nodes, hotplug | everything |
| `syslog-ng` | 2208 | 144 | syslog; UDP 127.0.0.1:514 | logs only |
| `crond` | 1696 | 0.5 | Amazon cron: `tinyrot` (log rotate), `loginfo` ×6 hourly, `tmpfs_scan`, DEM heartbeat (metrics) | nothing of ours |
| `dbus-daemon` (pid shown by initctl) | — | — | system bus | `lipcd` stops on `stopping dbus` |
| `lipc-daemon` (`lipcd`) | 2912 | 0.8 | LIPC broker (all `lipc-*` calls) | Zarathustra (powerd, wifid, volumd, blanket props), run.sh |
| `tee-supplicant` | 384 | 0 | OP-TEE client (secure world storage/keys) | unknown, probably DRM/keys |
| `mdpd` | 1636 | 0 | MediaTek MDP (display/image pipeline) daemon | probably hwtcon |
| `zram_setup` | 904 | 0 | wrapper holding `sleep infinity` after enabling zram | swap |
| `fsp` | 1280 | 94 | FUSE layer exposing the vfat userstore as `/mnt/us` | all of `/mnt/us` |
| `console` | — | — | getty on UART | debugging (serial) |
| `sshd` (Amazon) | — | — | job "running" but `/usr/local/sbin/dropbear` does not exist: dead | nothing |

### Power, storage, network

| Job / process | RSS | CPU | What it does | Depends on it |
|---|---|---|---|---|
| `powerd -f` | 6052 | 24 | suspend/resume, screensaver state, power button, idle timer, battery props | Zarathustra (events, `touchScreenSaverTimeout`, battery) |
| `deviced -f` | 3528 | 0.4 | HAL events (power button, hall sensor, shutdown) for powerd/volumd | powerd |
| `volumd` | 4536 | 1.3 | userstore mount/unmount, USB drive mode (`driveModeState`, `userstoreDeferUnmountRequest`) | USB mass storage |
| `cmd -f` | 4312 | 6.7 | connection manager (online state, wireless enable, time auto-update) | `tmd`, `phd` start on `started cmd`; wifid |
| `wifid -f` | 5292 | 10 | Wi-Fi profiles, scan, join (LIPC API) | Zarathustra Wi-Fi screen |
| `wifim` (task) | — | — | loads the Wi-Fi driver | wifi |
| `wifis` → `wpa_supplicant` | 3388 | 5 | WPA supplicant on wlan0 | wifi |
| `wmt` → `wmt_service` | 2460 | 0.4 | MTK combo chip (Wi-Fi/BT) firmware/control service | wifi and BT |
| `udhcpc` | 80 | 0 | DHCP client (spawned by wifid/cmd) | IP address |
| `battstressd` | 3736 | 26 | battery stress/health logic (possibly charge limiting at high temperature/voltage) | unknown — see (c) |

### X / screen

| Job / process | RSS | CPU | What it does |
|---|---|---|---|
| `x` → `lxinit` | 2660 | 0.1 | starts Xorg, awesome, blanket |
| `Xorg` | 8272 | 25 | X server on the framebuffer |
| `awesome` | 9480 | 7 | window manager (Amazon "winmgr" LIPC) |
| `blanket` | 8052 | 21 | full-screen modules: splash, screensaver/ad_screensaver, langpicker, blankwindow, **usb**, ota, shutdown, maru. Loaded now: `langpicker blankwindow usb` |

Processes holding the display besides Zarathustra: Xorg, awesome, blanket. The white
"USB drive mode" screen is blanket's **`usb`** module (still loaded; run.sh only unloads the
screensavers) — this answers the open USB-screen question in HANDOFF.

### Amazon services (cloud, metrics, store, media)

| Job / process | RSS | CPU | What it does (best knowledge) |
|---|---|---|---|
| `todo` | 7312 | 15 | server-driven to-do queue: downloads, deliveries, sync actions, remote commands |
| `tmd -f` | 8176 | 183 | transfer manager (HTTP downloads/uploads for todo/store/sync); highest CPU user |
| `archive` | 7248 | 1.6 | archived/cloud items (Amazon library) |
| `rcm` | 4220 | 25 | refresh-cache manager (cloud/content cache refresh) |
| `dynconfig` | 2184 | 0 | Amazon remote configuration |
| `phd` (as `framework`) | — | — | "phone home" daemon; **listens on UDP 0.0.0.0:46776** |
| `perfd` | 1856 | 0 | performance metrics |
| `stackdumpd` | 3800 | 0 | crash stack collector; TCP 127.0.0.1:55555 |
| `dpmd` | 3732 | 6 | device policy manager (parental controls / store / browser policy flags) |
| `appmgrd` | 5600 | 12 | Amazon application manager (framework apps, app history) |
| `playermgr` (+ `logger`) | 3840+1596 | 0.3 | audio/TTS/Audible player |
| `playermgr_limit` | 912 | 0 | loop moving playermgr into a memory cgroup |
| `btmanagerd` | — | — | Bluetooth manager (VoiceView/Audible) |
| `acsbtfd` | wrapper | 0 | ACS Bluetooth |
| `ace_eventmgr`, `ace_messaging_service`, `logmgr`, `acedropboxd` | wrappers | 0 | Amazon Common Services: events, device messaging, log upload, crash dropbox |
| `minervad`, `minerva_service` | wrappers | 0 | metrics (Minerva) |
| `demd` | wrapper | 0 | device event metrics |
| `iohwlogs`, `printklogs`, `cgroups`, `filesystems*`, `system_setup*` | tasks | — | one-shot boot tasks / log collection |

Not running (stop/waiting): `framework`, `lab126_gui`, `kb`, `pillow`, `poll_daemons`,
`bridge`, `chroot`, `otaupd`, `otav3`, `ota-update`, `stored`, `kindlesyncservice`,
`whisperstore`, `webreader`, `contentpackd`, `ivona_ttsd`, `asr*`, `audiomgrd`, `vnc`,
`usbnetd`, `fastmetrics`, `clickstream_logging`, `system_monitor`.

## 2. Classification

### (a) Needed now

| Keep | Why | Phase B replacement |
|---|---|---|
| upstart, udevd | boot, device nodes | Alpine init (OpenRC/busybox) + mdev/eudev |
| dbus + lipcd | every LIPC call (powerd, wifid, volumd, blanket) | gone (no LIPC in phase B) |
| powerd + deviced | suspend/resume, power button, idle, battery props | own suspend: `/sys/power/state`, RTC wake alarm, power button and hall sensor from input events, battery from `/sys/class/power_supply` |
| volumd + fsp | `/mnt/us` and USB drive mode | own USB gadget (configfs mass storage), unmount/remount of the vfat userstore directly (no FUSE) |
| wifim, wmt_service, wpa_supplicant, wifid, cmd, udhcpc | Wi-Fi | load the MTK driver + `wmt_service` (likely kept as a binary blob, it's hardware glue), then `wpa_supplicant` + `udhcpc` directly |
| clock | `cmd` does time auto-update over Amazon | busybox `ntpd`/sntp after Wi-Fi join; RTC sync with `hwclock` |
| zram | swap headroom | zram from Alpine |
| tee-supplicant, mdpd | MTK platform pieces, cheap | keep the binaries if the hardware needs them (test) |
| X + blanket (for now) | blanket draws USB/shutdown screens; powerd/volumd interplay assumed | Zarathustra draws everything itself |

### (b) Can be switched off now (no Zarathustra feature uses them)

`todo`, `tmd`, `archive`, `rcm`, `dynconfig`, `phd` (also closes the UDP port), `perfd`,
`stackdumpd`, `minerva_service`, `minervad`, `demd`, `ace_messaging_service` (takes `logmgr`
with it), `ace_eventmgr`, `acedropboxd`, `acsbtfd`, `btmanagerd`, `playermgr_limit`,
`playermgr`. Together ~55 MiB RSS plus most of the background CPU (tmd, rcm, todo).
Their LIPC services (`com.lab126.todo`, `transfer`, `archive`, `refreshCache`, `phd`,
`dynconfig`…) are not used by Zarathustra (verified: it only talks to `blanket`, `cmd`, `kaf`, `powerd`, `volumd`, `wifid`).

### (c) Unsure — test one at a time

| Job | Doubt | Test |
|---|---|---|
| `battstressd` | may protect the battery (charge/temperature policy) | read its strings/logs; leave on until known |
| `appmgrd` | powerd/blanket may query `activeApp`; USB screen flow | stop it, then suspend/resume, power button, USB plug |
| `dpmd` | policy flags only, probably safe | stop, check Wi-Fi toggle and USB |
| `x` (Xorg/awesome/blanket) | stopping it removes the USB white screen and the Amazon screensaver, but powerd's screensaver/suspend and volumd may wait on blanket | stop `x` with Zarathustra running; test suspend, power button, USB in/out, low-battery |
| `cmd` | wifid and time sync depend on it; it also starts tmd/phd | keep; just stop tmd/phd |
| `syslog`, `cron` | only logging/metrics, but syslog helps debugging | keep syslog; cron could go (tinyrot keeps /var from filling: keep it while syslog runs) |
| `tee-supplicant`, `mdpd` | unknown hardware role | leave on |

## 3. Other facts

### Boot sequence (upstart events)

`system` → `filesystems*` (rootfs ro, /var tmpfs, /var/local, userstore → `fsp`) →
`system_setup` → `dbus` → `lipcd` → **`lab126`** (meta job: 34 jobs `start on started lab126`,
incl. powerd, volumd, cmd, wifid, todo…) → `x_setup` → `x` (lxinit/Xorg/awesome/blanket) →
`lab126_gui` (on `n_ready and langpicker_ready and ekart_ready`) → `framework`
(on `contentpack_font_ready`) → `framework_ready` → `bridge`, `chroot`, `poll_daemons` →
`kindle` (final target: `poll_daemons`, `kb`, `pillow`, `cmd`).

- **`framework.conf` pre-start honours `/mnt/us/DONT_START_FRAMEWORK`**: if present the
  framework stops itself cleanly. The flag lives on the userstore, so it can be deleted from a
  computer over USB (volumd's drive mode works without the framework) — a natural undo.
- `lab126_gui` counts failed restarts (`MAX_RESTARTS=3`, then reboot; after `MAX_REBOOTS=2`
  it shows the customer-service screen and halts). Whether a framework that stops itself via the
  flag counts as a failure is **untested** — test before relying on it.
- `/etc/upstart/hooks` has an empty `hook_reboot_precheck` (Amazon hook, nothing in it).
- Where a boot hook can go: rootfs is ro but can be remounted rw (28 MiB free); one new
  `/etc/upstart/zarathustra.conf` (e.g. `start on started lab126 and started x`, or on
  `x_setup_ready`) is the same mechanism the jailbreak's own `bridge.conf` uses. No existing hook
  runs a userstore script at boot without the framework (`usbnet-autostart` only runs
  `/mnt/base-us/ENABLE_FQC` once and deletes it).
- The `chroot` job (Chromium sandbox) re-adds its bind mounts on every `framework_ready`:
  the mount table holds ~12 duplicate copies from repeated run.sh framework restarts. Harmless,
  disappears when the framework stays off.

### Jailbreak persistence and recovery

- WinterBreak (`/mnt/us/winterbreak.log`, `jb.sh`) + MKK in `/var/local/mkk`
  (`gandalf` setuid `su`, developer keystore, `updater_keys.sqsh`), patched UKS mounted at
  `/etc/uks` (dev keys accepted → custom update packages / MRPI install).
- `bridge` upstart job (`start on framework_ready`): restores `/var/local/mkk` and
  `/var/local/rp` from `/mnt/us/mkk` / `/mnt/us/rp`, re-sets gandalf's setuid, restores
  `/var/local/system/fixup` (= `bridge.sh`). It also runs **`/mnt/us/emergency.sh`** if present
  (the standard rescue hook). **Both only run when the framework reaches `framework_ready`**:
  with the framework off at boot, no self-repair and no emergency.sh.
- `/mnt/us/rp` is empty and `/var/local/rp` does not exist: **no RescuePack installed** (no
  `debrick` recovery). Install it before touching boot.
- KUAL (`extensions/KUAL.cfg`), MRInstaller, KOReader, kterm, renameotabin in `extensions/`.
  KUAL is an Amazon-UI booklet: unreachable while the framework is off (SSH and USB remain).

### OTA

Blocked: `/usr/bin/otaupd` and `/usr/bin/otav3` are renamed `*.bck` (renameotabin), both jobs
stop/waiting, the cron `otacheck` line is commented out, no `update*.bin` on the userstore.

### Network ports

| Port | Process | Note |
|---|---|---|
| TCP 22 (v4+v6) | Zarathustra SSH plugin (dropbear) | ours |
| UDP 0.0.0.0:46776 | `phd` | Amazon, listens on all interfaces: stop it |
| TCP 127.0.0.1:55555 | `stackdumpd` | local only |
| UDP 127.0.0.1:514 | `syslog-ng` | local only |

### Who holds /mnt/us

Only Zarathustra: `/mnt/us/zarathustra/state/zarathustra.db` (+ `-wal`, `-shm`) is open.
Note: HANDOFF says nothing holds the userstore on USB plug; the SQLite DB now does — check that
volumd's drive mode still works (it may refuse, or force-unmount under an open WAL DB). Possible
fix: close the DB on `userstoreDeferUnmountRequest`/USB events, or keep the DB in `/var/local`.

### Other

- `/var/local` is owned by `framework:javauser` and the `framework` job chgrp/chmods it on
  every start (why the SSH keys live elsewhere).
- Cron (`/etc/crontab/root`): `tinyrot` every 15 min, six `loginfo` hourly, `tmpfs_scan` every
  30 min, DEM heartbeat daily.

### Upstart version (checked again 2026-11-09, afternoon)

`initctl` reports **upstart 0.6.6**: no `.override` files and no `manual` stanza (both came in
1.x), so a job cannot be kept from starting without editing its `.conf`. The jobs we need
(`powerd`, `volumd`, `wifid`, `cmd`) and those to switch off (`todo`, `rcm`, `archive`…) all
`start on started lab126` (and `tmd`, `phd` on `started cmd`): disabling has to be a runtime
`stop` from our own job, as step 4 below already says. Job states and the OTA block
(`otaupd.bck`, `otav3.bck`) were as recorded above. Design that follows from this:
`docs/superpowers/specs/2026-11-09-boot-to-zarathustra-design.md`.

## 4. Recommended first steps (phase A)

Principle: runtime `stop` first (a reboot undoes everything), persistent only after a few days
of normal use.

0. **Safety first.** Install the RescuePack (debrick) so `bridge` has something to restore;
   keep the SSH plugin on; confirm USB drive mode still works with the DB open (see above).
1. **Batch 1, runtime only, from `run.sh`** after `stop lab126_gui`:
   ```sh
   AMAZON_JOBS="todo tmd archive rcm dynconfig phd perfd stackdumpd minerva_service minervad \
     demd ace_messaging_service ace_eventmgr acedropboxd acsbtfd btmanagerd playermgr_limit playermgr"
   for j in $AMAZON_JOBS; do stop "$j" 2>/dev/null; done
   ```
   and in `restore()` start them again (reverse order) before `start lab126_gui`, so the Amazon
   UI comes back intact. Undo: remove the lines, or reboot. Check for a day: suspend/resume,
   power button, Wi-Fi join and toggle, USB in/out, battery %, clock, battery drain overnight.
2. **Batch 2, one at a time:** `dpmd`, `appmgrd`, then `x` (removes the white USB screen and
   Amazon's screensaver; verify suspend/power button/USB/low battery). Leave `battstressd`
   until its role is known.
3. **Boot to Zarathustra** (opt-in): one `/etc/upstart/zarathustra.conf` on the rootfs
   (remount rw, add file, remount ro) that starts after `lab126` and `x`, skips itself when
   `/mnt/us/zarathustra/SAFE` exists, and runs `run.sh` logic. To keep the framework from
   starting use the existing `/mnt/us/DONT_START_FRAMEWORK` flag (undo: delete it over USB,
   SSH, or create `SAFE`). Test first that `lab126_gui` doesn't count this as a failed boot
   (reboot loop → customer-service halt), and remember `bridge`/`emergency.sh` won't run;
   a safe-mode gesture (e.g. hold the power button at boot → skip Zarathustra) is worth adding.
4. **Persistent service disable:** do it from the same boot job (the batch-1 `stop` list) rather
   than renaming Amazon's `.conf` files, so the rootfs carries one added file and nothing else;
   undo = delete that file.
