//! Night mode: white on black, for any backend.
//!
//! Screens draw as usual; while `on`, a refresh inverts the framebuffer just before it goes
//! to the panel, and the next drawing access (or refresh with the mode off) waits for that
//! refresh and turns it back. Drawing code never sees inverted pixels, so partial redraws
//! (a panel over a page, a key under a finger) keep working unchanged.
// ponytail: inverts the whole frame (2 MB) twice per refresh, a few ms on the PW5. Use the
// controller's own inversion flag if the MTK driver turns out to have a working one.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crate::{Battery, Canvas, Device, Event, Light, PowerEvent, Refresh, Wifi};

pub struct Night {
    inner: Box<dyn Device>,
    pub on: bool,
    /// The framebuffer is inverted for the refresh with this marker.
    inverted: Option<u32>,
}

impl Night {
    pub fn new(inner: Box<dyn Device>) -> Night {
        Night { inner, on: false, inverted: None }
    }

    fn invert(&mut self) {
        for p in self.inner.canvas().pixels.iter_mut() {
            *p = !*p;
        }
    }

    /// Back to the pixels as drawn, once the panel has read the inverted ones.
    fn restore(&mut self) -> io::Result<()> {
        if let Some(marker) = self.inverted.take() {
            self.inner.wait_refresh(marker)?;
            self.invert();
        }
        Ok(())
    }
}

impl Device for Night {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn dpi(&self) -> u32 {
        self.inner.dpi()
    }
    fn size(&self) -> (i32, i32) {
        self.inner.size()
    }

    fn canvas(&mut self) -> Canvas<'_> {
        if let Err(e) = self.restore() {
            eprintln!("night: {e}");
        }
        self.inner.canvas()
    }

    fn refresh(&mut self, r: &Refresh) -> io::Result<u32> {
        if !self.on {
            self.restore()?;
            return self.inner.refresh(r);
        }
        // Already inverted when nothing was drawn since the last refresh.
        if self.inverted.is_none() {
            self.invert();
        }
        let marker = self.inner.refresh(r)?;
        self.inverted = Some(marker);
        Ok(marker)
    }

    fn wait_refresh(&mut self, marker: u32) -> io::Result<()> {
        self.inner.wait_refresh(marker)
    }
    fn wait_events(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) -> io::Result<()> {
        self.inner.wait_events(timeout, out)
    }
    fn light(&self) -> Option<Light> {
        self.inner.light()
    }
    fn set_light(&mut self, light: Light) -> io::Result<()> {
        self.inner.set_light(light)
    }
    fn battery(&self) -> Option<Battery> {
        self.inner.battery()
    }
    fn power_event(&mut self) -> Option<PowerEvent> {
        self.inner.power_event()
    }
    fn sleep(&mut self) -> io::Result<()> {
        self.inner.sleep()
    }
    fn wifi(&self) -> Option<Arc<dyn Wifi>> {
        self.inner.wifi()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Rect, Waveform};
    use std::sync::Mutex;

    /// Records what each refresh sent to the "panel": the first pixel.
    struct Panel {
        px: Vec<u8>,
        sent: Arc<Mutex<Vec<u8>>>,
    }

    impl Device for Panel {
        fn name(&self) -> &str {
            "panel"
        }
        fn dpi(&self) -> u32 {
            300
        }
        fn size(&self) -> (i32, i32) {
            (4, 1)
        }
        fn canvas(&mut self) -> Canvas<'_> {
            Canvas { width: 4, height: 1, stride: 4, pixels: &mut self.px }
        }
        fn refresh(&mut self, _: &Refresh) -> io::Result<u32> {
            self.sent.lock().unwrap().push(self.px[0]);
            Ok(1)
        }
        fn wait_refresh(&mut self, _: u32) -> io::Result<()> {
            Ok(())
        }
        fn wait_events(&mut self, _: Option<Duration>, _: &mut Vec<Event>) -> io::Result<()> {
            Ok(())
        }
        fn light(&self) -> Option<Light> {
            None
        }
        fn set_light(&mut self, _: Light) -> io::Result<()> {
            Ok(())
        }
        fn battery(&self) -> Option<Battery> {
            None
        }
    }

    #[test]
    fn refreshes_inverted_while_drawing_sees_the_original() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut n = Night::new(Box::new(Panel { px: vec![255; 4], sent: sent.clone() }));
        let r = Refresh { rect: Rect::new(0, 0, 4, 1), waveform: Waveform::Text, flash: false, swipe: None };
        n.refresh(&r).unwrap();
        n.on = true;
        n.refresh(&r).unwrap();
        n.refresh(&r).unwrap(); // nothing drawn in between: still inverted, not twice
        assert_eq!(n.canvas().pixels[0], 255, "drawing code sees what it drew");
        n.canvas().pixels[0] = 10;
        n.refresh(&r).unwrap();
        n.on = false;
        n.refresh(&r).unwrap();
        assert_eq!(*sent.lock().unwrap(), vec![255, 0, 0, 245, 10]);
    }
}
