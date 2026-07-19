---
paths:
  - "tools/*/src/**/*.rs"
---

# Module Structure Convention

> 파일 이름만 보고 내용을 예측할 수 있어야 한다. `tools/discord/src/`가 여러 서브커맨드를 가진 도구의 규범 트리다.

## 원칙

1. **파일명 = 기능 (discoverability first)**: 파일 이름만 보고 내용을 예측할 수 있어야 한다. 한 파일에 서로 다른 기능 2개 이상을 담지 않는다.
2. **루트 3층 구조**: `main.rs`(배선)·`cli.rs`(명령 표면) + 기능 디렉토리(`commands/`, `output/`) + 공유 기반(`common/`)으로 구성한다.
3. **서브커맨드가 여럿이면 `commands/<이름>.rs` 파일당 하나**: 새 서브커맨드는 새 파일을 추가하는 것으로 끝난다 — 기존 파일은 건드리지 않는다 (OCP).
4. **`common/` grab-bag 가드**: `common/`에는 **둘 이상의 기능이 공유하는 기반**(config/error/api 계약/http 구현)만 둔다. 특정 기능 전용 코드가 들어가면 그 기능 파일로 되돌린다.
5. **크기는 하드 리밋이 아닌 신호**: 프로덕션 코드(테스트 제외) ~300줄이면 분리를 검토한다. 테스트 줄수로 분리를 촉발하지 않는다. 검토 후 유지로 결정하면 커밋/PR에 사유를 남긴다.
6. **테스트 배치**: private 함수를 블랙박스로 검증하므로 `#[cfg(test)]` 모듈이 필수다 (통합 `tests/` 디렉토리는 private 접근이 안 되므로 금지). 기본은 동일 파일 인라인, 프로덕션 탐색을 해칠 만큼 커지면 `<이름>/tests.rs` 자식 모듈로 분리한다. 공유 목·픽스처는 `testutil.rs`가 단일 소유한다.
7. **공유 로직은 개념 이름 파일로**: 여러 파일이 쓰는 헬퍼는 개념을 드러내는 이름으로 추출한다. `mod.rs`에 로직을 두지 않는다 — `mod.rs`는 선언·재노출만 담당한다.
8. **가시성 최소**: 파일 경계를 넘는 것만 `pub(crate)`(형제 트리까지) 또는 `pub(super)`(부모까지만) 로 연다. 그 외는 private.
9. **과잉 분리 방지 (졸업 기준)**: 서브커맨드 0~1개·프로덕션 ~150줄 이하 도구는 `main.rs` 단일 파일을 유지한다 (`tools/hello`가 기준 사례). 16줄짜리 trait 하나를 독립 파일로 빼지 않는다.

## DO

루트는 배선(`main.rs`)·명령 표면(`cli.rs`) + 기능 디렉토리(`commands/`, `output/`) + 공유 기반(`common/`)의 3층 구조를 따른다 (`tools/discord/src/` 실물).

```
tools/discord/src/
├── main.rs              # mod 선언 + config/token 배선 + 커맨드 디스패치
├── cli.rs                # clap Parser/Subcommand 정의 (명령 표면)
├── commands/             # 기능 디렉토리: 서브커맨드별 로직
│   ├── mod.rs             # 선언 + 재노출만
│   ├── send.rs
│   ├── read.rs
│   ├── wait.rs
│   ├── init.rs
│   ├── init/tests.rs      # init.rs 전용 테스트 자식 모듈
│   ├── read/tests.rs
│   ├── send/tests.rs
│   ├── wait/tests.rs
│   ├── cursor.rs          # send/read/wait가 공유하는 헬퍼 (개념 이름)
│   └── testutil.rs        # 공유 목/픽스처 단일 소유
├── output/                # 기능 디렉토리: 렌더링
│   ├── mod.rs
│   ├── payload.rs
│   └── envelope.rs
└── common/                # 공유 기반: 2개 이상 기능이 공유
    ├── mod.rs
    ├── config.rs
    ├── api.rs
    ├── error.rs
    └── http.rs
```

서브커맨드는 `commands/<이름>.rs` 파일당 하나, `commands/mod.rs`는 선언·재노출만 한다 (`tools/discord/src/commands/mod.rs` 전문).

```rust
mod cursor;
mod init;
mod read;
mod send;
mod wait;

#[cfg(test)]
pub(crate) mod testutil;

pub(crate) use init::run_init;
pub(crate) use read::run_read;
pub(crate) use send::run_send;
pub(crate) use wait::{TokioSleeper, run_wait};
```

새 서브커맨드는 새 파일을 추가하는 것으로 끝난다 — `commands/read.rs`는 `send.rs`/`wait.rs`를 참조하지 않는다.

