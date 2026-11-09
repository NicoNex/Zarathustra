//! Hardware abstraction: everything device-specific lives behind [`Device`].

pub mod canvas;
mod fb;
pub mod geom;
pub mod input;
pub mod kindle;
pub mod kobo;
mod night;
pub mod platform;
mod sys;
pub mod sysfs;
pub mod wifi;
mod wpa;

#[cfg(feature = "emulator")]
pub mod emulator;

use std::io;
use std::sync::Arc;
use std::time::Duration;

pub use canvas::Canvas;
pub use geom::Rect;
pub use input::{Phase, Touch};
pub use night::{night, set_night, Night};
pub use platform::{platform, Kind, Platform};
pub use wifi::Wifi;

/// What a refresh is for; each backend maps it to its own waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    /// Full 16-level redraw, clears ghosting (flashes when `flash`).
    Full,
    /// Grayscale content (page text) without flashing.
    Text,
    /// Fast black/white, for UI feedback and drags.
    Fast,
    /// Fastest 1-bit, for animations; ghosts the most.
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swipe {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy)]
pub struct Refresh {
    pub rect: Rect,
    pub waveform: Waveform,
    pub flash: bool,
    /// Hardware page-turn animation, where supported.
    pub swipe: Option<Swipe>,
}

/// Light levels in percent (0–100). `warmth` is `None` without a warm LED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Light {
    pub brightness: u8,
    pub warmth: Option<u8>,
}

impl Light {
    /// Moves brightness (or warmth) by `delta` percent, clamped to 0–100.
    pub fn step(mut self, warmth: bool, delta: i16) -> Light {
        let step = |v: u8| (v as i16 + delta).clamp(0, 100) as u8;
        if warmth {
            self.warmth = self.warmth.map(step);
        } else {
            self.brightness = step(self.brightness);
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

/// Power-button and suspend notifications, from [`Device::power_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    /// The user pressed the power button (or the sleep timer fired): draw the sleep screen
    /// now; the device suspends on its own a few seconds later. Touch is ignored and the
    /// light is off until [`PowerEvent::Wake`].
    Sleep,
    /// Back from sleep: redraw the screen that was showing. Light and touch are restored.
    Wake,
    /// The device suspends in `delay` seconds (informational).
    Suspend { delay: u32 },
    /// Woke from suspend but still in the sleep screen, e.g. a timer (informational).
    Resumed,
    /// A computer is connected over USB (true) or went away (false).
    Usb(bool),
}

#[derive(Debug)]
pub enum Event {
    Touch(Touch),
    /// The host asked us to quit (emulator window closed).
    Quit,
}

pub trait Device {
    fn name(&self) -> &str;
    fn dpi(&self) -> u32;
    /// Screen size in pixels.
    fn size(&self) -> (i32, i32);
    fn canvas(&mut self) -> Canvas<'_>;
    /// Submits a refresh; returns a marker for [`Device::wait_refresh`].
    fn refresh(&mut self, r: &Refresh) -> io::Result<u32>;
    fn wait_refresh(&mut self, marker: u32) -> io::Result<()>;
    /// Blocks until input arrives or `timeout` passes; appends events to `out`.
    // ponytail: no worker wake-up yet; add an eventfd waker when the render worker lands (sub-project 2).
    fn wait_events(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()>;
    fn light(&self) -> Option<Light>;
    fn set_light(&mut self, light: Light) -> io::Result<()>;
    fn battery(&self) -> Option<Battery>;
    /// Next pending power event, if any. [`Device::wait_events`] returns early when one
    /// arrives, so poll this after every wait.
    fn power_event(&mut self) -> Option<PowerEvent> {
        None
    }
    /// Asks the device to go to sleep, as a power-button press would (sleep timer).
    fn sleep(&mut self) -> io::Result<()> {
        Ok(())
    }
    /// The Wi-Fi, when the device has one we can drive.
    fn wifi(&self) -> Option<Arc<dyn Wifi>> {
        None
    }
}

/// The backend this build targets: the desktop emulator with the `emulator` feature, else the
/// device [`platform()`] detects.
pub fn open_default() -> io::Result<Box<dyn Device>> {
    #[cfg(feature = "emulator")]
    return Ok(Box::new(emulator::Emulator::new()?));
    #[cfg(not(feature = "emulator"))]
    return match platform().kind {
        Kind::Kobo => Ok(Box::new(kobo::Kobo::open()?)),
        Kind::Kindle => Ok(Box::new(kindle::Kindle::open()?)),
        Kind::Desktop => {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no e-reader here: build with --features emulator"))
        }
    };
}
