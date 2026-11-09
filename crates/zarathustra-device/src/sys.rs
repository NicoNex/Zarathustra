//! Small Linux ioctl helpers shared by the backends.

use std::io::{self, Write};
use std::os::fd::RawFd;
use std::process::{Command, Stdio};

pub const IOC_WRITE: u32 = 1;
pub const IOC_READ: u32 = 2;

/// Linux `_IOC(dir, type, nr, size)` request encoding.
pub const fn ioc(dir: u32, ty: u8, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((ty as u32) << 8) | nr
}

/// `ioctl(2)` mapped to `io::Result`.
///
/// # Safety
/// `arg` must point to valid memory of the type and size that `req` expects.
pub unsafe fn ioctl<T>(fd: RawFd, req: u32, arg: *mut T) -> io::Result<()> {
    // SAFETY: forwarded to the caller's contract.
    if unsafe { libc::ioctl(fd, req as _, arg) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Runs `cmd`, feeding it `input`; its trimmed stdout, or an error with its stderr.
pub fn run(cmd: &mut Command, input: Option<&str>) -> io::Result<String> {
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
