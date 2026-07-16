# Discord Agent CLI — 조사 · 설계 노트

에이전트(Claude Code, Codex 등)가 **봇 토큰으로 Discord와 소통**하는 CLI를 만들기 위한 조사·설계 정리.
로그인은 수단일 뿐이고 목표는 "동작"(메시지 보내기/받기).

---

## 1. 결론 요약

- **OAuth 유저 로그인이 아니라 Bot 토큰 방식이 정답.** 에이전트는 사람처럼 브라우저 승인을 못 하고, OAuth 유저 토큰으로는 "메시지 전송" API 자체가 존재하지 않는다.
- 봇으로 동작하므로 메시지는 "나"가 아니라 "봇 이름"으로 전송된다.
- CLI 표면: `discord send`, `discord read`, `discord wait` 정도의 얇은 명령으로 충분.
- 받기는 **REST 폴링**이 CLI 원샷에 적합. 실시간 상주가 필요하면 그때 gateway(websocket).

---

## 2. Discord 인증의 핵심 제약 (사실 정리)

Discord는 인증 수단이 두 종류이고 할 수 있는 일이 다르다.

| 기능 | OAuth2 (유저 로그인) | Bot 토큰 |
|---|---|---|
| 로그인 방식 | 브라우저에서 권한 승인 | 개발자 포털에서 발급 |
| 내 정보 조회 (`/users/@me`) | ✅ | ❌ (봇 자신만) |
| 내 서버 목록 (`/users/@me/guilds`) | ✅ | ❌ |
| **메시지 전송** | ❌ (엔드포인트 없음) | ✅ (봇 이름으로) |
| 채널/멤버 관리 | ❌ | ✅ (권한 범위 내) |
| 게이트웨이(실시간 이벤트) | 제한적 | ✅ |

주의사항:
- **OAuth로 로그인해 "내 계정으로 메시지 보내는" CLI는 불가능** — Discord에 그런 API가 없다.
- **유저 토큰을 뽑아 자동화(셀프봇)는 ToS 위반 → 계정 밴 대상.** 절대 채택하지 않는다.
- OAuth로는 읽기 위주(내 정보, 서버/채널 목록, 연결 계정)만 가능.
- REST(`GET /channels/{id}/messages`)로 과거 메시지 본문을 읽을 때 **MESSAGE CONTENT 특권 인텐트가 필요한지는 아직 미검증** — 첫 live 실행 시 확인 필요(§6).

---

## 3. 봇 준비 (수동, 개발자 포털)

CLI가 대신할 수 없는 1회성 수동 작업:

1. https://discord.com/developers/applications 에서 New Application 생성
2. Bot 탭에서 봇 추가 → **Token** 발급 (한 번만 노출되므로 안전 저장)
3. **Privileged Gateway Intents → MESSAGE CONTENT INTENT 활성화**
   - gateway로 메시지 본문을 실시간 수신하려면 필수.
   - REST로 과거 메시지 본문을 읽는 데도 필요한지는 실사용 시 검증 필요(§2, §6).
4. OAuth2 → URL Generator에서 `bot` scope + 권한(최소: View Channels, Send Messages, Read Message History) 선택 → 생성된 초대 링크로 봇을 **대상 서버에 초대**
5. 봇이 접근할 채널의 **Channel ID** 확보 (Discord 개발자 모드 → 채널 우클릭 → ID 복사)

---

## 4. CLI 동작 설계 (초안)

에이전트가 Bash로 호출하는 얇은 명령 집합:

```bash
discord send <channel_id> "<text>"          # 메시지 전송
discord read <channel_id> [--limit N] [--after <message_id>]   # 최근 메시지 조회
discord wait <channel_id> [--after <id>] [--timeout N]         # 새 메시지 올 때까지 폴링(선택)
```

- **전송**: `POST /channels/{channel_id}/messages`
- **조회**: `GET /channels/{channel_id}/messages?limit=N&after=<id>`
- **대기**: 조회를 폴링 루프로 감싸 새 메시지 등장 시 반환 (상주 프로세스 불필요)
- 출력 포맷·플래그 계약은 §5 참조

