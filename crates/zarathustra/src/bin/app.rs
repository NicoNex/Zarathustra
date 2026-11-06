//! Zarathustra: home and reader on the device.
//!
//!   zarathustra [BOOK] [--no-swipe] [--pgm DIR] [--search WORD]
//!
//! With `--pgm`, renders the home screen and the first pages of BOOK into DIR instead.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use zarathustra::highlight::{Button, LONG_PRESS, SLOP};
use zarathustra::home::Home;
use zarathustra::pdfview::PdfView;
use zarathustra::quick::{QuickPanel, QuickResult, PANEL};
use zarathustra::reader::{Reader, HEADER};
use zarathustra::sleep::render_sleep;
use zarathustra::toc::Tab;
use zarathustra::{Action, Ctx, Landing, H, W};
use zarathustra_device::{Canvas, Event, Phase, PowerEvent, Rect, Refresh, Swipe, Waveform};
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
            self.fired = true;
            // Accents on the search keyboard are tapped next, not dragged over.
            self.selecting = !typing;
            *selection = Some(selection.map_or(rect, |s| s.union(&rect)));
        }
    }
}

fn library_dirs() -> Vec<PathBuf> {
    // Only the books the user put in library/; Amazon's documents/ is left alone.
    vec![zarathustra::library_dir()]
}

