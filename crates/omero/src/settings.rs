//! User settings that outlive a session, stored as `key=value` lines in the state dir.

use std::io;
use std::path::{Path, PathBuf};

use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Hardware swipe animation on page turns.
    pub swipe_animation: bool,
    /// Clock in the reader header.
    pub clock: bool,
    /// Warmth follows the time of day instead of `warmth`.
    pub auto_warmth: bool,
    /// Warmth (0–100) the automatic schedule reaches at night.
    pub night_warmth: u8,
    /// Last manual light levels (0–100).
    pub brightness: u8,
    pub warmth: u8,
    /// UI language.
    pub lang: Lang,
    /// Page turns between full (flashing) refreshes that clear e-ink ghosting.
    pub flash_every: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            swipe_animation: true,
            clock: true,
            auto_warmth: false,
            night_warmth: 60,
            brightness: 40,
            warmth: 0,
            lang: Lang::It,
            flash_every: 8,
        }
    }
}

impl Settings {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("settings.txt")
    }

    /// The saved settings; defaults for a missing file and for missing or invalid keys.
    pub fn load(state_dir: &Path) -> Settings {
        std::fs::read_to_string(Self::path(state_dir)).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    /// Writes to a temporary file first so a crash never leaves half a file.
    pub fn save(&self, state_dir: &Path) -> io::Result<()> {
        crate::write_atomic(&Self::path(state_dir), self.to_text())
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for (key, value) in text.lines().filter_map(|l| l.split_once('=')) {
            let (key, value) = (key.trim(), value.trim());
            let flag = |v: &mut bool| {
                if let Ok(b) = value.parse() {
                    *v = b;
                }
            };
            let percent = |v: &mut u8| {
                if let Some(p) = value.parse::<u8>().ok().filter(|p| *p <= 100) {
                    *v = p;
                }
            };
            match key {
                "swipe_animation" => flag(&mut s.swipe_animation),
                "clock" => flag(&mut s.clock),
                "auto_warmth" => flag(&mut s.auto_warmth),
                "night_warmth" => percent(&mut s.night_warmth),
                "brightness" => percent(&mut s.brightness),
                "warmth" => percent(&mut s.warmth),
                "lang" => s.lang = Lang::from_code(value).unwrap_or(s.lang),
                "flash_every" => {
                    s.flash_every = value.parse().ok().filter(|n| (1..=100).contains(n)).unwrap_or(s.flash_every)
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let Settings { swipe_animation, clock, auto_warmth, night_warmth, brightness, warmth, lang, flash_every } =
            self;
        format!(
            "swipe_animation={swipe_animation}\nclock={clock}\nauto_warmth={auto_warmth}\n\
             night_warmth={night_warmth}\nbrightness={brightness}\nwarmth={warmth}\nlang={}\n\
             flash_every={flash_every}\n",
            lang.code()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omero-settings-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_through_text_and_disk() {
        let s = Settings {
            swipe_animation: false,
            clock: false,
            auto_warmth: true,
            night_warmth: 75,
            brightness: 12,
            warmth: 33,
            lang: Lang::En,
            flash_every: 16,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        let dir = scratch("round-trip");
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_or_invalid_values_fall_back_to_defaults() {
        let dir = scratch("missing");
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::remove_dir_all(dir).ok();
        let s = Settings::parse("clock=maybe\nbrightness=250\nwarmth = 20\njunk\nfuture_key=1\n");
        assert_eq!(s, Settings { warmth: 20, ..Settings::default() });
    }
}
