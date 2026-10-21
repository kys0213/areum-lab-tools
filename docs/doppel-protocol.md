# doppel — 세션 연결과 질문 중계 · 설계

Claude 세션이 doppel 에 붙는 방식, 그리고 같은 질문을 터미널·doppel·다른 채널
(Slack 등)이 함께 받을 때 답 하나를 고르는 방식. 전체 구조는 [doppel.md](./doppel.md)에 있다.

---

## 1. 연결 구조

```
claude sessions  N:1  doppel     doppel plugin  ⇄ doppel 데몬
                 N:1  slack      slack plugin   ⇄ slack 로컬 프로세스 ⇄ Slack
                 N:1  …          (채널마다 같은 모양)
```

- 세션 하나에 채널 plugin 이 여럿 깔린다. plugin 끼리는 서로 모른다.
- 채널마다 외부 연결은 로컬 프로세스 하나가 갖는다. Slack Socket Mode 는 이벤트를 연결
  중 하나에만 보내고, Discord 게이트웨이는 같은 토큰으로 두 곳에서 열면 응답이 경쟁하기
  때문이다. 세션이 외부 서비스에 직접 붙지 않는다.
- 모든 채널 plugin 은 §3 의 relay 계약을 따른다. doppel 은 그 계약을 따르는 채널이면서
  세션 상태·브리프·원장을 더 갖는다.

---

## 2. 세션 ↔ doppel

function hook 런타임에는 WebSocket API 가 없다. 네트워크는 `$.http.fetch` 요청-응답뿐이고
`socketPath` 로 Unix socket 을 탈 수 있다. 그래서 방향마다 수단이 다르다.

```
plugin hook ──$.http.fetch (Unix socket, POST /v1/events)──────────▶ doppel 데몬
plugin hook ◀── stdout JSON lines ── doppel attach ◀── WebSocket ── doppel 데몬
```

| 방향 | 수단 | 내용 |
|---|---|---|
| 세션 → doppel | `$.http.fetch` POST, 보내고 잊는다 | 세션 이벤트, 질문, 확정된 답 |
| doppel → 세션 | `session.start` 에서 `$.process.spawn` 으로 띄운 `doppel attach --session <id>` 의 stdout | 답, 맥락 주입, 새 턴 지시 |

- `doppel attach` 는 세션 수명 동안 산다. 끊기면 plugin 이 `$.clock.after` 로 다시 띄운다.
- 데몬이 없으면 POST 가 실패하고 plugin 은 아무것도 바꾸지 않는다.
- 소켓 경로: `~/.areum/doppel/doppel.sock` (권한 600). Windows 를 지원할 때 loopback
  TCP + 토큰 transport 를 더한다.

### 세션 → doppel 이벤트

모든 이벤트는 `{ "v": 1, "session_id", "at", "kind", ... }` 이다. `at` 은 hook 의 시각(RFC3339)
으로 참고만 하고, doppel 은 받은 시각으로 상태를 판정한다 — 세션마다 시계가 어긋나도 정체
판정이 흔들리지 않게. `launched_by` 는 doppel 이 띄운 세션에 넣는 env `DOPPEL_LAUNCHED=1`
로 정한다.

| kind | 보내는 hook | 추가 필드 |
|---|---|---|
| `session.start` | `session.start` | `cwd`, `surface`(`terminal`·`headless`), `launched_by`(`doppel`·`user`) |
| `session.end` | `session.end` | `reason` |
| `heartbeat` | `$.clock.every` | — |
| `turn.start` / `turn.complete` | 같은 이름 (main 루프만) | `turn.complete` 는 `summary`(답 텍스트 앞부분) |
| `tool.failure` | `classic.PostToolUseFailure` | `tool` |

질문과 권한 확인은 이벤트가 아니라 ask 로 다룬다 — `POST /v1/asks`(열기)와
`POST /v1/asks/{id}/close`(확정, `answered_by`: `session`·`doppel-ui`·`slack`·…)다 (§6).

### doppel → 세션 메시지 (attach stdout, 한 줄에 JSON 하나)

| kind | 처리 |
|---|---|
| `answer` | `{ kind, ask_id, tool_use_id, answers, answered_by }`. 보류된 질문의 늦은 답이다 — hook 이 `$.prompt.submit` 으로 답을 담은 새 턴을 열고 ask 를 `close` 한다. 닫히기 전에 attach 가 다시 열리면 다시 보낸다 |
| `context` | `$.session.append` 로 진행 중 턴에 meta 행을 넣는다 |
| `prompt` | `$.prompt.submit` 으로 새 턴을 연다 |

