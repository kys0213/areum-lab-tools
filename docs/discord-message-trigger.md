# Discord 메시지 트리거 (`on_message`)

상주 데몬(`discord daemon`)이 봇 멘션이나 "이슈 채널"의 새 글을 감지하면, 설정한 외부 명령을 실행하고 메시지 정보를 JSON 한 줄로 stdin에 넘겨요.

- **왜**: Discord에서 봇을 부르면 다른 프로그램(런처 등)이 작업을 시작할 수 있게 하려고요. 게이트웨이 연결은 데몬 하나만 가지므로, 감지도 데몬이 맡아요.
- **CLI가 하는 일**: 메시지를 거르고, 명령을 실행하고, JSON을 넘기는 것까지예요. 명령이 무엇을 하는지는 몰라요.
- 데몬 구조와 `ask` 흐름은 [discord-daemon-hitl.md](discord-daemon-hitl.md)를 보세요.

## 설정

`~/.areum/discord/config.json`에 두 필드를 더해요.

| 필드 | 형식 | 설명 |
|---|---|---|
| `on_message` | 문자열 배열 (argv) | 메시지마다 실행할 명령. 첫 요소가 실행 파일, 나머지가 인자예요. 셸을 거치지 않아요. **없으면 감지 기능이 꺼져요.** 빈 배열은 데몬 시작 시 설정 오류로 끝나요. |
| `issue_channels` | 문자열 배열 | 멘션 없이도 새 글을 발행할 채널. 채널 ID나 `channels`의 별칭을 써요. 생략하면 멘션만 감지해요. |

```json
{
  "token": "...",
  "channels": { "issues": "123456789012345678" },
  "issue_channels": ["issues"],
  "on_message": ["/usr/local/bin/discord-run", "--verbose"]
}
```

- 두 필드가 없는 기존 config도 그대로 읽혀요. 이때 데몬은 예전과 똑같이 동작해요(메시지 인텐트를 요청하지 않음).
- 설정은 데몬 시작 시 한 번 읽어요. 바꾼 뒤에는 `discord daemon stop` 후 `discord daemon start`로 다시 띄우세요.
- `discord init --force`로 토큰을 바꿔도 두 필드는 보존돼요.

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
| 1 | 작성자가 봇(`author.bot`)이거나 이 봇 자신 | 발행 안 함 |
| 2 | 사람이 쓴 일반 글·답장(메시지 type 0, 19)이 아님 — 스레드 생성 알림, 고정 알림, 입장 알림 같은 시스템 메시지 | 발행 안 함 |
| 3 | 스레드 안의 글 | 봇 멘션이 있을 때만 `trigger: "mention"` |
| 4 | 이슈 채널의 최상위 글 | 멘션이 없어도 `trigger: "issue_channel"` (멘션이 있어도 이 한 번만) |
| 5 | 그 밖의 채널 | 봇 멘션이 있을 때만 `trigger: "mention"` |

- **봇 멘션**은 메시지의 `mentions` 목록에 봇 user ID가 있는지로 판정해요. 역할 멘션과 `@everyone`/`@here`는 멘션으로 치지 않아요.
- 이슈 채널 글에서 파생된 스레드 안의 글은 3번(스레드) 규칙을 따라요. 즉 스레드 답글은 멘션해야 발행돼요.
- 포럼 채널은 이슈 채널로 쓸 수 없어요. 포럼 글은 스레드라서 봇을 멘션해야 발행돼요.
- 봇 user ID는 게이트웨이 READY 이벤트에서 얻어요.

### 스레드 판정

게이트웨이 `MESSAGE_CREATE`에는 채널이 스레드인지 들어 있지 않아서, REST `GET /channels/{channel_id}`로 채널 type과 `parent_id`를 조회해요.

- 채널 type 10·11·12(공지·공개·비공개 스레드)면 스레드로 보고 `parent_id`를 상위 채널로 써요.
- 조회 결과는 데몬이 떠 있는 동안 캐시해요. 채널이 스레드인지와 상위 채널은 바뀌지 않기 때문이에요. 실패한 조회는 캐시하지 않고 다음 메시지에서 다시 조회해요.
- 멘션도 없고 이슈 채널도 아닌 글은 조회 없이 바로 버려요.

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

스레드 안에서 봇을 멘션한 예시(실제로는 한 줄):

```json
{"trigger":"mention","guild_id":"111","channel_id":"333","parent_channel_id":"222","is_thread":true,"message_id":"444","content":"<@999> 이 에러 봐줘","author":{"id":"555","username":"alice","bot":false},"timestamp":"2026-10-04T01:02:03.000000+00:00","bot_user_id":"999","message_reference":null}
```

이슈 채널 최상위 글 예시:

```json
{"trigger":"issue_channel","guild_id":"111","channel_id":"222","parent_channel_id":null,"is_thread":false,"message_id":"666","content":"로그인 버튼이 안 눌려요","author":{"id":"555","username":"alice","bot":false},"timestamp":"2026-10-04T01:05:00.000000+00:00","bot_user_id":"999","message_reference":null}
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
| 채널 조회(REST) 실패 | 발행하지 않고 로그. 추측으로 발행하지 않아요 |
| 채널 조회가 3초를 넘김 | 발행하지 않고 `channel lookup for <channel> timed out after 3s, message <id> not published` 로그 |
| 인텐트 거부 close(4013/4014) | 데몬이 위 오류로 종료(재시도 안 함) |

- 발행에 성공하면 `daemon: message <id> in <channel> -> on_message (Mention)` 로그가 남아요.
- 백그라운드 데몬(`discord daemon start`)은 stdio를 닫고 떠서 이 로그가 보이지 않아요. 확인하려면 `discord daemon start --foreground`로 띄우세요.

## 확인 방법

stdin을 파일에 쌓는 명령으로 설정하고 포그라운드로 띄워요.

```json
{ "token": "...", "issue_channels": ["123456789012345678"], "on_message": ["sh", "-c", "cat >> /tmp/discord-trigger.log"] }
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
- **4013/4014 close 처리**: Developer Portal에서 Message Content Intent를 끈 채 띄우면 데몬이 위 `4014 (Disallowed Intents)` 오류로 끝나고 재시도하지 않는지 봐요. 4013(Invalid Intents)은 데몬이 잘못된 인텐트 값을 보낼 때만 나서 수동 재현이 어렵고, close 코드 분류는 단위 테스트로 덮어요.
