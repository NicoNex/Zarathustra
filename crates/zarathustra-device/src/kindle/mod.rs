//! Jailbroken Kindles with the MediaTek e-ink controller.

mod mtk;
mod power;
mod wifi;

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// How often touches restart powerd's idle timeout (a spawned `lipc-set-prop` each time).
const IDLE_RESET_EVERY: Duration = Duration::from_secs(30);
/// How long, and how often, the light is watched after a wake.
const RELIGHT_FOR: Duration = Duration::from_secs(4);
const RELIGHT_EVERY: Duration = Duration::from_millis(200);

use crate::input::{Touch, TouchPanel, Transform};
use crate::sysfs::{self, Led, SysfsLight};
use crate::{Battery, Canvas, Device, Event, Light, PowerEvent, Refresh};

/// Per-model hardware facts. Adding a model = adding an entry to [`MODELS`].
struct Model {
    name: &'static str,
    dpi: u32,
    /// Detected by the presence of this white-LED backlight node.
    white_led: &'static str,
    warm_led: Option<&'static str>,
    /// The vendor's measured LED mix ([`sysfs::Mix`]), when known.
    light_mix: Option<&'static str>,
    battery: &'static str,
    touch: Transform,
}

const MODELS: &[Model] = &[Model {
    name: "Kindle Paperwhite 5",
    dpi: 300,
    white_led: "/sys/class/backlight/fp9966-bl1",
    warm_led: Some("/sys/class/backlight/fp9966-bl0"),
    light_mix: Some(include_str!("pw5-light.txt")),
    battery: "/sys/class/power_supply/bd71827_bat",
    touch: Transform { swap_xy: false, mirror_x: false, mirror_y: false },
}];

pub struct Kindle {
    model: &'static Model,
    fb: mtk::MtkFb,
    touch: TouchPanel,
    light: Option<SysfsLight>,
    /// Reused across `wait_events` calls.
    touches: Vec<Touch>,
    /// `None` when powerd can't be reached; the device then just never sleeps.
    powerd: Option<power::Powerd>,
    /// Held for its lifetime: lets powerd honour the power button without the framework.
    _kaf: Option<power::KafStub>,
    power_events: VecDeque<PowerEvent>,
    /// Set while the sleep screen shows: the light to restore on wake.
    asleep: Option<Option<Light>>,
    /// The level last asked for. On wake powerd puts back its own, so reading sysfs at sleep
    /// could save powerd's level instead of ours.
    wanted: Option<Light>,
    /// Until when the light is checked after a wake: powerd puts back its own level some
    /// moments after telling us, so one restore at the event is overwritten.
    relight_until: Option<Instant>,
    /// When touches last restarted powerd's idle timeout.
    idle_reset: Option<Instant>,
    /// The USB controller's state file (`configured` when a computer has enumerated us) and
    /// the last state seen. The kernel notifies changes through poll(POLLPRI).
    udc: Option<(std::fs::File, bool)>,
}

impl Kindle {
    pub fn open() -> io::Result<Self> {
        let model = MODELS
            .iter()
            .find(|m| Path::new(m.white_led).exists())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unknown Kindle model"))?;
        let fb = mtk::MtkFb::open("/dev/fb0")?;
        let touch = TouchPanel::find(Path::new("/dev/input"), (fb.fb.width, fb.fb.height), model.touch)?;
        let light = Led::open(model.white_led).ok().map(|white| SysfsLight {
            white,
            warm: model.warm_led.and_then(|p| Led::open(p).ok()),
            mix: model.light_mix.and_then(sysfs::Mix::parse),
        });
        // ponytail: one-time bring-up log; drop once more models are verified.
        eprintln!("{} {}x{} rotate={}", model.name, fb.fb.width, fb.fb.height, fb.fb.rotate);
        let powerd = power::Powerd::spawn().map_err(|e| eprintln!("powerd events unavailable: {e}")).ok();
        let _kaf = power::KafStub::spawn().map_err(|e| eprintln!("kaf stub: {e}")).ok().flatten();
        Ok(Kindle {
            model,
            fb,
            touch,
            light,
            touches: Vec::new(),
            powerd,
            _kaf,
            power_events: VecDeque::new(),
            asleep: None,
            wanted: None,
            relight_until: None,
            idle_reset: None,
            udc: udc_state_file().map(|f| (f, false)),
        })
    }

    /// Reads the USB state again; a [`PowerEvent::Usb`] when it changed.
    fn read_usb(&mut self) {
        use std::io::{Read, Seek};
        let Some((file, last)) = &mut self.udc else { return };
        let mut state = String::new();
        if file.rewind().and_then(|_| file.read_to_string(&mut state)).is_err() {
            return;
        }
        let host = state.trim() == "configured";
        if host != *last {
            *last = host;
            self.power_events.push_back(PowerEvent::Usb(host));
        }
    }

    fn read_power(&mut self) {
        let Some(powerd) = &mut self.powerd else { return };
        let mut events = Vec::new();
        if let Err(e) = powerd.read(&mut events) {
            eprintln!("powerd events lost: {e}");
            self.powerd = None;
            // No Wake can arrive any more: wake now rather than stay dark with touch muted.
            if self.asleep.is_some() {
                events.push(PowerEvent::Wake);
            }
        }
        for ev in events {
            self.on_power(ev);
            self.power_events.push_back(ev);
        }
    }

