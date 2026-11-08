//! Zarathustra: home and reader on the device.
//!
//!   zarathustra [BOOK] [--no-swipe] [--pgm DIR] [--search WORD]
//!   zarathustra --start-services [--recovery]
//!
//! With `--pgm`, renders the home screen and the first pages of BOOK into DIR instead.
//! `--start-services` starts the plugins' services meant for power-on (with `--recovery`, the
//! recovery ones too) and exits: the boot runs it before the UI, independently of it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use zarathustra::highlight::{Button, LONG_PRESS, SLOP};
use zarathustra::home::Home;
use zarathustra::keyboard::{Keyboard, Typed};
use zarathustra::pdfview::PdfView;
use zarathustra::quick::{QuickPanel, QuickResult, PANEL};
use zarathustra::reader::{Reader, HEADER};
use zarathustra::sleep::render_sleep;
use zarathustra::toc::Tab;
use zarathustra::typing::Typing;
use zarathustra::{Action, Ctx, Landing, H, W};
use zarathustra_device::{Canvas, Device, Event, Phase, PowerEvent, Rect, Refresh, Swipe, Waveform};
use zarathustra_engine::library::Format;

enum Screen {
    Home(Box<Home>),
    Reader(Box<Reader>),
    Pdf(Box<PdfView>),
}

impl Screen {
    /// Draws the screen; `clock` is the current `Settings::clock`.
    fn render(&mut self, c: &mut Canvas, ctx: &mut Ctx, clock: bool) {
        match self {
            Screen::Home(h) => h.render(c, ctx),
            Screen::Reader(r) => {
                r.clock = clock;
                r.render(c, ctx)
            }
            Screen::Pdf(p) => {
                p.clock = clock;
                p.render(c, ctx)
            }
        }
    }

    /// The keyboard up on the screen, if any.
    fn keyboard(&self) -> Option<&Keyboard> {
        match self {
            Screen::Home(h) => h.keyboard(),
            Screen::Reader(r) => r.keyboard(),
            Screen::Pdf(_) => None,
        }
    }

    /// The reading screen for the book at `path`: PDFs as pages, everything else reflowed.
    fn open(path: &std::path::Path, ctx: &mut Ctx) -> Result<Screen> {
        Ok(match Format::from_path(path) {
            Some(Format::Pdf) => Screen::Pdf(Box::new(PdfView::open(path, ctx)?)),
            _ => Screen::Reader(Box::new(Reader::open(path, ctx)?)),
        })
    }
}

/// The touch in progress, for long-press detection.
#[derive(Default)]
struct Press {
    /// When the finger went down, while it may still become a long press.
    at: Option<Instant>,
    /// The long press happened (the lift is not a tap).
    fired: bool,
    /// It selected a word: moves extend the selection.
    selecting: bool,
    /// The selection turned the page: drawn black and white, the whole page settles on the lift.
    turned: bool,
    /// How the selection's frames went, logged on the lift.
    frames: Frames,
}

/// Selection frames: how many, time spent waiting for the previous refresh, time drawing and
/// queuing the refresh.
#[derive(Default)]
struct Frames {
    count: u32,
    wait: Duration,
    draw: Duration,
}

impl Frames {
    fn add(&mut self, wait: Duration, draw: Duration) {
        self.count += 1;
        self.wait += wait;
        self.draw += draw;
    }

    fn log(&self) {
        if self.count > 1 {
            let n = self.count;
            eprintln!("selection: {n} frames, wait {:?} + draw {:?} per frame", self.wait / n, self.draw / n);
        }
    }
}

impl Press {
    fn pending(&self) -> Option<Instant> {
        self.at.filter(|_| !self.fired)
    }

    /// Turns the press into a long press at `start` when there is a word there to select;
    /// otherwise the lift stays a plain tap (menu controls, margins, the popup's buttons).
    fn fire(&mut self, r: &mut Reader, start: (i32, i32), selection: &mut Option<Rect>) {
        self.at = None;
        let typing = r.search_shown();
        if let Some(rect) = r.long_press(start) {
            // Accents on the search keyboard are tapped next, not dragged over.
            self.select(rect, !typing, selection);
        }
    }

    /// The press selected text (`rect` to refresh); `drag` when moves extend it.
    fn select(&mut self, rect: Rect, drag: bool, selection: &mut Option<Rect>) {
        self.fired = true;
        self.selecting = drag;
        *selection = Some(selection.map_or(rect, |s| s.union(&rect)));
    }
}

