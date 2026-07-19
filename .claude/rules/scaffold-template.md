---
paths:
  - "templates/tool/**"
---

# Scaffold Template Convention

> `templates/tool/`은 `tools/hello`의 미러다. 한쪽을 바꾸면 다른 쪽도 함께 바꾼다.

## 원칙

1. **`tools/hello`와 컨벤션을 동기화한다**: clap derive + 순수 함수 분리 + 동일 파일 테스트라는 컨벤션이 바뀌면 `tools/hello`와 `templates/tool/`을 함께 갱신한다. 벌어지면 `make new` 직후 `make check`가 깨진다. `tools/hello`와 `templates/tool/`은 flat 단일 파일 구조를 유지하는 기준 사례다 (졸업 기준은 `module-structure.md` 참조).
2. **`{{NAME}}`은 `make new`의 sed 치환 계약**: Makefile의 `new` 타깃이 `sed 's/{{NAME}}/$(NAME)/g'`로 치환한다. 다른 템플릿 문법(`{{ name }}`, `${NAME}` 등)을 도입하지 않는다.
3. **최소 골격만 담는다**: 템플릿은 컴파일되는 가장 작은 예시여야 한다. 특정 도구의 기능(HTTP 호출, 파일 파싱 등)을 반영하지 않는다.
4. **`make new`는 순수 파일 생성**: cargo를 호출하지 않는다 (`README.md`: "Cargo.lock을 오염시키지 않는다"). 템플릿이 후처리 cargo 명령을 암묵적으로 요구하는 구조를 만들지 않는다.

## DO

`templates/tool/src/main.rs.tmpl`은 `tools/hello/src/main.rs`와 구조가 동일하고 이름만 플레이스홀더다.

```rust
use clap::Parser;

/// Minimal placeholder CLI for {{NAME}}.
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

`templates/tool/Cargo.toml.tmpl`도 `tools/hello/Cargo.toml`과 동일한 workspace 참조 구조를 유지한다.

```toml
[package]
name = "{{NAME}}"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
clap = { workspace = true }
```

## DON'T

`tools/hello`만 리팩터링하고 템플릿을 남겨두지 않는다.

```rust
// tools/hello/src/main.rs만 output.rs로 분리하고
// templates/tool/src/main.rs.tmpl은 옛 구조 그대로 두면
// make new 직후 새 도구가 hello와 다른 골격으로 시작한다
```

`{{NAME}}` 외의 치환 문법을 쓰지 않는다.

```toml
name = "${NAME}" # Makefile의 sed 's/{{NAME}}/.../ '가 매칭하지 못해 치환되지 않는다
```

템플릿에 특정 기능을 미리 넣지 않는다.

```rust
// 모든 새 도구가 HTTP 클라이언트를 필요로 하지는 않는다 — 최소 골격 원칙 위반
async fn fetch(url: &str) -> anyhow::Result<String> { ... }
```

## 체크리스트

- [ ] `tools/hello`와 `templates/tool/`의 구조가 동일한가
- [ ] `{{NAME}}` 플레이스홀더만 사용하는가
- [ ] 템플릿이 컴파일 가능한 최소 골격을 유지하는가
- [ ] `make new NAME=<test>` 직후 `make check`가 통과하는가
