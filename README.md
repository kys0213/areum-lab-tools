# areum-lab-tools

로컬에서 실행하는 작은 Rust CLI 도구 모음. 각 도구는 `tools/<name>/` 아래의
독립된 crate이며, AI 에이전트(Claude Code 등)가 Bash로 직접 호출해 사용하는
도구도 포함한다.

## 요구 환경

- [rustup](https://rustup.rs/)만 있으면 된다. 이 레포는 `rust-toolchain.toml`로
  Rust 버전(1.96.0, `rustfmt`/`clippy` 포함)을 고정하고 있어, `cargo`/`make`
  명령을 처음 실행하는 시점에 rustup이 해당 툴체인을 자동으로 설치한다.

## 빠른 시작

```sh
make help                 # 사용 가능한 타깃 목록 확인
make check                 # fmt-check + lint + test (push 전 로컬 게이트)
make install TOOL=hello    # tools/hello 를 $HOME/.local 에 설치
```

## 새 도구 추가하기

```sh
make new NAME=foo   # templates/tool/ 을 tools/foo/ 로 복사 (Cargo.lock 미변경)
# tools/foo/src/main.rs 구현
make check           # fmt/lint/test 통과 확인
```

`make new`는 `cargo`를 호출하지 않는다. 파일만 복사하므로 워크스페이스의
`Cargo.lock`을 오염시키지 않는다. 이후 `cargo build`/`make check` 등 cargo가
실행되는 시점에 비로소 `tools/*` glob 멤버로 편입된다.

## 검증 책임 (macOS / Apple Silicon)

CI는 ubuntu 전용으로 동작한다. **macOS/Apple Silicon 환경에서의 동작 검증은
로컬 `make check` 실행이 책임진다** — CI가 macOS를 대신 검증해 주지 않는다.
변경 사항을 push하기 전에 반드시 로컬에서 `make check`를 통과시켜야 한다.

## Install

로컬 설치는 `cargo install --path`를 감싼 `make install`/`make install-all`로
수행한다.

```sh
make install TOOL=hello                     # $HOME/.local 에 설치
make install TOOL=hello INSTALL_ROOT=/path   # 설치 경로 지정
make install-all                             # tools/* 전체 설치
```

배포판(dist) 빌드/태깅 관련 문서는 이후 추가된다.

## 디렉토리 구조

```
.
├── Cargo.toml              # 워크스페이스 매니페스트 (members = tools/*)
├── rust-toolchain.toml     # Rust 버전 고정 (SSOT)
├── Makefile                # build/test/lint/install/new 등 태스크 러너
├── templates/tool/         # `make new`가 사용하는 스캐폴드 템플릿 (.tmpl)
├── tools/
│   └── hello/               # 예시 CLI 도구
└── docs/                    # 설계/조사 노트
```
