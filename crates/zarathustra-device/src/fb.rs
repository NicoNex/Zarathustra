//! A Linux framebuffer drawn through a RAM back buffer, shared by the e-ink backends: each
//! adds its own refresh ioctls on top.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, RawFd};

use crate::sys::ioctl;
use crate::{Canvas, Rect};

const FBIOGET_VSCREENINFO: u32 = 0x4600;
const FBIOPUT_VSCREENINFO: u32 = 0x4601;
const FBIOGET_FSCREENINFO: u32 = 0x4602;

#[repr(C)]
struct FixInfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    ty: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

/// `fb_var_screeninfo` is 40 u32s; we only touch a few by index.
type VarInfo = [u32; 40];
const VAR_XRES: usize = 0;
const VAR_YRES: usize = 1;
const VAR_YOFFSET: usize = 5;
const VAR_BPP: usize = 6;
const VAR_GRAYSCALE: usize = 7;
const VAR_ROTATE: usize = 34;
/// `grayscale` value for plain 8-bit gray (mxcfb's GRAYSCALE_8BIT).
const GRAYSCALE_8BIT: u32 = 1;

pub struct Framebuffer {
    file: File,
    map: *mut u8,
    map_len: usize,
    offset: usize,
    pub width: i32,
    pub height: i32,
    stride: usize,
    pub rotate: u32,
    /// What the app draws into. Framebuffer mappings are uncached: a byte read there costs
    /// ~0.3 µs (a third of the screen read-modify-written took 186 ms on a PW5, against 2 ms
    /// to write the whole screen), and blending and tinting read every pixel. So drawing
    /// happens in RAM and [`Framebuffer::flush`] copies a rectangle out, write-only.
    back: Vec<u8>,
    /// The mode found at open, put back on drop when [`Framebuffer::open`] changed it.
    saved: Option<VarInfo>,
}

fn var_info(fd: RawFd) -> io::Result<VarInfo> {
    let mut var = [0u32; 40];
    // SAFETY: fb_var_screeninfo is 40 u32s.
    unsafe { ioctl(fd, FBIOGET_VSCREENINFO, var.as_mut_ptr())? };
    Ok(var)
}

impl Framebuffer {
    /// Maps the framebuffer at `path`. It must be 8-bit gray; with `to_gray`, another depth
    /// (a Kobo's 32 bpp, as Nickel leaves it) is switched to 8-bit gray and restored on drop.
    pub fn open(path: &str, to_gray: bool) -> io::Result<Self> {
        let file = File::options().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        let mut var = var_info(fd)?;
        let mut saved = None;
        if var[VAR_BPP] != 8 {
            if !to_gray {
                // shortcut: 8-bit gray only (what MTK Kindles expose); add a converter if a device can't switch.
                return Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} bpp framebuffer", var[VAR_BPP])));
            }
            saved = Some(var);
            var[VAR_BPP] = 8;
            var[VAR_GRAYSCALE] = GRAYSCALE_8BIT;
            // SAFETY: fb_var_screeninfo is 40 u32s, read just above.
            unsafe { ioctl(fd, FBIOPUT_VSCREENINFO, var.as_mut_ptr())? };
            var = var_info(fd)?;
            if var[VAR_BPP] != 8 {
                return Err(io::Error::other(format!("framebuffer stayed at {} bpp", var[VAR_BPP])));
            }
        }
        // SAFETY: FixInfo is plain old data; all-zero is valid.
        let mut fix: FixInfo = unsafe { std::mem::zeroed() };
        // SAFETY: FixInfo mirrors fb_fix_screeninfo.
        unsafe { ioctl(fd, FBIOGET_FSCREENINFO, &mut fix)? };
        let map_len = fix.smem_len as usize;
        // SAFETY: mapping the framebuffer device's whole memory, shared, read/write.
        let map = unsafe {
            libc::mmap(std::ptr::null_mut(), map_len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0)
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let mut fb = Framebuffer {
            map: map.cast(),
            map_len,
            offset: var[VAR_YOFFSET] as usize * fix.line_length as usize,
            width: var[VAR_XRES] as i32,
            height: var[VAR_YRES] as i32,
            stride: fix.line_length as usize,
            rotate: var[VAR_ROTATE],
            back: Vec::new(),
            saved,
            file,
        };
        // Start from what the screen shows, so a partial refresh never copies out stale zeros.
        fb.back = fb.visible().to_vec();
        Ok(fb)
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// The visible area of the mapping.
    fn visible(&mut self) -> &mut [u8] {
        let len = self.stride * self.height as usize;
        assert!(self.offset + len <= self.map_len);
        // SAFETY: the visible area lies inside the mapping (asserted) and we hold &mut self.
        unsafe { std::slice::from_raw_parts_mut(self.map.add(self.offset), len) }
    }

    pub fn canvas(&mut self) -> Canvas<'_> {
        Canvas { width: self.width, height: self.height, stride: self.stride, pixels: &mut self.back }
    }

    /// Copies `rect` (clipped to the screen) from the back buffer to the screen memory and
    /// returns the clipped rectangle, empty when nothing is left to refresh.
    pub fn flush(&mut self, rect: Rect) -> Rect {
        let rect = rect.intersect(&Rect::new(0, 0, self.width, self.height));
        if rect.is_empty() {
            return rect;
        }
        let (x, w, stride) = (rect.x as usize, rect.w as usize, self.stride);
        let back = std::mem::take(&mut self.back);
        let fb = self.visible();
        for y in rect.y as usize..rect.bottom() as usize {
            let row = y * stride + x..y * stride + x + w;
            fb[row.clone()].copy_from_slice(&back[row]);
        }
        self.back = back;
        rect
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what open() mapped.
        unsafe { libc::munmap(self.map.cast(), self.map_len) };
        if let Some(mut var) = self.saved.take() {
            // SAFETY: fb_var_screeninfo is 40 u32s, as read at open.
            if let Err(e) = unsafe { ioctl(self.file.as_raw_fd(), FBIOPUT_VSCREENINFO, var.as_mut_ptr()) } {
                eprintln!("framebuffer mode not restored: {e}");
            }
        }
    }
}
