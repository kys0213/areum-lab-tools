---
paths:
  - ".github/workflows/*.yml"
  - "Makefile"
---

# CI/Release Convention

> 툴체인은 `rust-toolchain.toml`이 SSOT, 품질 게이트 순서는 로컬과 CI가 항상 같다.

## 원칙

1. **툴체인 버전의 SSOT는 `rust-toolchain.toml`**: 워크플로에 Rust 버전/components를 재기재하지 않는다. `actions-rust-lang/setup-rust-toolchain`이 이 파일을 자동으로 읽는다. 별도 Swatinem 캐시 스텝도 추가하지 않는다 (내장 캐시와 중복).
2. **품질 게이트 순서 고정**: 품질 게이트 3단계(fmt-check → clippy(`-D warnings`) → test)는 `make check`와 `ci.yml`이 같은 명령을 유지한다. `ci.yml`은 여기에 release build 스텝을 추가로 실행한다(설치 산출물 컴파일 게이트) — 드리프트가 생기면 로컬에서 green이어도 CI가 fail할 수 있다.
3. **릴리스 태그 파싱은 fail-fast**: 태그 `<tool>-vX.Y.Z`에서 version은 마지막 `-v` 뒤. 태그 버전과 `Cargo.toml`의 `version`이 불일치하면 경고만 남기고 진행하지 않는다 — 즉시 실패시킨다.
4. **Makefile은 GNU Make 3.81(macOS 기본) 호환**: `.RECIPEPREFIX`, `$(file ...)` 같은 최신 GNU Make 문법을 쓰지 않는다. 탭 인덴트 레시피만 사용하고 `.PHONY`를 유지한다. 새 타깃은 트레일링 `## 설명` 코멘트로 self-document한다.
5. **CI는 ubuntu 전용, macOS 검증은 로컬 책임**: `ci.yml`에 임의로 macOS 잡을 추가하지 않는다 (릴리스 워크플로의 macOS 러너는 예외 — 실제 배포 바이너리 빌드 목적).

## DO

`ci.yml`은 툴체인을 재기재하지 않고 `make check`와 동일한 명령 순서를 따른다.

```yaml
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

`release.yml`은 태그 버전과 매니페스트 버전 불일치를 즉시 fail시킨다.

```yaml
- name: Validate tag version matches Cargo.toml (fail-fast)
  run: |
    MANIFEST_VERSION="$(grep -m1 '^version = ' "$MANIFEST" | sed -E 's/version = "(.*)"/\1/')"
    if [ "$MANIFEST_VERSION" != "$VERSION" ]; then
      echo "::error::tag version ($VERSION) does not match $MANIFEST version ($MANIFEST_VERSION)"
      exit 1
    fi
```

Makefile 타깃은 트레일링 코멘트로 self-document한다.

```makefile
check: fmt-check lint test ## Run fmt-check + lint + test (local pre-push gate)
```

## DON'T

워크플로에 툴체인 버전을 재기재하지 않는다.

```yaml
# rust-toolchain.toml과 별도로 버전을 명시 — SSOT가 두 곳으로 쪼개져 드리프트 발생
- uses: actions-rust-lang/setup-rust-toolchain@v1
  with:
    toolchain: 1.96.0
```

버전 불일치를 경고만 남기고 진행시키지 않는다.

```bash
# fail-fast 원칙 위반 — 불일치를 감지했는데도 빌드를 계속 진행한다
if [ "$MANIFEST_VERSION" != "$VERSION" ]; then
  echo "::warning::version mismatch, continuing anyway"
fi
```

Makefile에 GNU Make 3.81 미지원 문법을 쓰지 않는다.

```makefile
.RECIPEPREFIX = > # macOS 기본 make(3.81)에서 인식되지 않는다
```

`ci.yml`에 macOS 검증 잡을 임의로 추가하지 않는다.

```yaml
# README가 "macOS 검증은 로컬 make check 책임"이라 명시 — CI에 macOS 잡을 얹지 않는다
jobs:
  check-macos:
    runs-on: macos-latest
```

## 체크리스트

- [ ] 워크플로가 `rust-toolchain.toml`을 재기재하지 않는가
- [ ] `ci.yml`과 `make check`의 명령 순서가 같은가 (fmt-check → clippy → test)
- [ ] 릴리스 워크플로가 태그/매니페스트 버전 불일치를 fail-fast로 처리하는가
- [ ] Makefile이 GNU Make 3.81 호환 문법만 쓰는가 (탭 레시피, `.PHONY`)
- [ ] 새 Makefile 타깃에 트레일링 `## 설명`이 있는가
- [ ] `ci.yml`에 불필요한 macOS 잡이 없는가
