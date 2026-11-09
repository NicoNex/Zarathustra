//! Kobo e-readers on NXP i.MX "Mark 7"+ boards (Clara 2E…), with Nickel stopped by the
//! launch script (`dist/kobo/zarathustra.sh`).

mod light;
mod mxcfb;
mod power;
mod wifi;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::fb::Framebuffer;
use crate::input::{Keys, Touch, TouchPanel, Transform};
use crate::{Battery, Canvas, Device, Event, Light, PowerEvent, Refresh, Wifi};
use light::KoboLight;
use power::{Input, Power};
use wifi::KoboWifi;

/// Per-model hardware facts, keyed by the code that ends `/mnt/onboard/.kobo/version`.
/// Adding a model = adding an entry.
struct Model {
    code: &'static str,
    name: &'static str,
    dpi: u32,
    touch: Transform,
    white_led: &'static str,
    /// The warmth node and whether its step 0 is the warmest.
    warm_led: Option<(&'static str, bool)>,
    battery: &'static str,
    /// Input device names: the power button, the sleep cover's hall sensor.
    power_key: &'static str,
    cover: &'static str,
}

const MODELS: &[Model] = &[Model {
    code: "386",
    name: "Kobo Clara 2E",
    dpi: 300,
    // Measured: screen x = width − raw y, screen y = raw x.
    touch: Transform { swap_xy: true, mirror_x: true, mirror_y: false },
    white_led: "/sys/class/backlight/mxc_msp430.0",
    warm_led: Some(("/sys/class/leds/aw99703-bl_FL1", true)),
    battery: "/sys/class/power_supply/battery",
    power_key: "bd71828-pwrkey",
    cover: "gpio-keys",
}];

/// The model whose code ends the version line (`serial,…,00000000-…-000000000386`).
fn model_for(version: &str) -> Option<&'static Model> {
    let line = version.lines().next()?.trim();
    MODELS.iter().find(|m| line.ends_with(&format!("-{:0>12}", m.code)))
}

const KEY_F1: u16 = 59;
const KEY_POWER: u16 = 116;

/// Kobo's udev scripts report USB to Nickel through this FIFO ("usb plug add", …).
const HARDWARE_STATUS: &str = "/tmp/nickel-hardware-status";

/// `usb plug add|remove` lines in `text` as [`PowerEvent::Usb`]; wall chargers (`usb ac …`)
/// don't matter to the app.
fn hardware_status(text: &str) -> Vec<PowerEvent> {
    (text.lines())
        .filter_map(|l| match l.trim() {
            "usb plug add" => Some(PowerEvent::Usb(true)),
            "usb plug remove" => Some(PowerEvent::Usb(false)),
            _ => None,
        })
        .collect()
}

pub struct Kobo {
    model: &'static Model,
    fb: Framebuffer,
    marker: u32,
    touch: TouchPanel,
    touches: Vec<Touch>,
    power_key: Option<Keys>,
    cover: Option<Keys>,
    light: Option<KoboLight>,
    /// The light to restore on wake, while asleep.
    wanted: Option<Light>,
    /// Opened read-write so poll doesn't report a hang-up whenever no writer has it open.
    hardware: Option<File>,
    /// A line read only in part, completed by the next read.
    partial: String,
    power: Power,
    power_events: VecDeque<PowerEvent>,
    /// `None` when the launch script didn't pass Nickel's Wi-Fi environment.
    wifi: Option<Arc<KoboWifi>>,
}

impl Kobo {
    pub fn open() -> io::Result<Self> {
        let version = std::fs::read_to_string("/mnt/onboard/.kobo/version")?;
        let model =
            model_for(&version).ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unknown Kobo model"))?;
        let fb = Framebuffer::open("/dev/fb0", true)?;
        let touch = TouchPanel::find(Path::new("/dev/input"), (fb.width, fb.height), model.touch)?;
        let light =
            KoboLight::open(model.white_led, model.warm_led).map_err(|e| eprintln!("frontlight unavailable: {e}")).ok();
        let keys = |name| Keys::find(name).map_err(|e| eprintln!("{e}")).ok();
        let hardware = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(HARDWARE_STATUS)
            .map_err(|e| eprintln!("USB events unavailable: {e}"))
            .ok();
        // shortcut: one-time bring-up log; drop once the Clara 2E is verified.
        eprintln!("{} {}x{} rotate={}", model.name, fb.width, fb.height, fb.rotate);
        Ok(Kobo {
            model,
            fb,
            marker: 0,
            touch,
            touches: Vec::new(),
            power_key: keys(model.power_key),
            cover: keys(model.cover),
            light,
            wanted: None,
            hardware,
            partial: String::new(),
            power: Power::default(),
            power_events: VecDeque::new(),
            wifi: KoboWifi::from_env().map(Arc::new),
        })
    }

    fn feed(&mut self, input: Input) {
        let mut out = Vec::new();
        self.power.input(input, Instant::now(), &mut out);
        for ev in out {
            match ev {
                PowerEvent::Sleep => {
                    let saved = self.wanted.or_else(|| self.light());
                    if let (Some(l), Some(light)) = (saved, &self.light) {
                        light.set(Light { brightness: 0, ..l }).ok();
                    }
                    self.wanted = saved;
                }
                PowerEvent::Wake => {
                    if let (Some(l), Some(light)) = (self.wanted, &self.light) {
                        light.set(l).ok();
                    }
                    // Drop what the panel queued while asleep.
                    self.touches.clear();
                    self.touch.read(&mut self.touches).ok();
                }
                _ => {}
            }
            self.power_events.push_back(ev);
        }
    }

