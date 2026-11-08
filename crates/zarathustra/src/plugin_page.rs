//! A plugin's page, opened from the settings: its status lines, the service rows, the
//! settings its manifest declares and its actions. Everything is laid out by [`PluginPage::rows`],
//! which both drawing and taps read, so they can't disagree.

use std::collections::HashMap;
use std::path::PathBuf;

use zarathustra_device::{Canvas, Rect};

use crate::i18n::t;
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::plugin::{Kind, Plugin, ACTIVE, AT_BOOT};
use crate::ui::{button, segmented_labels, segments, Label, INK, INK_SOFT, LINE, PAPER};
use crate::{Ctx, H, W};

const STATUS_H: i32 = 58;
const ROW_H: i32 = 150;
const CONTROL_H: i32 = 96;
const CONTROL_W: i32 = 520;
const ITEM_H: i32 = 96;
const REMOVE_W: i32 = 220;
const BACK_W: i32 = 420;
/// The boot runs `zarathustra --start-services` (not yet: see the "start at power-on" row).
const BOOT_HOOK: bool = false;

/// What a part of the page is, and what a tap on it does.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Note(String),
    Status(String, String),
    Rule,
    /// A two-way switch: setting id, label, the two choices' labels.
    Switch(String, String, [String; 2]),
    Choice(usize),
    Number(usize),
    Text(usize),
    ListLabel(String),
    /// An item of list setting `setting`: its id and text.
    Item {
        setting: String,
        id: String,
        text: String,
    },
    Action(usize),
    Message(String),
}

pub struct PluginPage {
    pub plugin: Plugin,
    state_dir: PathBuf,
    values: HashMap<String, String>,
    status: Vec<(String, String)>,
    running: bool,
    lists: HashMap<String, Vec<(String, String)>>,
    /// The last action's output or error.
    message: Option<String>,
}

impl PluginPage {
    pub fn open(plugin: Plugin, ctx: &Ctx) -> PluginPage {
        // Its runtime copy, refreshed once here rather than before every run on the page.
        let prepared = plugin.prepare();
        let mut page = PluginPage {
            plugin,
            state_dir: ctx.state_dir.clone(),
            values: HashMap::new(),
            status: Vec::new(),
            running: false,
            lists: HashMap::new(),
            message: None,
        };
        page.reload();
        if let Err(e) = prepared {
            page.message = Some(e.to_string());
        }
        page
    }

    /// Reads values, status, lists and the service state again (after any change).
    fn reload(&mut self) {
        let p = &self.plugin;
        self.values = p.values(&self.state_dir);
        self.running = p.is_running();
        let mut errors = Vec::new();
        self.status = p.status(&self.state_dir).unwrap_or_else(|e| {
            errors.push(e.to_string());
            Vec::new()
        });
        self.lists.clear();
        for s in p.manifest.iter().flat_map(|m| &m.setting) {
            if let Kind::List { .. } = s.kind {
                match p.list(&self.state_dir, &s.id) {
                    Ok(items) => drop(self.lists.insert(s.id.clone(), items)),
                    Err(e) => errors.push(e.to_string()),
                }
            }
        }
        if let Some(e) = errors.into_iter().next() {
            self.message = Some(e);
        }
    }

    fn flag(&self, key: &str) -> bool {
        self.values.get(key).is_some_and(|v| v == "true")
    }

