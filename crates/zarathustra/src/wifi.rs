//! The Wi-Fi screen, opened from the quick panel's Wi-Fi tile: the radio switch with the
//! network joined, the saved networks (tap to join, "Forget"), the networks in range (tap to
//! join, a password first for a new secured one) and a rescan.
//!
//! Scans and connections take seconds: each runs on a worker thread while the screen says
//! what it is doing; [`WifiScreen::poll`] takes the result in. One operation at a time.

use std::io;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use zarathustra_device::wifi::{Network, WifiStatus};
use zarathustra_device::{Canvas, Rect, Wifi};

use crate::i18n::t;
use crate::keyboard::{Key, Keyboard};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::ui::{button, rounded_outline, rounded_rect, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const BACK_W: i32 = 400;
const SWITCH_ROW: Rect = Rect { x: 0, y: TOPBAR_H, w: W, h: 160 };
const SWITCH: Rect = Rect { x: W - PAD - 132, y: TOPBAR_H + 44, w: 132, h: 72 };
const HEADER_H: i32 = 80;
const ROW_H: i32 = 112;
/// Saved networks shown; the rest of the screen lists those in range.
// ponytail: no scrolling; past this many saved networks (or the screen's worth in range)
// the extra ones are not shown. Add paging when someone has that many.
const MAX_SAVED: usize = 3;
const ACTION_W: i32 = 220;
/// The longest WPA passphrase.
const MAX_PSK: usize = 63;
/// Password entry: the field (with the show/hide eye at its right end) and the connect button.
const FIELD: Rect = Rect { x: PAD, y: TOPBAR_H + 100, w: W - 2 * PAD, h: 112 };
const EYE: Rect = Rect { x: W - PAD - 112, y: TOPBAR_H + 100, w: 112, h: 112 };
const CONNECT: Rect = Rect { x: (W - 400) / 2, y: TOPBAR_H + 270, w: 400, h: 100 };

const FULL: Rect = Rect { x: 0, y: 0, w: W, h: H };

/// What is running on the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Busy {
    /// Status, saved networks and a scan, on opening, a rescan or the radio turned on.
    Scanning,
    Switching,
    Connecting(String),
    Forgetting,
}

/// What a worker brings back; `None` for what it didn't read (or failed to).
struct Done {
    error: Option<String>,
    status: Option<WifiStatus>,
    saved: Option<Vec<String>>,
    networks: Option<Vec<Network>>,
}

struct Entry {
    ssid: String,
    password: String,
    show: bool,
    keyboard: Keyboard,
}

pub enum Tap {
    None,
    /// Redraw, refreshing this area.
    Redraw(Rect),
    /// Back to the quick panel.
    Back,
}

pub struct WifiScreen {
    wifi: Arc<dyn Wifi>,
    status: WifiStatus,
    saved: Vec<String>,
    networks: Vec<Network>,
    busy: Option<(Busy, Receiver<Done>)>,
    error: Option<String>,
    entry: Option<Entry>,
}

impl WifiScreen {
    /// Opens the screen and starts reading the state and scanning.
    pub fn open(wifi: Arc<dyn Wifi>, status: WifiStatus) -> WifiScreen {
        let mut s =
            WifiScreen { wifi, status, saved: Vec::new(), networks: Vec::new(), busy: None, error: None, entry: None };
        s.start(Busy::Scanning, true, |_| Ok(()));
        s
    }

    /// The radio state and network as last read, for the quick panel's tile.
    pub fn status(&self) -> &WifiStatus {
        &self.status
    }

    /// An operation is running (poll often).
    pub fn busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Runs `op` on a worker, then reads the state back (and scans when `scan`).
    fn start(&mut self, busy: Busy, scan: bool, op: impl FnOnce(&dyn Wifi) -> io::Result<()> + Send + 'static) {
        if self.busy.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let wifi = self.wifi.clone();
        let failed = if matches!(busy, Busy::Connecting(_)) { t().wifi_failed } else { t().wifi_error };
        let spawned = std::thread::Builder::new().name("wifi".into()).spawn(move || {
            let mut error = op(&*wifi).err().map(|e| format!("{failed}: {e}"));
            let status = read(wifi.status(), &mut error);
            let saved = read(wifi.saved(), &mut error);
            let on = status.as_ref().is_some_and(|s| s.enabled);
            let networks = if scan && on { read(wifi.scan(), &mut error) } else { None };
            // The receiver is gone when the screen closed meanwhile: nothing to tell.
            let _ = tx.send(Done { error, status, saved, networks });
        });
        match spawned {
            Ok(_) => {
                self.error = None;
                self.busy = Some((busy, rx));
            }
            Err(e) => self.error = Some(format!("{}: {e}", t().wifi_error)),
        }
    }