    fn read_keys(&mut self) {
        let mut keys = Vec::new();
        for dev in [&mut self.power_key, &mut self.cover].into_iter().flatten() {
            dev.read(&mut keys).ok();
        }
        for (code, value) in keys {
            match (code, value) {
                (KEY_POWER, 1) => self.feed(Input::Button),
                (KEY_F1, 1) => self.feed(Input::Cover(true)),
                (KEY_F1, 0) => self.feed(Input::Cover(false)),
                _ => {}
            }
        }
    }

    fn read_hardware(&mut self) {
        let Some(f) = &mut self.hardware else { return };
        let mut buf = [0u8; 512];
        while let Ok(n @ 1..) = f.read(&mut buf) {
            self.partial.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        let Some(end) = self.partial.rfind('\n') else { return };
        let lines: String = self.partial.drain(..=end).collect();
        self.power_events.extend(hardware_status(&lines));
    }

    fn suspend_if_due(&mut self) {
        if !self.power.due(Instant::now()) {
            return;
        }
        // The Wi-Fi driver doesn't survive a suspend (KOReader turns it off too).
        // shortcut: it stays off after the wake; turn it back on there if users miss it.
        if let Some(w) = &self.wifi {
            w.off().map_err(|e| eprintln!("wifi off: {e}")).ok();
        }
        if let Err(e) = power::suspend() {
            eprintln!("suspend: {e}");
        }
        self.feed(Input::Resumed);
    }
}

impl Device for Kobo {
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
        let rect = self.fb.flush(r.rect);
        if rect.is_empty() {
            return Ok(0);
        }
        self.marker = self.marker.wrapping_add(1).max(1);
        mxcfb::send_update(self.fb.fd(), rect, r.waveform, r.flash, self.marker)?;
        Ok(self.marker)
    }

    fn wait_refresh(&mut self, marker: u32) -> io::Result<()> {
        if marker == 0 {
            return Ok(());
        }
        // shortcut: KOReader calls this wait unreliable on this board (it can time out or
        // return early); an error is logged and treated as done. Measure on the device.
        if let Err(e) = mxcfb::wait(self.fb.fd(), marker) {
            eprintln!("refresh wait: {e}");
        }
        Ok(())
    }

    fn wait_events(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()> {
        self.suspend_if_due();
        if !self.power_events.is_empty() {
            return Ok(());
        }
        let pfd = |fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // A negative fd makes poll skip the slot.
        let fd = |k: &Option<Keys>| k.as_ref().map_or(-1, |k| k.fd());
        let mut fds = [
            pfd(self.touch.fd()),
            pfd(fd(&self.power_key)),
            pfd(fd(&self.cover)),
            pfd(self.hardware.as_ref().map_or(-1, |f| f.as_raw_fd())),
        ];
        let due = self.power.until_due(Instant::now());
        let timeout = match (timeout, due) {
            (Some(t), Some(d)) => Some(t.min(d)),
            (t, d) => t.or(d),
        };
        crate::sys::poll(&mut fds, timeout)?;
        self.read_keys();
        if fds[3].revents != 0 {
            self.read_hardware();
        }
        self.suspend_if_due();
        if !self.power_events.is_empty() {
            return Ok(());
        }
        self.touches.clear();
        self.touch.read(&mut self.touches)?;
        if !self.power.asleep() {
            out.extend(self.touches.iter().copied().map(Event::Touch));
        }
        Ok(())
    }

    fn light(&self) -> Option<Light> {
        self.light.as_ref()?.get()
    }

    fn set_light(&mut self, light: Light) -> io::Result<()> {
        // Asleep the light stays off; keep the newest level for the wake.
        self.wanted = Some(light);
        match &self.light {
            Some(l) if !self.power.asleep() => l.set(light),
            _ => Ok(()),
        }
    }

    fn battery(&self) -> Option<Battery> {
        crate::sysfs::battery(Path::new(self.model.battery))
    }

    fn power_event(&mut self) -> Option<PowerEvent> {
        self.power_events.pop_front()
    }

    fn sleep(&mut self) -> io::Result<()> {
        self.feed(Input::Button);
        Ok(())
    }

    fn wifi(&self) -> Option<Arc<dyn Wifi>> {
        self.wifi.clone().map(|w| w as Arc<dyn Wifi>)
    }
}

/// Leaves the hardware as the vendor UI expects it, before the launch script restarts it:
/// the screen mode it left, the Wi-Fi off. Both are tried; the first failure is returned.
pub fn release() -> io::Result<()> {
    let screen = crate::fb::restore_mode("/dev/fb0");
    let wifi = KoboWifi::from_env().map_or(Ok(()), |w| w.off());
    screen.and(wifi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_from_version() {
        let line = "N000000000000,4.1.15,4.38.23852,4.1.15,4.1.15,00000000-0000-0000-0000-000000000386\n";
        assert_eq!(model_for(line).map(|m| m.name), Some("Kobo Clara 2E"));
        assert!(
            model_for("N000000000000,4.1.15,4.38.23852,4.1.15,4.1.15,00000000-0000-0000-0000-000000000999").is_none()
        );
    }

    #[test]
    fn parses_hardware_status() {
        let events = hardware_status("usb plug add\nusb ac add\nusb plug remove\n");
        assert_eq!(events, [PowerEvent::Usb(true), PowerEvent::Usb(false)]);
    }
}
