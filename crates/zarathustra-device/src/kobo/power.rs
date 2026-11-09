//! Sleep on a Kobo: no power daemon, so the backend follows the power button and the sleep
//! cover itself and suspends the kernel. The app sees the same [`PowerEvent`]s as on a Kindle.

use std::fs;
use std::io;
use std::time::{Duration, Instant};

use crate::PowerEvent;

/// From the sleep screen to the kernel suspend: time for the app to draw it and for the
/// panel to finish.
pub const SUSPEND_DELAY: Duration = Duration::from_secs(5);

/// What drives sleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Power button pressed.
    Button,
    /// Sleep cover closed (true) or opened (false).
    Cover(bool),
    /// The kernel came back from a suspend.
    Resumed,
}

#[derive(Debug, Default)]
pub struct Power {
    asleep: bool,
    /// When to suspend the kernel, while asleep.
    suspend_at: Option<Instant>,
}

impl Power {
    pub fn asleep(&self) -> bool {
        self.asleep
    }

    /// Feeds one input at `now`; appends what the app must hear.
    pub fn input(&mut self, input: Input, now: Instant, out: &mut Vec<PowerEvent>) {
        match input {
            Input::Button if !self.asleep => self.sleep(now, out),
            Input::Cover(true) if !self.asleep => self.sleep(now, out),
            Input::Button | Input::Cover(false) if self.asleep => {
                self.asleep = false;
                self.suspend_at = None;
                out.push(PowerEvent::Wake);
            }
            // Woken by something else (USB, charger, alarm): stay on the sleep screen and
            // suspend again after the same delay.
            Input::Resumed if self.asleep => {
                self.suspend_at = Some(now + SUSPEND_DELAY);
                out.push(PowerEvent::Resumed);
            }
            _ => {}
        }
    }

    fn sleep(&mut self, now: Instant, out: &mut Vec<PowerEvent>) {
        self.asleep = true;
        self.suspend_at = Some(now + SUSPEND_DELAY);
        out.push(PowerEvent::Sleep);
        out.push(PowerEvent::Suspend { delay: SUSPEND_DELAY.as_secs() as u32 });
    }

    /// True when the kernel should be suspended now (then feed [`Input::Resumed`]).
    pub fn due(&self, now: Instant) -> bool {
        self.asleep && self.suspend_at.is_some_and(|t| now >= t)
    }

    /// How long until [`Power::due`], for the event loop's timeout.
    pub fn until_due(&self, now: Instant) -> Option<Duration> {
        self.suspend_at.filter(|_| self.asleep).map(|t| t.saturating_duration_since(now))
    }
}

/// Suspends to RAM and returns once the kernel is back (any wake source). The sequence
/// (state-extended first, a pause, sync, then `mem`) is the one the Kobo firmware and
/// KOReader use on these boards.
pub fn suspend() -> io::Result<()> {
    fs::write("/sys/power/state-extended", "1")?;
    std::thread::sleep(Duration::from_secs(2));
    // SAFETY: sync(2) has no arguments and cannot fail.
    unsafe { libc::sync() };
    let slept = fs::write("/sys/power/state", "mem");
    fs::write("/sys/power/state-extended", "0")?;
    // EBUSY: a wake-up event came in meanwhile; the caller simply tries again later.
    slept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_button_sleeps_then_wakes() {
        let (mut p, mut out, t0) = (Power::default(), Vec::new(), Instant::now());
        p.input(Input::Button, t0, &mut out);
        assert_eq!(out, [PowerEvent::Sleep, PowerEvent::Suspend { delay: 5 }]);
        assert!(!p.due(t0 + Duration::from_secs(4)));
        assert!(p.due(t0 + SUSPEND_DELAY));
        out.clear();
        p.input(Input::Resumed, t0 + Duration::from_secs(60), &mut out);
        p.input(Input::Button, t0 + Duration::from_secs(60), &mut out);
        assert_eq!(out, [PowerEvent::Resumed, PowerEvent::Wake]);
        assert!(!p.asleep() && !p.due(t0 + Duration::from_secs(600)));
    }

    #[test]
    fn resume_without_the_button_suspends_again() {
        let (mut p, mut out, t0) = (Power::default(), Vec::new(), Instant::now());
        p.input(Input::Button, t0, &mut out);
        let back = t0 + Duration::from_secs(100);
        out.clear();
        p.input(Input::Resumed, back, &mut out);
        assert_eq!(out, [PowerEvent::Resumed]);
        assert!(p.asleep(), "still on the sleep screen");
        assert!(!p.due(back) && p.due(back + SUSPEND_DELAY));
    }

    #[test]
    fn sleep_cover_closes_and_opens() {
        let (mut p, mut out, t0) = (Power::default(), Vec::new(), Instant::now());
        p.input(Input::Cover(true), t0, &mut out);
        p.input(Input::Cover(true), t0, &mut out);
        p.input(Input::Cover(false), t0, &mut out);
        assert_eq!(out, [PowerEvent::Sleep, PowerEvent::Suspend { delay: 5 }, PowerEvent::Wake]);
    }
}
