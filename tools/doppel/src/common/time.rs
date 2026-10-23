//! Canonical UTC timestamps without a date/time crate. Every time column is
//! TEXT in `YYYY-MM-DDTHH:MM:SSZ`, which compares lexicographically as time
//! order — `decision list` and `search` order by `decided_at` relying on it.
//!
//! Source timestamps (transcript millisecond RFC3339, plan `YYYY-MM-DD`
//! dates) are normalized into the same shape here so every row sorts on one
//! scale.

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock "now" as a canonical UTC RFC3339 string (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn now_rfc3339() -> String {
    format_epoch_secs(now_epoch_secs())
}

/// A pre-epoch system clock is not a state the ledger can represent: clamping
/// it would stamp `1970-01-01T00:00:00Z` into `created_at` and silently place
/// the row ahead of every real entry in time-ordered listings.
pub(crate) fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
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
/// a date crate for the one conversion the ledger needs.
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

/// Seconds since the Unix epoch of a canonical timestamp — the inverse of
/// [`format_epoch_secs`], for the session-state thresholds that compare two
/// stored instants. `None` for anything not in the canonical shape.
pub(crate) fn epoch_from_canonical(raw: &str) -> Option<u64> {
    if raw.len() != 20 || !raw.ends_with('Z') || !has_datetime_shape(&raw[..19]) {
        return None;
    }
    let field = |range: std::ops::Range<usize>| raw[range].parse::<i64>().ok();
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    let days = days_from_civil(year, month, day);
    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

/// Hinnant's `days_from_civil`, the inverse of [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Normalizes a transcript timestamp (`2026-10-21T14:47:37.935Z`, with or
/// without fractional seconds) to the canonical shape. `None` for anything
/// that is not a UTC RFC3339 instant — the caller counts it as unreadable
/// rather than guessing an offset.
pub(crate) fn canonical_from_rfc3339(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    if bytes.len() < 20 || !raw.ends_with('Z') {
        return None;
    }
    let head = &raw[..19];
    let tail = &raw[19..raw.len() - 1];
    let fraction_ok = tail.is_empty()
        || (tail.starts_with('.')
            && tail.len() > 1
            && tail[1..].bytes().all(|b| b.is_ascii_digit()));
    (has_datetime_shape(head) && fraction_ok).then(|| format!("{head}Z"))
}

/// Turns a plan header date (`YYYY-MM-DD`) into midnight UTC of that day.
pub(crate) fn canonical_from_date(raw: &str) -> Option<String> {
    let probe = format!("{raw}T00:00:00");
    (raw.len() == 10 && has_datetime_shape(&probe)).then(|| format!("{probe}Z"))
}

/// `YYYY-MM-DDTHH:MM:SS` with digits where digits belong.
fn has_datetime_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        })
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
        // Listing orders by decided_at as TEXT, so lexicographic order must
        // equal chronological order across a year boundary.
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
    #[test]
    fn epoch_round_trips_through_the_canonical_shape() {
        for secs in [0, 1_700_000_000, 1_709_164_800, 4_102_444_799] {
            assert_eq!(epoch_from_canonical(&format_epoch_secs(secs)), Some(secs));
        }
        assert_eq!(epoch_from_canonical("2026-10-21T14:47:37.9Z"), None);
        assert_eq!(epoch_from_canonical("garbage"), None);
    }

    #[test]
    fn transcript_timestamps_drop_fractional_seconds() {
        assert_eq!(
            canonical_from_rfc3339("2026-10-21T14:47:37.935Z").as_deref(),
            Some("2026-10-21T14:47:37Z")
        );
        assert_eq!(
            canonical_from_rfc3339("2026-10-21T14:47:37Z").as_deref(),
            Some("2026-10-21T14:47:37Z")
        );
    }

    #[test]
    fn non_utc_or_malformed_timestamps_are_rejected() {
        assert_eq!(canonical_from_rfc3339("2026-10-21T14:47:37+09:00"), None);
        assert_eq!(canonical_from_rfc3339("2026-10-21 14:47:37Z"), None);
        assert_eq!(canonical_from_rfc3339("2026-10-21T14:47:37.Z"), None);
        assert_eq!(canonical_from_rfc3339(""), None);
    }

    #[test]
    fn plan_dates_become_midnight_utc() {
        assert_eq!(
            canonical_from_date("2026-10-21").as_deref(),
            Some("2026-10-21T00:00:00Z")
        );
        assert_eq!(canonical_from_date("2026-10-2"), None);
        assert_eq!(canonical_from_date("2026/10/21"), None);
    }
}
