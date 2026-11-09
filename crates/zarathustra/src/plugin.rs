//! Plugins: folders of executables described by `plugin.toml`, run out of process
//! (docs/superpowers/specs/2026-11-08-plugins-design.md).
//!
//! A plugin's files are copied to a runtime folder on the rootfs before anything of it runs
//! (the userstore is vfat: no exec bits, and a running service must not keep it busy over USB).
//! Services are started in their own process group, so they outlive the app; their pid file
//! says whether they run.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde::Deserialize;

mod action;
pub use action::{LastRun, Outcome};

/// Why a plugin, or one of its runs, failed: shown on its page.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("plugin.toml: {0}")]
    Manifest(#[from] toml::de::Error),
    #[error("needs a newer Zarathustra (format {0})")]
    NewerFormat(u32),
    #[error("{executable}: {source}")]
    Spawn { executable: String, source: std::io::Error },
    #[error("{executable}: no answer in {} s", RUN_TIMEOUT.as_secs())]
    Timeout { executable: String },
    #[error("{executable}: {status}")]
    Failed { executable: String, status: std::process::ExitStatus },
    #[error("an action of this plugin is running")]
    Busy,
    #[error("{0} is not a list setting")]
    NotAList(String),
}

pub type Result<T, E = PluginError> = std::result::Result<T, E>;

/// The newest `format_version` this build understands.
pub const FORMAT_VERSION: u32 = 1;
/// How long a status, list or action run may take.
const RUN_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a service has to exit after SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(3);
/// The built-in service rows' setting ids.
pub const ACTIVE: &str = "service.active";
pub const AT_BOOT: &str = "service.at_boot";

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub display_name: String,
    pub description: String,
    pub plugin_version: String,
    pub format_version: u32,
    pub service: Option<Service>,
    pub status: Option<Run>,
    #[serde(default)]
    pub action: Vec<Action>,
    #[serde(default)]
    pub setting: Vec<Setting>,
}

/// An executable of the plugin and its arguments.
#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Service {
    #[serde(flatten)]
    pub run: Run,
    #[serde(default)]
    pub start_at_boot: bool,
    #[serde(default)]
    pub start_in_recovery: bool,
}

/// A button on the plugin's page.
// ponytail: only on the plugin's page; `shown_in` (home, book menu, selection) and the book
// and selection placeholders come with those menus.
#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    /// Names the action in translations (`action-<key>`); see [`Action::key`].
    pub id: Option<String>,
    pub menu_label: String,
    #[serde(flatten)]
    pub run: Run,
    /// What a successful run shows: just "done", or the first line it printed.
    #[serde(default)]
    pub output: Output,
    /// Where the action is offered besides the plugin's own page.
    #[serde(default = "extensions_only")]
    pub shown_in: Vec<Place>,
    /// Stopped like "Stop" when it runs longer; no limit when absent.
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    #[default]
    Quiet,
    Show,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Extensions,
    Book,
}

fn extensions_only() -> Vec<Place> {
    vec![Place::Extensions]
}

