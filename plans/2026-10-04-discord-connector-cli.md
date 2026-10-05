# discord CLI — Discord connector 지원 확장 계획

> **Plan — 이 시점의 결정 기록.** 현재 정책의 신뢰 소스가 아니다.
> 날짜: 2026-10-04 · 브랜치: epic/discord-connector · 이슈: 없음 · 승인 출처: 사용자 승인

- 상태: 사용자 승인 후 구현 착수 (작성 시점 기준)

## 왜

Discord에서 봇을 멘션하거나 이슈 채널에 글을 올리면 Claude가 headless로 작업하고, 실행 중 질문은 Discord 버튼·모달로 받는 "Discord connector"를 만든다.
책임을 둘로 나눈다.

- **discord CLI (이 저장소)**: Discord 연결 방법만 담당한다. Claude를 모른다.
- **Claude 플러그인 (`kys-claude-plugin/plugins/discord-connector`, 별도 저장소·별도 세션 담당)**: CLI를 도구로 써서 Claude 실행을 감싼다. 런처 스크립트(채널→디렉토리 매핑, 스레드→`session_id` 매핑, `claude -p` 분리 실행, 결과 `send`)와 function hook(`AskUserQuestion` 가로채기 → `discord ask`)을 둔다.
- 결합점은 CLI 설정의 `on_message` 한 줄뿐이다. 게이트웨이 연결은 CLI 데몬 하나만 갖는다 — 같은 토큰으로 연결이 둘이면 interaction 응답이 경쟁하기 때문이다.

## 결정 사항

- 권한 승인 버튼은 만들지 않는다. 플러그인 세션 실험에서 auto 모드의 권한 hook이 "승인이 필요할 때만" 발화하지 않음을 확인했고, 권한은 auto 모드에 맡기기로 했다. `ask`는 `AskUserQuestion` 답변용이다.
- 플러그인의 function hook은 `$.process.run`으로 `ask wait`를 부르며 한 번에 최대 10분까지 기다릴 수 있다. ask 마감은 플러그인이 10분 이하로 잡는다. CLI의 `ask wait --timeout`은 그대로 둔다.
- 노트북마다 Discord 앱(봇 토큰)을 따로 만든다. 다중 인스턴스 대응 코드는 넣지 않는다.

## 작업 단위

### A. 메시지 감지 + `on_message` exec 훅

- 데몬 인텐트에 `GUILD_MESSAGES | MESSAGE_CONTENT`를 더하고 `MESSAGE_CREATE`를 받는다. 사용자가 Developer Portal에서 Message Content 특권 인텐트를 켜야 한다.
- 설정(`~/.areum/discord/config.json`)에 `issue_channels`(채널 ID 또는 별칭 목록)와 `on_message`(argv 배열)를 더한다. `on_message`가 없으면 감지 기능은 꺼진다.
- 필터(CLI가 결정적으로 수행): 봇 자신·다른 봇의 메시지 제외 / 스레드 안에서는 봇 멘션이 있을 때만 / 이슈 채널은 최상위 새 글만(스레드 안 글 제외) / 그 밖의 채널은 봇 멘션이 있을 때만.
- 발행: argv 명령을 비동기로 실행하고 stdin에 JSON 한 개를 쓴다. 종료를 기다리지 않는다. 실행 실패(spawn 실패, exit≠0)는 데몬 stderr 로그에만 남긴다.
- stdin JSON 필드: `trigger`(`"mention"` | `"issue_channel"`), `guild_id`, `channel_id`(스레드면 스레드 ID), `parent_channel_id`(스레드일 때 상위 채널, 아니면 null), `is_thread`, `message_id`, `content`(멘션 토큰 포함 원문), `author{id, username, bot}`, `timestamp`, `bot_user_id`, `message_reference`(답장 대상 메시지 ID, 없으면 null).
  - 이슈 채널에서 봇을 멘션한 글은 `trigger: "issue_channel"`로 한 번만 발행한다.
- 스레드 여부와 상위 채널은 게이트웨이 이벤트만으로 알 수 없으면 채널 정보를 조회해 판정한다. 조회 방식은 구현이 정하되 조회 실패는 발행하지 않고 로그로 드러낸다.

### B. `send --split`

