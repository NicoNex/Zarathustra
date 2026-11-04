//! Small Linux ioctl helpers shared by the backends.

use std::io;
use std::os::fd::RawFd;

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