### 받기 방식: REST 폴링 vs Gateway
- **REST 폴링** — 원샷 CLI에 적합. 지속 연결 불필요, 구현 단순. 지연·rate limit만 관리.
- **Gateway(websocket)** — 실시간이지만 상주 프로세스 필요. 봇 상주가 목적일 때만.
- 1차 구현은 **REST 폴링** 권장.

이 초안은 §5의 확정 계약으로 구체화되었다. 아래 §5가 실제 CLI 표면·출력 스펙의 SSOT다.

---

## 5. 인터페이스 계약 (확정)

이 CLI의 1차 사용자는 **AI 에이전트**다. 아래 절만 보고 파싱 코드를 작성할 수 있어야 한다.
근거: `tools/discord/src/cli.rs`(인자 파싱), `tools/discord/src/output.rs`(JSON 봉투·exit code).

기본 출력은 **사람용 텍스트**다. **에이전트는 항상 `--json`을 붙여** 아래 봉투 계약을 받는다.

### CLI surface

```
discord [--token <TOKEN>] [--config <PATH>] [--json] <COMMAND>
  send <CHANNEL> [BODY] [--reply-to <MSG_ID>] [--file <PATH>]...   # BODY 생략/'-' → stdin. --text <TEXT>는 BODY와 상호배타. --file 최대 10회 반복
  read <CHANNEL> [--after <MSG_ID>] [--limit N]          # limit 기본 50, 1..=100
  wait <CHANNEL> [--after <MSG_ID>] [--timeout SECS] [--interval SECS] [--limit N]  # 기본 60/5/50
```

- `CHANNEL` = raw channel id 또는 config alias.
- `--token`/`--config`/`--json`은 전역 플래그로, 서브커맨드 앞뒤 어디서나 지정 가능.
- 긴 본문은 `echo "..." | discord send ops -` (셸 이스케이프 회피).
- `--reply-to <MSG_ID>`는 같은 채널의 메시지에 답글을 단다. 대상 메시지가 삭제되었으면 Discord가 400을 반환해 `kind:"api"`/exit 4로 매핑된다. 빈 문자열(`--reply-to ""`)은 usage 오류(exit 2).

#### 파일 첨부 (`--file`)

`--file <PATH>`를 반복해 최대 10개 파일을 첨부한다. 캡션(메시지 본문)과 파일의 조합 규칙:

| 파일 | BODY / --text | 결과 캡션 |
|---|---|---|
| 있음 | 없음 | 빈 캡션. **stdin을 읽지 않는다** (파이프가 없어도 블로킹 없음) |
| 있음 | `'-'` | stdin에서 캡션을 읽는다 (명시적) |
| 있음 | 있음 | 해당 값이 캡션 (2000자 제한·BODY↔--text 상호배타 유지) |
| 없음 | — | 기존과 동일 (빈 본문 거부, BODY 생략/`'-'` → stdin 폴백) |

파일 검증(모두 usage 오류, exit 2, 메시지에 경로 포함):

- 파일 11개 이상 → exit 2 (Discord 메시지당 첨부 10개 상한).
- 읽기 실패(부재·권한) → exit 2.
- 0바이트 파일 → exit 2 (업로드 실수 fail-fast).
- 파일 크기 100 MiB 초과 → exit 2 (전량 메모리 적재 구조의 새너티 상한; Discord 최고 부스트 티어 상한과 정합).

전송 시 `filename`은 경로의 마지막 컴포넌트만 노출하고, MIME 타입은 확장자로 추론한다(모르면 `application/octet-stream`). 서버 측 업로드 상한(요금제별)을 넘으면 Discord가 413을 반환해 `kind:"api"`/exit 4로 매핑된다. 업로드가 성공하면 `data.attachments`에 Discord CDN `url`이 채워져 돌아온다(아래 공통 스키마와 동일 shape).

### JSON 봉투 (stdout, `--json` 시)

성공:
```json
{"ok":true,"command":"send|read|wait","data":{...}}
```