fn write_pgm(path: PathBuf, px: &[u8]) -> Result<()> {
    let mut out = format!("P5\n{W} {H}\n255\n").into_bytes();
    out.extend_from_slice(px);
    Ok(std::fs::write(path, out)?)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut book, mut swipe, mut pgm, mut query) = (None, true, None, "natura".to_string());
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-swipe" => swipe = false,
            "--search" => query = args.next().context("--search WORD")?,
            "--pgm" => pgm = Some(PathBuf::from(args.next().context("--pgm DIR")?)),
            path => book = Some(PathBuf::from(path)),
        }
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
        home.render(&mut Canvas { width: W, height: H, stride: W as usize, pixels: &mut px }, &mut ctx);
        write_pgm(dir.join("home.pgm"), &px)?;
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

    let mut dev = zarathustra_device::open_default()?;
    let mut quick = QuickPanel::new(&ctx.state_dir, dev.as_mut());
    let full = Rect::new(0, 0, W, H);
    screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
    let mut marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
    let mut turns = 0u32;
    let mut down = None;
    let mut press = Press::default();
    let mut events = Vec::new();
    let mut asleep = false;
    loop {
        events.clear();
        // Wake every few minutes so automatic warmth follows the clock, or when a finger held
        // still on a page becomes a long press.
        let long_press = match (&screen, press.pending()) {
            (Screen::Reader(_) | Screen::Home(_), Some(at)) if !quick.is_open() => {
                Some(LONG_PRESS.saturating_sub(at.elapsed()))
            }
            _ => None,
        };
        // A search under way only peeks at the input queue between chapters.
        let searching = matches!(&screen, Screen::Reader(r) if r.is_searching()) && !quick.is_open();
        // The send-books screen checks for arriving books every second; the home looks for
        // new books every half minute (a walk and a stat per file).
        let idle = match &screen {
            Screen::Home(h) if h.transferring() => Duration::from_secs(1),
            Screen::Home(_) => Duration::from_secs(30),
            _ => Duration::from_secs(300),
        };
        let timeout = if searching { Duration::ZERO } else { long_press.unwrap_or(idle) };
        dev.wait_events(Some(timeout), &mut events)?;
        let mut power = false;
        while let Some(ev) = dev.power_event() {
            power = true;
            dev.wait_refresh(marker)?;
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
                PowerEvent::Suspend { .. } | PowerEvent::Resumed => continue,
            }
            marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Full, flash: true, swipe: None })?;
            down = None;
            press = Press::default();
        }
        // The periodic wake-up (not a long-press or power one): bring the reader's clock up to date.
        let periodic = events.is_empty() && long_press.is_none() && !power && !searching;
        // On the home, books that arrived meanwhile.
        if let (true, false, Screen::Home(h)) = (periodic && !quick.is_open(), asleep, &mut screen) {
            if h.refresh(&ctx) {
                dev.wait_refresh(marker)?;
                h.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        if periodic && !asleep && !quick.is_open() && quick.settings().clock {
            if let Screen::Reader(_) | Screen::Pdf(_) = screen {
                dev.wait_refresh(marker)?;
                screen.render(&mut dev.canvas(), &mut ctx, true);
                marker = dev.refresh(&Refresh { rect: HEADER, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        // Panel changes from the whole batch, drawn once: a drag queues many moves per frame.
        let mut panel: Option<(Rect, Waveform)> = quick.tick(dev.as_mut()).map(|r| (r, Waveform::Text));
        let mut closed = false;
        // Selection changes from the whole batch, refreshed once like the panel.
        let mut selection: Option<Rect> = None;
        for e in &events {
            let t = match e {
                Event::Touch(t) => t,
                Event::Quit => return Ok(()),
            };
            if quick.is_open() {
                match quick.touch(t, dev.as_mut()) {
                    QuickResult::None => {}
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
                }
                down = None;
                continue;
            }
            match t.phase {
                Phase::Down => {
                    down = Some((t.x, t.y));
                    press = Press { at: Some(Instant::now()), ..Press::default() };
                    // A search keyboard key under the finger inverts at once, black and white.
                    let pressed = match &mut screen {
                        Screen::Reader(r) => r.press((t.x, t.y)),
                        Screen::Home(h) => h.press((t.x, t.y)),
                        Screen::Pdf(_) => None,
                    };
                    if let Some(rect) = pressed {
                        dev.wait_refresh(marker)?;
                        screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
                        marker = dev.refresh(&Refresh { rect, waveform: Waveform::Fast, flash: false, swipe: None })?;
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
            if std::mem::take(&mut press).fired {
                // The end of a long press: show the actions for the selection, if any.
                if let Screen::Reader(r) = &mut screen {
                    if let Some(rect) = r.release(&mut ctx) {
                        // Drag frames of this batch are not drawn yet: cover them too.
                        let rect = selection.map_or(rect, |s| s.union(&rect));
                        dev.wait_refresh(marker)?;
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
                let rect = quick.open(dev.as_ref());
                dev.wait_refresh(marker)?;
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
                Action::Region(rect) => {
                    dev.wait_refresh(marker)?;
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
                    (None, false)
                }
                Action::Home => {
                    let h = match shelf.take() {
                        Some((mut h, path)) => {
                            h.returned(&path, &ctx);
                            h.refresh(&ctx);
                            h
                        }
                        None => Box::new(home(&ctx)),
                    };
                    screen = Screen::Home(h);
                    (None, true)
                }
            };
            dev.wait_refresh(marker)?;
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
        let held = press.pending().is_some_and(|at| at.elapsed() >= LONG_PRESS) && !quick.is_open();
        if let Some(start) = down.filter(|_| held) {
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
                        _ => None,
                    };
                    if let Some(rect) = rect {
                        press.fired = true;
                        dev.wait_refresh(marker)?;
                        h.render(&mut dev.canvas(), &mut ctx);
                        marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
                    }
                }
                Screen::Pdf(_) => {}
            }
        }
        if let (Screen::Home(h), false) = (&mut screen, quick.is_open() || asleep) {
            if h.poll_transfer() {
                dev.wait_refresh(marker)?;
                h.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect: full, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        if let (Screen::Reader(r), false) = (&mut screen, quick.is_open()) {
            if let Some(rect) = r.search_step() {
                dev.wait_refresh(marker)?;
                r.render(&mut dev.canvas(), &mut ctx);
                marker = dev.refresh(&Refresh { rect, waveform: Waveform::Text, flash: false, swipe: None })?;
            }
        }
        if let Some(rect) = selection.filter(|r| !r.is_empty()) {
            dev.wait_refresh(marker)?;
            screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
            // Black-and-white while the finger moves; the gray settles on release.
            marker = dev.refresh(&Refresh { rect, waveform: Waveform::Fast, flash: false, swipe: None })?;
        }
        if closed {
            dev.wait_refresh(marker)?;
            screen.render(&mut dev.canvas(), &mut ctx, quick.settings().clock);
            marker = dev.refresh(&Refresh { rect: PANEL, waveform: Waveform::Text, flash: false, swipe: None })?;
        } else if let Some((rect, waveform)) = panel.filter(|_| quick.is_open()) {
            // Wait before drawing: the MTK driver reads the framebuffer while an update runs,
            // so drawing into a region still refreshing shows half-drawn gray blocks.
            dev.wait_refresh(marker)?;
            let t0 = Instant::now();
            quick.render(&mut dev.canvas(), &mut ctx);
            marker = dev.refresh(&Refresh { rect, waveform, flash: false, swipe: None })?;
            eprintln!("panel drawn in {:?}", t0.elapsed());
        }
    }
}
