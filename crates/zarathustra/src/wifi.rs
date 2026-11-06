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
use crate::keyboard::{Key, Keyboard, Typed};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::ui::{button, rounded_outline, rounded_rect, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const BACK_W: i32 = 400;
const SWITCH_ROW: Rect = Rect { x: 0, y: TOPBAR_H, w: W, h: 160 };
const SWITCH: Rect = Rect { x: W - PAD - 132, y: TOPBAR_H + 44, w: 132, h: 72 };
const HEADER_H: i32 = 96;
const ROW_H: i32 = 112;
/// The page arrows under the list, when it takes more than one page.
const PAGER: Rect = Rect { x: 0, y: H - 110, w: W, h: 110 };
/// The list under the switch, paged rather than scrolled: e-ink redraws a page in one go.
const LIST_TOP: i32 = SWITCH_ROW.y + SWITCH_ROW.h;
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

/// One line of the list under the switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    SavedHeader,
    /// Index into `saved`.
    Saved(usize),
    /// "Networks available" with the rescan button.
    AvailableHeader,
    /// Index into [`WifiScreen::available`].
    Available(usize),
    NoNetworks,
}

impl Row {
    fn height(self) -> i32 {
        match self {
            Row::SavedHeader | Row::AvailableHeader => HEADER_H,
            _ => ROW_H,
        }
    }
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
    /// Typing the password: redraw, refreshing what the keyboard says.
    Typed(Typed),
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
    page: usize,
}

