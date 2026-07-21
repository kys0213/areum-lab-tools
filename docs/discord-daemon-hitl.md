# Discord Daemon HITL — 설계 스펙

`discord` CLI에 HITL(Human-In-The-Loop) 기능을 추가하는 설계. 에이전트가 버튼+모달 질문을 Discord로 보내고, 사람의 응답을 비동기로 받는다.

---

## 1. 목적·범위

- **목적**: 에이전트가 작업 중 사람의 확인/선택이 필요할 때, Discord 메시지(버튼 또는 모달)로 질문을 보내고 그 응답을 CLI로 수신한다.
- **이번 범위**: SP1(데몬 골격) + SP2(`ask` 명령).
- **후속(이번 범위 밖)**: SP3(메시지 캐시), SP4(이벤트 구독). 단 SP1에서 만드는 SQLite DB는 같은 파일에 테이블을 추가하는 방식으로 SP3/SP4까지 확장 가능해야 한다 — 별도 저장소나 스키마 마이그레이션 체계를 새로 도입하지 않는다.

---

## 2. 아키텍처

`discord` CLI(단명 프로세스)와 `discord daemon`(상주 프로세스)은 **SQLite(WAL 모드) 파일 하나로만 통신**한다. 둘 사이에 소켓·IPC·HTTP는 없다.

```
                         ┌────────────────────────────┐
                         │   Discord Gateway (WS)      │
                         │   INTERACTION_CREATE 등     │
                         └───────────┬────────────────┘
                                     │ intents=0
                                     ▼
┌───────────────┐   REST    ┌──────────────────────────┐
│  discord CLI   │──────────▶│   discord daemon (상주)   │
│  (단명 프로세스) │  버튼메시지 │   daemon/gateway.rs       │
│  ask create    │  전송     │   daemon/interactions.rs  │
│  ask result    │           │   pidfile + 시그널 수명   │
│  ask wait      │           └─────────┬────────────────┘
└───────┬────────┘                     │
        │                              │ rusqlite (WAL)
        │        ┌─────────────────────┴───────────────┐
        └───────▶│  ~/.areum/discord/discord.db          │
   INSERT/SELECT │  asks 테이블, schema_version 테이블   │
   (common/store)│  (SP3/SP4은 테이블 추가로 확장)       │
                 └────────────────────────────────────────┘
```

- **CLI**: 자체 `HttpDiscordApi`(기존 REST 클라이언트, `tools/discord/src/common/http.rs`)로 버튼이 달린 메시지를 전송하고, 같은 트랜잭션 흐름 안에서 `asks` 테이블에 INSERT한다. `ask_id`(= 전송된 메시지 id)를 즉시 반환한다.
- **데몬**: `twilight-gateway`로 웹소켓 연결을 유지한다(intents=0 — INTERACTION_CREATE는 intent 무관하게 항상 수신되므로 별도 특권 인텐트가 필요 없다). `INTERACTION_CREATE` 이벤트를 받으면 3초 내에 callback을 보내고 DB를 갱신한다.
- CLI와 데몬은 서로의 존재를 직접 호출하지 않는다 — DB 행이 유일한 접점이다.

---

## 3. 데이터 모델

DB 파일: `~/.areum/discord/discord.db` (WAL 모드).

### `asks` 테이블

| 컬럼 | 타입 | 설명 |
|---|---|---|
| `ask_id` | TEXT PK | Discord 메시지 id(snowflake, 문자열). CLI가 버튼 메시지 전송 응답으로 받은 id를 그대로 사용 |
| `channel_id` | TEXT NOT NULL | 질문을 보낸 채널 |
| `question` | TEXT NOT NULL | 질문 본문 |
| `options` | TEXT NOT NULL | 선택지 목록 (JSON 배열) |
| `allow_text` | INTEGER NOT NULL | 자유 텍스트 응답 허용 여부 (0/1) |
| `status` | TEXT NOT NULL | `pending` \| `answered` \| `timed_out` |
| `kind` | TEXT NOT NULL | `choice` \| `text` — 실제로 채택된 응답의 종류 |
| `value` | TEXT NULL | 채택된 응답 값 (선택지 텍스트 또는 자유 텍스트) |
| `answered_by` | TEXT NULL | 응답한 Discord 유저 id |
| `created_at` | INTEGER NOT NULL | 생성 시각 (unix epoch) |
| `timeout_at` | INTEGER NOT NULL | 마감 시각 (unix epoch) |
| `answered_at` | INTEGER NULL | 응답 확정 시각 (unix epoch) |