실패:
```json
{"ok":false,"command":"send|read|wait","error":{"kind":"usage|config|auth|api|rate_limit|network|internal","message":"...","http_status":401,"retry_after_ms":1200}}
```
`http_status`/`retry_after_ms`는 값이 있을 때만 포함된다(해당 없으면 키 자체가 없음).

커맨드별 `data`:
- `send.data` = `{message_id, channel_id, timestamp, attachments}`
- `read.data` = `{channel_id, count, cursor, messages}`
- `wait.data` = `{channel_id, count, cursor, timed_out, messages}`

공통:
- `messages`는 `[{id, channel_id, author:{id,username,bot}, content, timestamp, attachments}]`.
- `messages`는 **오름차순(과거→최신)** 정렬.
- `content`는 빈 문자열일 수 있다(첨부·임베드 전용 메시지).
- `cursor`는 값이 없으면 `null`이며 키 자체는 항상 존재한다(생략되지 않음).
- `attachments`는 `[{id, filename, size, url, content_type?}]`. 첨부가 없으면 `[]`(키 자체는 항상 존재). `content_type`은 Discord가 값을 주지 않으면 키 자체가 생략된다.
- `send.data.attachments`도 동일한 shape이며 항상 배열이다 — 텍스트만 보낸 경우엔 `[]`, `--file`로 업로드하면 각 파일의 CDN `url`이 담겨 돌아온다.
- 첨부의 CDN `url`은 **`--json` 출력에만** 노출된다. 기본 human 출력은 첨부가 있을 때 파일명만 한 줄(`attachments: a.png, b.png`) 덧붙이고 url은 표시하지 않는다.

### 커서 시맨틱 (stateless 폴링)

에이전트는 이전 출력의 `.data.cursor`를 다음 호출의 `--after`에 그대로 전달하면 된다.

- `cursor` = 이번 호출로 회수한 메시지들의 **max snowflake id**(수치 비교 기준).
- 회수한 메시지가 0건이면 입력받은 `--after` 값을 그대로 echo(입력이 없었으면 `null`).
- 에이전트 쪽에서 별도 상태(마지막으로 본 id 등)를 계산·저장할 필요가 없다.

### exit code

| code | 의미 | 발생 조건 | 에이전트 기대 행동 |
|---|---|---|---|
| 0 | 성공 | 정상 처리. `wait` 타임아웃(`data.timed_out:true`)도 포함 — 타임아웃은 정상 흐름 | 그대로 진행 |
| 1 | 내부 오류 | 예기치 못한 내부 실패 | 재시도 무의미. 로그를 남기고 에스컬레이션 |
| 2 | usage 오류 | 인자 조합이 스펙 위반(예: BODY와 --text 동시 지정), 또는 clap 소유 usage 에러 | 호출 인자를 고쳐서 재호출. 그대로 재시도 금지 |
| 3 | config·인증 오류 | 토큰 없음, 401, 403 | 토큰/config 재점검 없이 재시도 무의미 |
| 4 | API 오류 | 429를 제외한 4xx·5xx, 응답 스키마 불일치 | 채널 id·권한 등 요청 자체를 재검토. 단순 재시도 비권장 |
| 5 | rate limit 소진 | 내부 자동 재시도(최대 3회) 후에도 429 지속 | `error.retry_after_ms`만큼 대기 후 재호출 |
| 6 | 네트워크 오류 | 연결 실패·타임아웃 등 전송 계층 문제 | 잠시 후 재시도 가능 |

### clap 경계 (봉투 없는 유일한 예외)

미지 플래그·필수 인자 누락 등 **clap이 소유한 usage 에러**는 `--json` 여부와 무관하게 봉투 없이 stderr 출력 + exit 2로 끝난다.
`--help`/`--version`도 비-JSON stdout이다.

에이전트 규칙:
- `--json` 시 stdout은 항상 봉투 JSON이다 (유일한 예외: clap 소유 usage 에러·`--help`/`--version`은 봉투 없음).
- 기본(human) 모드의 에러는 stderr로 가고 stdout은 비어 있다 — 진단은 stderr라는 원칙.

### config