    /// Takes in a finished operation; true if the screen changed.
    pub fn poll(&mut self) -> bool {
        let Some((_, rx)) = &self.busy else { return false };
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                Done { error: Some(t().wifi_error.into()), status: None, saved: None, networks: None }
            }
        };
        self.busy = None;
        self.error = done.error;
        if let Some(s) = done.status {
            self.status = s;
        }
        if let Some(s) = done.saved {
            self.saved = s;
        }
        if let Some(n) = done.networks {
            self.networks = n;
        }
        if !self.status.enabled {
            self.networks.clear();
        }
        true
    }

    fn saved_shown(&self) -> &[String] {
        &self.saved[..self.saved.len().min(MAX_SAVED)]
    }

    /// Networks in range that aren't saved, as many as fit.
    fn available(&self) -> Vec<&Network> {
        let rows = ((H - 20 - self.available_top()) / ROW_H).max(0) as usize;
        self.networks.iter().filter(|n| !self.saved.contains(&n.ssid)).take(rows).collect()
    }

    fn saved_top(&self) -> i32 {
        SWITCH_ROW.bottom() + HEADER_H
    }

    fn available_header(&self) -> i32 {
        self.saved_top() + self.saved_shown().len().max(1) as i32 * ROW_H + 24
    }

    fn available_top(&self) -> i32 {
        self.available_header() + HEADER_H
    }

    fn rescan_button(&self) -> Rect {
        Rect::new(W - PAD - ACTION_W, self.available_header() + 8, ACTION_W, 64)
    }

    /// Row `i` of the networks in range (for `--pgm` renders).
    pub fn available_row(&self, i: usize) -> Rect {
        Self::row(self.available_top(), i)
    }

    fn row(top: i32, i: usize) -> Rect {
        Rect::new(0, top + i as i32 * ROW_H, W, ROW_H)
    }

    fn forget_button(row: Rect) -> Rect {
        Rect::new(W - PAD - ACTION_W, row.y + (ROW_H - 72) / 2, ACTION_W, 72)
    }

    /// The finger went down: on the password keyboard, the key shows pressed.
    pub fn press(&mut self, x: i32, y: i32) -> Option<Rect> {
        self.entry.as_mut()?.keyboard.press(x, y)
    }

    pub fn tap(&mut self, x: i32, y: i32) -> Tap {
        if self.entry.is_some() {
            return self.entry_tap(x, y);
        }
        if y < TOPBAR_H {
            return if x < BACK_W { Tap::Back } else { Tap::None };
        }
        if self.busy.is_some() {
            return Tap::None;
        }
        if SWITCH_ROW.contains(x, y) {
            let on = !self.status.enabled;
            self.start(Busy::Switching, on, move |w| w.set_enabled(on));
            return Tap::Redraw(FULL);
        }
        if self.status.enabled && self.rescan_button().contains(x, y) {
            self.start(Busy::Scanning, true, |_| Ok(()));
            return Tap::Redraw(FULL);
        }
        let saved = (0..self.saved_shown().len()).find(|&i| Self::row(self.saved_top(), i).contains(x, y));
        if let Some(i) = saved {
            let ssid = self.saved[i].clone();
            if Self::forget_button(Self::row(self.saved_top(), i)).contains(x, y) {
                self.start(Busy::Forgetting, false, move |w| w.forget(&ssid));
            } else if self.status.connected.as_ref() != Some(&ssid) {
                self.join(ssid, None);
            } else {
                return Tap::None;
            }
            return Tap::Redraw(FULL);
        }
        let top = self.available_top();
        let hit = self.available().into_iter().enumerate().find(|(i, _)| Self::row(top, *i).contains(x, y));
        let Some((_, net)) = hit else { return Tap::None };
        let (ssid, secured) = (net.ssid.clone(), net.secured);
        if secured {
            let mut keyboard = Keyboard::default();
            keyboard.enter = Some(t().connect);
            self.entry = Some(Entry { ssid, password: String::new(), show: false, keyboard });
        } else {
            self.join(ssid, None);
        }
        Tap::Redraw(FULL)
    }

    fn join(&mut self, ssid: String, password: Option<String>) {
        let name = ssid.clone();
        self.start(Busy::Connecting(name), false, move |w| w.connect(&ssid, password.as_deref()));
    }

    fn entry_tap(&mut self, x: i32, y: i32) -> Tap {
        let Some(e) = self.entry.as_mut() else { return Tap::None };
        if y < TOPBAR_H {
            if x < BACK_W {
                self.entry = None;
                return Tap::Redraw(FULL);
            }
            return Tap::None;
        }
        if EYE.contains(x, y) {
            e.show = !e.show;
            return Tap::Redraw(FIELD);
        }
        let submit = if CONNECT.contains(x, y) {
            true
        } else if e.keyboard.holds(x, y) {
            let Some((key, rect)) = e.keyboard.tap(x, y) else { return Tap::None };
            match key {
                Key::Char(c) if e.password.chars().count() < MAX_PSK => e.password.push(c),
                Key::Space if e.password.chars().count() < MAX_PSK => e.password.push(' '),
                Key::Backspace => {
                    e.password.pop();
                }
                Key::Enter => return self.submit(),
                _ => return Tap::Redraw(rect),
            }
            return Tap::Redraw(FIELD.union(&rect));
        } else {
            false
        };
        if submit {
            self.submit()
        } else {
            Tap::None
        }
    }

    fn submit(&mut self) -> Tap {
        match self.entry.take() {
            Some(e) if !e.password.is_empty() => {
                self.join(e.ssid, Some(e.password));
                Tap::Redraw(FULL)
            }
            other => {
                self.entry = other;
                Tap::None
            }
        }
    }

    /// The line under the switch: what is running, the last error, or the state.
    fn state_line(&self) -> String {
        let s = t();
        match (&self.busy, &self.error) {
            (Some((Busy::Connecting(ssid), _)), _) => s.wifi_connecting.replace("{}", ssid),
            (Some((Busy::Scanning, _)), _) if self.status.enabled => s.wifi_scanning.into(),
            (_, Some(e)) => e.clone(),
            _ => match (&self.status.enabled, &self.status.connected) {
                (false, _) => s.wifi_off.into(),
                (true, Some(net)) => s.wifi_connected.replace("{}", net),
                (true, None) => s.wifi_not_connected.into(),
            },
        }
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(sans_bold), Some(serif), Some(serif_bold)) = (
            crate::sans(fonts, false),
            crate::sans(fonts, true),
            crate::serif(fonts, false),
            crate::serif(fonts, true),
        ) else {
            return;
        };
        let s = t();
        if let Some(e) = &self.entry {
            topbar(c, raster, sans, serif_bold, s.back, &e.ssid);
            Label { face: sans, size: 28.0, ink: INK_SOFT }.draw(c, raster, s.password, PAD as f32, FIELD.y - 24);
            rounded_outline(c, FIELD, 20, 3, INK, PAPER);
            let text: String = if e.show { e.password.clone() } else { "•".repeat(e.password.chars().count()) };
            let field = Label { face: sans, size: 40.0, ink: INK };
            let width = (EYE.x - FIELD.x - 48) as f32;
            // The end of a long password stays in view.
            let shown: String = {
                let mut chars: Vec<char> = text.chars().collect();
                while !chars.is_empty() && field.width(&chars.iter().collect::<String>()) > width {
                    chars.remove(0);
                }
                chars.into_iter().collect()
            };
            field.draw(c, raster, &shown, (FIELD.x + 28) as f32, field.baseline(FIELD));
            eye(c, EYE, e.show);
            rounded_rect(c, CONNECT, CONNECT.h / 2, if e.password.is_empty() { LINE } else { INK });
            Label { face: sans_bold, size: 34.0, ink: PAPER }.centered(c, raster, s.connect, CONNECT);
            e.keyboard.render(c, ctx);
            return;
        }

        topbar(c, raster, sans, serif_bold, s.back, "Wi-Fi");
        let name = Label { face: serif, size: 38.0, ink: INK };
        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        Label { face: serif_bold, size: 40.0, ink: INK }.draw(c, raster, "Wi-Fi", PAD as f32, SWITCH_ROW.y + 70);
        let line = self.state_line();
        soft.draw(c, raster, &soft.fit(&line, (SWITCH.x - PAD - 40) as f32), PAD as f32, SWITCH_ROW.y + 118);
        switch(c, SWITCH, self.status.enabled);
        c.fill(Rect::new(PAD, SWITCH_ROW.bottom() - 2, W - 2 * PAD, 2), LINE);

        let header = Label { face: sans_bold, size: 26.0, ink: INK_SOFT };
        header.draw(c, raster, s.saved_networks, PAD as f32, self.saved_top() - 24);
        let in_range = |ssid: &str| self.networks.iter().find(|n| n.ssid == ssid);
        for (i, ssid) in self.saved_shown().iter().enumerate() {
            let r = Self::row(self.saved_top(), i);
            let forget = Self::forget_button(r);
            button(c, forget, false);
            Label { face: sans, size: 28.0, ink: INK }.centered(c, raster, s.forget, forget);
            let mut right = forget.x - 32;
            if let Some(n) = in_range(ssid) {
                right = signal(c, right, r, n);
            }
            let here = self.status.connected.as_ref() == Some(ssid);
            let face = Label { face: if here { serif_bold } else { serif }, ..name };
            let width = (right - PAD - 24) as f32;
            if here {
                face.draw(c, raster, &face.fit(ssid, width), PAD as f32, r.y + 54);
                soft.draw(c, raster, s.wifi_joined, PAD as f32, r.y + 92);
            } else {
                face.draw(c, raster, &face.fit(ssid, width), PAD as f32, r.y + 70);
            }
            c.fill(Rect::new(PAD, r.bottom() - 1, W - 2 * PAD, 1), LINE);
        }

        let h = self.available_header();
        header.draw(c, raster, s.available_networks, PAD as f32, h + 56);
        if !self.status.enabled {
            return;
        }
        let rescan = self.rescan_button();
        button(c, rescan, false);
        Label { face: sans, size: 28.0, ink: INK }.centered(c, raster, s.rescan, rescan);
        let available = self.available();
        if available.is_empty() && self.busy.is_none() {
            soft.draw(c, raster, s.no_networks, PAD as f32, self.available_top() + 60);
        }
        for (i, n) in available.into_iter().enumerate() {
            let r = Self::row(self.available_top(), i);
            let right = signal(c, W - PAD, r, n);
            name.draw(c, raster, &name.fit(&n.ssid, (right - PAD - 24) as f32), PAD as f32, r.y + 70);
            c.fill(Rect::new(PAD, r.bottom() - 1, W - 2 * PAD, 1), LINE);
        }
    }
}

