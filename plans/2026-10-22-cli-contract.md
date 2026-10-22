# cli-contract — 출력 봉투·render·시간 헬퍼를 `libs/` 로 추출

> **Plan — 이 시점의 결정 기록.** 현재 정책의 신뢰 소스가 아니다.
> 날짜: 2026-10-22 · 브랜치: refactor/cli-contract (base: epic/doppel) · 이슈: 없음 · 승인 출처: 사용자 승인 (2026-10-22, 결정 1·2·3 모두 제안안 채택)

## 왜

`output/envelope.rs`·`output/mod.rs` 의 `render`·`common/error.rs`·`common/time.rs` 가 discord·kanban·doppel 에 세 벌 있다. `tool-crate.md` 원칙 6 (Rule of 3) 은 세 번째 복사에서 `libs/` 로 추출하라고 하고, `docs/kanban-board.md §7` 은 세 번째 도구가 나올 때까지 미뤘다. doppel 이 그 세 번째이고, P0 plan 이 이 추출을 별도 작업으로 뺐다.

## 요구사항

1. `libs/cli-contract` 크레이트: 제네릭 봉투(`success_json`·`error_json`), render/sink, 타임스탬프 헬퍼.
2. 도구 고유 `Payload`·`ErrorKind`·`AppError` 는 도구에 남긴다. 도구 고유 kind 가 공유 매핑에 어떻게 붙는지 정한다. 기존 exit code 는 바꾸지 않는다.
3. 워크스페이스 `members` 에 `libs/*` 추가, `tools/hello` placeholder 유지.
4. 세 도구 이관. JSON 출력·exit code 는 바이트 단위로 같아야 한다. 각 도구 `output/envelope.rs` 의 리터럴 계약 테스트는 **`mod tests` 본문을 고치지 않고** 통과해야 한다.
5. `module-structure.md`·`rust-coding.md`·`manifest.md` 준수, `make check` 통과.

## 현황 조사 (사이드이펙트)

세 사본은 "거의" 같다. 차이가 설계 결정을 강제한다.

| 항목 | discord | kanban | doppel |
|---|---|---|---|
| `envelope.rs` 프로덕션 | 동일 | 동일 | 동일 |
| `render` | 동일 | 동일 | 동일 |
| `AppError` 필드 | kind·message·http_status·retry_after_ms | 같음 (api/rate_limit 를 실제로 만드는 곳은 없음) | kind·message |
| kind → exit | usage 2, config 3, **auth 3**, api 4, rate_limit 5, network 6, internal 1 | internal 1, usage 2, config 3, api 4, rate_limit 5, network 6, not_found 7, conflict 8 | internal 1, usage 2, config 3, network 6, not_found 7, conflict 8 |
| 시계가 1970 이전일 때 | **0 으로 clamp** (`unwrap_or(0)`) | `expect` (panic) | `expect` (panic) |
| `time.rs` 고유 함수 | `rfc3339_after_secs` | 없음 | `epoch_from_canonical`·`days_from_civil`·`canonical_from_rfc3339`·`canonical_from_date`·`has_datetime_shape` |

- `network` 는 세 도구 모두 6 이다. "discord 고유" 가 아니라 공유 kind 다 (Rule of 3 충족).
- `api`(4)·`rate_limit`(5) 는 discord·kanban 두 곳. `auth` 는 discord 만, exit 는 config 와 같은 3.
- `AppError` 를 역직렬화하는 곳은 없다 (`Deserialize` derive 만 있음). `impl From<_> for AppError` 도 없다.
- 테스트는 `render("init", &Ok(payload), true)` 처럼 에러 타입을 적지 않고 부른다. 공유 `render` 가 제네릭이면 `E` 를 추론할 수 없다 → 도구 쪽에 타입을 고정하는 얇은 `render` 가 남아야 한다.
- kanban·discord 봉투 테스트는 `use super::*;` 로 `Payload`·`AppError` 를 가져온다 → `envelope.rs` 상단 `use` 두 줄은 남는다.
- CI·Makefile·release: `cargo … --workspace` 는 lib 를 자동 포함한다. `release.yml`·`dist-build` 는 `-p <tool>` 이라 lib 는 의존성으로만 빌드된다. `install-all`·`release_targets.sh` 는 `tools/*/` 만 보므로 영향 없다.
- 버전: 동작이 그대로이므로 도구 버전 범프 없음. 다음 릴리스에 함께 실린다.

## 설계

### 크레이트 배치

```
libs/cli-contract/
├── Cargo.toml          # name = "cli-contract", version = "0.0.0", publish = false
└── src/
    ├── lib.rs          # mod 선언 + 재노출만
    ├── envelope.rs     # success_json / error_json
    ├── render.rs       # Sink, HumanText, ExitStatus, render
    ├── kind.rs         # CommonKind — 공유 kind 이름·exit code 표
    └── time.rs         # now_epoch_secs, now_rfc3339, format_epoch_secs (+ private civil_from_days)
```