impl Action {
    /// Its `id`, else its executable's file name.
    pub fn key(&self) -> String {
        self.id.clone().unwrap_or_else(|| {
            let exe = Path::new(&self.run.executable);
            exe.file_name().map_or_else(|| self.run.executable.clone(), |n| n.to_string_lossy().into_owned())
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Setting {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    Toggle {
        #[serde(default)]
        default_value: bool,
    },
    Choice {
        options: Vec<String>,
        default_value: Option<String>,
    },
    Number {
        minimum: i64,
        maximum: i64,
        #[serde(default = "one")]
        step: i64,
        default_value: i64,
    },
    Text {
        #[serde(default)]
        default_value: String,
    },
    List {
        list_executable: String,
        /// Shown when the list is empty: how to add an item.
        #[serde(default)]
        empty_label: Option<String>,
    },
}

fn one() -> i64 {
    1
}

impl Kind {
    /// The value a setting has until the user changes it (`None` for lists).
    fn default_value(&self) -> Option<String> {
        match self {
            Kind::Toggle { default_value } => Some(default_value.to_string()),
            Kind::Choice { options, default_value } => default_value.clone().or_else(|| options.first().cloned()),
            Kind::Number { default_value, .. } => Some(default_value.to_string()),
            Kind::Text { default_value } => Some(default_value.clone()),
            Kind::List { .. } => None,
        }
    }
}

/// Where plugins are installed, run from and keep their data.
#[derive(Debug, Clone)]
pub struct Dirs {
    /// What the user installs: one folder per plugin.
    pub installed: PathBuf,
    /// Runtime copies; `None` runs plugins in place (desktop).
    pub runtime: Option<PathBuf>,
    pub data: PathBuf,
    pub database: PathBuf,
}

impl Dirs {
    pub fn new(state_dir: &Path) -> Dirs {
        let database = crate::db_path(state_dir);
        if crate::on_device() {
            Dirs {
                installed: PathBuf::from("/mnt/us/zarathustra/plugins"),
                runtime: Some(PathBuf::from("/var/local/zarathustra/plugins")),
                data: PathBuf::from("/var/local/zarathustra/data"),
                database,
            }
        } else {
            Dirs {
                installed: crate::first_existing(&["dist/plugins"]),
                runtime: None,
                data: state_dir.join("plugins"),
                database,
            }
        }
    }
}

/// An installed plugin: its manifest, or why it can't be used.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: String,
    /// The manifest, or why it can't be used (shown in its place).
    pub manifest: std::result::Result<Manifest, String>,
    dirs: Dirs,
    pub(crate) locales: crate::plugin_i18n::Locales,
}

/// The plugins installed in `dirs`, by name; folders with an invalid id are skipped.
pub fn discover(dirs: &Dirs) -> Vec<Plugin> {
    let Ok(entries) = std::fs::read_dir(&dirs.installed) else { return Vec::new() };
    let mut plugins: Vec<Plugin> = (entries.flatten())
        .filter(|e| e.path().join("plugin.toml").is_file())
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            let valid = !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !valid {
                eprintln!("plugin {id:?}: the folder name must be lowercase letters, digits and '-'");
                return None;
            }
            let manifest = load_manifest(&e.path().join("plugin.toml")).map_err(|e| e.to_string());
            if let Err(e) = &manifest {
                eprintln!("plugin {id}: {e}");
            }
            Some(Plugin { id, manifest, dirs: dirs.clone(), locales: Default::default() })
        })
        .collect();
    plugins.sort_by_key(|p| p.name().to_lowercase());
    plugins
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    let manifest: Manifest = toml::from_str(&std::fs::read_to_string(path)?)?;
    if manifest.format_version > FORMAT_VERSION {
        return Err(PluginError::NewerFormat(manifest.format_version));
    }
    Ok(manifest)
}

/// Replaces `{name}` placeholders found in `values`; unknown ones stay as written.
fn expand(arg: &str, values: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        match tail.find('}').and_then(|close| Some((values.get(&tail[1..close])?, close))) {
            Some((value, close)) => {
                out.push_str(value);
                rest = &tail[close + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `ZARATHUSTRA_SETTING_<ID>`: upper case, `-` and `.` as `_`.
fn setting_var(id: &str) -> String {
    format!("ZARATHUSTRA_SETTING_{}", id.to_ascii_uppercase().replace(['-', '.'], "_"))
}

impl Plugin {
    pub fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.display_name)
    }

    /// The folder the user installed.
    pub fn user_dir(&self) -> PathBuf {
        self.dirs.installed.join(&self.id)
    }

    /// The folder the plugin runs from.
    pub fn plugin_dir(&self) -> PathBuf {
        self.dirs.runtime.as_ref().map_or_else(|| self.user_dir(), |r| r.join(&self.id))
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dirs.data.join(&self.id)
    }

    fn pref_group(&self) -> String {
        format!("plugin.{}", self.id)
    }

    /// The user's values: every declared setting (its default until changed) and the service
    /// rows.
    pub fn values(&self, state_dir: &Path) -> HashMap<String, String> {
        let mut values: HashMap<String, String> = HashMap::new();
        if let Ok(m) = &self.manifest {
            for s in &m.setting {
                if let Some(v) = s.kind.default_value() {
                    values.insert(s.id.clone(), v);
                }
            }
            if m.service.is_some() {
                values.insert(ACTIVE.into(), "false".into());
                values.insert(AT_BOOT.into(), "false".into());
            }
        }
        let saved = crate::load_pref(state_dir, &self.pref_group()).unwrap_or_default();
        for (k, v) in saved.lines().filter_map(|l| l.split_once('=')) {
            if values.contains_key(k) {
                values.insert(k.to_string(), v.to_string());
            }
        }
        values
    }

    /// Saves `key = value` (newlines dropped: one value per line).
    pub fn set_value(&self, state_dir: &Path, key: &str, value: &str) {
        let mut values = self.values(state_dir);
        values.insert(key.to_string(), value.replace(['\n', '\r'], " "));
        let mut lines: Vec<String> = values.iter().map(|(k, v)| format!("{k}={v}")).collect();
        lines.sort();
        crate::save_pref(state_dir, &self.pref_group(), &(lines.join("\n") + "\n"));
    }

    pub fn flag(&self, state_dir: &Path, key: &str) -> bool {
        self.values(state_dir).get(key).is_some_and(|v| v == "true")
    }

    /// Refreshes the runtime copy (files whose size or time changed) and makes its files
    /// executable. A no-op when plugins run in place.
    pub fn prepare(&self) -> std::io::Result<()> {
        make_dirs(&self.data_dir())?;
        if self.dirs.runtime.is_none() {
            return Ok(());
        }
        copy_tree(&self.user_dir(), &self.plugin_dir())
    }

    /// The command for `run`, its arguments expanded and the environment set. `extra` are more
    /// placeholders, also exported as `ZARATHUSTRA_<NAME>` (an action's book).
    fn command(&self, run: &Run, values: &HashMap<String, String>, extra: &[(&str, &str)]) -> Command {
        let mut vars: HashMap<String, String> = HashMap::new();
        for (k, v) in values {
            vars.insert(format!("setting.{k}"), v.clone());
        }
        let dirs = [
            ("plugin_dir", self.plugin_dir()),
            ("data_dir", self.data_dir()),
            ("user_dir", self.user_dir()),
            ("database", self.dirs.database.clone()),
        ];
        for (k, p) in &dirs {
            vars.insert((*k).into(), p.to_string_lossy().into_owned());
        }
        for (k, v) in extra {
            vars.insert((*k).into(), (*v).into());
        }
        let mut cmd = Command::new(self.plugin_dir().join(&run.executable));
        cmd.args(run.arguments.iter().map(|a| expand(a, &vars)));
        cmd.current_dir(self.plugin_dir());
        cmd.env("ZARATHUSTRA_LANGUAGE", crate::i18n::lang().code());
        for (k, p) in &dirs {
            cmd.env(format!("ZARATHUSTRA_{}", k.to_ascii_uppercase()), p);
        }
        for (k, v) in values {
            cmd.env(setting_var(k), v);
        }
        for (k, v) in extra {
            cmd.env(format!("ZARATHUSTRA_{}", k.to_ascii_uppercase()), v);
        }
        cmd
    }

    /// Runs `run` to completion within [`RUN_TIMEOUT`]; its stdout, or why it failed. The
    /// runtime copy must be [prepared](Self::prepare).
    fn capture(&self, state_dir: &Path, run: &Run, args: &[&str]) -> Result<String> {
        let mut cmd = self.command(run, &self.values(state_dir), &[]);
        cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        let executable = || run.executable.clone();
        let mut child = cmd.spawn().map_err(|source| PluginError::Spawn { executable: executable(), source })?;
        let deadline = Instant::now() + RUN_TIMEOUT;
        let status = loop {
            match child.try_wait()? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PluginError::Timeout { executable: executable() });
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        // ponytail: stdout is read after the exit, so an output over the pipe buffer (64 KB)
        // stalls into the timeout; read it on a thread if a plugin ever prints that much.
        let mut out = String::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_string(&mut out)?;
        }
        if !status.success() {
            return Err(PluginError::Failed { executable: executable(), status });
        }
        Ok(out)
    }

    /// The status lines, `Label=value`.
    pub fn status(&self, state_dir: &Path) -> Result<Vec<(String, String)>> {
        let Some(run) = self.manifest.as_ref().ok().and_then(|m| m.status.clone()) else { return Ok(Vec::new()) };
        let out = self.capture(state_dir, &run, &[])?;
        Ok(out.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().into(), v.trim().into())).collect())
    }

