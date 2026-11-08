//! User settings that outlive a session, stored as `key=value` lines in the database.

use std::path::Path;

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
    /// The home's first page quotes a highlight (else shows the reading figures).
    pub home_quotes: bool,
    /// Night mode: the whole screen inverted, white on black.
    pub night: bool,
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
            home_quotes: true,
            night: false,
        }
    }
}

impl Settings {
    /// The saved settings, defaults for missing or invalid keys; `None` when never saved.
    pub fn load_saved(state_dir: &Path) -> Option<Settings> {
        crate::load_pref(state_dir, "settings").map(|s| Self::parse(&s))
    }

    /// The saved settings, or the defaults.
    pub fn load(state_dir: &Path) -> Settings {
        Self::load_saved(state_dir).unwrap_or_default()
    }

    pub fn save(&self, state_dir: &Path) {
        crate::save_pref(state_dir, "settings", &self.to_text());
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
                "night" => flag(&mut s.night),
                "night_warmth" => percent(&mut s.night_warmth),
                "brightness" => percent(&mut s.brightness),
                "warmth" => percent(&mut s.warmth),
                "lang" => s.lang = Lang::from_code(value).unwrap_or(s.lang),
                "home_quotes" => flag(&mut s.home_quotes),
                "flash_every" => {
                    s.flash_every = value.parse().ok().filter(|n| (1..=100).contains(n)).unwrap_or(s.flash_every)
                }
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let Settings {
            swipe_animation,
            clock,
            auto_warmth,
            night_warmth,
            brightness,
            warmth,
            lang,
            flash_every,
            home_quotes,
            night,
        } = self;
        format!(
            "swipe_animation={swipe_animation}\nclock={clock}\nauto_warmth={auto_warmth}\n\
             night_warmth={night_warmth}\nbrightness={brightness}\nwarmth={warmth}\nlang={}\n\
             flash_every={flash_every}\nhome_quotes={home_quotes}\nnight={night}\n",
            lang.code()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zarathustra-settings-{name}-{}", std::process::id()));
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
            home_quotes: false,
            night: true,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
        let dir = scratch("round-trip");
        s.save(&dir);
        assert_eq!(Settings::load(&dir), s);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_or_invalid_values_fall_back_to_defaults() {
        let dir = scratch("missing");
        assert_eq!(Settings::load_saved(&dir), None);
        // A settings.txt from an older build is taken into the database, once.
        std::fs::write(dir.join("settings.txt"), "clock=false\n").unwrap();
        assert!(!Settings::load(&dir).clock);
        assert!(!dir.join("settings.txt").exists());
        assert!(!Settings::load(&dir).clock);
        std::fs::remove_dir_all(dir).ok();
        let s = Settings::parse("clock=maybe\nbrightness=250\nwarmth = 20\njunk\nfuture_key=1\n");
        assert_eq!(s, Settings { warmth: 20, ..Settings::default() });
    }
}
