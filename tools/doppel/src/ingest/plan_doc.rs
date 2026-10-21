//! `plans/*.md` decision records → ledger entries
//! (docs/doppel-judgment.md §1). Only files with the plan header
//! (`> **Plan —`) are read; the header's date and approval source, the
//! decision section and the rejected options are what the user decided.

use crate::common::store::{ChoiceOption, NewDecision};
use crate::common::time::canonical_from_date;
use crate::ingest::redact::redact;
use crate::ingest::text::head_chars;

/// How much of the decision section is kept as the rationale. Its opening
/// carries the decisions; later parts are details.
const RATIONALE_CHARS: usize = 4_000;

const PLAN_MARKER: &str = "> **Plan —";

/// Why a markdown file under `plans/` did not become an entry. The command
/// layer counts these by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanSkip {
    NotAPlan,
    MissingTitle,
    MissingDate,
    MissingApproval,
}

impl PlanSkip {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            PlanSkip::NotAPlan => "not_a_plan",
            PlanSkip::MissingTitle => "missing_title",
            PlanSkip::MissingDate => "missing_date",
            PlanSkip::MissingApproval => "missing_approval",
        }
    }
}

/// `repo` names the project; `relative_path` (repo-relative, `/`-separated)
/// with it is the dedup key.
pub(crate) fn parse_plan(
    text: &str,
    repo: &str,
    relative_path: &str,
) -> Result<NewDecision, PlanSkip> {
    let header: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with(PLAN_MARKER))
        .take_while(|l| l.starts_with('>'))
        .collect();
    if header.is_empty() {
        return Err(PlanSkip::NotAPlan);
    }
    let title = text
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(PlanSkip::MissingTitle)?;
    let decided_at = header_field(&header, "날짜:")
        .and_then(|date| canonical_from_date(&date))
        .ok_or(PlanSkip::MissingDate)?;
    let approval = header_field(&header, "승인 출처:").ok_or(PlanSkip::MissingApproval)?;

    let rationale = section(text, &["결정"])
        .map(|body| redact(&head_chars(body.trim(), RATIONALE_CHARS)))
        .filter(|body| !body.is_empty());
    let options = section(text, &["버린 선택지", "검토한 대안"])
        .map(rejected_options)
        .unwrap_or_default();

    Ok(NewDecision {
        source: "plan",
        external_id: format!("{repo}/{relative_path}"),
        kind: "plan",
        project: Some(repo.to_owned()),
        session_id: None,
        question: redact(title),
        options,
        answers: vec![redact(&approval)],
        rationale,
        context: None,
        decided_at,
    })
}

/// A `label: value` field of the header. Fields share lines separated by
/// ` · `; the approval source is last and may itself contain ` · `, so it
/// runs to the end of its line.
fn header_field(header: &[&str], label: &str) -> Option<String> {
    header.iter().find_map(|line| {
        let start = line.find(label)? + label.len();
        let rest = line[start..].trim_start();
        let value = if label == "승인 출처:" {
            rest
        } else {
            rest.split(" · ").next().unwrap_or(rest)
        };
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// Body of the first `## ` section whose heading starts with one of
/// `headings`, up to the next `## `.
fn section<'a>(text: &'a str, headings: &[&str]) -> Option<&'a str> {
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let heading = line.strip_prefix("## ").map(str::trim);
        match (start, heading) {
            (None, Some(h)) if headings.iter().any(|want| h.starts_with(want)) => {
                start = Some(offset + line.len());
            }
            (Some(from), Some(_)) => return Some(&text[from..offset]),
            _ => {}
        }
        offset += line.len();
    }
    start.map(|from| &text[from..])
}

/// Top-level list items, or the rows of a two-column table, of a rejected
/// options section. In a list item the bold run is the label; in a table the
/// first cell is.
fn rejected_options(body: &str) -> Vec<ChoiceOption> {
    body.lines()
        .filter_map(|line| {
            if let Some(item) = line.strip_prefix("- ") {
                Some(list_option(item))
            } else {
                table_option(line)
            }
        })
        .map(|o| ChoiceOption {
            label: redact(&o.label),
            description: redact(&o.description),
        })
        .collect()
}

fn list_option(item: &str) -> ChoiceOption {
    let item = item.trim();
    if let Some(rest) = item.strip_prefix("**")
        && let Some(end) = rest.find("**")
    {
        let description = rest[end + 2..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '—' || c == ':' || c == '-')
            .trim();
        return ChoiceOption {
            label: rest[..end].trim().to_owned(),
            description: description.to_owned(),
        };
    }
    ChoiceOption {
        label: item.to_owned(),
        description: String::new(),
    }
}