경로: `~/.areum/discord/config.json` (파일 권한 600 권장 — 아니면 stderr 경고만 출력하고 계속 진행).

```json
{"token": "...", "channels": {"alias": "channel_id"}}
```
두 필드 모두 선택(optional). 파일이 없어도 에러가 아니다(토큰이 `--token`/env로 올 수 있으므로).

토큰 우선순위: `--token` > `DISCORD_BOT_TOKEN` env > `config.token`. 전부 없으면 exit 3.

디렉토리·파일은 CLI가 생성하지 않는다 — 사용자가 1회 수동 셋업한다.

### rate limit

429 응답 시 `Retry-After`만큼 대기 후 자동 재시도, 최대 3회. 소진되면 exit 5 + `error.retry_after_ms` 반환.
에이전트는 그 값만큼 backoff 후 재호출하면 된다.

### 에이전트 사용 예

에이전트는 항상 `--json`을 붙여 jq로 파싱한다. send로 보내고, read로 초기 커서를 얻은 뒤, wait을 반복하며 `.data.cursor`를 이어받는 폴링 루프:

```bash
discord send ops "빌드 시작" --json

cursor=$(discord read ops --limit 1 --json | jq -r '.data.cursor')

while :; do
  out=$(discord wait ops --after "$cursor" --timeout 60 --json)
  cursor=$(echo "$out" | jq -r '.data.cursor')
  echo "$out" | jq -r '.data.messages[] | "\(.author.username): \(.content)"'
done
```

---

## 6. 설계 결정 현황

**확정**
- 스택: Rust — 이 레포(`areum-lab-tools`) `tools/discord` (workspace tool crate).
- 토큰 저장 위치: `~/.areum/discord/config.json`.
- 채널 지정 방식: raw channel_id와 config alias(`channels` 맵) 병용.
- MCP 대안: 채택하지 않음 — CLI로 확정.

**열림**

1. **봇 재사용 vs 신규 봇**
   - 기존 yora#8040 봇 재사용 → 세팅 0, 단 메시지 주체가 yora로 섞임.
   - 신규 "agent" 봇 → 메시지 출처가 명확히 구분됨(추천 후보). 포털 세팅 1회 필요.
2. **REST read의 MESSAGE CONTENT intent 필요 여부** — `GET /channels/{id}/messages`로 과거 메시지 본문을 읽는 데 특권 인텐트가 필요한지 문서상 불명확. 첫 live 실행 시 검증 필요(§2 주의사항과 연결).

---

## 7. 참고: 기존 yora 환경 (조사 결과)

새 작업은 별도 레포(`/Users/kys0213/workspace/areum-lab-tools`)에서 진행하되, 아래는 참고용.

- 기존 yora 레포(`/Users/kys0213/workspace/yora`)는 셸 스크립트 + Python MCP 서버 구성.
- Discord 봇 인프라가 이미 가동 중: Hermes 게이트웨이(launchd `ai.hermes.gateway`)가 yora#8040 봇 관리.
  - 관련 상태 파일: `~/.hermes/` 아래 `gateway.pid`, `discord_threads.json`, `channel_directory.json`, `gateway_state.json`.
  - 봇 토큰·허용 유저: `~/.hermes/.env`의 `DISCORD_BOT_TOKEN`, `DISCORD_ALLOWED_USERS`, `DISCORD_HOME_CHANNEL`.
- MCP 서버 패턴 예시: `mcp/cloud-agents/server.py` — stdlib-only stdio MCP 서버로 `delegate_to_claude`/`delegate_to_codex` 노출. 신규 CLI를 MCP로 갈 경우 참고 템플릿.

---

## 8. 다음 단계 제안

CLI 인터페이스 계약(§5)과 크레이트 골격은 확정됐다. 남은 것은 실사용 준비:

1. §6의 남은 열린 결정 확정 — 봇 재사용 vs 신규 봇.
2. 봇 포털 세팅(§3) — 신규 봇 채택 시 1회 수동 작업.
3. 첫 live 실행으로 REST read의 MESSAGE CONTENT intent 필요 여부 검증(§6-2, §2).
