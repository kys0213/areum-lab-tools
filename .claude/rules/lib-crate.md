---
paths:
  - "libs/**"
---

# Lib Crate Convention

> `libs/<name>/`는 세 도구 이상이 실제로 공유하는 결정적 계약만 담는다. 도구 고유 타입은 도구에 남는다.

## 원칙

1. **세 번째 사본에서만 들어온다**: `tool-crate.md`의 Rule of 3을 통과한 코드만 둔다. 두 도구만 쓰는 것은 lib에 넣지 않는다.
2. **도구 무관·결정적 계약만**: 봉투 모양, exit code 표, 시각 포맷처럼 같은 입력에 같은 출력을 내는 변환만 둔다. 도구의 `Payload`·`ErrorKind`·`AppError`, 도구 고유 kind와 번호는 도구가 소유한다.
3. **도구 타입은 trait으로 받는다**: lib은 도구 타입을 알지 않는다. 제네릭 + 역할별 작은 trait(`HumanText`, `ExitStatus`)으로 받는다.
4. **공개 API를 바꾸면 모든 소비 도구를 같은 PR에서 이관한다**: 반쯤 이관된 workspace를 남기지 않는다. 출력이 바뀌면 각 도구의 리터럴 계약 테스트가 잡는다 — 그 테스트를 lib 변경에 맞춰 고치지 않는다.
5. **공개 표면은 최소**: 실제로 두 도구 이상이 부르는 것만 `pub`. 내부 헬퍼는 private.

## DO

lib은 제네릭으로 도구 타입을 받고, 도구는 타입만 고정하는 얇은 wrapper를 둔다 (`tools/kanban/src/output/mod.rs` 실례).

```rust
// libs/cli-contract/src/render.rs
pub fn render<P, E>(command: &str, result: &Result<P, E>, json: bool) -> (Sink, String, i32)
where
    P: Serialize + HumanText,
    E: Serialize + HumanText + ExitStatus,
{ /* ... */ }

// tools/<tool>/src/output/mod.rs
pub fn render(command: &str, result: &Result<Payload, AppError>, json: bool) -> (Sink, String, i32) {
    cli_contract::render(command, result, json)
}
```

공유 kind는 도구의 평평한 `ErrorKind`가 `CommonKind`에 위임하고, 도구 고유 kind만 자기 번호를 쓴다.

```rust
pub fn exit_code(err: &AppError) -> i32 {
    match err.kind {
        ErrorKind::Usage => CommonKind::Usage.exit_code(),
        ErrorKind::Api => 4, // 도구 고유
        // ...
    }
}
```

## DON'T

도구 고유 kind나 타입을 lib에 올리지 않는다.

```rust
pub enum CommonKind {
    Usage,
    Auth, // discord만 쓴다 — discord의 ErrorKind에 있어야 한다
}
```

기존 exit code를 다시 매기지 않는다 — 에이전트가 번호로 분기한다.

```rust
CommonKind::Network => 4, // 6을 쓰던 세 도구의 계약이 깨진다
```

## 체크리스트

- [ ] 세 도구 이상이 실제로 쓰는 코드인가
- [ ] 도구 타입·도구 고유 kind가 lib에 없는가
- [ ] 공개 API 변경이 모든 소비 도구 이관과 같은 PR에 있는가
- [ ] 도구의 리터럴 계약 테스트를 고치지 않고 통과하는가
