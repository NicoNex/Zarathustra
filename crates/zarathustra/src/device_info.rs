//! "About this device", opened from the top of the settings: model, system and app versions,
//! battery, storage, memory, network and screen, read once when it opens. Everything comes from
//! the usual Linux files, so a value the device does not have is just left out.

use std::path::Path;

use zarathustra_device::{Canvas, Rect};

use crate::i18n::{fill, t};
use crate::menu::{topbar, PAD, TOPBAR_H};
use crate::toc::BACK_W;
use crate::ui::{Label, INK, INK_SOFT, LINE, PAPER};
use crate::{screen_h, screen_w, Ctx};

const ROW_H: i32 = 84;
const VALUE_X: i32 = 420;

pub struct DeviceInfo {
    rows: Vec<(&'static str, String)>,
}

impl DeviceInfo {
    pub fn new(ctx: &Ctx) -> DeviceInfo {
        let s = t();
        let mut rows = Vec::new();
        if !ctx.device.is_empty() {
            rows.push((s.device_model, ctx.device.clone()));
        }
        // "Kindle 5.17.1.0.4 (…)" on Kindles, "Kobo 4.38.23852" on a Kobo.
        if let Some(v) = zarathustra_device::platform().system_version() {
            rows.push((s.device_system, v));
        }
        rows.push((s.device_app, format!("Zarathustra {}", env!("CARGO_PKG_VERSION"))));
        if let Some(b) = battery() {
            let charging = if b.charging { format!(", {}", s.device_charging) } else { String::new() };
            rows.push((s.device_battery, format!("{} %{charging}", b.percent)));
        }
        if let Some((free, total)) = storage(&crate::library_dir()) {
            rows.push((s.device_storage, fill(s.device_free_of, &[&size(free), &size(total)])));
        }
        if let Some((free, total)) = memory() {
            rows.push((s.device_memory, fill(s.device_free_of, &[&size(free), &size(total)])));
        }
        rows.push((
            s.device_address,
            crate::transfer::local_ip().map_or_else(|| s.wifi_not_connected.into(), |ip| ip.to_string()),
        ));
        if let Some(mac) = first_line("/sys/class/net/wlan0/address") {
            rows.push((s.device_mac, mac.to_uppercase()));
        }
        let dpi = if ctx.dpi > 0 { format!(", {} dpi", ctx.dpi) } else { String::new() };
        rows.push((s.device_screen, format!("{} × {}{dpi}", screen_w(), screen_h())));
        if let Some(up) = uptime() {
            rows.push((s.device_uptime, up));
        }
        DeviceInfo { rows }
    }

    /// Only the back link does anything.
    pub fn back(&self, (x, y): (i32, i32)) -> bool {
        y < TOPBAR_H && x < BACK_W
    }

    pub fn render(&self, c: &mut Canvas, ctx: &mut Ctx) {
        c.fill(c.bounds(), PAPER);
        let Ctx { fonts, raster, .. } = ctx;
        let (Some(sans), Some(serif_bold)) = (crate::sans(fonts, false), crate::serif(fonts, true)) else { return };
        topbar(c, raster, sans, serif_bold, t().back_settings, t().device_title);
        let name = Label { face: sans, size: 28.0, ink: INK_SOFT };
        let value = Label { face: sans, size: 30.0, ink: INK };
        for (i, (k, v)) in self.rows.iter().enumerate() {
            let y = TOPBAR_H + 30 + i as i32 * ROW_H;
            let base = y + ROW_H / 2 + 10;
            name.draw(c, raster, &name.fit(k, (VALUE_X - PAD - 20) as f32), PAD as f32, base);
            value.draw(c, raster, &value.fit(v, (screen_w() - PAD - VALUE_X) as f32), VALUE_X as f32, base);
            c.fill(Rect::new(PAD, y + ROW_H - 1, screen_w() - 2 * PAD, 1), LINE);
        }
    }
}

fn first_line(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.lines().next()?.trim().to_string()).filter(|l| !l.is_empty())
}

/// The first power supply that is a battery.
fn battery() -> Option<zarathustra_device::Battery> {
    let dirs = std::fs::read_dir("/sys/class/power_supply").ok()?;
    dirs.flatten()
        .map(|e| e.path())
        .find(|d| std::fs::read_to_string(d.join("type")).is_ok_and(|t| t.trim() == "Battery"))
        .and_then(|d| zarathustra_device::sysfs::battery(&d))
}

/// Free and total bytes of the file system holding `path`.
// The counts are u32 on the Kindle's 32-bit ARM, u64 on the desktop.
#[allow(clippy::unnecessary_cast)]
fn storage(path: &Path) -> Option<(u64, u64)> {
    let v = nix::sys::statvfs::statvfs(path).ok()?;
    let block = v.fragment_size() as u64;
    Some((v.blocks_available() as u64 * block, v.blocks() as u64 * block))
}

/// Available and total memory, in bytes.
fn memory() -> Option<(u64, u64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |key: &str| -> Option<u64> {
        let line = info.lines().find(|l| l.starts_with(key))?;
        Some(line.split_whitespace().nth(1)?.parse::<u64>().ok()? * 1024)
    };
    Some((kb("MemAvailable:")?, kb("MemTotal:")?))
}

fn uptime() -> Option<String> {
    let secs: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    let m = (secs / 60.0) as u64;
    Some(match (m / 1440, m / 60 % 24, m % 60) {
        (0, 0, m) => format!("{m} min"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, h, _) => format!("{d} d {h} h"),
    })
}

/// Bytes as MB or GB, one decimal.
fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_machine() {
        let info = DeviceInfo::new(&Ctx::new());
        let s = t();
        let has = |k: &str| info.rows.iter().any(|(name, _)| *name == k);
        assert!(has(s.device_app) && has(s.device_screen) && has(s.device_address), "{:?}", info.rows);
        assert_eq!(size(3 * 1_073_741_824 / 2), "1.5 GB");
        assert_eq!(size(512 * 1_048_576), "512 MB");
    }
}
