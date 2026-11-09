//! Reading speed: a moving average of the seconds spent on a page, per device, used to
//! estimate the time left in a chapter; and the clock of the page in front, paused while
//! something covers it, that both the speed and the reading statistics read.
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
/// Reading statistics: a page shown less than this is a skim and adds nothing; a longer one
/// counts at most `STAT_MAX` (the reader probably looked away), as KOReader does.
pub const STAT_MIN: Duration = Duration::from_secs(5);
pub const STAT_MAX: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq)]
pub struct Pace {
    secs_per_page: f32,
    samples: u32,
    /// Time the current page has been in front, up to the last pause.
    shown: Duration,
    /// Since when the page is in front, while its clock runs.
    running: Option<Instant>,
    state_dir: PathBuf,
}

impl Pace {
    pub fn load(state_dir: &Path) -> Pace {
        let saved = crate::load_pref(state_dir, "pace").and_then(|s| {
            let mut it = s.split_whitespace();
            let secs: f32 = it.next()?.parse().ok()?;
            let samples: u32 = it.next()?.parse().ok()?;
            (secs.is_finite() && secs > 0.0).then_some((secs, samples))
        });
        let (secs_per_page, samples) = saved.unwrap_or((DEFAULT_SECS, 0));
        Pace { secs_per_page, samples, shown: Duration::ZERO, running: None, state_dir: state_dir.to_path_buf() }
    }

    /// The page is in front again: its clock runs. Idempotent.
    pub fn resume(&mut self, now: Instant) {
        self.running.get_or_insert(now);
    }

    /// Something covers the page (a menu, sleep): its clock stops. Idempotent.
    pub fn pause(&mut self, now: Instant) {
        if let Some(t) = self.running.take() {
            self.shown += now.saturating_duration_since(t);
        }
    }

    /// The page stops being in front for good (turned forward or back, a jump, the book
    /// closed). Returns the time to log in the statistics: nothing for a skim, else capped.
    /// A forward turn within SKIM..=AWAY is also a speed sample, saved. The clock starts
    /// over for the next page, running or paused as it was.
    pub fn leave(&mut self, now: Instant, forward: bool) -> Option<Duration> {
        let running = self.running.is_some();
        self.pause(now);
        let d = std::mem::take(&mut self.shown);
        if running {
            self.running = Some(now);
        }
        if forward && (SKIM..=AWAY).contains(&d) {
            self.samples = self.samples.saturating_add(1);
            // A plain mean over the first samples, so the default doesn't linger; then an EWMA.
            let alpha = (1.0 / self.samples as f32).max(ALPHA);
            self.secs_per_page += alpha * (d.as_secs_f32() - self.secs_per_page);
            crate::save_pref(&self.state_dir, "pace", &format!("{:.2} {}\n", self.secs_per_page, self.samples));
        }
        (d >= STAT_MIN).then(|| d.min(STAT_MAX))
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
        let dir = std::env::temp_dir().join(format!("zarathustra-pace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = Pace::load(&dir);
        assert_eq!(p.minutes(10), 10); // default 60 s/page
        let mut t = Instant::now();
        p.resume(t);
        for secs in [30, 1, 30, 3600, 30] {
            t += Duration::from_secs(secs);
            p.leave(t, true);
        }
        // Three 30 s samples; the 1 s skim and the hour away are dropped.
        assert_eq!(p.samples, 3);
        assert!((p.secs_per_page() - 30.0).abs() < 0.01);
        assert_eq!(p.minutes(10), 5);
        let back = Pace::load(&dir);
        assert!((back.secs_per_page() - 30.0).abs() < 0.01);
        std::fs::remove_dir_all(dir).ok();
    }

    fn pace(name: &str) -> (Pace, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("zarathustra-pace-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        (Pace::load(&dir), dir)
    }

    #[test]
    fn paused_time_not_counted() {
        let (mut p, dir) = pace("paused");
        let t = Instant::now();
        let at = |s| t + Duration::from_secs(s);
        p.resume(at(0));
        p.pause(at(30));
        p.pause(at(100)); // idempotent
        p.resume(at(300));
        p.resume(at(305)); // idempotent
        assert_eq!(p.leave(at(310), true), Some(Duration::from_secs(40)));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn skim_counts_nothing() {
        let (mut p, dir) = pace("skim");
        let t = Instant::now();
        p.resume(t);
        assert_eq!(p.leave(t + Duration::from_secs(4), true), None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_page_capped() {
        let (mut p, dir) = pace("capped");
        let t = Instant::now();
        p.resume(t);
        assert_eq!(p.leave(t + Duration::from_secs(900), true), Some(STAT_MAX));
        // The clock restarted for the next page.
        assert_eq!(p.leave(t + Duration::from_secs(930), true), Some(Duration::from_secs(30)));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn backward_leave_logs_but_no_speed_sample() {
        let (mut p, dir) = pace("backward");
        let t = Instant::now();
        p.resume(t);
        assert_eq!(p.leave(t + Duration::from_secs(30), false), Some(Duration::from_secs(30)));
        assert_eq!(p.samples, 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
