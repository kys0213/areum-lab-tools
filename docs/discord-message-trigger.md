# Discord 메시지 트리거 (`on_message`)

상주 데몬(`discord daemon`)이 봇 멘션이나 "이슈 채널"의 새 글을 감지하면, 설정한 외부 명령을 실행하고 메시지 정보를 JSON 한 줄로 stdin에 넘겨요.

- **왜**: Discord에서 봇을 부르면 다른 프로그램(런처 등)이 작업을 시작할 수 있게 하려고요. 게이트웨이 연결은 데몬 하나만 가지므로, 감지도 데몬이 맡아요.
- **CLI가 하는 일**: 메시지를 거르고, 명령을 실행하고, JSON을 넘기는 것까지예요. 명령이 무엇을 하는지는 몰라요.
- 데몬 구조와 `ask` 흐름은 [discord-daemon-hitl.md](discord-daemon-hitl.md)를 보세요.

## 설정

`~/.areum/discord/config.json`에 다섯 필드를 더해요.

| 필드 | 형식 | 설명 |
|---|---|---|
| `on_message` | 문자열 배열 (argv) | 메시지마다 실행할 명령. 첫 요소가 실행 파일, 나머지가 인자예요. 셸을 거치지 않아요. **없으면 감지 기능이 꺼져요.** 빈 배열은 데몬 시작 시 설정 오류로 끝나요. |
| `issue_channels` | 문자열 배열 | 멘션 없이도 새 글을 발행할 채널. 채널 ID나 `channels`의 별칭을 써요. 생략하면 멘션만 감지해요. |
| `workdirs` | `{ "<채널 ID 또는 별칭>": "<디렉토리>" }` | 채널별로 `on_message`를 실행할 디렉토리. 키의 별칭은 `channels`로 풀어요. |
| `default_workdir` | 문자열 | `workdirs`에 없는 채널에서 쓸 디렉토리. |
| `trigger_bots` | 문자열 배열 | 메시지가 트리거 대상이 되는 봇의 user ID(이 봇 자신 포함). 숫자 ID만 받아요. 생략하거나 비우면 모든 봇 메시지를 무시해요. |

```json
{
  "token": "...",
  "channels": { "issues": "123456789012345678", "docs": "223456789012345678" },
  "issue_channels": ["issues"],
  "on_message": ["/usr/local/bin/discord-run", "--verbose"],
  "workdirs": { "issues": "/Users/me/work/app", "docs": "~/work/docs" },
  "default_workdir": "~/work/scratch",
  "trigger_bots": ["999888777666555444"]
}
```

- `on_message`가 없는 기존 config도 그대로 읽혀요. 이때 데몬은 예전과 똑같이 동작해요(메시지 인텐트를 요청하지 않음).
- `on_message`를 설정했다면 `workdirs`와 `default_workdir` 중 하나는 있어야 해요. 둘 다 없으면 어떤 메시지도 실행할 수 없어서 데몬 시작 시 설정 오류로 끝나요.
- `on_message`를 설정했을 때만 경로·디렉토리 설정을 검증해요. 디렉토리는 절대경로이거나 `~/`로 시작해야 해요(`~/`는 홈 디렉토리로 펼쳐요). 상대경로와 `~user/...` 형식은 데몬 시작 시 설정 오류로 끝나요.
- `workdirs`의 서로 다른 키(채널 ID나 별칭)가 같은 채널로 풀리면 디렉토리 값이 같아도 데몬 시작 시 설정 오류로 끝나요. 어느 디렉토리를 쓸지 모호해지기 때문이에요.
- 설정은 데몬 시작 시 한 번 읽어요. 바꾼 뒤에는 `discord daemon stop` 후 `discord daemon start`로 다시 띄우세요.
- `trigger_bots`에 숫자가 아닌 값(별칭 포함)이 있으면 `on_message`를 설정했을 때 데몬 시작 시 설정 오류로 끝나요. `channels` 별칭은 채널용이라 봇에는 쓰지 않아요.
- `discord init --force`로 토큰을 바꿔도 토큰 외 필드는 보존돼요.

### 실행 디렉토리 결정

메시지를 발행하기로 판정한 뒤에 정해요.

1. 스레드 안의 글은 상위 채널 ID(`parent_channel_id`), 최상위 글은 채널 ID로 `workdirs`를 찾아요.
2. 없으면 `default_workdir`를 써요.
3. 정해진 디렉토리가 없거나 디렉토리가 아니면 명령을 실행하지 않고 데몬 stderr에 로그만 남겨요. Discord에는 아무것도 쓰지 않아요.

