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

/// Writes a hash-array property, one record given as `{ key = "value", … }` on stdin.
fn write_hash(prop: &str, fields: &[(&str, &str)]) -> io::Result<()> {
    run(Command::new("lipc-hash-prop").args([WIFID, prop]), Some(&hash_record(fields))).map(drop)
}

type Record = Vec<(String, String)>;

fn field<'a>(r: &'a Record, key: &str) -> Option<&'a str> {
    r.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// One record in the `{ key = "value", … }` form lipc-hash-prop takes on stdin.
fn hash_record(fields: &[(&str, &str)]) -> String {
    let quote = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{k} = \"{}\"", quote(v))).collect();
    format!("{{ {} }}\n", body.join(", "))
}

/// The records of lipc-hash-prop's output. Lenient by design (its exact layout is to be
/// checked on a device): each `{ … }` is a record of `key = value` or `"key": value` pairs
/// separated by commas or newlines, values quoted (with `\` escapes) or bare. Anything it
/// doesn't understand is skipped, never an error.
fn parse_hash_array(text: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut chars = text.chars().peekable();
    let mut current: Option<Record> = None;
    // A key read, waiting for its value.
    let mut key: Option<String> = None;
    let word = |chars: &mut std::iter::Peekable<std::str::Chars>| {
        if chars.peek() == Some(&'"') {
            chars.next();
            let mut s = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => s.extend(chars.next()),
                    '"' => break,
                    c => s.push(c),
                }
            }
            s
        } else {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || matches!(c, ',' | '{' | '}' | '=' | ':') {
                    break;
                }
                s.push(c);
                chars.next();
            }
            s
        }
    };
    while let Some(&c) = chars.peek() {
        match c {
            '{' => {
                chars.next();
                current = Some(Vec::new());
                key = None;
            }
            '}' => {
                chars.next();
                records.extend(current.take());
                key = None;
            }
            '=' | ':' | ',' => {
                chars.next();
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            _ => {
                let w = word(&mut chars);
                if w.is_empty() {
                    chars.next(); // a stray character: skip it
                    continue;
                }
                let Some(rec) = current.as_mut() else { continue };
                match key.take() {
                    None => key = Some(w),
                    Some(k) => rec.push((k, w)),
                }
            }
        }
    }
    records
}

impl Wifi for KindleWifi {
    fn status(&self) -> io::Result<WifiStatus> {
        let enabled = get("-i", CMD, "wirelessEnable")? == "1";
        let connected = if enabled && get("-s", WIFID, "cmState")? == "CONNECTED" {
            Some(get("-s", WIFID, "currentEssid")?).filter(|s| !s.is_empty())
        } else {
            None
        };
        Ok(WifiStatus { enabled, connected })
    }

    fn set_enabled(&self, on: bool) -> io::Result<()> {
        set("-i", CMD, "wirelessEnable", if on { "1" } else { "0" })
    }

    fn scan(&self) -> io::Result<Vec<Network>> {
        // Listen first, then ask: the event can't slip between the two.
        let mut wait = Command::new("lipc-wait-event")
            .args(["-s", &SCAN_TIMEOUT.to_string(), WIFID, "scanComplete,scanFailed"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
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
                let secured = field(r, "key_mgmt").is_some_and(|k| !k.eq_ignore_ascii_case("none"));
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
        if !self.saved()?.iter().any(|s| s == ssid) {
            let secured = if password.is_some() { "yes" } else { "no" };
            let mut fields = vec![("essid", ssid), ("secured", secured)];
            fields.extend(password.map(|p| ("psk", p)));
            // Keep the password on the device (not synced to Amazon's account).
            fields.push(("store_nw_user_pref", "0"));
            write_hash("createProfile", &fields)?;
        }
        set("-s", CMD, "ensureConnection", &format!("wifi:{ssid}"))?;
        let start = Instant::now();
        while start.elapsed() < CONNECT_TIMEOUT {
            std::thread::sleep(Duration::from_millis(500));
            if self.status()?.connected.as_deref() == Some(ssid) {
                return Ok(());
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, format!("{ssid}: not connected after {CONNECT_TIMEOUT:?}")))
    }

    fn forget(&self, ssid: &str) -> io::Result<()> {
        write_hash("deleteProfile", &[("essid", ssid)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hash_arrays_leniently() {
        let lua = r#"{ essid = "Casa \"5G\"", signal = 4, signal_max = 5, key_mgmt = "wpa2-psk", known = "yes" }
{ essid = "Biblioteca", signal = 1, signal_max = 5, key_mgmt = "none" }"#;
        let r = parse_hash_array(lua);
        assert_eq!(r.len(), 2);
        assert_eq!(field(&r[0], "essid"), Some("Casa \"5G\""));
        assert_eq!(field(&r[0], "signal"), Some("4"));
        assert_eq!(field(&r[1], "key_mgmt"), Some("none"));
        let json = "[ 0 ] = {\n  \"essid\": \"Ufficio\",\n  \"secured\": \"yes\"\n}\n";
        let r = parse_hash_array(json);
        assert_eq!(r, vec![vec![("essid".into(), "Ufficio".into()), ("secured".into(), "yes".into())]]);
        assert!(parse_hash_array("").is_empty());
        assert!(parse_hash_array("error: no such property").is_empty());
    }

    #[test]
    fn writes_records_with_escaped_values() {
        assert_eq!(hash_record(&[("essid", "a\"b"), ("psk", "c\\d")]), "{ essid = \"a\\\"b\", psk = \"c\\\\d\" }\n");
    }
}
