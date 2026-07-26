# kanban — 로컬 칸반 보드 · 설계

프로젝트별 이슈를 로컬에 모아 상태를 관리하는 CLI. 유입된 이슈를 로컬 LLM으로
분류해 프로젝트에 배정하고, 실행 에이전트가 작업을 점유·완료하는 경로를 제공한다.

분류기 상세(파이프라인·프롬프트 계약·골든셋)는 [kanban-classifier.md](./kanban-classifier.md)에 있다.

---

## 1. 목표와 경계

### 해결하는 문제

Discord·GitHub 등 여러 경로로 들어오는 아이디어와 이슈가 흩어져 있고, 어느 프로젝트
일감인지 사람이 매번 판단해야 한다. kanban은 유입을 한 곳에 모으고, 분류를 로컬 모델에
맡기고, 실행 에이전트가 집어갈 수 있는 형태로 정리한다.

### belt과의 관계

belt(별도 레포)은 그대로 유지한다. 코드를 이식하지 않았고 개념만 참고했다. 두 도구의
책임은 겹치지 않는다.

| | kanban | belt |
|---|---|---|
| 담당 | 분류 · 상태관리 | 에이전트 실행 |
| 상주 | 분류 데몬 | 실행 데몬 |
| 외부 의존 | 로컬 LLM 하나 | GitHub · Claude/Gemini/Codex |

belt은 kanban의 소비자다. `next`로 작업을 점유하고 `done`/`release`로 결과를 돌려준다.

### 명시적 범위 밖

| 항목 | 담당 |
|---|---|
| 에이전트 실행 · 생명주기 | belt |
| HITL (사람 확인·승인) | 실행 에이전트가 자신의 소통 도구로 직접 |
| Discord·GitHub 폴링 | 외부가 `kanban add`로 push |
| git worktree 격리 | 코딩 에이전트에 내장됨 |
| TUI 대시보드 | `list --json`으로 충분 |
| 중복 이슈 자동 병합·종료 | 라벨까지만 — 보드를 자동으로 옮기거나 지우지 않는다 |

---

## 2. 설계 원칙

### 1. 분류기는 라벨과 배정까지만 한다

중복·유사 판정은 라벨로 남기고 보드를 움직이지 않는다. 로컬 모델의 오판이 작업을
삭제하거나 묻어버리는 일이 없어야 한다. 오분류의 비용은 라벨 하나가 틀리는 것에
그친다.

### 2. 실패는 상태가 아니라 점유 해제다

`failed` 상태를 두지 않는다. 작업이 실패하면 `release`로 점유를 풀어 `backlog`로
되돌린다. 실패를 상태로 만들면 "실패했지만 다시 해야 하는 것"과 "실패해서 끝난 것"을
구분하는 규칙이 계속 늘어난다.

### 3. 불변식은 DB에 새긴다

상태와 부속 컬럼의 정합성을 `CHECK` 제약으로 강제한다. `running`인데 점유자가 없거나
`backlog`인데 프로젝트가 없는 행은 애초에 쓰이지 않는다. 코드 버그가 조용히 이상한
행을 만드는 대신 즉시 실패한다.

### 4. 점유는 원자적이다

`next`는 조회가 아니라 점유다. 조회와 갱신을 한 SQL 문으로 묶어 여러 에이전트가 동시에
호출해도 같은 아이템을 두 번 내주지 않는다. 구경만 할 때는 `list`를 쓴다.

### 5. 확정적으로 알 수 있는 것은 모델에게 묻지 않는다

같은 원본이 다시 들어왔는지는 `UNIQUE(source, external_id)`로 판정한다. 모델의 중복
판정과는 다른 층이며, 서로를 대체하지 않는다.

### 6. 사람이 손댄 것만 정답으로 취급한다

모델 판정을 사람이 그냥 둔 것은 골든셋에 넣지 않는다. 모델이 만든 답을 다시 few-shot으로
먹이면 자기 편향이 증폭된다. 상세는 [kanban-classifier.md](./kanban-classifier.md) 참조.