- `workdirs`에서 찾은 디렉토리가 실제로 없을 때 `default_workdir`로 대신하지 않아요. 엉뚱한 저장소에서 작업이 실행되는 것을 막으려는 거예요.
- 존재 여부는 발행할 때마다 확인해요. 데몬이 떠 있는 동안 디렉토리가 지워져도 놓치지 않아요.
- 명령은 정해진 디렉토리를 현재 디렉토리로 해서 실행되고, 실제로 쓴 디렉토리(정규화된 절대경로)는 stdin JSON의 `cwd`로 넘어가요.

## Developer Portal 설정 (필수)

감지 기능은 메시지 본문을 읽어야 해서 특권 인텐트(Message Content)가 필요해요.

1. https://discord.com/developers/applications 에서 봇 애플리케이션을 열어요.
2. **Bot** → **Privileged Gateway Intents** → **Message Content Intent**를 켜고 저장해요.

- 데몬은 `on_message`가 있을 때만 `GUILD_MESSAGES | MESSAGE_CONTENT` 인텐트를 요청해요. `on_message`를 쓰지 않으면 인텐트 요청이 없어서 Portal 설정과 무관하게 동작해요.
- 인텐트를 켜지 않은 채 `on_message`를 설정하면 게이트웨이가 4014(Disallowed Intents)로 연결을 끊어요. 데몬은 재시도하지 않고 아래처럼 원인과 해결 방법을 담은 오류로 끝나요.

```
error [config]: gateway closed with 4014 (Disallowed Intents): message detection (`on_message` in config) needs the privileged Message Content intent. Enable "Message Content Intent" under Bot > Privileged Gateway Intents in the Discord Developer Portal, or remove `on_message` from config; daemon will not retry
```

## 필터 규칙

위에서부터 처음 맞는 규칙을 적용해요. 한 메시지는 최대 한 번만 발행해요.

| # | 조건 | 결과 |
|---|---|---|
| 1 | 작성자가 봇(`author.bot`)이거나 이 봇 자신이고, `trigger_bots`에 없음 | 발행 안 함 |
| 2 | 사람이 쓴 일반 글·답장(메시지 type 0, 19)이 아님 — 스레드 생성 알림, 고정 알림, 입장 알림 같은 시스템 메시지 | 발행 안 함 |
| 3 | 스레드 안의 글 | 봇 멘션이 있을 때만 `trigger: "mention"` |
| 4 | 이슈 채널의 최상위 글 | 멘션이 없어도 `trigger: "issue_channel"` (멘션이 있어도 이 한 번만) |
| 5 | 그 밖의 채널 | 봇 멘션이 있을 때만 `trigger: "mention"` |

- `trigger_bots`에 있는 봇(자기 자신 포함)의 메시지는 1번을 통과해 2~5번 규칙을 사람 메시지와 똑같이 거쳐요. stdin JSON에서는 `author.bot`이 `true`로 올 수 있어요.
- **봇 멘션**은 메시지의 `mentions` 목록에 봇 user ID가 있는지로 판정해요. 역할 멘션과 `@everyone`/`@here`는 멘션으로 치지 않아요.
- 이슈 채널 글에서 파생된 스레드 안의 글은 3번(스레드) 규칙을 따라요. 즉 스레드 답글은 멘션해야 발행돼요.
- 포럼 채널은 이슈 채널로 쓸 수 없어요. 포럼 글은 스레드라서 봇을 멘션해야 발행돼요.
- 봇 user ID는 게이트웨이 READY 이벤트에서 얻어요.

### 스레드 판정

게이트웨이 `MESSAGE_CREATE`에는 채널이 스레드인지 들어 있지 않아서, REST `GET /channels/{channel_id}`로 채널 type과 `parent_id`를 조회해요.

- 채널 type 10·11·12(공지·공개·비공개 스레드)면 스레드로 보고 `parent_id`를 상위 채널로 써요.
- 조회 결과는 데몬이 떠 있는 동안 캐시해요. 채널이 스레드인지와 상위 채널은 바뀌지 않기 때문이에요. 실패한 조회는 캐시하지 않고 다음 메시지에서 다시 조회해요.
- 멘션도 없고 이슈 채널도 아닌 글은 조회 없이 바로 버려요.

### 루프 위험과 안전 조건

