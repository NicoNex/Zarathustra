//! Plugin actions run in the background: detached in their own process group (they outlive
//! the app, like services), their output and exit status in files of the plugin's data dir,
//! so the last run's result survives the app restarting. One run per plugin at a time.
//!
//! Files: `action.pid`, `action.run` (start time in ms, timeout in s or empty, action index,
//! label), `action.out`/`action.err` (output), `action.status` (exit code, written by the `sh`
//! wrapper), `action.end` (written by the app: `stopped`, `timeout` or `error: …`).

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{live_pid, terminate, Plugin, PluginError, Result};

/// Most of a stream the output sheet reads.
const SHEET_MAX: u64 = 256 * 1024;
/// How much of a stream the one-line summary looks at (its head or its tail).
const LINE_WINDOW: u64 = 4096;
/// How often the reaper looks at a running action.
const REAP_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Running,
    /// Exit code (128 + n when killed by signal n, as the shell reports it).
    Exited(i32),
    /// Stopped by the user, or interrupted (the device restarted).
    Stopped,
    TimedOut,
    SpawnFailed(String),
}

/// The plugin's last action run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastRun {
    /// Index in the manifest's actions.
    pub action: usize,
    pub label: String,
    pub started: SystemTime,
    /// `None` while running.
    pub duration: Option<Duration>,
    pub outcome: Outcome,
    /// First non-empty stdout line, last non-empty stderr line.
    pub stdout_head: String,
    pub stderr_tail: String,
}

impl LastRun {
    pub fn running(&self) -> bool {
        self.outcome == Outcome::Running
    }

