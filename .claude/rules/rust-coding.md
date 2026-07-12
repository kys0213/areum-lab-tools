---
paths:
  - "**/*.rs"
---

# Rust Coding Convention

> 바이너리 crate 경계에서는 `anyhow`로 실패를 서술하고, 순수 로직은 I/O에서 분리한다.

## 원칙

1. **에러는 즉시 전파한다 (fail-fast)**: 외부 응답이 스펙과 다르면 fallback이나 기본값으로 우회하지 않는다. 방어적 파싱(텍스트 fallback)은 스펙 불일치를 숨겨 디버깅을 어렵게 만든다.
2. **순수 로직과 I/O를 분리한다**: 파싱/검증/변환 같은 순수 함수를 먼저 추출하고, `println!`/파일/네트워크 같은 I/O는 가장자리(`main`)로 밀어낸다.
3. **`unwrap()`/`expect()`는 근거가 있을 때만**: 프로덕션 경로에서 무근거 `unwrap()`은 금지한다. 불변식이 보장된 지점에서는 `expect("왜 안전한지")`로 사유를 명시한다.
4. **API 표면은 최소로, 소유권은 명확히**: 파라미터는 빌린 타입(`&str`, `&[T]`)을 받고 반환은 소유 타입을 돌려준다. 불필요한 `clone()`은 만들지 않는다.
5. **주석은 why만**: PR 번호·버전 시점·변경 이력 narration은 stale해진다 (git blame 영역). 비자명한 제약만 남긴다.
6. **단일 책임**: 파싱/실행/출력 포맷팅이 한 함수·모듈에 혼재하면 분리한다. 변경의 이유가 둘 이상이면 그것이 분리 신호다.
7. **DIP — 구체 구현이 아닌 trait에 의존한다**: HTTP/FS/프로세스 같은 외부 의존성은 trait로 추상화하고 구현을 주입받는다. 코어 로직이 `reqwest::Client` 같은 구체 클라이언트를 직접 호출하지 않는다. 테스트 관점은 `tool-crate.md` 참조.
8. **OCP — 분기 대신 확장 지점을 하나로 수렴한다**: 동일한 `match`/`if` 분기가 여러 함수·모듈에 흩어져 있으면, 새 케이스 추가 시 그 모든 지점을 고쳐야 한다. trait 객체나 제네릭으로 변경 지점을 하나로 모은다.

## DO

`tools/hello/src/main.rs`처럼 순수 함수를 분리하고 동일 파일에서 테스트한다.

```rust
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

바이너리 crate 경계에서 실패 지점을 서술한다.

```rust
let body = response
    .text()
    .await
    .context("failed to read response body from Discord API")?;
```

불변식이 보장된 곳에서만 사유를 명시한 `expect`를 쓴다.

```rust
// clap이 required=true로 검증하므로 None이 될 수 없다.
let token = cli.token.expect("clap guarantees --token is present");
```

외부 의존성을 trait로 추상화해 코어 로직에서 분리한다.

```rust
trait Api {
    fn send(&self, payload: &str) -> anyhow::Result<()>;
}

fn notify(api: &impl Api, message: &str) -> anyhow::Result<()> {
    api.send(message)
}
```

## DON'T

외부 응답이 스펙과 다를 때 fallback으로 우회하지 않는다.

```rust
// 스펙 불일치를 숨긴다 — 실패를 감지해야 할 자리에서 기본값으로 넘어간다.
let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
```

사유 없는 `unwrap()`을 프로덕션 경로에 두지 않는다.

```rust
let config = std::fs::read_to_string(path).unwrap(); // 왜 안전한지 알 수 없다
```

I/O와 순수 로직을 한 함수에 섞지 않는다.

```rust
fn process() {
    let data = std::fs::read_to_string("input.txt").unwrap();
    let parsed = data.trim().to_uppercase(); // 순수 로직이 I/O 함수 내부에 파묻힘
    println!("{parsed}");
}
```

함수 안에서 구체 클라이언트를 직접 생성하고 호출하지 않는다.

```rust
fn notify(message: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new(); // 코어 로직이 구체 구현에 결합됨 — mock으로 대체 불가
    client.post("https://example.com").body(message.to_owned()).send()?;
    Ok(())
}
```

## 체크리스트

- [ ] 외부 의존성 실패 시 fallback 없이 즉시 에러 전파하는가
- [ ] `unwrap()`/`expect()`에 무근거 사용이 없고, 있다면 사유가 주석으로 남아 있는가
- [ ] 순수 함수와 I/O가 분리되어 있는가 (테스트 가능한 형태인가)
- [ ] 주석이 why/제약만 담고 이력 narration이 없는가
- [ ] 파싱/실행/출력 포맷팅이 한 함수·모듈에 혼재하지 않는가
- [ ] 외부 의존성이 trait로 추상화되어 코어 로직이 구체 구현에 직접 의존하지 않는가
- [ ] `cargo fmt`·`clippy -D warnings`·`test`를 통과하는가