`trigger_bots`에 이 봇 자신을 넣으면, 훅이 `discord send`로 올린 글이 다시 훅을 실행하는 순환이 생길 수 있어요.

- 훅이 결과를 **스레드 안에 봇 멘션 없이** 올리면 스레드 규칙(3번) 때문에 다시 발행되지 않아요. 안전해요.
- 훅이 **이슈 채널 최상위**에 글을 올리면 4번 규칙으로 다시 발행돼요. 무한 반복되므로 피하세요.
- 목록의 봇이 이슈 채널이 아닌 일반 채널 최상위에 봇을 멘션해 글을 올려도 다시 트리거돼요.
- `discord send --reply-to`로 봇 자신의 메시지에 답장하면, Discord가 답장 대상 작성자를 멘션 목록에 넣어(답장 알림이 켜진 기본 동작) 멘션으로 판정될 수 있어요. 이 CLI의 `send`는 답장 알림을 끄지 않으므로, 훅이 결과를 올릴 때는 `--reply-to`를 쓰지 마세요.
- 목록에 넣은 봇이 올리는 글은 위 규칙으로 판정되니, 넣기 전에 그 봇이 어디에 어떤 글을 올리는지 확인하세요.
- 목록에 없는 봇·웹훅은 그대로 무시해서, 다른 봇이 Claude 실행을 일으키지 못해요.

## stdin JSON 계약

명령의 stdin에 JSON 객체 하나를 한 줄로 쓰고(끝에 개행), stdin을 닫아요. ID는 모두 문자열이에요.

| 필드 | 형식 | 설명 |
|---|---|---|
| `trigger` | `"mention"` \| `"issue_channel"` | 발행 이유 |
| `guild_id` | 문자열 | 서버 ID |
| `channel_id` | 문자열 | 메시지가 올라온 채널. 스레드면 스레드 ID |
| `parent_channel_id` | 문자열 \| `null` | 스레드일 때 상위 채널 ID, 아니면 `null` |
| `is_thread` | bool | 스레드 안의 글인지 |
| `message_id` | 문자열 | 메시지 ID |
| `content` | 문자열 | 원문 그대로(`<@봇ID>` 같은 멘션 토큰 포함) |
| `author` | `{id, username, bot}` | 작성자 |
| `timestamp` | 문자열 | 메시지 작성 시각(ISO 8601) |
| `bot_user_id` | 문자열 | 이 봇의 user ID. `content`에서 멘션 토큰을 걷어낼 때 써요 |
| `message_reference` | 문자열 \| `null` | 답장이면 답장 대상 메시지 ID, 아니면 `null` |
| `cwd` | 문자열 | 명령을 실행한 디렉토리. 심볼릭 링크를 풀어 정규화한 절대경로예요 |

스레드 안에서 봇을 멘션한 예시(실제로는 한 줄):

```json
{"trigger":"mention","guild_id":"111","channel_id":"333","parent_channel_id":"222","is_thread":true,"message_id":"444","content":"<@999> 이 에러 봐줘","author":{"id":"555","username":"alice","bot":false},"timestamp":"2026-10-04T01:02:03.000000+00:00","bot_user_id":"999","message_reference":null,"cwd":"/Users/me/work/app"}
```

이슈 채널 최상위 글 예시:

```json
{"trigger":"issue_channel","guild_id":"111","channel_id":"222","parent_channel_id":null,"is_thread":false,"message_id":"666","content":"로그인 버튼이 안 눌려요","author":{"id":"555","username":"alice","bot":false},"timestamp":"2026-10-04T01:05:00.000000+00:00","bot_user_id":"999","message_reference":null,"cwd":"/Users/me/work/app"}
```

## 실행 방식과 실패 시 동작

- 명령은 셸 없이 바로 실행해요. 파이프나 리다이렉션이 필요하면 `["sh", "-c", "..."]`처럼 직접 셸을 넣으세요.
- 데몬은 명령의 종료를 기다리지 않아요. 별도 스레드가 stdin을 쓰고 닫은 뒤 종료를 회수해서 좀비 프로세스가 남지 않아요.
- 명령의 stdout은 버리고, stderr는 데몬의 stderr로 이어져요. 오래 걸리는 작업이면 명령 쪽에서 스스로 로그를 남기세요.
- 명령에는 `DISCORD_BOT_TOKEN` 환경변수가 넘어가지 않아요. 봇 토큰이 외부 명령으로 새지 않게 하려고 데몬이 지우고 실행해요. 명령 안에서 `discord send`·`discord ask`를 부르려면 config 파일에 토큰이 있어야 해요(`discord init`으로 저장).
- 채널 조회(REST)는 3초 제한이 있어요. 넘기면 그 메시지는 발행하지 않고 로그를 남겨요. 느린 Discord 응답 하나가 다른 이벤트 처리까지 막지 않게 하려는 제한이에요.
- 아래 실패는 모두 데몬 stderr에 한 줄로 남기고, 데몬은 계속 돌아요.

