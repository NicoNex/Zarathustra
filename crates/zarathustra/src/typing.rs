//! Refreshes while typing, pipelined. The panel takes a tenth of a second or more per
//! refresh, and drawing into an area still refreshing leaves gray blocks, so the rest of the
//! app waits for the last refresh before drawing anything. Typing can't afford that: each key
//! would wait for the field's gray refresh of the key before. Here the screen is drawn into a
//! frame of its own and copied to the panel one area at a time; an area that overlaps a
//! refresh still running waits, coalesced with later changes, while the others go at once.
//! So the key under the finger shows pressed even while the field is still settling.
//!
//! Call [`Typing::settle`] before drawing on the panel any other way.

use std::io;
use std::time::{Duration, Instant};

use zarathustra_device::{Canvas, Device, Rect, Refresh, Waveform};

use crate::keyboard::Typed;
use crate::{H, W};

/// How long a refresh is expected to take, after which its area may be drawn into again.
// ponytail: guesses for the PW5's MTK panel (DU about 120 ms, GL16 about 300 ms). Too short
// only costs a brief wait in the driver, too long a later field; measure on the device with
// the update marker's completion time if typing ever stutters.
fn duration(w: Waveform) -> Duration {
    Duration::from_millis(match w {
        Waveform::Fast | Waveform::Mono => 150,
        Waveform::Text => 350,
        Waveform::Full => 600,
    })
}

/// A refresh submitted from here: its area, marker and when it should be over.
#[derive(Debug, Clone, Copy)]
struct Flight {
    rect: Rect,
    marker: u32,
    done: Instant,
}

pub struct Typing {
    /// The screen as last drawn for typing; areas are copied from it to the panel.
    frame: Vec<u8>,
    flights: Vec<Flight>,
    /// Areas drawn into `frame` but not yet on the panel, waiting for a refresh over them.
    pending: Vec<(Rect, Waveform)>,
}

impl Default for Typing {
    fn default() -> Typing {
        Typing { frame: vec![0; (W * H) as usize], flights: Vec::new(), pending: Vec::new() }
    }
}