---

## 3. 질문 중계 — relay 계약

### 경주

`AskUserQuestion` 의 `tool.call` 은 plugin hook 들을 차례로 지나 core(터미널 기본
대화상자)에 닿는다. 모든 채널 plugin 의 hook 이 같은 일을 한다.

```ts
on('tool.call', { tool: 'AskUserQuestion' }, async ($, e, next) => {
  const posted = await channel.post($, e)            // 내 채널에 질문을 올린다
  if (!posted) return next(e)                        // 채널에 닿지 못함 → 기존 동작

  const winner = await Promise.race([
    next(e).then(result => ({ mine: false, result })),
    channel.answer($, posted).then(answers => ({ mine: true, answers })),
  ])
  return winner.mine
    ? { result: { questions: e.input.questions, answers: winner.answers } }
    : winner.result
})
```

- 내 채널 답이 이기면 `{ result }` 로 답한다. `next` 가 걸린 채 돌아가면 그 아래가
  중단된다 — 안쪽 plugin 들과 터미널 대화상자가 함께 닫힌다.
- `next(e)` 가 이기면 안쪽(다른 채널 또는 터미널)의 결과를 그대로 돌려준다.
- 경주는 대칭이라 plugin 순서와 무관하게 먼저 온 답 하나가 체인 전체의 결과가 된다.

### 모든 곳에 반영

경주에 진 plugin 은 최종 답을 `session.append` 의 tool-result 행(같은 `tool_use_id`)에서
읽어 자기 채널 글을 "○○에서 답변됨: …" 으로 갱신한다. 위에서 중단된 plugin 도 이 행으로
답을 안다. 답한 곳은 tool-result 행에 남기지 않으므로, 이긴 plugin 이 `ask.closed` 를 낼 때
자기 이름을 싣는다.

### 기다림과 보류

- 대기는 예산에 세지 않는 호출로 한다 (진행 중인 `next`·`$` 호출은 hook 예산에 들지 않는다).
  doppel 채널은 `$.process.run(['doppel', 'ask', 'wait', ...])` 를 쓴다. 호출당 최대 10분이라
  끝날 때까지 다시 부른다.
- 터미널이 있는 세션은 대화상자가 떠 있으므로 보류하지 않는다.
- 터미널이 없는 세션은 `ask_defer_after` 가 지나면 doppel hook 이 "사용자 판단 보류 중" 으로
  답한다. 나중에 온 답은 `answer` 메시지 → `$.prompt.submit` 으로 전달된다.

### 권한 확인

엔진 판정이 `ask` 일 때만 같은 경주를 한다.

- 원격 답이 거부면 `{ deny }` 로 끝난다.
- 원격 답이 허용이면 프롬프트 없이 도구를 실행해야 한다. 걸린 `next` 를 버리고 그
  `tool_use_id` 에 대해 `tool.check` 가 `allow` 로 답하게 한 뒤 `next(e)` 를 다시 부른다.
  재호출이 되지 않는 빌드에서는 터미널 기본 프롬프트 대신 doppel band(`AbovePrompt` 의
  허용·거부 버튼)를 터미널 surface 로 쓴다.

### 브리프를 다른 채널에도

doppel hook 이 체인 바깥에 있으면 `next({ ...e, input })` 으로 질문 설명에 추천 요약을
덧붙일 수 있다. 브리프가 준비될 때까지 질문이 늦어지므로 기본은 덧붙이지 않고, 브리프는
doppel UI 와 터미널 band 에만 붙인다.

---

## 4. 리액티브 구현

plugin 과 데몬 모두 "이벤트 스트림 → 변환 → 구독" 한 모양으로 짠다.

- **plugin (TypeScript)** — rxjs 를 번들하지 않는다. `Observable`·`Subject`·`map`·`filter`·
  `merge`·`scan` 만 가진 최소 구현을 plugin 끼리 공유하는 모듈로 둔다. `on(...)` 콜백들이
  나가는 스트림 하나로 합쳐지고, attach stdout 이 들어오는 스트림이 되어 `kind` 로 갈린다.