| 상황 | 결과 |
|---|---|
| 명령 실행 실패(파일 없음, 권한 없음) | `daemon: message handling failed: ...` 로그, 해당 메시지는 발행 안 됨 |
| 명령이 0이 아닌 코드로 종료 | `daemon: on_message "..." exited with ...` 로그 |
| 명령이 stdin을 읽지 않고 종료 | `failed to write stdin` 로그 |
| 채널에 맞는 디렉토리가 없음(`workdirs`에도 `default_workdir`에도 없음) | 실행하지 않고 `no workdir for channel <id> ...` 로그 |
| 정해진 디렉토리가 없거나 디렉토리가 아님 | 실행하지 않고 `workdir <path> for channel <id> is unusable ...` 또는 `... is not a directory` 로그. `default_workdir`로 대신하지 않음 |
| 채널 조회(REST) 실패 | 발행하지 않고 로그. 추측으로 발행하지 않아요 |
| 채널 조회가 3초를 넘김 | 발행하지 않고 `channel lookup for <channel> timed out after 3s, message <id> not published` 로그 |
| 인텐트 거부 close(4013/4014) | 데몬이 위 오류로 종료(재시도 안 함) |

- 발행에 성공하면 `daemon: message <id> in <channel> -> on_message (Mention)` 로그가 남아요.
- 백그라운드 데몬(`discord daemon start`)은 stdio를 닫고 떠서 이 로그가 보이지 않아요. 확인하려면 `discord daemon start --foreground`로 띄우세요.

## 확인 방법

stdin을 파일에 쌓는 명령으로 설정하고 포그라운드로 띄워요.

```json
{ "token": "...", "issue_channels": ["123456789012345678"], "on_message": ["sh", "-c", "cat >> /tmp/discord-trigger.log"], "default_workdir": "/tmp" }
```

```sh
discord daemon start --foreground
```

다른 터미널에서 지켜봐요.

```sh
tail -f /tmp/discord-trigger.log
```

- 일반 채널에서 봇을 멘션하면 `"trigger":"mention"` 한 줄이 쌓여요.
- 이슈 채널에 새 글을 올리면 `"trigger":"issue_channel"` 한 줄이 쌓여요.
- 멘션 없는 일반 채널 글, 봇이 보낸 글은 쌓이지 않아요.

## E2E로 확인할 항목

필터 규칙·JSON 변환·명령 실행은 단위 테스트로 덮지만, 아래 연결부는 실제 게이트웨이가 있어야 확인돼요. 릴리스 전에 위 "확인 방법"으로 직접 봐요.

- **READY에서 봇 ID 기록**: 데몬을 띄운 직후 봇을 멘션했을 때 `"trigger":"mention"`이 쌓이고 `bot_user_id`가 봇 ID와 같은지 봐요. READY 처리가 빠지면 멘션 판정이 안 돼요.
- **MESSAGE_CREATE 구독**: `on_message`를 설정했을 때 새 글이 데몬에 도착하는지 봐요. 이벤트 구독이 빠지면 아무 글도 쌓이지 않고 로그도 남지 않아요.
- **채널별 실행 디렉토리**: 채널 2개를 서로 다른 디렉토리에 매핑하고 각 채널에서 멘션했을 때, 기록된 JSON의 `cwd`와 훅의 실제 현재 디렉토리(`pwd`)가 일치하는지 봐요. 매핑한 디렉토리를 지운 뒤 멘션하면 명령이 실행되지 않고 데몬 로그만 남는지도 봐요.
- **4013/4014 close 처리**: Developer Portal에서 Message Content Intent를 끈 채 띄우면 데몬이 위 `4014 (Disallowed Intents)` 오류로 끝나고 재시도하지 않는지 봐요. 4013(Invalid Intents)은 데몬이 잘못된 인텐트 값을 보낼 때만 나서 수동 재현이 어렵고, close 코드 분류는 단위 테스트로 덮어요.
