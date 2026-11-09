//! Desktop window standing in for a device: same canvas size, mouse = touch.

use std::io;
use std::time::{Duration, Instant};

use minifb::{Key, MouseButton, MouseMode, Window, WindowOptions};

use crate::input::MtDecoder;
use crate::{Battery, Canvas, Device, Event, Light, Refresh};

pub struct Emulator {
    window: Window,
    pixels: Vec<u8>,
    rgb: Vec<u32>,
    width: i32,
    height: i32,
    /// Window shows the device at 1/scale.
    scale: i32,
    /// Window size in pixels.
    win: (usize, usize),
    /// Mouse state is fed through the real touch decoder, as protocol-B events.
    decoder: MtDecoder,
    down: bool,
    last: (i32, i32),
    light: Light,
    wifi: std::sync::Arc<crate::wifi::Fake>,
}

const EV_SYN: u16 = 0;
const EV_ABS: u16 = 3;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;

impl Emulator {
    /// A Paperwhite-sized screen shown at 50%, or the size in `ZARATHUSTRA_SIZE` (`1072x1448`
    /// for a Kobo Clara).
    pub fn new() -> io::Result<Self> {
        let size = std::env::var("ZARATHUSTRA_SIZE").ok().and_then(|s| crate::sys::parse_size(&s));
        let (w, h) = size.unwrap_or((1236, 1648));
        Self::with_size(w, h, 2)
    }

    pub fn with_size(width: i32, height: i32, scale: i32) -> io::Result<Self> {
        let win = ((width / scale) as usize, (height / scale) as usize);
        let window = Window::new("Zarathustra emulator", win.0, win.1, WindowOptions::default())
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Emulator {
            window,
            pixels: vec![255; (width * height) as usize],
            rgb: vec![0; win.0 * win.1],
            width,
            height,
            scale,
            win,
            decoder: MtDecoder::default(),
            down: false,
            last: (0, 0),
            light: Light { brightness: 50, warmth: Some(0) },
            wifi: std::sync::Arc::new(crate::wifi::Fake::new(Duration::from_secs(2))),
        })
    }

    fn present(&mut self) -> io::Result<()> {
        // ponytail: converts the whole frame on every refresh; blit only the refresh rect if the emulator gets slow.
        let (ww, wh) = self.win;
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
        self.light = match key {
            Key::LeftBracket => self.light.step(false, -10),
            Key::RightBracket => self.light.step(false, 10),
            Key::Minus => self.light.step(true, -10),
            Key::Equal => self.light.step(true, 10),
            _ => return,
        };
        eprintln!("emulator light: {:?}", self.light);
    }
}

impl Device for Emulator {
    /// Stands in for the Kindle the layouts were first made for.
    fn page_animation(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Emulator"
    }

    fn dpi(&self) -> u32 {
        300
    }

    fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    fn canvas(&mut self) -> Canvas<'_> {
        Canvas { width: self.width, height: self.height, stride: self.width as usize, pixels: &mut self.pixels }
    }

    fn refresh(&mut self, _r: &Refresh) -> io::Result<u32> {
        // ponytail: shows the frame instantly; simulate e-ink latency/ghosting if it ever matters for tuning.
        self.present()?;
        Ok(1)
    }

    fn wait_refresh(&mut self, _marker: u32) -> io::Result<()> {
        Ok(())
    }

    fn wait_events(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()> {
        // ponytail: polls the mouse every 8 ms, so a click shorter than that can be missed; fine for a dev tool.
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut touches = Vec::new();
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
                let pos = (mx as i32 * self.scale, my as i32 * self.scale);
                if pressed != self.down || (pressed && pos != self.last) {
                    let feed = |d: &mut MtDecoder, code, value, out: &mut Vec<_>| d.feed(EV_ABS, code, value, out);
                    feed(&mut self.decoder, ABS_MT_TRACKING_ID, if pressed { 0 } else { -1 }, &mut touches);
                    feed(&mut self.decoder, ABS_MT_POSITION_X, pos.0, &mut touches);
                    feed(&mut self.decoder, ABS_MT_POSITION_Y, pos.1, &mut touches);
                    self.decoder.feed(EV_SYN, 0, 0, &mut touches);
                    (self.down, self.last) = (pressed, pos);
                }
            }
            if !touches.is_empty() {
                out.extend(touches.into_iter().map(Event::Touch));
                return Ok(());
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

    fn wifi(&self) -> Option<std::sync::Arc<dyn crate::Wifi>> {
        Some(self.wifi.clone())
    }
}
