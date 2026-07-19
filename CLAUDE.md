# areum-lab-tools

## 파일 네이밍 컨벤션

- 도구 디렉토리: `tools/<name>/` — 소문자 단일 단어 (예: `hello`, `discord`)
- 크레이트 소스: `src/main.rs`에서 시작한다. 모듈 분리 시 `src/<역할>.rs` 스네이크케이스, 기능 디렉토리로 묶일 때는 `src/<그룹>/<항목>.rs` 스네이크케이스 (예: `commands/send.rs`, `common/config.rs`) — 구조 상세 원칙은 `.claude/rules/module-structure.md` 참조
- 스캐폴드 템플릿: 산출물과 동일 경로 + `.tmpl` 확장자, 플레이스홀더는 `{{NAME}}`
- Makefile 타깃: 짧은 동사, 복합 타깃은 하이픈 연결 (예: `fmt-check`, `dist-build`)
- 릴리스 태그: `<tool>-vX.Y.Z` — version은 태그의 마지막 `-v` 뒤에 온다
- 브랜치: `epic/<주제>` (에픽 단위), `feature/*`·`fix/*`·`docs/*` 등 (일반 작업)
- 문서: `docs/<kebab-case>.md`
- 워크플로: `.github/workflows/<목적>.yml` (예: `ci.yml`, `release.yml`)
- 툴링 설정: 표준 Rust 파일명 그대로 사용한다 (`rust-toolchain.toml`, `rustfmt.toml` — 변형 금지)