- **doppel 데몬 (Rust)** — `futures::Stream` 조합과 `tokio::sync::broadcast`. 세션 이벤트
  스트림을 세션별로 접어 상태([doppel.md §5](./doppel.md))를 만들고, 상태 변화에서 결정
  지점을 뽑아 브리프·inbox 로 잇는다.

---

## 5. 런타임 가정과 확인 결과

Claude Code 2.1.306 타입 선언으로 설계했고, 2026-10-21 headless 세션(`claude -p
--input-format stream-json --permission-prompt-tool stdio`, 기본 권한 프롬프트가
stdio 로 나가 응답 없이 대기하는 상태)에서 확인했다.

| 가정 | 결과 |
|---|---|
| `session.start` 에서 띄운 자식 프로세스가 세션 내내 살고, 세션이 끝나면 정리된다 | 확인 — 60초 동안 출력을 받았고 `session.end` 직전에 끝났다 |
| `$.http.fetch` 가 `socketPath` 로 Unix socket HTTP 서버에 닿는다 | 확인 |
| core 의 질문이 대기 중일 때 `next` 를 건 채 원격 답으로 돌아가면 그 답이 모델에 간다 | 확인 (headless). 터미널 대화상자가 화면에서 닫히는지는 대화형 세션에서 볼 일이다 |
| 권한 허용 시 걸린 `next` 를 버리고 `tool.check` allow + `next(e)` 재호출로 프롬프트 없이 실행된다 | 확인 — 두 번째 호출은 권한 요청을 다시 내지 않았다 |
| `$.prompt.submit` 이 idle 세션에 새 턴을 연다 | 확인 (stream-json 입력을 열어 둔 headless). 터미널 세션은 대화형 세션에서 볼 일이다 |
| `classic.Notification` 의 `notification_type` 이 권한·idle 을 구분한다 | 미확인 — 대화형에서만 발화한다. 권한 대기는 `permission.open` 으로 판정하므로 의존하지 않는다 |

doppel 이 띄우는 headless 세션은 `--input-format stream-json` 으로 stdin 을 열어 둔다.
그래야 턴이 끝나도 프로세스가 남아 늦은 답을 `$.prompt.submit` 으로 받을 수 있다.

---

## 6. doppel 데몬 API

데몬은 같은 라우터를 두 곳에서 연다.

| 리스너 | 주소 | 누가 |
|---|---|---|
| Unix socket | `~/.areum/doppel/doppel.sock` (권한 600) | plugin hook, `doppel` CLI, `doppel attach` |
| loopback TCP | `127.0.0.1:7319` (`--ui-port`) | 웹 UI |

- 단일 실행은 소켓이 보장한다. 소켓에 연결되면 이미 떠 있는 것이고, 연결이 거부되는 소켓
  파일은 지난 실행의 잔해라 지우고 다시 연다.
- TCP 리스너는 `Host` 가 `127.0.0.1:<port>`·`localhost:<port>` 일 때만, `POST` 는 `Origin` 이
  없거나 같은 출처이고 `X-Doppel-Client` 헤더가 있을 때만 받는다 — 다른 사이트가 브라우저를
  시켜 질문에 답하거나(DNS rebinding·CSRF) 데몬을 끄는 것을 막는다. `/v1/shutdown` 은 Unix
  socket 으로만 받는다.
- 본문은 JSON 이고, 오류는 `{ "ok": false, "error": { "kind", "message" } }` 와 상태 코드
  (`400` usage, `404` not_found, `409` conflict, `500` internal)다.

### 엔드포인트

| 메서드·경로 | 본문 → 응답 |
|---|---|
| `GET /v1/health` | → `{ "ok": true, "version", "pid" }` |
| `POST /v1/shutdown` | → `{ "ok": true }` 후 종료 |
| `POST /v1/events` | §2 의 세션 이벤트 → `202 { "ok": true }` |
| `GET /v1/sessions` | → `{ "sessions": [Session] }` (최근 이벤트 순) |
| `POST /v1/asks` | `{ session_id, tool_use_id, kind, questions }` → `Ask` (같은 `session_id`·`tool_use_id` 면 기존 것) |
| `GET /v1/asks?state=<state>` | → `{ "asks": [Ask] }` |
| `GET /v1/asks/{id}` | → `Ask` |
| `GET /v1/asks/{id}/wait?timeout_ms=<n>` | `open` 이 아니게 되거나 시간이 다 되면 → `Ask` |
| `POST /v1/asks/{id}/answer` | `{ answers, answered_by }` → `Ask` — `open`·`deferred` 일 때만, 아니면 `409` |
| `POST /v1/asks/{id}/defer` | → `Ask` — `open` 일 때만 |
| `POST /v1/asks/{id}/close` | `{ answers?, answered_by }` → `Ask` — 이미 `closed` 면 `409` |
| `GET /v1/attach/{session_id}` | WebSocket. doppel → 세션 메시지(§2) |
| `GET /v1/ui` | WebSocket. `{ "kind": "changed", "what": "sessions" \| "asks" }` 알림 |
| `GET /` | 웹 UI |

