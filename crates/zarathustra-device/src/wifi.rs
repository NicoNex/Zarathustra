//! Wi-Fi: on/off, the network joined, scanning, joining and the saved networks.
//!
//! Every call may block for seconds (a scan, a connection) and may fail: callers run them
//! off the UI thread and show the error. Handles are shared (`Arc<dyn Wifi>`) so a worker
//! thread can hold one.

use std::io;
use std::sync::Mutex;
use std::time::Duration;

/// A network in range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub ssid: String,
    /// Signal strength, 0–100.
    pub signal: u8,
    /// Needs a password (WPA/WEP); open networks don't.
    pub secured: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WifiStatus {
    /// The radio is on.
    pub enabled: bool,
    /// The network joined, if any.
    pub connected: Option<String>,
}

pub trait Wifi: Send + Sync {
    fn status(&self) -> io::Result<WifiStatus>;
    fn set_enabled(&self, on: bool) -> io::Result<()>;
    /// Scans and returns the networks in range, strongest first.
    fn scan(&self) -> io::Result<Vec<Network>>;
    /// The SSIDs of the saved networks.
    fn saved(&self) -> io::Result<Vec<String>>;
    /// Joins `ssid` and waits until connected (or fails). A new network is saved first,
    /// with `password` when it is secured; a saved one reuses its stored password.
    fn connect(&self, ssid: &str, password: Option<&str>) -> io::Result<()>;
    /// Removes a saved network.
    fn forget(&self, ssid: &str) -> io::Result<()>;
}

/// A pretend Wi-Fi for the emulator, tests and `--pgm` renders: a few networks in range
/// (one open, some secured, one already saved) and a saved one out of range. Joining a new
/// secured network takes the password "password".
pub struct Fake {
    state: Mutex<FakeState>,
    /// How long scans and connections pretend to take.
    delay: Duration,
}

struct FakeState {
    status: WifiStatus,
    saved: Vec<String>,
}

const IN_RANGE: [(&str, u8, bool); 4] =
    [("Casa", 92, true), ("Vodafone-A1B2", 64, true), ("Biblioteca", 41, false), ("TIM-Ospiti", 18, true)];

impl Fake {
    pub fn new(delay: Duration) -> Fake {
        let state = FakeState {
            status: WifiStatus { enabled: true, connected: Some("Casa".into()) },
            saved: vec!["Casa".into(), "Ufficio".into()],
        };
        Fake { state: Mutex::new(state), delay }
    }

    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, FakeState>> {
        self.state.lock().map_err(|_| io::Error::other("fake wifi: poisoned"))
    }
}

impl Wifi for Fake {
    fn status(&self) -> io::Result<WifiStatus> {
        Ok(self.lock()?.status.clone())
    }

    fn set_enabled(&self, on: bool) -> io::Result<()> {
        let mut s = self.lock()?;
        s.status = WifiStatus { enabled: on, connected: on.then(|| "Casa".into()) };
        Ok(())
    }

    fn scan(&self) -> io::Result<Vec<Network>> {
        std::thread::sleep(self.delay);
        if !self.lock()?.status.enabled {
            return Ok(Vec::new());
        }
        Ok(IN_RANGE.iter().map(|&(ssid, signal, secured)| Network { ssid: ssid.into(), signal, secured }).collect())
    }

    fn saved(&self) -> io::Result<Vec<String>> {
        Ok(self.lock()?.saved.clone())
    }

    fn connect(&self, ssid: &str, password: Option<&str>) -> io::Result<()> {
        std::thread::sleep(self.delay);
        let mut s = self.lock()?;
        if !s.status.enabled {
            return Err(io::Error::other("wifi is off"));
        }
        let Some(&(_, _, secured)) = IN_RANGE.iter().find(|n| n.0 == ssid) else {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{ssid}: not in range")));
        };
        let known = s.saved.iter().any(|n| n == ssid);
        if secured && !known && password != Some("password") {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "wrong password"));
        }
        if !known {
            s.saved.push(ssid.into());
        }
        s.status.connected = Some(ssid.into());
        Ok(())
    }

    fn forget(&self, ssid: &str) -> io::Result<()> {
        let mut s = self.lock()?;
        s.saved.retain(|n| n != ssid);
        if s.status.connected.as_deref() == Some(ssid) {
            s.status.connected = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_joins_with_the_right_password_and_forgets() {
        let w = Fake::new(Duration::ZERO);
        assert_eq!(w.status().unwrap().connected.as_deref(), Some("Casa"));
        assert!(w.connect("Vodafone-A1B2", Some("nope")).is_err());
        assert!(w.connect("Vodafone-A1B2", None).is_err());
        w.connect("Vodafone-A1B2", Some("password")).unwrap();
        assert_eq!(w.status().unwrap().connected.as_deref(), Some("Vodafone-A1B2"));
        assert!(w.saved().unwrap().contains(&"Vodafone-A1B2".to_string()));
        // Saved now: no password needed any more. Open networks never need one.
        w.connect("Vodafone-A1B2", None).unwrap();
        w.connect("Biblioteca", None).unwrap();
        assert!(w.connect("Ufficio", None).is_err(), "saved but out of range");
        w.forget("Biblioteca").unwrap();
        assert_eq!(w.status().unwrap().connected, None);
        w.set_enabled(false).unwrap();
        assert!(w.scan().unwrap().is_empty());
    }
}