    fn list_run(&self, setting: &str) -> Option<Run> {
        let m = self.manifest.as_ref().ok()?;
        match &m.setting.iter().find(|s| s.id == setting)?.kind {
            Kind::List { list_executable, .. } => {
                Some(Run { executable: list_executable.clone(), arguments: Vec::new() })
            }
            _ => None,
        }
    }

    /// A list setting's items: id, text.
    pub fn list(&self, state_dir: &Path, setting: &str) -> Result<Vec<(String, String)>> {
        let run = self.list_run(setting).ok_or_else(|| PluginError::NotAList(setting.into()))?;
        let out = self.capture(state_dir, &run, &["list"])?;
        Ok(out.lines().filter_map(|l| l.split_once('\t')).map(|(id, text)| (id.into(), text.into())).collect())
    }

    pub fn remove(&self, state_dir: &Path, setting: &str, id: &str) -> Result<()> {
        let run = self.list_run(setting).ok_or_else(|| PluginError::NotAList(setting.into()))?;
        self.capture(state_dir, &run, &["remove", id]).map(drop)
    }

    fn service(&self) -> Option<&Service> {
        self.manifest.as_ref().ok()?.service.as_ref()
    }

    fn pid_file(&self) -> PathBuf {
        self.data_dir().join("service.pid")
    }

