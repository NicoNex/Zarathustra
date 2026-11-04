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

    pub fn get(&self) -> io::Result<u8> {
        let raw = read_int(&self.dir.join("brightness"))?;
        Ok(((raw * 100 + self.max / 2) / self.max).clamp(0, 100) as u8)
    }

    pub fn set(&self, pct: u8) -> io::Result<()> {
        let raw = (pct.min(100) as i64 * self.max + 50) / 100;
        fs::write(self.dir.join("brightness"), raw.to_string())
    }
}

/// White LED plus an optional warm LED.
#[derive(Debug, Clone)]
pub struct SysfsLight {
    pub white: Led,
    pub warm: Option<Led>,
}

impl SysfsLight {
    pub fn get(&self) -> Option<Light> {
        Some(Light { brightness: self.white.get().ok()?, warmth: self.warm.as_ref().and_then(|w| w.get().ok()) })
    }

    pub fn set(&self, light: Light) -> io::Result<()> {
        self.white.set(light.brightness)?;
        match (&self.warm, light.warmth) {
            (Some(led), Some(w)) => led.set(w),
            _ => Ok(()),
        }
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

    #[test]
    fn led_percent_round_trips_through_raw_range() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("max_brightness"), "2047\n").unwrap();
        fs::write(dir.path().join("brightness"), "0\n").unwrap();
        let led = Led::open(dir.path()).unwrap();
        led.set(50).unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("brightness")).unwrap(), "1024");
        assert_eq!(led.get().unwrap(), 50);
        led.set(100).unwrap();
        assert_eq!(led.get().unwrap(), 100);
        led.set(0).unwrap();
        assert_eq!(led.get().unwrap(), 0);
    }

    #[test]
    fn battery_reads_capacity_and_status() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("capacity"), "86\n").unwrap();
        fs::write(dir.path().join("status"), "Discharging\n").unwrap();
        assert_eq!(battery(dir.path()), Some(Battery { percent: 86, charging: false }));
    }
}
