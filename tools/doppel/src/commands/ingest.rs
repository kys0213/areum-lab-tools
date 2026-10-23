use std::path::{Path, PathBuf};

use crate::common::error::ErrorKind;
use crate::common::store::{NewDecision, Store};
use crate::ingest::plan_doc::parse_plan;
use crate::ingest::tally::SkipTally;
use crate::ingest::transcript::parse_transcript;
use crate::output::{AppError, IngestData, Payload};

/// Reads every `*.jsonl` under `root` (Claude Code keeps one per session,
/// grouped by project directory). Files are read in path order so ids are
/// minted in the same order on every machine.
pub(crate) fn run_ingest_sessions(db_path: &Path, root: &Path) -> Result<Payload, AppError> {
    require_dir(root, "transcript root")?;
    let mut skipped = SkipTally::default();
    let files = collect_files(root, "jsonl", &mut skipped);
    let mut batch = Vec::new();
    for file in &files {
        match std::fs::read_to_string(file) {
            Ok(text) => {
                let scan = parse_transcript(&text);
                batch.extend(scan.decisions);
                skipped.merge(scan.skipped);
            }
            Err(_) => skipped.add("unreadable_file"),
        }
    }
    store_batch(db_path, root, files.len(), &batch, skipped)
}

/// Reads every `*.md` under `<repo>/plans`. The repository's directory name
/// is the project; it and the repo-relative path are the dedup key, so the
/// same plan ingested from two clones of one repo is one entry.
pub(crate) fn run_ingest_plans(db_path: &Path, repo: &Path) -> Result<Payload, AppError> {
    let plans_dir = repo.join("plans");
    require_dir(&plans_dir, "plans directory")?;
    let repo_name = repo_name(repo)?;
    let mut skipped = SkipTally::default();
    let files = collect_files(&plans_dir, "md", &mut skipped);
    let mut batch = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            skipped.add("unreadable_file");
            continue;
        };
        match parse_plan(&text, &repo_name, &relative_path(repo, file)) {
            Ok(decision) => batch.push(decision),
            Err(skip) => skipped.add(skip.reason()),
        }
    }
    store_batch(db_path, &plans_dir, files.len(), &batch, skipped)
}

fn store_batch(
    db_path: &Path,
    root: &Path,
    files: usize,
    batch: &[NewDecision],
    skipped: SkipTally,
) -> Result<Payload, AppError> {
    let mut store = Store::open(db_path)?;
    let outcome = store.insert_decisions(batch)?;
    Ok(Payload::Ingest(IngestData {
        root: root.display().to_string(),
        files,
        added: outcome.added,
        skipped_existing: outcome.existing,
        skipped: skipped.into_map(),
    }))
}

/// A missing root is a setup problem, not "nothing to ingest" — answering
/// zero files would hide a typo in `--root`.
fn require_dir(dir: &Path, what: &str) -> Result<(), AppError> {
    if dir.is_dir() {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::NotFound,
            format!("{what} {} does not exist", dir.display()),
        ))
    }
}

fn repo_name(repo: &Path) -> Result<String, AppError> {
    let resolved = repo.canonicalize().map_err(|e| {
        AppError::new(
            ErrorKind::NotFound,
            format!("cannot resolve repository {}: {e}", repo.display()),
        )
    })?;
    resolved
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            AppError::new(
                ErrorKind::Usage,
                format!("repository path {} has no directory name", repo.display()),
            )
        })
}

/// `/`-separated so the dedup key is the same on every OS.
fn relative_path(repo: &Path, file: &Path) -> String {
    file.strip_prefix(repo)
        .unwrap_or(file)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Files with `extension` under `dir`, recursively, in path order. A
/// subdirectory that cannot be listed is counted and skipped rather than
/// aborting the whole ingest.
fn collect_files(dir: &Path, extension: &str, skipped: &mut SkipTally) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            skipped.add("unreadable_dir");
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testutil::{TempDir, seed_store};
    use crate::common::store::DecisionFilter;

    const ASK_SESSION: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"배경"},{"type":"tool_use","id":"toolu_1","name":"AskUserQuestion","input":{"questions":[{"question":"어디에 둘까요?","options":[{"label":"A","description":"a"}]}]}}]}}
{"type":"user","sessionId":"sess-1","uuid":"u-1","cwd":"/work","timestamp":"2026-10-21T01:02:03.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1"}]},"toolUseResult":{"answers":{"어디에 둘까요?":"A"}}}
{broken"#;

    const PLAN: &str = "# 캐시 결정\n\n> **Plan — 기록.**\n> 날짜: 2026-10-01 · 승인 출처: 사용자 승인\n\n## 결정\n\n1. 디스크.\n";

    fn ingest_data(payload: Payload) -> IngestData {
        match payload {
            Payload::Ingest(data) => data,
            other => panic!("expected Payload::Ingest, got {other:?}"),
        }
    }

    fn all_decisions(db: &Path) -> Vec<crate::common::store::DecisionRecord> {
        seed_store(db)
            .list_decisions(&DecisionFilter {
                limit: 100,
                ..DecisionFilter::default()
            })
            .unwrap()
    }

    #[test]
    fn sessions_are_read_recursively_and_reingest_is_idempotent() {
        let dir = TempDir::new("ingest-sessions");
        dir.write("projects/-work/sess-1.jsonl", ASK_SESSION);
        dir.write("projects/-work/notes.txt", "ignored");
        let root = dir.path().join("projects");

        let first = ingest_data(run_ingest_sessions(&dir.db_path(), &root).unwrap());
        assert_eq!(first.files, 1);
        assert_eq!(first.added, 1);
        assert_eq!(first.skipped.get("invalid_json"), Some(&1));

        let second = ingest_data(run_ingest_sessions(&dir.db_path(), &root).unwrap());
        assert_eq!(second.added, 0);
        assert_eq!(second.skipped_existing, 1);

        let decisions = all_decisions(&dir.db_path());
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].question, "어디에 둘까요?");
        assert_eq!(decisions[0].external_id, "sess-1:toolu_1:0");
    }

    #[test]
    fn a_missing_transcript_root_is_not_found() {
        let dir = TempDir::new("ingest-missing-root");
        let err = run_ingest_sessions(&dir.db_path(), &dir.path().join("nope")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
    }

    #[test]
    fn plans_are_keyed_by_repo_name_and_relative_path() {
        let dir = TempDir::new("ingest-plans");
        dir.write("my-repo/plans/2026-10-01-cache.md", PLAN);
        dir.write("my-repo/plans/README.md", "# plans\n\n설명");
        let repo = dir.path().join("my-repo");

        let data = ingest_data(run_ingest_plans(&dir.db_path(), &repo).unwrap());
        assert_eq!(data.files, 2);
        assert_eq!(data.added, 1);
        assert_eq!(data.skipped.get("not_a_plan"), Some(&1));

        let decisions = all_decisions(&dir.db_path());
        assert_eq!(
            decisions[0].external_id,
            "my-repo/plans/2026-10-01-cache.md"
        );
        assert_eq!(decisions[0].project.as_deref(), Some("my-repo"));
    }

    #[test]
    fn a_repo_without_plans_is_not_found() {
        let dir = TempDir::new("ingest-no-plans");
        std::fs::create_dir_all(dir.path().join("repo")).unwrap();
        let err = run_ingest_plans(&dir.db_path(), &dir.path().join("repo")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
    }
}
