//! Where things live on this system: the books, Zarathustra's own files, fonts, the system
//! version. Detected once from the filesystem, so the shell never tests paths itself.
//! Porting to a new system = a new [`Kind`] and its branch in [`detect`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Kindle,
    Kobo,
    /// A development machine: the repo's dev assets stand in for the device's.
    Desktop,
}

#[derive(Debug, Clone)]
pub struct Platform {
    pub kind: Kind,
    /// The device's name in user-facing text ("Collega il Kobo…").
    pub name: &'static str,
    /// The books the home shows.
    pub library: PathBuf,
    /// The app binary the launch script copies to /tmp and runs; `None` on a desktop.
    pub binary: Option<PathBuf>,
    /// Positions, preferences, the database.
    pub state: PathBuf,
    pub hyphenation: PathBuf,
    pub dictionaries: PathBuf,
    /// Font folders, searched in order.
    pub fonts: Vec<PathBuf>,
    /// Installed plugins.
    pub plugins: PathBuf,
    /// Where plugins are copied to run (executable, owned by root); `None` runs them in place.
    pub plugin_runtime: Option<PathBuf>,
    /// The plugins' own data (keys, settings), kept across restarts.
    pub plugin_data: PathBuf,
    /// A file whose first line names the system version.
    version_file: Option<PathBuf>,
}

impl Platform {
    /// The system version for the device information, never a serial number: Kindle's
    /// `/etc/prettyversion.txt` line; on a Kobo the firmware field of `.kobo/version`
    /// (`serial,kernel,firmware,…`).
    pub fn system_version(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.version_file.as_ref()?).ok()?;
        let line = text.lines().next()?.trim();
        match self.kind {
            Kind::Kobo => line.split(',').nth(2).map(|v| format!("Kobo {v}")),
            _ => Some(line.to_string()).filter(|l| !l.is_empty()),
        }
    }
}

/// This system's platform, detected on first use.
pub fn platform() -> &'static Platform {
    static PLATFORM: OnceLock<Platform> = OnceLock::new();
    PLATFORM.get_or_init(|| detect(Path::new("/")))
}

/// The platform of the system mounted at `root` (`/`, or a test folder).
fn detect(root: &Path) -> Platform {
    if root.join("bin/kobo_config.sh").exists() && root.join("mnt/onboard").exists() {
        let onboard = root.join("mnt/onboard");
        let app = onboard.join(".adds/zarathustra");
        Platform {
            kind: Kind::Kobo,
            name: "Kobo",
            library: onboard.join("library"),
            state: app.join("state"),
            hyphenation: app.join("hyph"),
            dictionaries: app.join("dictionaries"),
            fonts: vec![onboard.join("fonts"), app.join("fonts")],
            plugins: app.join("plugins"),
            // /var is a few small tmpfs on the Kobo: run plugins from /tmp, keep data on onboard.
            plugin_runtime: Some(root.join("tmp/zarathustra/plugins")),
            plugin_data: app.join("data"),
            version_file: Some(onboard.join(".kobo/version")),
            binary: Some(app.join("bin/zarathustra")),
        }
    } else if root.join("mnt/us").exists() {
        let us = root.join("mnt/us");
        let app = us.join("zarathustra");
        Platform {
            kind: Kind::Kindle,
            name: "Kindle",
            library: us.join("library"),
            state: app.join("state"),
            hyphenation: app.join("hyph"),
            dictionaries: us.join("dictionaries"),
            fonts: vec![us.join("fonts"), root.join("usr/java/lib/fonts")],
            plugins: app.join("plugins"),
            plugin_runtime: Some(root.join("var/local/zarathustra/plugins")),
            plugin_data: root.join("var/local/zarathustra/data"),
            version_file: Some(root.join("etc/prettyversion.txt")),
            binary: Some(app.join("bin/zarathustra")),
        }
    } else {
        let repo = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        let state = repo.join("target/zarathustra-state");
        Platform {
            kind: Kind::Desktop,
            // Desktop renders stand in for the Kindle the layouts were first made for.
            name: "Kindle",
            library: repo.join("dev-books"),
            hyphenation: repo.join("assets/hyphenation"),
            dictionaries: repo.join("dev-dicts"),
            fonts: vec![repo.join("dev-fonts")],
            plugins: repo.join("dist/plugins"),
            plugin_runtime: None,
            plugin_data: state.join("plugins"),
            version_file: None,
            binary: None,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(dirs: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zarathustra-platform-{}-{}", std::process::id(), dirs.join("+")));
        std::fs::remove_dir_all(&dir).ok();
        for d in dirs {
            let p = dir.join(d);
            if d.ends_with(".sh") {
                std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
                std::fs::write(p, "").expect("write");
            } else {
                std::fs::create_dir_all(p).expect("mkdir");
            }
        }
        dir
    }

    #[test]
    fn detects_kindle() {
        let r = root(&["mnt/us"]);
        let p = detect(&r);
        assert_eq!(p.kind, Kind::Kindle);
        assert!(p.library.ends_with("mnt/us/library"));
        assert!(p.state.ends_with("mnt/us/zarathustra/state"));
    }

    #[test]
    fn detects_kobo() {
        let r = root(&["bin/kobo_config.sh", "mnt/onboard"]);
        let p = detect(&r);
        assert_eq!(p.kind, Kind::Kobo);
        assert!(p.state.ends_with("mnt/onboard/.adds/zarathustra/state"));
        assert!(p.library.ends_with("mnt/onboard/library"));
        assert!(p.binary.as_ref().is_some_and(|b| b.ends_with(".adds/zarathustra/bin/zarathustra")));
        std::fs::create_dir_all(r.join("mnt/onboard/.kobo")).expect("mkdir");
        std::fs::write(r.join("mnt/onboard/.kobo/version"), "N123456789012,4.1.15,4.38.23852,4.1.15,4.1.15,0-386\n")
            .expect("write");
        assert_eq!(p.system_version().as_deref(), Some("Kobo 4.38.23852"), "no serial number");
    }

    #[test]
    fn detects_desktop() {
        let p = detect(&root(&["home"]));
        assert_eq!(p.kind, Kind::Desktop);
        assert!(p.library.ends_with("dev-books"));
        assert_eq!(p.binary, None);
    }
}