/// The value read, or `None` with the error noted (the first one wins).
fn read<T>(r: io::Result<T>, error: &mut Option<String>) -> Option<T> {
    r.map_err(|e| {
        eprintln!("wifi: {e}");
        error.get_or_insert_with(|| format!("{}: {e}", t().wifi_error));
    })
    .ok()
}

/// Signal bars (and a lock for a secured network) ending at `right`, centred in `row`.
/// Returns where they start.
fn signal(c: &mut Canvas, right: i32, row: Rect, n: &Network) -> i32 {
    let bars = (n.signal as i32 + 24) / 25;
    let (bar_w, gap, base) = (10, 6, row.y + row.h / 2 + 20);
    let x0 = right - 4 * bar_w - 3 * gap;
    for i in 0..4 {
        let h = 12 + 8 * i;
        let color = if i < bars { INK } else { LINE };
        c.fill(Rect::new(x0 + i * (bar_w + gap), base - h, bar_w, h), color);
    }
    if !n.secured {
        return x0;
    }
    // A padlock: a ring for the shackle, a solid body over its lower half.
    let lx = x0 - 52;
    rounded_outline(c, Rect::new(lx + 5, base - 42, 22, 28), 11, 4, INK, PAPER);
    rounded_rect(c, Rect::new(lx, base - 24, 32, 24), 4, INK);
    lx
}