```rust
// tools/discord/src/commands/read.rs
pub(crate) async fn run_read(
    api: &impl DiscordApi,
    channel_id: &str,
    after: Option<&str>,
    limit: u8,
) -> Result<Payload, AppError> {
    validate_limit(limit)?;
    let messages = api.get_messages(channel_id, after, limit).await?;
    let sorted = sort_ascending_by_id(messages)?;
    let cursor = newest_cursor(&sorted, after);
    Ok(Payload::Read(ReadData { channel_id: channel_id.to_owned(), count: sorted.len(), cursor, messages: sorted }))
}
```

`common/`은 두 개 이상의 기능이 공유하는 기반만 담는다 (`tools/discord/src/common/mod.rs` 전문) — `config`는 `main.rs`와 `commands/*`가, `error`는 모든 계층이, `api`(trait 계약)는 `http`(구현)와 `commands/testutil`(mock)이 함께 쓴다.

```rust
pub(crate) mod api;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod http;
```

`send`/`read`/`wait`가 공유하는 헬퍼는 `mod.rs`가 아니라 개념을 드러내는 이름의 파일로 뺀다 (`tools/discord/src/commands/cursor.rs` 발췌) — `read.rs`/`wait.rs`는 `use super::cursor::{newest_cursor, sort_ascending_by_id, validate_limit};`로 가져다 쓴다.

```rust
/// Discord snowflake ids sort correctly only by numeric value, not lexically
/// ("9" > "10" as strings). Parse failure means the API returned something
/// that isn't a snowflake, which is an API contract violation, not a usage
/// error.
pub(crate) fn parse_snowflake(id: &str) -> Result<u64, AppError> {
    id.parse::<u64>().map_err(|_| {
        AppError::new(
            ErrorKind::Api,
            format!("message id '{id}' is not a valid u64 snowflake"),
        )
    })
}
```

크기는 신호일 뿐 하드 리밋이 아니다. `common/http.rs`는 프로덕션 코드만 약 300줄이지만 HTTP 클라이언트라는 단일 책임이라 유지한다(예: `common/http.rs` — 단일 책임이라 유지, 커밋/PR에 사유 기록). 반대로 `output/envelope.rs`(284줄)와 `common/api.rs`(247줄)는 프로덕션이 각각 41줄·89줄뿐이고 나머지는 테스트라서 분리 신호가 아니다.

테스트는 기본적으로 동일 파일에 인라인으로 둔다 (`tools/discord/src/common/error.rs` 발췌).

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_maps_every_kind() {
        assert_eq!(exit_code(&AppError::new(ErrorKind::Usage, "")), 2);
        // ...
    }
}
```

프로덕션 탐색을 해칠 만큼 테스트가 커지면 `<이름>/tests.rs` 자식 모듈로 분리한다 (`tools/discord/src/commands/init.rs` 끝부분과 `commands/init/tests.rs`).

```rust
// tools/discord/src/commands/init.rs
#[cfg(test)]
mod tests;
```

공유 목/픽스처는 `testutil.rs`가 단일 소유한다 (`tools/discord/src/commands/testutil.rs` 발췌) — `send`/`read`/`wait` 테스트가 모두 여기의 `MockDiscordApi`/`FakeSleeper`를 가져다 쓴다.

```rust
pub(crate) struct MockDiscordApi {
    pub(crate) send_responses: RefCell<VecDeque<Result<SentMessage, AppError>>>,
    pub(crate) send_calls: RefCell<Vec<SendRequest>>,
    pub(crate) get_responses: RefCell<VecDeque<Result<Vec<Message>, AppError>>>,
    pub(crate) get_calls: RefCell<Vec<GetCall>>,
}
```

가시성은 필요한 만큼만 연다 — 부모 모듈에서만 쓰이면 `pub(super)`, 형제 트리를 넘나들면 `pub(crate)` (`tools/discord/src/output/envelope.rs`·`common/error.rs` 발췌).

```rust
// output/envelope.rs — output/mod.rs(부모)에서만 호출되므로 pub(super)
pub(super) fn success_json(command: &str, data: &Payload) -> String { /* ... */ }

// common/error.rs — crate::output::render(다른 트리)에서 호출되므로 pub(crate)
pub(crate) fn to_human(&self) -> String { /* ... */ }
```

서브커맨드가 없거나 하나뿐이고 프로덕션이 ~150줄 이하면 `main.rs` 단일 파일을 유지한다 (`tools/hello/src/main.rs` 전문, 29줄).

```rust
use clap::Parser;

/// Minimal placeholder CLI for the workspace scaffold.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Name to greet
    #[arg(long, default_value = "world")]
    name: String,
}

fn greeting(name: &str) -> String {
    format!("Hello, {name}!")
}