- 2000자를 넘는 본문을 여러 메시지로 나눠 보낸다. `--split`이 없으면 지금처럼 2000자 초과를 거부한다.
- 줄 경계 우선으로 나누고, 코드 블록(```) 안에서 잘리면 앞 조각을 닫고 다음 조각을 같은 언어로 다시 연다.
- 첨부 파일은 첫 조각에만 붙인다. `--json` 출력은 보낸 메시지 목록을 담는다(형식은 구현이 정하고 docs에 반영).

### C. ask 확장

- `ask create --allowed-user <id>`(반복 가능): 지정되면 그 사용자만 답으로 채택한다. 그 밖의 사람이 누르면 채택하지 않고 ephemeral로 안내한다. 지정이 없으면 지금처럼 누구나 답할 수 있다.
- multiSelect: select menu 컴포넌트로 복수 선택을 지원한다. 결과 `value`의 형식(배열 등)은 구현이 정하고 docs에 반영한다. 플러그인 세션에 확정 형식을 알린다.
- 스키마 변경은 기존 `schema_version` 방식으로 하위 호환을 지킨다.

## 버린 선택지

- 플러그인이 `discord events --follow` 스트림을 상주하며 구독: 상주 프로세스가 하나 더 생긴다.
- CLI 데몬이 `claude -p`를 직접 실행: CLI가 Claude에 묶인다.
- 상주 Claude 세션 안의 mod가 트리거까지 처리: 세션에 프롬프트를 넣는 기능이 미확인이고, 모든 스레드가 한 세션에 섞인다.
- `Stop` hook으로 대화 턴을 붙잡기: 다음 멘션까지 몇 시간이 걸릴 수 있고, 연속 block 한도가 있다.
- 권한 승인 버튼: auto 모드에서 권한 hook이 원하는 시점에 발화하지 않는다.

## 작업 순서

- A·B·C를 각각 격리된 worktree에서 병렬로 구현한다. 변경 파일은 대부분 겹치지 않는다. `cli.rs`·`main.rs`는 플래그·서브커맨드를 추가하는 hot-spot이라 머지할 때 정리한다.
- 머지 순서는 B → C → A다. 범위가 작은 것부터 넣어 A의 rebase 부담을 한 번으로 줄인다.
- 각 단위는 관련 docs를 함께 갱신한다. B는 `docs/discord-agent-cli.md`, C는 `docs/discord-daemon-hitl.md`, A는 새 문서 `docs/discord-message-trigger.md`를 쓴다. 문서 겹침을 피하려고 A는 HITL 문서를 고치지 않는다.

## 테스트 계획

### 1. 단위 테스트 (각 단위, 구현 전 블랙박스 테스트 먼저)

- 위치는 `.claude/rules/module-structure.md`를 따른다(인라인 `#[cfg(test)]` 또는 `<이름>/tests.rs`, 공유 목은 `commands/testutil.rs`).
- A: 필터 판정 표(봇 메시지 / 스레드+멘션 / 스레드+무멘션 / 이슈 채널 최상위 / 이슈 채널 스레드 안 / 일반 채널+멘션 / 일반 채널+무멘션 / 이슈 채널+멘션 중복 발행 없음), JSON 필드 직렬화, `on_message` 미설정 시 비활성, argv 실행 실패가 데몬을 멈추지 않음, 설정 파싱(하위 호환: 새 필드 없는 기존 config).
- B: 2000자 이하 무분할, 경계 정확히 2000자, 줄 경계 우선, 한 줄이 2000자 초과일 때 강제 분할, 코드 블록 닫고 다시 열기, 첨부는 첫 조각에만, `--split` 없이 초과 시 기존 오류 유지.
- C: 허용 사용자 클릭 채택 / 비허용 사용자 클릭 미채택+ephemeral / 미지정 시 누구나 / 모달 제출에도 같은 규칙 / multiSelect 선택 결과 저장·`ask wait` 출력 / 기존 DB(이전 schema_version)에서 마이그레이션.

### 2. 통합 게이트 (자동)

- 각 단위 머지 직후와 epic 최종 HEAD에서 `make check`(fmt-check + clippy -D warnings + test)가 통과해야 한다.

### 3. E2E (사람 필요, 실제 Discord)

전제: 테스트 서버·테스트 봇, Developer Portal에서 Message Content 인텐트 활성화, `on_message`에 stdin을 파일로 남기는 테스트 명령 설정(예: `["sh", "-c", "cat >> /tmp/discord-trigger.log"]`).

| # | 시나리오 | 기대 결과 |
|---|---|---|
| 1 | 일반 채널에서 봇 멘션 | `trigger: "mention"` JSON 1건 기록 |
| 2 | 일반 채널에서 멘션 없는 글 | 기록 없음 |
| 3 | 이슈 채널에 최상위 새 글 | `trigger: "issue_channel"` 1건 |
| 4 | 이슈 채널 글에서 만든 스레드 안 답글(무멘션 / 멘션) | 무멘션은 기록 없음, 멘션은 `mention` + `is_thread: true` + `parent_channel_id` |
| 5 | 봇 자신이 보낸 메시지 | 기록 없음 |
| 6 | `send --split`으로 5000자 + 코드 블록 본문 전송 | 3개 메시지, 코드 블록 렌더링 유지 |
| 7 | 스레드 ID로 `ask create` → 버튼 클릭 | 스레드 안에 질문 표시, `ask wait`가 답 수신 |
| 8 | `--allowed-user` 지정 후 다른 계정으로 클릭 | ephemeral 안내, 미채택. 지정 계정 클릭은 채택 |
| 9 | multiSelect 질문에서 2개 선택 | `ask wait`가 두 값 반환 |
| 10 | 데몬 재시작 후 1 반복 | 정상 발행 |

- E2E 결과는 PR 본문에 시나리오별 통과 여부로 남긴다.
- 플러그인 연동 E2E(런처·function hook 포함)는 플러그인 세션 몫이다. CLI 릴리스 버전을 그 세션에 알린다.

## 완료 조건

- A·B·C가 epic에 머지되고 epic 최종 HEAD에서 `make check` 통과.
- E2E 1~10 통과(사용자 수행).
- 이후 버전업(`tools/discord/Cargo.toml` 0.4.0) → PR → main 머지 → `make dist-tag TOOL=discord`는 사용자 승인 후 진행한다.
