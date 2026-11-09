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

/// What a Wi-Fi driver needs to come up.
struct Chip {
    /// Loaded first.
    dep: &'static str,
    params: &'static [&'static str],
    /// wpa_supplicant's `-D`.
    driver: &'static str,
    /// From the interface appearing to it taking `up`.
    settle: Duration,
}

/// The chips we know, by `WIFI_MODULE`.
// shortcut: only NXP's `moal` (Clara 2E, Libra 2…); Realtek (`8821cs`) or MediaTek boards
// power up through `sdio_wifi_pwr`/`wmt` instead: add them here when such a Kobo is ported.
fn chip(module: &str) -> Option<Chip> {
    (module == "moal").then_some(Chip {
        dep: "mlan",
        params: &["mod_para=nxp/wifi_mod_para_sd8987.conf"],
        driver: "nl80211",
        settle: Duration::from_secs(2),
    })
}

pub struct KoboWifi {
    /// `WIFI_MODULE` from rcS (`moal` on the Clara 2E).
    module: String,
    chip: Chip,
    /// `INTERFACE` (`mlan0`).
    iface: String,
    /// `/drivers/$PLATFORM/wifi`.
    modules: PathBuf,
    wpa: Wpa,
}

impl KoboWifi {
    /// From the environment the launch script copies from Nickel; `None` without it or for
    /// a chip we don't know.
    pub fn from_env() -> Option<KoboWifi> {
        let var = |k| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (module, iface, platform) = (var("WIFI_MODULE")?, var("INTERFACE")?, var("PLATFORM")?);
        Some(KoboWifi {
            chip: chip(&module)?,
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
        let f = File::options().read(true).custom_flags(libc::O_NONBLOCK).open("/dev/ntx_io")?;
        // SAFETY: this ntx_io request takes its argument by value, not a pointer.
        if unsafe { libc::ioctl(f.as_raw_fd(), NTX_WIFI_POWER as _, on as libc::c_int) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// wpa_supplicant runs for the interface (its control socket is there).
    fn supplicant(&self) -> bool {
        Path::new("/var/run/wpa_supplicant").join(&self.iface).exists()
    }

    fn up(&self) -> io::Result<()> {
        self.power(true)?;
        std::thread::sleep(Duration::from_millis(250));
        self.insmod(self.chip.dep, &[])?;
        std::thread::sleep(Duration::from_millis(250));
        let region = fs::read_to_string("/mnt/onboard/.kobo/Kobo/Kobo eReader.conf")
            .ok()
            .and_then(|c| c.lines().find_map(|l| l.strip_prefix("WifiRegulatoryDomain=")).map(str::to_string));
        let mut params: Vec<String> = region.map(|r| format!("reg_alpha2={r}")).into_iter().collect();
        params.extend(self.chip.params.iter().map(|p| p.to_string()));
        self.insmod(&self.module, &params)?;
        let net = Path::new("/sys/class/net").join(&self.iface);
        let start = Instant::now();
        while !net.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(200));
        }
        std::thread::sleep(self.chip.settle);
        run(Command::new("ifconfig").args([&self.iface, "up"]), None)?;
        if !self.supplicant() {
            let args =
                ["-D", self.chip.driver, "-s", "-i", &self.iface, "-c", CONFIG, "-C", "/var/run/wpa_supplicant", "-B"];
            run(Command::new("wpa_supplicant").args(args), None)?;
        }
        // A daemon that follows the link: an address whenever a network is joined. One may
        // run already (a second start fails): that's fine, it's the radio that matters.
        if let Err(e) = run(Command::new("dhcpcd").args(["-b", &self.iface]), None) {
            eprintln!("wifi: {e}");
        }
        Ok(())
    }

    /// Takes everything down as far as it goes, and always powers the chip off.
    fn down(&self) -> io::Result<()> {
        run(Command::new("dhcpcd").args(["-k", &self.iface]), None).ok();
        run(Command::new("wpa_cli").args(["-i", &self.iface, "terminate"]), None).ok();
        run(Command::new("ifconfig").args([&self.iface, "down"]), None).ok();
        for m in [self.module.as_str(), self.chip.dep] {
            if self.loaded(m) {
                std::thread::sleep(Duration::from_millis(250));
                run(Command::new("rmmod").arg(m), None).map_err(|e| eprintln!("wifi: {e}")).ok();
            }
        }
        self.power(false)
    }

    /// Off, if it was on (before a suspend, and before the vendor UI comes back).
    pub fn off(&self) -> io::Result<()> {
        if self.loaded(&self.module) {
            self.down()?;
        }
        Ok(())
    }

    /// An IPv4 address on the interface: joined and usable.
    fn has_address(&self) -> bool {
        run(Command::new("ip").args(["-4", "-o", "addr", "show", "dev", &self.iface]), None)
            .is_ok_and(|o| o.contains(" inet "))
    }
}

impl Wifi for KoboWifi {
    fn status(&self) -> io::Result<WifiStatus> {
        // Polled every second while a panel waits for a network: no process unless it runs.
        let enabled = self.loaded(&self.module) && Path::new("/sys/class/net").join(&self.iface).exists();
        let connected = if enabled && self.supplicant() { self.wpa.connected().ok().flatten() } else { None };
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
