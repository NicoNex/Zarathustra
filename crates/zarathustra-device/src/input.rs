//! Linux evdev multitouch (protocol B) decoding.

use std::fs::{self, File};
use std::io::{self, Read};
use std::mem::size_of;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::sys::{ioc, ioctl, IOC_READ};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touch {
    pub id: i32,
    pub phase: Phase,
    pub x: i32,
    pub y: i32,
}

const EV_SYN: u16 = 0;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const SYN_DROPPED: u16 = 3;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;

#[derive(Clone, Copy)]
struct Slot {
    /// Tracking id last reported by the kernel (-1 = no contact).
    id: i32,
    /// Id of the contact we reported as down, -1 when none.
    down_id: i32,
    x: i32,
    y: i32,
    dirty: bool,
}

const EMPTY_SLOT: Slot = Slot { id: -1, down_id: -1, x: 0, y: 0, dirty: false };

/// Turns raw `(type, code, value)` events into touch phases, one batch per `SYN_REPORT`.
#[derive(Default)]
pub struct MtDecoder {
    slots: Vec<Slot>,
    cur: usize,
    /// After `SYN_DROPPED`, events are discarded until the next `SYN_REPORT`.
    dropping: bool,
}

impl MtDecoder {
    fn slot(&mut self) -> &mut Slot {
        if self.cur >= self.slots.len() {
            self.slots.resize(self.cur + 1, EMPTY_SLOT);
        }
        let s = &mut self.slots[self.cur];
        s.dirty = true;
        s
    }

    pub fn feed(&mut self, ty: u16, code: u16, value: i32, out: &mut Vec<Touch>) {
        if self.dropping {
            self.dropping = !(ty == EV_SYN && code == SYN_REPORT);
            return;
        }
        match (ty, code) {
            // ponytail: slot index is trusted; the kernel bounds it by the device's slot count.
            (EV_ABS, ABS_MT_SLOT) => self.cur = value.clamp(0, 63) as usize,
            (EV_ABS, ABS_MT_TRACKING_ID) => self.slot().id = value,
            (EV_ABS, ABS_MT_POSITION_X) => self.slot().x = value,
            (EV_ABS, ABS_MT_POSITION_Y) => self.slot().y = value,
            (EV_SYN, SYN_DROPPED) => {
                // Kernel queue overflowed: end every contact so none stays stuck down.
                // ponytail: no EVIOCGMTSLOTS resync; a finger still down reappears as Down on its next move.
                for s in self.slots.iter_mut().filter(|s| s.down_id >= 0) {
                    out.push(Touch { id: s.down_id, phase: Phase::Up, x: s.x, y: s.y });
                    *s = Slot { down_id: -1, dirty: false, ..*s };
                }
                self.dropping = true;
            }
            (EV_SYN, SYN_REPORT) => {
                for s in self.slots.iter_mut().filter(|s| s.dirty) {
                    s.dirty = false;
                    let (x, y) = (s.x, s.y);
                    match (s.id >= 0, s.down_id >= 0) {
                        (true, false) => out.push(Touch { id: s.id, phase: Phase::Down, x, y }),
                        (true, true) if s.id == s.down_id => out.push(Touch { id: s.id, phase: Phase::Move, x, y }),
                        // A new contact replaced the old one within a single frame.
                        (true, true) => {
                            out.push(Touch { id: s.down_id, phase: Phase::Up, x, y });
                            out.push(Touch { id: s.id, phase: Phase::Down, x, y });
                        }
                        (false, true) => out.push(Touch { id: s.down_id, phase: Phase::Up, x, y }),
                        (false, false) => {}
                    }
                    s.down_id = s.id;
                }
            }
            _ => {}
        }
    }
}

/// Maps raw touch-panel coordinates onto the screen.
#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub swap_xy: bool,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

