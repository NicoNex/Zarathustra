//! The Extensions screen, opened from the settings: every installed plugin with its service
//! switch, its action buttons and how its last action went; a tap on a plugin's name opens its
//! page ([`PluginPage`]), a tap on the last run opens its full output ([`OutputSheet`]).
//!
//! The action block (buttons, then the last run's line with Stop or Details) is shared with the
//! plugin page. Running actions are polled by the app while any runs ([`Extensions::poll`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use zarathustra_device::{Canvas, Rect};
use zarathustra_engine::font::{Face, Rasterizer};

use crate::i18n::{fill, t};
use crate::menu::{topbar, BACK_W, PAD, TOPBAR_H};
use crate::outsheet::OutputSheet;
use crate::plugin::{discover, Dirs, LastRun, Outcome, Output, Place, Plugin, ACTIVE};
use crate::plugin_page::PluginPage;
use crate::toc::{next_button, pager, prev_button, FOOTER_H};
use crate::ui::{button, rounded_rect, segmented_labels, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

/// Action buttons: two per row.
const BUTTON_H: i32 = 96;
const GAP: i32 = 20;
/// The last run's line under the buttons.
const RUN_H: i32 = 84;
/// Stop or Details, at the right of that line.
const RUN_BUTTON_W: i32 = 210;
/// A plugin's head: its name, its service switch.
const HEAD_H: i32 = 100;
const ABOUT_H: i32 = 56;
const SWITCH_W: i32 = 380;
/// Space after a plugin, its rule in the middle.
const SPACE: i32 = 48;

/// What a tap in an action block does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Run(usize),
    Stop,
    Sheet,
}

/// The actions shown in `place`, with their manifest indices.
pub fn actions_in(plugin: &Plugin, place: Option<Place>) -> Vec<usize> {
    let Ok(m) = &plugin.manifest else { return Vec::new() };
    (0..m.action.len()).filter(|&i| place.is_none_or(|p| m.action[i].shown_in.contains(&p))).collect()
}

/// Every plugin action offered in the book menu: the plugin, the action's index, its label.
pub fn book_actions(ctx: &Ctx) -> Vec<(Plugin, usize, String)> {
    let plugins = discover(&Dirs::new(&ctx.state_dir));
    (plugins.into_iter())
        .flat_map(|p| {
            let shown = actions_in(&p, Some(Place::Book));
            shown.into_iter().filter_map(move |i| {
                let label = p.action_label(p.manifest.as_ref().ok()?.action.get(i)?);
                Some((p.clone(), i, label))
            })
        })
        .collect()
}

/// Height of an action block with `n` buttons.
pub fn block_height(n: usize, run: Option<&LastRun>) -> i32 {
    let rows = n.div_ceil(2) as i32;
    rows * (BUTTON_H + GAP) + if run.is_some() { RUN_H } else { 0 }
}

/// The block's parts at `top` across `x..x+w`: one button per action of `shown`, then the run
/// line, which opens the output (with Stop at its right while running).
pub fn block_layout(x: i32, top: i32, w: i32, shown: &[usize], run: Option<&LastRun>) -> Vec<(Hit, Rect)> {
    let bw = (w - GAP) / 2;
    let mut out: Vec<(Hit, Rect)> = (shown.iter().enumerate())
        .map(|(k, &i)| {
            let (row, col) = (k as i32 / 2, k as i32 % 2);
            (Hit::Run(i), Rect::new(x + col * (bw + GAP), top + row * (BUTTON_H + GAP), bw, BUTTON_H))
        })
        .collect();
    if let Some(r) = run {
        let y = top + shown.len().div_ceil(2) as i32 * (BUTTON_H + GAP);
        // Stop first, so a tap on it isn't taken by the line around it.
        if r.running() {
            out.push((Hit::Stop, Rect::new(x + w - RUN_BUTTON_W, y + 6, RUN_BUTTON_W, RUN_H - 12)));
        }
        out.push((Hit::Sheet, Rect::new(x, y, w, RUN_H)));
    }
    out
}

/// "3 s" under a minute, else as the reading statistics write it ("4 min", "2 h 05 min").
pub fn duration_text(d: Duration) -> String {
    match d.as_secs() {
        s @ 0..60 => format!("{s} s"),
        s => crate::stats::duration(s as i64),
    }
}

