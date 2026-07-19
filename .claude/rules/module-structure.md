---
paths:
  - "tools/*/src/**/*.rs"
---

# Module Structure Convention

> 파일 이름만 보고 내용을 예측할 수 있어야 한다. 서브커맨드가 여럿인 도구는 배선(`main.rs`)·명령 표면(`cli.rs`) + 기능 디렉토리 + 공유 기반(`common/`)의 3층 구조를 따른다.

## 원칙

1. **파일명 = 기능 (discoverability first)**: 파일 이름만 보고 내용을 예측할 수 있어야 한다. 한 파일에 서로 다른 기능 2개 이상을 담지 않는다.
2. **루트 3층 구조**: `main.rs`(배선)·`cli.rs`(명령 표면) + 기능 디렉토리(예: `commands/`) + 공유 기반(`common/`)으로 구성한다.
3. **서브커맨드가 여럿이면 `commands/<이름>.rs` 파일당 하나**: 새 서브커맨드는 새 파일을 추가하는 것으로 끝난다 — 기존 파일은 건드리지 않는다 (OCP).
4. **`common/` grab-bag 가드**: `common/`에는 **둘 이상의 기능이 공유하는 기반**(config/error/외부 API 계약 등)만 둔다. 특정 기능 전용 코드가 들어가면 그 기능 파일로 되돌린다.
5. **크기는 하드 리밋이 아닌 신호**: 프로덕션 코드(테스트 제외) ~300줄이면 분리를 검토한다. 테스트 줄수로 분리를 촉발하지 않는다. 검토 후 유지로 결정하면 커밋/PR에 사유를 남긴다.
6. **테스트 배치**: private 함수를 블랙박스로 검증하므로 `#[cfg(test)]` 모듈이 필수다 (통합 `tests/` 디렉토리는 private 접근이 안 되므로 금지). 기본은 동일 파일 인라인, 프로덕션 탐색을 해칠 만큼 커지면 `<이름>/tests.rs` 자식 모듈로 분리한다. 공유 목/픽스처는 `testutil.rs`가 단일 소유한다.
7. **공유 로직은 개념 이름 파일로**: 여러 파일이 쓰는 헬퍼는 개념을 드러내는 이름으로 추출한다. `mod.rs`에 로직을 두지 않는다 — `mod.rs`는 선언·재노출만 담당한다.
8. **가시성 최소**: 파일 경계를 넘는 것만 `pub(crate)`(형제 트리까지) 또는 `pub(super)`(부모까지만) 로 연다. 그 외는 private.
9. **과잉 분리 방지 (졸업 기준)**: 서브커맨드 0~1개·프로덕션 ~150줄 이하 도구는 `main.rs` 단일 파일을 유지한다. 짧은 trait/헬퍼 하나를 독립 파일로 빼지 않는다.

## DO

서브커맨드가 여럿인 도구는 배선·명령 표면 + 기능 디렉토리 + 공유 기반의 3층 구조를 따른다.

```
tools/<tool>/src/
├── main.rs                    # mod 선언 + 배선 + 커맨드 디스패치
├── cli.rs                     # 명령 표면 정의 (clap Parser/Subcommand 등)
├── commands/                  # 기능 디렉토리: 서브커맨드별 로직
│   ├── mod.rs                  # 선언 + 재노출만
│   ├── <subcommand-a>.rs
│   ├── <subcommand-b>.rs
│   ├── <subcommand-a>/tests.rs # 비대해졌을 때만 분리하는 테스트 자식 모듈
│   ├── <shared-concept>.rs     # 두 서브커맨드 이상이 공유하는 헬퍼 (개념 이름)
│   └── testutil.rs             # 공유 목/픽스처 단일 소유
└── common/                     # 공유 기반: 2개 이상 기능이 공유
    ├── mod.rs
    ├── config.rs
    ├── error.rs
    └── <shared-infra>.rs
```

서브커맨드는 `commands/<이름>.rs` 파일당 하나로 두고 `commands/mod.rs`는 선언·재노출만 한다. 새 서브커맨드는 새 파일을 추가하는 것으로 끝난다 — 다른 서브커맨드 파일은 건드리지 않는다.

```rust
// commands/mod.rs — 선언 + 재노출만
mod <subcommand_a>;
mod <subcommand_b>;
pub(crate) use <subcommand_a>::run_<subcommand_a>;
pub(crate) use <subcommand_b>::run_<subcommand_b>;
```

```rust
// commands/<subcommand_a>.rs — 새 서브커맨드는 새 파일 추가로 끝난다
pub(crate) fn run_<subcommand_a>(input: &Input) -> Result<Output, AppError> {
    validate(input).map(Output::from)
}
```

`common/`은 두 개 이상의 기능이 공유하는 기반만 담는다.

```rust
// common/config.rs — main.rs와 commands/* 양쪽이 쓰는 설정 로딩 (2개 이상 공유 → common 적합)
pub(crate) fn load_config(path: &Path) -> Result<Config, AppError> { /* ... */ }
```