`status`/`kind`/`value`/`answered_by`/`answered_at`는 생성 시 `status='pending'`, 나머지 NULL로 시작해 첫 응답 또는 타임아웃 시 채워진다.

### `schema_version` 테이블

단일 행으로 현재 스키마 버전을 기록한다. 데몬 기동 시 이 값을 읽어 필요한 마이그레이션(있다면)을 적용한 뒤 갱신한다. SP3/SP4는 새 테이블을 추가하면서 이 버전을 올리는 방식으로 확장한다.

### 보존 정책 (retention)

`retention_days`(기본 30) — 데몬 로직 상수/설정값으로, `created_at` 기준 보존 기간을 넘긴 `asks` 행을 삭제 대상으로 삼는다. 데몬이 기동할 때 1회, 이후 하루 1회 주기로 만료 행을 삭제한다.

---

## 4. 상호작용 흐름 (ask 생애주기)

1. **생성**: `discord ask create ...` 실행.
   - CLI가 데몬 pidfile을 확인해 프로세스 생존 여부를 시그널로 검사한다. 데몬이 떠 있지 않으면 **즉시 에러로 실패한다** (Fail Fast) — 응답을 받을 수 없는 질문을 만들지 않는다.
   - `HttpDiscordApi`로 버튼(선택지별) 및 필요 시 모달 트리거가 달린 메시지를 채널에 전송한다.
   - 전송 성공 시 반환된 메시지 id를 `ask_id`로 `asks`에 `status='pending'` INSERT.
   - CLI는 결과를 기다리지 않고 `ask_id`를 즉시 반환한다(비동기 생성).
2. **버튼 클릭**: 사람이 Discord 클라이언트에서 버튼을 누르면 Discord가 `INTERACTION_CREATE`를 게이트웨이로 데몬에 전달한다.
   - `custom_id`를 파싱한다: `ask:<ask_id>:opt:<index>` (선택지 응답) 또는 `ask:<ask_id>:text` (자유 텍스트 모달 오픈 트리거).
   - **선택지 응답**: `daemon/interactions.rs`가 `UPDATE asks SET status='answered', kind='choice', value=<option>, answered_by=<user_id>, answered_at=<now> WHERE ask_id=? AND status='pending'` 조건부 UPDATE를 수행한다. 영향받은 행이 1이면 **첫 응답으로 채택**된 것 — 3초 내에 `UPDATE_MESSAGE`(interaction response type 7)로 버튼을 비활성화한 확인 메시지를 회신한다.
   - **자유 텍스트**: `ask:<ask_id>:text` 트리거를 받으면 `MODAL`(type 9) 응답으로 텍스트 입력 모달을 띄운다. 사용자가 제출하면 별도의 `MODAL_SUBMIT` interaction이 도착하고, 동일한 조건부 UPDATE(`kind='text'`)로 첫 제출을 채택한 뒤 확인 응답을 보낸다.
   - **마감 후 클릭**: 조건부 UPDATE의 영향 행이 0이면(이미 `answered` 또는 `timed_out`) 채택하지 않는다 — ephemeral 메시지(`flags: 64`)로 "이미 마감/응답됨"을 안내한다.
   - 이 조건부 UPDATE(`WHERE status='pending'`)가 동시 클릭에 대한 **유일한 동시성 해소 지점**이다 — 먼저 SQLite에 반영된 트랜잭션이 이긴다.
