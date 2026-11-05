//! Reading speed: a moving average of the seconds spent on a page, per device, used to
//! estimate the time left in a chapter.
//!
//! Stored in the state dir as one line, `seconds_per_page samples`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Assumed speed until there are enough samples (a typical paperback page).
const DEFAULT_SECS: f32 = 60.0;
/// Samples before the measured speed replaces the default.
const MIN_SAMPLES: u32 = 3;
/// Weight of a new sample once the average has warmed up (about the last 10 pages).
const ALPHA: f32 = 0.1;
/// Faster than this is skimming; slower is the reader away from the page.
const SKIM: Duration = Duration::from_secs(2);
const AWAY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub struct Pace {
    secs_per_page: f32,
    samples: u32,
    /// When the current page was turned to.
    since: Option<Instant>,
    path: PathBuf,
}

impl Pace {
    pub fn load(state_dir: &Path) -> Pace {
        let path = state_dir.join("pace.txt");
        let saved = std::fs::read_to_string(&path).ok().and_then(|s| {
            let mut it = s.split_whitespace();
            let secs: f32 = it.next()?.parse().ok()?;
            let samples: u32 = it.next()?.parse().ok()?;
            (secs.is_finite() && secs > 0.0).then_some((secs, samples))
        });
        let (secs_per_page, samples) = saved.unwrap_or((DEFAULT_SECS, 0));
        Pace { secs_per_page, samples, since: None, path }
    }

    /// Starts timing the page shown now (after opening a book or turning back).
    pub fn restart(&mut self, now: Instant) {
        self.since = Some(now);
    }

    /// A forward turn at `now`: the time on the page just left becomes a sample, unless it
    /// was a skim or the reader was away. Saves when the average changed. Returns the sample,
    /// the time actually spent reading that page.
    pub fn turned(&mut self, now: Instant) -> Option<Duration> {
        let sample = self.since.map(|t| now.saturating_duration_since(t));
        self.since = Some(now);
        let d = sample.filter(|d| (SKIM..=AWAY).contains(d))?;
        self.samples = self.samples.saturating_add(1);
        // A plain mean over the first samples, so the default doesn't linger; then an EWMA.
        let alpha = (1.0 / self.samples as f32).max(ALPHA);
        self.secs_per_page += alpha * (d.as_secs_f32() - self.secs_per_page);
        crate::write_atomic(&self.path, format!("{:.2} {}\n", self.secs_per_page, self.samples)).ok();
        Some(d)
    }

    /// Seconds per page: measured, or the default until there are enough samples.
    pub fn secs_per_page(&self) -> f32 {
        if self.samples < MIN_SAMPLES {
            DEFAULT_SECS
        } else {
            self.secs_per_page
        }
    }

    /// Whole minutes (at least 1) to read `pages` pages.
    pub fn minutes(&self, pages: usize) -> u32 {
        ((pages as f32 * self.secs_per_page() / 60.0).round() as u32).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_speed_ignores_skims_and_persists() {
        let dir = std::env::temp_dir().join(format!("omero-pace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Pace::load(&dir);
        assert_eq!(p.minutes(10), 10); // default 60 s/page
        let mut t = Instant::now();
        p.restart(t);
        for secs in [30, 1, 30, 3600, 30] {
            t += Duration::from_secs(secs);
            p.turned(t);
        }
        // Three 30 s samples; the 1 s skim and the hour away are dropped.
        assert_eq!(p.samples, 3);
        assert!((p.secs_per_page() - 30.0).abs() < 0.01);
        assert_eq!(p.minutes(10), 5);
        let back = Pace::load(&dir);
        assert!((back.secs_per_page() - 30.0).abs() < 0.01);
        std::fs::remove_dir_all(dir).ok();
    }
}
