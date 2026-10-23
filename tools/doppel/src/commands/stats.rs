use std::path::Path;

use crate::common::store::Store;
use crate::output::{AppError, CategoryStatsData, Payload, ProposalStatsData, StatsData};

pub(crate) fn run_stats(db_path: &Path) -> Result<Payload, AppError> {
    let store = Store::open(db_path)?;
    let stats = store.brief_stats()?;
    let proposals = store.proposal_stats()?;
    Ok(Payload::Stats(StatsData {
        briefs: stats.briefs,
        failed: stats.failed,
        new_opinions: stats.new_opinions,
        judged: stats.judged,
        accepted: stats.accepted,
        acceptance: rate(stats.accepted, stats.judged),
        by_category: stats
            .by_category
            .into_iter()
            .map(|c| CategoryStatsData {
                acceptance: rate(c.accepted, c.judged),
                category: c.category,
                judged: c.judged,
                accepted: c.accepted,
            })
            .collect(),
        proposals: ProposalStatsData {
            proposals: proposals.proposals,
            failed: proposals.failed,
            applied: proposals.applied,
            judged: proposals.judged,
            accepted: proposals.accepted,
            acceptance: rate(proposals.accepted, proposals.judged),
        },
    }))
}

/// `None` with nothing judged yet — 0% would read as "never right".
fn rate(accepted: u32, judged: u32) -> Option<f64> {
    (judged > 0).then(|| f64::from(accepted) / f64::from(judged))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::TempDir;

    #[test]
    fn an_empty_ledger_has_no_rate() {
        let dir = TempDir::new("stats-empty");
        match run_stats(&dir.db_path()).unwrap() {
            Payload::Stats(data) => {
                assert_eq!(data.judged, 0);
                assert_eq!(data.acceptance, None);
                assert!(data.by_category.is_empty());
            }
            other => panic!("expected Stats, got {other:?}"),
        }
    }

    #[test]
    fn rates_are_fractions_of_judged_briefs() {
        assert_eq!(rate(3, 4), Some(0.75));
        assert_eq!(rate(0, 0), None);
    }
}
