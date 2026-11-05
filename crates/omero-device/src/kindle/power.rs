//! The power button and sleep, through the Kindle's `powerd`.
//!
//! `powerd` owns the power key (`/dev/input/event0`) and the suspend state machine even with
//! the Amazon framework stopped: a press moves it from `active` to `screenSaver`
//! (`goingToScreenSaver 2`), then a few seconds later to `readyToSuspend <delay>` and into
//! suspend; another press resumes (`wakeupFromSuspend`) and leaves the screen saver
//! (`outOfScreenSaver 1`). We follow it rather than fight it: listen to its events, draw our
//! own screen saver, and let it suspend. See `docs/superpowers/notes/kindle-power.md`.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdout, Command, Stdio};

use crate::PowerEvent;

const EVENTS: &str = "goingToScreenSaver,outOfScreenSaver,readyToSuspend,wakeupFromSuspend";

/// A `lipc-wait-event` child streaming powerd's events, one per line (`name [int args]`).
// ponytail: the CLI tool instead of liblipc FFI; it line-buffers its stdout (verified on
// FW 5.17.1) and costs one idle process. Switch to FFI only if we need to publish properties.
pub struct Powerd {
    child: Child,
    out: ChildStdout,
    line: Vec<u8>,
}

impl Powerd {
    pub fn spawn() -> io::Result<Self> {
        let mut cmd = Command::new("lipc-wait-event");
        cmd.args(["-m", "com.lab126.powerd", EVENTS]).stdin(Stdio::null()).stdout(Stdio::piped());
        // SAFETY: the closure runs in the forked child before exec and only calls prctl(2),
        // which is async-signal-safe. PR_SET_PDEATHSIG kills the listener if we die, even
        // from SIGKILL, so a crash never leaves it behind.
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn()?;
        let out = child.stdout.take().ok_or_else(|| io::Error::other("lipc-wait-event: no stdout"))?;
        // SAFETY: plain fcntl flag changes on a pipe fd we own.
        unsafe {
            let fd = out.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Powerd { child, out, line: Vec::new() })
    }

    pub fn fd(&self) -> RawFd {
        self.out.as_raw_fd()
    }

    /// Drains pending events (non-blocking). EOF (the listener died) is an error.
    pub fn read(&mut self, out: &mut Vec<PowerEvent>) -> io::Result<()> {
        let mut buf = [0u8; 256];
        loop {
            match self.out.read(&mut buf) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "lipc-wait-event exited")),
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' {
                            out.extend(parse(&String::from_utf8_lossy(&self.line)));
                            self.line.clear();
                        } else {
                            self.line.push(b);
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for Powerd {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// One `lipc-wait-event -m` line, e.g. `goingToScreenSaver 2` or `readyToSuspend 5`.
fn parse(line: &str) -> Option<PowerEvent> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    let arg = words.next().and_then(|w| w.parse::<u32>().ok());
    Some(match name {
        "goingToScreenSaver" => PowerEvent::Sleep,
        "outOfScreenSaver" => PowerEvent::Wake,
        "readyToSuspend" => PowerEvent::Suspend { delay: arg.unwrap_or(0) },
        "wakeupFromSuspend" => PowerEvent::Resumed,
        _ => return None,
    })
}

/// `lipc-set-prop -i com.lab126.powerd <prop> <value>`.
pub fn set_prop(prop: &str, value: u32) -> io::Result<()> {
    let status = Command::new("lipc-set-prop")
        .args(["-i", "com.lab126.powerd", prop, &value.to_string()])
        .stdout(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("lipc-set-prop {prop} {value}: {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_powerd_lines() {
        assert_eq!(parse("goingToScreenSaver 2"), Some(PowerEvent::Sleep));
        assert_eq!(parse("outOfScreenSaver 1"), Some(PowerEvent::Wake));
        assert_eq!(parse("readyToSuspend 5"), Some(PowerEvent::Suspend { delay: 5 }));
        assert_eq!(parse("wakeupFromSuspend 0"), Some(PowerEvent::Resumed));
        assert_eq!(parse("battLevelChanged 80"), None);
        assert_eq!(parse(""), None);
    }
}