`Session`: `{ session_id, cwd, surface, launched_by, state, started_at, last_event_at,
ended_at, open_asks }` — `state` 는 [doppel.md §5](./doppel.md) 의 값.

`Ask`: `{ ask_id, session_id, tool_use_id, kind, questions, state, answers, answered_by,
created_at, answered_at, closed_at, brief }`. `brief` 는 데몬이 브리프를 쓰지 않으면 `null`,
쓰면 `{ state: pending | ready | failed, background, recommendation, new_opinion, confidence,
reversible, rationale, cited, error, accepted }` 다 ([doppel-judgment.md §5](./doppel-judgment.md)). `questions` 는 `AskUserQuestion` 입력 그대로
(`[{ question, header, multiSelect, options: [{ label, description }] }]`), `answers` 는
`{ "<question>": "<label>" | ["<label>", …] }`. 권한 확인(`kind: "permission"`)은 질문 하나
(`"<도구> 실행 허용: <입력 요약>"`)에 선택지 `allow`·`deny` 다.

### ask 상태

```
open ──answer──▶ answered ──close──▶ closed
  │                                    ▲
  ├──defer──▶ deferred ──answer──▶ answered (attach 로 세션에 전달)
  └───────────────close (다른 곳이 이김 · 세션 종료)───┘
```

- `answered` 는 doppel surface 가 답을 정했지만 세션의 hook 이 아직 확정하지 않은 상태다.
  확정은 hook 의 `close` 다 — hook 이 경주의 최종 판정자라서, doppel 이 답한 직후 터미널이
  먼저 이겼다면 `close` 가 터미널 답으로 덮는다.
- `deferred` 에서 답이 오면 doppel 은 그 세션의 attach 연결로 `{ "kind": "answer", ask_id,
  tool_use_id, answers }` 를 보낸다.
- 데몬이 멈추면 attach·UI WebSocket 도 닫힌다. plugin 은 attach 를 다시 띄우고, 다시 열린 연결에는
  아직 닫히지 않은 늦은 답이 먼저 간다.
- 세션이 `session.end` 를 보내면 남은 `open`·`deferred` ask 는 `closed`(`answered_by:
  "session-ended"`)가 된다.
- `close` 에 `answers` 가 있으면 doppel 은 판단 원장에 기록한다. 질문은 세션 transcript
  ingest 와 같은 키(`source: "session"`, `<session_id>:<tool_use_id>:<순번>`)로 넣어 나중의
  ingest 가 중복을 만들지 않는다. 권한은 터미널에서 답한 것(`answered_by: "session"`)은 넣지
  않는다 — transcript 의 `permissionDecision` 이 그 기록이다.

### 저장 스키마

판단 원장과 같은 파일(`~/.areum/doppel/doppel.db`)에 둔다. 세션의 상태는 저장하지 않고 이
사실들로 그때그때 판정한다 ([doppel.md §5](./doppel.md)).

