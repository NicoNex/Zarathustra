//! Quick settings: a panel that slides down from the top edge with the clock, battery,
//! toggles and the two light sliders.
//!
//! E-ink first: every handler returns the smallest rect that changed and the waveform to
//! refresh it with. A slider drag repaints only its row with the fast black/white waveform
//! and writes the light live (throttled); the release repaints the row in full gray.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zarathustra_device::wifi::WifiStatus;
use zarathustra_device::{Battery, Canvas, Device, Light, Phase, Rect, Touch, Waveform, Wifi};

use crate::clock::{date_text, hhmm, local_now};
use crate::i18n::t;
use crate::keyboard::{Keyboard, Typed};
use crate::settings::Settings;
use crate::ui::{rounded_outline, rounded_rect, Label, FILL, INK, INK_SOFT, LINE, PAPER};
use crate::wifi::{self, WifiScreen};
use crate::{Ctx, H, W};

/// The panel covers the top of the screen, full width.
pub const PANEL: Rect = Rect { x: 0, y: 0, w: W, h: 1016 };
/// A downward swipe that starts this close to the top edge opens the panel.
pub const EDGE: i32 = 60;
/// Minimum travel for a swipe; less is a tap.
const SWIPE: i32 = 100;
const SLOP: i32 = 40;
/// Light writes while dragging are throttled to about 20 Hz.
const LIVE_EVERY: Duration = Duration::from_millis(50);

const PAD: i32 = 72;
const RADIUS: i32 = 28;
const TILE_TOP: i32 = 176;
const TILE_H: i32 = 196;
const TILE_GAP: i32 = 28;
const TILE_W: i32 = (W - 2 * PAD - 3 * TILE_GAP) / 4;
/// Slider rows: a label line, then the control (− button, track, + button).
const ROW_H: i32 = 96;
const BUTTON: i32 = 88;
const TRACK_H: i32 = 24;
const KNOB_R: i32 = 38;
const TRACK_X0: i32 = PAD + BUTTON + 28;
const TRACK_X1: i32 = W - PAD - BUTTON - 28;
const STEP: i16 = 5;

/// Automatic warmth schedule: the evening ramp is centered on `DUSK`, the morning one on
/// `DAWN`, each `RAMP` minutes long.
// ponytail: fixed hours; the spec's sunset→sunrise (NOAA solar position from lat/lon) and
// custom hours replace these constants once Settings has a location.
pub const DUSK: u32 = 20 * 60;
pub const DAWN: u32 = 7 * 60;
pub const RAMP: u32 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slider {
    Brightness,
    Warmth,
}

impl Slider {
    /// Baseline of the label line.
    fn label_y(self) -> i32 {
        match self {
            Slider::Brightness => 676,
            Slider::Warmth => 860,
        }
    }

    /// The control row (buttons and track).
    fn row(self) -> Rect {
        Rect::new(PAD, self.label_y() + 22, W - 2 * PAD, ROW_H)
    }

    /// Everything the slider draws: label, value and control.
    fn rect(self) -> Rect {
        let top = self.label_y() - 44;
        Rect::new(PAD - KNOB_R, top, W - 2 * (PAD - KNOB_R), self.row().bottom() - top)
    }

    fn minus(self) -> Rect {
        let r = self.row();
        Rect::new(r.x, r.y + (ROW_H - BUTTON) / 2, BUTTON, BUTTON)
    }

    fn plus(self) -> Rect {
        let r = self.row();
        Rect::new(r.right() - BUTTON, r.y + (ROW_H - BUTTON) / 2, BUTTON, BUTTON)
    }

    /// Where the track takes drags: between the buttons, the full row height.
    fn track(self) -> Rect {
        let r = self.row();
        Rect::new(TRACK_X0 - KNOB_R / 2, r.y, TRACK_X1 - TRACK_X0 + KNOB_R, ROW_H)
    }
}

/// Knob center for `value` (0–100); the knob stays inside the track at both ends.
fn knob_x(value: u8) -> i32 {
    TRACK_X0 + KNOB_R + (value as i32 * (TRACK_X1 - TRACK_X0 - 2 * KNOB_R) + 50) / 100
}

