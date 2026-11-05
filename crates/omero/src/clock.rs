//! Local wall-clock time without a time-zone crate: the UTC offset comes from the TZif file
//! at `/etc/localtime`, the calendar date from the day count.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::{strings, Lang};

/// Local wall-clock time now: (days since 1970-01-01, minutes since midnight).
pub fn local_now() -> (i64, u32) {
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    // The reader header asks on every page turn: re-read the zone file at most every 15
    // minutes (DST switches on the hour, so the offset can be a few minutes late at most).
    static CACHE: std::sync::Mutex<(i64, i64)> = std::sync::Mutex::new((i64::MIN, 0));
    let slot = utc.div_euclid(900);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.0 != slot {
        let offset = std::fs::read("/etc/localtime").ok().and_then(|d| tzif_offset(&d, utc)).unwrap_or(0);
        *cache = (slot, offset);
    }
    let local = utc + cache.1;
    (local.div_euclid(86_400), (local.rem_euclid(86_400) / 60) as u32)
}

/// UTC offset in seconds at `utc` from a TZif zone file (RFC 8536): the type of the last
/// transition at or before `utc`.
// ponytail: ignores the POSIX TZ footer, so times past the last listed transition keep its
// offset. The Kindle's zone files list transitions to 2037; "slim" desktop files stop
// earlier and may be an hour off in summer. Parse the footer rule if that matters.
fn tzif_offset(data: &[u8], utc: i64) -> Option<i64> {
    fn counts(b: &[u8]) -> Option<[usize; 6]> {
        if b.get(..4)? != b"TZif" {
            return None;
        }
        let mut c = [0; 6];
        for (i, v) in c.iter_mut().enumerate() {
            *v = u32::from_be_bytes(b.get(20 + 4 * i..24 + 4 * i)?.try_into().ok()?) as usize;
        }
        Some(c)
    }
    let mut body = data;
    let mut c = counts(body)?;
    let mut size = 4;
    if *data.get(4)? >= b'2' {
        // Version 2+ repeats the data with 64-bit times after the version 1 block.
        let [isut, isstd, leap, times, types, chars] = c;
        let v1 = 44 + times * 5 + types * 6 + chars + leap * 8 + isstd + isut;
        body = data.get(v1..)?;
        c = counts(body)?;
        size = 8;
    }
    let times = c[3];
    let time = |i: usize| -> Option<i64> {
        let b = body.get(44 + i * size..44 + (i + 1) * size)?;
        Some(if size == 8 {
            i64::from_be_bytes(b.try_into().ok()?)
        } else {
            i32::from_be_bytes(b.try_into().ok()?) as i64
        })
    };
    let mut n = 0;
    while n < times && time(n)? <= utc {
        n += 1;
    }
    let indices = 44 + times * size;
    let ty = if n == 0 { 0 } else { *body.get(indices + n - 1)? as usize };
    let info = indices + times + ty * 6;
    Some(i32::from_be_bytes(body.get(info..info + 4)?.try_into().ok()?) as i64)
}

/// "Giovedì 5 novembre" / "Thursday 5 November" for a day count since 1970-01-01.
pub fn date_text(lang: Lang, days: i64) -> String {
    let names = strings(lang);
    // Civil-from-days (Howard Hinnant's algorithm), March-based years.
    let z = days + 719_468;
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 2 } else { mp - 10 } as usize;
    format!("{} {day} {}", names.weekdays[(days + 4).rem_euclid(7) as usize], names.months[month])
}

/// "07:05" for minutes since midnight.
pub fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 1 zone file: UTC+1, UTC+2 from t=1000, back to UTC+1 from t=2000.
    fn zone() -> Vec<u8> {
        let mut z = b"TZif".to_vec();
        z.extend([0; 16]);
        for n in [0u32, 0, 0, 2, 2, 4] {
            z.extend(n.to_be_bytes());
        }
        z.extend(1000i32.to_be_bytes());
        z.extend(2000i32.to_be_bytes());
        z.extend([1, 0]);
        z.extend(3600i32.to_be_bytes());
        z.extend([0, 0]);
        z.extend(7200i32.to_be_bytes());
        z.extend([1, 0]);
        z.extend(b"CET\0");
        z
    }

    #[test]
    fn tzif_offset_follows_transitions() {
        let z = zone();
        assert_eq!(tzif_offset(&z, 0), Some(3600));
        assert_eq!(tzif_offset(&z, 1000), Some(7200));
        assert_eq!(tzif_offset(&z, 1999), Some(7200));
        assert_eq!(tzif_offset(&z, 5000), Some(3600));
        assert_eq!(tzif_offset(&z[..30], 0), None);
        assert_eq!(tzif_offset(b"nope", 0), None);
    }

    #[test]
    fn date_text_names_the_day() {
        assert_eq!(date_text(Lang::En, 0), "Thursday 1 January");
        assert_eq!(date_text(Lang::En, 20_762), "Thursday 5 November"); // 2026
        assert_eq!(date_text(Lang::En, 19_782), "Thursday 29 February"); // 2024
        assert_eq!(date_text(Lang::En, 11_017), "Wednesday 1 March"); // 2000
        assert_eq!(date_text(Lang::It, 20_762), "Giovedì 5 novembre");
    }

    #[test]
    fn hhmm_pads() {
        assert_eq!(hhmm(7 * 60 + 5), "07:05");
        assert_eq!(hhmm(23 * 60 + 59), "23:59");
    }
}
