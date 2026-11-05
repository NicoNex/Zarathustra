//! Jailbroken Kindles with the MediaTek e-ink controller.

mod mtk;
mod power;

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::time::Duration;

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
    power_events: VecDeque<PowerEvent>,
    /// Set while the sleep screen shows: the light to restore on wake.
    asleep: Option<Option<Light>>,
}

impl Kindle {
    pub fn open() -> io::Result<Self> {
        let model = MODELS
            .iter()
            .find(|m| Path::new(m.white_led).exists())
            .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unknown Kindle model"))?;
        let fb = mtk::MtkFb::open("/dev/fb0")?;
        let touch = TouchPanel::find(Path::new("/dev/input"), (fb.width, fb.height), model.touch)?;
        let light = Led::open(model.white_led).ok().map(|white| SysfsLight {
            white,
            warm: model.warm_led.and_then(|p| Led::open(p).ok()),
            mix: model.light_mix.and_then(sysfs::Mix::parse),
        });
        // ponytail: one-time bring-up log; drop once more models are verified.
        eprintln!("{} {}x{} rotate={}", model.name, fb.width, fb.height, fb.rotate);
        let powerd = power::Powerd::spawn().map_err(|e| eprintln!("powerd events unavailable: {e}")).ok();
        Ok(Kindle { model, fb, touch, light, touches: Vec::new(), powerd, power_events: VecDeque::new(), asleep: None })
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

    /// Light off and touch muted while asleep; both restored on wake.
    fn on_power(&mut self, ev: PowerEvent) {
        match ev {
            PowerEvent::Sleep if self.asleep.is_none() => {
                let saved = self.light();
                if let Some(l) = saved {
                    self.set_light(Light { brightness: 0, ..l }).ok();
                }
                self.asleep = Some(saved);
            }
            PowerEvent::Wake => {
                if let Some(Some(l)) = self.asleep.take() {
                    self.set_light(l).ok();
                }
                // Drop what the panel queued while asleep. A contact begun asleep then ends
                // with an Up whose Down was never delivered, which callers already ignore.
                self.touches.clear();
                self.touch.read(&mut self.touches).ok();
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
        (self.fb.width, self.fb.height)
    }

    fn canvas(&mut self) -> Canvas<'_> {
        self.fb.canvas()
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
        let mut fds = [pfd(self.touch.fd()), pfd(self.powerd.as_ref().map_or(-1, |p| p.fd()))];
        // Round up so a sub-millisecond remainder doesn't become a busy-spinning 0.
        let ms = timeout.map_or(-1, |t| t.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32);
        // SAFETY: `fds` is a valid array of `fds.len()` pollfds.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) } < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(()) } else { Err(e) };
        }
        if fds[1].revents != 0 {
            self.read_power();
        }
        self.touches.clear();
        self.touch.read(&mut self.touches)?;
        if self.asleep.is_none() {
            out.extend(self.touches.iter().copied().map(Event::Touch));
        }
        Ok(())
    }

    fn light(&self) -> Option<Light> {
        self.light.as_ref()?.get()
    }

    fn set_light(&mut self, light: Light) -> io::Result<()> {
        // Asleep the light stays off; keep the newest level (auto warmth) for the wake.
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
}
