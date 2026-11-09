//! Wi-Fi through `wpa_cli`, talking to a running wpa_supplicant: scans, saved networks,
//! joining and forgetting. Used by the Kobo backend, and by the Kindle's when its own
//! daemon refuses a password.

use std::io;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::sys::run;
use crate::wifi::Network;

/// How long a scan is given before its results are read.
// shortcut: a fixed wait instead of the supplicant's scan-done event (needs an attached
// control socket); fine for a screen that shows a spinner meanwhile.
const SCAN_WAIT: Duration = Duration::from_secs(3);

type Cli = Box<dyn Fn(&[&str]) -> io::Result<String> + Send + Sync>;

pub struct Wpa {
    cli: Cli,
}

impl Wpa {
    /// The supplicant serving `iface`.
    pub fn new(iface: &str) -> Wpa {
        let iface = iface.to_string();
        Wpa { cli: Box::new(move |args| run(Command::new("wpa_cli").args(["-i", &iface]).args(args), None)) }
    }

    fn cli(&self, args: &[&str]) -> io::Result<String> {
        (self.cli)(args)
    }

    /// Runs a command that answers `OK`.
    fn ok(&self, args: &[&str]) -> io::Result<()> {
        let out = self.cli(args)?;
        match out.lines().last() {
            Some(l) if l.trim() == "OK" => Ok(()),
            _ => Err(io::Error::other(format!("wpa_cli {}: {out}", args[0]))),
        }
    }

    /// The network joined, if any.
    pub fn connected(&self) -> io::Result<Option<String>> {
        let status = self.cli(&["status"])?;
        let line = |key: &str| status.lines().find_map(|l| l.strip_prefix(key)).map(str::to_string);
        Ok(if line("wpa_state=").as_deref() == Some("COMPLETED") { line("ssid=") } else { None })
    }

    pub fn scan(&self) -> io::Result<Vec<Network>> {
        self.ok(&["scan"])?;
        std::thread::sleep(SCAN_WAIT);
        Ok(parse_scan_results(&self.cli(&["scan_results"])?))
    }

    pub fn saved(&self) -> io::Result<Vec<String>> {
        Ok(parse_networks(&self.cli(&["list_networks"])?).into_iter().map(|(_, ssid)| ssid).collect())
    }

    /// Joins `ssid` and waits for the association: a saved network with its stored
    /// password, a new one with `password` (then saved).
    pub fn connect(&self, ssid: &str, password: Option<&str>, timeout: Duration) -> io::Result<()> {
        let saved = parse_networks(&self.cli(&["list_networks"])?);
        match saved.iter().find(|(_, s)| s == ssid) {
            Some((id, _)) => {
                self.ok(&["select_network", id])?;
                self.wait_for(ssid, timeout)
            }
            None => self.join(ssid, password, timeout),
        }
    }

    /// Adds `ssid` as a new network (`wpa_cli` arguments go straight through, so any
    /// character in the password is fine), joins it and saves the config; removed again on
    /// failure.
    pub fn join(&self, ssid: &str, password: Option<&str>, timeout: Duration) -> io::Result<()> {
        let id = self.cli(&["add_network"])?.lines().last().unwrap_or_default().trim().to_string();
        id.parse::<u32>().map_err(|_| io::Error::other(format!("wpa_cli add_network: {id}")))?;
        let joined = (|| {
            self.ok(&["set_network", &id, "ssid", &format!("\"{ssid}\"")])?;
            match password {
                Some(p) => self.ok(&["set_network", &id, "psk", &format!("\"{p}\"")])?,
                None => self.ok(&["set_network", &id, "key_mgmt", "NONE"])?,
            }
            self.ok(&["select_network", &id])?;
            self.wait_for(ssid, timeout)
        })();
        if joined.is_err() {
            self.cli(&["remove_network", &id]).ok();
            // Back to the networks known before.
            self.cli(&["enable_network", "all"]).ok();
        }
        joined
    }

