# doppel — 내 판단을 닮아 가는 로컬 비서 · 설계

이 컴퓨터에서 도는 Claude 세션들을 대신 살펴보고, 결정이 필요한 지점마다 배경과
추천을 붙여 나에게 보고하는 로컬 데몬 + CLI. 외부 작업을 받아 라벨링·분배하고,
내가 내린 결정을 판단 원장에 쌓아 카테고리별 골든셋으로 다듬는다. 쓸수록 추천이
내 판단과 닮아 간다.

판단 학습(원장·골든셋·브리프·평가)은 [doppel-judgment.md](./doppel-judgment.md),
세션·채널 연결 프로토콜은 [doppel-protocol.md](./doppel-protocol.md)에 있다.

---

## 1. 목표와 경계

### 해결하는 문제

Claude 세션이 여러 개 돌면 어느 세션이 나를 기다리는지 찾아다녀야 하고, 같은 종류의
결정을 매번 처음부터 다시 내린다. 결정의 근거도 세션과 함께 흩어져 사라진다.

doppel 은 세 가지를 한다.

1. **살펴보고 보고한다** — 세션 상태를 모아 막힌 곳·결정할 곳을 inbox 하나로 보여 준다.
2. **결정을 준비한다** — 결정할 내용의 배경, 과거 내 판단 중 가장 비슷한 것, 그에
   기댄 추천(또는 판례가 맞지 않을 때 새 의견)을 붙인다. 결정은 내가 한다.
3. **판단을 쌓는다** — 내 결정을 원장에 기록하고, 묶음 리뷰로 골든셋을 확정한다.
   추천과 내 결정의 일치가 곧 평가 지표다.

### 역할 분담

| | doppel 데몬·CLI (이 도구) | doppel plugin (`kys-claude-plugin/plugins/doppel`) |
|---|---|---|
| 성격 | 결정적 — 같은 입력이면 같은 결과 | 판단 — 맥락에 따라 다른 결론 |
| 담당 | 원장·골든셋 상태 전이, 판례 검색 번들, 세션 연결·상태 집계, 칸반, 세션 실행·resume, 지표 집계 | 질문 중계(경주 참여), 브리프 작성, 카테고리 분류 제안, 골든 후보 추천, rule 승격 제안, 유입 triage |
| 형태 | Rust 바이너리 하나 | function hook 모듈(TypeScript) + skill·agent |

데몬은 판단 agent 를 `claude -p` 로 실행만 한다. 판단의 내용은 plugin 의 agent
프롬프트에 있다.

외부 메시징 채널(Slack·Discord·mail)은 doppel 의 일부가 아니다. 채널마다 별도 plugin 이
세션에 붙고, 그 채널의 외부 연결은 채널별 로컬 프로세스 하나가 갖는다. doppel 도 그 중
하나의 채널처럼 세션에 붙는다.

```
claude sessions  N:1  doppel   (웹 UI · 판단 학습)
                 N:1  slack
                 N:1  discord …
```

### 명시적 범위 밖

| 항목 | 이유 |
|---|---|
| 파인튜닝 | 로컬에서 재현·감사가 어렵다. 판례 검색 + 인용이면 왜 그렇게 추천했는지 사람이 검토할 수 있다 |
| 되돌릴 수 없는 행동의 자동 처리 | push·merge·삭제·외부 게시·비용 발생은 어떤 설정으로도 doppel 이 대신 답하지 않는다 |
| 여러 사용자의 판단을 섞기 | doppel 하나는 한 사람의 분신이다 |
| 기존 `kanban`·belt 소비 | doppel 이 수신·라벨링·분배를 직접 갖는다 — 골든셋과 피드백 루프를 한 엔진에 두기 위해 |

---

## 2. 설계 원칙

### 1. 결정은 사람이 한다

기본 동작은 보고다. doppel 이 대신 답하는 것은 내가 카테고리별로 명시적으로 켰을
때뿐이고, 되돌릴 수 없는 행동은 켤 수 없다.

