//! Wi-Fi through the firmware's own daemons, which keep running with the framework stopped:
//! `cmd` (`com.lab126.cmd`: the radio switch, "ensure a connection") and `wifid`
//! (`com.lab126.wifid`: scans, connection state, saved profiles), driven with the `lipc-*`
//! command-line tools. Going through wifid rather than `wpa_cli` keeps its profiles (shared
//! with the Amazon UI) the single source of truth, and wifid never fights us over
//! wpa_supplicant. Commands and what to check on a device:
//! `docs/superpowers/notes/kindle-wifi.md`.
// ponytail: CLI tools, a process per call (tens of ms); calls run off the UI thread anyway.
// Replace with liblipc FFI only if a device trace shows them in the way.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::wifi::{Network, Wifi, WifiStatus};

const CMD: &str = "com.lab126.cmd";
const WIFID: &str = "com.lab126.wifid";
/// How long a scan and a connection may take before we give up.
const SCAN_TIMEOUT: u32 = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct KindleWifi;

/// Runs `cmd`, feeding it `input`; its trimmed stdout, or an error with its stderr.
fn run(cmd: &mut Command, input: Option<&str>) -> io::Result<String> {
    let mut child = cmd
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::other(format!("{cmd:?}: {} {}", out.status, err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `lipc-get-prop -i|-s <publisher> <prop>`.
fn get(kind: &str, publisher: &str, prop: &str) -> io::Result<String> {
    run(Command::new("lipc-get-prop").args([kind, publisher, prop]), None)
}

/// `lipc-set-prop -i|-s <publisher> <prop> <value>`.
fn set(kind: &str, publisher: &str, prop: &str, value: &str) -> io::Result<()> {
    run(Command::new("lipc-set-prop").args([kind, publisher, prop, value]), None).map(drop)
}

/// Reads a hash-array property: `lipc-hash-prop -n <publisher> <prop>`.
fn read_hash(prop: &str) -> io::Result<Vec<Record>> {
    run(Command::new("lipc-hash-prop").args(["-n", WIFID, prop]), None).map(|s| parse_hash_array(&s))
}

/// Writes a hash-array property, one record on stdin. The PW5's lipc-hash-prop rejected
/// `{ key = "value" }` ("unexpected KEY, expecting STRING or INTEGER or HEX"); until the
/// accepted syntax is known, the likely ones are tried in turn and the first that parses wins.
// ponytail: up to four tries per call; keep only the one that works once the log says which
// (`wifi: createProfile syntax N`).
fn write_hash(prop: &str, fields: &[(&str, &str)]) -> io::Result<()> {
    let mut last = io::Error::other("no record");
    for (n, record) in hash_records(fields).iter().enumerate() {
        match run(Command::new("lipc-hash-prop").args([WIFID, prop]), Some(record)) {
            Ok(_) => {
                eprintln!("wifi: {prop} syntax {n}");
                return Ok(());
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

type Record = Vec<(String, String)>;

fn field<'a>(r: &'a Record, key: &str) -> Option<&'a str> {
    r.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Refuses what could break out of a quoted value: SSIDs come off the air. Whether
/// lipc-hash-prop honours `\` escapes is unverified, so quotes, backslashes, braces and
/// control characters are rejected rather than escaped.
fn plain(v: &str) -> io::Result<&str> {
    match v.chars().find(|c| matches!(c, '"' | '\\' | '{' | '}') || c.is_control()) {
        Some(c) => Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unsupported character {c:?}"))),
        None => Ok(v),
    }
}

/// One record in the `{ key = "value", … }` form lipc-hash-prop takes on stdin.
/// The candidate syntaxes for one record: bare keys, quoted keys, quoted keys with `:`, bare
/// keys without commas. Integer fields go bare, the rest quoted; values are [`plain`].
fn hash_records(fields: &[(&str, &str)]) -> [String; 4] {
    const INTS: [&str; 1] = ["store_nw_user_pref"];
    let record = |quote_keys: bool, eq: &str, sep: &str| {
        let body: Vec<String> = (fields.iter())
            .map(|(k, v)| {
                let key = if quote_keys { format!("\"{k}\"") } else { k.to_string() };
                let value = if INTS.contains(k) { v.to_string() } else { format!("\"{v}\"") };
                format!("{key} {eq} {value}")
            })
            .collect();
        format!("{{ {} }}\n", body.join(sep))
    };
    [record(false, "=", ", "), record(true, "=", ", "), record(true, ":", ", "), record(false, "=", " ")]
}

/// The records of `lipc-hash-prop -n` output, as a PW5 (5.16) prints them:
///
/// ```text
/// Hash Index: 0   (fields indented with a tab)
///     essid = "Casa"
///     signal = (5)
/// ```
///
/// Strings are quoted (and may run over several lines), integers in parentheses. Lines it
/// doesn't understand are skipped, never an error.
fn parse_hash_array(text: &str) -> Vec<Record> {
    let mut records: Vec<Record> = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.starts_with("Hash Index") {
            records.push(Vec::new());
            continue;
        }
        let (Some(rec), Some((key, value))) = (records.last_mut(), line.split_once(" = ")) else { continue };
        let value = if let Some(open) = value.strip_prefix('"') {
            // A quoted value ends at the line ending in a quote, maybe a later one.
            let mut v = open.to_string();
            while !v.ends_with('"') {
                let Some(more) = lines.next() else { break };
                v.push('\n');
                v.push_str(more);
            }
            v.strip_suffix('"').unwrap_or(&v).to_string()
        } else {
            value.trim_start_matches('(').trim_end_matches(')').to_string()
        };
        rec.push((key.trim().to_string(), value));
    }
    records
}

impl Wifi for KindleWifi {
    fn status(&self) -> io::Result<WifiStatus> {
        let enabled = get("-i", CMD, "wirelessEnable")? == "1";
        // currentEssid is a hash (essid, bssid, connected, signal…), not a string.
        let connected = if enabled && get("-s", WIFID, "cmState")? == "CONNECTED" {
            let current = read_hash("currentEssid")?;
            current.first().and_then(|r| field(r, "essid")).filter(|s| !s.is_empty()).map(str::to_string)
        } else {
            None
        };
        Ok(WifiStatus { enabled, connected })
    }

    fn set_enabled(&self, on: bool) -> io::Result<()> {
        set("-i", CMD, "wirelessEnable", if on { "1" } else { "0" })
    }

    fn scan(&self) -> io::Result<Vec<Network>> {
        // Listen first, then ask. spawn() returns at exec, before the listener has subscribed:
        // give it a moment, or a quick scanComplete slips and the scan waits its full timeout.
        let mut wait = Command::new("lipc-wait-event")
            .args(["-s", &SCAN_TIMEOUT.to_string(), WIFID, "scanComplete,scanFailed"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        std::thread::sleep(Duration::from_millis(150));
        if let Err(e) = set("-s", WIFID, "scan", "") {
            wait.kill().ok();
            wait.wait().ok();
            return Err(e);
        }
        // A timeout still leaves the last results in scanList: read them either way.
        wait.wait().ok();
        let mut nets: Vec<Network> = read_hash("scanList")?
            .iter()
            .filter_map(|r| {
                let ssid = field(r, "essid").filter(|s| !s.is_empty())?.to_string();
                let level: u32 = field(r, "signal").and_then(|s| s.parse().ok()).unwrap_or(0);
                let max: u32 = field(r, "signal_max").and_then(|s| s.parse().ok()).filter(|m| *m > 0).unwrap_or(5);
                let signal = (level.min(max) * 100 / max) as u8;
                let secured = field(r, "secured").is_none_or(|v| v == "yes");
                Some(Network { ssid, signal, secured })
            })
            .collect();
        nets.sort_by_key(|n| std::cmp::Reverse(n.signal));
        // One row per name: several access points of a network share the strongest.
        let mut seen = std::collections::HashSet::new();
        nets.retain(|n| seen.insert(n.ssid.clone()));
        Ok(nets)
    }

    fn saved(&self) -> io::Result<Vec<String>> {
        Ok(read_hash("profileData")?.iter().filter_map(|r| field(r, "essid").map(str::to_string)).collect())
    }

    fn connect(&self, ssid: &str, password: Option<&str>) -> io::Result<()> {
        plain(ssid)?;
        password.map(plain).transpose()?;
        let created = !self.saved()?.iter().any(|s| s == ssid);
        if created {
            let secured = if password.is_some() { "yes" } else { "no" };
            let mut fields = vec![("essid", ssid), ("secured", secured)];
            fields.extend(password.map(|p| ("psk", p)));
            // Keep the password on the device (not synced to Amazon's account).
            fields.push(("store_nw_user_pref", "0"));
            write_hash("createProfile", &fields)?;
        }
        let joined = || {
            set("-s", CMD, "ensureConnection", &format!("wifi:{ssid}"))?;
            let start = Instant::now();
            while start.elapsed() < CONNECT_TIMEOUT {
                std::thread::sleep(Duration::from_millis(500));
                // wifid can be busy while associating: a failed read is not a failed join.
                if self.status().is_ok_and(|s| s.connected.as_deref() == Some(ssid)) {
                    return Ok(());
                }
            }
            Err(io::Error::new(io::ErrorKind::TimedOut, format!("{ssid}: not connected after {CONNECT_TIMEOUT:?}")))
        };
        let result = joined();
        // A wrong password must not stay saved: the next tap would reuse it without asking.
        if result.is_err() && created {
            self.forget(ssid).ok();
        }
        result
    }

    /// deleteProfile takes the profile's netid (an integer), not its name.
    fn forget(&self, ssid: &str) -> io::Result<()> {
        let profiles = read_hash("profileData")?;
        let netid = profiles.iter().find(|r| field(r, "essid") == Some(ssid)).and_then(|r| field(r, "netid"));
        let Some(netid) = netid else { return Ok(()) };
        set("-i", WIFID, "deleteProfile", netid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_that_could_escape_a_record_are_refused() {
        assert!(plain("Casa Rossi 5G").is_ok());
        assert!(plain("caffè·wifi").is_ok());
        for bad in ["x\", store_nw_user_pref = \"1", "a\\b", "{x}", "a\nb"] {
            assert!(plain(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_hash_arrays_as_the_pw5_prints_them() {
        let out = "Hash Index: 0\n\tnetid = (1)\n\tessid = \"Casa\"\n\tsignal = (5)\n\tsecured = \"yes\"\n\n\
                   Hash Index: 1\n\tessid = \"\"\n\tcmResolvConf = \"nameserver 192.168.1.1\n\"\n\tkey_mgmt = \"SAE\"\n";
        let r = parse_hash_array(out);
        assert_eq!(r.len(), 2);
        assert_eq!(field(&r[0], "essid"), Some("Casa"));
        assert_eq!(field(&r[0], "netid"), Some("1"));
        assert_eq!(field(&r[0], "signal"), Some("5"));
        assert_eq!(field(&r[1], "essid"), Some(""));
        assert_eq!(field(&r[1], "cmResolvConf"), Some("nameserver 192.168.1.1\n"));
        assert_eq!(field(&r[1], "key_mgmt"), Some("SAE"), "a value after a multi-line one");
        assert!(parse_hash_array("").is_empty());
        assert!(parse_hash_array("com.lab126.wifid failed to access property").is_empty());
    }

    #[test]
    fn writes_ints_bare_and_strings_quoted() {
        let recs = hash_records(&[("essid", "Casa"), ("store_nw_user_pref", "0")]);
        assert_eq!(recs[0], "{ essid = \"Casa\", store_nw_user_pref = 0 }\n");
        assert_eq!(recs[1], "{ \"essid\" = \"Casa\", \"store_nw_user_pref\" = 0 }\n");
    }
}