---

## 3. 전체 구조

```
   유입 (외부가 CLI 를 호출해 밀어넣는다)
   discord ┐
   github  ├──►  ┌──────────────────────────────┐
   사람    ┘     │  inbox   (프로젝트 미배정)    │
                 │   itm-000021  itm-000022     │
                 └──────────────┬───────────────┘
                                │  분류기 = 로컬 LLM
                                │  (데몬이 N분마다 실행)
                 ┌──────────────┴───────────────┐
                 ▼                              ▼
           unmatched                    project: areum-lab-tools
        (모델이 확신 못함)      ┌─────────┬──────────────────────┬───────┐
        사람 후보정 대기        │ backlog │ running              │ done  │
                               │ itm-17  │ itm-12               │ itm-08│
                               │  P1     │  session: sess-abc   │       │
                               │  labels │  agent:   claude     │       │
                               └─────────┴──────────────────────┴───────┘
                                     ▲          │
                                     └──────────┘
                                       release
```

`unmatched`를 1급 상태로 둔 이유는 **모델이 몰라서 못 정한 것**과 **아직 안 본 것**이
다른 상태이기 때문이다. 전자만 사람이 봐야 한다.

---

## 4. 상태 모델

### 상태

| 상태 | 의미 | project | session_id |
|---|---|---|---|
| `inbox` | 유입됨, 아직 분류 안 됨 | NULL | NULL |
| `unmatched` | 분류 실패 — 사람 후보정 대기 | NULL | NULL |
| `backlog` | 프로젝트 배정됨, 대기 | 있음 | NULL |
| `running` | 에이전트가 점유 중 | 있음 | 있음 |
| `done` | 완료 | 있음 | 완료한 에이전트 (사람이 직접 완료했으면 NULL) |

`done`은 `session_id`를 **지우지 않는다.** 누가 그 작업을 했는지가 완료 후에도 남아야
한다. 반대로 `release`는 점유를 푸는 것이므로 `session_id`·`agent`·`claimed_at`을
비운다.

### 전이

| 전이 | 주체 | 명령 |
|---|---|---|
| (유입) → `inbox` | discord / github / 사람 | `add` |
| `inbox` → `backlog` | 분류기 (데몬) | 자동 |
| `inbox` → `unmatched` | 분류기 (데몬) | 자동 |
| `unmatched` → `backlog` | 사람 | `assign` |
| `unmatched` → `backlog` 또는 `unmatched` 유지 | 사람 | `reclassify` |
| `backlog` → `running` | belt / 실행 에이전트 | `next` (원자적 점유) |
| `running` → `done` | belt / 실행 에이전트 | `done` |
| `running` → `backlog` | belt / 사람 | `release` |
| 임의 | 사람 | `move` (탈출구) |

분류기는 `inbox` 아이템만 건드린다. 이미 에이전트가 점유한 아이템을 모델이 옆에서
옮기면 경합이 생긴다.

### 에이전트가 죽었을 때

`running` 아이템은 자동으로 회수되지 않는다. `release`가 유일한 복구 경로다.
`list --state running`으로 오래 잡혀 있는 아이템을 찾아 사람 또는 belt이 푼다.

> 자동 회수(타임아웃 기반)는 넣지 않았다. 얼마나 걸려야 정상인지가 작업마다 다르고,
> 잘못 잡으면 진행 중인 작업을 뺏는다. 필요해지면 `claimed_at` 컬럼이 이미 있으므로
> 나중에 얹을 수 있다.

---

## 5. 명령 표면

기본 출력은 사람용 텍스트(성공 stdout / 에러 stderr), `--json`은 기계 파싱용 봉투를
stdout으로 낸다. 에이전트는 항상 `--json`을 붙인다. discord와 동일한 규약이다.

### 설정 · 프로젝트

```sh
kanban init                                   # ~/.areum/kanban/ 생성
kanban project add <name> --desc "<설명>"      # 설명이 곧 분류 근거
kanban project list
kanban project rm <name>
```

