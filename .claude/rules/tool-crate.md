---
paths:
  - "tools/*/src/**/*.rs"
---

# Tool Crate Convention

> `tools/<name>/`는 독립 CLI crate다. 1차 소비자가 AI 에이전트임을 전제로 설계한다.

## 원칙

1. **`main.rs`는 파싱 + 위임만 한다**: `clap`으로 인자를 파싱한 뒤 순수 함수/모듈에 위임한다. 비즈니스 로직을 `main` 함수 본문에 직접 쓰지 않는다.
2. **에이전트 친화 출력 계약**: 이 도구들은 사람이 아니라 AI 에이전트가 Bash로 직접 호출해 쓴다. 기계 파싱 가능한 출력(JSON)을 기본으로 하고, 사람이 읽는 포맷은 opt-in 플래그로 둔다. 진단/로그는 stdout이 아닌 stderr로, exit code는 성공(0)/실패(비0)를 의미 있게 구분한다.
3. **블랙박스 TDD**: 공개 API(순수 함수 시그니처, CLI 동작) 기준으로 테스트한다. 내부 구현 세부사항에 결합된 테스트는 작성하지 않는다. `tools/hello`의 `greeting()` 테스트처럼 순수 함수는 동일 파일의 `#[cfg(test)] mod tests`에 둔다.
4. **외부 의존성은 추상화 뒤로**: HTTP·파일시스템·프로세스 호출은 trait로 추상화하고, 테스트는 mock 구현체로 대체한다. E2E 테스트는 단위 테스트와 디렉토리/파일명으로 분리한다.
5. **시크릿/설정은 `~/.areum/<tool>/`**: 도구별 config·시크릿 파일은 이 경로 규약을 따르고, 시크릿 파일 권한은 600으로 제한한다.
6. **선제 추상화 금지 (Rule of 3)**: 도구 간 코드 복붙이 세 번째로 발생하는 시점에 `libs/`로 추출한다. 두 번째 복붙까지는 각 도구에 그대로 둔다.

## DO

`main.rs`는 파싱과 위임만 담당한다 (`tools/hello/src/main.rs` 실례).

```rust
use clap::Parser;

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
```

에이전트가 소비할 결과는 JSON 기본, 사람용은 플래그로 분기한다.

```rust
#[derive(Parser)]
struct Cli {
    /// 사람이 읽기 좋은 텍스트 출력 (기본은 JSON)
    #[arg(long)]
    human: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let result = run(&cli)?;
    if cli.human {
        println!("{}", result.to_human_string());
    } else {
        println!("{}", serde_json::to_string(&result)?);
    }
    Ok(())
}
```

## DON'T

`main` 함수에 비즈니스 로직을 직접 쓰지 않는다.

```rust
fn main() {
    let cli = Cli::parse();
    // 파싱, 검증, 포맷팅이 전부 main에 뒤섞임 — 테스트 불가능
    let trimmed = cli.name.trim();
    let capitalized = format!("{}{}", &trimmed[..1].to_uppercase(), &trimmed[1..]);
    println!("Hello, {capitalized}!");
}
```

외부 의존성(HTTP 클라이언트)을 구체 타입으로 직접 호출해 테스트를 불가능하게 만들지 않는다.

```rust
async fn fetch_user(id: &str) -> anyhow::Result<User> {
    // reqwest::Client가 하드코딩되어 mock으로 대체할 수 없다
    let client = reqwest::Client::new();
    let resp = client.get(format!("https://api.example.com/users/{id}")).send().await?;
    Ok(resp.json().await?)
}
```

사람용 로그를 stdout에 섞어 기계 파싱을 깨뜨리지 않는다.

```rust
fn main() {
    println!("Fetching data..."); // 진단 메시지가 JSON 출력과 섞여 파싱 실패
    println!("{}", serde_json::to_string(&data).unwrap());
}
```

## 체크리스트

- [ ] `main.rs`가 파싱과 위임만 하고 로직은 별도 함수/모듈에 있는가
- [ ] 기본 출력이 JSON이고, 사람용 포맷은 opt-in인가
- [ ] 진단/로그가 stderr로 가는가
- [ ] exit code가 성공/실패를 의미 있게 구분하는가
- [ ] 외부 의존성이 추상화되어 mock으로 테스트 가능한가
- [ ] 순수 함수 테스트가 동일 파일 `#[cfg(test)] mod tests`에 있는가