    /// The page from the top bar down, with each row's height.
    fn rows(&self) -> Vec<(Row, i32)> {
        let s = t();
        let m = match &self.plugin.manifest {
            Ok(m) => m,
            Err(e) => return vec![(Row::Note(e.clone()), STATUS_H * 2)],
        };
        let mut rows = vec![(Row::Note(m.description.clone()), STATUS_H + 20)];
        rows.extend(self.status.iter().map(|(k, v)| (Row::Status(k.clone(), v.clone()), STATUS_H)));
        rows.push((Row::Rule, 40));
        if let Some(service) = &m.service {
            let choices = [s.service_off.to_string(), s.service_on.to_string()];
            rows.push((Row::Switch(ACTIVE.into(), s.service.into(), choices), ROW_H));
            // ponytail: "start at power-on" (AT_BOOT) is stored and honoured by
            // `zarathustra --start-services`, but nothing runs that at boot yet; the row shows
            // once the boot hook (de-Amazon phase A) exists.
            if service.start_at_boot && BOOT_HOOK {
                let choices = [s.off.to_string(), s.on.to_string()];
                rows.push((Row::Switch(AT_BOOT.into(), s.at_power_on.into(), choices), ROW_H));
            }
            if self.flag(ACTIVE) && !self.running {
                rows.push((Row::Message(s.service_stopped.into()), STATUS_H * 2));
            }
        }
        for (i, setting) in m.setting.iter().enumerate() {
            match &setting.kind {
                Kind::Toggle { .. } => {
                    let choices = [s.off.to_string(), s.on.to_string()];
                    rows.push((Row::Switch(setting.id.clone(), setting.label.clone(), choices), ROW_H));
                }
                Kind::Choice { .. } => rows.push((Row::Choice(i), ROW_H)),
                Kind::Number { .. } => rows.push((Row::Number(i), ROW_H)),
                Kind::Text { .. } => rows.push((Row::Text(i), ROW_H)),
                Kind::List { empty_label, .. } => {
                    rows.push((Row::ListLabel(setting.label.clone()), 80));
                    let items = self.lists.get(&setting.id).map(Vec::as_slice).unwrap_or_default();
                    for (id, text) in items {
                        let item = Row::Item { setting: setting.id.clone(), id: id.clone(), text: text.clone() };
                        rows.push((item, ITEM_H));
                    }
                    if items.is_empty() {
                        let hint = empty_label.clone().unwrap_or_else(|| s.nothing_listed.into());
                        rows.push((Row::Note(hint), STATUS_H * 2));
                    }
                }
            }
        }
        rows.extend((0..m.action.len()).map(|i| (Row::Action(i), ITEM_H + 24)));
        if let Some(msg) = &self.message {
            rows.push((Row::Message(msg.clone()), STATUS_H * 2));
        }
        rows
    }

    /// Each row with its screen rectangle; rows past the bottom are left out.
    // ponytail: no scrolling, a page holds what fits (SSH uses about half); page through
    // rows like the TOC if a plugin declares more.
    fn laid_out(&self) -> Vec<(Row, Rect)> {
        let mut y = TOPBAR_H + 30;
        let mut out = Vec::new();
        for (row, h) in self.rows() {
            if y + h > H - 40 {
                break;
            }
            out.push((row, Rect::new(PAD, y, W - 2 * PAD, h)));
            y += h;
        }
        out
    }

    /// The control on the right of a row.
    fn control(r: Rect) -> Rect {
        Rect::new(r.right() - CONTROL_W, r.y + (r.h - CONTROL_H) / 2, CONTROL_W, CONTROL_H)
    }

    /// A tap: `false` for the back link (leave the page), else `true` (redraw).
    pub fn tap(&mut self, (x, y): (i32, i32)) -> bool {
        if y < TOPBAR_H {
            return x >= BACK_W;
        }
        let Some((row, r)) = self.laid_out().into_iter().find(|(_, r)| r.contains(x, y)) else { return true };
        let control = Self::control(r);
        let m = match &self.plugin.manifest {
            Ok(m) => m.clone(),
            Err(_) => return true,
        };
        let picked = |n| segments(control, n).iter().position(|s| s.contains(x, y));
        match row {
            Row::Switch(id, _, _) => {
                if let Some(c) = picked(2) {
                    self.set(&id, if c == 1 { "true" } else { "false" });
                }
            }
            Row::Choice(i) => {
                if let Kind::Choice { options, .. } = &m.setting[i].kind {
                    if let Some(c) = picked(options.len()) {
                        self.set(&m.setting[i].id, &options[c]);
                    }
                }
            }
            Row::Number(i) => {
                if let Kind::Number { minimum, maximum, step, .. } = m.setting[i].kind {
                    let segs = segments(control, 3);
                    let delta = if segs[0].contains(x, y) {
                        -step
                    } else if segs[2].contains(x, y) {
                        step
                    } else {
                        return true;
                    };
                    let id = &m.setting[i].id;
                    let now: i64 = self.values.get(id).and_then(|v| v.parse().ok()).unwrap_or(minimum);
                    self.set(id, &(now + delta).clamp(minimum, maximum).to_string());
                }
            }
            Row::Item { setting, id, .. } => {
                if Self::remove_button(r).contains(x, y) {
                    if let Err(e) = self.plugin.remove(&self.state_dir, &setting, &id) {
                        self.message = Some(e.to_string());
                    }
                    self.reload();
                }
            }
            Row::Action(i) => {
                self.message = Some(self.plugin.run_action(&self.state_dir, i)).filter(|m| !m.is_empty());
                self.reload();
            }
            _ => {}
        }
        true
    }