### 유입

```sh
kanban add --source discord --external-id <message-id> \
           --title "<제목>" --body "<본문>"
kanban add --source cli --external-id <임의값> --title "..." --body -   # 본문 stdin
```

`--source`와 `--external-id` 조합이 이미 있으면 거부한다(exit 8). 같은 Discord 메시지가
두 번 들어와도 중복 아이템이 생기지 않는다.

### 조회

```sh
kanban list                                    # 전체
kanban list --project areum-lab-tools --state backlog
kanban list --state unmatched --json           # 후보정 대상 + 판단 근거
kanban list --label duplicate-of
kanban show <id> --json
```

### 실행 에이전트용

```sh
kanban next --project <name> --session <session-id> --agent claude
  # → 점유 성공: 아이템 JSON / 비어있음: {"id": null}
kanban done <id>
kanban release <id> --reason "빌드 실패"
```

### 후보정

```sh
kanban assign <id> --project belt [--priority P1]   # 사람이 직접 배정 → 골든셋 기록
kanban priority <id> P0                             # 우선순위만 정정 → 골든셋 기록
kanban reclassify <id>                              # 분류 1회 재시도
kanban move <id> <state>                            # 임의 전이 (탈출구)
```

### 골든셋 · 분류기

```sh
kanban golden list
kanban golden rm <id>
kanban golden export --json
kanban eval                                    # 골든셋 재분류로 정확도 측정
kanban daemon start | stop | status
```

---

## 6. 저장 스키마

`~/.areum/kanban/kanban.db` (rusqlite, bundled).

```sql
CREATE TABLE projects (
  name        TEXT PRIMARY KEY,
  description TEXT NOT NULL,          -- 분류 근거
  created_at  TEXT NOT NULL
);

CREATE TABLE items (
  id          TEXT PRIMARY KEY,       -- itm-000017 (도구가 발급)
  source      TEXT NOT NULL,          -- discord | github | cli
  external_id TEXT NOT NULL,
  title       TEXT NOT NULL,
  body        TEXT NOT NULL,
  state       TEXT NOT NULL,          -- inbox|unmatched|backlog|running|done
  project     TEXT REFERENCES projects(name),
  priority    TEXT NOT NULL DEFAULT 'P2',
  session_id  TEXT,                   -- 점유자
  agent       TEXT,                   -- claude | codex | ...
  claimed_at  TEXT,
  created_at  TEXT NOT NULL,
  updated_at  TEXT NOT NULL,

  UNIQUE (source, external_id),
  CHECK (priority IN ('P0','P1','P2','P3')),
  CHECK (state IN ('inbox','unmatched','backlog','running','done')),
  CHECK (
    (state IN ('inbox','unmatched') AND project IS NULL     AND session_id IS NULL)
 OR (state = 'backlog'              AND project IS NOT NULL AND session_id IS NULL)
 OR (state = 'running'              AND project IS NOT NULL AND session_id IS NOT NULL)
 OR (state = 'done'                 AND project IS NOT NULL)
  )
);

CREATE TABLE labels (
  item_id    TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  key        TEXT NOT NULL,           -- kind | area | duplicate-of | similar
  value      TEXT NOT NULL,
  confidence REAL,
  PRIMARY KEY (item_id, key, value)
);
```

라벨은 아이템의 부속물이므로 `ON DELETE CASCADE`로 함께 사라진다. 반면 골든셋은
독립 자산이라 연결하지 않는다 — 이유는 [kanban-classifier.md](./kanban-classifier.md) 참조.

`classification`·`golden` 테이블은 [kanban-classifier.md](./kanban-classifier.md)에 정의한다.

### `done`만 `session_id` 제약이 느슨한 이유

`done`은 두 경로로 도달한다 — 에이전트가 `next`로 점유해 끝냈거나(`session_id` 있음),
사람이 `move`로 직접 완료 처리했거나(`session_id` 없음). 둘 다 유효하므로 `done`에서는
`session_id`의 존재 여부를 강제하지 않는다.

