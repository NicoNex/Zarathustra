//! Frontlight and battery through Linux sysfs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Battery, Light};

pub(crate) fn read_int(path: &Path) -> io::Result<i64> {
    fs::read_to_string(path)?.trim().parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// One backlight LED (`.../backlight/<name>/{brightness,max_brightness}`).
#[derive(Debug, Clone)]
pub struct Led {
    dir: PathBuf,
    max: i64,
}

impl Led {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        let max = read_int(&dir.join("max_brightness"))?.max(1);
        Ok(Led { dir, max })
    }

    /// Output as a fraction of the maximum, 0–1.
    pub fn get(&self) -> io::Result<f32> {
        let raw = read_int(&self.dir.join("brightness"))?;
        Ok((raw as f32 / self.max as f32).clamp(0.0, 1.0))
    }

    pub fn set(&self, frac: f32) -> io::Result<()> {
        self.set_raw(frac.clamp(0.0, 1.0) * self.max as f32)
    }

    fn get_raw(&self) -> io::Result<f32> {
        Ok(read_int(&self.dir.join("brightness"))? as f32)
    }

    fn set_raw(&self, raw: f32) -> io::Result<()> {
        fs::write(self.dir.join("brightness"), (raw.round() as i64).clamp(0, self.max).to_string())
    }
}

/// The vendor's own frontlight mix: the raw (white, warm) levels its power daemon writes for
/// each of `steps`×`steps` (intensity, warmth) settings, measured on the device. The raw
/// scale isn't linear in light, so blending by hand dims the light while warming it; this
/// interpolates the table instead, so a slider matches the stock firmware exactly.
#[derive(Debug, Clone)]
pub struct Mix {
    steps: usize,
    /// Row-major by intensity, then warmth.
    raw: Vec<[f32; 2]>,
}

impl Mix {
    /// Lines `intensity warmth white warm`, `#` comments; a full square grid from 0.
    pub fn parse(text: &str) -> Option<Mix> {
        let rows: Vec<[f32; 4]> = (text.lines())
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .map(|l| {
                let mut it = l.split_whitespace().map(|v| v.parse::<f32>().ok());
                Some([it.next()??, it.next()??, it.next()??, it.next()??])
            })
            .collect::<Option<_>>()?;
        let steps = (rows.len() as f64).sqrt() as usize;
        let in_order = |(n, r): (usize, &[f32; 4])| r[0] as usize == n / steps && r[1] as usize == n % steps;
        (steps >= 2 && steps * steps == rows.len() && rows.iter().enumerate().all(in_order))
            .then(|| Mix { steps, raw: rows.iter().map(|r| [r[2], r[3]]).collect() })
    }

    /// Raw (white, warm) for brightness and warmth in 0–1, bilinear between grid points.
    fn at(&self, b: f32, w: f32) -> [f32; 2] {
        let last = (self.steps - 1) as f32;
        let (x, y) = (b.clamp(0.0, 1.0) * last, w.clamp(0.0, 1.0) * last);
        let (x0, y0) = ((x as usize).min(self.steps - 2), (y as usize).min(self.steps - 2));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |i: usize, j: usize| self.raw[i * self.steps + j];
        let lerp = |a: [f32; 2], b: [f32; 2], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        lerp(lerp(p(x0, y0), p(x0, y0 + 1), fy), lerp(p(x0 + 1, y0), p(x0 + 1, y0 + 1), fy), fx)
    }

    /// The setting whose mix is closest to `raw` (reading back what is on the LEDs).
    fn inverse(&self, raw: [f32; 2]) -> (u8, u8) {
        let err = |(b, w): (u8, u8)| {
            let m = self.at(b as f32 / 100.0, w as f32 / 100.0);
            (m[0] - raw[0]).powi(2) + (m[1] - raw[1]).powi(2)
        };
        // ponytail: brute force over 101² settings (~10k lerps), only on startup and sleep.
        let all = (0..=100u8).flat_map(|b| (0..=100u8).map(move |w| (b, w)));
        all.min_by(|a, b| err(*a).total_cmp(&err(*b))).unwrap_or((0, 0))
    }
}

/// White LED plus an optional warm LED. With a measured [`Mix`] the two follow the vendor's
/// table; without one, brightness sets the total output on a quadratic curve and warmth the
/// warm LED's share of it.
#[derive(Debug, Clone)]
pub struct SysfsLight {
    pub white: Led,
    pub warm: Option<Led>,
    pub mix: Option<Mix>,
}

