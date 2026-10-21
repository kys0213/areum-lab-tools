# doppel — 판단 원장 · 골든셋 · 브리프 · 평가 · 설계

내 결정을 기록하고, 판례로 다듬고, 다음 결정의 추천에 쓰고, 추천이 나를 얼마나
닮았는지 재는 루프. 전체 구조는 [doppel.md](./doppel.md)에 있다.

```
수집(ingest·실시간) → 판단 원장 → 분류 → 골든 후보 → 묶음 리뷰 → 확정 골든
                                                          │
         다음 질문 ─▶ 판례 검색 번들 ─▶ 브리프(추천) ─▶ 내 결정 ─▶ 원장 (수락 여부)
```

---

## 1. 판단 원장

내가 판단한 순간 하나가 행 하나다. 원장은 지우지 않는다.

| 출처 (`source`) | 종류 (`kind`) | 무엇을 | `external_id` |
|---|---|---|---|
| `session` | `ask` | 세션 JSONL 의 `AskUserQuestion` 답 — 질문 하나당 한 행 | `<session_id>:<tool_use_id>:<질문 순번>` |
| `session` | `permission` | 세션 JSONL 에서 사람이 직접 내린 권한 결정 (`permissionDecision.source` 가 `user` 로 시작) | `<session_id>:<record uuid>` |
| `plan` | `plan` | `plans/*.md` 의 결정 기록 — 제목·승인 출처·결정·버린 선택지 | `<repo 이름>/<repo 기준 경로>` |
| `doppel` | `ask`·`permission` | doppel 이 중계한 질문의 답 (P1) | `<session_id>:<tool_use_id>:<질문 순번>` |

### 세션 JSONL 에서 읽는 것

- `AskUserQuestion` 의 질문·선택지는 assistant 행의 `tool_use.input.questions`, 답은 같은
  `tool_use_id` 를 가진 user 행의 `toolUseResult.answers` (질문 문장 → 답 문자열 또는
  문자열 배열)다. 다중 선택·직접 입력도 그대로 담는다.
- 맥락은 그 질문 직전의 assistant 텍스트 블록이다 (끝에서 2,000자).
- 권한 결정은 `permissionDecision` 이 있는 user 행이다. `source` 가 `config`(규칙·분류기)인
  것은 내 판단이 아니므로 읽지 않는다. 질문은 도구 이름과 입력 요약(Bash 는 `command`,
  파일 도구는 경로)이다.
- 서브에이전트 행(`isSidechain: true`)은 읽지 않는다.

### plans 에서 읽는 것

`> **Plan —` 헤더를 가진 파일만 읽는다.

| 필드 | 원천 |
|---|---|
| 질문 | 첫 `# ` 제목 |
| 답 | 헤더의 `승인 출처:` 값 |
| 근거 | `## 결정` 으로 시작하는 절 본문 (앞에서 4,000자 — 결정은 절 앞쪽에 온다) |
| 선택지 | `## 버린 선택지`·`## 검토한 대안` 절의 목록 항목 (굵은 글씨가 label, 나머지가 설명) |
| 결정 시각 | 헤더의 `날짜:` (`YYYY-MM-DD` → 그날 `00:00:00Z`) |

### 비밀 가리기

원장에 넣기 전 모든 텍스트 필드에서 알려진 비밀 형태를 `[REDACTED]` 로 바꾼다:
`sk-`·`ghp_`·`gho_`·`github_pat_`·`xox[abpr]-`·`AKIA` 접두 토큰, `Bearer <토큰>`, PEM 개인키
블록. 같은 입력이면 같은 결과다.

---

## 2. 카테고리

판단의 종류. `doppel init` 이 시드 셋을 넣는다.

| 이름 | 설명 |
|---|---|
| `design-scope` | 설계 승인·범위 자르기 — plan 승인·거절, 범위 밖 판정, 버린 선택지 |
| `naming-api-test` | 네이밍·API·테스트 전략 — 이름, 인터페이스 모양, 테스트 방식 |
| `dependency-review` | 의존성·리뷰 판정 — 라이브러리 선택, 리뷰 finding 수용·거절 |

- 분류 체계는 로컬 세션 데이터로 키운다. 분류 skill 이 원장을 보고 새 카테고리와 배정을
  제안하고(`--by model`), 묶음 리뷰에서 내가 확정한다.
- 카테고리는 지우지 않고 `retired` 로 내린다. 내린 카테고리로는 새로 분류할 수 없다.
- 판단의 카테고리를 사람이 정했는지(`human`) 모델이 정했는지(`model`)를 남긴다.

---

## 3. 골든셋

판례. 판단 원장의 행을 골라 그 시점의 내용을 복사해 둔다 — 원장이 나중에 다시 분류돼도
판례는 리뷰한 그대로다.

| 상태 | 의미 |
|---|---|
| `candidate` | 후보. 아직 판례가 아니다 |
| `confirmed` | 내가 확정한 판례. 검색의 기본 대상 |
| `retired` | 내림. 더 맞지 않거나(`superseded_by` 로 대체 판례를 가리킬 수 있다) 후보에서 버림 |

| 전이 | 명령 |
|---|---|
| (판단) → `candidate` | `golden propose <decision-id> --reason <이유>` |
| `candidate` → `confirmed` | `golden confirm` |
| `candidate`·`confirmed` → `retired` | `golden retire [--superseded-by <id>]` |

- 후보가 된 이유(`reason`): `override`(추천을 내가 뒤집음), `ingest`(과거 기록에서 뽑힘),
  `manual`(내가 직접 고름). `doppel review` 는 `override` 를 맨 앞에 둔다.
