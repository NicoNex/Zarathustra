//! Wi-Fi on a Kobo with Nickel stopped: the radio is brought up and down by hand (power,
//! driver modules, wpa_supplicant, dhcpcd), the way the firmware does it, and driven through
//! `wpa_cli`. The networks live in Nickel's own wpa_supplicant config, so both sides share
//! them. Facts per Wi-Fi module from KOReader's and Plato's Kobo scripts.

use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::sys::run;
use crate::wifi::{Network, Wifi, WifiStatus};
use crate::wpa::Wpa;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CONFIG: &str = "/etc/wpa_supplicant/wpa_supplicant.conf";
/// `/dev/ntx_io` request that powers the Wi-Fi chip on NXP boards with the `moal` driver.
const NTX_WIFI_POWER: libc::c_ulong = 208;

pub struct KoboWifi {
    /// `WIFI_MODULE` from rcS (`moal` on the Clara 2E).
    module: String,
    /// `INTERFACE` (`mlan0`).
    iface: String,
    /// `/drivers/$PLATFORM/wifi`.
    modules: PathBuf,
    wpa: Wpa,
}

impl KoboWifi {
    /// From the environment the launch script copies from Nickel; `None` without it.
    pub fn from_env() -> Option<KoboWifi> {
        let var = |k| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (module, iface, platform) = (var("WIFI_MODULE")?, var("INTERFACE")?, var("PLATFORM")?);
        Some(KoboWifi {
            wpa: Wpa::new(&iface),
            modules: Path::new("/drivers").join(platform).join("wifi"),
            module,
            iface,
        })
    }

    fn loaded(&self, module: &str) -> bool {
        fs::read_to_string("/proc/modules").is_ok_and(|m| m.lines().any(|l| l.split(' ').next() == Some(module)))
    }

    fn insmod(&self, module: &str, params: &[String]) -> io::Result<()> {
        if self.loaded(module) {
            return Ok(());
        }
        run(Command::new("insmod").arg(self.modules.join(format!("{module}.ko"))).args(params), None).map(drop)
    }

    fn power(&self, on: bool) -> io::Result<()> {
        if self.module == "moal" {
            let f = File::options().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/ntx_io")?;
            // SAFETY: this ntx_io request takes its argument by value, not a pointer.
            if unsafe { libc::ioctl(f.as_raw_fd(), NTX_WIFI_POWER as _, on as libc::c_int) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        } else if on {
            self.insmod("sdio_wifi_pwr", &[])
        } else {
            run(Command::new("rmmod").arg("sdio_wifi_pwr"), None).map(drop)
        }
    }

    fn up(&self) -> io::Result<()> {
        self.power(true)?;
        std::thread::sleep(Duration::from_millis(250));
        let mut params = Vec::new();
        let region = fs::read_to_string("/mnt/onboard/.kobo/Kobo/Kobo eReader.conf")
            .ok()
            .and_then(|c| c.lines().find_map(|l| l.strip_prefix("WifiRegulatoryDomain=")).map(str::to_string));
        if self.module == "moal" {
            self.insmod("mlan", &[])?;
            std::thread::sleep(Duration::from_millis(250));
            params.extend(region.map(|r| format!("reg_alpha2={r}")));
            params.push("mod_para=nxp/wifi_mod_para_sd8987.conf".into());
        }
        self.insmod(&self.module, &params)?;
        let net = Path::new("/sys/class/net").join(&self.iface);
        let start = Instant::now();
        while !net.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(Duration::from_secs(if self.module == "moal" { 2 } else { 1 }));
        run(Command::new("ifconfig").args([&self.iface, "up"]), None)?;
        let driver = if self.module == "moal" { "nl80211" } else { "wext" };
        if self.wpa.connected().is_err() {
            run(
                Command::new("wpa_supplicant").args(["-D", driver, "-s", "-i", &self.iface, "-c", CONFIG]).args([
                    "-C",
                    "/var/run/wpa_supplicant",
                    "-B",
                ]),
                None,
            )?;
        }
        // A daemon that follows the link: an address whenever a network is joined.
        run(Command::new("dhcpcd").args(["-b", &self.iface]), None).map(drop)
    }

    fn down(&self) -> io::Result<()> {
        run(Command::new("dhcpcd").args(["-k", &self.iface]), None).ok();
        run(Command::new("wpa_cli").args(["-i", &self.iface, "terminate"]), None).ok();
        run(Command::new("ifconfig").args([&self.iface, "down"]), None).ok();
        for m in [self.module.as_str(), "mlan"] {
            if self.loaded(m) {
                std::thread::sleep(Duration::from_millis(250));
                run(Command::new("rmmod").arg(m), None)?;
            }
        }
        self.power(false)
    }

    /// An IPv4 address on the interface: joined and usable.
    fn has_address(&self) -> bool {
        run(Command::new("ip").args(["-4", "-o", "addr", "show", "dev", &self.iface]), None)
            .is_ok_and(|o| o.contains(" inet "))
    }
}

impl Wifi for KoboWifi {
    fn status(&self) -> io::Result<WifiStatus> {
        let enabled = self.loaded(&self.module) && Path::new("/sys/class/net").join(&self.iface).exists();
        let connected = if enabled { self.wpa.connected().ok().flatten() } else { None };
        Ok(WifiStatus { enabled, connected })
    }

    fn set_enabled(&self, on: bool) -> io::Result<()> {
        if on {
            let up = self.up();
            if up.is_err() {
                // Never leave half a radio behind: the next try, and Nickel, start clean.
                self.down().ok();
            }
            up
        } else {
            self.down()
        }
    }

    fn scan(&self) -> io::Result<Vec<Network>> {
        self.wpa.scan()
    }

    fn saved(&self) -> io::Result<Vec<String>> {
        self.wpa.saved()
    }

    fn connect(&self, ssid: &str, password: Option<&str>) -> io::Result<()> {
        let start = Instant::now();
        self.wpa.connect(ssid, password, CONNECT_TIMEOUT)?;
        while !self.has_address() {
            if start.elapsed() >= CONNECT_TIMEOUT {
                return Err(io::Error::new(io::ErrorKind::TimedOut, format!("{ssid}: no address from the network")));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok(())
    }

    fn forget(&self, ssid: &str) -> io::Result<()> {
        self.wpa.forget(ssid)
    }
}
