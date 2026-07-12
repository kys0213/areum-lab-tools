---
paths:
  - "Cargo.toml"
  - "tools/*/Cargo.toml"
---

# Manifest Convention

> 공용 의존성 버전은 워크스페이스에서만 관리하고, crate의 `version`은 항상 리터럴이다.

## 원칙

1. **공용 의존성은 `[workspace.dependencies]`에서만 버전을 정한다**: crate의 `[dependencies]`는 `{ workspace = true }`로 참조한다. crate에 개별 버전을 명시하지 않는다.
2. **`edition`/`rust-version`은 워크스페이스 상속**: `edition.workspace = true`, `rust-version.workspace = true`로 SSOT를 유지한다.
3. **`version`은 crate별 리터럴, 워크스페이스 상속 금지**: 릴리스 태그(`<tool>-vX.Y.Z`)와 `make dist-tag`/`make dist-build`의 버전 추출(`grep -m1 '^version = '`)이 리터럴 문자열을 전제로 동작한다.
4. **`members = ["tools/*"]` glob은 0매칭이면 전체가 깨진다**: 이 glob이 비면 `cargo` 명령 전체가 실패한다. `tools/hello`는 구조적 placeholder이므로 삭제하지 않는다 (삭제하려면 먼저 `members`를 명시 리스트로 전환해야 한다).
5. **`Cargo.lock`은 커밋 대상**: 워크스페이스 루트의 `Cargo.lock`을 `.gitignore`에 추가하지 않는다.

## DO

루트 `Cargo.toml`의 워크스페이스 의존성 선언 (`Cargo.toml` 실례).

```toml
[workspace]
resolver = "3"
members = ["tools/*"]

[workspace.package]
edition = "2024"
rust-version = "1.96"

[workspace.dependencies]
clap = { version = "4", features = ["derive"] }
anyhow = "1"
```

crate는 workspace 참조와 리터럴 `version`을 함께 쓴다 (`tools/hello/Cargo.toml` 실례).

```toml
[package]
name = "hello"
version = "0.0.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
clap = { workspace = true }
```

## DON'T

crate에 개별 버전을 다시 명시하지 않는다.

```toml
[dependencies]
clap = { version = "4.5.3", features = ["derive"] } # 워크스페이스 버전과 드리프트 발생
```

`version`을 워크스페이스 상속으로 바꾸지 않는다.

```toml
[package]
version.workspace = true # 태그 파싱(grep -m1 '^version = ')과 dist-tag가 깨진다
```

`tools/hello`를 삭제하지 않는다 (glob 0매칭 = 전체 cargo 명령 실패).

```toml
# tools/hello/ 삭제 후 members = ["tools/*"] 유지 → cargo build 즉시 실패
```

## 체크리스트

- [ ] 공용 의존성이 `[workspace.dependencies]`에만 버전을 갖고 crate는 `{ workspace = true }`로 참조하는가
- [ ] `edition`/`rust-version`이 워크스페이스 상속인가
- [ ] `version`이 crate별 리터럴 문자열인가 (워크스페이스 상속 아님)
- [ ] `members` glob이 최소 1개 이상 매칭되는가
- [ ] `Cargo.lock`이 `.gitignore`에 없는가