impl Typing {
    /// The frame to draw the screen into (whole, or the keys just changed).
    pub fn canvas(&mut self) -> Canvas<'_> {
        Canvas { width: W, height: H, stride: W as usize, pixels: &mut self.frame }
    }

    /// Puts what a touch on the keyboard changed (drawn into [`Typing::canvas`]) on the panel.
    pub fn typed(&mut self, dev: &mut dyn Device, marker: &mut u32, t: Typed) -> io::Result<()> {
        self.queue(t.keys, Waveform::Fast);
        self.queue(t.text, Waveform::Text);
        self.flush(dev, marker)
    }

    fn queue(&mut self, rect: Rect, waveform: Waveform) {
        if rect.is_empty() {
            return;
        }
        // An area already waiting with the same waveform grows to take this one in.
        match self.pending.iter_mut().find(|(r, w)| *w == waveform && !r.intersect(&rect).is_empty()) {
            Some((r, _)) => *r = r.union(&rect),
            None => self.pending.push((rect, waveform)),
        }
    }

    /// Puts on the panel the waiting areas no running refresh overlaps any more.
    pub fn flush(&mut self, dev: &mut dyn Device, marker: &mut u32) -> io::Result<()> {
        self.sync(dev, *marker)?;
        let now = Instant::now();
        let mut i = 0;
        while i < self.pending.len() {
            let (rect, _) = self.pending[i];
            if self.flights.iter().any(|f| f.done > now && !f.rect.intersect(&rect).is_empty()) {
                i += 1;
                continue;
            }
            let (rect, waveform) = self.pending.remove(i);
            self.show(dev, marker, rect, waveform)?;
        }
        Ok(())
    }

    /// Everything waiting goes on the panel and every refresh from here ends, so the rest of
    /// the app may draw anywhere. Blocks for at most one refresh.
    pub fn settle(&mut self, dev: &mut dyn Device, marker: &mut u32) -> io::Result<()> {
        if self.flights.is_empty() && self.pending.is_empty() {
            return Ok(());
        }
        self.sync(dev, *marker)?;
        for (rect, waveform) in std::mem::take(&mut self.pending) {
            self.show(dev, marker, rect, waveform)?;
        }
        for f in std::mem::take(&mut self.flights) {
            dev.wait_refresh(f.marker)?;
        }
        Ok(())
    }

    /// How long until a waiting area may go on the panel: wake up then and call
    /// [`Typing::flush`].
    pub fn due(&self) -> Option<Duration> {
        let now = Instant::now();
        self.pending
            .iter()
            .filter_map(|(rect, _)| {
                let busy = self.flights.iter().filter(|f| !f.rect.intersect(rect).is_empty());
                busy.map(|f| f.done.saturating_duration_since(now)).max()
            })
            .min()
            .or((!self.pending.is_empty()).then_some(Duration::ZERO))
    }

    /// A refresh from elsewhere (`marker` is not one of ours) may still be running anywhere:
    /// wait for it once, before drawing over it.
    fn sync(&mut self, dev: &mut dyn Device, marker: u32) -> io::Result<()> {
        if self.flights.last().is_none_or(|f| f.marker != marker) {
            dev.wait_refresh(marker)?;
            self.flights.clear();
        }
        Ok(())
    }

    /// Copies `rect` from the frame to the panel and refreshes it, once the refreshes
    /// overlapping it have ended.
    fn show(&mut self, dev: &mut dyn Device, marker: &mut u32, rect: Rect, waveform: Waveform) -> io::Result<()> {
        let rect = rect.intersect(&Rect::new(0, 0, W, H));
        for f in self.flights.iter().filter(|f| !f.rect.intersect(&rect).is_empty()) {
            dev.wait_refresh(f.marker)?;
        }
        // Waited for, the overlapping ones go; the others stay until settle() waits for them
        // too: the durations are estimates, and a slow gray refresh may outlive its own.
        self.flights.retain(|f| f.rect.intersect(&rect).is_empty());
        let now = Instant::now();
        {
            let c = dev.canvas();
            for y in rect.y..rect.bottom() {
                let (src, dst) = (y as usize * W as usize + rect.x as usize, y as usize * c.stride + rect.x as usize);
                c.pixels[dst..dst + rect.w as usize].copy_from_slice(&self.frame[src..src + rect.w as usize]);
            }
        }
        *marker = dev.refresh(&Refresh { rect, waveform, flash: false, swipe: None })?;
        self.flights.push(Flight { rect, marker: *marker, done: now + duration(waveform) });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zarathustra_device::{Battery, Event, Light, Wifi};

    /// Records refreshes and waits; the panel is a plain buffer.
    struct Panel {
        px: Vec<u8>,
        log: Vec<String>,
        marker: u32,
    }

    impl Device for Panel {
        fn name(&self) -> &str {
            "test"
        }
        fn dpi(&self) -> u32 {
            300
        }
        fn size(&self) -> (i32, i32) {
            (W, H)
        }
        fn canvas(&mut self) -> Canvas<'_> {
            Canvas { width: W, height: H, stride: W as usize, pixels: &mut self.px }
        }
        fn refresh(&mut self, r: &Refresh) -> io::Result<u32> {
            self.marker += 1;
            self.log.push(format!("refresh {} {:?} y{}", self.marker, r.waveform, r.rect.y));
            Ok(self.marker)
        }
        fn wait_refresh(&mut self, marker: u32) -> io::Result<()> {
            self.log.push(format!("wait {marker}"));
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
        fn wifi(&self) -> Option<Arc<dyn Wifi>> {
            None
        }
    }

    #[test]
    fn keys_go_at_once_and_an_area_still_refreshing_waits() {
        let mut dev = Panel { px: vec![255; (W * H) as usize], log: Vec::new(), marker: 7 };
        let mut typing = Typing::default();
        // The keyboard opened with a refresh from elsewhere: waited for once.
        let mut marker = 7;
        let (key, other, field) =
            (Rect::new(10, 1300, 100, 100), Rect::new(300, 1300, 100, 100), Rect::new(0, 100, W, 100));
        typing.canvas().fill(key, 0);
        typing.typed(&mut dev, &mut marker, Typed::keys(key)).expect("pressed");
        assert_eq!(dev.log, ["wait 7", "refresh 8 Fast y1300"]);
        assert_eq!(dev.px[1350 * W as usize + 50], 0, "copied from the frame");
        // Released at once, with the field: the key waits for its own refresh, the field goes.
        typing.typed(&mut dev, &mut marker, Typed::keys(key).and_text(field)).expect("released");
        assert_eq!(dev.log[2..], ["refresh 9 Text y100"]);
        assert!(typing.due().is_some_and(|d| d > Duration::ZERO));
        // Another key in the meantime goes straight through.
        typing.typed(&mut dev, &mut marker, Typed::keys(other)).expect("other key");
        assert_eq!(dev.log[3..], ["refresh 10 Fast y1300"]);
        assert_eq!(marker, 10);
        // Before drawing elsewhere: the waiting key goes, and every refresh is waited for.
        typing.settle(&mut dev, &mut marker).expect("settled");
        assert_eq!(dev.log[4..], ["wait 8", "refresh 11 Fast y1300", "wait 9", "wait 10", "wait 11"]);
        assert_eq!(typing.due(), None);
    }
}
