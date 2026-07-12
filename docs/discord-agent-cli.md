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

---

## 3. 봇 준비 (수동, 개발자 포털)

CLI가 대신할 수 없는 1회성 수동 작업:

1. https://discord.com/developers/applications 에서 New Application 생성
2. Bot 탭에서 봇 추가 → **Token** 발급 (한 번만 노출되므로 안전 저장)
3. **Privileged Gateway Intents → MESSAGE CONTENT INTENT 활성화**
   - gateway로 메시지 본문을 실시간 수신하려면 필수.
   - REST(`GET /channels/{id}/messages`)로 과거 메시지 본문을 읽는 것은 채널 읽기 권한이 있으면 가능 — 구현 시 실제 확인 필요.
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
- 출력은 JSON(에이전트 파싱 친화) 기본, `--text`로 사람용 포맷 옵션 고려

### 받기 방식: REST 폴링 vs Gateway
- **REST 폴링** — 원샷 CLI에 적합. 지속 연결 불필요, 구현 단순. 지연·rate limit만 관리.
- **Gateway(websocket)** — 실시간이지만 상주 프로세스 필요. 봇 상주가 목적일 때만.
- 1차 구현은 **REST 폴링** 권장.

---

## 5. 열린 설계 결정 (작업 전 확정 필요)

1. **봇 재사용 vs 신규 봇**
   - 기존 yora#8040 봇 재사용 → 세팅 0, 단 메시지 주체가 yora로 섞임.
   - 신규 "agent" 봇 → 메시지 출처가 명확히 구분됨(추천 후보). 포털 세팅 1회 필요.
2. **스택**: Node(v22, `discord.js` 또는 순수 fetch) vs Python(3.14, 순수 REST). 의존성 최소화 원하면 순수 REST 한 파일.
3. **토큰 저장 위치**: 전용 config(`~/.areum/` 등) vs 환경변수. 시크릿이므로 파일 권한 `600`.
4. **채널 지정 방식**: raw channel_id vs 별칭(alias) 매핑 파일.
5. **MCP 대안**: 순수 CLI 대신 MCP 서버로 노출하면 Claude Code/Codex에 툴로 직접 붙일 수 있음(기존 `mcp/cloud-agents/` 패턴과 동일). 단 사용자는 "CLI로 소통"을 명시 → CLI 우선.

---

## 6. 참고: 기존 yora 환경 (조사 결과)

새 작업은 별도 레포(`/Users/kys0213/workspace/areum-lab-tools`)에서 진행하되, 아래는 참고용.

- 기존 yora 레포(`/Users/kys0213/workspace/yora`)는 셸 스크립트 + Python MCP 서버 구성.
- Discord 봇 인프라가 이미 가동 중: Hermes 게이트웨이(launchd `ai.hermes.gateway`)가 yora#8040 봇 관리.
  - 관련 상태 파일: `~/.hermes/` 아래 `gateway.pid`, `discord_threads.json`, `channel_directory.json`, `gateway_state.json`.
  - 봇 토큰·허용 유저: `~/.hermes/.env`의 `DISCORD_BOT_TOKEN`, `DISCORD_ALLOWED_USERS`, `DISCORD_HOME_CHANNEL`.
- MCP 서버 패턴 예시: `mcp/cloud-agents/server.py` — stdlib-only stdio MCP 서버로 `delegate_to_claude`/`delegate_to_codex` 노출. 신규 CLI를 MCP로 갈 경우 참고 템플릿.

---

## 7. 다음 단계 제안

1. 위 §5 열린 결정 확정 (봇 재사용 여부 / 스택 / 토큰 저장)
2. 봇 포털 세팅(§3) — 수동 1회
3. `discord send` 최소 버전부터 구현 → 실제 채널 전송 검증
4. `read` → `wait` 순으로 확장