3. **타임아웃 마감**: 데몬이 `asks WHERE status='pending' AND timeout_at <= now`를 주기적으로 스캔한다. 마감 대상을 찾으면 `status='timed_out'`로 UPDATE하고, **봇 토큰 기반 일반 메시지 편집**(interaction 토큰이 아닌 REST 메시지 수정)으로 원본 메시지의 버튼을 비활성화한다 — interaction 토큰은 생성 후 15분이면 만료되므로 이 경로에는 쓸 수 없다.
4. **조회**: `discord ask result <ask_id>`는 `asks`를 1회 SELECT해 현재 상태를 그대로 반환한다(폴링 없음).
5. **대기**: `discord ask wait <ask_id>`는 상태가 `pending`이 아니게 될 때까지 폴링 블로킹한다(`--timeout`, `--interval`). 기존 `discord wait` 규약과 동일하게, 타임아웃에 도달해도 **exit 0의 정상 종료**로 처리한다(에러 아님) — 응답이 아직 없다는 것도 유효한 결과다.

---

## 5. 명령 표면

기존 `discord` CLI(`tools/discord/src/cli.rs`)에 아래 서브커맨드가 추가된다.

```
discord daemon start [--foreground]     # 데몬 기동 (기본: 백그라운드로 detach, --foreground는 전경 실행)
discord daemon stop                     # pidfile 기준으로 시그널을 보내 종료
discord daemon status                   # pidfile + 프로세스 생존 여부 조회

discord ask create <CHANNEL> <QUESTION> [--option <TEXT>]... [--allow-text] [--timeout <SECS>]
                                         # 버튼 메시지 전송 + asks INSERT, ask_id 즉시 반환
                                         # 데몬 미기동 시 즉시 에러(Fail Fast)
discord ask result <ASK_ID>             # asks 1회 SELECT, 현재 상태 반환
discord ask wait <ASK_ID> [--timeout <SECS>] [--interval <SECS>]
                                         # status != pending 될 때까지 폴링 블로킹
                                         # 타임아웃은 exit 0 정상 종료 (기존 wait 규약)
```

- `<CHANNEL>`은 기존 `send`/`read`/`wait`와 동일하게 raw channel id 또는 config alias.
- `--option`은 반복 지정해 선택지 목록(JSON 배열로 저장)을 구성한다.
- `--allow-text`를 주면 버튼 응답 외에 자유 텍스트(모달) 응답도 허용한다.
- `custom_id` 값은 100자 한도 내에서 `ask:<ask_id>:opt:<index>` 또는 `ask:<ask_id>:text` 규약을 따른다.

---

## 6. 모듈 구조

`.claude/rules/module-structure.md`의 3층 구조(배선/명령 표면 + 기능 디렉토리 + `common/`)를 따른다.

```
tools/discord/src/
├── main.rs
├── cli.rs                     # daemon/ask 서브커맨드 표면 추가
├── commands/
│   ├── daemon.rs              # daemon start/stop/status — 데몬 수명 관리
│   └── ask.rs                 # ask create/result/wait
├── daemon/
│   ├── gateway.rs              # twilight-gateway Shard 루프를 감싸는 래퍼(웹소켓 연결·재접속·이벤트 수신)
│   └── interactions.rs         # 순수 로직 — INTERACTION_CREATE 이벤트(JSON) 입력 → DB 갱신 + REST 응답 출력
│                                 # 웹소켓 의존성 없이 단위 테스트 가능
└── common/
    └── store.rs                # rusqlite 래퍼 — CLI(commands/ask.rs)와 데몬(daemon/interactions.rs) 양쪽이 공유
```

- `daemon/interactions.rs`는 웹소켓 이벤트를 직접 수신하지 않는다 — `daemon/gateway.rs`가 파싱한 이벤트를 넘겨받아 DB/REST만 다룬다. 이 분리가 §8 단위 테스트 전략의 전제다.
- `common/store.rs`는 CLI와 데몬 양쪽이 쓰는 공유 기반이므로 `common/`에 둔다(module-structure 규칙 4 — 둘 이상의 기능이 공유하는 기반).
- 신규 의존성: `twilight-gateway`, `twilight-model`. REST 전송은 기존 `HttpDiscordApi`(`tools/discord/src/common/http.rs`, `reqwest` 기반)를 그대로 재사용하며 `twilight-http`는 도입하지 않는다. DB는 `rusqlite`(`bundled` feature)를 사용한다.

---

## 7. 외부 제약 (리서치 확정 사실)

