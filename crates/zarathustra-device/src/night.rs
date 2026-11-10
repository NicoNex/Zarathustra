//! Night mode: white on black, for any backend.
//!
//! Screens draw as usual; while `on`, a refresh inverts the framebuffer just before it goes
//! to the panel, and the next drawing access (or refresh with the mode off) waits for that
//! refresh and turns it back. Drawing code never sees inverted pixels, so partial redraws
//! (a panel over a page, a key under a finger) keep working unchanged.
//!
//! Pictures (covers, book images, PDF pages) must look as they are, not as negatives: their
//! blit asks [`night`] and draws them pre-inverted, so the frame's inversion turns them back.
// ponytail: inverts the whole frame (2 MB) twice per refresh, a few ms on the PW5. Use the
// controller's own inversion flag if the MTK driver turns out to have a working one.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::{Battery, Canvas, Device, Event, Light, PowerEvent, Rect, Refresh, Wifi};

/// Night mode, process-wide: one panel, one mode. A global so the image blit can ask without
/// threading a flag through every screen's render.
static ON: AtomicBool = AtomicBool::new(false);

/// Whether night mode is on: refreshes go out inverted.
pub fn night() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Turns night mode on or off. The screen's pictures were drawn for the old mode, so the
/// caller repaints the whole screen after a change.
pub fn set_night(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

pub struct Night {
    inner: Box<dyn Device>,
    /// Areas inverted for refreshes still to be undone, each with its refresh's marker.
    inverted: Vec<(Rect, u32)>,
}

impl Night {
    pub fn new(inner: Box<dyn Device>) -> Night {
        Night { inner, inverted: Vec::new() }
    }

    /// Inverts only the refreshed area: a key press costs its key, not the 2 MB frame.
    fn invert(&mut self, r: Rect) {
        let c = self.inner.canvas();
        let r = r.intersect(&c.bounds());
        for y in r.y..r.bottom() {
            let start = y as usize * c.stride + r.x as usize;
            for p in &mut c.pixels[start..start + r.w as usize] {
                *p = !*p;
            }
        }
    }

    /// Back to the pixels as drawn once the panel has read the inverted ones; put back even
    /// when a wait fails, so drawing never lands on inverted pixels.
    // ponytail: waits for every pending refresh, so typing in night mode is serialized again;
    // drawing in night colours (a flag on Canvas) would remove the inversion and the wait.
    fn restore(&mut self) -> io::Result<()> {
        let mut result = Ok(());
        let wait = !self.inner.draws_over_refresh();
        for (r, marker) in std::mem::take(&mut self.inverted) {
            if let Err(e) = if wait { self.inner.wait_refresh(marker) } else { Ok(()) } {
                result = Err(e);
            }
            self.invert(r);
        }
        result
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
        if !night() {
            self.restore()?;
            return self.inner.refresh(r);
        }
        // An area still inverted would flip back: undo those first.
        if self.inverted.iter().any(|(done, _)| !done.intersect(&r.rect).is_empty()) {
            self.restore()?;
        }
        self.invert(r.rect);
        let marker = self.inner.refresh(r)?;
        self.inverted.push((r.rect, marker));
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
    fn draws_over_refresh(&self) -> bool {
        self.inner.draws_over_refresh()
    }
    fn page_animation(&self) -> bool {
        self.inner.page_animation()
    }
    fn has_sleep_cover(&self) -> bool {
        self.inner.has_sleep_cover()
    }
    fn set_sleep_cover(&mut self, on: bool) {
        self.inner.set_sleep_cover(on)
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
        fn draws_over_refresh(&self) -> bool {
            true
        }
        fn page_animation(&self) -> bool {
            true
        }
        fn has_sleep_cover(&self) -> bool {
            true
        }
    }

    #[test]
    fn forwards_the_capabilities() {
        let n = Night::new(Box::new(Panel { px: vec![255; 4], sent: Arc::default() }));
        assert!(n.draws_over_refresh() && n.page_animation() && n.has_sleep_cover());
    }

    #[test]
    fn refreshes_inverted_while_drawing_sees_the_original() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut n = Night::new(Box::new(Panel { px: vec![255; 4], sent: sent.clone() }));
        let r = Refresh { rect: Rect::new(0, 0, 4, 1), waveform: Waveform::Text, flash: false, swipe: None };
        n.refresh(&r).unwrap();
        set_night(true);
        n.refresh(&r).unwrap();
        n.refresh(&r).unwrap(); // nothing drawn in between: still inverted, not twice
        assert_eq!(n.canvas().pixels[0], 255, "drawing code sees what it drew");
        n.canvas().pixels[0] = 10;
        n.refresh(&r).unwrap();
        set_night(false);
        n.refresh(&r).unwrap();
        assert_eq!(*sent.lock().unwrap(), vec![255, 0, 0, 245, 10]);
    }
}