    /// After a wake, puts our level back whenever powerd has changed it (checked every poll,
    /// at most every [`RELIGHT_EVERY`], for [`RELIGHT_FOR`]).
    // ponytail: polls sysfs for a few seconds; setting powerd's own stored level instead
    // (`flIntensity`) would stop the fight, once its scale is known.
    fn keep_light(&mut self) {
        let Some(until) = self.relight_until else { return };
        if Instant::now() >= until {
            self.relight_until = None;
            return;
        }
        let (Some(want), Some(now)) = (self.wanted, self.light()) else { return };
        // Reading back goes through the LED mix: a point either way is the same level.
        let off = |a: u8, b: u8| a.abs_diff(b) > 1;
        let warmth_off = matches!((want.warmth, now.warmth), (Some(a), Some(b)) if off(a, b));
        if off(want.brightness, now.brightness) || warmth_off {
            self.set_light(want).ok();
        }
    }

    /// Light off and touch muted while asleep; both restored on wake.
    fn on_power(&mut self, ev: PowerEvent) {
        match ev {
            PowerEvent::Sleep if self.asleep.is_none() => {
                let saved = self.wanted.or_else(|| self.light());
                if let Some(l) = saved {
                    self.set_light(Light { brightness: 0, ..l }).ok();
                }
                self.asleep = Some(saved);
            }
            PowerEvent::Wake => {
                if let Some(Some(l)) = self.asleep.take() {
                    self.set_light(l).ok();
                }
                self.relight_until = Some(Instant::now() + RELIGHT_FOR);
                // Drop what the panel queued while asleep. A contact begun asleep then ends
                // with an Up whose Down was never delivered, which callers already ignore.
                self.touches.clear();
                self.touch.read(&mut self.touches).ok();
            }
            // After a real suspend powerd may have set its level again: ours wins.
            PowerEvent::Resumed if self.asleep.is_none() => {
                if let Some(l) = self.wanted {
                    self.set_light(l).ok();
                }
                self.relight_until = Some(Instant::now() + RELIGHT_FOR);
            }
            _ => {}
        }
    }
}

impl Device for Kindle {
    fn name(&self) -> &str {
        self.model.name
    }

    fn dpi(&self) -> u32 {
        self.model.dpi
    }

    fn size(&self) -> (i32, i32) {
        (self.fb.fb.width, self.fb.fb.height)
    }

    fn canvas(&mut self) -> Canvas<'_> {
        self.fb.fb.canvas()
    }

    fn refresh(&mut self, r: &Refresh) -> io::Result<u32> {
        self.fb.refresh(r)
    }

    fn wait_refresh(&mut self, marker: u32) -> io::Result<()> {
        self.fb.wait(marker)
    }

    fn wait_events(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()> {
        if !self.power_events.is_empty() {
            return Ok(());
        }
        let pfd = |fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // A negative fd makes poll skip the slot once the powerd listener is gone.
        use std::os::fd::AsRawFd;
        let udc = self.udc.as_ref().map_or(-1, |(f, _)| f.as_raw_fd());
        let mut fds = [
            pfd(self.touch.fd()),
            pfd(self.powerd.as_ref().map_or(-1, |p| p.fd())),
            libc::pollfd { fd: udc, events: libc::POLLPRI | libc::POLLERR, revents: 0 },
        ];
        // Watching the light after a wake: wake up often enough to catch powerd.
        let timeout = match self.relight_until {
            Some(_) => Some(timeout.map_or(RELIGHT_EVERY, |t| t.min(RELIGHT_EVERY))),
            None => timeout,
        };
        crate::sys::poll(&mut fds, timeout)?;
        if fds[1].revents != 0 {
            self.read_power();
        }
        // Read on every wake too, in case the notification never comes.
        self.read_usb();
        if !self.power_events.is_empty() {
            return Ok(());
        }
        self.keep_light();
        self.touches.clear();
        self.touch.read(&mut self.touches)?;
        if self.asleep.is_none() && !self.touches.is_empty() {
            out.extend(self.touches.iter().copied().map(Event::Touch));
            // powerd can't see our touches: restart its idle timeout (10 min to sleep) now
            // and then, as the framework would.
            if self.idle_reset.is_none_or(|t| t.elapsed() >= IDLE_RESET_EVERY) {
                self.idle_reset = Some(Instant::now());
                // On a thread: `lipc-set-prop` takes tens of ms, and this is the touch path.
                std::thread::spawn(|| {
                    power::set_prop("touchScreenSaverTimeout", 1).map_err(|e| eprintln!("powerd idle reset: {e}")).ok()
                });
            }
        }
        Ok(())
    }

    fn light(&self) -> Option<Light> {
        self.light.as_ref()?.get()
    }

    fn set_light(&mut self, light: Light) -> io::Result<()> {
        // Asleep the light stays off; keep the newest level (auto warmth) for the wake.
        self.wanted = Some(light);
        if let Some(saved) = &mut self.asleep {
            *saved = Some(light);
            return Ok(());
        }
        match &self.light {
            Some(l) => l.set(light),
            None => Ok(()),
        }
    }

    fn battery(&self) -> Option<Battery> {
        sysfs::battery(Path::new(self.model.battery))
    }

    fn power_event(&mut self) -> Option<PowerEvent> {
        self.power_events.pop_front()
    }

    fn sleep(&mut self) -> io::Result<()> {
        power::set_prop("powerButton", 1)
    }

    fn page_animation(&self) -> bool {
        true
    }

    fn wifi(&self) -> Option<std::sync::Arc<dyn crate::Wifi>> {
        Some(std::sync::Arc::new(wifi::KindleWifi))
    }
}

/// `/sys/class/udc/<controller>/state`, opened for reading.
fn udc_state_file() -> Option<std::fs::File> {
    let dir = std::fs::read_dir("/sys/class/udc").ok()?.flatten().next()?;
    std::fs::File::open(dir.path().join("state")).ok()
}
