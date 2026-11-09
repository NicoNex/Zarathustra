# Kobo port and device abstractions — design

Date: 2026-11-09. Status: approved by the user in chat ("procedi"); calls marked *(my call)* are
the assistant's and open to correction.

## Intent

Run Zarathustra on the user's **Kobo Clara 2E**, launched from NickelMenu next to Nickel (as
KOReader and Plato are), and, in the same move, shape the project so the next device (another
Kobo, a newer Kindle, a PocketBook…) is a backend module plus a launch script, not a hunt
through the shell for Kindle assumptions.

Success: on the Clara 2E, NickelMenu → Zarathustra opens the home with the books already on
the Kobo, reads them with page turns, menu, highlights and dictionary as on the Kindle,
sleeps and wakes on the power button, and quitting brings Nickel back. The Kindle build keeps
compiling with its tests green (it is not deployed or tried today). A short guide says how to add a device.

## Goals

1. **Screen size from the device.** The `W`/`H` constants (1236×1648, 93 uses in 29 files)
   become the size the backend reports at startup. Every screen lays out within it.
2. **Platform facts from the backend.** Where books, state, fonts, dictionaries,
   hyphenation and plugins live; the system version file; the device's name in user-facing
   text ("il Kindle" / "il Kobo"). No `/mnt/us` left in the shell outside the Kindle backend.
3. **One binary, model detected at startup.** `open_default()` picks Kindle or Kobo by
   probing the system (e.g. `/mnt/us` vs `/mnt/onboard` + `/bin/kobo_config.sh`), as decided
   on 2026-11-06 (no dynamic drivers).
4. **Kobo backend** (`zarathustra-device/src/kobo/`): mxcfb e-ink refresh (i.MX "Mark 7"
   API), touch, power button, frontlight with warmth, battery, suspend/resume done by us,
   USB plug events, Wi-Fi.
5. **Launch and lifecycle per platform** with one shared contract: a launch script per
   platform (`dist/kual/…` exists; new `dist/kobo/…` with the NickelMenu entry), the same exit
   codes meaning the same things (quit, restart into a new build, USB).
6. **Emulator at any size**, so each layout is checked at 1072×1448 on the desktop (`--pgm`
   renders too) before touching the device.
7. **"Add a device" guide** (`docs/porting.md`): the trait to implement, the platform facts,
   model detection, the launch script contract, testing with the emulator.

## Non-goals (this round)

- **Scaling by dpi.** The Clara 2E and the PW5 are both 300 dpi, so sizes in pixels stay right.
  A 167/212 dpi device will need a scale factor; leave a `shortcut:` note where sizes are
  defined, upgrade when such a device is ported.
- Landscape, other Kobo models (but nothing in the backend should be Clara-only where a table
  entry would do), booting without Nickel, kepub-specific features (Kobo spans are ordinary
  markup to us).

## Anti-goals

- A port that breaks the Kindle build: every step keeps it compiling and its tests green.
- An abstraction layer with nothing behind it: traits and structs only where two backends
  already differ. No plugin-loaded drivers, no config files for things the code can detect.
- Copying code from KOReader, Plato or FBInk. They are references for facts (ioctl numbers,
  sysfs paths, quirks), never a source of code.

## Constraints

- Rust, one static musl binary for `armv7-unknown-linux-musleabihf` (the Kobo's i.MX6 runs it
  as the Kindle does).
- Device facts found so far (to confirm with a probe on the device):
  - Kobo Clara 2E, codename *goldfinch*, `/mnt/onboard/.kobo/version` ends in `…0386`;
    firmware 4.38.
  - Framebuffer `mxc_epdc_fb`, 1072×1448, 300 dpi, 8 bpp at boot, rotation 3; Mark 7 mxcfb API
    (send update v2, wait for update complete v3). KOReader marks its "wait for update" as
    unreliable on this board (`hasReliableMxcWaitFor = no`).
  - Frontlight: `/sys/class/backlight/mxc_msp430.0/brightness` (white),
    `/sys/class/leds/aw99703-bl_FL1/color` (warmth, 0–10, inverted).
  - Battery: `/sys/class/power_supply/battery`.
  - Touch: multitouch protocol B; axis mapping to be measured.
  - No powerd: sleep means we suspend the kernel ourselves and wake on the power button.
- Books on the Kobo today: `.kepub.epub` files in the root of `/mnt/onboard`. Nickel's own
  folders are hidden (`.kobo`, `.adds`, `.kobo-images`…).
- Without SSH, every device test is: copy over USB, eject, tap the NickelMenu entry, read the
  log after reconnecting. Steps are sized so each needs few such round trips.

## Decided

- Launched from NickelMenu (user). Quitting brings Nickel back (user).
- The port comes with abstractions that make the next port easy (user).
- Look at how KOReader and Plato do it (user), within the anti-goal above.
- Library on the Kobo: a **dedicated `/mnt/onboard/library/`**, as on the Kindle (user, on
  the assistant's advice). It is not hidden, so Nickel lists those books too; books stay
  where they are until the user moves them (moving a book loses Nickel's progress for it,
  Nickel keys by path). The deploy creates the folder empty.
- Zarathustra's own files on the Kobo under `/mnt/onboard/.adds/zarathustra/` (binaries,
  hyphenation, state, plugins), hidden from Nickel's library scan *(my call)*. Dictionaries
  and extra fonts in `/mnt/onboard/.adds/zarathustra/{dictionaries,fonts}` plus the Kobo's own
  `/mnt/onboard/fonts` *(my call)*.

