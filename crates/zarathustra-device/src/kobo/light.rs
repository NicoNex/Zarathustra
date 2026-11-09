//! Kobo ComfortLight PRO: a brightness node plus a separate warmth ("color") step.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::Light;

/// The warmth control: `color` from 0 to `max_color` in a LED class node.
pub struct Warm {
    dir: PathBuf,
    max: u8,
    /// 0 is the warmest step (the Clara 2E's `aw99703` driver).
    inverted: bool,
}

pub struct KoboLight {
    /// A backlight class node with `brightness` 0–`max_brightness`.
    white: PathBuf,
    max: u32,
    warm: Option<Warm>,
}

fn read_u32(path: &Path) -> io::Result<u32> {
    fs::read_to_string(path)?.trim().parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

impl KoboLight {
    pub fn open(white: &str, warm: Option<(&str, bool)>) -> io::Result<KoboLight> {
        let white = PathBuf::from(white);
        let max = read_u32(&white.join("max_brightness"))?.max(1);
        let warm = warm.and_then(|(dir, inverted)| {
            let dir = PathBuf::from(dir);
            let max = read_u32(&dir.join("max_color")).ok()?.clamp(1, 255) as u8;
            Some(Warm { dir, max, inverted })
        });
        Ok(KoboLight { white, max, warm })
    }

    pub fn get(&self) -> Option<Light> {
        // `brightness` reads back 0 on the Clara 2E whatever was written; this one is live.
        let raw = read_u32(&self.white.join("actual_brightness")).ok()?;
        let brightness = (raw * 100 / self.max).min(100) as u8;
        let warmth = self.warm.as_ref().and_then(|w| {
            let step = read_u32(&w.dir.join("color")).ok()?.min(w.max as u32) as u8;
            let step = if w.inverted { w.max - step } else { step };
            Some((step as u32 * 100 / w.max as u32) as u8)
        });
        Some(Light { brightness, warmth })
    }

    pub fn set(&self, light: Light) -> io::Result<()> {
        let raw = light.brightness.min(100) as u32 * self.max / 100;
        fs::write(self.white.join("brightness"), raw.to_string())?;
        if let (Some(w), Some(warmth)) = (&self.warm, light.warmth) {
            let step = (warmth.min(100) as u32 * w.max as u32 + 50) / 100;
            let step = if w.inverted { w.max as u32 - step } else { step };
            fs::write(w.dir.join("color"), step.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmth_maps_inverted_steps() {
        let (white, warm) = (tempfile::tempdir().expect("dir"), tempfile::tempdir().expect("dir"));
        fs::write(white.path().join("max_brightness"), "100\n").expect("write");
        fs::write(warm.path().join("max_color"), "10\n").expect("write");
        let light =
            KoboLight::open(white.path().to_str().expect("utf8"), Some((warm.path().to_str().expect("utf8"), true)))
                .expect("open");
        let color = || fs::read_to_string(warm.path().join("color")).expect("read");
        for (warmth, step) in [(100, "0"), (0, "10"), (50, "5")] {
            light.set(Light { brightness: 40, warmth: Some(warmth) }).expect("set");
            assert_eq!(color(), step, "warmth {warmth}%");
            // The driver mirrors `brightness` into `actual_brightness`; the test does it by hand.
            fs::copy(white.path().join("brightness"), white.path().join("actual_brightness")).expect("copy");
            assert_eq!(light.get(), Some(Light { brightness: 40, warmth: Some(warmth) }));
        }
    }
}