    /// The service's pid, when it runs: the pid file names a live process running the
    /// service's executable.
    fn running_pid(&self) -> Option<i32> {
        live_pid(&self.pid_file(), &self.plugin_dir().join(&self.service()?.run.executable))
    }

    pub fn is_running(&self) -> bool {
        self.running_pid().is_some()
    }

    /// Starts the service (if not running) in its own process group, its output in
    /// `service.log`.
    pub fn start(&self, state_dir: &Path) -> Result<()> {
        use std::os::unix::process::CommandExt;
        let Some(service) = self.service() else { return Ok(()) };
        if self.is_running() {
            return Ok(());
        }
        self.prepare()?;
        let log = std::fs::File::create(self.data_dir().join("service.log"))?;
        let err = log.try_clone()?;
        let mut cmd = self.command(&service.run, &self.values(state_dir), &[]);
        cmd.stdin(Stdio::null()).stdout(log).stderr(err).process_group(0);
        let executable = service.run.executable.clone();
        let child = cmd.spawn().map_err(|source| PluginError::Spawn { executable, source })?;
        std::fs::write(self.pid_file(), format!("{}\n", child.id()))?;
        // Not waited on: the service outlives the app (its zombie, if it exits first, is
        // reaped when the app does).
        drop(child);
        Ok(())
    }

    /// Stops the service: SIGTERM to its process group, SIGKILL after [`STOP_GRACE`].
    pub fn stop(&self) {
        let Some(pid) = self.running_pid() else { return };
        terminate(pid, || self.is_running());
        let _ = std::fs::remove_file(self.pid_file());
    }

    /// Restarts a running service (its settings changed).
    pub fn restart(&self, state_dir: &Path) -> Result<()> {
        if !self.is_running() {
            return Ok(());
        }
        self.stop();
        self.start(state_dir)
    }
}

/// Ends process group `pid`: SIGTERM, up to [`STOP_GRACE`] for its leader to exit (`alive`),
/// then SIGKILL to whatever of the group is left (a child that ignored SIGTERM). Blocks for
/// up to the grace time.
fn terminate(pid: i32, alive: impl Fn() -> bool) {
    let group = Pid::from_raw(pid);
    let _ = killpg(group, Signal::SIGTERM);
    let deadline = Instant::now() + STOP_GRACE;
    while alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = killpg(group, Signal::SIGKILL);
}