    fn wait_for(&self, ssid: &str, timeout: Duration) -> io::Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(500).min(timeout));
            if self.connected().ok().flatten().as_deref() == Some(ssid) {
                // select_network leaves the others disabled: let them be joined again later.
                self.cli(&["enable_network", "all"]).ok();
                self.cli(&["save_config"]).ok();
                return Ok(());
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, format!("{ssid}: not connected after {timeout:?}")))
    }

    pub fn forget(&self, ssid: &str) -> io::Result<()> {
        for (id, _) in parse_networks(&self.cli(&["list_networks"])?).iter().filter(|(_, s)| s == ssid) {
            self.ok(&["remove_network", id])?;
        }
        self.ok(&["save_config"])
    }
}

/// `scan_results`: a header, then `bssid\tfrequency\tsignal dBm\tflags\tssid` per access
/// point. Hidden networks are skipped; one row per name, the strongest, strongest first.
fn parse_scan_results(text: &str) -> Vec<Network> {
    let mut nets: Vec<Network> = (text.lines().skip(1))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let [_, _, dbm, flags, ssid, ..] = f.as_slice() else { return None };
            let ssid = ssid.trim();
            if ssid.is_empty() {
                return None;
            }
            let dbm: i32 = dbm.trim().parse().ok()?;
            let signal = ((dbm + 100) * 2).clamp(0, 100) as u8;
            let secured = ["WPA", "WEP", "RSN", "SAE"].iter().any(|k| flags.contains(k));
            Some(Network { ssid: ssid.to_string(), signal, secured })
        })
        .collect();
    nets.sort_by_key(|n| std::cmp::Reverse(n.signal));
    let mut seen = std::collections::HashSet::new();
    nets.retain(|n| seen.insert(n.ssid.clone()));
    nets
}

/// `list_networks`: a header, then `id\tssid\tbssid\tflags`.
fn parse_networks(text: &str) -> Vec<(String, String)> {
    (text.lines().skip(1))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let id = f.next()?.trim();
            let ssid = f.next()?;
            id.parse::<u32>().ok().map(|_| (id.to_string(), ssid.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scan_results() {
        let text = "bssid / frequency / signal level / flags / ssid\n\
                    aa:aa:aa:aa:aa:01\t2412\t-80\t[WPA2-PSK-CCMP][ESS]\tCasa\n\
                    aa:aa:aa:aa:aa:02\t5180\t-40\t[WPA2-PSK-CCMP][ESS]\tCasa\n\
                    aa:aa:aa:aa:aa:03\t2437\t-60\t[ESS]\tBiblioteca\n\
                    aa:aa:aa:aa:aa:04\t2462\t-30\t[WPA2-PSK-CCMP][ESS]\t\n";
        let nets = parse_scan_results(text);
        assert_eq!(
            nets,
            [
                Network { ssid: "Casa".into(), signal: 100, secured: true },
                Network { ssid: "Biblioteca".into(), signal: 80, secured: false },
            ]
        );
    }

    #[test]
    fn parses_saved_networks() {
        let text = "network id / ssid / bssid / flags\n0\tCasa\tany\t[CURRENT]\n1\tUfficio 5G\tany\t[DISABLED]\n";
        assert_eq!(parse_networks(text), [("0".into(), "Casa".into()), ("1".into(), "Ufficio 5G".into())]);
    }

    #[test]
    fn join_times_out_and_removes_the_network() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = calls.clone();
        let wpa = Wpa {
            cli: Box::new(move |args| {
                log.lock().expect("lock").push(args.join(" "));
                Ok(match args[0] {
                    "add_network" => "3".into(),
                    "status" => "wpa_state=SCANNING".into(),
                    _ => "OK".into(),
                })
            }),
        };
        let started = Instant::now();
        let err = wpa.join("Casa", Some("segreta"), Duration::from_millis(50)).expect_err("never associates");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(calls.lock().expect("lock").iter().any(|c| c == "remove_network 3"));
    }
}