impl WifiScreen {
    /// Opens the screen and starts reading the state and scanning.
    pub fn open(wifi: Arc<dyn Wifi>, status: WifiStatus) -> WifiScreen {
        let mut s = WifiScreen {
            wifi,
            status,
            saved: Vec::new(),
            networks: Vec::new(),
            busy: None,
            error: None,
            entry: None,
            page: 0,
        };
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

    /// Networks in range that aren't saved.
    fn available(&self) -> Vec<&Network> {
        self.networks.iter().filter(|n| !self.saved.contains(&n.ssid)).collect()
    }

    /// The list split into pages that fit above the arrows, each row with its rect.
    fn pages(&self) -> Vec<Vec<(Row, Rect)>> {
        let mut rows = Vec::new();
        if !self.saved.is_empty() {
            rows.push(Row::SavedHeader);
            rows.extend((0..self.saved.len()).map(Row::Saved));
        }
        rows.push(Row::AvailableHeader);
        if self.status.enabled {
            let available = self.available().len();
            if available == 0 && self.busy.is_none() {
                rows.push(Row::NoNetworks);
            }
            rows.extend((0..available).map(Row::Available));
        }
        let mut pages = vec![Vec::new()];
        let mut y = LIST_TOP;
        for row in rows {
            // A header never ends a page: it keeps a row's room under it.
            let room = row.height() + if row.height() == HEADER_H { ROW_H } else { 0 };
            if y + room > PAGER.y && pages.last().is_some_and(|p: &Vec<_>| !p.is_empty()) {
                pages.push(Vec::new());
                y = LIST_TOP;
            }
            if let Some(page) = pages.last_mut() {
                page.push((row, Rect::new(0, y, W, row.height())));
            }
            y += row.height();
        }
        pages
    }

    /// The rows on the page shown, and how many pages there are.
    fn page(&self) -> (Vec<(Row, Rect)>, usize) {
        let mut pages = self.pages();
        let n = pages.len();
        (pages.swap_remove(self.page.min(n - 1)), n)
    }

    /// Turns the page (a swipe or an arrow); true if it changed.
    pub fn turn(&mut self, forward: bool) -> bool {
        let n = self.pages().len();
        let page = self.page.min(n - 1);
        let to = if forward { (page + 1).min(n - 1) } else { page.saturating_sub(1) };
        self.page = to;
        to != page
    }

    /// The password screen is up (swipes don't turn pages there).
    pub fn entering(&self) -> bool {
        self.entry.is_some()
    }

    /// Row `i` of the networks in range on the page shown (for `--pgm` renders).
    pub fn available_row(&self, i: usize) -> Rect {
        let (rows, _) = self.page();
        rows.into_iter().find(|(r, _)| *r == Row::Available(i)).map_or(Rect::new(0, 0, 0, 0), |(_, r)| r)
    }

    fn rescan_button(header: Rect) -> Rect {
        Rect::new(W - PAD - ACTION_W, header.y + 20, ACTION_W, 64)
    }

    fn forget_button(row: Rect) -> Rect {
        Rect::new(W - PAD - ACTION_W, row.y + (ROW_H - 72) / 2, ACTION_W, 72)
    }

    /// The finger went down: on the password keyboard, the key shows pressed.
    pub fn press(&mut self, x: i32, y: i32) -> Option<Rect> {
        self.entry.as_mut()?.keyboard.press(x, y)
    }

    /// A finger held on a key of the password keyboard: its accents. The area to refresh.
    pub fn long_press(&mut self, x: i32, y: i32) -> Option<Rect> {
        self.entry.as_mut()?.keyboard.long_press(x, y)
    }

    /// The password keyboard, while it is up.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.entry.as_ref().map(|e| &e.keyboard)
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
        let (rows, pages) = self.page();
        if pages > 1 && PAGER.contains(x, y) {
            return if self.turn(x >= W / 2) { Tap::Redraw(FULL) } else { Tap::None };
        }
        let Some(&(row, r)) = rows.iter().find(|(_, r)| r.contains(x, y)) else { return Tap::None };
        match row {
            Row::AvailableHeader if self.status.enabled && Self::rescan_button(r).contains(x, y) => {
                self.start(Busy::Scanning, true, |_| Ok(()));
            }
            Row::Saved(i) => {
                let ssid = self.saved[i].clone();
                if Self::forget_button(r).contains(x, y) {
                    self.start(Busy::Forgetting, false, move |w| w.forget(&ssid));
                } else if self.status.connected.as_ref() != Some(&ssid) {
                    self.join(ssid, None);
                } else {
                    return Tap::None;
                }
            }
            Row::Available(i) => {
                let Some(net) = self.available().get(i).copied() else { return Tap::None };
                let (ssid, secured) = (net.ssid.clone(), net.secured);
                if secured {
                    let mut keyboard = Keyboard::default();
                    keyboard.enter = Some(t().connect);
                    self.entry = Some(Entry { ssid, password: String::new(), show: false, keyboard });
                } else {
                    self.join(ssid, None);
                }
            }
            _ => return Tap::None,
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
            let Some((key, typed)) = e.keyboard.tap(x, y) else { return Tap::None };
            match key {
                Key::Char(c) if e.password.chars().count() < MAX_PSK => e.password.push(c),
                Key::Space if e.password.chars().count() < MAX_PSK => e.password.push(' '),
                Key::Backspace => {
                    e.password.pop();
                }
                Key::Enter => return self.submit(),
                _ => return Tap::Typed(typed),
            }
            // The connect button greys out or in with the password.
            return Tap::Typed(typed.and_text(FIELD.union(&CONNECT)));
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
        let in_range = |ssid: &str| self.networks.iter().find(|n| n.ssid == ssid);
        let available = self.available();
        let (rows, pages) = self.page();
        for (row, r) in rows {
            match row {
                Row::SavedHeader => {
                    header.draw(c, raster, s.saved_networks, PAD as f32, r.y + 72);
                }
                Row::AvailableHeader => {
                    header.draw(c, raster, s.available_networks, PAD as f32, r.y + 72);
                    if self.status.enabled {
                        let rescan = Self::rescan_button(r);
                        button(c, rescan, false);
                        Label { face: sans, size: 28.0, ink: INK }.centered(c, raster, s.rescan, rescan);
                    }
                }
                Row::NoNetworks => {
                    soft.draw(c, raster, s.no_networks, PAD as f32, r.y + 60);
                }
                Row::Saved(i) => {
                    let ssid = &self.saved[i];
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
                Row::Available(i) => {
                    let Some(n) = available.get(i) else { continue };
                    let right = signal(c, W - PAD, r, n);
                    name.draw(c, raster, &name.fit(&n.ssid, (right - PAD - 24) as f32), PAD as f32, r.y + 70);
                    c.fill(Rect::new(PAD, r.bottom() - 1, W - 2 * PAD, 1), LINE);
                }
            }
        }
        if pages > 1 {
            let text = format!("‹   {} / {}   ›", self.page.min(pages - 1) + 1, pages);
            Label { face: sans, size: 32.0, ink: INK }.centered(c, raster, &text, PAGER);
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
        let row = s.available_row(0);
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
        let first = s.page().0.into_iter().find(|(r, _)| *r == Row::Saved(0)).expect("Casa").1;
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
    fn the_password_takes_accents_from_a_long_press() {
        let mut s = screen();
        let row = s.available_row(0);
        s.tap(centre(row).0, centre(row).1);
        type_text(&mut s, "caf");
        let e = s.keyboard().and_then(|kb| kb.keys().into_iter().find(|(k, _)| *k == Key::Char('e')));
        let (_, e) = e.expect("e");
        let strip = s.long_press(centre(e).0, centre(e).1).expect("accents strip");
        // The strip's first key, è, sits at its left end.
        let (x, y) = (strip.x + 12 + 40, strip.y + 12 + 40);
        assert!(matches!(s.tap(x, y), Tap::Typed(_)));
        assert_eq!(s.entry.as_ref().map(|e| e.password.as_str()), Some("cafè"));
    }

    #[test]
    fn an_open_network_joins_without_a_password() {
        let mut s = screen();
        let row = s.available_row(1);
        s.tap(centre(row).0, centre(row).1);
        assert!(s.entry.is_none());
        settle(&mut s);
        assert_eq!(s.status.connected.as_deref(), Some("Biblioteca"));
    }

    #[test]
    fn a_long_list_pages_and_every_network_stays_reachable() {
        let mut s = screen();
        s.saved.extend((0..12).map(|i| format!("Rete {i}")));
        let (_, pages) = s.page();
        assert!(pages > 1);
        assert!(s.available_row(0).h == 0, "the networks in range moved to a later page");
        let pager = (W - 100, PAGER.y + PAGER.h / 2);
        while s.available_row(0).h == 0 {
            assert!(matches!(s.tap(pager.0, pager.1), Tap::Redraw(_)), "one more page");
        }
        let row = s.available_row(1);
        s.tap(centre(row).0, centre(row).1);
        settle(&mut s);
        assert_eq!(s.status.connected.as_deref(), Some("Biblioteca"));
        // Back to the first page from the left arrow.
        while s.turn(false) {}
        assert_eq!(s.page, 0);
    }
}