impl SysfsLight {
    pub fn get(&self) -> Option<Light> {
        if let (Some(mix), Some(warm)) = (&self.mix, &self.warm) {
            let (b, w) = mix.inverse([self.white.get_raw().ok()?, warm.get_raw().ok()?]);
            return Some(Light { brightness: b, warmth: Some(w) });
        }
        let white = self.white.get().ok()?;
        let warm = self.warm.as_ref().and_then(|w| w.get().ok());
        let total = (white + warm.unwrap_or(0.0)).min(1.0);
        let pct = |f: f32| (f * 100.0).round() as u8;
        Some(Light {
            brightness: pct(total.sqrt()),
            warmth: warm.map(|w| if total > 0.0 { pct(w / total) } else { 0 }),
        })
    }

    pub fn set(&self, light: Light) -> io::Result<()> {
        let w = light.warmth.map_or(0.0, |w| w.min(100) as f32 / 100.0);
        if let (Some(mix), Some(warm)) = (&self.mix, &self.warm) {
            let [white_raw, warm_raw] = mix.at(light.brightness.min(100) as f32 / 100.0, w);
            self.white.set_raw(white_raw)?;
            return warm.set_raw(warm_raw);
        }
        let total = (light.brightness.min(100) as f32 / 100.0).powi(2);
        let Some(led) = &self.warm else { return self.white.set(total) };
        self.white.set(total * (1.0 - w))?;
        led.set(total * w)
    }
}

/// `dir` is a power_supply node with `capacity` and `status`.
pub fn battery(dir: &Path) -> Option<Battery> {
    let percent = read_int(&dir.join("capacity")).ok()?.clamp(0, 100) as u8;
    let status = fs::read_to_string(dir.join("status")).unwrap_or_default();
    Some(Battery { percent, charging: matches!(status.trim(), "Charging" | "Full") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn led(dir: &Path) -> Led {
        fs::write(dir.join("max_brightness"), "2047\n").unwrap();
        fs::write(dir.join("brightness"), "0\n").unwrap();
        Led::open(dir).unwrap()
    }

    #[test]
    fn warmth_mixes_leds_without_changing_brightness() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let light = SysfsLight { white: led(a.path()), warm: Some(led(b.path())), mix: None };
        let raw = |d: &tempfile::TempDir| fs::read_to_string(d.path().join("brightness")).unwrap();
        for warmth in [0, 30, 100] {
            let l = Light { brightness: 50, warmth: Some(warmth) };
            light.set(l).unwrap();
            let sum: i64 = raw(&a).parse::<i64>().unwrap() + raw(&b).parse::<i64>().unwrap();
            assert!((sum - 512).abs() <= 1, "total output stays at 25% of max, got {sum}");
            assert_eq!(light.get(), Some(l));
        }
        light.set(Light { brightness: 100, warmth: Some(0) }).unwrap();
        assert_eq!((raw(&a).as_str(), raw(&b).as_str()), ("2047", "0"));
        light.set(Light { brightness: 0, warmth: Some(40) }).unwrap();
        assert_eq!(light.get(), Some(Light { brightness: 0, warmth: Some(0) }));
    }

    #[test]
    fn battery_reads_capacity_and_status() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("capacity"), "86\n").unwrap();
        fs::write(dir.path().join("status"), "Discharging\n").unwrap();
        assert_eq!(battery(dir.path()), Some(Battery { percent: 86, charging: false }));
    }

    #[test]
    fn pw5_mix_matches_powerd_and_reads_back() {
        let mix = Mix::parse(include_str!("kindle/pw5-light.txt")).expect("table parses");
        // Grid points are powerd's own values: intensity 24 / amber 12 and 4 / 0.
        assert_eq!(mix.at(1.0, 0.5), [1730.0, 1642.0]);
        assert_eq!(mix.at(4.0 / 24.0, 0.0), [549.0, 0.0]);
        assert_eq!(mix.at(0.0, 0.7), [0.0, 0.0]);
        // Raising warmth never dims the white LED below what powerd itself does.
        let w0 = mix.at(0.5, 0.0)[0];
        assert!(mix.at(0.5, 0.1)[0] <= w0 && mix.at(0.5, 0.1)[0] > w0 * 0.9);
        for (b, w) in [(50, 30), (100, 0), (10, 90)] {
            let (rb, rw) = mix.inverse(mix.at(b as f32 / 100.0, w as f32 / 100.0));
            assert!(rb.abs_diff(b) <= 1 && rw.abs_diff(w) <= 1, "{b},{w} read back as {rb},{rw}");
        }
    }
}