/// The last run in one line, and whether it failed (drawn behind an "Error" chip).
pub fn run_line(plugin: &Plugin, run: &LastRun) -> (bool, String) {
    let s = t();
    let label = run.label.as_str();
    let took = duration_text(run.duration.unwrap_or_default());
    let show =
        plugin.manifest.as_ref().ok().and_then(|m| m.action.get(run.action)).is_some_and(|a| a.output == Output::Show);
    match &run.outcome {
        Outcome::Running => {
            let since = run.started.elapsed().unwrap_or_default();
            (false, fill(s.run_running, &[label, &duration_text(since)]))
        }
        Outcome::Exited(0) if show && !run.stdout_head.is_empty() => (false, run.stdout_head.clone()),
        Outcome::Exited(0) => (false, fill(s.run_done, &[label, &took])),
        Outcome::Stopped => (false, fill(s.run_stopped, &[label])),
        Outcome::TimedOut => (true, fill(s.run_timed_out, &[label, &took])),
        Outcome::SpawnFailed(e) => (true, format!("{label}: {e}")),
        Outcome::Exited(code) if run.stderr_tail.is_empty() => (true, fill(s.run_exit, &[label, &code.to_string()])),
        Outcome::Exited(_) => (true, format!("{label}: {}", run.stderr_tail)),
    }
}

/// Draws an action block laid out by [`block_layout`].
pub fn draw_block(
    c: &mut Canvas,
    raster: &mut Rasterizer,
    sans: &Face,
    plugin: &Plugin,
    parts: &[(Hit, Rect)],
    run: Option<&LastRun>,
) {
    let s = t();
    let Ok(m) = &plugin.manifest else { return };
    let busy = run.is_some_and(LastRun::running);
    let text = Label { face: sans, size: 30.0, ink: INK };
    for &(hit, r) in parts {
        match hit {
            Hit::Run(i) => {
                let Some(a) = m.action.get(i) else { continue };
                let this = busy && run.is_some_and(|r| r.action == i);
                button(c, r, false);
                let name = plugin.action_label(a);
                let label = if this { s.running } else { name.as_str() };
                // Another action of this plugin running: these wait, drawn soft.
                let ink = if busy && !this { INK_SOFT } else { INK };
                Label { ink, ..text }.centered(c, raster, label, r);
            }
            Hit::Stop => {
                button(c, r, true);
                Label { ink: PAPER, ..text }.centered(c, raster, s.stop, r);
            }
            Hit::Sheet => {}
        }
    }
    let line = parts.iter().find(|(h, _)| *h == Hit::Sheet).map(|&(_, r)| r);
    let (Some(run), Some(line)) = (run, line) else { return };
    let (failed, msg) = run_line(plugin, run);
    let mut x = line.x;
    let baseline = line.y + line.h / 2 + 11;
    if failed {
        // The one loud thing on the screen: a failure is never missed.
        let chip = Label { face: sans, size: 26.0, ink: PAPER };
        let w = chip.width(s.error) as i32 + 32;
        rounded_rect(c, Rect::new(x, line.y + (line.h - 48) / 2, w, 48), 10, INK);
        chip.draw(c, raster, s.error, (x + 16) as f32, baseline - 1);
        x += w + 16;
    }
    let right = line.right() - RUN_BUTTON_W - 16;
    let msg_label = Label { face: sans, size: 28.0, ink: if failed { INK } else { INK_SOFT } };
    msg_label.draw(c, raster, &msg_label.fit(&msg, (right - x) as f32), x as f32, baseline);
    if !run.running() {
        let more = Label { face: sans, size: 28.0, ink: INK };
        let w = more.width(s.details);
        more.draw(c, raster, s.details, line.right() as f32 - w, baseline);
    }
}

/// Applies a tap on an action block; returns the sheet to open, if the tap asks for one.
pub fn tap_block(plugin: &Plugin, state_dir: &Path, hit: Hit, run: Option<&LastRun>) -> Option<OutputSheet> {
    match hit {
        Hit::Run(i) => {
            if let Err(e) = plugin.run(state_dir, i, None) {
                eprintln!("plugin {}: {e}", plugin.id);
            }
        }
        Hit::Stop => plugin.stop_action(),
        Hit::Sheet => return run.map(|r| OutputSheet::new(plugin, r)),
    }
    None
}

/// What a part of the Extensions screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The plugin's name and description: opens its page.
    Open(usize),
    /// Its service switch, Off then On.
    Service(usize),
    Action(usize, Hit),
    Prev,
    Next,
}

pub struct Extensions {
    plugins: Vec<Plugin>,
    runs: Vec<Option<LastRun>>,
    services: Vec<bool>,
    page: usize,
    detail: Option<Box<PluginPage>>,
    sheet: Option<OutputSheet>,
    state_dir: PathBuf,
}