/// Raw kernel `struct input_event`. On 32-bit ARM the timestamp is two
/// `unsigned long`s even with a 64-bit `time_t` libc, so don't use `libc::input_event`.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawEvent {
    sec: libc::c_ulong,
    usec: libc::c_ulong,
    ty: u16,
    code: u16,
    value: i32,
}

fn abs_info(fd: RawFd, axis: u16) -> io::Result<libc::input_absinfo> {
    // SAFETY: input_absinfo is plain old data; all-zero is valid.
    let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
    let req = ioc(IOC_READ, b'E', 0x40 + axis as u32, size_of::<libc::input_absinfo>());
    // SAFETY: EVIOCGABS writes one input_absinfo into `info`.
    unsafe { ioctl(fd, req, &mut info)? };
    Ok(info)
}

fn has_mt_position(fd: RawFd) -> bool {
    let mut bits = [0u8; 8]; // ABS_MAX (0x3f) + 1 bits
    let req = ioc(IOC_READ, b'E', 0x20 + EV_ABS as u32, bits.len());
    // SAFETY: EVIOCGBIT writes at most `bits.len()` bytes.
    let ok = unsafe { ioctl(fd, req, bits.as_mut_ptr()) }.is_ok();
    ok && bits[(ABS_MT_POSITION_X / 8) as usize] & (1 << (ABS_MT_POSITION_X % 8)) != 0
}

/// An opened touch panel, scaled to screen coordinates.
pub struct TouchPanel {
    file: File,
    decoder: MtDecoder,
    range_x: (i32, i32),
    range_y: (i32, i32),
    screen: (i32, i32),
    transform: Transform,
}

