//! Store records → payload types, shared by every command that answers a
//! decision or a golden entry.

use crate::common::store::{CategoryRecord, ChoiceOption, DecisionRecord, GoldenRecord};
use crate::output::{CategoryData, DecisionData, GoldenData, OptionData};

pub(super) fn option_data(options: Vec<ChoiceOption>) -> Vec<OptionData> {
    options
        .into_iter()
        .map(|o| OptionData {
            label: o.label,
            description: o.description,
        })
        .collect()
}

pub(super) fn decision_data(d: DecisionRecord) -> DecisionData {
    DecisionData {
        id: d.id,
        source: d.source,
        external_id: d.external_id,
        kind: d.kind,
        project: d.project,
        session_id: d.session_id,
        question: d.question,
        options: option_data(d.options),
        answers: d.answers,
        rationale: d.rationale,
        context: d.context,
        category: d.category,
        category_by: d.category_by,
        decided_at: d.decided_at,
        created_at: d.created_at,
    }
}

pub(super) fn golden_data(g: GoldenRecord) -> GoldenData {
    GoldenData {
        id: g.id,
        decision_id: g.decision_id,
        category: g.category,
        project: g.project,
        question: g.question,
        options: option_data(g.options),
        answers: g.answers,
        rationale: g.rationale,
        context: g.context,
        decided_at: g.decided_at,
        state: g.state,
        reason: g.reason,
        superseded_by: g.superseded_by,
        note: g.note,
        created_at: g.created_at,
        reviewed_at: g.reviewed_at,
    }
}

pub(super) fn category_data(c: CategoryRecord) -> CategoryData {
    CategoryData {
        name: c.name,
        description: c.description,
        state: c.state,
        created_at: c.created_at,
    }
}