/// The pid in `pid_file`, when it names a live process running `exe` (a reused pid runs
/// something else; a zombie has an empty command line).
fn live_pid(pid_file: &Path, exe: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(pid_file).ok()?.trim().parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let exe = exe.as_os_str().as_encoded_bytes();
    cmdline.windows(exe.len()).any(|w| w == exe).then_some(pid)
}

/// Starts services: the ones the user made active (`boot` false: the app opening), or the
/// ones to start at power-on (`boot`), plus every recovery service when `recovery`.
pub fn start_services(dirs: &Dirs, state_dir: &Path, boot: bool, recovery: bool) {
    for p in discover(dirs) {
        let Some(service) = p.service() else { continue };
        let wanted = if boot { service.start_at_boot && p.flag(state_dir, AT_BOOT) } else { p.flag(state_dir, ACTIVE) };
        if wanted || (recovery && service.start_in_recovery) {
            if let Err(e) = p.start(state_dir) {
                eprintln!("plugin {}: {e}", p.id);
            }
        }
    }
}

/// Creates `dir` and its parents, writable by their owner only (0755, whatever the umask):
/// Dropbear refuses keys under a folder others can write to, and the Kindle's processes run
/// with umask 0.
fn make_dirs(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o755).create(dir)?;
    // DirBuilder's mode is still masked by the umask, and folders made before keep theirs.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
}