### 2. 사람이 손댄 것만 정답으로 취급한다

골든셋은 내가 리뷰에서 확정한 것만 담는다. 모델이 분류한 카테고리, 모델이 고른
후보는 확정 전까지 판례가 아니다. 모델의 답을 다시 판례로 먹이면 자기 편향이
증폭된다.

### 3. 확정적으로 알 수 있는 것은 모델에게 묻지 않는다

같은 원본이 다시 들어왔는지는 `UNIQUE(source, external_id)` 로 판정한다. 세션이
막혔는지는 이벤트와 시간 기준값으로 판정한다. 모델은 해석이 필요한 곳(턴 종료가
질문인지 보고인지, 브리프 내용)에만 쓴다.

### 4. 불변식은 DB 에 새긴다

상태와 부속 컬럼의 정합성을 `CHECK` 제약으로 강제한다. 확정되지 않은 골든이 리뷰
시각을 갖거나, 폐기되지 않은 골든이 대체 골든을 가리키는 행은 애초에 쓰이지 않는다.

### 5. 먼저 온 답 하나를 세션의 hook 체인이 고른다

같은 질문이 터미널·doppel UI·Slack 에 동시에 떠도 확정되는 답은 하나다. 판정 서버를
따로 두지 않는다. 각 채널 plugin 의 hook 이 `next(e)` 와 자기 채널의 답을 경주시키고,
이긴 쪽의 답이 체인 전체의 결과가 된다. 진 쪽은 확정된 답으로 자기 채널을 갱신한다
([doppel-protocol.md §3](./doppel-protocol.md)).

### 6. doppel 이 없으면 세션은 원래대로 돈다

plugin 은 데몬에 닿지 못하면 아무것도 바꾸지 않는다 (fail-open). doppel 장애가
Claude 작업을 막지 않는다.

---

## 3. 전체 구조

```
              ┌──────────── Claude 세션 (터미널 · doppel 이 띄운 headless) ────────────┐
              │  tool.call 체인:  doppel hook → slack hook → … → core(터미널 대화상자) │
              └──────┬─────────────────────────────┬──────────────────────────────────┘
     HTTP(Unix socket) │ ▲ doppel attach (WebSocket)    │ ▲  slack plugin 의 연결
                     ▼ │                             ▼ │
  ┌──────────────── doppel 데몬 ────────────────┐   slack 로컬 프로세스 ── Slack
  │ 세션 상태(sweep) → 결정 지점 → 브리프 → inbox  │   (Socket Mode 연결 하나)
  │ 판단 원장 → 골든셋 → 판례 검색 → 지표          │
  │ 칸반 (유입 → 라벨링 → 분배 → 세션 실행)        │
  │ SQLite ~/.areum/doppel/doppel.db · 웹 UI      │
  └──────────────────────────────────────────────┘
```

세션 하나에 여러 채널 plugin 이 깔린다. 채널끼리는 서로 모르고, 같은 질문을 두고 세션
안에서 경주한다.

---

## 4. 사용자 시나리오

### 결정 대기 보고

1. 세션 A 가 `AskUserQuestion` 을 낸다. 터미널에 기본 대화상자가 뜬다.
2. doppel plugin 의 hook 이 doppel 에 질문을 올린다. doppel 이 판례를 검색하고 브리프
   agent 로 브리프를 만들어 웹 UI inbox 에 올린다. 터미널에도 band 로 추천 요약이 붙는다.
3. slack plugin 의 hook 은 같은 질문을 Slack 에 올린다.
4. 나는 Slack 에서 답한다. slack hook 이 경주에서 이겨 그 답으로 도구 호출을 끝낸다.
   터미널 대화상자는 닫힌다.
5. doppel hook 은 확정된 답을 tool-result 행에서 읽어 inbox 글을 "Slack 에서 답변됨" 으로
   갱신하고, 질문·브리프 추천·내 답·답한 곳·수락 여부를 원장에 남긴다.

