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

모든 이벤트는 `{ "v": 1, "session_id", "at", "kind", ... }` 이다.

| kind | 보내는 hook | 추가 필드 |
|---|---|---|
| `session.start` | `session.start` | `cwd`, `surface`(`terminal`·`headless`), `launched_by`(`doppel`·`user`) |
| `session.end` | `session.end` | `reason` |
| `heartbeat` | `$.clock.every` | — |
| `turn.start` / `turn.complete` | 같은 이름 (main 루프만) | `turn.complete` 는 답 텍스트 요약 |
| `tool.failure` | `classic.PostToolUseFailure` | `tool` |
| `ask.open` | `tool.call {AskUserQuestion}` | `tool_use_id`, `questions` |
| `permission.open` | 권한 확인이 사람에게 넘어올 때 | `tool_use_id`, `tool`, `input` 요약 |
| `ask.closed` | `session.append` 의 tool-result 행 | `tool_use_id`, `answers`, `answered_by`(`terminal`·`doppel`·`slack`·…) |

### doppel → 세션 메시지 (attach stdout, 한 줄에 JSON 하나)

| kind | 처리 |
|---|---|
| `answer` | 대기 중인 질문이면 그 hook 이 경주에서 이긴다. 이미 보류된 질문이면 `$.prompt.submit` 으로 답을 담은 새 턴을 연다 |
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

## 5. 확인이 필요한 런타임 가정

Claude Code 2.1.306 타입 선언으로 설계했고 실세션 spike 가 남았다.

| 가정 | 틀렸을 때 |
|---|---|
| `session.start` 에서 띄운 `doppel attach` 가 세션 내내 산다 | `$.http.fetch` long-poll 로 들어오는 방향을 바꾼다 |
| `next` 를 건 채 답하면 터미널 `AskUserQuestion` 대화상자가 닫힌다 | 터미널도 doppel band 로 질문을 그린다 |
| 권한 허용 시 `next(e)` 재호출 + `tool.check` allow 로 프롬프트 없이 실행된다 | 권한은 doppel band 로 그린다 |
| `$.prompt.submit` 이 터미널 세션에서 새 턴을 연다 | 터미널 세션의 늦은 답은 기록만 한다 |
| `classic.Notification` 의 `notification_type` 이 권한·idle 을 구분한다 | 권한 대기는 `PermissionRequest` 로만 판정한다 |