## Settled after the builder check

- **Kobo only today** (user: "oggi lavoriamo solo al Kobo, quindi il Kindle lo mettiamo da
  parte"). The boot-to-Zarathustra plan stays parked; no Kindle device work, no Kindle
  deploys. Shared code still keeps the Kindle build compiling and its tests green (CLAUDE.md
  gates), and the Kindle launch script is left as it is.
- **Access to the Kobo:** NickelMenu is installed (KOReader and Plato launch through it).
  There is no shell on the device (no telnet/SSH running). Probes and the app start from a
  NickelMenu `cmd_spawn` entry and log to a file on `/mnt/onboard`, read after reconnecting
  USB. No `KoboRoot.tgz` install step is needed.
- **USB cable on the Kobo, as on the Kindle** (user: "guarda koreader eccetera"). With Nickel
  stopped, Kobo's udev writes `usb plug add` / `usb plug remove` (and `usb ac …` for a wall
  charger) to the FIFO `/tmp/nickel-hardware-status`; Plato reads it the same way. The Kobo
  backend turns these into `PowerEvent::Usb(true/false)`, so the app's existing prompt
  (`usb_prompt`: hand the storage to the computer, or just charge) shows unchanged.
  "Transfer" exits with the existing `USB_EXIT` (42). The Kobo launch script then exports
  the storage itself, as Plato's `usb-enable.sh`/`usb-disable.sh` do: sync, unmount
  `/mnt/onboard`, load the mass-storage gadget for the platform (`$PLATFORM`, likely `mx6sll-ntx` on
  the Clara 2E: configfs + libcomposite + usb_f_mass_storage + g_file_storage on
  `/dev/mmcblk0p3`). It waits for `usb plug remove` on the FIFO, then unloads the gadget, runs
  `dosfsck`, remounts, and starts Zarathustra again (which rescans the library). KOReader does
  the same from its launch script with exit code 86 and a helper binary. Like `run.sh` on the
  Kindle, the script runs the binary from a copy in `/tmp`, so nothing holds `/mnt/onboard`
  open when it is unmounted. The screen keeps a "connected" message while exported (the app
  draws it before exiting). "Restart into a new build" (the update flag) works the same way,
  so `make install-usb` for the Kobo copies the files and the app restarts into them when the
  cable comes out.
- **Wi-Fi on the Kobo, in scope** (user: "stessa cosa vale per il wifi", i.e. look at how
  KOReader and Plato do it). A Kobo `Wifi` implementation behind the existing trait, so the
  Wi-Fi settings, quick-panel tile, Wi-Fi transfer and plugins work as on the Kindle. Facts
  from the references (to confirm on the device): the driver module comes from the
  environment Nickel's rcS sets (`$PLATFORM`, `$WIFI_MODULE`, `$INTERFACE`, readable from
  `/proc/$(pidof udevd)/environ` once Nickel is gone); bring-up is power on (module
  `sdio_wifi_pwr`, or `ntx_io`/`wmt` on other boards) → `insmod` the driver from
  `/drivers/$PLATFORM/…` → `ifconfig $INTERFACE up` → `wpa_supplicant -D <wext|nl80211>
  -c /etc/wpa_supplicant/wpa_supplicant.conf -C /var/run/wpa_supplicant -B` → DHCP (`dhcpcd`
  if present, as Nickel does, else `udhcpc`); bring-down is the reverse. That config file is
  Nickel's, so the networks the user already saved in Nickel are known to us, and networks
  joined in Zarathustra (`wpa_cli add_network … save_config`, as the Kindle fallback already
  does) are known to Nickel. Scan, list and status go through `wpa_cli`. Bring-up/down live
  in a small script per platform shipped with the app *(my call: shell is where KOReader and
  Plato keep it, and the module dance is easier to fix there)*; the Rust side calls it.
- **Missing capabilities never change the UI** (user: "al più non cambiare la UI ma magari
  fai che se clicco sul tile non succede nulla"). Every screen looks the same on every
  device; when a backend lacks something (`wifi()` is `None`, Wi-Fi bring-up failed, no warm
  light…) the tile or button stays where it is and a tap does nothing. No hiding, no
  per-device layouts.
- **Then over the air:** once Wi-Fi works, the SSH plugin ships on the Kobo too (same armv7
  musl Dropbear build; its Kindle-only paths become platform facts), and `make update` /
  `make install-ssh` gain a Kobo target, so later steps stop needing the cable. The plugins
  folder is a platform fact like the others.
- **Fonts:** the user's Bookerly family is already in `/mnt/onboard/fonts`, so the default
  reading font works as on the Kindle. The UI font falls back as it does on the desktop. Nickel's
  firmware fonts are not used *(my call)*.

## Left to the builder

- The exact shape of the platform facts (a struct returned by `Device`, or a small
  `Platform` value next to it), as long as the shell asks the device and never tests paths.
- How the screen size reaches layout code (passed in `Ctx`, or a process-wide value set once
  at startup) — whichever keeps the diff smallest and the code readable.
- The touch transform, refresh waveform mapping and suspend sequence, from a probe on the
  device and the references.
- What the Kobo launch script stops and restarts around Zarathustra so Nickel comes back
  cleanly.
- Fixing layouts that only fit 1236 px wide.