/// An on/off switch: a pill with a knob, solid black when on.
fn switch(c: &mut Canvas, r: Rect, on: bool) {
    let knob = r.h - 16;
    if on {
        rounded_rect(c, r, r.h / 2, INK);
        rounded_rect(c, Rect::new(r.right() - 8 - knob, r.y + 8, knob, knob), knob / 2, PAPER);
    } else {
        rounded_outline(c, r, r.h / 2, 3, INK, PAPER);
        rounded_rect(c, Rect::new(r.x + 8, r.y + 8, knob, knob), knob / 2, INK);
    }
}

/// The show/hide eye: an outlined almond with a pupil, struck through while hidden.
fn eye(c: &mut Canvas, r: Rect, shown: bool) {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    rounded_outline(c, Rect::new(cx - 30, cy - 18, 60, 36), 18, 4, INK, PAPER);
    rounded_rect(c, Rect::new(cx - 10, cy - 10, 20, 20), 10, INK);
    if !shown {
        for d in -26..=26 {
            c.fill(Rect::new(cx + d - 2, cy + d * 3 / 4 - 2, 5, 5), INK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use zarathustra_device::wifi::Fake;

    fn screen() -> WifiScreen {
        let mut s = WifiScreen::open(Arc::new(Fake::new(Duration::ZERO)), WifiStatus::default());
        settle(&mut s);
        s
    }

    fn settle(s: &mut WifiScreen) {
        let t0 = Instant::now();
        while s.busy() && t0.elapsed() < Duration::from_secs(5) {
            s.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!s.busy(), "the worker finished");
    }

    fn centre(r: Rect) -> (i32, i32) {
        (r.x + r.w / 2, r.y + r.h / 2)
    }

    fn type_text(s: &mut WifiScreen, text: &str) {
        for ch in text.chars() {
            let kb = &s.entry.as_ref().expect("entry").keyboard;
            let (_, r) = kb.keys().into_iter().find(|(k, _)| *k == Key::Char(ch)).expect("key");
            s.tap(centre(r).0, centre(r).1);
        }
    }

    #[test]
    fn lists_saved_and_available_and_joins_with_a_password() {
        let mut s = screen();
        assert_eq!(s.status.connected.as_deref(), Some("Casa"));
        assert_eq!(s.saved, ["Casa", "Ufficio"]);
        let names: Vec<&str> = s.available().iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["Vodafone-A1B2", "Biblioteca", "TIM-Ospiti"], "saved ones aren't listed twice");
        // A secured network asks for the password; a wrong one fails and says so.
        let row = WifiScreen::row(s.available_top(), 0);
        assert!(matches!(s.tap(centre(row).0, centre(row).1), Tap::Redraw(_)));
        assert!(s.entry.is_some());
        type_text(&mut s, "nope");
        s.tap(centre(CONNECT).0, centre(CONNECT).1);
        assert!(s.entry.is_none() && s.busy());
        assert!(s.state_line().contains("Vodafone-A1B2"), "connecting…");
        settle(&mut s);
        assert!(s.error.is_some(), "wrong password");
        assert_eq!(s.status.connected.as_deref(), Some("Casa"));
        // The right one joins and saves it.
        s.tap(centre(row).0, centre(row).1);
        type_text(&mut s, "password");
        assert_eq!(s.entry.as_ref().map(|e| e.password.as_str()), Some("password"));
        s.tap(centre(EYE).0, centre(EYE).1);
        assert!(s.entry.as_ref().is_some_and(|e| e.show));
        let enter = s.entry.as_ref().and_then(|e| e.keyboard.keys().into_iter().find(|(k, _)| *k == Key::Enter));
        let (_, enter) = enter.expect("enter");
        s.tap(centre(enter).0, centre(enter).1);
        settle(&mut s);
        assert_eq!(s.error, None);
        assert_eq!(s.status.connected.as_deref(), Some("Vodafone-A1B2"));
        assert!(s.saved.contains(&"Vodafone-A1B2".to_string()));
        // Forget the first saved network (Casa), then switch the radio off.
        let first = WifiScreen::row(s.saved_top(), 0);
        let forget = WifiScreen::forget_button(first);
        s.tap(centre(forget).0, centre(forget).1);
        settle(&mut s);
        assert!(!s.saved.contains(&"Casa".to_string()));
        s.tap(W / 2, SWITCH_ROW.y + 40);
        settle(&mut s);
        assert!(!s.status.enabled && s.networks.is_empty());
        assert!(matches!(s.tap(100, 60), Tap::Back));
    }

    #[test]
    fn an_open_network_joins_without_a_password() {
        let mut s = screen();
        let row = WifiScreen::row(s.available_top(), 1);
        s.tap(centre(row).0, centre(row).1);
        assert!(s.entry.is_none());
        settle(&mut s);
        assert_eq!(s.status.connected.as_deref(), Some("Biblioteca"));
    }
}
