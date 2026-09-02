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
}