impl Extensions {
    pub fn new(ctx: &Ctx) -> Extensions {
        let plugins = discover(&Dirs::new(&ctx.state_dir));
        let mut e = Extensions {
            runs: Vec::new(),
            services: Vec::new(),
            plugins,
            page: 0,
            detail: None,
            sheet: None,
            state_dir: ctx.state_dir.clone(),
        };
        e.reload();
        e
    }

    fn reload(&mut self) {
        self.runs = self.plugins.iter().map(Plugin::last_run).collect();
        self.services = self.plugins.iter().map(Plugin::is_running).collect();
    }

    /// The number of plugins, for the settings row.
    pub fn count(ctx: &Ctx) -> usize {
        discover(&Dirs::new(&ctx.state_dir)).len()
    }

    fn has_service(&self, i: usize) -> bool {
        self.plugins[i].manifest.as_ref().is_ok_and(|m| m.service.is_some())
    }

    fn height(&self, i: usize) -> i32 {
        let shown = actions_in(&self.plugins[i], Some(Place::Extensions));
        HEAD_H + ABOUT_H + 16 + block_height(shown.len(), self.runs[i].as_ref()) + SPACE
    }

    /// Plugins per page, filled by height.
    fn pages(&self) -> Vec<Vec<usize>> {
        let room = H - TOPBAR_H - 30 - FOOTER_H;
        let mut pages = vec![Vec::new()];
        let mut used = 0;
        for i in 0..self.plugins.len() {
            let h = self.height(i).min(room); // ponytail: a plugin taller than a page is cut
            if used + h > room && !pages.last().is_some_and(Vec::is_empty) {
                pages.push(Vec::new());
                used = 0;
            }
            used += h;
            if let Some(p) = pages.last_mut() {
                p.push(i);
            }
        }
        pages
    }

    fn parts(&self) -> Vec<(Part, Rect)> {
        let pages = self.pages();
        let page = self.page.min(pages.len() - 1);
        let w = W - 2 * PAD;
        let mut y = TOPBAR_H + 30;
        let mut out = Vec::new();
        for &i in &pages[page] {
            let mut open_w = w;
            if self.has_service(i) {
                out.push((Part::Service(i), Rect::new(PAD + w - SWITCH_W, y + (HEAD_H - 84) / 2, SWITCH_W, 84)));
                open_w -= SWITCH_W + 24;
            }
            out.push((Part::Open(i), Rect::new(PAD, y, open_w, HEAD_H + ABOUT_H)));
            let shown = actions_in(&self.plugins[i], Some(Place::Extensions));
            let top = y + HEAD_H + ABOUT_H + 16;
            for (hit, r) in block_layout(PAD, top, w, &shown, self.runs[i].as_ref()) {
                out.push((Part::Action(i, hit), r));
            }
            y += self.height(i);
        }
        if pages.len() > 1 {
            out.extend([(Part::Prev, prev_button()), (Part::Next, next_button())]);
        }
        out
    }

    /// A tap: `false` for the back link (leave the screen), else `true` (redraw).
    pub fn tap(&mut self, (x, y): (i32, i32), ctx: &Ctx) -> bool {
        if OutputSheet::forward(&mut self.sheet, (x, y)) {
            return true;
        }
        if let Some(page) = &mut self.detail {
            if !page.tap((x, y)) {
                self.detail = None;
                self.reload();
            }
            return true;
        }
        if y < TOPBAR_H {
            return x >= BACK_W;
        }
        let Some((part, r)) = self.parts().into_iter().find(|(_, r)| r.contains(x, y)) else { return true };
        match part {
            Part::Open(i) => self.detail = Some(Box::new(PluginPage::open(self.plugins[i].clone(), ctx))),
            Part::Service(i) => {
                let on = x >= r.x + r.w / 2;
                let p = &self.plugins[i];
                p.set_value(&self.state_dir, ACTIVE, if on { "true" } else { "false" });
                if on {
                    if let Err(e) = p.start(&self.state_dir) {
                        eprintln!("plugin {}: {e}", p.id);
                    }
                } else {
                    p.stop();
                }
            }
            Part::Action(i, hit) => {
                self.sheet = tap_block(&self.plugins[i], &self.state_dir, hit, self.runs[i].as_ref());
            }
            Part::Prev => self.page = self.page.saturating_sub(1),
            Part::Next => self.page = (self.page + 1).min(self.pages().len() - 1),
        }
        self.reload();
        true
    }