impl TouchPanel {
    /// Finds the first `/dev/input/event*` reporting multitouch positions.
    pub fn find(dir: &Path, screen: (i32, i32), transform: Transform) -> io::Result<Self> {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(file) = File::options().read(true).custom_flags(libc::O_NONBLOCK).open(&path) else {
                continue;
            };
            if !has_mt_position(file.as_raw_fd()) {
                continue;
            }
            let (Ok(ax), Ok(ay)) =
                (abs_info(file.as_raw_fd(), ABS_MT_POSITION_X), abs_info(file.as_raw_fd(), ABS_MT_POSITION_Y))
            else {
                continue;
            };
            return Ok(TouchPanel {
                file,
                decoder: MtDecoder::default(),
                range_x: (ax.minimum, ax.maximum),
                range_y: (ay.minimum, ay.maximum),
                screen,
                transform,
            });
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "no multitouch input device"))
    }

    pub fn fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Drains pending kernel events (non-blocking) into screen-space touches.
    pub fn read(&mut self, out: &mut Vec<Touch>) -> io::Result<()> {
        const N: usize = 64;
        let mut buf = [0u8; N * size_of::<RawEvent>()];
        let start = out.len();
        loop {
            let n = match self.file.read(&mut buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            };
            for chunk in buf[..n].chunks_exact(size_of::<RawEvent>()) {
                // SAFETY: chunk is exactly one RawEvent; read_unaligned has no alignment needs.
                let ev: RawEvent = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
                self.decoder.feed(ev.ty, ev.code, ev.value, out);
            }
            // evdev fills the buffer while events are queued; a short read means drained.
            if n < buf.len() {
                break;
            }
        }
        for t in &mut out[start..] {
            (t.x, t.y) = self.map(t.x, t.y);
        }
        Ok(())
    }

    fn map(&self, x: i32, y: i32) -> (i32, i32) {
        let t = self.transform;
        let (mut x, mut y, rx, ry) =
            if t.swap_xy { (y, x, self.range_y, self.range_x) } else { (x, y, self.range_x, self.range_y) };
        let scale = |v: i32, (lo, hi): (i32, i32), len: i32| {
            ((v - lo) as i64 * (len - 1) as i64 / (hi - lo).max(1) as i64) as i32
        };
        x = scale(x, rx, self.screen.0);
        y = scale(y, ry, self.screen.1);
        if t.mirror_x {
            x = self.screen.0 - 1 - x;
        }
        if t.mirror_y {
            y = self.screen.1 - 1 - y;
        }
        (x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(events: &[(u16, u16, i32)]) -> Vec<Touch> {
        let mut d = MtDecoder::default();
        let mut out = Vec::new();
        for &(t, c, v) in events {
            d.feed(t, c, v, &mut out);
        }
        out
    }

    const SYN: (u16, u16, i32) = (EV_SYN, SYN_REPORT, 0);

    #[test]
    fn single_finger_down_move_up() {
        let out = run(&[
            (EV_ABS, ABS_MT_TRACKING_ID, 7),
            (EV_ABS, ABS_MT_POSITION_X, 10),
            (EV_ABS, ABS_MT_POSITION_Y, 20),
            SYN,
            (EV_ABS, ABS_MT_POSITION_X, 15),
            SYN,
            (EV_ABS, ABS_MT_TRACKING_ID, -1),
            SYN,
        ]);
        assert_eq!(
            out,
            vec![
                Touch { id: 7, phase: Phase::Down, x: 10, y: 20 },
                Touch { id: 7, phase: Phase::Move, x: 15, y: 20 },
                Touch { id: 7, phase: Phase::Up, x: 15, y: 20 },
            ]
        );
    }

    #[test]
    fn two_fingers_on_separate_slots() {
        let out = run(&[
            (EV_ABS, ABS_MT_SLOT, 0),
            (EV_ABS, ABS_MT_TRACKING_ID, 1),
            (EV_ABS, ABS_MT_POSITION_X, 1),
            (EV_ABS, ABS_MT_SLOT, 1),
            (EV_ABS, ABS_MT_TRACKING_ID, 2),
            (EV_ABS, ABS_MT_POSITION_X, 2),
            SYN,
            (EV_ABS, ABS_MT_SLOT, 0),
            (EV_ABS, ABS_MT_TRACKING_ID, -1),
            SYN,
        ]);
        let phases: Vec<_> = out.iter().map(|t| (t.id, t.phase)).collect();
        assert_eq!(phases, vec![(1, Phase::Down), (2, Phase::Down), (1, Phase::Up)]);
    }

    #[test]
    fn contact_replaced_within_one_frame_is_up_then_down() {
        let out = run(&[
            (EV_ABS, ABS_MT_TRACKING_ID, 7),
            SYN,
            (EV_ABS, ABS_MT_TRACKING_ID, -1),
            (EV_ABS, ABS_MT_TRACKING_ID, 8),
            SYN,
        ]);
        let phases: Vec<_> = out.iter().map(|t| (t.id, t.phase)).collect();
        assert_eq!(phases, vec![(7, Phase::Down), (7, Phase::Up), (8, Phase::Down)]);
    }

    #[test]
    fn syn_dropped_releases_contacts_and_skips_to_next_report() {
        let out = run(&[
            (EV_ABS, ABS_MT_TRACKING_ID, 7),
            SYN,
            (EV_SYN, SYN_DROPPED, 0),
            (EV_ABS, ABS_MT_POSITION_X, 99), // discarded
            SYN,
            (EV_ABS, ABS_MT_POSITION_X, 5),
            SYN,
        ]);
        let phases: Vec<_> = out.iter().map(|t| (t.id, t.phase, t.x)).collect();
        assert_eq!(phases, vec![(7, Phase::Down, 0), (7, Phase::Up, 0), (7, Phase::Down, 5)]);
    }

    #[test]
    fn untouched_slots_emit_nothing() {
        assert!(run(&[SYN, SYN]).is_empty());
    }

    #[test]
    fn raw_event_is_16_bytes_on_32bit() {
        #[cfg(target_pointer_width = "32")]
        assert_eq!(size_of::<RawEvent>(), 16);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<RawEvent>(), 24);
    }
}