/// A body row of a markdown table; the header row and the `|---|` separator
/// are not options.
fn table_option(line: &str) -> Option<ChoiceOption> {
    let cells: Vec<&str> = line
        .trim()
        .strip_prefix('|')?
        .strip_suffix('|')?
        .split('|')
        .map(str::trim)
        .collect();
    let is_separator = cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'));
    let label = cells.first()?.trim_matches('*').trim();
    if is_separator || label.is_empty() || label == "대안" || label == "후보" {
        return None;
    }
    Some(ChoiceOption {
        label: label.to_owned(),
        description: cells.get(1).copied().unwrap_or("").to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "# 캐시를 디스크로 옮긴다

> **Plan — 이 시점의 결정 기록.** 현재 정책의 신뢰 소스가 아니다.
> 날짜: 2026-10-01 · 브랜치: feature/cache · 이슈: #12 · 승인 출처: 사용자 승인 (\"진행\" · 범위 확정)

## 요구사항

1. 재시작해도 캐시가 남아야 한다.

## 결정

1. 디스크 캐시를 쓴다.
2. TTL 은 하루.

## 버린 선택지

- **메모리 캐시** — 재시작에 사라진다.
- Redis 도입
  - 들여쓴 줄은 항목이 아니다.

## 범위 밖

- 분산 캐시
";

    #[test]
    fn a_plan_becomes_one_entry_with_its_decision_and_rejected_options() {
        let decision = parse_plan(PLAN, "repo", "plans/2026-10-01-cache.md").unwrap();
        assert_eq!(decision.external_id, "repo/plans/2026-10-01-cache.md");
        assert_eq!(decision.kind, "plan");
        assert_eq!(decision.question, "캐시를 디스크로 옮긴다");
        assert_eq!(decision.answers, ["사용자 승인 (\"진행\" · 범위 확정)"]);
        assert_eq!(decision.decided_at, "2026-10-01T00:00:00Z");
        assert_eq!(
            decision.rationale.as_deref(),
            Some("1. 디스크 캐시를 쓴다.\n2. TTL 은 하루.")
        );
        assert_eq!(
            decision.options,
            [
                ChoiceOption {
                    label: "메모리 캐시".into(),
                    description: "재시작에 사라진다.".into()
                },
                ChoiceOption {
                    label: "Redis 도입".into(),
                    description: String::new()
                },
            ]
        );
    }

    #[test]
    fn decision_section_heading_may_carry_a_suffix() {
        let text = PLAN.replace("## 결정\n", "## 결정 사항\n");
        let decision = parse_plan(&text, "repo", "plans/x.md").unwrap();
        assert!(decision.rationale.unwrap().starts_with("1. 디스크"));
    }

    #[test]
    fn table_alternatives_become_options() {
        let text = PLAN.replace(
            "## 버린 선택지\n\n- **메모리 캐시** — 재시작에 사라진다.\n- Redis 도입\n  - 들여쓴 줄은 항목이 아니다.\n",
            "## 검토한 대안\n\n| 대안 | 판단 |\n|---|---|\n| 메모리 캐시 | 버림 — 휘발 |\n",
        );
        let decision = parse_plan(&text, "repo", "plans/x.md").unwrap();
        assert_eq!(
            decision.options,
            [ChoiceOption {
                label: "메모리 캐시".into(),
                description: "버림 — 휘발".into()
            }]
        );
    }

    #[test]
    fn files_without_the_plan_header_are_not_plans() {
        assert_eq!(
            parse_plan("# README\n\nplans 설명", "repo", "plans/README.md"),
            Err(PlanSkip::NotAPlan)
        );
    }

    #[test]
    fn missing_header_fields_are_reported() {
        let no_date = PLAN.replace("날짜: 2026-10-01 · ", "");
        assert_eq!(
            parse_plan(&no_date, "repo", "p.md"),
            Err(PlanSkip::MissingDate)
        );
        let no_approval = PLAN.replace(" · 승인 출처: 사용자 승인 (\"진행\" · 범위 확정)", "");
        assert_eq!(
            parse_plan(&no_approval, "repo", "p.md"),
            Err(PlanSkip::MissingApproval)
        );
        let no_title = PLAN.replace("# 캐시를 디스크로 옮긴다\n", "");
        assert_eq!(
            parse_plan(&no_title, "repo", "p.md"),
            Err(PlanSkip::MissingTitle)
        );
    }

    #[test]
    fn plans_without_optional_sections_still_parse() {
        let minimal = "# 제목\n\n> **Plan — 기록.**\n> 날짜: 2026-01-02 · 승인 출처: grill 합의\n";
        let decision = parse_plan(minimal, "repo", "p.md").unwrap();
        assert_eq!(decision.rationale, None);
        assert!(decision.options.is_empty());
        assert_eq!(decision.answers, ["grill 합의"]);
    }
}