- 판단 하나에는 골든이 하나뿐이다.
- 분류되지 않은 판단은 후보가 될 수 없다.
- 대체 판례는 확정된 골든이어야 하고 자기 자신일 수 없다.

---

## 4. 판례 검색

`doppel search` 는 결정적이다. 같은 DB 와 같은 질의면 같은 결과다.

- SQLite FTS5 (`unicode61`). 질의는 공백·구두점으로 나눈 단어마다 접두 일치(`"단어"*`)로
  바꿔 OR 로 묶는다 — 색인된 단어에 조사가 붙어 있어도("저장소에") 어간 질의("저장소")로
  찾힌다.
- 질의 쪽에 조사가 붙은 경우("테스트는")를 위해, 한글 단어는 끝 음절을 하나·둘 뗀 것도
  접두 질의로 더한다. 두 음절보다 짧게는 떼지 않는다.
- 정렬: `bm25` 점수, 같으면 최근 결정 순.
- 필터: 카테고리, 프로젝트. 기본 대상은 `confirmed`, `--include-candidates` 로 후보까지.
- 결과(검색 번들)는 판례마다 질문·선택지·답·근거·카테고리·프로젝트·결정 시각·점수를 담는다.
  브리프 agent 는 이 번들만 보고 추천을 쓴다.

---

## 5. 브리프와 평가 (P1)

### 브리프

결정 지점마다 한 장.

- 무엇을 결정해야 하나 (질문, 선택지)
- 배경 (세션이 하던 일, 지금까지 흐름, 관련 파일·PR·이슈)
- 과거 유사 판단 (검색 번들 상위 판례와 그때의 답·근거, 서로 다른 답이 섞이면 표시)
- 의견: 판례와 가장 가까운 선택을 추천하거나, 판례가 맞지 않으면 "새 의견" 으로 표시해
  제시한다. 확신도와 가역성을 함께 적는다.

브리프 레코드는 사용한 골든셋 상태와 브리프 프롬프트 버전을 남긴다.

### 평가

모든 브리프가 곧 예측이다. 따로 shadow 모드를 두지 않는다.

- 카테고리별 추천 수락률을 다수 답 baseline 대비 향상폭(lift)으로 본다. 늘 같은 답을
  추천하는 분신이 높은 점수를 얻지 못하게.
- 거절·정정 사례와 "새 의견" 채택률은 따로 센다.
- 골든셋 leave-one-out 재현으로 프롬프트·골든셋 변경의 회귀를 잰다.

### 자동 처리

기본은 끈다. 내가 카테고리별로 켤 때만 doppel 이 추천안으로 바로 답하고 사후 보고한다.
켤 수 있는지는 측정값과 내가 정한 기준값(표본 수·lift)으로 CLI 가 판정하고, 기준값을
스스로 바꾸지 않는다. 되돌릴 수 없는 행동은 켤 수 없다.

---

## 6. 저장 스키마

```sql
CREATE TABLE categories (
  name        TEXT PRIMARY KEY,          -- kebab-case
  description TEXT NOT NULL,
  state       TEXT NOT NULL DEFAULT 'active',
  created_at  TEXT NOT NULL,
  CHECK (state IN ('active','retired'))
);

CREATE TABLE decisions (
  id            TEXT PRIMARY KEY,        -- dec-000001
  source        TEXT NOT NULL,           -- session | plan | doppel
  external_id   TEXT NOT NULL,
  kind          TEXT NOT NULL,           -- ask | permission | plan
  project       TEXT,                    -- 세션 cwd 또는 repo 이름
  session_id    TEXT,
  question      TEXT NOT NULL,
  options       TEXT NOT NULL,           -- JSON [{label, description}]
  answers       TEXT NOT NULL,           -- JSON [string]
  rationale     TEXT,
  context       TEXT,
  category      TEXT REFERENCES categories(name),
  category_by   TEXT,                    -- human | model
  decided_at    TEXT NOT NULL,
  created_at    TEXT NOT NULL,

  UNIQUE (source, external_id),
  CHECK (source IN ('session','plan','doppel')),
  CHECK (kind IN ('ask','permission','plan')),
  CHECK ((category IS NULL) = (category_by IS NULL)),
  CHECK (category_by IS NULL OR category_by IN ('human','model'))
);

CREATE TABLE golden (
  id            TEXT PRIMARY KEY,        -- gld-000001
  decision_id   TEXT NOT NULL UNIQUE REFERENCES decisions(id),
  category      TEXT NOT NULL REFERENCES categories(name),
  project       TEXT,
  question      TEXT NOT NULL,           -- 이하 후보가 된 시점의 복사본
  options       TEXT NOT NULL,
  answers       TEXT NOT NULL,
  rationale     TEXT,
  context       TEXT,
  decided_at    TEXT NOT NULL,
  state         TEXT NOT NULL,           -- candidate | confirmed | retired
  reason        TEXT NOT NULL,           -- override | ingest | manual
  superseded_by TEXT REFERENCES golden(id),
  note          TEXT,
  created_at    TEXT NOT NULL,
  reviewed_at   TEXT,

  CHECK (state IN ('candidate','confirmed','retired')),
  CHECK (reason IN ('override','ingest','manual')),
  CHECK ((state = 'candidate') = (reviewed_at IS NULL)),
  CHECK (superseded_by IS NULL OR state = 'retired'),
  CHECK (superseded_by IS NULL OR superseded_by <> id)
);

CREATE VIRTUAL TABLE golden_fts USING fts5(
  golden_id UNINDEXED, question, answers, rationale, context,
  tokenize = 'unicode61'
);
```

`golden_fts` 는 골든을 만들 때 함께 쓴다. 골든의 검색 대상 텍스트는 바뀌지 않으므로
갱신 경로가 없다.