    pub fn failed(&self) -> bool {
        matches!(self.outcome, Outcome::Exited(c) if c != 0)
            || matches!(self.outcome, Outcome::TimedOut | Outcome::SpawnFailed(_))
    }
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A file's text, lossy, from `from` (negative: that many bytes before the end) up to `max`
/// bytes.
fn read_part(path: &Path, from: i64, max: u64) -> String {
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let pos = if from < 0 { SeekFrom::End(from) } else { SeekFrom::Start(from as u64) };
    // A file shorter than the tail window: read it from the start.
    if f.seek(pos).is_err() {
        let _ = f.seek(SeekFrom::Start(0));
    }
    let mut buf = Vec::new();
    let _ = f.take(max).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn first_line(s: &str) -> String {
    s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string()
}

fn last_line(s: &str) -> String {
    s.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or_default().to_string()
}

impl Plugin {
    fn action_file(&self, name: &str) -> PathBuf {
        self.data_dir().join(format!("action.{name}"))
    }

    /// The last run's `action.run`: start, timeout, action index, label.
    fn read_run(&self) -> Option<(SystemTime, Option<Duration>, usize, String)> {
        let run = std::fs::read_to_string(self.action_file("run")).ok()?;
        let mut lines = run.lines();
        let started = UNIX_EPOCH + Duration::from_millis(lines.next()?.parse().ok()?);
        let timeout = lines.next()?.parse().ok().map(Duration::from_secs);
        let action = lines.next()?.parse().ok()?;
        Some((started, timeout, action, lines.next().unwrap_or_default().to_string()))
    }

    /// The running action's process (the `sh` wrapper, its group leader), if any.
    fn action_pid(&self) -> Option<i32> {
        let (_, _, i, _) = self.read_run()?;
        let action = self.manifest.as_ref().ok()?.action.get(i)?;
        live_pid(&self.action_file("pid"), &self.plugin_dir().join(&action.run.executable))
    }

    /// Starts action `i` in the background. `book` is the book it acts on (book menu): its
    /// path and saved position, as `{book}`/`ZARATHUSTRA_BOOK` and `ZARATHUSTRA_POSITION`.
    pub fn run(&self, state_dir: &Path, i: usize, book: Option<(&Path, &str)>) -> Result<()> {
        let Some(action) = self.manifest.as_ref().ok().and_then(|m| m.action.get(i)).cloned() else {
            return Ok(());
        };
        if self.action_pid().is_some() {
            return Err(PluginError::Busy);
        }
        // The run is recorded before anything can fail, so a failure shows on its line.
        super::make_dirs(&self.data_dir())?;
        for f in ["pid", "status", "end", "out", "err"] {
            let _ = std::fs::remove_file(self.action_file(f));
        }
        let started = SystemTime::now();
        let timeout = action.timeout_seconds.map(|s| s.to_string()).unwrap_or_default();
        let label = self.action_label(&action).replace('\n', " ");
        std::fs::write(self.action_file("run"), format!("{}\n{timeout}\n{i}\n{label}\n", millis(started)))?;
        let mut child = match self.spawn_action(state_dir, &action, book) {
            Ok(child) => child,
            Err(e) => {
                let _ = std::fs::write(self.action_file("end"), format!("error: {e}\n"));
                return Err(e);
            }
        };
        // Reaps the wrapper (no zombies) and enforces the timeout while the app runs; after a
        // restart, [`Plugin::last_run`] enforces it.
        let me = self.clone();
        let limit = action.timeout_seconds.map(Duration::from_secs);
        std::thread::spawn(move || loop {
            match child.try_wait() {
                Ok(None) => {
                    if limit.is_some_and(|l| started.elapsed().unwrap_or_default() >= l) {
                        me.end_action("timeout");
                    }
                    std::thread::sleep(REAP_EVERY);
                }
                _ => return,
            }
        });
        Ok(())
    }

    /// Refreshes the runtime copy and starts the action's wrapper, its output in files.
    fn spawn_action(
        &self,
        state_dir: &Path,
        action: &super::Action,
        book: Option<(&Path, &str)>,
    ) -> Result<std::process::Child> {
        use std::os::unix::process::CommandExt;
        self.prepare()?;

        let book_path = book.map(|(p, _)| p.to_string_lossy().into_owned()).unwrap_or_default();
        let position = book.map_or("", |(_, pos)| pos);
        let extra = [("book", book_path.as_str()), ("position", position), ("selection", "")];
        let inner = self.command(&action.run, &self.values(state_dir), &extra);
        // The wrapper records the exit status; the action's own arguments pass through "$@"
        // untouched (no shell parsing of them).
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(r#"s=$1; shift; "$@"; echo $? > "$s""#).arg("sh").arg(self.action_file("status"));
        cmd.arg(inner.get_program()).args(inner.get_args());
        for (k, v) in inner.get_envs() {
            if let Some(v) = v {
                cmd.env(k, v);
            }
        }
        if let Some(dir) = inner.get_current_dir() {
            cmd.current_dir(dir);
        }
        let out = std::fs::File::create(self.action_file("out"))?;
        let err = std::fs::File::create(self.action_file("err"))?;
        cmd.stdin(Stdio::null()).stdout(out).stderr(err).process_group(0);
        let executable = || action.run.executable.clone();
        let child = cmd.spawn().map_err(|source| PluginError::Spawn { executable: executable(), source })?;
        std::fs::write(self.action_file("pid"), format!("{}\n", child.id()))?;
        Ok(child)
    }

    /// Stops the running action: SIGTERM to its group, SIGKILL after the grace time. Returns
    /// at once.
    pub fn stop_action(&self) {
        self.end_action("stopped");
    }

    /// Records why the run ends and terminates it in the background; once per run (the
    /// reaper and [`Plugin::last_run`] may both ask).
    fn end_action(&self, why: &str) {
        let Some(pid) = self.action_pid() else { return };
        if self.action_file("end").exists() {
            return;
        }
        if std::fs::write(self.action_file("end"), format!("{why}\n")).is_err() {
            eprintln!("plugin {}: can't record the action's end", self.id);
        }
        // Alive while anything of its process group is: the wrapper dies at the first SIGTERM,
        // an action that traps it must still get the SIGKILL.
        let group = nix::unistd::Pid::from_raw(pid);
        std::thread::spawn(move || terminate(pid, || nix::sys::signal::killpg(group, None).is_ok()));
    }

    /// The last action run, if any. Stops a run past its timeout (the app was not running
    /// when it expired).
    pub fn last_run(&self) -> Option<LastRun> {
        let (started, timeout, action, label) = self.read_run()?;
        let elapsed = started.elapsed().unwrap_or_default();
        let end = std::fs::read_to_string(self.action_file("end")).ok();
        let outcome = if self.action_pid().is_some() {
            if timeout.is_some_and(|t| elapsed >= t) {
                self.end_action("timeout");
                Outcome::TimedOut
            } else {
                Outcome::Running
            }
        } else if let Some(end) = end.as_deref().map(str::trim) {
            match end {
                "timeout" => Outcome::TimedOut,
                e if e.starts_with("error: ") => Outcome::SpawnFailed(e["error: ".len()..].to_string()),
                _ => Outcome::Stopped,
            }
        } else if let Some(code) =
            std::fs::read_to_string(self.action_file("status")).ok().and_then(|s| s.trim().parse().ok())
        {
            Outcome::Exited(code)
        } else {
            Outcome::Stopped
        };
        // The run ended when its status (or end) file was written.
        let ended =
            ["end", "status"].iter().filter_map(|f| std::fs::metadata(self.action_file(f)).ok()?.modified().ok()).min();
        let duration = match outcome {
            Outcome::Running => None,
            _ => Some(ended.and_then(|t| t.duration_since(started).ok()).unwrap_or(elapsed)),
        };
        Some(LastRun {
            action,
            label,
            started,
            duration,
            outcome,
            stdout_head: first_line(&read_part(&self.action_file("out"), 0, LINE_WINDOW)),
            stderr_tail: last_line(&read_part(&self.action_file("err"), -(LINE_WINDOW as i64), LINE_WINDOW)),
        })
    }

    /// The last run's stdout and stderr for the output sheet: at most [`SHEET_MAX`] bytes each,
    /// invalid UTF-8 replaced.
    pub fn action_output(&self) -> (String, String) {
        (read_part(&self.action_file("out"), 0, SHEET_MAX), read_part(&self.action_file("err"), 0, SHEET_MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::tests::setup;
    use crate::plugin::{Output, Place};

    /// The last run once it is no longer running (within 10 s).
    fn finished(p: &Plugin) -> LastRun {
        for _ in 0..200 {
            match p.last_run() {
                Some(r) if !r.running() => return r,
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        panic!("the action never finished: {:?}", p.last_run());
    }

    #[test]
    fn actions_run_in_the_background_and_keep_their_result() {
        let (root, p) = setup("actions");
        let state = root.join("state");
        let m = p.manifest.as_ref().expect("manifest");
        assert_eq!((m.action[0].output, &m.action[0].shown_in[..]), (Output::Quiet, &[Place::Extensions][..]));
        assert_eq!(
            (m.action[3].output, &m.action[3].shown_in[..]),
            (Output::Show, &[Place::Extensions, Place::Book][..])
        );

        p.run(&state, 0, None).expect("run");
        let r = finished(&p);
        assert_eq!((r.label.as_str(), &r.outcome, r.stdout_head.as_str()), ("Say", &Outcome::Exited(0), "book"));
        assert!(!r.failed() && r.duration.is_some());

        p.run(&state, 1, None).expect("run");
        let r = finished(&p);
        assert_eq!((&r.outcome, r.stderr_tail.as_str()), (&Outcome::Exited(3), "disk full"));
        assert!(r.failed());
        assert_eq!(p.action_output(), ("partial\n".into(), "first problem\ndisk full\n".into()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_running_action_is_busy_survives_a_restart_and_stops() {
        let (root, p) = setup("stop");
        let state = root.join("state");
        p.run(&state, 2, None).expect("run");
        assert!(matches!(p.run(&state, 0, None), Err(PluginError::Busy)), "one run at a time");
        // A new value, as after the app restarted: still running.
        let again = p.clone();
        assert!(again.last_run().is_some_and(|r| r.running() && r.label == "Sleep"));
        again.stop_action();
        let r = finished(&again);
        assert_eq!(r.outcome, Outcome::Stopped);
        assert!(!r.failed());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_timeout_stops_the_action() {
        let (root, p) = setup("timeout");
        p.run(&root.join("state"), 3, None).expect("run");
        let r = finished(&p);
        assert_eq!(r.outcome, Outcome::TimedOut);
        assert!(r.failed() && r.duration.is_some_and(|d| d < Duration::from_secs(5)));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_reused_pid_is_not_a_running_action() {
        let (root, p) = setup("pid");
        p.run(&root.join("state"), 0, None).expect("run");
        finished(&p);
        // Our own pid runs the test binary, not the action.
        std::fs::write(p.action_file("pid"), format!("{}\n", std::process::id())).expect("pid");
        assert!(p.last_run().is_some_and(|r| !r.running()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn book_actions_get_the_book_and_huge_binary_output_is_capped() {
        let (root, p) = setup("book");
        let book = Path::new("/mnt/us/library/Un libro.epub");
        p.run(&root.join("state"), 4, Some((book, "3 12 40"))).expect("run");
        let r = finished(&p);
        let want = "/mnt/us/library/Un libro.epub|/mnt/us/library/Un libro.epub|3 12 40";
        assert!(r.stdout_head.starts_with(want), "{:?}", r.stdout_head);
        let (out, _) = p.action_output();
        assert!(out.len() as u64 <= SHEET_MAX * 3, "lossy text of at most SHEET_MAX bytes");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stop_kills_an_action_that_ignores_sigterm() {
        let (root, p) = setup("stubborn");
        p.run(&root.join("state"), 5, None).expect("run");
        std::thread::sleep(Duration::from_millis(200));
        let pid: i32 = std::fs::read_to_string(p.action_file("pid")).expect("pid").trim().parse().expect("number");
        let group = nix::unistd::Pid::from_raw(pid);
        p.stop_action();
        let gone = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            nix::sys::signal::killpg(group, None).is_err()
        });
        assert!(gone, "the whole group is killed after the grace time");
        std::fs::remove_dir_all(root).ok();
    }
}
