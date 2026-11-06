# Kindle power button and sleep (PW5, FW 5.17.1)

Findings from the device (read-only inspection, plus `powerprobe` runs) and from KOReader's
`frontend/device/kindle/{device,powerd}.lua`, which we read to learn from, not to copy.

## Who owns what with `lab126_gui` stopped

`stop lab126_gui` only stops the Java framework. Still running: `powerd`, `lipc-daemon`,
`Xorg`, `awesome`, and `blanket` (with modules `langpicker blankwindow ad_screensaver usb`).

- **Power key**: `/dev/input/event0`, `bd71828-pwrkey` (gpio-keys, `EV_KEY` `KEY_POWER` only).
  `powerd` reads it. We don't: fighting powerd over suspend would be fragile.
- **powerd** runs the sleep state machine: `active` → `screenSaver` → `readyToSuspend` →
  `suspended`, and back. It publishes lipc events on `com.lab126.powerd`:
  - `goingToScreenSaver <src>`: 2 = button, 4 = hall sensor (cover).
  - `readyToSuspend <delay>`: it will suspend in `delay` seconds; `rtcWakeup <s>` is only
    accepted in this state.
  - `wakeupFromSuspend <ts>`: resumed from suspend but still in the screen saver (an RTC
    alarm, or the first step of a button wake).
  - `outOfScreenSaver <src>`: 1 = button, 6 = hall.
- **blanket**'s `ad_screensaver` (Special Offers device) or `screensaver` module draws
  Amazon's screen saver on `goingToScreenSaver`, framework or not. KOReader unloads it
  (`lipc-set-prop com.lab126.blanket unload screensaver`) when it stops the framework.
  `run.sh` now unloads whichever of the two is loaded and reloads it on exit.
- `powerButton` (w, int) on powerd simulates a press: `lipc-set-prop -i com.lab126.powerd
  powerButton 1`. KOReader's "suspend" action uses exactly this; there's no difference
  between it and the real key for powerd.
- `preventScreenSaver 1` blocks the idle timeout AND the power button ("Prevent screen saver
  set. Ignoring power button"; only the cover still works): never set it. powerd can't see our
  touches, so Zarathustra restarts its idle timer (`screenSaverTimeout`, 600 s) with
  `touchScreenSaverTimeout 1` at most every 30 s while touches arrive.
- `deferSuspend`, `suspendGrace`, `abortSuspend`, `wakeUp`: writable knobs; not needed.
- `/sys/power/state` offers `freeze mem`. Writing it ourselves would bypass powerd's
  bookkeeping (wifi, RTC, battery, hibernation after 1 h): don't.

## How we listen

`lipc-wait-event -m com.lab126.powerd goingToScreenSaver,outOfScreenSaver,readyToSuspend,wakeupFromSuspend`
prints one line per event, `name arg`, **line-buffered even into a pipe** (verified with
`lipc-send-event` on a private source). Zarathustra spawns it as a child with
`PR_SET_PDEATHSIG`, sets the pipe non-blocking and polls its fd next to the touch panel in
`Device::wait_events`. No liblipc FFI is needed.

## Sleep sequence (what Zarathustra does)

1. `goingToScreenSaver`: the device backend saves the light level and turns it off, and
   mutes touch. The app draws the sleep screen and refreshes it with a full flash.
2. `readyToSuspend <d>`: nothing to do; powerd suspends `d` s later.
3. Suspended. The panel keeps the image with no power.
4. Button: `wakeupFromSuspend`, then `outOfScreenSaver 1`. The backend drains touches queued
   while asleep, restores the light; the app redraws its screen.

## Light and touch across sleep

- Light: see the probe log below for whether powerd turns the light off by itself. The backend
  sets it to 0 on sleep and restores the saved level on wake either way.
- Touch: the panel still reports while in the screen saver; the backend drops those reports.
  A contact that began asleep ends with an `Up` without a `Down`, which the app ignores.

## Risks

- If Zarathustra dies while asleep, `run.sh` restarts the framework; the next button press wakes
  into the Amazon UI. The listener child dies with us (PDEATHSIG).
- If the blanket module were left unloaded (run.sh killed with SIGKILL), the Amazon UI would
  sleep with a white screen until the next reboot or `lipc-set-prop com.lab126.blanket load
  ad_screensaver`. Harmless.

## Power button without the framework (2026-11-06)

Symptom: with `lab126_gui` stopped, every press logs `E lipc:dbuserr ... The name
com.lab126.kaf was not provided by any .service files` then `I def:pbisplash::Splash screen
is on.  Ignoring power button`. The cover kept working.

Root cause (powerd reverse-engineered with REA/Hopper, FW 5.17.1): the power-button branch of
the `active` state handler calls a "boot finished?" check (at `0x14888`) that is true only if
1. `LipcGetIntProperty(KAF_NAME, KAF_FRAMEWORK_STARTED)`, i.e. `com.lab126.kaf
   frameworkStarted`, succeeds and is non-zero, and
2. `LipcGetStringProperty(BLANKET_NAME, BLANKET_LOAD)`, blanket's `load`, contains no
   `splash`.

With the framework stopped (1) fails, so the press is dropped. The hall-sensor branch never
calls the check. `bootSplashCleanup` only affects (2), which is why sending it did nothing.

Fix: Zarathustra publishes `com.lab126.kaf frameworkStarted = 1` itself while the framework is
down. The firmware ships Lua 5.1 with Amazon's lipc binding (`/usr/lib/lua/liblipclua.so`,
what `awesome` uses), so the kindle backend spawns

    lua -e 'require("liblipclua"); local h = assert(lipc.init("com.lab126.kaf"))
            h:register_int_property("frameworkStarted", "r").value = 1
            while true do lipc.run_event_loop(3600) end'

as a child with `PR_SET_PDEATHSIG` (it dies with Zarathustra, before `run.sh` restarts the
framework, which needs the name back). It is skipped when `com.lab126.kaf` already answers
(framework running). `lipc.run_event_loop()` with no argument returns at once; with `n` it
runs `n` seconds. Cost: one idle process, ~1.5 MB RSS, no CPU.

Alternatives weighed: keeping the framework alive (KOReader's default) costs ~100 MB of Java
and needs EVIOCGRAB plus muting pillow/awesome/blanket so the hidden UI doesn't react;
reading `KEY_POWER` ourselves doesn't help because `powerButton` (and `powerd_test -p`) go
through the same check, and driving `/sys/power/state` would bypass powerd's bookkeeping.

`run.sh` still sends `bootSplashCleanup` for condition (2) after a stop during boot.
