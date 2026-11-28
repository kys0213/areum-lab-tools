# Discord MCP 서버 (`discord mcp serve`)

`discord` CLI 하나로 여러 Discord 봇을 여러 Orca 봇에 붙이는 로컬 MCP(Model Context Protocol, 에이전트가 외부 도구를 부르는 표준 규약) 서버예요.

- **왜**: 봇마다 CLI 설정 파일이나 프로세스를 따로 두지 않고, 서버 하나에 봇마다 다른 헤더로 등록하려고요.
- **어떻게**: 서버는 토큰을 갖지 않아요. 요청마다 `Authorization` 헤더로 받은 봇 토큰으로 Discord를 불러요. 세션도 두지 않아서(stateless) 서버를 다시 켜도 등록을 다시 할 필요가 없어요.
- **전송**: MCP Streamable HTTP, 엔드포인트 `/mcp`. 응답은 일반 JSON이에요.

## 실행

```sh
discord mcp serve
```

- 기본 주소는 `127.0.0.1:8765`예요. 바꾸려면 `--listen 127.0.0.1:9000`처럼 줘요.
- **loopback 주소(`127.0.0.1`, `[::1]`)만 받아요.** 토큰이 평문 HTTP 헤더로 오기 때문에, 다른 컴퓨터에서 이 포트에 닿으면 토큰이 노출돼요. `0.0.0.0` 같은 주소는 usage 오류(exit 2)로 끝나요. 이를 풀어 주는 플래그는 일부러 두지 않았어요. 원격으로 쓰려면 TLS를 앞에 둬야 하는데, 이 서버는 TLS를 하지 않아요.
- `Host` 헤더도 `localhost`·`127.0.0.1`·`::1`만 받아요. 브라우저를 통한 DNS 리바인딩 공격을 막아요.
- SIGINT/SIGTERM을 받으면 끝나요. 도구 호출마다 stderr에 `mcp: <도구> ok` 또는 `mcp: <도구> failed: <오류>` 한 줄을 남겨요. 토큰은 로그에 남지 않아요.
- 설정 파일(`~/.areum/discord/config.json`)의 토큰과 `channels`는 쓰지 않아요. 모든 값은 요청 헤더에서 와요.

## Orca에 등록하기 ("Add an app by URL")

봇마다 앱을 하나씩 등록해요. URL은 같고 헤더만 달라요.

| 항목 | 값 |
|---|---|
| Server URL | `http://localhost:8765/mcp` |
| Header `Authorization` | `Bot <discord-bot-token>` |
| Header `X-Discord-Channels` (선택) | `product-dev=1530477900095164517,ops=223456789012345678` |
| Header `X-Discord-Allowed-Channels` (선택) | `1530477900095164517` |

## 요청 헤더

| 헤더 | 필수 | 형식 | 설명 |
|---|---|---|---|
| `Authorization` | 예 | `Bot <token>` | 이 요청이 쓸 봇 토큰. 없거나 형식이 틀리면 MCP 프로토콜 오류(`initialize`·`tools/list`·`tools/call` 모두)로 끝나고 도구가 실행되지 않아요. 오류 메시지에는 토큰 값이 들어가지 않아요. |
| `X-Discord-Channels` | 아니요 | `alias=id,alias=id` | 도구의 `channel` 인자에 별칭을 쓸 수 있게 해요. 목록에 없는 값은 채널 id로 그대로 써요. |
| `X-Discord-Allowed-Channels` | 아니요 | `id,id` | 있으면 이 채널들만 허용해요. 별칭을 푼 뒤 id로 비교해요. 비어 있는 값은 오류예요(모든 호출이 막히는 실수를 막으려고요). |

### 스레드 허용 규칙

`X-Discord-Allowed-Channels`가 있고 대상 id가 목록에 없으면, 서버가 `GET /channels/{id}`로 채널을 한 번 조회해요. **스레드이고 부모 채널이 목록에 있으면 허용해요.**

- **왜**: 스레드는 `thread_create`나 사람이 그때그때 만들어서, 미리 적어 둔 목록에 id가 있을 수 없어요. 스레드는 부모 채널의 구성원만 볼 수 있으니, 부모를 허용했다면 그 안의 스레드도 같은 범위예요.
- 목록에 있는 id는 조회 없이 바로 허용해요. 조회가 실패하면(없는 채널 등) 그 오류를 그대로 돌려줘요.
- 거부되면 도구 오류 `kind: "auth"`, 메시지 `channel <id> is not in X-Discord-Allowed-Channels`로 끝나요.

## 도구

결과는 CLI `--json` 봉투의 `data`와 같은 JSON이에요(`structuredContent`와 `content[0].text`에 같이 들어가요). 실패하면 `isError: true`이고, JSON은 봉투의 `error`(`kind`, `message`, 선택 `http_status`·`retry_after_ms`)와 같아요.

| 도구 | 인자 | 대응 CLI |
|---|---|---|
| `send` | `channel`, `text`, `reply_to?` | `discord send <CHANNEL> --text <TEXT> [--reply-to]` |
| `read` | `channel`, `limit?`(기본 50, 1..=100), `after?` | `discord read` |
| `react` | `channel`, `message_id`, `emoji`, `remove?` | `discord react` |
| `thread_create` | `channel`, `name`, `from_message?` | `discord thread create` |

- 정의되지 않은 인자는 usage 오류예요.
- **`send`에는 파일 첨부(`files`)가 없어요.** CLI의 `--file`은 로컬 경로를 읽어요. MCP 서버에서 같은 일을 하면, 헤더만 가진 호출자가 서버 프로세스 권한으로 아무 파일이나 읽어 Discord에 올릴 수 있어요. 첨부가 필요하면 파일 내용을 인자로 받는 방식을 따로 설계해야 해요.
- 2000자를 넘는 본문은 거부돼요(`--split`은 없어요).

## 상주 실행 (launchd 예시)

설치는 하지 않아요. 필요하면 아래 파일을 `~/Library/LaunchAgents/dev.areum.discord-mcp.plist`로 저장하고 `discord` 경로를 맞춰요.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.areum.discord-mcp</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Users/me/.local/bin/discord</string>
    <string>mcp</string>
    <string>serve</string>
    <string>--listen</string>
    <string>127.0.0.1:8765</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardErrorPath</key>
  <string>/tmp/discord-mcp.log</string>
</dict>
</plist>
```

등록과 해제:

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/dev.areum.discord-mcp.plist
```

```sh
launchctl bootout gui/$(id -u)/dev.areum.discord-mcp
```

plist에는 토큰이 없어요. 토큰은 Orca에 등록한 헤더에만 있어요.

## 확인 방법

서버를 띄운 뒤, 토큰을 화면에 찍지 않고 셸 변수로 읽어 `tools/list`를 불러요.

```sh
TOKEN="$(python3 -c "import json,os;print(json.load(open(os.path.expanduser('~/.areum/discord/config.json')))['token'])")"
curl -sS http://localhost:8765/mcp -H "Authorization: Bot $TOKEN" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' --data '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```

## 다음 단계

- `ask`(버튼 질문)와 멘션으로 봇 깨우기는 이 서버에 없어요. 멘션 감지는 지금처럼 데몬(`discord daemon`, [discord-message-trigger.md](discord-message-trigger.md))이 맡아요.
