//! doppel's `--json` wire contract, pinned with its own payloads. The
//! envelope shape lives in `cli_contract`; these literal-string tests guard
//! that doppel's output stays byte-identical through it.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::common::error::ErrorKind;
    use crate::output::payload::{CategorizeData, IngestData, InitData, Payload};
    use crate::output::{AppError, Sink, render};

    #[test]
    fn init_success_matches_contract() {
        let payload = Payload::Init(InitData {
            path: "/home/user/.areum/doppel/doppel.db".into(),
            created: true,
            seeded_categories: vec!["design-scope".into()],
        });
        let (sink, json, code) = render("init", &Ok(payload), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 0);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"init","data":{"path":"/home/user/.areum/doppel/doppel.db","created":true,"seeded_categories":["design-scope"]}}"#
        );
    }

    #[test]
    fn ingest_success_matches_contract() {
        let mut skipped = BTreeMap::new();
        skipped.insert("invalid_json".to_owned(), 2);
        let payload = Payload::Ingest(IngestData {
            root: "/home/user/.claude/projects".into(),
            files: 3,
            added: 5,
            skipped_existing: 1,
            skipped,
        });
        let (_, json, _) = render("ingest sessions", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"ingest sessions","data":{"root":"/home/user/.claude/projects","files":3,"added":5,"skipped_existing":1,"skipped":{"invalid_json":2}}}"#
        );
    }

    #[test]
    fn categorize_success_matches_contract() {
        let payload = Payload::DecisionCategorize(CategorizeData {
            id: "dec-000001".into(),
            category: "design-scope".into(),
            category_by: "human".into(),
            previous_category: None,
        });
        let (_, json, _) = render("decision categorize", &Ok(payload), true);
        assert_eq!(
            json,
            r#"{"ok":true,"command":"decision categorize","data":{"id":"dec-000001","category":"design-scope","category_by":"human","previous_category":null}}"#
        );
    }

    #[test]
    fn error_matches_contract() {
        let err = AppError::new(ErrorKind::Conflict, "golden gld-000001 is already retired");
        let (sink, json, code) = render("golden retire", &Err(err), true);
        assert_eq!(sink, Sink::Stdout);
        assert_eq!(code, 8);
        assert_eq!(
            json,
            r#"{"ok":false,"command":"golden retire","error":{"kind":"conflict","message":"golden gld-000001 is already retired"}}"#
        );
    }
}