/// Copies `from` into `to`, recursively, skipping files of the same size and time, and
/// makes every file executable (the source is vfat, which has no exec bits).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    make_dirs(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let meta = entry.metadata()?;
        if meta.is_dir() {
            copy_tree(&src, &dst)?;
            continue;
        }
        let same =
            std::fs::metadata(&dst).is_ok_and(|d| d.len() == meta.len() && d.modified().ok() == meta.modified().ok());
        if !same {
            // Through a temporary name: a running service's binary is never half written.
            let tmp = dst.with_extension("part");
            std::fs::copy(&src, &tmp)?;
            if let Ok(t) = meta.modified() {
                std::fs::File::options().write(true).open(&tmp)?.set_modified(t)?;
            }
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
            std::fs::rename(&tmp, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const MANIFEST: &str = r#"
display_name = "Echo"
description = "Test plugin"
plugin_version = "1.0.0"
format_version = 1

[service]
executable = "serve"
arguments = ["{setting.port}"]
start_at_boot = true
start_in_recovery = true

[status]
executable = "status"
arguments = ["--port={setting.port}", "{unknown}"]

[[action]]
menu_label = "Say"
executable = "say"

[[action]]
menu_label = "Fail"
executable = "fail"

[[action]]
menu_label = "Sleep"
executable = "sleep"

[[action]]
menu_label = "Slow"
executable = "sleep"
timeout_seconds = 1
output = "show"
shown_in = ["extensions", "book"]

[[action]]
menu_label = "Env"
executable = "env"
arguments = ["{book}"]

[[action]]
menu_label = "Stubborn"
executable = "stubborn"

[[setting]]
id = "port"
kind = "number"
label = "Port"
default_value = 22
minimum = 1
maximum = 65535

[[setting]]
id = "items"
kind = "list"
label = "Items"
list_executable = "items"
"#;

    fn script(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), format!("#!/bin/sh\n{body}\n")).expect("write");
    }

    /// A plugin installed in a scratch folder, copied to a runtime folder like on the device.
    pub(crate) fn setup(name: &str) -> (PathBuf, Plugin) {
        let root = std::env::temp_dir().join(format!("zarathustra-plugin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let installed = root.join("installed/echo");
        std::fs::create_dir_all(&installed).expect("dirs");
        std::fs::write(installed.join("plugin.toml"), MANIFEST).expect("manifest");
        script(&installed, "status", r#"echo "Args=$*"; echo "Port=$ZARATHUSTRA_SETTING_PORT""#);
        script(&installed, "say", r#"echo book; echo second"#);
        script(&installed, "fail", r#"echo partial; echo "first problem" >&2; echo "disk full" >&2; exit 3"#);
        script(&installed, "sleep", "sleep 30");
        script(&installed, "stubborn", "trap '' TERM; touch \"$ZARATHUSTRA_DATA_DIR/trapped\"; sleep 30");
        script(&installed, "env", r#"echo "$1|$ZARATHUSTRA_BOOK|$ZARATHUSTRA_POSITION"; head -c 300000 /dev/urandom"#);
        script(
            &installed,
            "items",
            r#"[ "$1" = list ] && printf 'a\tFirst\nb\tSecond\n'; [ "$1" = remove ] && echo "$2" > "$ZARATHUSTRA_DATA_DIR/removed"; exit 0"#,
        );
        script(&installed, "serve", "trap 'exit 0' TERM; while :; do sleep 0.1; done");
        std::fs::create_dir_all(root.join("installed/Bad Name")).expect("dirs");
        std::fs::write(root.join("installed/Bad Name/plugin.toml"), MANIFEST).expect("manifest");
        let dirs = Dirs {
            installed: root.join("installed"),
            runtime: Some(root.join("runtime")),
            data: root.join("data"),
            database: root.join("state/zarathustra.db"),
        };
        std::fs::create_dir_all(root.join("state")).expect("dirs");
        let mut plugins = discover(&dirs);
        assert_eq!(plugins.len(), 1, "the bad folder name is skipped");
        (root, plugins.remove(0))
    }

    #[test]
    fn placeholders_expand_and_unknown_ones_stay() {
        let values: HashMap<String, String> = [("setting.port".to_string(), "2222".to_string())].into();
        assert_eq!(expand("-p{setting.port}", &values), "-p2222");
        assert_eq!(expand("{nope} {setting.port}}", &values), "{nope} 2222}");
        assert_eq!(expand("{", &values), "{");
        assert_eq!(setting_var("service.at-boot"), "ZARATHUSTRA_SETTING_SERVICE_AT_BOOT");
    }

    #[test]
    fn manifests_parse_and_newer_formats_are_refused() {
        let m: Manifest = toml::from_str(MANIFEST).expect("parses");
        assert_eq!(m.setting[0].kind, Kind::Number { minimum: 1, maximum: 65535, step: 1, default_value: 22 });
        assert!(m.service.as_ref().is_some_and(|s| s.start_in_recovery));
        let dir = std::env::temp_dir().join(format!("zarathustra-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("plugin.toml"), MANIFEST.replace("format_version = 1", "format_version = 9"))
            .expect("w");
        assert!(matches!(load_manifest(&dir.join("plugin.toml")), Err(PluginError::NewerFormat(9))));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_plugin_runs_from_its_runtime_copy_with_its_settings() {
        let (root, p) = setup("run");
        let state = root.join("state");
        assert_eq!(p.values(&state).get("port").map(String::as_str), Some("22"));
        p.set_value(&state, "port", "2222");
        p.set_value(&state, "ignored", "x");
        p.prepare().expect("prepare");
        let status = p.status(&state).expect("status");
        assert_eq!(status, [("Args".into(), "--port=2222 {unknown}".into()), ("Port".into(), "2222".into())]);
        assert!(p.plugin_dir().join("status").exists(), "copied to the runtime folder");
        assert_eq!(
            p.list(&state, "items").expect("list"),
            [("a".into(), "First".into()), ("b".into(), "Second".into())]
        );
        p.remove(&state, "items", "b").expect("remove");
        assert_eq!(std::fs::read_to_string(p.data_dir().join("removed")).expect("removed").trim(), "b");
        assert!(matches!(p.list(&state, "port"), Err(PluginError::NotAList(_))));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn services_start_detached_and_stop() {
        let (root, p) = setup("service");
        let state = root.join("state");
        start_services(&p.dirs, &state, false, false);
        assert!(!p.is_running(), "not active");
        p.set_value(&state, ACTIVE, "true");
        start_services(&p.dirs, &state, false, false);
        assert!(p.is_running());
        p.restart(&state).expect("restart");
        assert!(p.is_running());
        p.stop();
        assert!(!p.is_running());
        // Recovery starts it whatever the user chose; boot only with "start at power-on".
        p.set_value(&state, ACTIVE, "false");
        start_services(&p.dirs, &state, true, false);
        assert!(!p.is_running());
        start_services(&p.dirs, &state, true, true);
        assert!(p.is_running());
        p.stop();
        std::fs::remove_dir_all(root).ok();
    }
}
