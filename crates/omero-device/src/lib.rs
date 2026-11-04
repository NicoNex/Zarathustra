//! Hardware abstraction: everything device-specific lives behind [`Device`].

pub mod canvas;
pub mod geom;
pub mod input;
pub mod kindle;
mod sys;
pub mod sysfs;

#[cfg(feature = "emulator")]
pub mod emulator;

use std::io;
use std::time::Duration;

pub use canvas::Canvas;
pub use geom::Rect;
pub use input::{Phase, Touch};

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
}

/// The backend this build targets: the desktop emulator with the `emulator` feature, else the Kindle.
pub fn open_default() -> io::Result<Box<dyn Device>> {
    #[cfg(feature = "emulator")]
    return Ok(Box::new(emulator::Emulator::new()?));
    #[cfg(not(feature = "emulator"))]
    return Ok(Box::new(kindle::Kindle::open()?));
}
