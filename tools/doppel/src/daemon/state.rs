//! Session state derivation (docs/doppel.md §5). Pure: the same facts, the
//! same open asks, the same clock and the same thresholds always give the
//! same state. The thresholds are the daemon's arguments, never adjusted
//! here.

use crate::common::store::SessionFacts;
use crate::common::time::epoch_from_canonical;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Thresholds {
    /// A running turn with no event for this long is `stalled`.
    pub(crate) stall_after_secs: u64,
    /// This many tool failures in one turn is `failing`.
    pub(crate) fail_streak: u32,
    /// No sign of life for this long is `disconnected`.
    pub(crate) heartbeat_timeout_secs: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            stall_after_secs: 600,
            fail_streak: 3,
            heartbeat_timeout_secs: 90,
        }
    }
}

/// The first matching state wins: an ended session is ended whatever else
/// is true, a silent one is disconnected before it is anything else, and a
/// waiting user outranks how the turn is going.
pub(crate) fn derive_state(facts: &SessionFacts, now_secs: u64, t: &Thresholds) -> &'static str {
    let age =
        |at: &str| epoch_from_canonical(at).map_or(u64::MAX, |then| now_secs.saturating_sub(then));
    if facts.ended_at.is_some() {
        return "ended";
    }
    if age(&facts.last_heartbeat_at).min(age(&facts.last_event_at)) > t.heartbeat_timeout_secs {
        return "disconnected";
    }
    if facts.open_permissions > 0 {
        return "waiting_permission";
    }
    if facts.open_asks > 0 {
        return "waiting_ask";
    }
    if !facts.turn_active {
        return "waiting_turn";
    }
    if facts.fail_streak >= t.fail_streak {
        return "failing";
    }
    if age(&facts.last_event_at) > t.stall_after_secs {
        return "stalled";
    }
    "running"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::time::epoch_from_canonical;

    const NOW: &str = "2026-10-21T12:00:00Z";

    fn facts() -> SessionFacts {
        SessionFacts {
            session_id: "sess-1".into(),
            cwd: Some("/work".into()),
            surface: "terminal".into(),
            launched_by: "user".into(),
            started_at: "2026-10-21T11:00:00Z".into(),
            last_event_at: "2026-10-21T11:59:30Z".into(),
            last_heartbeat_at: "2026-10-21T11:59:50Z".into(),
            turn_active: true,
            fail_streak: 0,
            last_summary: None,
            ended_at: None,
            end_reason: None,
            open_asks: 0,
            open_permissions: 0,
        }
    }

    fn state(f: &SessionFacts) -> &'static str {
        derive_state(
            f,
            epoch_from_canonical(NOW).unwrap(),
            &Thresholds::default(),
        )
    }

    #[test]
    fn an_active_turn_with_recent_events_is_running() {
        assert_eq!(state(&facts()), "running");
    }

    #[test]
    fn a_finished_turn_waits_for_the_user() {
        let f = SessionFacts {
            turn_active: false,
            ..facts()
        };
        assert_eq!(state(&f), "waiting_turn");
    }

    #[test]
    fn open_permissions_outrank_open_questions() {
        let f = SessionFacts {
            open_asks: 1,
            open_permissions: 1,
            ..facts()
        };
        assert_eq!(state(&f), "waiting_permission");
        let f = SessionFacts {
            open_asks: 1,
            ..facts()
        };
        assert_eq!(state(&f), "waiting_ask");
    }

    #[test]
    fn a_quiet_running_turn_stalls_after_the_threshold() {
        let f = SessionFacts {
            last_event_at: "2026-10-21T11:49:59Z".into(),
            ..facts()
        };
        assert_eq!(state(&f), "stalled");
        let f = SessionFacts {
            last_event_at: "2026-10-21T11:50:00Z".into(),
            ..facts()
        };
        assert_eq!(state(&f), "running");
    }

    #[test]
    fn repeated_failures_mark_the_turn_failing() {
        let f = SessionFacts {
            fail_streak: 3,
            ..facts()
        };
        assert_eq!(state(&f), "failing");
    }

    #[test]
    fn silence_beyond_the_heartbeat_timeout_is_disconnected() {
        let f = SessionFacts {
            last_event_at: "2026-10-21T11:50:00Z".into(),
            last_heartbeat_at: "2026-10-21T11:58:29Z".into(),
            open_asks: 1,
            ..facts()
        };
        assert_eq!(state(&f), "disconnected");
    }

    #[test]
    fn ended_wins_over_everything() {
        let f = SessionFacts {
            ended_at: Some("2026-10-21T11:59:59Z".into()),
            turn_active: false,
            last_heartbeat_at: "2026-10-01T00:00:00Z".into(),
            ..facts()
        };
        assert_eq!(state(&f), "ended");
    }

    #[test]
    fn thresholds_are_arguments() {
        let f = SessionFacts {
            fail_streak: 1,
            ..facts()
        };
        let strict = Thresholds {
            fail_streak: 1,
            ..Thresholds::default()
        };
        assert_eq!(
            derive_state(&f, epoch_from_canonical(NOW).unwrap(), &strict),
            "failing"
        );
    }
}