/// The value a touch at `x` sets, clamped to 0–100.
fn value_at(x: i32) -> u8 {
    let span = TRACK_X1 - TRACK_X0 - 2 * KNOB_R;
    ((x - TRACK_X0 - KNOB_R) * 100 + span / 2).div_euclid(span).clamp(0, 100) as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tile {
    Wifi,
    Transfer,
    /// A full flashing refresh, clearing e-ink ghosting now.
    Clean,
    Sleep,
    AutoWarmth,
    Animation,
    Clock,
    /// White text on black.
    Night,
}

/// Two rows of four: connections and actions first, then reading toggles.
const TILES: [Tile; 8] =
    [Tile::Wifi, Tile::Transfer, Tile::Clean, Tile::Sleep, Tile::AutoWarmth, Tile::Animation, Tile::Clock, Tile::Night];

impl Tile {
    fn rect(self) -> Rect {
        let i = TILES.iter().position(|t| *t == self).unwrap_or(0) as i32;
        Rect::new(PAD + (i % 4) * (TILE_W + TILE_GAP), TILE_TOP + (i / 4) * (TILE_H + TILE_GAP), TILE_W, TILE_H)
    }

    fn label(self) -> [&'static str; 2] {
        let i = TILES.iter().position(|t| *t == self).unwrap_or(0);
        t().tiles[i]
    }
}

/// What the app loop should do after [`QuickPanel::touch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickResult {
    None,
    /// Call [`QuickPanel::render`], then refresh `rect` with `waveform`.
    Redraw {
        rect: Rect,
        waveform: Waveform,
    },
    /// A key of the Wi-Fi password keyboard went down: draw it pressed
    /// ([`Keyboard::draw_key`] on [`QuickPanel::keyboard`]) and refresh it.
    Pressed(Rect),
    /// Typing the Wi-Fi password: render into [`crate::typing::Typing::canvas`] and hand it this.
    Typed(Typed),
    /// The panel closed: redraw the screen under it and refresh [`PANEL`].
    Close,
    /// The Sleep tile was tapped (the panel closed too).
    Sleep,
    /// Redraw the screen, then the panel over it if still open, and refresh everything
    /// (flashing when `flash`): night mode switched, the screen cleaned, back from Wi-Fi.
    Repaint {
        flash: bool,
    },
    /// The Send-books tile was tapped (the panel closed): open that screen.
    Transfer,
}

pub struct QuickPanel {
    settings: Settings,
    state_dir: PathBuf,
    /// The warmth currently applied: `settings.warmth`, or the schedule's when automatic.
    warmth: u8,
    has_warmth: bool,
    battery: Option<Battery>,
    /// `None` without a Wi-Fi we can drive.
    wifi_dev: Option<Arc<dyn Wifi>>,
    /// The radio and network as last read.
    wifi: WifiStatus,
    /// The Wi-Fi screen, opened from the tile: it covers the screen while the panel is open,
    /// so it works the same over the home and the reader.
    wifi_screen: Option<WifiScreen>,
    /// Until when the radio state is read again every second: a network joins some seconds
    /// after the radio comes on, and the tile and the home should say so.
    watch: Option<Instant>,
    open: bool,
    down: Option<(i32, i32)>,
    drag: Option<Slider>,
    applied: Option<Instant>,
}

impl QuickPanel {
    /// Loads the settings and applies their light levels. On the first run (no settings file)
    /// it adopts the device's current levels instead.
    pub fn new(state_dir: &Path, dev: &mut dyn Device) -> QuickPanel {
        let saved = Settings::load_saved(state_dir);
        let mut settings = saved.unwrap_or_default();
        let current = dev.light();
        if let (None, Some(l)) = (saved, current) {
            settings.brightness = l.brightness;
            settings.warmth = l.warmth.unwrap_or(0);
        }
        let mut panel = QuickPanel {
            settings,
            state_dir: state_dir.to_path_buf(),
            warmth: settings.warmth,
            has_warmth: current.is_none_or(|l| l.warmth.is_some()),
            battery: None,
            wifi_dev: dev.wifi(),
            wifi: WifiStatus::default(),
            wifi_screen: None,
            watch: None,
            open: false,
            down: None,
            drag: None,
            applied: None,
        };
        panel.read_wifi();
        if settings.auto_warmth {
            panel.warmth = auto_warmth(local_now().1, settings.night_warmth);
        }
        panel.apply(dev);
        panel
    }

