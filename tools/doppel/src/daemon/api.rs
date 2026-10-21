//! The JSON shapes of the daemon API (docs/doppel-protocol.md §6). The CLI
//! reads these back, so the field names are the contract on both sides.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::store::{AskRecord, BriefRecord, SessionFacts};
use crate::daemon::state::{Thresholds, derive_state};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AskView {
    pub(crate) ask_id: String,
    pub(crate) session_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) kind: String,
    pub(crate) questions: Value,
    pub(crate) state: String,
    pub(crate) answers: Option<Value>,
    pub(crate) answered_by: Option<String>,
    pub(crate) created_at: String,
    pub(crate) answered_at: Option<String>,
    pub(crate) closed_at: Option<String>,
    /// `null` when the daemon writes no briefs.
    #[serde(default)]
    pub(crate) brief: Option<BriefView>,
}

/// The brief as the API shows it (docs/doppel-judgment.md §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BriefView {
    pub(crate) state: String,
    pub(crate) background: Option<String>,
    pub(crate) recommendation: Option<Value>,
    pub(crate) new_opinion: Option<String>,
    pub(crate) confidence: Option<String>,
    pub(crate) reversible: Option<bool>,
    pub(crate) rationale: Option<String>,
    pub(crate) cited: Vec<String>,
    pub(crate) error: Option<String>,
    pub(crate) accepted: Option<bool>,
}

impl From<BriefRecord> for BriefView {
    fn from(b: BriefRecord) -> Self {
        BriefView {
            state: b.state,
            background: b.background,
            recommendation: b.recommendation,
            new_opinion: b.new_opinion,
            confidence: b.confidence,
            reversible: b.reversible,
            rationale: b.rationale,
            cited: b.cited,
            error: b.error,
            accepted: b.accepted,
        }
    }
}

impl AskView {
    pub(crate) fn with_brief(ask: AskRecord, brief: Option<BriefRecord>) -> Self {
        AskView {
            brief: brief.map(BriefView::from),
            ..AskView::from(ask)
        }
    }
}

impl From<AskRecord> for AskView {
    fn from(a: AskRecord) -> Self {
        AskView {
            ask_id: a.id,
            session_id: a.session_id,
            tool_use_id: a.tool_use_id,
            kind: a.kind,
            questions: a.questions,
            state: a.state,
            answers: a.answers,
            answered_by: a.answered_by,
            created_at: a.created_at,
            answered_at: a.answered_at,
            closed_at: a.closed_at,
            brief: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionView {
    pub(crate) session_id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) surface: String,
    pub(crate) launched_by: String,
    pub(crate) state: String,
    pub(crate) started_at: String,
    pub(crate) last_event_at: String,
    pub(crate) ended_at: Option<String>,
    pub(crate) last_summary: Option<String>,
    pub(crate) open_asks: u32,
}

impl SessionView {
    pub(crate) fn derive(facts: SessionFacts, now_secs: u64, thresholds: &Thresholds) -> Self {
        let state = derive_state(&facts, now_secs, thresholds).to_owned();
        SessionView {
            open_asks: facts.open_asks + facts.open_permissions,
            session_id: facts.session_id,
            cwd: facts.cwd,
            surface: facts.surface,
            launched_by: facts.launched_by,
            state,
            started_at: facts.started_at,
            last_event_at: facts.last_event_at,
            ended_at: facts.ended_at,
            last_summary: facts.last_summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SessionList {
    pub(crate) sessions: Vec<SessionView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct AskList {
    pub(crate) asks: Vec<AskView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Health {
    pub(crate) ok: bool,
    pub(crate) version: String,
    pub(crate) pid: u32,
}

/// A session event as the plugin posts it. Kind-specific fields are optional
/// here and checked per kind when the event is applied.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EventBody {
    pub(crate) session_id: String,
    pub(crate) kind: String,
    pub(crate) cwd: Option<String>,
    pub(crate) surface: Option<String>,
    pub(crate) launched_by: Option<String>,
    pub(crate) reason: Option<String>,
    pub(crate) summary: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OpenAskBody {
    pub(crate) session_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) kind: String,
    pub(crate) questions: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AnswerBody {
    pub(crate) answers: Value,
    pub(crate) answered_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CloseBody {
    pub(crate) answers: Option<Value>,
    pub(crate) answered_by: String,
}

/// doppel → session messages carried by the attach connection
/// (docs/doppel-protocol.md §2).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AttachMessage {
    Answer {
        ask_id: String,
        tool_use_id: String,
        answers: Value,
        /// The surface that answered, for the hook's `close`.
        answered_by: String,
    },
}

/// Pushed to the web UI: something it lists changed, re-fetch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct UiNotice {
    pub(crate) kind: &'static str,
    pub(crate) what: &'static str,
}

impl UiNotice {
    pub(crate) const SESSIONS: UiNotice = UiNotice {
        kind: "changed",
        what: "sessions",
    };
    pub(crate) const ASKS: UiNotice = UiNotice {
        kind: "changed",
        what: "asks",
    };
}
