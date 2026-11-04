//! Jailbroken Kindles with the MediaTek e-ink controller.

mod mtk;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::input::{TouchPanel, Transform};
use crate::sysfs::{self, Led, SysfsLight};
use crate::{Battery, Canvas, Device, Event, Light, Refresh};

/// Per-model hardware facts. Adding a model = adding an entry to [`MODELS`].
struct Model {
    name: &'static str,
    dpi: u32,
    /// Detected by the presence of this white-LED backlight node.
    white_led: &'static str,
    warm_led: Option<&'static str>,
    battery: &'static str,
    touch: Transform,
}

const MODELS: &[Model] = &[Model {
    name: "Kindle Paperwhite 5",
    dpi: 300,
    white_led: "/sys/class/backlight/fp9966-bl1",
    warm_led: Some("/sys/class/backlight/fp9966-bl0"),
    battery: "/sys/class/power_supply/bd71827_bat",
    touch: Transform { swap_xy: false, mirror_x: false, mirror_y: false },
}];

pub struct Kindle {
    model: &'static Model,
    fb: mtk::MtkFb,
    touch: TouchPanel,
    light: Option<SysfsLight>,
    battery: PathBuf,
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
        });
        Ok(Kindle { model, fb, touch, light, battery: model.battery.into() })
    }

    /// Framebuffer facts worth logging while bringing up a new model.
    pub fn describe(&self) -> String {
        format!("{} {}x{} rotate={}", self.model.name, self.fb.width, self.fb.height, self.fb.rotate)
    }
}

impl Device for Kindle {
    fn name(&self) -> &str {
        self.model.name
    }

    fn dpi(&self) -> u32 {
        self.model.dpi
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
        let mut pfd = libc::pollfd { fd: self.touch.fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
        // SAFETY: one valid pollfd.
        if unsafe { libc::poll(&mut pfd, 1, ms) } < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(()) } else { Err(e) };
        }
        let mut touches = Vec::new();
        self.touch.read(&mut touches)?;
        out.extend(touches.into_iter().map(Event::Touch));
        Ok(())
    }

    fn light(&self) -> Option<Light> {
        self.light.as_ref()?.get()
    }

    fn set_light(&mut self, light: Light) -> io::Result<()> {
        match &self.light {
            Some(l) => l.set(light),
            None => Ok(()),
        }
    }

    fn battery(&self) -> Option<Battery> {
        sysfs::battery(&self.battery)
    }
}