의존성은 `serde`·`serde_json` (workspace 참조) 뿐이다.

### 공개 API

```rust
// envelope.rs — T 의 Serialize 가 실패하지 않아야 한다(문자열 키 map 등 plain data). expect 사유에 명시.
pub fn success_json<T: Serialize + ?Sized>(command: &str, data: &T) -> String;
pub fn error_json<E: Serialize + ?Sized>(command: &str, error: &E) -> String;

// render.rs — 입력 계약을 역할별로 나눈다 (ISP)
pub trait HumanText { fn to_human(&self) -> String; }
pub trait ExitStatus { fn exit_code(&self) -> i32; }
pub enum Sink { Stdout, Stderr }
pub fn render<P, E>(command: &str, result: &Result<P, E>, json: bool) -> (Sink, String, i32)
where P: Serialize + HumanText, E: Serialize + HumanText + ExitStatus;

// kind.rs
pub enum CommonKind { Internal, Usage, Config, Network, NotFound, Conflict }
impl CommonKind { pub const fn name(self) -> &'static str; pub const fn exit_code(self) -> i32; }

// time.rs
pub fn now_epoch_secs() -> u64;
pub fn now_rfc3339() -> String;
pub fn format_epoch_secs(secs: u64) -> String;
```

봉투 구조체(`SuccessEnvelope`/`ErrorEnvelope`)의 필드 순서 `ok, command, data|error` 는 그대로 옮긴다 — 바이트 동일성의 핵심이다.

### 에러 kind 확장 방식

**도구의 `ErrorKind` 는 평평한 enum 으로 도구에 남고, 공유 kind 의 이름·번호만 `CommonKind` 에 위임한다.**

```rust
// tools/kanban/src/common/error.rs (이관 후)
impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Usage => CommonKind::Usage.name(),
            // … 공유 kind 는 전부 CommonKind 에 위임
            ErrorKind::Api => "api",            // 도구 고유
            ErrorKind::RateLimit => "rate_limit",
        }
    }
}
pub fn exit_code(err: &AppError) -> i32 {
    match err.kind {
        ErrorKind::Usage => CommonKind::Usage.exit_code(),
        // …
        ErrorKind::Api => 4,
        ErrorKind::RateLimit => 5,
    }
}
impl HumanText for AppError { fn to_human(&self) -> String { AppError::to_human(self) } }
impl ExitStatus for AppError { fn exit_code(&self) -> i32 { exit_code(self) } }
```

- `CommonKind` = 세 도구가 모두 쓰거나 도구 무관한 의미의 kind: internal 1, usage 2, config 3, network 6, not_found 7, conflict 8.
- 도구 고유 kind 는 도구가 번호를 정한다. 이미 쓰인 번호는 `CommonKind` 문서에 예약으로 남긴다: 4 `api`·5 `rate_limit` (discord·kanban HTTP upstream), discord `auth` 는 config 와 같은 3 을 공유하는 별칭. 새 도구 고유 kind 는 9 부터 쓰고, 기존 번호는 다시 매기지 않는다. 세 번째 도구가 `api`/`rate_limit` 을 쓰면 그때 `CommonKind` 로 올린다.
- `ErrorKind` 의 serde 직렬화(`rename_all = "snake_case"`)·`AppError` 구조체·`to_human` 형식은 도구에 그대로 둔다. 호출부(`ErrorKind::Usage`, `AppError { … }` 리터럴) 변경이 없다.

### 도구 쪽 이관 후 모습

- `output/mod.rs`: `#[cfg(test)] mod envelope;` 로 바뀌고, 기존 재노출(`AppError`·`ErrorKind`·`exit_code`·payload 타입) 유지, `pub use cli_contract::Sink;`, 그리고 타입만 고정하는 한 줄 wrapper:
  ```rust
  pub fn render(command: &str, result: &Result<Payload, AppError>, json: bool) -> (Sink, String, i32) {
      cli_contract::render(command, result, json)
  }
  ```
  `mod.rs` 에 로직을 두지 않는다는 원칙 7 에는 걸리지 않는 수준(분기 없는 타입 고정)이라 별도 파일로 빼지 않는다.
- `output/envelope.rs`: 프로덕션 코드가 빠지고, 도구 Payload 로 봉투 계약을 고정하는 테스트 파일로 남는다. 상단 `use` 와 `mod tests` 본문은 그대로.
- `output/payload.rs`: `impl HumanText for Payload` (기존 inherent `to_human` 위임) 추가.
- `common/time.rs`: kanban 은 파일 삭제, 호출부가 `cli_contract::time` 을 쓴다. discord 는 `rfc3339_after_secs` 만, doppel 은 정규화 파서들만 남는다. 공통 함수의 테스트(epoch 0, 알려진 시각, 윤일, 사전순=시간순, now 모양)는 lib 로 옮긴다. 도구 고유 함수의 테스트는 도구에 남는다.
- `common/error.rs`·`output/mod.rs` 의 기존 테스트는 고치지 않는다.

