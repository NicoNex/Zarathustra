//! NXP i.MX EPDC refreshes on Kobo "Mark 7" and later boards (Clara 2E…), via the mxcfb
//! ioctls. Struct layouts and numbers follow FBInk's `eink/mxcfb-kobo.h` (facts only).

use std::io;
use std::mem::size_of;
use std::os::fd::RawFd;

use crate::sys::{ioc, ioctl, IOC_READ, IOC_WRITE};
use crate::{Rect, Waveform};

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

/// `mxcfb_update_data` (v2, Mark 7).
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct UpdateData {
    region: MxcRect,
    waveform_mode: u32,
    update_mode: u32,
    update_marker: u32,
    temp: i32,
    flags: u32,
    dither_mode: i32,
    quant_bit: i32,
    alt_buffer: AltBuffer,
}

#[repr(C)]
struct MarkerData {
    marker: u32,
    collision_test: u32,
}

const SEND_UPDATE_V2: u32 = ioc(IOC_WRITE, b'F', 0x2E, size_of::<UpdateData>());
const WAIT_FOR_UPDATE_COMPLETE_V3: u32 = ioc(IOC_READ | IOC_WRITE, b'F', 0x2F, size_of::<MarkerData>());

// NTX waveform modes (the first number in sysfs `waveform_mode_*` on the Clara 2E).
const WAVEFORM_DU: u32 = 1;
const WAVEFORM_GC16: u32 = 2;
const WAVEFORM_A2: u32 = 4;
const WAVEFORM_GL16: u32 = 5;
const UPDATE_MODE_PARTIAL: u32 = 0;
const UPDATE_MODE_FULL: u32 = 1;
const TEMP_USE_AMBIENT: i32 = 0x1000;
const FLAG_FORCE_MONOCHROME: u32 = 2;

/// Submits a refresh of `rect` tagged `marker`.
pub fn send_update(fd: RawFd, rect: Rect, waveform: Waveform, flash: bool, marker: u32) -> io::Result<()> {
    let (waveform_mode, flags) = match waveform {
        Waveform::Full => (WAVEFORM_GC16, 0),
        Waveform::Text => (WAVEFORM_GL16, 0),
        Waveform::Fast => (WAVEFORM_DU, FLAG_FORCE_MONOCHROME),
        Waveform::Mono => (WAVEFORM_A2, 0),
    };
    let mut data = UpdateData {
        region: MxcRect { top: rect.y as u32, left: rect.x as u32, width: rect.w as u32, height: rect.h as u32 },
        waveform_mode,
        update_mode: if flash { UPDATE_MODE_FULL } else { UPDATE_MODE_PARTIAL },
        update_marker: marker,
        temp: TEMP_USE_AMBIENT,
        flags,
        ..Default::default()
    };
    // SAFETY: UpdateData mirrors mxcfb_update_data v2 (size checked in tests).
    unsafe { ioctl(fd, SEND_UPDATE_V2, &mut data) }
}

/// Waits until the refresh tagged `marker` is on the panel.
pub fn wait(fd: RawFd, marker: u32) -> io::Result<()> {
    let mut data = MarkerData { marker, collision_test: 0 };
    // SAFETY: MarkerData mirrors mxcfb_update_marker_data.
    unsafe { ioctl(fd, WAIT_FOR_UPDATE_COMPLETE_V3, &mut data) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_struct_and_ioctls_match_kobo_headers() {
        assert_eq!(size_of::<UpdateData>(), 72);
        assert_eq!(SEND_UPDATE_V2, 0x4048_462E);
        assert_eq!(WAIT_FOR_UPDATE_COMPLETE_V3, 0xC008_462F);
    }
}
