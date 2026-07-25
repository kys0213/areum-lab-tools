//! Canonical UTC timestamp generation without a date/time crate. The board's
//! `created_at`/`updated_at`/`claimed_at` columns are TEXT, and canonical
//! `YYYY-MM-DDTHH:MM:SSZ` strings compare lexicographically as time order —
//! which is what `ORDER BY created_at ASC` in the atomic claim relies on.
//!
//! The clock has no caller yet: the store that writes these columns lands with
//! the command bodies, so nothing outside the tests reaches it in this stage.
#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock "now" as a canonical UTC RFC3339 string (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn now_rfc3339() -> String {
    format_epoch_secs(now_epoch_secs())
}

/// A system clock set before 1970 is not a real state we support; clamping to
/// the epoch keeps every caller monotone-parseable rather than panicking here.
fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Formats seconds-since-Unix-epoch as `YYYY-MM-DDTHH:MM:SSZ`. Pure, so the
/// civil-date conversion is testable against known instants without a clock.
fn format_epoch_secs(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let time_of_day = secs % 86_400;
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts a day count since 1970-01-01 into a `(year, month, day)` civil
/// date using Howard Hinnant's `civil_from_days` algorithm — avoids pulling in
/// a date crate for the one conversion the board needs.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_unix_zero() {
        assert_eq!(format_epoch_secs(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_instant_formats_to_utc() {
        // 1_700_000_000 == 2023-11-14T22:13:20Z (independently verifiable).
        assert_eq!(format_epoch_secs(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn leap_day_is_handled() {
        // 2024-02-29T00:00:00Z == 1_709_164_800.
        assert_eq!(format_epoch_secs(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn canonical_strings_sort_in_time_order() {
        // The atomic claim orders by created_at as TEXT, so lexicographic
        // order must equal chronological order across a year boundary.
        let earlier = format_epoch_secs(1_700_000_000);
        let later = format_epoch_secs(1_709_164_800);
        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn now_has_canonical_shape() {
        let now = now_rfc3339();
        assert_eq!(now.len(), 20, "expected fixed-width RFC3339 UTC: {now}");
        assert!(now.ends_with('Z'));
        assert_eq!(now.as_bytes()[10], b'T');
    }
}