### 규칙·문서 갱신

- `Cargo.toml`: `members = ["tools/*", "libs/*"]`.
- `.claude/rules/manifest.md`: `paths` 에 `libs/*/Cargo.toml`, members 예시 갱신, lib 는 `version = "0.0.0"` 리터럴 + `publish = false`.
- `.claude/rules/tool-crate.md` 원칙 6: 추출된 공용 계약은 `libs/cli-contract` 를 쓴다는 한 줄.
- `.claude/rules/lib-crate.md` (신규, `paths: libs/**`): libs 에는 도구 무관·결정적 계약만 둔다 (도구 Payload·고유 kind 금지), 공개 API 를 바꾸면 모든 소비 도구를 같은 PR 에서 이관한다.
- `CLAUDE.md` 네이밍: `libs/<kebab-case>/` 한 줄.
- `docs/kanban-board.md §7`: 모듈 트리(`common/time.rs` 제거, `envelope.rs` 설명)와 "discord와의 코드 중복" 절을 추출 완료로 갱신 (spec drift 방지).

## 구현 순서 (TDD)

1. `libs/cli-contract` 뼈대 + members 추가. 블랙박스 테스트 먼저: 테스트용 payload/error 구조체로 봉투 리터럴 JSON, render 의 sink·exit code (human/json 모드), `CommonKind` 이름·번호 표, time 테스트 이전 → fail 확인 → 구현.
2. doppel 이관 (가장 단순: http 필드 없음) → `cargo test -p doppel`.
3. kanban 이관 (time.rs 삭제 포함) → `cargo test -p kanban`.
4. discord 이관 (`auth`·clamp 결정 반영) → `cargo test -p discord`.
5. 규칙·문서 갱신, `make check`.

## 검증

- `make check` (fmt-check, clippy `-D warnings`, test).
- `git diff epic/doppel -- 'tools/*/src/output/envelope.rs'` 에서 `#[cfg(test)]` 이하 변경이 0 줄인지 확인. `common/error.rs`·`output/mod.rs` 의 테스트 블록도 같은 방식으로 확인.
- 테스트 수: lib 로 옮긴 time 테스트만큼 도구 쪽이 줄고 lib 쪽이 는다. 옮긴 목록을 PR 에 적는다.

## 확정된 결정 (승인 시 선택)

1. **1970 이전 시계 정책.** 공유 `now_epoch_secs` 를 kanban·doppel 처럼 `expect`(fail-fast) 로 통일하고 discord 의 clamp 를 버리는 것을 제안한다. clamp 면 discord ask 의 `timeout_at` 이 1970 년이 되어 바로 만료된다 — 숨은 오동작보다 panic 이 낫다 (`rust-coding.md` 원칙 1). 실제로 도달할 수 없는 상태라 JSON·exit code 계약에는 영향이 없다. 대안: discord 만 자체 `now_epoch_secs` 를 유지.
2. **`network` 의 소속.** 과제 설명은 discord 고유로 봤지만 세 도구 모두 6 이라 `CommonKind` 에 넣는 것을 제안한다. `api`·`rate_limit` 은 두 도구라 도구에 둔다.
3. **브랜치.** `epic/doppel` 에서 `refactor/cli-contract` 를 따서 `epic/doppel` 로 PR 하는 것을 제안한다 (doppel 이 main 에 없으므로). 대안: epic 이 main 에 들어간 뒤 main 기준으로 진행.

## 버린 선택지

- **공유 `ErrorKind` 하나에 모든 kind 합집합** — discord 가 쓰지 않는 not_found/conflict, doppel 이 쓰지 않는 api 를 떠안는다. 도구별 kind 목록이 문서(§8·§9)와 어긋난다.
- **`ErrorKind::Common(CommonKind)` 중첩 enum** — `ErrorKind::Usage` 호출부 수십 곳이 바뀌고, `kind` 직렬화를 맞추려면 `untagged` 같은 serde 조정이 필요하다.
- **제네릭 `AppError<K>` 를 lib 에** — 도구가 `impl From<외부 에러> for AppError` 를 더할 수 없게 된다 (orphan rule). `http_status`·`retry_after_ms` 는 HTTP 를 쓰는 도구의 관심사다.
- **도구 `envelope.rs` 삭제·테스트 이동** — 리터럴 계약 테스트를 그대로 두라는 요구와 어긋나고, 도구 Payload 기준 계약 고정이 사라진다.
- **`civil_from_days`·`days_from_civil` 공개** — `days_from_civil` 은 doppel 만 쓴다. lib 공개 API 는 실제 공유되는 것만.

## 범위 밖

daemon pid 처리 중복(discord·kanban, `docs/kanban-board.md §7` 언급 — 아직 두 벌), `templates/tool` 스캐폴드가 lib 를 쓰게 하는 것, `docs/discord-agent-cli.md` 의 낡은 경로(`output.rs`) 참조.
