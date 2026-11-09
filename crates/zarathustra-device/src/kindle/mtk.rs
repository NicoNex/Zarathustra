//! MediaTek hwtcon e-ink framebuffer (Kindle PW5 and newer), via the mxcfb-style ioctls.
//! Constants and layouts mirror KOReader's `ffi/mxcfb_kindle_h.lua`.

use std::io;
use std::mem::size_of;

use crate::fb::Framebuffer;
use crate::sys::{ioc, ioctl, IOC_READ, IOC_WRITE};
use crate::{Refresh, Swipe, Waveform};

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

pub struct MtkFb {
    pub fb: Framebuffer,
    marker: u32,
}

impl MtkFb {
    pub fn open(path: &str) -> io::Result<Self> {
        let fb = Framebuffer::open(path, false)?;
        // Keep the driver from silently promoting refreshes to slow REAGL.
        let mut fast = 0x8000_0000u32 | 1;
        // SAFETY: the request takes one u32.
        if let Err(e) = unsafe { ioctl(fb.fd(), SET_UPDATE_FLAGS_MTK, &mut fast) } {
            // Best effort: refreshes still work, the driver may just pick slower REAGL.
            eprintln!("MTK fast mode unavailable: {e}");
        }
        Ok(MtkFb { fb, marker: 0 })
    }

    pub fn refresh(&mut self, r: &Refresh) -> io::Result<u32> {
        let rect = self.fb.flush(r.rect);
        if rect.is_empty() {
            return Ok(0);
        }
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
        unsafe { ioctl(self.fb.fd(), SEND_UPDATE_MTK, &mut data)? };
        Ok(self.marker)
    }

    pub fn wait(&mut self, marker: u32) -> io::Result<()> {
        if marker == 0 {
            return Ok(());
        }
        let mut data = MarkerData { marker, collision_test: 0 };
        // SAFETY: MarkerData mirrors mxcfb_update_marker_data.
        unsafe { ioctl(self.fb.fd(), WAIT_FOR_UPDATE_COMPLETE, &mut data) }
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