    /// Some action shown here is running: the app polls every half second.
    pub fn running(&self) -> bool {
        self.runs.iter().flatten().any(LastRun::running) || self.detail.as_ref().is_some_and(|d| d.running())
    }

    /// Looks again at running actions; `true` when the screen must be redrawn.
    pub fn poll(&mut self) -> bool {
        if let Some(d) = &mut self.detail {
            return d.poll();
        }
        if self.sheet.is_some() || !self.running() {
            return false;
        }
        // The elapsed time on the run line changes too: redraw while running. Only the
        // running plugins are read again.
        for (p, run) in self.plugins.iter().zip(&mut self.runs) {
            if run.as_ref().is_some_and(LastRun::running) {
                *run = p.last_run();
            }
        }
        true
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        if let Some(sheet) = &self.sheet {
            return sheet.render(c, ctx);
        }
        if let Some(page) = &self.detail {
            return page.render(c, ctx);
        }
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_settings, s.extensions);
        let name = Label { face: serif, size: 40.0, ink: INK };
        let about = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let parts = self.parts();
        for &(part, r) in &parts {
            let Part::Open(i) = part else { continue };
            let p = &self.plugins[i];
            name.draw(c, raster, &name.fit(&p.display_name(), r.w as f32), r.x as f32, r.y + 66);
            let text = p.description();
            about.draw(c, raster, &about.fit(&text, r.w as f32), r.x as f32, r.y + HEAD_H + 26);
            if self.has_service(i) {
                let sw = parts.iter().find(|(p, _)| *p == Part::Service(i)).map_or_else(Rect::default, |&(_, r)| r);
                let choices = [s.service_off, s.service_on];
                segmented_labels(c, raster, sans, 28.0, sw, &choices, Some(usize::from(self.services[i])));
            }
            let block: Vec<(Hit, Rect)> = (parts.iter())
                .filter_map(|&(p, r)| match p {
                    Part::Action(j, hit) if j == i => Some((hit, r)),
                    _ => None,
                })
                .collect();
            draw_block(c, raster, sans, p, &block, self.runs[i].as_ref());
            let bottom = r.y + self.height(i) - SPACE / 2;
            c.fill(Rect::new(PAD, bottom, W - 2 * PAD, 2), LINE);
        }
        let pages = self.pages().len();
        pager(c, raster, sans, self.page.min(pages - 1), pages);
        if self.plugins.is_empty() {
            about.draw(c, raster, s.nothing_listed, PAD as f32, TOPBAR_H + 80);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::tests::setup;

    #[test]
    fn blocks_lay_out_two_buttons_a_row_and_the_run_line() {
        let parts = block_layout(64, 300, 1108, &[0, 1, 2], None);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].1.y, parts[0].1.y, "two per row");
        assert!(parts[2].1.y > parts[0].1.y);
        assert_eq!(block_height(3, None), 2 * (BUTTON_H + GAP));
        assert_eq!(duration_text(Duration::from_secs(3725)), "1 h 02 min");
    }

    #[test]
    fn a_failed_run_says_why_and_a_tap_opens_its_output() {
        let (root, p) = setup("ext-fail");
        let state = root.join("state");
        p.run(&state, 1, None).expect("run");
        let run = loop {
            match p.last_run() {
                Some(r) if !r.running() => break r,
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let (failed, line) = run_line(&p, &run);
        assert!(failed);
        assert_eq!(line, "Fail: disk full");
        let parts = block_layout(64, 300, 1108, &[1], Some(&run));
        let (hit, _) = parts.last().copied().expect("run line");
        assert_eq!(hit, Hit::Sheet);
        assert!(tap_block(&p, &state, hit, Some(&run)).is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_screen_lists_the_bundled_plugins_and_opens_one() {
        let mut ctx = Ctx::new();
        let root = std::env::temp_dir().join(format!("zarathustra-extensions-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("dir");
        ctx.state_dir = root.clone();
        let mut e = Extensions::new(&ctx);
        let (part, r) = e.parts().into_iter().find(|(p, _)| matches!(p, Part::Open(_))).expect("a plugin");
        assert!(matches!(part, Part::Open(_)));
        assert!(e.tap((r.x + 10, r.y + 10), &ctx) && e.detail.is_some());
        assert!(e.tap((300, 60), &ctx), "back from the plugin page");
        assert!(e.detail.is_none());
        assert!(!e.tap((100, 60), &ctx), "back to the settings");
        std::fs::remove_dir_all(root).ok();
    }
}
