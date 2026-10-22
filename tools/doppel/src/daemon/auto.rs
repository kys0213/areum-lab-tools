//! Opt-in automation (docs/doppel.md §6 "자동 처리"). Every decision here is
//! a comparison of measurements with the daemon's arguments — the same
//! measurements and arguments always give the same answer, and nothing here
//! adjusts an argument.

use crate::common::store::{ProposalRecord, ProposalStats};
use crate::daemon::launch::{self, Mode};
use crate::daemon::routes::AppState;

/// When doppel may assign a card as proposed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TriageGate {
    /// Proposals a person has judged so far, at least.
    pub(crate) min_judged: u32,
    /// Share of judged proposals a person took, at least.
    pub(crate) min_acceptance: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct AutoConfig {
    /// `None` leaves every assignment to a person.
    pub(crate) triage: Option<TriageGate>,
    /// Launch triaged cards that were never sent back.
    pub(crate) dispatch: bool,
}

/// A proposal is applied only when the record so far clears the gate, the
/// proposal is sure of itself, and it names a project.
pub(super) fn may_apply(
    gate: &TriageGate,
    stats: &ProposalStats,
    proposal: &ProposalRecord,
) -> bool {
    let acceptance = if stats.judged == 0 {
        0.0
    } else {
        f64::from(stats.accepted) / f64::from(stats.judged)
    };
    proposal.state == "ready"
        && proposal.project.is_some()
        && proposal.confidence.as_deref() == Some("high")
        && stats.judged >= gate.min_judged
        && acceptance >= gate.min_acceptance
}

/// Assigns the card as proposed when the gate allows. The assignment is
/// doppel's (`by: model`), so it does not enter the ledger.
pub(super) fn apply_proposal(app: &AppState, card_id: &str) {
    let Some(gate) = app.auto.triage else {
        return;
    };
    let applied = {
        let mut store = app.store();
        let (Ok(Some(proposal)), Ok(stats)) = (store.get_proposal(card_id), store.proposal_stats())
        else {
            return;
        };
        if !may_apply(&gate, &stats, &proposal) {
            return;
        }
        let (Some(project), Some(priority)) = (&proposal.project, &proposal.priority) else {
            return;
        };
        store
            .triage_card(card_id, project, priority, "model")
            .and_then(|_| store.mark_proposal_applied(card_id))
            .is_ok()
    };
    if applied {
        app.hub.cards_changed();
        pump(app);
    }
}

/// Launches triaged cards that were never sent back — most urgent and oldest
/// first — until the running limit refuses. A card a person released keeps
/// its `last_error` and waits for them.
pub(super) fn pump(app: &AppState) {
    let Some(max_running) = app.launch.as_ref().map(|l| l.max_running) else {
        return;
    };
    if !app.auto.dispatch {
        return;
    }
    let (Ok(waiting), Ok(running)) = ({
        let store = app.store();
        (
            store.list_cards(Some("triaged")),
            store.list_cards(Some("running")),
        )
    }) else {
        return;
    };
    let mut free = (max_running as usize).saturating_sub(running.len());
    for card in waiting.into_iter().filter(|c| c.last_error.is_none()) {
        if free == 0 {
            return;
        }
        // A card that cannot start (its project directory is gone, another
        // dispatcher took it) is skipped; it stays where it is.
        if launch::start(app, &card.id, Mode::Fresh).is_ok() {
            free -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(confidence: &str, project: Option<&str>) -> ProposalRecord {
        ProposalRecord {
            card_id: "crd-000001".into(),
            state: "ready".into(),
            project: project.map(str::to_owned),
            priority: Some("P1".into()),
            confidence: Some(confidence.into()),
            rationale: Some("r".into()),
            cited: vec![],
            precedents: vec![],
            command: "c".into(),
            error: None,
            applied: false,
            accepted: None,
            created_at: String::new(),
            finished_at: None,
        }
    }

    fn stats(judged: u32, accepted: u32) -> ProposalStats {
        ProposalStats {
            judged,
            accepted,
            ..ProposalStats::default()
        }
    }

    const GATE: TriageGate = TriageGate {
        min_judged: 20,
        min_acceptance: 0.9,
    };

    #[test]
    fn the_gate_needs_enough_judged_proposals_and_acceptance() {
        let sure = proposal("high", Some("areum"));
        assert!(may_apply(&GATE, &stats(20, 18), &sure));
        assert!(!may_apply(&GATE, &stats(19, 19), &sure));
        assert!(!may_apply(&GATE, &stats(20, 17), &sure));
    }

    #[test]
    fn only_a_sure_proposal_with_a_project_is_applied() {
        assert!(!may_apply(
            &GATE,
            &stats(50, 50),
            &proposal("medium", Some("areum"))
        ));
        assert!(!may_apply(&GATE, &stats(50, 50), &proposal("high", None)));
    }

    #[test]
    fn a_zero_threshold_still_needs_a_sure_proposal() {
        let open = TriageGate {
            min_judged: 0,
            min_acceptance: 0.0,
        };
        assert!(may_apply(
            &open,
            &stats(0, 0),
            &proposal("high", Some("areum"))
        ));
        assert!(!may_apply(
            &open,
            &stats(0, 0),
            &proposal("low", Some("areum"))
        ));
    }
}
