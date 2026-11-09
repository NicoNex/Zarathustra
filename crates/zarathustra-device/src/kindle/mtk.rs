//! MediaTek hwtcon e-ink framebuffer (Kindle PW5 and newer), via the mxcfb-style ioctls.
//! Constants and layouts mirror KOReader's `ffi/mxcfb_kindle_h.lua`.

use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;

use crate::sys::{ioc, ioctl, IOC_READ, IOC_WRITE};
use crate::{Canvas, Rect, Refresh, Swipe, Waveform};

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct MxcRect {
    top: u32,
    left: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct AltBuffer {
    phys_addr: u32,
    width: u32,
    height: u32,
    region: MxcRect,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SwipeData {
    direction: u32,
    steps: u32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct UpdateDataMtk {
    region: MxcRect,
    waveform_mode: u32,
    update_mode: u32,
    update_marker: u32,
    temp: i32,
    flags: u32,
    dither_mode: i32,
    quant_bit: i32,
    alt_buffer: AltBuffer,
    swipe: SwipeData,
    hist_bw_waveform_mode: u32,
    hist_gray_waveform_mode: u32,
    ts_pxp: u32,
    ts_epdc: u32,
}

#[repr(C)]
struct MarkerData {
    marker: u32,
    collision_test: u32,
}

const SEND_UPDATE_MTK: u32 = ioc(IOC_WRITE, b'F', 0x2E, size_of::<UpdateDataMtk>());
const WAIT_FOR_UPDATE_COMPLETE: u32 = ioc(IOC_READ | IOC_WRITE, b'F', 0x2F, size_of::<MarkerData>());
const SET_UPDATE_FLAGS_MTK: u32 = ioc(IOC_WRITE, b'F', 0x3B, size_of::<u32>());

const WAVEFORM_DU: u32 = 1;
const WAVEFORM_GC16: u32 = 2;
const WAVEFORM_GL16: u32 = 3;
const WAVEFORM_A2: u32 = 6;
const UPDATE_MODE_PARTIAL: u32 = 0;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;
const FLAG_FORCE_MONOCHROME: u32 = 2;
const FLAG_ENABLE_SWIPE: u32 = 0x10000;
/// Stock reader value; valid 1–60. The driver crashes on regions smaller than this.
const SWIPE_STEPS: u32 = 12;

const FBIOGET_VSCREENINFO: u32 = 0x4600;
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

/// `fb_var_screeninfo` is 40 u32s; we only read a few by index.
const VAR_XRES: usize = 0;
const VAR_YRES: usize = 1;
const VAR_YOFFSET: usize = 5;
const VAR_BPP: usize = 6;
const VAR_ROTATE: usize = 34;

pub struct MtkFb {
    file: File,
    map: *mut u8,
    map_len: usize,
    offset: usize,
    pub width: i32,
    pub height: i32,
    stride: usize,
    pub rotate: u32,
    marker: u32,
    /// What the app draws into. The mapping is uncached: a byte read there costs ~0.3 µs (a
    /// third of the screen read-modify-written took 186 ms on a PW5, against 2 ms to write the
    /// whole screen), and blending and tinting read every pixel. So drawing happens in RAM and
    /// [`MtkFb::refresh`] copies the refreshed rectangle out, write-only.
    back: Vec<u8>,
}

impl MtkFb {
    pub fn open(path: &str) -> io::Result<Self> {
        let file = File::options().read(true).write(true).open(path)?;
        let fd = file.as_raw_fd();
        let mut var = [0u32; 40];
        // SAFETY: fb_var_screeninfo is 40 u32s.
        unsafe { ioctl(fd, FBIOGET_VSCREENINFO, var.as_mut_ptr())? };
        // SAFETY: FixInfo is plain old data; all-zero is valid.
        let mut fix: FixInfo = unsafe { std::mem::zeroed() };
        // SAFETY: FixInfo mirrors fb_fix_screeninfo.
        unsafe { ioctl(fd, FBIOGET_FSCREENINFO, &mut fix)? };
        if var[VAR_BPP] != 8 {
            // ponytail: 8-bit gray only (what MTK Kindles expose); add a converter if a device needs 16/32 bpp.
            return Err(io::Error::new(io::ErrorKind::Unsupported, format!("{} bpp framebuffer", var[VAR_BPP])));
        }
        let map_len = fix.smem_len as usize;
        // SAFETY: mapping the framebuffer device's whole memory, shared, read/write.
        let map = unsafe {
            libc::mmap(std::ptr::null_mut(), map_len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0)
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let mut fb = MtkFb {
            map: map.cast(),
            map_len,
            offset: var[VAR_YOFFSET] as usize * fix.line_length as usize,
            width: var[VAR_XRES] as i32,
            height: var[VAR_YRES] as i32,
            stride: fix.line_length as usize,
            rotate: var[VAR_ROTATE],
            marker: 0,
            back: Vec::new(),
            file,
        };
        // Start from what the screen shows, so a partial refresh never copies out stale zeros.
        fb.back = fb.visible().to_vec();
        // Keep the driver from silently promoting refreshes to slow REAGL.
        let mut fast = 0x8000_0000u32 | 1;
        // SAFETY: the request takes one u32.
        if let Err(e) = unsafe { ioctl(fd, SET_UPDATE_FLAGS_MTK, &mut fast) } {
            // Best effort: refreshes still work, the driver may just pick slower REAGL.
            eprintln!("MTK fast mode unavailable: {e}");
        }
        Ok(fb)
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

    pub fn refresh(&mut self, r: &Refresh) -> io::Result<u32> {
        let rect = r.rect.intersect(&Rect::new(0, 0, self.width, self.height));
        if rect.is_empty() {
            return Ok(0);
        }
        let (x, w, stride) = (rect.x as usize, rect.w as usize, self.stride);
        let back = std::mem::take(&mut self.back);
        let fb = self.visible();
        for y in rect.y as usize..rect.bottom() as usize {
            let row = y * stride + x..y * stride + x + w;
            fb[row.clone()].copy_from_slice(&back[row]);
        }
        self.back = back;
        self.marker = self.marker.wrapping_add(1).max(1);
        let (waveform_mode, mut flags) = match r.waveform {
            Waveform::Full => (WAVEFORM_GC16, 0),
            Waveform::Text => (WAVEFORM_GL16, 0),
            Waveform::Fast => (WAVEFORM_DU, FLAG_FORCE_MONOCHROME),
            Waveform::Mono => (WAVEFORM_A2, 0),
        };
        let mut swipe = SwipeData { direction: 0, steps: SWIPE_STEPS };
        if let Some(dir) = r.swipe {
            if rect.w as u32 >= SWIPE_STEPS && rect.h as u32 >= SWIPE_STEPS {
                flags |= FLAG_ENABLE_SWIPE;
                swipe.direction = match dir {
                    Swipe::Down => 0,
                    Swipe::Up => 1,
                    Swipe::Left => 2,
                    Swipe::Right => 3,
                };
            }
        }
        let mut data = UpdateDataMtk {
            region: MxcRect { top: rect.y as u32, left: rect.x as u32, width: rect.w as u32, height: rect.h as u32 },
            waveform_mode,
            update_mode: if r.flash { UPDATE_MODE_FULL } else { UPDATE_MODE_PARTIAL },
            update_marker: self.marker,
            temp: TEMP_USE_AMBIENT,
            flags,
            swipe,
            hist_bw_waveform_mode: WAVEFORM_DU,
            hist_gray_waveform_mode: WAVEFORM_GC16,
            ..Default::default()
        };
        // SAFETY: UpdateDataMtk mirrors mxcfb_update_data_mtk (size checked in tests).
        unsafe { ioctl(self.file.as_raw_fd(), SEND_UPDATE_MTK, &mut data)? };
        Ok(self.marker)
    }

    pub fn wait(&mut self, marker: u32) -> io::Result<()> {
        if marker == 0 {
            return Ok(());
        }
        let mut data = MarkerData { marker, collision_test: 0 };
        // SAFETY: MarkerData mirrors mxcfb_update_marker_data.
        unsafe { ioctl(self.file.as_raw_fd(), WAIT_FOR_UPDATE_COMPLETE, &mut data) }
    }
}

impl Drop for MtkFb {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what open() mapped.
        unsafe { libc::munmap(self.map.cast(), self.map_len) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_struct_and_ioctls_match_kindle_headers() {
        assert_eq!(size_of::<UpdateDataMtk>(), 96);
        assert_eq!(SEND_UPDATE_MTK, 1080051246);
        assert_eq!(WAIT_FOR_UPDATE_COMPLETE, 3221767727);
        assert_eq!(SET_UPDATE_FLAGS_MTK, 1074021947);
    }
}
