// SPDX-License-Identifier: AGPL-3.0-only

//! Timestamps as the registry writes them (§4.1): ISO 8601 in UTC, to the
//! second, `2026-09-02T14:03:07Z`. No dependency, no time zone, no locale.

use std::time::{SystemTime, UNIX_EPOCH};

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Now, as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso() -> String {
    iso_of(unix_secs())
}

/// Now, as seconds since the Unix epoch.
pub fn now_secs() -> u64 {
    unix_secs()
}

/// A `YYYY-MM-DDTHH:MM:SSZ` stamp back to seconds since the Unix epoch; none
/// for anything else.
pub fn secs_of(iso: &str) -> Option<u64> {
    let b = iso.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let num = |from: usize, to: usize| iso[from..to].parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m as u32, d as u32);
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A proleptic Gregorian date to days since 1970-01-01 (the inverse of
/// [`civil_from_days`]).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Today, as `YYYY-MM-DD`.
pub fn today() -> String {
    let (y, m, d) = civil_from_days((unix_secs() / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A Unix time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso_of(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_come_out_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_698), (2026, 9, 2));
        assert_eq!(iso_of(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_of(20_698 * 86_400 + 50_587), "2026-09-02T14:03:07Z");
        assert_eq!(today().len(), 10);
        assert_eq!(now_iso().len(), 20);
    }

    #[test]
    fn stamps_parse_back() {
        for secs in [
            0u64,
            86_399,
            951_782_400,
            20_698 * 86_400 + 50_587,
            4_102_444_800,
        ] {
            assert_eq!(secs_of(&iso_of(secs)), Some(secs), "{secs}");
        }
        assert_eq!(
            secs_of("2026-09-02T14:03:07Z"),
            Some(20_698 * 86_400 + 50_587)
        );
        assert_eq!(secs_of("2026-09-02 14:03:07"), None);
        assert_eq!(secs_of("2026-13-02T14:03:07Z"), None);
        assert_eq!(secs_of(""), None);
        for days in [-1_000_000, -1, 0, 1, 19_782, 20_698, 3_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{days}");
        }
    }
}