    /// Saves a value and applies it: the service rows start or stop the service, any other
    /// change restarts a running one (its arguments may have changed).
    fn set(&mut self, id: &str, value: &str) {
        if self.values.get(id).is_some_and(|v| v == value) {
            return;
        }
        let (p, dir) = (&self.plugin, &self.state_dir);
        p.set_value(dir, id, value);
        self.message = None;
        let applied = match id {
            ACTIVE if value == "true" => p.start(dir),
            ACTIVE => {
                p.stop();
                Ok(())
            }
            AT_BOOT => Ok(()),
            _ => p.restart(dir),
        };
        if let Err(e) = applied {
            self.message = Some(e.to_string());
        }
        self.reload();
    }

    fn remove_button(r: Rect) -> Rect {
        Rect::new(r.right() - REMOVE_W, r.y + 12, REMOVE_W, r.h - 24)
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif), Some(serif_bold)) =
            (crate::sans(fonts, false), crate::serif(fonts, false), crate::serif(fonts, true))
        else {
            return;
        };
        let s = t();
        topbar(c, raster, sans, serif_bold, s.back_settings, self.plugin.name());
        let name = Label { face: serif, size: 36.0, ink: INK };
        let soft = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let value = Label { face: sans, size: 30.0, ink: INK };
        let m = self.plugin.manifest.as_ref().ok();
        for (row, r) in self.laid_out() {
            let mid = r.y + r.h / 2 + 12;
            match row {
                Row::Note(ref text) | Row::Message(ref text) => {
                    let label = if matches!(row, Row::Message(_)) { &value } else { &soft };
                    for (i, line) in label.wrap(text, r.w as f32, 2).iter().enumerate() {
                        label.draw(c, raster, line, r.x as f32, r.y + 34 + i as i32 * 42);
                    }
                }
                Row::Status(k, v) => {
                    soft.draw(c, raster, &soft.fit(&k, 330.0), r.x as f32, r.y + 40);
                    value.draw(c, raster, &value.fit(&v, (r.w - 360) as f32), (r.x + 360) as f32, r.y + 40);
                }
                Row::Rule => c.fill(Rect::new(r.x, r.y + r.h / 2, r.w, 2), LINE),
                Row::Switch(id, label, choices) => {
                    name.draw(c, raster, &name.fit(&label, (r.w - CONTROL_W - 30) as f32), r.x as f32, mid);
                    let on = usize::from(self.flag(&id));
                    segmented_labels(c, raster, sans, 28.0, Self::control(r), &choices, Some(on));
                }
                Row::Choice(i) | Row::Number(i) | Row::Text(i) => {
                    let Some(setting) = m.and_then(|m| m.setting.get(i)) else { continue };
                    name.draw(c, raster, &name.fit(&setting.label, (r.w - CONTROL_W - 30) as f32), r.x as f32, mid);
                    let current = self.values.get(&setting.id).cloned().unwrap_or_default();
                    match &setting.kind {
                        Kind::Choice { options, .. } => {
                            let on = options.iter().position(|o| *o == current);
                            segmented_labels(c, raster, sans, 28.0, Self::control(r), options, on);
                        }
                        Kind::Number { .. } => {
                            let labels = ["−".to_string(), current, "+".to_string()];
                            let segs = segments(Self::control(r), 3);
                            for k in [0, 2] {
                                button(c, segs[k], false);
                            }
                            for (k, seg) in segs.into_iter().enumerate() {
                                Label { face: sans, size: if k == 1 { 32.0 } else { 40.0 }, ink: INK }
                                    .centered(c, raster, &labels[k], seg);
                            }
                        }
                        _ => {
                            let text = value.fit(&current, CONTROL_W as f32);
                            let w = value.width(&text);
                            value.draw(c, raster, &text, r.right() as f32 - w, mid);
                        }
                    }
                }
                Row::ListLabel(label) => {
                    name.draw(c, raster, &label, r.x as f32, r.y + 56);
                }
                Row::Item { text, .. } => {
                    value.draw(c, raster, &value.fit(&text, (r.w - REMOVE_W - 30) as f32), r.x as f32, mid - 6);
                    let b = Self::remove_button(r);
                    button(c, b, false);
                    Label { face: sans, size: 28.0, ink: INK }.centered(c, raster, s.remove, b);
                    c.fill(Rect::new(r.x, r.bottom() - 1, r.w, 1), LINE);
                }
                Row::Action(i) => {
                    let Some(a) = m.and_then(|m| m.action.get(i)) else { continue };
                    let b = Rect::new(r.x, r.y + 12, r.w, ITEM_H);
                    button(c, b, false);
                    Label { face: sans, size: 30.0, ink: INK }.centered(c, raster, &a.menu_label, b);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::{discover, Dirs};

    /// The bundled SSH plugin, run in place against a scratch state dir.
    fn ssh_page(name: &str) -> (PathBuf, PluginPage) {
        let root = std::env::temp_dir().join(format!("zarathustra-plugin-page-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let dirs = Dirs {
            installed: concat!(env!("CARGO_MANIFEST_DIR"), "/../../dist/plugins").into(),
            runtime: None,
            data: root.join("data"),
            database: root.join("zarathustra.db"),
        };
        let plugin = discover(&dirs).into_iter().find(|p| p.id == "ssh").expect("bundled ssh plugin");
        let mut ctx = Ctx::new();
        ctx.state_dir = root.clone();
        (root, PluginPage::open(plugin, &ctx))
    }

    fn tap_row(page: &mut PluginPage, want: impl Fn(&Row) -> bool, x: impl Fn(Rect) -> i32) {
        let (_, r) = page.laid_out().into_iter().find(|(row, _)| want(row)).expect("row on the page");
        assert!(page.tap((x(r), r.y + r.h / 2)));
    }

    #[test]
    fn the_ssh_page_shows_its_rows_and_changes_the_port() {
        let (root, mut page) = ssh_page("port");
        let rows: Vec<Row> = page.laid_out().into_iter().map(|(r, _)| r).collect();
        assert!(rows.iter().any(|r| matches!(r, Row::Status(k, _) if k == "Indirizzo")), "{rows:?}");
        assert!(rows.iter().any(|r| matches!(r, Row::Switch(id, ..) if id == ACTIVE)));
        assert_eq!(rows.iter().any(|r| matches!(r, Row::Switch(id, ..) if id == AT_BOOT)), BOOT_HOOK);
        // No keys yet: the hint on how to add one.
        assert!(rows.iter().any(|r| matches!(r, Row::Note(n) if n.contains("authorized_keys"))));
        let choice = |r: &Row| matches!(r, Row::Choice(_));
        // The middle of three options: 2222.
        tap_row(&mut page, choice, |r| {
            let c = PluginPage::control(r);
            c.x + c.w / 2
        });
        assert_eq!(page.values.get("port").map(String::as_str), Some("2222"));
        let right = |r: Rect| PluginPage::control(r).right() - 20;
        tap_row(&mut page, |r| matches!(r, Row::Switch(id, ..) if id == ACTIVE), right);
        assert!(page.flag(ACTIVE));
        page.plugin.stop();
        assert!(!page.tap((60, 60)), "back");
        std::fs::remove_dir_all(root).ok();
    }
}