### 외래키와 `project rm`

연결 시 `PRAGMA foreign_keys = ON`을 건다. 따라서 아이템이 참조 중인 프로젝트는
`project rm`으로 지울 수 없고 `conflict`(exit 8)로 거부된다. 프로젝트를 지우려면 먼저
아이템을 다른 프로젝트로 옮기거나 정리해야 한다.

### 우선순위를 `P0`~`P3` 문자열로 두는 이유

사전순 정렬이 곧 우선순위 정렬이라 별도 매핑이 필요 없고, 모델이 뱉기에도 자연스러운
토큰이다. 확신이 없을 때의 기본값은 `P2`다.

### 원자적 점유

```sql
UPDATE items
   SET state='running', session_id=?, agent=?, claimed_at=?, updated_at=?
 WHERE id = (SELECT id FROM items
              WHERE project=? AND state='backlog'
              ORDER BY priority ASC, created_at ASC
              LIMIT 1)
RETURNING id, title, body, project, priority;
```

SQLite는 writer를 직렬화하므로 claude와 codex가 같은 순간에 호출해도 서로 다른 행을
받는다. 별도 락이 필요 없다. `ORDER BY priority ASC, created_at ASC`로 P0을 먼저,
같은 우선순위면 오래된 것을 먼저 내준다.

---

## 7. 모듈 구조

`.claude/rules/module-structure.md`의 3층 구조를 따른다.

```
tools/kanban/src/
├── main.rs                 # mod 선언 + 배선 + 디스패치
├── cli.rs                  # clap 명령 표면
├── commands/               # 서브커맨드 하나당 파일 하나
│   ├── mod.rs              #   선언·재노출만
│   ├── init.rs
│   ├── project.rs
│   ├── add.rs
│   ├── list.rs
│   ├── show.rs
│   ├── next.rs
│   ├── done.rs
│   ├── release.rs
│   ├── assign.rs
│   ├── priority.rs
│   ├── reclassify.rs
│   ├── move_item.rs        #   `move`는 예약어라 파일명만 다르다
│   ├── golden.rs
│   ├── eval.rs
│   ├── daemon.rs
│   └── testutil.rs         #   공유 목·픽스처 단일 소유
├── classify/               # 분류 파이프라인 — daemon 과 reclassify 가 공유
│   ├── mod.rs
│   ├── engine.rs           #   Classifier trait 정의 + 파이프라인
│   ├── llm.rs              #   OpenAI 호환 HTTP 구현체
│   └── prompt.rs           #   프롬프트 조립 · 응답 파싱
├── daemon/
│   ├── mod.rs
│   └── ticker.rs           # 틱 루프 · pid · SIGTERM
├── output/
│   ├── mod.rs
│   ├── envelope.rs         # JSON 봉투 (discord 와 동일 규약)
│   └── payload.rs
└── common/                 # 2개 이상 기능이 공유하는 기반만
    ├── mod.rs
    ├── config.rs           # ~/.areum/kanban/config.json
    ├── error.rs            # AppError + ErrorKind → exit code
    ├── store.rs            # SQLite 접근 · 상태 전이
    └── time.rs
```

`store.rs`는 테이블이 다섯이라 프로덕션 300줄을 넘길 가능성이 높다. 넘기면
`common/store/`로 승격하고 `tests.rs`를 자식 모듈로 분리한다.

### discord와의 코드 중복

`output/envelope.rs`·`common/error.rs`·`daemon`의 pid 처리는 discord와 구조가 같다.
`tool-crate.md`의 Rule of 3에 따라 **지금은 추출하지 않는다** — 이번이 두 번째다.
세 번째 도구에서 같은 코드가 필요해지면 그때 `libs/`로 뺀다.

---

## 8. 에러와 exit code

discord와 공유하는 kind는 같은 번호를 유지하고, kanban 고유 kind만 뒤에 붙인다.

