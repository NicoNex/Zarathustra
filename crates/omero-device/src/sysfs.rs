//! Frontlight and battery through Linux sysfs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Battery, Light};

fn read_int(path: &Path) -> io::Result<i64> {
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
        let raw = (frac.clamp(0.0, 1.0) * self.max as f32).round() as i64;
        fs::write(self.dir.join("brightness"), raw.to_string())
    }
}

/// White LED plus an optional warm LED. Like the Kindle's own powerd, the two are mixed:
/// brightness sets the total output through a perceptual curve and warmth the share of it
/// the warm LED gives, so moving one slider doesn't change the other.
// ponytail: quadratic curve and equal LED efficiency assumed; per-model gamma and a warm-LED
// gain go here if a calibrated meter says brightness shifts with warmth.
#[derive(Debug, Clone)]
pub struct SysfsLight {
    pub white: Led,
    pub warm: Option<Led>,
}

impl SysfsLight {
    pub fn get(&self) -> Option<Light> {
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
        let total = (light.brightness.min(100) as f32 / 100.0).powi(2);
        let Some(led) = &self.warm else { return self.white.set(total) };
        let w = light.warmth.map_or(0.0, |w| w.min(100) as f32 / 100.0);
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
        let light = SysfsLight { white: led(a.path()), warm: Some(led(b.path())) };
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
}
