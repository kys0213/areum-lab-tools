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

### 릴리스 설치 (권장)

GitHub Release에서 미리 빌드된 바이너리를 받아 설치한다. 현재
**Apple Silicon macOS**만 지원한다.

**curl 원라이너** (레포 클론 불필요, 새 머신에서 부트스트랩):

```sh
curl -fsSL https://raw.githubusercontent.com/kys0213/areum-lab-tools/main/scripts/install.sh \
  | sh -s -- <tool> [version]

# 예: hello 최신 릴리스 설치 (특정 버전은 `-- hello 0.0.0` 처럼 지정)
curl -fsSL https://raw.githubusercontent.com/kys0213/areum-lab-tools/main/scripts/install.sh \
  | sh -s -- hello
```

버전을 생략하면 해당 도구의 최신 릴리스를 자동으로 조회한다. 바이너리는
체크섬(sha256)으로 검증한 뒤 `~/.local/bin`에 설치된다. `INSTALL_ROOT`
환경 변수로 설치 경로를 변경할 수 있다.

**레포 클론 후 설치** (또는 개발 환경):

```sh
make install-release TOOL=hello               # GitHub Release에서 다운로드, 검증, 설치
make install-release TOOL=hello VERSION=0.0.0 # 특정 버전 지정
```

### 소스 빌드

로컬 설치는 `cargo install --path`를 감싼 `make install`/`make install-all`로
수행한다.

```sh
make install TOOL=hello                     # $HOME/.local 에 설치
make install TOOL=hello INSTALL_ROOT=/path   # 설치 경로 지정
make install-all                             # tools/* 전체 설치
```

### cargo install --git (폴백)

Rust가 설치된 환경에서 Git 레포지토리에서 직접 빌드하여 설치한다.

```sh
cargo install --git https://github.com/kys0213/areum-lab-tools hello
cargo install --git https://github.com/kys0213/areum-lab-tools hello --tag hello-v0.0.0
```

## 배포 (Release)

도구별로 독립적으로 릴리스한다. 태그 규약: `<tool>-vX.Y.Z` (예: `hello-v0.0.0`).
**version은 항상 태그의 마지막 `-v` 뒤**다 (도구 이름에 `-v`가 포함돼도 안전하게
분리된다). 태그의 버전은 `tools/<tool>/Cargo.toml`의 `version` 리터럴과 반드시
일치해야 하며, 불일치 시 `release.yml`이 즉시 실패한다.

### 릴리스 절차 (CI 경유)

```sh
# tools/<tool>/Cargo.toml의 version을 먼저 bump한 뒤 커밋/main 반영
make dist-tag TOOL=hello   # 워킹트리 clean + main 브랜치일 때만 태그 push
```

`dist-tag`가 `hello-v0.0.0` 같은 태그를 push하면 GitHub Actions
(`.github/workflows/release.yml`)가 macOS arm64(`aarch64-apple-darwin`)
러너에서 바이너리를 빌드해 tar.gz로 패키징하고, 해당 태그의 GitHub Release에
첨부한다.

### 다른 머신에 설치하기

릴리스 바이너리는 `install.sh`로 자동 다운로드 및 설치된다
([위의 Install 섹션](#install) 참고). Rust가 설치된 환경이라면
`cargo install --git`을 사용할 수도 있다.

### 무설정 경로 (CI 없이 로컬에서)

같은 아키텍처(macOS arm64) 머신에 직접 배포할 때는 CI 없이 로컬 빌드 후
scp로 옮기면 된다.

```sh
make dist-build TOOL=hello   # dist/hello-0.0.0-aarch64-apple-darwin.tar.gz 생성
scp dist/hello-0.0.0-aarch64-apple-darwin.tar.gz user@host:/tmp/
```

## 디렉토리 구조

```
.
├── Cargo.toml              # 워크스페이스 매니페스트 (members = tools/*)
├── rust-toolchain.toml     # Rust 버전 고정 (SSOT)
├── Makefile                # build/test/lint/install/new 등 태스크 러너
├── templates/tool/         # `make new`가 사용하는 스캐폴드 템플릿 (.tmpl)
├── scripts/                # 설치 스크립트
├── tools/
│   └── hello/               # 예시 CLI 도구
└── docs/                    # 설계/조사 노트
```