여러 서브커맨드가 공유하는 헬퍼는 `mod.rs`가 아니라 개념을 드러내는 이름의 파일로 뺀다.

```rust
// commands/<shared-concept>.rs — <subcommand_a>.rs와 <subcommand_b>.rs가 함께 쓰는 헬퍼
pub(crate) fn <shared_helper>(items: &[Item]) -> Option<Cursor> { /* ... */ }
```

테스트는 기본적으로 동일 파일에 인라인으로 두고, 프로덕션 탐색을 해칠 만큼 커지면 `<이름>/tests.rs` 자식 모듈로 분리한다.

```rust
// 기본: 동일 파일 인라인
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn <behavior>_returns_expected() {
        assert_eq!(<function>(<input>), <expected>);
    }
}

// 비대해지면: commands/<subcommand_a>.rs 끝에서 자식 모듈로 위임
#[cfg(test)]
mod tests;
```

가시성은 필요한 만큼만 연다 — 부모 모듈에서만 쓰이면 `pub(super)`, 형제 트리를 넘나들면 `pub(crate)`.

```rust
// 부모 모듈에서만 호출되면 pub(super)
pub(super) fn render_success(data: &OutputData) -> String { /* ... */ }

// 다른 트리(예: common::error)에서도 호출되면 pub(crate)
pub(crate) fn describe(&self) -> String { /* ... */ }
```

서브커맨드가 없거나 하나뿐이고 프로덕션이 ~150줄 이하면 `main.rs` 단일 파일을 유지한다.

```rust
// tools/<tool>/src/main.rs — 서브커맨드 0~1개, 프로덕션 ~150줄 이하면 이 한 파일로 충분하다
fn run(args: &Args) -> Result<Output, AppError> { /* ... */ }

fn main() {
    let args = Args::parse();
    // run() 결과를 여기서 바로 처리한다 — 별도 모듈로 쪼개지 않는다
}
```

## DON'T

한 파일에 서로 다른 기능을 섞지 않는다 — 파일명으로 내용을 예측할 수 없게 된다.

```rust
// commands/<subcommand_a>.rs에 다른 서브커맨드 로직까지 얹으면
// 파일명과 내용이 어긋나고, 하나만 고쳐도 나머지 서브커맨드가 diff에 섞여 리뷰가 어려워진다.
pub(crate) fn run_<subcommand_a>(...) -> Result<Output, AppError> { /* ... */ }
pub(crate) fn run_<subcommand_b>(...) -> Result<Output, AppError> { /* 이 파일에 있을 이유가 없다 */ }
```

`common/`을 grab-bag으로 쓰지 않는다 — 특정 기능 전용 코드는 그 기능 파일로 되돌린다.

```rust
// common/<shared-infra>.rs에 <subcommand_a> 전용 상수를 두면
// 하나만 쓰는 로직이 "공유 기반"인 것처럼 보인다.
pub(crate) const MAX_ITEMS: usize = 10; // commands/<subcommand_a>.rs로 되돌려야 한다
```

새 서브커맨드를 추가하며 기존 커맨드 파일을 함께 고치지 않는다 — 확장은 새 파일 추가로 끝나야 한다.

```rust
// commands/mod.rs에 새 서브커맨드 분기를 직접 얹지 않는다 — 커맨드가 늘 때마다 이 함수를 계속 고쳐야 한다 (OCP 위반).
fn dispatch(cmd: Command) {
    match cmd {
        "<subcommand_a>" => { /* ... */ }
        "<subcommand_c>" => { /* ... */ }
        _ => unreachable!(),
    }
}
```

`mod.rs`에 로직을 두지 않는다 — 선언·재노출만 담당해야 한다.

```rust
// commands/mod.rs에 헬퍼 로직을 직접 넣지 않는다 — 통과 지점일 뿐이다.
mod <subcommand_a>;

pub(crate) fn <shared_helper>(items: &[Item]) -> Option<Cursor> {
    items.last().map(Item::cursor) // 이 로직은 commands/<shared-concept>.rs에 있어야 한다
}
```

크기가 넘었다고 사유 없이 쪼개거나, 테스트 줄수 때문에 프로덕션 파일을 나누지 않는다.

```rust
// 프로덕션 90줄 + 테스트 200줄(총 290줄)을 "300에 가깝다"며
// <이름>_core.rs / <이름>_tests_helpers.rs로 쪼개면 테스트 볼륨이 분리를 촉발한 것 — 신호가 아니다.
```

서브커맨드 0~1개짜리 소형 도구를 과잉 분리하지 않는다.

```rust
// 150줄 이하·서브커맨드 없는 도구에서 10줄짜리 trait 하나를 위해
// src/<concept>.rs를 새로 만들지 않는다.
trait Formatter {
    fn format(&self, value: &str) -> String;
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

실제 적용례는 `tools/discord` 참조.
