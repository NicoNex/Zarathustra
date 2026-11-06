//! Night mode end to end: a picture blitted through `image::draw` reaches the panel as it
//! is, while UI ink around it reaches it inverted. Its own test binary, since night mode is
//! a process-wide flag.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zarathustra_device::{set_night, Battery, Device, Event, Light, Night, Rect, Refresh, Waveform};
use zarathustra_engine::image::{self, GrayImage};

const W: i32 = 4;

/// A one-row panel that records the pixels each refresh sent out.
struct Panel {
    px: Vec<u8>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Device for Panel {
    fn name(&self) -> &str {
        "panel"
    }
    fn dpi(&self) -> u32 {
        300
    }
    fn size(&self) -> (i32, i32) {
        (W, 1)
    }
    fn canvas(&mut self) -> zarathustra_device::Canvas<'_> {
        zarathustra_device::Canvas { width: W, height: 1, stride: W as usize, pixels: &mut self.px }
    }
    fn refresh(&mut self, _: &Refresh) -> io::Result<u32> {
        self.sent.lock().unwrap().push(self.px.clone());
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
fn night_inverts_ui_ink_but_not_pictures() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut dev = Night::new(Box::new(Panel { px: vec![255; W as usize], sent: sent.clone() }));
    let picture = GrayImage { width: 2, height: 1, pixels: vec![34, 200] };
    let r = Refresh { rect: Rect::new(0, 0, W, 1), waveform: Waveform::Full, flash: false, swipe: None };
    let paint = |dev: &mut Night| {
        let mut c = dev.canvas();
        c.fill(Rect::new(0, 0, W, 1), 255); // paper
        c.fill(Rect::new(0, 0, 1, 1), 0); // a stroke of text
        image::draw(&mut c, &picture, 2, 0);
    };

    paint(&mut dev);
    dev.refresh(&r).unwrap();
    set_night(true);
    paint(&mut dev);
    dev.refresh(&r).unwrap();
    set_night(false);

    let sent = sent.lock().unwrap();
    assert_eq!(sent[0], vec![0, 255, 34, 200], "day: as drawn");
    assert_eq!(sent[1], vec![255, 0, 34, 200], "night: text and paper inverted, the picture as it is");
}