### 늦은 답

1. doppel 이 띄운 headless 세션 B 가 질문을 낸다. 설정한 시간 동안 답이 없다.
2. hook 은 "사용자 판단 보류 중" 을 답으로 돌려주고, 세션은 턴을 끝낸다.
3. 몇 시간 뒤 나는 inbox 에서 답한다. doppel 이 답을 세션 B 로 보내고, hook 이 그
   답으로 새 턴을 연다. 세션 프로세스가 끝났으면 doppel 이 `--resume` 으로 다시 띄운다.

### 묶음 리뷰

1. `doppel review` 가 골든 후보를 보여 준다. 추천을 내가 뒤집은 사례가 맨 앞이다.
2. 나는 후보를 확정하거나 버린다. 예전 판단이 지금 판단으로 뒤집혔으면 예전 골든을
   새 골든으로 대체 처리한다.

---

## 5. 세션 상태

doppel 은 세션별 이벤트를 누적해 상태를 만든다. 시간·횟수 기준값은 설정으로 받는다.

| 상태 | 신호 |
|---|---|
| `running` | `turn.start` 이후 `turn.step`·도구 이벤트가 이어진다 |
| `waiting_ask` | `AskUserQuestion` 도구 호출이 답을 받지 못했다 |
| `waiting_permission` | 권한 확인이 사람에게 넘어왔다 (`PermissionRequest`, `Notification` 의 권한 유형) |
| `waiting_turn` | main 루프의 `turn.complete` 뒤 새 프롬프트가 없다 |
| `stalled` | 턴이 진행 중인데 마지막 이벤트 이후 `stall_after` 가 지났다 |
| `failing` | `PostToolUseFailure` 가 연속 `fail_streak` 회 |
| `disconnected` | `session.end` 없이 attach 연결이 끊기거나 heartbeat 가 빠졌다 |
| `ended` | `session.end` |

`waiting_*` 와 `stalled`·`failing` 이 결정 지점이다. `waiting_turn` 이 나에게 묻는
것인지 단순 완료 보고인지는 브리프 agent 가 마지막 답 텍스트로 해석한다.

---

## 6. 칸반

외부 작업(멘션·이슈·메일·수동 입력)은 카드가 된다.

| 상태 | 의미 |
|---|---|
| `inbox` | 들어옴, 아직 라벨링 안 됨 |
| `triaged` | 프로젝트·우선순위·라벨이 붙음 (라벨링 제안은 모델, 확정은 사람 또는 opt-in 자동) |
| `running` | doppel 이 세션을 띄워 작업 중 (세션 id 를 가짐) |
| `review` | 세션이 끝나 결과 확인 대기 |
| `done` | 완료 |

- 같은 원본은 `UNIQUE(source, external_id)` 로 한 번만 들어온다.
- `running` 진입은 한 SQL 문으로 원자적으로 점유한다.
- 실패는 상태가 아니다. 점유를 풀어 `triaged` 로 되돌린다.
- 라벨링·우선순위·분배 결정도 판단 카테고리다. 내가 고친 것이 골든 후보가 된다.

---

## 7. 명령 표면

기본 출력은 사람용 텍스트(성공 stdout / 에러 stderr), `--json` 은 기계 파싱용 봉투를
stdout 으로 낸다. plugin 과 skill 은 항상 `--json` 을 붙인다. 모든 명령은 `--db <path>`
로 DB 위치를 바꿀 수 있다.

### 설정

```sh
doppel init                                   # ~/.areum/doppel/ 생성, 시드 카테고리 등록
```

### 판단 원장

```sh
doppel ingest sessions [--root ~/.claude/projects]   # 세션 JSONL 의 질문 답·권한 결정
doppel ingest plans <repo>                           # <repo>/plans/*.md 의 결정 기록
doppel decision list [--kind ask|permission|plan] [--category <name> | --uncategorized] [--limit N]
doppel decision show <id>
doppel decision categorize <id> <category> [--by human|model]
```