    /// The panel as it looks open with a sample battery and network, for `--pgm` reviews.
    pub fn preview(state_dir: &Path, night: bool) -> QuickPanel {
        let settings = Settings { night, ..Settings::load(state_dir) };
        QuickPanel {
            settings,
            state_dir: state_dir.to_path_buf(),
            warmth: settings.warmth,
            has_warmth: true,
            battery: Some(Battery { percent: 76, charging: false }),
            wifi_dev: None,
            wifi: WifiStatus { enabled: true, connected: Some("Casa".into()) },
            wifi_screen: None,
            watch: None,
            open: true,
            down: None,
            drag: None,
            applied: None,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Takes the settings screen's fields (language, full refresh) and saves. The rest is
    /// the panel's own: the screen's copy may be older than a slider moved since.
    pub fn update(&mut self, from: Settings) {
        self.settings.lang = from.lang;
        self.settings.flash_every = from.flash_every;
        self.settings.home_quotes = from.home_quotes;
        crate::i18n::set_lang(from.lang);
        self.save();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether a finished touch from `start` to `end` is the opening gesture: a downward
    /// swipe from the top edge.
    pub fn opens(start: (i32, i32), end: (i32, i32)) -> bool {
        let (dx, dy) = (end.0 - start.0, end.1 - start.1);
        start.1 < EDGE && dy > SWIPE && dy > dx.abs()
    }

    pub fn contains(x: i32, y: i32) -> bool {
        PANEL.contains(x, y)
    }

    /// Opens the panel; render it over the screen and refresh the returned rect.
    pub fn open(&mut self, dev: &dyn Device) -> Rect {
        self.battery = dev.battery();
        self.read_wifi();
        self.open = true;
        PANEL
    }

    /// Reads the radio state for the tile (two quick `lipc` calls on a Kindle).
    fn read_wifi(&mut self) {
        let Some(w) = &self.wifi_dev else { return };
        match w.status() {
            Ok(s) => self.wifi = s,
            Err(e) => eprintln!("quick settings: wifi: {e}"),
        }
    }

    /// The network joined, for the home's header.
    pub fn connected(&self) -> Option<&str> {
        self.wifi.connected.as_deref()
    }

    /// Waiting for a network to join: call [`QuickPanel::refresh_wifi`] every second.
    pub fn watching_wifi(&self) -> bool {
        self.watch.is_some()
    }

    /// Reads the radio state again; true if it changed. Done watching once a network joined,
    /// the radio went off or half a minute passed. Redraw the tile (and the home's header).
    // ponytail: two blocking `lipc` calls on the main loop (tens of ms), once a second while
    // watching and every half minute on the home; move to a thread if typing stutters then.
    pub fn refresh_wifi(&mut self) -> bool {
        let before = self.wifi.clone();
        self.read_wifi();
        let changed = self.wifi != before;
        // Joined (a new network, maybe replacing the one before) or given up.
        if (changed && self.wifi.connected.is_some()) || self.watch.is_some_and(|until| Instant::now() >= until) {
            self.watch = None;
        }
        changed
    }

    /// The tile, to refresh after the radio state changed with the panel open.
    pub fn wifi_tile(&self) -> Option<Rect> {
        (self.open && self.wifi_screen.is_none()).then(|| Tile::Wifi.rect())
    }

    /// Watches for a network joining: when `joining` (a connection under way, the radio just
    /// switched on), or while on with none joined.
    fn watch_wifi(&mut self, joining: bool) {
        let waiting = joining || (self.wifi.enabled && self.wifi.connected.is_none());
        self.watch = waiting.then(|| Instant::now() + Duration::from_secs(30));
    }

    /// Leaves the Wi-Fi screen. Its worker may still be connecting: the watch follows it.
    fn leave_wifi(&mut self) {
        if let Some(w) = self.wifi_screen.take() {
            self.wifi = w.status().clone();
            self.watch_wifi(w.busy());
        }
    }

    /// The keyboard on the panel (the Wi-Fi password's), while it is up.
    pub fn keyboard(&self) -> Option<&Keyboard> {
        self.wifi_screen.as_ref().filter(|_| self.open)?.keyboard()
    }

    /// A finger held on the panel: the accents of a key, if it is on one that has them.
    /// The area to refresh.
    pub fn long_press(&mut self, x: i32, y: i32) -> Option<Rect> {
        self.wifi_screen.as_mut().filter(|_| self.open)?.long_press(x, y)
    }

    /// The Wi-Fi screen has an operation running: call [`QuickPanel::poll`] often.
    pub fn busy(&self) -> bool {
        self.wifi_screen.as_ref().is_some_and(|w| w.busy())
    }

    /// Takes in a finished Wi-Fi operation; the rect to refresh after [`QuickPanel::render`].
    pub fn poll(&mut self) -> Option<Rect> {
        let w = self.wifi_screen.as_mut().filter(|_| self.open)?;
        w.poll().then_some(Rect::new(0, 0, W, H))
    }

    /// Closes the panel; redraw the screen and refresh the returned rect.
    pub fn close(&mut self) -> Rect {
        self.open = false;
        self.leave_wifi();
        self.down = None;
        self.drag = None;
        PANEL
    }

    /// Applies the automatic warmth for the current time; call every few minutes.
    /// Returns the rect to refresh (with `Waveform::Text`) when the open panel changed.
    pub fn tick(&mut self, dev: &mut dyn Device) -> Option<Rect> {
        if !self.settings.auto_warmth || self.drag.is_some() {
            return None;
        }
        let target = auto_warmth(local_now().1, self.settings.night_warmth);
        if target == self.warmth {
            return None;
        }
        self.warmth = target;
        self.apply(dev);
        (self.open && self.wifi_screen.is_none()).then(|| Slider::Warmth.rect())
    }

    /// Handles every touch phase while the panel is open.
    pub fn touch(&mut self, t: &Touch, dev: &mut dyn Device) -> QuickResult {
        if !self.open {
            return QuickResult::None;
        }
        if let Some(w) = &mut self.wifi_screen {
            return match t.phase {
                Phase::Down => {
                    self.down = Some((t.x, t.y));
                    w.press(t.x, t.y).map_or(QuickResult::None, QuickResult::Pressed)
                }
                Phase::Move => QuickResult::None,
                Phase::Up => {
                    let Some((x0, y0)) = self.down.take() else { return QuickResult::None };
                    // A vertical swipe turns the network list's page.
                    let (dx, dy) = (t.x - x0, t.y - y0);
                    if !w.entering() && dy.abs() > SWIPE && dy.abs() > dx.abs() {
                        return match w.turn(dy < 0) {
                            true => QuickResult::Redraw { rect: Rect::new(0, 0, W, H), waveform: Waveform::Text },
                            false => QuickResult::None,
                        };
                    }
                    match w.tap(x0, y0) {
                        wifi::Tap::None => QuickResult::None,
                        wifi::Tap::Redraw(rect) => QuickResult::Redraw { rect, waveform: Waveform::Text },
                        wifi::Tap::Typed(t) => QuickResult::Typed(t),
                        wifi::Tap::Back => {
                            self.leave_wifi();
                            QuickResult::Repaint { flash: false }
                        }
                    }
                }
            };
        }
        match t.phase {
            Phase::Down => {
                self.down = Some((t.x, t.y));
                let slider = [Slider::Brightness, Slider::Warmth].into_iter().find(|s| s.track().contains(t.x, t.y));
                match slider.filter(|s| self.enabled(*s)) {
                    Some(s) => {
                        self.drag = Some(s);
                        self.applied = None;
                        self.slide(s, t.x, dev)
                    }
                    None => QuickResult::None,
                }
            }
            Phase::Move => match self.drag {
                Some(s) => self.slide(s, t.x, dev),
                None => QuickResult::None,
            },
            Phase::Up => {
                let start = self.down.take();
                if let Some(s) = self.drag.take() {
                    self.set(s, value_at(t.x));
                    self.apply(dev);
                    self.save();
                    return QuickResult::Redraw { rect: s.rect(), waveform: Waveform::Text };
                }
                let Some((x0, y0)) = start else { return QuickResult::None };
                if !Self::contains(x0, y0) || t.y - y0 < -SWIPE {
                    self.close();
                    return QuickResult::Close;
                }
                if (t.x - x0).abs() > SLOP || (t.y - y0).abs() > SLOP {
                    return QuickResult::None;
                }
                self.tap(x0, y0, dev)
            }
        }
    }

    fn tap(&mut self, x: i32, y: i32, dev: &mut dyn Device) -> QuickResult {
        let inside = |r: Rect| r.contains(x, y);
        let s = &mut self.settings;
        let rect = match TILES.into_iter().find(|t| inside(t.rect())) {
            Some(Tile::Wifi) => {
                let Some(w) = &self.wifi_dev else { return QuickResult::None };
                let r = Tile::Wifi.rect();
                // The "›" third opens the network list; the rest switches the radio.
                if x >= r.right() - r.w / 3 {
                    self.wifi_screen = Some(WifiScreen::open(w.clone(), self.wifi.clone()));
                    return QuickResult::Redraw { rect: Rect::new(0, 0, W, H), waveform: Waveform::Text };
                }
                let on = !self.wifi.enabled;
                if let Err(e) = w.set_enabled(on) {
                    eprintln!("quick settings: wifi: {e}");
                }
                self.read_wifi();
                // wifid comes up a moment after the radio: the state read now may lag.
                self.watch_wifi(on && self.wifi.connected.is_none());
                return QuickResult::Redraw { rect: r, waveform: Waveform::Text };
            }
            Some(Tile::Transfer) => {
                self.close();
                return QuickResult::Transfer;
            }
            Some(Tile::Clean) => {
                self.close();
                return QuickResult::Repaint { flash: true };
            }
            Some(Tile::Night) => {
                s.night = !s.night;
                self.save();
                return QuickResult::Repaint { flash: true };
            }
            Some(Tile::Sleep) => {
                self.close();
                return QuickResult::Sleep;
            }
            Some(Tile::Animation) => {
                s.swipe_animation = !s.swipe_animation;
                Tile::Animation.rect()
            }
            Some(Tile::Clock) => {
                s.clock = !s.clock;
                Tile::Clock.rect()
            }
            Some(Tile::AutoWarmth) => {
                s.auto_warmth = !s.auto_warmth;
                if s.auto_warmth {
                    self.warmth = auto_warmth(local_now().1, s.night_warmth);
                } else {
                    // Keep what is on screen as the manual level.
                    s.warmth = self.warmth;
                }
                self.apply(dev);
                Tile::AutoWarmth.rect().union(&Slider::Warmth.rect())
            }
            None => {
                let step = [Slider::Brightness, Slider::Warmth].into_iter().find_map(|s| {
                    let delta = if inside(s.minus()) {
                        -STEP
                    } else if inside(s.plus()) {
                        STEP
                    } else {
                        return None;
                    };
                    self.enabled(s).then_some((s, delta))
                });
                let Some((slider, delta)) = step else { return QuickResult::None };
                let v = (self.value(slider) as i16 + delta).clamp(0, 100) as u8;
                self.set(slider, v);
                self.apply(dev);
                slider.rect()
            }
        };
        self.save();
        QuickResult::Redraw { rect, waveform: Waveform::Text }
    }

    /// A drag moved to `x`: repaint fast and write the light at most every [`LIVE_EVERY`].
    fn slide(&mut self, s: Slider, x: i32, dev: &mut dyn Device) -> QuickResult {
        let v = value_at(x);
        if v == self.value(s) {
            return QuickResult::None;
        }
        self.set(s, v);
        if self.applied.is_none_or(|t| t.elapsed() >= LIVE_EVERY) {
            self.apply(dev);
        }
        QuickResult::Redraw { rect: s.rect(), waveform: Waveform::Fast }
    }

    fn enabled(&self, s: Slider) -> bool {
        s == Slider::Brightness || (self.has_warmth && !self.settings.auto_warmth)
    }

    fn value(&self, s: Slider) -> u8 {
        match s {
            Slider::Brightness => self.settings.brightness,
            Slider::Warmth => self.warmth,
        }
    }

    fn set(&mut self, s: Slider, v: u8) {
        match s {
            Slider::Brightness => self.settings.brightness = v,
            Slider::Warmth => {
                self.warmth = v;
                self.settings.warmth = v;
            }
        }
    }

    fn apply(&mut self, dev: &mut dyn Device) {
        self.applied = Some(Instant::now());
        let light = Light { brightness: self.settings.brightness, warmth: self.has_warmth.then_some(self.warmth) };
        if let Err(e) = dev.set_light(light) {
            eprintln!("quick settings: set light: {e}");
        }
    }

    fn save(&self) {
        self.settings.save(&self.state_dir);
    }

    /// Draws the whole panel over whatever is on the canvas.
    // ponytail: a slider drag repaints the whole panel (only its rect is refreshed); clip to
    // the slider if the render time ever shows up next to the ~100 ms fast waveform.
    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(w) = &self.wifi_screen {
            return w.render(c, ctx);
        }
        // The top corners and the side edges of the outline fall outside the screen.
        let sheet = Rect::new(-3, -RADIUS, W + 6, PANEL.h + RADIUS);
        rounded_outline(c, sheet, RADIUS, 3, INK, PAPER);
        rounded_rect(c, Rect::new(W / 2 - 40, PANEL.h - 28, 80, 8), 4, LINE);

        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(bold)) = (crate::sans(fonts, false), crate::sans(fonts, true)) else {
            return;
        };

        // Header: time and date on the left, battery on the right.
        let (days, minutes) = local_now();
        let clock = Label { face: bold, size: 72.0, ink: INK };
        let x = PAD as f32 + clock.draw(c, raster, &hhmm(minutes), PAD as f32, 124) + 28.0;
        Label { face: sans, size: 32.0, ink: INK_SOFT }.draw(c, raster, &date_text(crate::i18n::lang(), days), x, 124);
        if let Some(b) = self.battery {
            let icon = Rect::new(W - PAD - 58, 98, 52, 28);
            rounded_outline(c, icon, 7, 3, INK, PAPER);
            rounded_rect(c, Rect::new(icon.right() + 2, icon.y + 8, 5, 12), 2, INK);
            let fill = (icon.w - 12) * b.percent as i32 / 100;
            rounded_rect(c, Rect::new(icon.x + 6, icon.y + 6, fill, icon.h - 12), 3, INK);
            let text =
                if b.charging { format!("{} · {}%", t().charging, b.percent) } else { format!("{}%", b.percent) };
            let label = Label { face: sans, size: 32.0, ink: INK };
            label.draw(c, raster, &text, icon.x as f32 - 18.0 - label.width(&text), 124);
        }

        for tile in TILES {
            let r = tile.rect();
            // Toggles show their state; actions (transfer, clean, sleep) an icon.
            let on = match tile {
                Tile::Wifi => Some(self.wifi.enabled),
                Tile::AutoWarmth => Some(self.settings.auto_warmth),
                Tile::Animation => Some(self.settings.swipe_animation),
                Tile::Clock => Some(self.settings.clock),
                Tile::Night => Some(self.settings.night),
                Tile::Transfer | Tile::Clean | Tile::Sleep => None,
            };
            let (bg, ink) = if on == Some(true) { (INK, PAPER) } else { (FILL, INK) };
            rounded_rect(c, r, 24, bg);
            let (x, y) = (r.x + 26, r.y);
            match (tile, on) {
                (_, Some(on)) => {
                    let state = Label { face: bold, size: 38.0, ink };
                    state.draw(c, raster, if on { t().on } else { t().off }, x as f32, y + 66);
                }
                // A crescent moon: a disc with an offset disc of the background cut out.
                (Tile::Sleep, _) => {
                    rounded_rect(c, Rect::new(x, y + 26, 48, 48), 24, ink);
                    rounded_rect(c, Rect::new(x + 16, y + 16, 44, 44), 22, bg);
                }
                // An arrow down into a tray.
                (Tile::Transfer, _) => {
                    c.fill(Rect::new(x + 21, y + 22, 6, 26), ink);
                    for n in 0..12 {
                        c.fill(Rect::new(x + 10 + n, y + 44 + n, 28 - 2 * n, 1), ink);
                    }
                    c.fill(Rect::new(x, y + 66, 48, 6), ink);
                    c.fill(Rect::new(x, y + 54, 6, 14), ink);
                    c.fill(Rect::new(x + 42, y + 54, 6, 14), ink);
                }
                // A ring with an arrowhead: refresh.
                _ => {
                    rounded_outline(c, Rect::new(x, y + 24, 48, 48), 24, 6, ink, bg);
                    c.fill(Rect::new(x + 30, y + 20, 20, 18), bg);
                    for n in 0..10 {
                        c.fill(Rect::new(x + 26 + n, y + 18 + n, 20 - 2 * n, 1), ink);
                    }
                }
            }
            let label = Label { face: sans, size: 30.0, ink };
            let [first, second] = tile.label();
            // The Wi-Fi tile's second line is the network joined.
            let second = match (tile, self.wifi.enabled, &self.wifi.connected) {
                (Tile::Wifi, true, Some(net)) => net.as_str(),
                (Tile::Wifi, true, None) => t().wifi_not_connected,
                _ => second,
            };
            for (i, line) in [first, second].iter().enumerate() {
                label.draw(c, raster, &label.fit(line, (r.w - 40) as f32), x as f32, y + 134 + i as i32 * 38);
            }
            if tile == Tile::Wifi {
                // The way into the network list.
                Label { face: bold, size: 44.0, ink }.draw(c, raster, "›", (r.right() - 44) as f32, y + 64);
            }
        }

        for s in [Slider::Brightness, Slider::Warmth] {
            self.render_slider(c, raster, sans, bold, s);
        }
    }

    fn render_slider(
        &self,
        c: &mut Canvas,
        raster: &mut zarathustra_engine::font::Rasterizer,
        sans: &zarathustra_engine::font::Face,
        bold: &zarathustra_engine::font::Face,
        s: Slider,
    ) {
        let enabled = self.enabled(s);
        let value = self.value(s);
        let y = s.label_y();
        let name = Label { face: bold, size: 34.0, ink: INK };
        let title = if s == Slider::Brightness { t().brightness } else { t().warmth };
        let end = PAD as f32 + name.draw(c, raster, title, PAD as f32, y);
        if s == Slider::Warmth && self.settings.auto_warmth {
            let chip = Label { face: bold, size: 22.0, ink: PAPER };
            let w = chip.width(t().auto) as i32 + 32;
            let r = Rect::new(end as i32 + 18, y - 30, w, 38);
            rounded_rect(c, r, r.h / 2, INK);
            chip.draw(c, raster, t().auto, r.x as f32 + 16.0, y - 3);
        }
        let text = format!("{value}%");
        let num = Label { face: sans, size: 34.0, ink: if enabled { INK } else { INK_SOFT } };
        num.draw(c, raster, &text, (W - PAD) as f32 - num.width(&text), y);

        let (fill, knob) = if enabled { (INK, INK) } else { (150, 150) };
        // While dragging, the fast waveform shows only black and white: light grays would
        // snap to white and pop back on release, so outline in ink instead.
        let mono = self.drag == Some(s);
        for (r, plus) in [(s.minus(), false), (s.plus(), true)] {
            if mono {
                rounded_outline(c, r, r.w / 2, 3, INK, PAPER);
            } else {
                rounded_rect(c, r, r.w / 2, FILL);
            }
            let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
            c.fill(Rect::new(cx - 15, cy - 2, 30, 5), fill);
            if plus {
                c.fill(Rect::new(cx - 2, cy - 15, 5, 30), fill);
            }
        }
        let row = s.row();
        let cy = row.y + row.h / 2;
        let track = Rect::new(TRACK_X0, cy - TRACK_H / 2, TRACK_X1 - TRACK_X0, TRACK_H);
        if mono {
            rounded_outline(c, track, TRACK_H / 2, 3, INK, PAPER);
        } else {
            rounded_rect(c, track, TRACK_H / 2, if enabled { LINE } else { FILL });
        }
        let kx = knob_x(value);
        rounded_rect(c, Rect { w: kx - track.x, ..track }, TRACK_H / 2, fill);
        rounded_outline(c, Rect::new(kx - KNOB_R, cy - KNOB_R, 2 * KNOB_R, 2 * KNOB_R), KNOB_R, 5, knob, PAPER);
    }
}

/// Warmth (0–`night`) for a time of day in minutes since midnight: 0 by day, `night` at
/// night, with smooth `RAMP`-minute transitions centered on [`DUSK`] and [`DAWN`].
pub fn auto_warmth(minutes: u32, night: u8) -> u8 {
    let ramp = |center: u32| {
        let t = ((minutes as f32 - center as f32) / RAMP as f32 + 0.5).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let f = if minutes < 12 * 60 { 1.0 - ramp(DAWN) } else { ramp(DUSK) };
    (night as f32 * f).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_warmth_is_zero_by_day_full_at_night_and_smooth_between() {
        assert_eq!(auto_warmth(12 * 60, 60), 0);
        assert_eq!(auto_warmth(DAWN + RAMP, 60), 0);
        assert_eq!(auto_warmth(DUSK - RAMP / 2, 60), 0);
        assert_eq!(auto_warmth(DUSK, 60), 30);
        assert_eq!(auto_warmth(DUSK + RAMP / 2, 60), 60);
        assert_eq!(auto_warmth(23 * 60, 60), 60);
        assert_eq!(auto_warmth(0, 60), 60);
        assert_eq!(auto_warmth(DAWN, 60), 30);
        // Monotonic, no jumps bigger than a couple of percent per minute.
        let evening: Vec<u8> = (DUSK - RAMP..DUSK + RAMP).map(|m| auto_warmth(m, 100)).collect();
        assert!(evening.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] <= 2));
        let morning: Vec<u8> = (DAWN - RAMP..DAWN + RAMP).map(|m| auto_warmth(m, 100)).collect();
        assert!(morning.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] <= 2));
    }

    #[test]
    fn slider_value_and_x_round_trip() {
        for v in 0..=100 {
            assert_eq!(value_at(knob_x(v)), v);
        }
        assert_eq!(value_at(0), 0);
        assert_eq!(value_at(W), 100);
        assert_eq!(knob_x(0), TRACK_X0 + KNOB_R);
        assert_eq!(knob_x(100), TRACK_X1 - KNOB_R);
    }

    #[test]
    fn opening_gesture_is_a_downward_swipe_from_the_top_edge() {
        assert!(QuickPanel::opens((600, 10), (610, 400)));
        assert!(!QuickPanel::opens((600, 200), (600, 600)));
        assert!(!QuickPanel::opens((600, 10), (600, 60)));
        assert!(!QuickPanel::opens((100, 10), (900, 200)));
    }

    /// Records light writes; everything else is inert.
    struct Fake {
        px: Vec<u8>,
        light: Light,
        writes: usize,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { px: vec![0; (W * crate::H) as usize], light: Light { brightness: 30, warmth: Some(10) }, writes: 0 }
        }
    }

    impl Device for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn dpi(&self) -> u32 {
            300
        }
        fn size(&self) -> (i32, i32) {
            (W, crate::H)
        }
        fn canvas(&mut self) -> Canvas<'_> {
            Canvas { width: W, height: crate::H, stride: W as usize, pixels: &mut self.px }
        }
        fn refresh(&mut self, _: &zarathustra_device::Refresh) -> std::io::Result<u32> {
            Ok(0)
        }
        fn wait_refresh(&mut self, _: u32) -> std::io::Result<()> {
            Ok(())
        }
        fn wait_events(&mut self, _: Option<Duration>, _: &mut Vec<zarathustra_device::Event>) -> std::io::Result<()> {
            Ok(())
        }
        fn light(&self) -> Option<Light> {
            Some(self.light)
        }
        fn set_light(&mut self, light: Light) -> std::io::Result<()> {
            self.light = light;
            self.writes += 1;
            Ok(())
        }
        fn battery(&self) -> Option<Battery> {
            Some(Battery { percent: 76, charging: true })
        }
        fn wifi(&self) -> Option<Arc<dyn Wifi>> {
            Some(Arc::new(zarathustra_device::wifi::Fake::new(Duration::ZERO)))
        }
    }

    fn state_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zarathustra-quick-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(phase: Phase, (x, y): (i32, i32)) -> Touch {
        Touch { id: 0, phase, x, y }
    }

    #[test]
    fn first_run_adopts_the_device_light_and_drag_sets_it_live() {
        let dir = state_dir("drag");
        let mut dev = Fake::new();
        let mut q = QuickPanel::new(&dir, &mut dev);
        assert_eq!(q.settings().brightness, 30);
        assert_eq!(q.open(&dev), PANEL);
        let row = Slider::Brightness.row();
        let y = row.y + row.h / 2;
        let down = q.touch(&touch(Phase::Down, (knob_x(50), y)), &mut dev);
        assert_eq!(down, QuickResult::Redraw { rect: Slider::Brightness.rect(), waveform: Waveform::Fast });
        assert_eq!(dev.light.brightness, 50);
        // Moves within the throttle window repaint but don't all write the LED.
        let writes = dev.writes;
        for v in 51..60 {
            q.touch(&touch(Phase::Move, (knob_x(v), y)), &mut dev);
        }
        assert!(dev.writes - writes < 9);
        let up = q.touch(&touch(Phase::Up, (knob_x(70), y)), &mut dev);
        assert_eq!(up, QuickResult::Redraw { rect: Slider::Brightness.rect(), waveform: Waveform::Text });
        assert_eq!(dev.light.brightness, 70);
        assert_eq!(Settings::load(&dir).brightness, 70);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn taps_step_toggle_and_close() {
        let dir = state_dir("taps");
        let mut dev = Fake::new();
        let mut q = QuickPanel::new(&dir, &mut dev);
        q.open(&dev);
        let tap = |q: &mut QuickPanel, dev: &mut Fake, r: Rect| {
            let p = (r.x + r.w / 2, r.y + r.h / 2);
            q.touch(&touch(Phase::Down, p), dev);
            q.touch(&touch(Phase::Up, p), dev)
        };
        tap(&mut q, &mut dev, Slider::Warmth.plus());
        assert_eq!(dev.light.warmth, Some(15));
        tap(&mut q, &mut dev, Tile::Animation.rect());
        assert!(!q.settings().swipe_animation);
        // Auto warmth disables the warmth slider.
        let r = tap(&mut q, &mut dev, Tile::AutoWarmth.rect());
        let both = Tile::AutoWarmth.rect().union(&Slider::Warmth.rect());
        assert_eq!(r, QuickResult::Redraw { rect: both, waveform: Waveform::Text });
        assert_eq!(tap(&mut q, &mut dev, Slider::Warmth.plus()), QuickResult::None);
        assert_eq!(Settings::load(&dir), *q.settings());
        assert_eq!(tap(&mut q, &mut dev, Rect::new(0, PANEL.bottom() + 100, 10, 10)), QuickResult::Close);
        assert!(!q.is_open());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn action_tiles_close_or_repaint_and_night_persists() {
        let dir = state_dir("tiles");
        let mut dev = Fake::new();
        let mut q = QuickPanel::new(&dir, &mut dev);
        let mut tap = |q: &mut QuickPanel, (x, y): (i32, i32)| {
            q.touch(&touch(Phase::Down, (x, y)), &mut dev);
            q.touch(&touch(Phase::Up, (x, y)), &mut dev)
        };
        let centre = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
        q.open(&Fake::new());
        assert_eq!(tap(&mut q, centre(Tile::Night.rect())), QuickResult::Repaint { flash: true });
        assert!(q.is_open() && Settings::load(&dir).night, "night is saved, the panel stays");
        assert_eq!(tap(&mut q, centre(Tile::Clean.rect())), QuickResult::Repaint { flash: true });
        assert!(!q.is_open(), "clean closes the panel first");
        q.open(&Fake::new());
        assert_eq!(tap(&mut q, centre(Tile::Transfer.rect())), QuickResult::Transfer);
        assert!(!q.is_open());
        q.open(&Fake::new());
        assert_eq!(tap(&mut q, centre(Tile::Sleep.rect())), QuickResult::Sleep);
        assert!(!q.is_open());
        // Wi-Fi: the left of the tile switches the radio, its "›" opens the network list.
        q.open(&Fake::new());
        assert!(q.wifi.enabled);
        let wifi = Tile::Wifi.rect();
        tap(&mut q, (wifi.x + 20, wifi.y + 20));
        assert!(!q.wifi.enabled);
        let full = QuickResult::Redraw { rect: Rect::new(0, 0, W, crate::H), waveform: Waveform::Text };
        assert_eq!(tap(&mut q, (wifi.right() - 20, wifi.y + 20)), full);
        assert!(q.wifi_screen.is_some());
        assert_eq!(tap(&mut q, (100, 60)), QuickResult::Repaint { flash: false }, "back to the panel");
        assert!(q.is_open() && q.wifi_screen.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_joining_network_is_watched_until_it_connects() {
        let dir = state_dir("watch");
        let mut dev = Fake::new();
        let mut q = QuickPanel::new(&dir, &mut dev);
        assert_eq!(q.connected(), Some("Casa"), "read at start");
        // The radio just came on, nothing joined yet.
        q.wifi = WifiStatus { enabled: true, connected: None };
        q.watch_wifi(false);
        assert!(q.watching_wifi());
        assert!(q.refresh_wifi(), "the network joined meanwhile");
        assert_eq!(q.connected(), Some("Casa"));
        assert!(!q.watching_wifi(), "done watching");
        assert!(!q.refresh_wifi(), "nothing changed");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn swipe_up_closes() {
        let dir = state_dir("swipe");
        let mut dev = Fake::new();
        let mut q = QuickPanel::new(&dir, &mut dev);
        q.open(&dev);
        q.touch(&touch(Phase::Down, (600, 120)), &mut dev);
        assert_eq!(q.touch(&touch(Phase::Up, (600, 60)), &mut dev), QuickResult::None, "too short");
        q.touch(&touch(Phase::Down, (600, PANEL.h - 20)), &mut dev);
        assert_eq!(q.touch(&touch(Phase::Up, (600, 400)), &mut dev), QuickResult::Close);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn renders_a_rounded_panel() {
        let mut ctx = Ctx::new();
        if ctx.sans(false).is_none() {
            return; // dev fonts are local-only
        }
        let dir = state_dir("render");
        let mut dev = Fake::new();
        let mut q = QuickPanel::new(&dir, &mut dev);
        q.open(&dev);
        let mut px = vec![200u8; (W * crate::H) as usize];
        let mut c = Canvas { width: W, height: crate::H, stride: W as usize, pixels: &mut px };
        crate::home::Home::scan(&[], &ctx).render(&mut c, &mut ctx);
        q.render(&mut c, &mut ctx);
        let at = |x: i32, y: i32| px[(y * W + x) as usize];
        assert_eq!(at(W / 2, 20), PAPER, "panel background");
        assert_eq!(at(W / 2, PANEL.h - 2), INK, "bottom edge");
        assert_eq!(at(1, PANEL.h - 2), PAPER, "rounded corner shows the screen below");
        let tile = Tile::Animation.rect();
        assert_eq!(at(tile.x + 5, tile.y + tile.h / 2), INK, "the animation tile is on");
        // Set ZARATHUSTRA_PGM_DIR to look at the design.
        let save = |name: &str, px: &[u8]| {
            if let Some(out) = std::env::var_os("ZARATHUSTRA_PGM_DIR") {
                let mut pgm = format!("P5\n{W} {}\n255\n", crate::H).into_bytes();
                pgm.extend_from_slice(px);
                std::fs::write(Path::new(&out).join(name), pgm).unwrap();
            }
        };
        save("quick.pgm", &px);
        q.settings.auto_warmth = true;
        let mut c = Canvas { width: W, height: crate::H, stride: W as usize, pixels: &mut px };
        q.render(&mut c, &mut ctx);
        save("quick-auto.pgm", &px);
        std::fs::remove_dir_all(dir).ok();
    }
}
