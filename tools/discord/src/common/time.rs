//! Deadlines in the canonical UTC shape that `cli_contract::time` produces.
//! The store re-normalizes every timestamp through SQLite `datetime()` at its
//! write boundary, so this only needs to emit a form SQLite can parse.

use cli_contract::time::{format_epoch_secs, now_epoch_secs};

/// `now` shifted forward by `secs`, in the same canonical UTC shape as
/// `cli_contract::time::now_rfc3339` — used to compute an ask's `timeout_at`
/// from its `--timeout` duration. Saturates instead of overflowing on a
/// pathologically large `secs`.
pub(crate) fn rfc3339_after_secs(secs: u64) -> String {
    format_epoch_secs(now_epoch_secs().saturating_add(secs))
}

#[cfg(test)]
mod tests {
    use cli_contract::time::now_rfc3339;

    use super::*;

    #[test]
    fn rfc3339_after_secs_is_later_than_now() {
        // Canonical UTC strings compare lexicographically as time order, so a
        // positive offset must sort strictly after `now`.
        let now = now_rfc3339();
        let later = rfc3339_after_secs(3600);
        assert!(later > now, "{later} should be after {now}");
    }

    #[test]
    fn rfc3339_after_secs_saturates_instead_of_overflowing() {
        // Must not panic under debug overflow checks on a pathological input.
        let _ = rfc3339_after_secs(u64::MAX);
    }
}