/// The binary run.sh copies to /tmp and runs (size, modification time); `None` off the device.
fn installed_binary() -> Option<(u64, std::time::SystemTime)> {
    let meta = std::fs::metadata("/mnt/us/zarathustra/bin/zarathustra").ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Exits so that run.sh runs the newly installed binary at once (the flag tells it this is
/// an update), keeping the Amazon UI stopped. The position and settings are already saved.
fn restart_into_update() -> ! {
    eprintln!("a new binary was installed: restarting into it");
    if let Err(e) = std::fs::write("/tmp/zarathustra-update", b"") {
        eprintln!("update flag: {e}");
    }
    std::process::exit(0)
}

fn library_dirs() -> Vec<PathBuf> {
    // Only the books the user put in library/; Amazon's documents/ is left alone.
    vec![zarathustra::library_dir()]
}

/// The home kept on the shelf while reading (brought up to date), or a fresh scan.
fn go_home(shelf: &mut Option<(Box<Home>, PathBuf)>, ctx: &Ctx, wifi: bool) -> Box<Home> {
    let mut home = match shelf.take() {
        Some((mut h, path)) => {
            h.returned(&path, ctx);
            h.refresh(ctx);
            h
        }
        None => Box::new(Home::scan(&library_dirs(), ctx)),
    };
    home.wifi = wifi;
    home
}

/// Before drawing on the panel other than by typing: typing's refreshes over, then the last
/// refresh from anywhere, so nothing is drawn into an area still refreshing.
fn ready(dev: &mut dyn Device, typing: &mut Typing, marker: &mut u32) -> std::io::Result<()> {
    typing.settle(dev, marker)?;
    dev.wait_refresh(*marker)
}

fn write_pgm(path: PathBuf, px: &[u8]) -> Result<()> {
    let mut out = format!("P5\n{W} {H}\n255\n").into_bytes();
    out.extend_from_slice(px);
    Ok(std::fs::write(path, out)?)
}

/// Writes `px` as the panel shows it in night mode: the whole frame inverted (the pictures
/// were drawn pre-inverted, so they come out as they are).
fn write_night_pgm(path: PathBuf, px: &[u8]) -> Result<()> {
    let inverted: Vec<u8> = px.iter().map(|p| !p).collect();
    write_pgm(path, &inverted)
}

/// The exit status that tells run.sh to hand the storage to a computer over USB (the Amazon
/// UI does that) and start Zarathustra again once the cable is out.
const USB_EXIT: i32 = 42;

/// A computer was plugged in: ask whether to hand it the storage or just charge. True to
/// hand it over; false to go on (also when the cable goes away meanwhile).
// ponytail: the Amazon UI does the mass-storage switch (volumd waits for it); do it ourselves
// (unmount /mnt/base-us, point g_mass_storage's lun at it) once the framework is gone for good.
fn usb_prompt(dev: &mut dyn zarathustra_device::Device, ctx: &mut Ctx) -> std::io::Result<bool> {
    use zarathustra::ui::{button, Label, INK, INK_SOFT, PAPER};
    let (transfer, charge) = (Rect::new(160, 900, W - 320, 120), Rect::new(160, 1060, W - 320, 120));
    {
        let mut c = dev.canvas();
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let s = zarathustra::i18n::t();
        if let (Some(serif), Some(sans), Some(bold)) =
            (zarathustra::serif(fonts, true), zarathustra::sans(fonts, false), zarathustra::sans(fonts, true))
        {
            Label { face: serif, size: 64.0, ink: INK }.centered(
                &mut c,
                raster,
                s.usb_title,
                Rect::new(0, 480, W, 100),
            );
            let body = Label { face: sans, size: 34.0, ink: INK_SOFT };
            for (i, line) in body.wrap(s.usb_body, (W - 320) as f32, 5).iter().enumerate() {
                body.centered(&mut c, raster, line, Rect::new(0, 620 + i as i32 * 52, W, 52));
            }
            button(&mut c, transfer, true);
            Label { face: bold, size: 36.0, ink: PAPER }.centered(&mut c, raster, s.usb_transfer, transfer);
            button(&mut c, charge, false);
            Label { face: sans, size: 36.0, ink: INK }.centered(&mut c, raster, s.usb_charge, charge);
        }
    }
    let full = Rect::new(0, 0, W, H);
    dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
    let mut events = Vec::new();
    loop {
        events.clear();
        dev.wait_events(None, &mut events)?;
        while let Some(ev) = dev.power_event() {
            if let PowerEvent::Usb(false) = ev {
                return Ok(false);
            }
        }
        for ev in &events {
            if let Event::Touch(t) = ev {
                if t.phase == Phase::Up && transfer.contains(t.x, t.y) {
                    return Ok(true);
                }
                if t.phase == Phase::Up && charge.contains(t.x, t.y) {
                    return Ok(false);
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut book, mut swipe, mut pgm, mut query) = (None, true, None, "natura".to_string());
    let (mut services, mut recovery) = (false, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--start-services" => services = true,
            "--recovery" => recovery = true,
            "--no-swipe" => swipe = false,
            "--search" => query = args.next().context("--search WORD")?,
            "--pgm" => pgm = Some(PathBuf::from(args.next().context("--pgm DIR")?)),
            path => book = Some(PathBuf::from(path)),
        }
    }
    if services {
        // No fonts, no screen: just the services, fast, at boot.
        let state_dir = zarathustra::state_dir();
        zarathustra::plugin::start_services(&zarathustra::plugin::Dirs::new(&state_dir), &state_dir, true, recovery);
        return Ok(());
    }
    let t = Instant::now();
    let mut ctx = Ctx::new();
    let home = |ctx: &Ctx| Home::scan(&library_dirs(), ctx);
    let mut screen = Screen::Home(Box::new(home(&ctx)));
    // The home kept while a book is open (and the book's path), so going back is a redraw,
    // not a library scan and cover decode.
    // ponytail: the covers stay in memory while reading (a few MB for a few dozen books);
    // drop the big ones on open if a large library makes that matter.
    let mut shelf: Option<(Box<Home>, PathBuf)> = None;
    eprintln!("home ready in {:?}", t.elapsed());
    if let Some(path) = &book {
        match Screen::open(path, &mut ctx) {
            Ok(s) => screen = s,
            Err(e) => eprintln!("{e:#}"),
        }
    }

    if let Some(dir) = pgm {
        std::fs::create_dir_all(&dir)?;
        let mut px = vec![0u8; (W * H) as usize];
        let mut home = home(&ctx);
        home.wifi = true;
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("home.pgm"), &px)?;
        // The first page's band with a sample quote, then as the reading figures.
        home.sample_band();
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("home-quote.pgm"), &px)?;
        home.band = zarathustra::home::Band::Stats;
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("home-stats.pgm"), &px)?;
        home.band = zarathustra::home::Band::Quote;
        // The quick panel pulled down over the library.
        zarathustra::quick::QuickPanel::preview(&ctx.state_dir, false)
            .render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("quick.pgm"), &px)?;
        // Night mode: the tile on, the whole screen inverted as the device shows it.
        zarathustra_device::set_night(true);
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        zarathustra::quick::QuickPanel::preview(&ctx.state_dir, true)
            .render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_night_pgm(dir.join("quick-night.pgm"), &px)?;
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_night_pgm(dir.join("home-night.pgm"), &px)?;
        zarathustra_device::set_night(false);
        // The Wi-Fi screen on the pretend networks, then a password being typed (caps on).
        {
            use zarathustra::keyboard::{Key, Keyboard};
            use zarathustra_device::wifi::{Fake, WifiStatus};
            let fake = std::sync::Arc::new(Fake::new(Duration::ZERO));
            let mut wifi = zarathustra::wifi::WifiScreen::open(fake, WifiStatus::default());
            let t0 = Instant::now();
            while wifi.busy() && t0.elapsed() < Duration::from_secs(5) {
                wifi.poll();
                std::thread::sleep(Duration::from_millis(1));
            }
            wifi.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_pgm(dir.join("wifi.pgm"), &px)?;
            let row = wifi.available_row(0);
            wifi.tap(row.x + row.w / 2, row.y + row.h / 2);
            let keys = Keyboard::default().keys();
            let mut press = |k: Key| {
                if let Some((_, r)) = keys.iter().find(|(key, _)| *key == k) {
                    wifi.tap(r.x + r.w / 2, r.y + r.h / 2);
                }
            };
            for ch in "libri".chars() {
                press(Key::Char(ch));
            }
            press(Key::Shift);
            press(Key::Char('l'));
            wifi.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_pgm(dir.join("wifi-password.pgm"), &px)?;
            // The keyboard alone, shift off, once (one tap) and locked (a second quick tap),
            // then a key held down.
            let mut kb = Keyboard::default();
            let centre = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
            let shift = kb.keys().into_iter().find(|(k, _)| *k == Key::Shift).map(|(_, r)| centre(r));
            for state in ["off", "once", "locked"] {
                px.fill(255);
                kb.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
                write_pgm(dir.join(format!("keyboard-shift-{state}.pgm")), &px)?;
                if let Some((x, y)) = shift {
                    kb.tap(x, y);
                }
            }
            if let Some((_, r)) = kb.keys().into_iter().find(|(k, _)| *k == Key::Char('g')) {
                kb.press(centre(r).0, centre(r).1);
            }
            kb.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_pgm(dir.join("keyboard-pressed.pgm"), &px)?;
        }
        home.tap((900, 800), (300, 800), &ctx); // swipe to the second page
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("home2.pgm"), &px)?;
        // Book menu on the first cover, its About screen, then the statistics.
        let mut shot = |home: &mut Home, ctx: &mut Ctx, at: Option<(i32, i32)>, name: &str| {
            if let Some(at) = at {
                home.tap(at, at, ctx);
            }
            home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, ctx);
            write_pgm(dir.join(name), &px)
        };
        // The floating book menu beside a cover in each grid column, then beside the
        // continue-reading card (left open for the About screen below).
        for (n, x) in [150, 620, 1050].into_iter().enumerate() {
            home.long_press((x, 800), &ctx);
            shot(&mut home, &mut ctx, None, &format!("bookmenu-col{n}.pgm"))?;
            home.tap((5, 5), (5, 5), &ctx); // outside the card: closes it
        }
        home.long_press((150, 400), &ctx);
        shot(&mut home, &mut ctx, None, "bookmenu.pgm")?;
        shot(&mut home, &mut ctx, Some((600, 1313)), "about.pgm")?;
        home.tap((100, 75), (100, 75), &ctx);
        shot(&mut home, &mut ctx, Some((722 + 78, 118)), "stats.pgm")?;
        home.tap((100, 75), (100, 75), &ctx);
        shot(&mut home, &mut ctx, Some((W - 72 - 36, 118)), "power.pgm")?;
        home.tap((5, 5), (5, 5), &ctx);
        shot(&mut home, &mut ctx, Some((516 + 78, 118)), "prefs.pgm")?;
        // The first plugin's page (SSH), then back to the settings and the home.
        shot(&mut home, &mut ctx, Some((200, 950)), "plugin.pgm")?;
        home.tap((100, 75), (100, 75), &ctx);
        home.tap((100, 75), (100, 75), &ctx);
        // Send books as it is now (no network: the retry screen), then with a fixed URL.
        shot(&mut home, &mut ctx, Some((490, 118)), "transfer.pgm")?;
        home.tap((100, 75), (100, 75), &ctx);
        // Library search: the field and keyboard, then "gra" typed.
        shot(&mut home, &mut ctx, Some((620 + 78, 118)), "search.pgm")?;
        let kb = zarathustra::keyboard::Keyboard::default();
        for ch in "gra".chars() {
            let key = kb.keys().into_iter().find(|(k, _)| *k == zarathustra::keyboard::Key::Char(ch));
            if let Some((_, r)) = key {
                let at = (r.x + r.w / 2, r.y + r.h / 2);
                home.press(at);
                home.tap(at, at, &ctx);
            }
        }
        shot(&mut home, &mut ctx, None, "search2.pgm")?;
        // Held, "e" raises its accents strip.
        if let Some((_, r)) = kb.keys().into_iter().find(|(k, _)| *k == zarathustra::keyboard::Key::Char('e')) {
            home.long_press((r.x + r.w / 2, r.y + r.h / 2), &ctx);
        }
        shot(&mut home, &mut ctx, None, "accents.pgm")?;
        let mut tr = zarathustra::transfer::Transfer::open();
        tr.set_url("http://192.168.1.42:8080/k7f3x2".into());
        tr.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("transfer-qr.pgm"), &px)?;
        drop(tr);
        if let Screen::Pdf(p) = &mut screen {
            zarathustra_device::set_night(true);
            p.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_night_pgm(dir.join("page0-night.pgm"), &px)?;
            zarathustra_device::set_night(false);
            for n in 0..3 {
                p.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
                write_pgm(dir.join(format!("page{n}.pgm")), &px)?;
                p.turn(true);
            }
            p.menu = true;
            p.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_pgm(dir.join("menu.pgm"), &px)?;
        }
        if let Screen::Reader(r) = &mut screen {
            zarathustra_device::set_night(true);
            r.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
            write_night_pgm(dir.join("page0-night.pgm"), &px)?;
            zarathustra_device::set_night(false);
            for n in 0..3 {
                r.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
                write_pgm(dir.join(format!("page{n}.pgm")), &px)?;
                r.turn(true, &mut ctx);
            }
            // The highlight flow on the page shown: select, save, tap it, delete it again.
            let mut snap = |r: &mut Reader, ctx: &mut Ctx, name: &str| {
                r.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, ctx);
                write_pgm(dir.join(name), &px)
            };
            let centre = |b: Rect| (b.x + b.w / 2, b.y + b.h / 2);
            let (x, y) = (W / 3, H / 2);
            r.long_press((x, y));
            r.drag((x + 350, y + 120));
            r.release(&mut ctx);
            snap(r, &mut ctx, "select.pgm")?;
            if let Some(b) = r.popup_button(Button::Highlight) {
                r.tap(centre(b), centre(b).0, &mut ctx);
            }
            snap(r, &mut ctx, "highlight.pgm")?;
            if let Some(path) = &book {
                let list = zarathustra::notebook::load(&ctx.state_dir, path);
                let nb = zarathustra::notebook::Notebook::new(&ctx, r.title().unwrap_or(""), "", list);
                let mut nb_px = vec![0u8; (W * H) as usize];
                nb.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut nb_px }, &mut ctx);
                write_pgm(dir.join("notebook.pgm"), &nb_px)?;
            }
            r.tap((x, y), x, &mut ctx);
            snap(r, &mut ctx, "delete.pgm")?;
            if let Some(b) = r.popup_button(Button::Delete) {
                r.tap(centre(b), centre(b).0, &mut ctx);
            }
            // Bookmark the page (the dog-ear corner), then the menu with its scrubber at rest
            // and dragged, the jump on release (with the way back), and the bookmarks tab.
            r.tap((W - 40, 40), W - 40, &mut ctx);
            snap(r, &mut ctx, "bookmark.pgm")?;
            r.menu = true;
            snap(r, &mut ctx, "menu.pgm")?;
            let bar = zarathustra::menu::scrubber();
            let (from, to) = ((bar.x + 40, bar.y + bar.h / 2), (bar.x + bar.w * 2 / 3, bar.y + bar.h / 2));
            r.scrub(from, to);
            snap(r, &mut ctx, "scrub.pgm")?;
            r.tap(from, to.0, &mut ctx);
            snap(r, &mut ctx, "jumped.pgm")?;
            // The first footnote from here, then the first one long enough to page, its
            // second page, and the page left as it was once closed.
            if r.open_first_note(&mut ctx) {
                snap(r, &mut ctx, "footnote.pgm")?;
                for _ in 0..500 {
                    if r.note_mut().is_some_and(|n| n.pages() > 1) {
                        break;
                    }
                    r.tap((5, 5), 5, &mut ctx); // outside the card: closes it
                    if !r.turn(true, &mut ctx) || !r.open_first_note(&mut ctx) {
                        break;
                    }
                }
                if let Some(card) = r.note_mut().filter(|n| n.pages() > 1).map(|n| n.bounds()) {
                    snap(r, &mut ctx, "footnote-long.pgm")?;
                    let inside = (card.x + card.w * 3 / 4, card.y + card.h / 2);
                    r.tap(inside, inside.0, &mut ctx);
                    snap(r, &mut ctx, "footnote-long2.pgm")?;
                }
                r.tap((5, 5), 5, &mut ctx);
                snap(r, &mut ctx, "footnote-closed.pgm")?;
            }
            // A long press on one word: its definition card (dictionaries from dev-dicts/),
            // its second page when it has one, then a tap outside closes it.
            let word = (W / 2, H / 4);
            r.long_press(word);
            r.release(&mut ctx);
            snap(r, &mut ctx, "define.pgm")?;
            if r.card_open() {
                r.tap((W - 100, H - 400), W - 100, &mut ctx);
                snap(r, &mut ctx, "define2.pgm")?;
                r.tap(word, word.0, &mut ctx);
            }
            r.menu = true;
            r.show_contents();
            snap(r, &mut ctx, "toc.pgm")?;
            r.tap((W - 100, 200), W - 100, &mut ctx);
            snap(r, &mut ctx, "bookmarks.pgm")?;
            // Search: type the query on the keyboard, run it, open the first hit.
            r.show_search();
            let keys = zarathustra::keyboard::Keyboard::default().keys();
            let key = |k: zarathustra::keyboard::Key| keys.iter().find(|(key, _)| *key == k).map(|(_, r)| centre(*r));
            for ch in query.chars() {
                let k =
                    if ch == ' ' { zarathustra::keyboard::Key::Space } else { zarathustra::keyboard::Key::Char(ch) };
                if let Some(at) = key(k) {
                    r.tap(at, at.0, &mut ctx);
                }
            }
            snap(r, &mut ctx, "search_keyboard.pgm")?;
            if let Some(at) = key(zarathustra::keyboard::Key::Enter) {
                r.tap(at, at.0, &mut ctx);
            }
            let t0 = Instant::now();
            r.search_step();
            eprintln!("search: first step {:?}", t0.elapsed());
            while r.is_searching() {
                r.search_step();
            }
            eprintln!("search: done in {:?}", t0.elapsed());
            snap(r, &mut ctx, "search_results.pgm")?;
            let row = zarathustra::search::LIST_TOP + zarathustra::search::ROW_H / 2;
            r.tap((W / 2, row), W / 2, &mut ctx);
            snap(r, &mut ctx, "search_hit.pgm")?;
        }
        return Ok(());
    }

    let mut dev = zarathustra_device::Night::new(zarathustra_device::open_default()?);
    let mut quick = QuickPanel::new(&ctx.state_dir, &mut dev);
    zarathustra_device::set_night(quick.settings().night);
    let full = Rect::new(0, 0, W, H);
    screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
    let mut marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
    // The plugins' services the user made active (running ones are left alone), once the first
    // screen is up: a service's first start copies its files.
    zarathustra::plugin::start_services(&zarathustra::plugin::Dirs::new(&ctx.state_dir), &ctx.state_dir, false, false);
    // When the home next looks for new books, and the radio state is next read: their own
    // cadences, whatever else wakes the loop (a Wi-Fi operation polls every 300 ms).
    let (mut rescan_at, mut wifi_read) = (Instant::now(), Instant::now());
    let mut turns = 0u32;
    let mut down = None;
    let mut press = Press::default();
    // Keys and the field while typing, refreshed apart and without waiting on each other.
    let mut typing = Typing::default();
    let mut events = Vec::new();
    let mut asleep = false;
    // The installed binary as it was at start: a new one copied over USB means restart into it.
    let installed = installed_binary();
    loop {
        events.clear();
        // Wake every few minutes so automatic warmth follows the clock, or when a finger held
        // still on a page becomes a long press.
        let long_press = match (&screen, press.pending()) {
            (Screen::Reader(_) | Screen::Home(_), Some(at)) if !quick.is_open() => {
                Some(LONG_PRESS.saturating_sub(at.elapsed()))
            }
            // The Wi-Fi password's keys have accents too.
            (_, Some(at)) if quick.keyboard().is_some() => Some(LONG_PRESS.saturating_sub(at.elapsed())),
            _ => None,
        };
        // A search under way only peeks at the input queue between chapters.
        let searching = matches!(&screen, Screen::Reader(r) if r.is_searching()) && !quick.is_open();
        // The send-books screen checks for arriving books every second; the home looks for
        // new books every half minute (a walk and a stat per file).
        if let Screen::Home(h) = &mut screen {
            h.wifi = quick.connected().is_some();
        }
        let home = matches!(screen, Screen::Home(_));
        let idle = match &screen {
            // A Wi-Fi scan or connection running: show its outcome promptly.
            _ if quick.busy() => Duration::from_millis(300),
            // A network joining: the tile or the home's header says so when it has.
            _ if quick.watching_wifi() && (quick.is_open() || home) => Duration::from_secs(1),
            Screen::Home(h) if h.transferring() => Duration::from_secs(1),
            Screen::Home(_) => Duration::from_secs(30),
            _ => Duration::from_secs(300),
        };
        // Typed keys or text waiting for the refresh over them to end.
        let due = typing.due();
        // A finger held at a page edge while selecting turns the page after a dwell.
        let edge = match &screen {
            Screen::Reader(r) if press.selecting => r.edge_wait(),
            _ => None,
        };
        let timeout = if searching { Duration::ZERO } else { long_press.or(edge).unwrap_or(idle) };
        dev.wait_events(Some(due.map_or(timeout, |d| d.min(timeout))), &mut events)?;
        typing.flush(&mut dev, &mut marker)?;
        let mut power = false;
        while let Some(ev) = dev.power_event() {
            power = true;
            ready(&mut dev, &mut typing, &mut marker)?;
            match ev {
                PowerEvent::Sleep => {
                    asleep = true;
                    quick.close();
                    let battery = dev.battery();
                    match &mut screen {
                        Screen::Reader(r) => r.render_sleep(&mut dev.canvas(), &mut ctx, battery),
                        Screen::Pdf(p) => p.render_sleep(&mut dev.canvas(), &mut ctx, battery),
                        Screen::Home(_) => render_sleep(&mut dev.canvas(), &mut ctx, None, "", "", battery),
                    }
                }
                PowerEvent::Wake => {
                    asleep = false;
                    screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock)
                }
                PowerEvent::Usb(true) if !asleep => {
                    quick.close();
                    if usb_prompt(&mut dev, &mut ctx)? {
                        std::process::exit(USB_EXIT);
                    }
                    screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock)
                }
                // The computer let the storage go: Amazon's USB screen and light are ours again.
                PowerEvent::Usb(false) if !asleep => {
                    if installed.is_some() && installed_binary() != installed {
                        restart_into_update();
                    }
                    quick.apply(&mut dev);
                    screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock)
                }
                PowerEvent::Suspend { .. } | PowerEvent::Resumed | PowerEvent::Usb(_) => continue,
            }
            marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
            down = None;
            press = Press::default();
        }
        // The periodic wake-up (not a long-press or power one): bring the reader's clock up to date.
        let periodic = events.is_empty() && long_press.is_none() && due.is_none() && !power && !searching;
        // On the home, books that arrived meanwhile.
        let rescan = periodic && Instant::now() >= rescan_at;
        if let (true, false, Screen::Home(h)) = (rescan && !quick.is_open(), asleep, &mut screen) {
            rescan_at = Instant::now() + Duration::from_secs(30);
            // `|` not `||`: a quote due is picked even when the library changed too.
            if h.refresh(&ctx) | h.rotate_quote(&ctx) {
                ready(&mut dev, &mut typing, &mut marker)?;
                h.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        if periodic && !asleep && !quick.is_open() && quick.settings().clock {
            if let Screen::Reader(_) | Screen::Pdf(_) = screen {
                ready(&mut dev, &mut typing, &mut marker)?;
                screen.render(&mut dev.canvas(), &mut ctx, true);
                marker = dev.refresh(&Refresh { rect: HEADER, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        // Panel changes from the whole batch, drawn once: a drag queues many moves per frame.
        let mut panel: Option<(Rect, Waveform)> = quick.tick(&mut dev).map(|r| (r, Waveform::Text));
        // The radio state: every second while a network joins, every half minute on the home.
        let every = Duration::from_secs(if quick.watching_wifi() { 1 } else { 30 });
        let read_wifi = periodic && !asleep && wifi_read.elapsed() >= every && (quick.watching_wifi() || home);
        if read_wifi {
            wifi_read = Instant::now();
        }
        if read_wifi && quick.refresh_wifi() {
            if let Some(tile) = quick.wifi_tile() {
                panel = Some(panel.map_or((tile, Waveform::Text), |(r, w)| (r.union(&tile), w)));
            } else if let (false, Screen::Home(h)) = (quick.is_open(), &mut screen) {
                h.wifi = quick.connected().is_some();
                ready(&mut dev, &mut typing, &mut marker)?;
                h.render(&mut dev.canvas(), &mut ctx);
                let header = Rect::new(0, 0, W, zarathustra::home::HEADER);
                marker = dev.refresh(&Refresh { rect: header, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        let mut closed = false;
        // Everything redrawn and refreshed at once, flashing or not (see `QuickResult::Repaint`).
        let mut repaint: Option<bool> = None;
        // Selection changes from the whole batch, refreshed once like the panel.
        let mut selection: Option<Rect> = None;
        for e in &events {
            let t = match e {
                Event::Touch(t) => t,
                Event::Quit => return Ok(()),
            };
            if quick.is_open() {
                // A finger held on a key of the Wi-Fi password: its accents (below, on the timeout).
                match t.phase {
                    Phase::Down => {
                        down = Some((t.x, t.y));
                        press = Press { at: Some(Instant::now()), ..Press::default() };
                    }
                    Phase::Move => {
                        if down.is_some_and(|s| (t.x - s.0).abs() > SLOP || (t.y - s.1).abs() > SLOP) {
                            press.at = None;
                        }
                    }
                    Phase::Up => {
                        down = None;
                        // The lift after the accents opened: they are tapped next.
                        if std::mem::take(&mut press).fired {
                            continue;
                        }
                    }
                }
                match quick.touch(t, &mut dev) {
                    QuickResult::None => {}
                    QuickResult::Pressed(rect) => {
                        if !quick.keyboard().is_some_and(|kb| kb.draw_key(&mut typing.canvas(), rect)) {
                            quick.render(&mut typing.canvas(), &mut ctx);
                        }
                        typing.typed(&mut dev, &mut marker, Typed::keys(rect))?;
                    }
                    QuickResult::Typed(t) => {
                        quick.render(&mut typing.canvas(), &mut ctx);
                        typing.typed(&mut dev, &mut marker, t)?;
                    }
                    QuickResult::Redraw { rect, waveform } => {
                        panel = Some(match panel {
                            // Any settled (non-drag) change in the batch needs the gray waveform.
                            Some((r, w)) => (r.union(&rect), if w == Waveform::Fast { waveform } else { w }),
                            None => (rect, waveform),
                        });
                    }
                    QuickResult::Close => closed = true,
                    QuickResult::Sleep => {
                        closed = true;
                        dev.sleep()?;
                    }
                    QuickResult::Repaint { flash } => {
                        zarathustra_device::set_night(quick.settings().night);
                        repaint = Some(repaint.unwrap_or(false) || flash);
                    }
                    QuickResult::Transfer => {
                        // From a book: back to the library first, where the screen lives.
                        if !matches!(screen, Screen::Home(_)) {
                            screen = Screen::Home(go_home(&mut shelf, &ctx, quick.connected().is_some()));
                        }
                        if let Screen::Home(h) = &mut screen {
                            h.open_transfer();
                        }
                        repaint = Some(true);
                    }
                }
                continue;
            }
            match t.phase {
                Phase::Down => {
                    down = Some((t.x, t.y));
                    press = Press { at: Some(Instant::now()), ..Press::default() };
                    // On a knob of the selection: dragging moves that end, no long press needed.
                    if let Screen::Reader(r) = &mut screen {
                        if let Some(rect) = r.grab((t.x, t.y)) {
                            press.at = None;
                            press.select(rect, true, &mut selection);
                            continue;
                        }
                    }
                    // A search keyboard key under the finger inverts at once, black and white.
                    let pressed = match &mut screen {
                        Screen::Reader(r) => r.press((t.x, t.y)),
                        Screen::Home(h) => h.press((t.x, t.y)),
                        Screen::Pdf(_) => None,
                    };
                    if let Some(rect) = pressed {
                        // Copied from the keyboard's own image: nothing else is drawn.
                        if !screen.keyboard().is_some_and(|kb| kb.draw_key(&mut typing.canvas(), rect)) {
                            screen.render(&mut typing.canvas(), &mut ctx, quick.settings().clock);
                        }
                        typing.typed(&mut dev, &mut marker, Typed::keys(rect))?;
                    }
                    continue;
                }
                Phase::Move => {
                    let Some(start) = down else { continue };
                    if press.selecting {
                        if let Screen::Reader(r) = &mut screen {
                            if let Some(rect) = r.drag((t.x, t.y)) {
                                selection = Some(selection.map_or(rect, |s| s.union(&rect)));
                            }
                        }
                    } else if (t.x - start.0).abs() > SLOP || (t.y - start.1).abs() > SLOP {
                        press.at = None; // moving: a swipe or a tap, not a long press
                    } else if let Screen::Reader(r) = &mut screen {
                        if press.pending().is_some_and(|at| at.elapsed() >= LONG_PRESS) {
                            press.fire(r, start, &mut selection);
                        }
                    }
                    // Dragging the menu's scrubber: redrawn with the selection's fast waveform.
                    if let Screen::Reader(r) = &mut screen {
                        if let Some(rect) = r.scrub(start, (t.x, t.y)).filter(|_| !press.selecting) {
                            selection = Some(selection.map_or(rect, |s| s.union(&rect)));
                        }
                    }
                    continue;
                }
                Phase::Up => {}
            }
            let Some(start) = down.take() else { continue };
            let lifted = std::mem::take(&mut press);
            lifted.frames.log();
            if lifted.fired {
                // The end of a long press: show the actions for the selection, if any.
                if let Screen::Reader(r) = &mut screen {
                    if let Some(rect) = r.release(&mut ctx) {
                        // Drag frames of this batch are not drawn yet: cover them too.
                        let rect = selection.map_or(rect, |s| s.union(&rect));
                        let rect = if lifted.turned { full } else { rect };
                        ready(&mut dev, &mut typing, &mut marker)?;
                        r.render(&mut dev.canvas(), &mut ctx);
                        marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                    }
                }
                selection = None;
                continue;
            }
            let t0 = Instant::now();
            // Swipe down from the top edge: the quick-settings panel, on every screen.
            if QuickPanel::opens(start, (t.x, t.y)) {
                let rect = quick.open(&dev);
                ready(&mut dev, &mut typing, &mut marker)?;
                quick.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                continue;
            }
            let action = match &mut screen {
                Screen::Reader(r) => match r.swipe_up(start, (t.x, t.y)) {
                    true => Action::Redraw { turn: None },
                    false => r.tap(start, t.x, &mut ctx),
                },
                Screen::Home(h) => h.tap(start, (t.x, t.y), &ctx),
                Screen::Pdf(p) => p.tap(start, t.x),
            };
            // Screen changes flash (clean slate); page turns animate and flash now and then.
            let (turn, flash) = match action {
                Action::None => continue,
                Action::Quit => return Ok(()),
                Action::Sleep => {
                    dev.sleep()?;
                    continue;
                }
                Action::Reboot | Action::PowerOff => {
                    let cmd = if let Action::Reboot = action { "reboot" } else { "poweroff" };
                    // A desktop run just quits.
                    if !zarathustra::on_device() {
                        return Ok(());
                    }
                    match std::process::Command::new(cmd).status() {
                        // Stay put while the system goes down: quitting now would have run.sh
                        // bring the Amazon UI back up in the middle of the shutdown.
                        Ok(s) if s.success() => loop {
                            std::thread::sleep(Duration::from_secs(60));
                        },
                        r => eprintln!("{cmd}: {r:?}"),
                    }
                    continue;
                }
                Action::Typed(t) => {
                    screen.render(&mut typing.canvas(), &mut ctx, quick.settings().clock);
                    typing.typed(&mut dev, &mut marker, t)?;
                    continue;
                }
                Action::Region(rect) => {
                    ready(&mut dev, &mut typing, &mut marker)?;
                    screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
                    marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                    continue;
                }
                Action::Redraw { turn } => {
                    turns += 1;
                    (turn, turns.is_multiple_of(quick.settings().flash_every.into()))
                }
                Action::Open(path, landing) => match Screen::open(&path, &mut ctx) {
                    Ok(mut s) => {
                        match (&mut s, landing) {
                            (Screen::Reader(r), Landing::At(data)) => r.goto(&data, &mut ctx),
                            (Screen::Reader(r), Landing::Contents) => r.show_contents(),
                            (Screen::Reader(r), Landing::Bookmarks) => r.show_toc(Tab::Bookmarks),
                            (Screen::Reader(r), Landing::Search) => r.show_search(),
                            (Screen::Pdf(p), Landing::Contents) => p.show_contents(),
                            _ => {}
                        }
                        if let Screen::Home(h) = std::mem::replace(&mut screen, s) {
                            shelf = Some((h, path));
                        }
                        (None, true)
                    }
                    Err(e) => {
                        eprintln!("{e:#}");
                        continue;
                    }
                },
                Action::Settings(s) => {
                    quick.update(s);
                    if let Screen::Home(h) = &mut screen {
                        h.band =
                            if s.home_quotes { zarathustra::home::Band::Quote } else { zarathustra::home::Band::Stats };
                    }
                    (None, false)
                }
                Action::Home => {
                    screen = Screen::Home(go_home(&mut shelf, &ctx, quick.connected().is_some()));
                    (None, true)
                }
            };
            ready(&mut dev, &mut typing, &mut marker)?;
            screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
            let drawn = t0.elapsed();
            marker = dev.refresh(&Refresh {
                rect: full,
                waveform: if flash { Waveform::Full } else { Waveform::Text },
                flash,
                swipe: turn.filter(|_| swipe && quick.settings().swipe_animation && !flash).map(|f| {
                    if f {
                        Swipe::Left
                    } else {
                        Swipe::Right
                    }
                }),
            })?;
            eprintln!("drawn in {drawn:?}");
        }
        // A finger held still past the long-press delay (the timeout above woke us).
        let held = press.pending().is_some_and(|at| at.elapsed() >= LONG_PRESS);
        if let (Some(start), true, true) = (down, held, quick.is_open()) {
            press.at = None;
            if let Some(rect) = quick.long_press(start.0, start.1) {
                press.fired = true;
                quick.render(&mut typing.canvas(), &mut ctx);
                typing.typed(&mut dev, &mut marker, Typed::keys(rect))?;
            }
        } else if let Some(start) = down.filter(|_| held) {
            match &mut screen {
                Screen::Reader(r) => press.fire(r, start, &mut selection),
                // On a book: its menu, while the finger is still down. Elsewhere (chips, page
                // turns) the press stays a tap.
                Screen::Home(h) => {
                    press.at = None;
                    // A book's menu (the whole screen) or a key's accents (their strip).
                    let rect = match h.long_press(start, &ctx) {
                        Action::Redraw { .. } => Some(full),
                        Action::Region(r) => Some(r),
                        Action::Typed(t) => {
                            press.fired = true;
                            h.render(&mut typing.canvas(), &mut ctx);
                            typing.typed(&mut dev, &mut marker, t)?;
                            None
                        }
                        _ => None,
                    };
                    if let Some(rect) = rect {
                        press.fired = true;
                        ready(&mut dev, &mut typing, &mut marker)?;
                        h.render(&mut dev.canvas(), &mut ctx);
                        marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                    }
                }
                Screen::Pdf(_) => {}
            }
        }
        if let (Screen::Home(h), false) = (&mut screen, quick.is_open() || asleep) {
            if h.poll_transfer() {
                ready(&mut dev, &mut typing, &mut marker)?;
                h.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        if let (Screen::Reader(r), false) = (&mut screen, quick.is_open()) {
            if let Some(rect) = r.search_step() {
                ready(&mut dev, &mut typing, &mut marker)?;
                r.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        if let Screen::Reader(r) = &mut screen {
            if press.selecting && r.edge_turn() {
                press.turned = true;
                selection = Some(full);
            }
        }
        if let Some(rect) = selection.filter(|r| !r.is_empty()) {
            let t0 = Instant::now();
            ready(&mut dev, &mut typing, &mut marker)?;
            let t1 = Instant::now();
            screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
            // Black-and-white while the finger moves; the gray settles on release.
            marker = dev.refresh(&Refresh { rect, waveform: Waveform::Fast, flash: false, swipe: None })?;
            press.frames.add(t1 - t0, t1.elapsed());
        }
        if let Some(rect) = quick.poll() {
            panel = Some(panel.map_or((rect, Waveform::Text), |(r, _)| (r.union(&rect), Waveform::Text)));
        }
        if let Some(flash) = repaint {
            ready(&mut dev, &mut typing, &mut marker)?;
            screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
            if quick.is_open() {
                quick.render(&mut dev.canvas(), &mut ctx);
            }
            let waveform = if flash { Waveform::Full } else { Waveform::Text };
            marker = dev.refresh(&Refresh { rect: full, waveform, flash, swipe: None })?;
        } else if closed {
            ready(&mut dev, &mut typing, &mut marker)?;
            screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
            marker = dev.refresh(&Refresh { rect: PANEL, waveform: Waveform::Text, flash: false, swipe: None })?;
        } else if let Some((rect, waveform)) = panel.filter(|_| quick.is_open()) {
            // Wait before drawing: the MTK driver reads the framebuffer while an update runs,
            // so drawing into a region still refreshing shows half-drawn gray blocks.
            ready(&mut dev, &mut typing, &mut marker)?;
            let t0 = Instant::now();
            quick.render(&mut dev.canvas(), &mut ctx);
            marker = dev.refresh(&Refresh { rect, waveform, flash: false, swipe: None })?;
            eprintln!("panel drawn in {:?}", t0.elapsed());
        }
    }
}