- 엔드포인트(HTTP Interactions Endpoint)를 설정하지 않으면 interaction은 게이트웨이의 `INTERACTION_CREATE` 이벤트로 수신된다. — https://docs.discord.com/developers/interactions/receiving-and-responding
- `INTERACTION_CREATE`는 intent와 무관하게 항상 수신되는 이벤트다 → 데몬은 `intents=0`으로 연결한다. — https://docs.discord.com/developers/events/gateway-events
- interaction에는 **3초 내에** `POST /interactions/{id}/{token}/callback`으로 응답해야 하며, 초과하면 토큰이 무효화된다. 버튼 응답은 `UPDATE_MESSAGE`(type 7)로 메시지 갱신과 컴포넌트 비활성화를 동시에 처리할 수 있다. 모달은 `MODAL`(type 9) 응답으로 띄우고, 제출은 별도의 `MODAL_SUBMIT` interaction으로 도착한다. interaction 토큰의 수명은 15분이다. — https://docs.discord.com/developers/interactions/receiving-and-responding
- Identify는 24시간당 1000회 제한이 있고(초과 시 토큰이 리셋됨) → **Resume을 우선**하고 재접속 시 지수 백오프를 필수로 둔다. 재접속에는 `resume_gateway_url`을 사용한다. close code `4004`/`4013`/`4014`는 재시도해서는 안 된다. — https://docs.discord.com/developers/events/gateway , https://docs.discord.com/developers/topics/opcodes-and-status-codes
  - 이 상태 머신(Resume/재접속/backoff)은 `twilight-gateway` 라이브러리가 내부적으로 담당한다. 우리 코드(`daemon/gateway.rs`)는 재시도 불가 close code를 받았을 때 데몬을 종료 처리하는 부분만 책임진다.
- `custom_id`는 1~100자, 하나의 action row에 버튼은 최대 5개까지 배치할 수 있다. — https://docs.discord.com/developers/components/reference
- 모달의 Text Input은 현재 `Label`(component type 18)로 감싸는 방식이 표준이며, 구 Action Row로 감싸는 방식은 deprecated 상태다. **이 부분은 구현 중 실제 API 호출로 검증이 필요한 항목으로 별도 표시한다** (§9). — https://docs.discord.com/developers/components/reference
- 게이트웨이 명령 전송에는 60초당 120개의 rate limit이 적용된다. — https://docs.discord.com/developers/events/gateway

---

## 8. 테스트 전략

- **단위 테스트**: `daemon/interactions.rs`는 웹소켓 없이 순수 함수로 테스트한다 — INTERACTION_CREATE 이벤트에 대응하는 JSON(또는 그 파싱 결과 구조체)을 입력으로 주고, DB 상태 변화와 반환되는 REST 응답 페이로드를 검증한다. 조건부 UPDATE의 동시성 해소(먼저 온 요청만 채택)를 케이스로 다룬다.
- **mock 웹소켓**: `daemon/gateway.rs`는 `twilight-gateway`의 Shard 루프를 감싸는 얇은 래퍼이므로, 실제 연결 대신 이벤트를 주입할 수 있는 목/페이크 소스로 이벤트 수신 → `daemon/interactions.rs` 호출 배선을 검증한다. 게이트웨이 연결·재접속 자체(twilight 라이브러리 책임)는 이 레벨에서 재검증하지 않는다.
- **라이브 E2E**: 실제 Discord 앱/봇 토큰으로 데몬을 기동하고 `ask create` → 실제 버튼 클릭 → `ask result`/`ask wait`까지 왕복하는 시나리오를 수동 또는 별도 게이트로 분리해 실행한다. 단위/mock 테스트와는 디렉토리 또는 실행 방식으로 분리해, 일반 CI 실행에는 포함하지 않는다.

---

## 9. 미해결·후속

- **SP3 (메시지 캐시)**: 이번 범위 밖. 같은 `discord.db`에 캐시용 테이블을 추가하는 방식으로 확장한다.
- **SP4 (이벤트 구독)**: 이번 범위 밖. 마찬가지로 같은 DB에 테이블을 추가해 확장한다.
- **모달 Text Input의 `Label`(type 18) 래핑**: 문서상 현행 표준으로 확인되었으나, 구현 단계에서 실제 모달 생성/제출 호출로 동작을 검증해야 한다. 검증 결과가 문서와 다르면 §7의 해당 항목과 §4의 자유 텍스트 흐름을 갱신한다.