| kind | exit | 상황 |
|---|---|---|
| `internal` | 1 | 예기치 못한 실패 |
| `usage` | 2 | 인자 조합 오류 |
| `config` | 3 | 설정 파일 없음·손상, LLM 엔드포인트 미설정 |
| `api` | 4 | LLM 엔드포인트가 오류 응답 |
| `rate_limit` | 5 | LLM 엔드포인트 레이트리밋 |
| `network` | 6 | LLM 엔드포인트 연결 실패 |
| `not_found` | 7 | 아이템·프로젝트 없음 |
| `conflict` | 8 | 전이 규칙 위반, `UNIQUE(source, external_id)` 충돌 |

전이 규칙 위반의 예: `backlog` 아이템에 `done`을 호출, 이미 점유된 아이템을 다시
`next`로 잡으려는 시도. 조용히 성공시키지 않고 `conflict`로 거부한다.

---

## 9. 테스트

`.claude/rules/module-structure.md`에 따라 전부 인라인 `#[cfg(test)]`다. 통합 `tests/`
디렉토리는 private 접근이 안 되므로 쓰지 않는다. 공유 목·픽스처는
`commands/testutil.rs`가 단독 소유한다.

| 대상 | 검증 |
|---|---|
| `common/store.rs` | `CHECK` 불변식 위반 시 쓰기 거부 · `UNIQUE` 재유입 차단 · **커넥션 2개로 동시 `next` → 서로 다른 id** · `ORDER BY priority` 순서 · **`done`이 `session_id`를 보존하고 `release`가 비우는가** · 참조 중인 `project rm` 거부 |
| `commands/*.rs` | 각 전이의 성공 경로와 거부 경로 |
| `output/envelope.rs` | JSON 문자열 전체를 리터럴로 고정 (계약 회귀 방지) |
| `common/error.rs` | 모든 kind의 exit code 매핑 |

분류기 쪽 테스트는 [kanban-classifier.md](./kanban-classifier.md)에 있다.

---

## 10. 의존성

워크스페이스 `Cargo.toml`에 **이미 전부 있다. 새로 추가할 것이 없다.**

| 용도 | 패키지 |
|---|---|
| CLI | `clap` |
| 직렬화 | `serde`, `serde_json` |
| 에러 | `anyhow` |
| 비동기 | `tokio` (`time`, `signal`) |
| HTTP | `reqwest` (`json`, `rustls-tls`) |
| DB | `rusqlite` (`bundled`) |
| 시그널 | `libc` |

`twilight-*`만 쓰지 않는다. 워크스페이스 의존성을 건드리지 않으므로 discord 도구를
재빌드시키거나 깨뜨릴 위험이 없다.

---

## 11. 구현 단계

| 단계 | 내용 | 산출 |
|---|---|---|
| 1 | 스캐폴드 + `store` + 상태 전이 | LLM 없이 완결되는 보드 |
| 2 | 분류 파이프라인 + `MockClassifier` | 로컬 모델 없이 전 경로 테스트 |
| 3 | OpenAI 호환 HTTP 구현체 | 런타임 설치 후 연결 |
| 4 | 골든셋 + `eval` | 사람 피드백 → few-shot |
| 5 | 데몬 | 주기 실행 |

**1단계가 LLM 없이 완결되는 것이 의도다.** 이 머신에 로컬 추론 런타임이 아직 없으므로
(설치 여부 확인됨), 수동 `assign`으로 쓰는 보드가 먼저 동작하고 분류기가 그 위에 얹힌다.

스코프 추정: 프로덕션 약 3천 줄, 테스트 포함 7~8천 줄. discord(10.5천 줄)보다 작다 —
외부 API 표면이 LLM HTTP 하나뿐이라 discord의 REST + Gateway보다 단순하다.

---

## 12. 릴리스

레포 규약을 그대로 따른다. 버전 SSOT는 `tools/kanban/Cargo.toml`의 `version`이고,
태그는 `kanban-vX.Y.Z`다.

```sh
cargo check -p kanban          # version bump 후 Cargo.lock 갱신
make dist-tag TOOL=kanban      # main 에서, 워킹트리 clean 상태로
```