### 카테고리

```sh
doppel category add <name> --desc "<설명>"
doppel category list
doppel category retire <name>
```

### 골든셋

```sh
doppel golden propose <decision-id> [--reason manual|override|ingest] [--note "..."]
doppel review                                  # 후보 큐 (override 먼저)
doppel golden confirm <id> [--note "..."]
doppel golden retire <id> [--superseded-by <id>] [--note "..."]
doppel golden list [--state candidate|confirmed|retired] [--category <name>]
doppel search "<질문>" [--category <name>] [--project <path>] [--limit 5] [--include-candidates]
```

### 세션·칸반·데몬 (P1 이후)

```sh
doppel daemon start | stop | status
doppel attach --session <id>                   # plugin 이 띄우는 세션별 연결
doppel ask create | wait | answer | close      # doppel 채널의 질문 (doppel-protocol.md)
doppel session list [--state <state>]
doppel card add | list | triage | claim | done | release
```

---

## 8. 저장 스키마

`~/.areum/doppel/doppel.db` (rusqlite, bundled, FTS5). 연결마다
`PRAGMA foreign_keys = ON` 을 건다. 시각은 `YYYY-MM-DDTHH:MM:SSZ` 문자열이다.

판단 원장·카테고리·골든셋·검색 색인은 [doppel-judgment.md §6](./doppel-judgment.md)에
정의한다. 세션·ask·카드 테이블은 해당 단계에서 이 문서에 더한다.

---

## 9. 에러와 exit code

discord·kanban 과 공유하는 kind 는 같은 번호를 쓴다.

| kind | exit | 상황 |
|---|---|---|
| `internal` | 1 | 예기치 못한 실패 |
| `usage` | 2 | 인자 조합 오류 |
| `config` | 3 | HOME 없음, DB 파일 손상 |
| `not_found` | 7 | 판단·카테고리·골든 없음 |
| `conflict` | 8 | 전이 규칙 위반 (확정된 골든을 다시 확정, 폐기된 카테고리로 분류 등) |

`ingest` 는 읽지 못한 줄·파일이 있어도 나머지를 계속 읽는다. 대신 건너뛴 수를 이유별로
결과에 싣는다 — 조용히 버리지 않는다. 이미 들어온 원본은 `skipped_existing` 으로 센다.

---

## 10. 배포

- 지금은 macOS 만 낸다: `aarch64-apple-darwin`, `x86_64-apple-darwin`.
- 릴리스 target 은 도구별로 선언한다 (`tools/<tool>/release-targets`). 공용
  `release.yml` 이 그 목록으로 matrix 를 만든다. 선언이 없는 도구는 기존 기본값
  (`aarch64-apple-darwin`) 그대로다.
- 설치 스크립트는 OS 별 파일이다. `scripts/install.sh` 가 macOS 의 arch 로 target 을
  고른다. 다른 OS 는 지원할 때 매핑(Linux)이나 파일(Windows `install.ps1`)을 더한다.
  해당 target 산출물이 없으면 받기 전에 실패한다.
- 로컬 연결은 transport 추상 하나로 감싼다. macOS 구현은 Unix socket(파일 권한이
  인증)이다.

---

## 11. 구현 단계

| 단계 | 내용 |
|---|---|
| P0 | 판단 원장, `ingest`(세션 JSONL·plans), 카테고리, 골든 묶음 리뷰, 판례 검색 |
| P1 | 데몬, 세션 연결·상태, 결정 지점, 브리프, 웹 UI inbox, doppel 채널의 질문 경주, 수락률 |
| P2 | 채널 plugin (Slack 먼저) — 채널별 로컬 연결 프로세스 + 세션 plugin, 공유 relay 계약 |
| P3 | 칸반 유입·라벨링·분배, 카테고리별 opt-in 자동 처리 |
