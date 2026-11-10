# Porting Zarathustra to another e-reader

One static binary runs everywhere; each device family is a module in
`crates/zarathustra-device/src/<vendor>/`, picked at startup. The Kobo port
(`kobo/`, `dist/kobo/`) is the worked example to copy from.

## 1. Where things live: `platform.rs`

Add a `Kind` and a branch in `platform::detect` that recognises the system by a file
only it has (Kobo: `/bin/kobo_config.sh` + `/mnt/onboard`). Fill in `Platform`: library,
state, fonts, hyphenation, dictionaries, plugin folders, the version file and the vendor
reader's database if the app should import from it. The shell never tests paths itself.
Add a `detects_<vendor>` test next to the others.

## 2. The hardware: `Device` (and `Wifi`)

Implement `zarathustra_device::Device` in `src/<vendor>/mod.rs`, plus an arm in
`open_default()` and `release()` (`lib.rs`). Keep a small model table (code, name, dpi,
touch transform, light and input nodes) keyed by what the system reports, as
`kobo::MODELS` does with `.kobo/version`.

Reuse before writing:

- `fb.rs`: framebuffer mapping with a RAM back buffer; `refresh()` flushes the rectangle,
  then the vendor ioctl (`kobo/mxcfb.rs`, `kindle/mtk.rs`) starts the panel update.
- `input.rs`: multitouch decoding (`TouchPanel`, `Transform` for swap/mirror) and `Keys`
  for buttons and the sleep cover's hall sensor.
- `sysfs.rs`: battery and frontlight nodes. `wpa.rs`: Wi-Fi through `wpa_supplicant`.

The capabilities have safe defaults; override the ones that are true:

- `draws_over_refresh`: the controller takes the pixels when an update starts and
  replays colliding updates itself (i.MX EPDC). Then the app draws without waiting for
  the panel: on the Clara 2E that removed a 330 ms stall per quick tap.
- `page_animation`, `has_sleep_cover`/`set_sleep_cover`, `wifi`, `light`, `battery`.

The app wraps the device in `Night` (night mode). A new trait method must be forwarded
there too, or the app only ever sees the default; `night::tests::forwards_the_capabilities`
checks the ones it has.

Find the panel's waveform numbers on the device, not in headers: the Clara 2E lists them in
`/sys/class/graphics/fb0/waveform_mode_*` as `code:index`, and the ioctl takes the code.
Timing a full-screen update per code tells them apart (A2 ≈ 240 ms, DU ≈ 290, GL16 ≈ 330,
GC16 ≈ 430 on the Clara 2E).

## 3. Launching: `dist/<vendor>/`

A launch script that stops the vendor UI, copies itself and the binary to `/tmp` (so nothing
stays open on the user storage), runs the app in a loop and always brings the vendor UI back.
The app's exit statuses are the contract:

- `0`: quit, restore the vendor UI.
- `42` (`USB_EXIT` in `app.rs`): hand the storage to the computer, run again once the cable
  is out.
- the update flag file present: a new binary was installed, run it again.

Before the first run, `zarathustra --release-device` puts the hardware back the way the
vendor UI expects (screen mode, radio off). See `dist/kobo/zarathustra.sh`.

## 4. Build and deploy

Each CPU gets its own flags for Rust and the C dependencies (SQLite, MuPDF): see `build` and
`build-kobo` in the `Makefile` (an A53 build dies with "Illegal instruction" on an A9). Add an
`update-<vendor>` target that stages the files and restarts the running app over SSH.

## 5. Checking layouts without the device

- `cargo run --release -p zarathustra --bin zarathustra -- [BOOK] --pgm DIR --size WxH`
  renders the home, pages and menus to PGM files at the device's size.
- `ZARATHUSTRA_SIZE=WxH cargo run -p zarathustra --features emulator --bin zarathustra`
  runs the emulator at that size.

Sizes across the UI are pixels for a 300 dpi screen (`screen_w()` in `zarathustra/src/lib.rs`).
A 167 or 212 dpi device needs a dpi scale there first.

## 6. On the device

Deploy, run, take screenshots, measure. Touch can be scripted over SSH by writing
`input_event` records to `/dev/input/eventN`, which is how the Kobo's latency was measured.
Mind that scripted taps move real reading positions.