```sql
CREATE TABLE sessions (
  session_id        TEXT PRIMARY KEY,
  cwd               TEXT,
  surface           TEXT NOT NULL DEFAULT 'unknown',   -- terminal | headless | unknown
  launched_by       TEXT NOT NULL DEFAULT 'user',      -- doppel | user
  started_at        TEXT NOT NULL,
  last_event_at     TEXT NOT NULL,
  last_heartbeat_at TEXT NOT NULL,
  turn_active       INTEGER NOT NULL DEFAULT 0,
  fail_streak       INTEGER NOT NULL DEFAULT 0,        -- 이번 턴의 도구 실패 수
  last_summary      TEXT,
  ended_at          TEXT,
  end_reason        TEXT,
  CHECK (surface IN ('terminal','headless','unknown')),
  CHECK (launched_by IN ('doppel','user')),
  CHECK (turn_active IN (0,1)),
  CHECK (fail_streak >= 0),
  CHECK (ended_at IS NULL OR turn_active = 0)
);

CREATE TABLE asks (
  id          TEXT PRIMARY KEY,                        -- ask-000001
  session_id  TEXT NOT NULL REFERENCES sessions(session_id),
  tool_use_id TEXT NOT NULL,
  kind        TEXT NOT NULL,                           -- ask | permission
  questions   TEXT NOT NULL,                           -- JSON, AskUserQuestion 입력 그대로
  state       TEXT NOT NULL,                           -- open | deferred | answered | closed
  answers     TEXT,                                    -- JSON
  answered_by TEXT,
  created_at  TEXT NOT NULL,
  answered_at TEXT,
  deferred_at TEXT,
  closed_at   TEXT,
  UNIQUE (session_id, tool_use_id),
  CHECK (kind IN ('ask','permission')),
  CHECK (state IN ('open','deferred','answered','closed')),
  CHECK (state NOT IN ('open','deferred') OR (answers IS NULL AND answered_by IS NULL)),
  CHECK (state <> 'answered' OR (answers IS NOT NULL AND answered_by IS NOT NULL)),
  CHECK ((state = 'closed') = (closed_at IS NOT NULL)),
  CHECK (state <> 'deferred' OR deferred_at IS NOT NULL)
);

CREATE TABLE briefs (
  ask_id         TEXT PRIMARY KEY REFERENCES asks(id),
  state          TEXT NOT NULL,                        -- pending | ready | failed
  background     TEXT,
  recommendation TEXT,                                 -- JSON answers
  new_opinion    TEXT,
  confidence     TEXT,                                 -- low | medium | high
  reversible     INTEGER,
  rationale      TEXT,
  cited          TEXT NOT NULL DEFAULT '[]',           -- 인용한 판례 id
  precedents     TEXT NOT NULL DEFAULT '[]',           -- 번들에 실은 판례 id
  command        TEXT NOT NULL,                        -- 실행한 명령 (프롬프트 버전의 대리값)
  error          TEXT,
  accepted       INTEGER,                              -- 확정된 답이 추천과 같은가
  created_at     TEXT NOT NULL,
  finished_at    TEXT,
  CHECK (state IN ('pending','ready','failed')),
  CHECK (confidence IS NULL OR confidence IN ('low','medium','high')),
  CHECK (reversible IS NULL OR reversible IN (0,1)),
  CHECK (accepted IS NULL OR accepted IN (0,1)),
  CHECK ((state = 'pending') = (finished_at IS NULL)),
  CHECK (state <> 'ready' OR (confidence IS NOT NULL AND rationale IS NOT NULL
                              AND (recommendation IS NOT NULL OR new_opinion IS NOT NULL))),
  CHECK (state <> 'failed' OR error IS NOT NULL),
  CHECK (accepted IS NULL OR (state = 'ready' AND recommendation IS NOT NULL))
);
```

### `doppel` CLI 의 짝

| 명령 | 하는 일 |
|---|---|
| `doppel daemon run [--ui-port N] [--no-ui] [--brief \| --brief-command "<cmd>"]` | 데몬을 이 프로세스에서 돌린다. 상태 기준값 `--stall-after-secs`·`--fail-streak`·`--heartbeat-timeout-secs`, 브리프 시간 제한 `--brief-timeout-secs` |
| `doppel daemon start \| stop \| status` | 분리 실행 · `/v1/shutdown` · `/v1/health` |
| `doppel attach --session <id>` | `/v1/attach/{id}` 를 열고 받은 메시지를 한 줄에 하나씩 stdout 으로 |
| `doppel ask wait <ask-id> [--timeout <초>] --json` | `/v1/asks/{id}/wait` 를 감싼다 — hook 이 예산에 세지 않고 기다리는 수단. stdout 은 `{"ok":true,"command":"ask wait","data":<Ask>}` 한 줄이고, 데몬이 답하면 `data.state` 가 아직 `open` 이어도 exit 0 이다 |
| `doppel ask list [--state <state>]` · `doppel ask answer <ask-id> <label>… [--answer "<q>=<label>"]` | inbox 를 터미널에서 (`answered_by: "doppel-cli"`) |
| `doppel session list` | 세션 상태 |