fn main() {
    let cli = Cli::parse();
    println!("{}", greeting(&cli.name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_includes_name() {
        assert_eq!(greeting("test"), "Hello, test!");
    }
}
```

## DON'T

한 파일에 서로 다른 기능을 섞지 않는다 — 파일명으로 내용을 예측할 수 없게 된다.

```rust
// commands/send.rs에 read/wait 로직까지 얹으면 파일명(send)과 내용이 어긋나고,
// 세 커맨드 중 하나만 고쳐도 diff에 나머지 두 커맨드가 섞여 리뷰가 어려워진다.
pub(crate) async fn run_send(...) -> Result<Payload, AppError> { /* ... */ }
pub(crate) async fn run_read(...) -> Result<Payload, AppError> { /* send.rs에 있을 이유가 없다 */ }
```

`common/`을 grab-bag으로 쓰지 않는다 — 특정 기능 전용 코드는 그 기능 파일로 되돌린다.

```rust
// common/api.rs는 DiscordApi 계약(http.rs 구현 + testutil.rs mock이 공유)만 담아야 한다.
// send 전용 파일 첨부 검증(MAX_FILES, MAX_FILE_BYTES)을 여기 두면
// send 하나만 쓰는 로직이 "공유 기반"인 것처럼 보인다.
pub(crate) const MAX_FILES: usize = 10; // commands/send.rs로 되돌려야 한다
```

새 서브커맨드를 추가하며 기존 커맨드 파일을 함께 고치지 않는다 — 확장은 새 파일 추가로 끝나야 한다.

```rust
// commands/mod.rs에 새 서브커맨드 로직을 직접 분기로 얹지 않는다.
pub(crate) async fn dispatch(cmd: &str, ...) -> Result<Payload, AppError> {
    match cmd {
        "send" => { /* ... */ }
        "edit" => { /* 새 서브커맨드를 기존 dispatch 안에 끼워 넣으면
                        커맨드가 늘 때마다 이 함수를 계속 고쳐야 한다 (OCP 위반) */ }
        _ => unreachable!(),
    }
}
```

`mod.rs`에 로직을 두지 않는다 — 선언·재노출만 담당해야 한다.

```rust
// commands/mod.rs에 커서 계산 로직을 직접 넣지 않는다.
mod send;
mod read;

pub(crate) fn newest_cursor(messages: &[Message]) -> Option<String> {
    // 이 로직은 commands/cursor.rs에 있어야 한다 — mod.rs는 통과 지점이 아니다.
    messages.last().map(|m| m.id.clone())
}
```

크기가 넘었다고 사유 없이 쪼개거나, 테스트 줄수 때문에 프로덕션 파일을 나누지 않는다.

```rust
// common/api.rs(89줄 프로덕션 + 158줄 테스트, 총 247줄)를 "247줄이니 300에 가깝다"며
// api_core.rs/api_tests_helpers.rs로 쪼개면 테스트 볼륨이 분리를 촉발한 것 — 신호가 아니다.
```

서브커맨드 0~1개짜리 소형 도구를 과잉 분리하지 않는다.

```rust
// tools/hello처럼 150줄 이하·서브커맨드 없는 도구에서
// 16줄짜리 trait 하나를 위해 src/greeter.rs를 새로 만들지 않는다.
trait Greeter {
    fn greet(&self, name: &str) -> String;
}
```

## 체크리스트

- [ ] 파일 이름만 보고 내용을 예측할 수 있는가 (파일당 기능 1개)
- [ ] 루트가 `main.rs`(배선)·`cli.rs`(명령 표면) + 기능 디렉토리 + `common/`(공유 기반) 3층인가
- [ ] 새 서브커맨드가 기존 `commands/*.rs`를 고치지 않고 새 파일 추가로 끝나는가
- [ ] `common/`에 둘 이상의 기능이 공유하는 코드만 있는가 (특정 기능 전용 코드가 섞이지 않았는가)
- [ ] 프로덕션 코드 ~300줄을 넘겼을 때 분리를 검토했고, 유지 결정이면 사유가 커밋/PR에 남았는가
- [ ] 테스트 줄수만으로 프로덕션 파일 분리를 촉발하지 않았는가
- [ ] 테스트가 기본은 동일 파일 인라인이고, 비대해졌을 때만 `<이름>/tests.rs`로 분리했는가 (통합 `tests/` 디렉토리 아님)
- [ ] 공유 목/픽스처가 `testutil.rs` 하나에 모여 있는가
- [ ] 여러 파일이 쓰는 헬퍼가 개념 이름 파일로 추출되어 있고, `mod.rs`는 선언·재노출만 하는가
- [ ] 가시성이 최소인가 (`pub(super)`/`pub(crate)`를 필요한 범위까지만 사용)
- [ ] 서브커맨드 0~1개·프로덕션 ~150줄 이하 도구가 `main.rs` 단일 파일을 유지하는가
