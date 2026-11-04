//! Desktop window standing in for a device: same canvas size, mouse = touch.

use std::io;
use std::time::{Duration, Instant};

use minifb::{Key, MouseButton, MouseMode, Window, WindowOptions};

use crate::{Battery, Canvas, Device, Event, Light, Phase, Refresh, Touch};

pub struct Emulator {
    window: Window,
    pixels: Vec<u8>,
    rgb: Vec<u32>,
    width: i32,
    height: i32,
    /// Window shows the device at 1/scale.
    scale: i32,
    down: bool,
    last: (i32, i32),
    light: Light,
}

impl Emulator {
    /// A Paperwhite-sized screen shown at 50%.
    pub fn new() -> io::Result<Self> {
        Self::with_size(1236, 1648, 2)
    }

    pub fn with_size(width: i32, height: i32, scale: i32) -> io::Result<Self> {
        let (ww, wh) = ((width / scale) as usize, (height / scale) as usize);
        let window = Window::new("Omero emulator", ww, wh, WindowOptions::default())
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Emulator {
            window,
            pixels: vec![255; (width * height) as usize],
            rgb: vec![0; ww * wh],
            width,
            height,
            scale,
            down: false,
            last: (0, 0),
            light: Light { brightness: 50, warmth: Some(0) },
        })
    }

    fn present(&mut self) -> io::Result<()> {
        let (ww, wh) = ((self.width / self.scale) as usize, (self.height / self.scale) as usize);
        let s = self.scale as usize;
        for y in 0..wh {
            for x in 0..ww {
                let g = self.pixels[y * s * self.width as usize + x * s] as u32;
                self.rgb[y * ww + x] = g << 16 | g << 8 | g;
            }
        }
        self.window.update_with_buffer(&self.rgb, ww, wh).map_err(|e| io::Error::other(e.to_string()))
    }

    fn adjust_light(&mut self, key: Key) {
        let step = |v: u8, d: i16| (v as i16 + d).clamp(0, 100) as u8;
        match key {
            Key::LeftBracket => self.light.brightness = step(self.light.brightness, -10),
            Key::RightBracket => self.light.brightness = step(self.light.brightness, 10),
            Key::Minus => self.light.warmth = self.light.warmth.map(|w| step(w, -10)),
            Key::Equal => self.light.warmth = self.light.warmth.map(|w| step(w, 10)),
            _ => return,
        }
        eprintln!("emulator light: {:?}", self.light);
    }
}

impl Device for Emulator {
    fn name(&self) -> &str {
        "Emulator"
    }

    fn dpi(&self) -> u32 {
        300
    }

    fn canvas(&mut self) -> Canvas<'_> {
        Canvas { width: self.width, height: self.height, stride: self.width as usize, pixels: &mut self.pixels }
    }

    fn refresh(&mut self, _r: &Refresh) -> io::Result<u32> {
        // ponytail: shows the whole frame instantly; simulate e-ink latency/ghosting if it ever matters for tuning.
        self.present()?;
        Ok(1)
    }

    fn wait_refresh(&mut self, _marker: u32) -> io::Result<()> {
        Ok(())
    }

    fn wait_events(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            self.window.update();
            if !self.window.is_open() {
                out.push(Event::Quit);
                return Ok(());
            }
            for key in self.window.get_keys_pressed(minifb::KeyRepeat::No) {
                self.adjust_light(key);
            }
            let pressed = self.window.get_mouse_down(MouseButton::Left);
            if let Some((mx, my)) = self.window.get_mouse_pos(MouseMode::Clamp) {
                let (x, y) = (mx as i32 * self.scale, my as i32 * self.scale);
                let moved = (x, y) != self.last;
                self.last = (x, y);
                let phase = match (pressed, self.down) {
                    (true, false) => Some(Phase::Down),
                    (true, true) if moved => Some(Phase::Move),
                    (true, true) => None,
                    (false, true) => Some(Phase::Up),
                    (false, false) => None,
                };
                self.down = pressed;
                if let Some(phase) = phase {
                    out.push(Event::Touch(Touch { id: 0, phase, x, y }));
                    return Ok(());
                }
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    fn light(&self) -> Option<Light> {
        Some(self.light)
    }

    fn set_light(&mut self, light: Light) -> io::Result<()> {
        self.light = light;
        Ok(())
    }

    fn battery(&self) -> Option<Battery> {
        Some(Battery { percent: 86, charging: false })
    }
}
