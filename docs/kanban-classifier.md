# kanban — 분류기 · 골든셋 설계

로컬 LLM으로 유입 이슈를 프로젝트에 배정하고, 우선순위와 라벨을 붙이는 파이프라인.
사람의 후보정을 골든셋으로 축적해 few-shot으로 되먹여 분류 품질을 개선한다.

보드·상태 모델·명령 표면은 [kanban-board.md](./kanban-board.md)에 있다.

---

## 1. 역할

| 하는 것 | 하지 않는 것 |
|---|---|
| `inbox` 아이템을 프로젝트에 배정 | 이미 배정된 아이템 재배치 |
| 우선순위(`P0`~`P3`) 판정 | `running`·`done` 아이템 조작 |
| 라벨 부여 (`kind`/`area`/`duplicate-of`/`similar`) | 중복 이슈 자동 병합·종료 |
| 확신 없으면 `unmatched`로 넘기고 근거 기록 | 애매한 것을 억지로 배정 |

분류기는 **라벨과 배정까지만** 한다. 중복을 발견해도 `duplicate-of` 라벨을 붙일 뿐
보드를 움직이지 않는다. 오분류의 비용을 라벨 하나가 틀리는 수준으로 묶어둔다.

---

## 2. 추상화 경계

`Classifier` trait은 **소비자인 `engine.rs`가 소유한다**(DIP). 파이프라인은 trait에만
의존하고 HTTP 구현체는 주입받는다.

```rust
// classify/engine.rs
pub(crate) trait Classifier {
    fn classify(&self, req: &ClassifyRequest) -> Result<Verdict, AppError>;
}

pub(crate) struct ClassifyRequest {
    pub projects: Vec<ProjectRef>,   // 이름 + 설명
    pub examples: Vec<GoldenRef>,    // few-shot
    pub title: String,
    pub body: String,
}

pub(crate) struct Verdict {
    pub project: Option<String>,     // None = 판정 불가
    pub confidence: f64,             // 0.0 ~ 1.0
    pub priority: Priority,
    pub labels: Vec<Label>,
    pub reason: String,
}
```

이 경계 덕분에 **로컬 모델 없이 분류 경로 전체를 테스트할 수 있다.** 현재 이 머신에는
추론 런타임이 설치돼 있지 않으므로(ollama·llama-server·lms 부재, 11434·1234 포트 미개방)
필수 조건이다.

구현체는 `classify/llm.rs`의 OpenAI 호환 HTTP 클라이언트 하나다. 대부분의 로컬
런타임(Ollama·LM Studio·llama.cpp server)이 이 인터페이스를 노출하므로 런타임 선택을
뒤로 미룰 수 있다.

```json
// ~/.areum/kanban/config.json
{
  "llm": {
    "base_url": "http://127.0.0.1:11434/v1",
    "model": "qwen2.5:7b",
    "timeout_ms": 60000
  },
  "classify": {
    "threshold": 0.6,
    "few_shot_per_project": 2,
    "max_attempts": 3,
    "tick_interval_sec": 300
  }
}
```

---

## 3. 파이프라인

데몬 틱(기본 5분) 또는 `kanban reclassify <id>`가 같은 경로를 탄다.

```
1. inbox 아이템 조회
      │
2. 프롬프트 조립  ◄── 등록된 프로젝트 (이름 + 설명)
      │           ◄── 골든셋 few-shot (프로젝트당 최근 K개, 기본 2)
      │           ◄── 이슈 제목 + 본문
      │
3. Classifier 호출
      │
4. 응답 검증 ──────► 스펙 위반 → outcome='error', attempts += 1, inbox 유지
      │                          attempts >= 3 → unmatched 승격
      ▼
5. 반영
   confidence >= threshold  →  backlog + project + priority + labels
   confidence <  threshold  →  unmatched + candidates + reason
```

`reclassify`는 `attempts`를 0으로 초기화하고 1회 실행한다. 사람이 프로젝트 설명을
고치거나 새 프로젝트를 등록한 뒤 다시 시도하는 용도이므로, 이전 실패 횟수를 이어받으면
곧바로 다시 `unmatched`로 튕긴다.

---

## 4. 프롬프트 계약

```
[system]
너는 이슈 분류기다. 아래 등록된 프로젝트 중 하나로 배정하라.
어느 프로젝트인지 확신할 수 없으면 project 에 null 을 넣어라.
반드시 JSON 하나만 출력하라. 설명 문장을 덧붙이지 마라.

[projects]
- areum-lab-tools: 로컬 실행용 작은 Rust CLI 도구 모음. tools/<name> 단일 crate.
- belt: 자율 개발 컨베이어 벨트. GitHub 이슈를 수집해 LLM 에이전트로 처리하는 데몬.

[examples]   ← 사람이 확정한 정답. 모델이 틀렸던 것은 오답도 함께 보여준다.
- "gh CLI 호출이 rate limit 에 걸림"
    모델 판정: areum-lab-tools / P2   →   정답: belt / P1
- "make dist-tag 가 태그를 두 번 밀음"
    정답: areum-lab-tools / P2

[issue]
제목: {title}
본문: {body}
```

### 우선순위 판정 기준

우선순위는 프로젝트 배정보다 주관적이라 소형 로컬 모델이 자주 틀린다. 그래서 **보수적
기본값**을 명시한다.

| 값 | 기준 |
|---|---|
| `P0` | 현재 동작이 깨져 있고 사용자가 즉시 막힌다 |
| `P1` | 명확한 버그이거나 진행 중 작업을 막는다 |
| `P2` | 기본값 — 판단이 서지 않으면 여기로 |
| `P3` | 있으면 좋은 개선, 급하지 않다 |

### few-shot 개수를 제한하는 이유

로컬 모델은 컨텍스트가 작다. 골든셋을 전부 넣으면 정작 이슈 본문이 밀려난다. 프로젝트당
최근 K개(기본 2)로 자르고, 오답 쌍을 우선 채운다.

---

## 5. 응답 계약과 검증

```json
{
  "project": "belt",
  "confidence": 0.82,
  "priority": "P1",
  "labels": [
    {"key": "kind", "value": "bug"},
    {"key": "duplicate-of", "value": "itm-000012"}
  ],
  "reason": "gh CLI 레이트리밋은 belt 의 GitHub DataSource 영역"
}
```

검증 규칙:

- `project`는 `null`이거나 **등록된 프로젝트 이름과 정확히 일치**해야 한다
- `confidence`는 `0.0 ~ 1.0` 범위의 수
- `priority`는 `P0`~`P3` 중 하나
- `labels[].key`는 `kind` \| `area` \| `duplicate-of` \| `similar`

### 에러 처리 — Fail Fast

`.claude/rules/rust-coding.md`의 원칙 1을 그대로 적용한다. 방어적 폴백은 모델이 스펙을
벗어나고 있다는 사실을 숨겨서, 정작 프롬프트나 모델을 고쳐야 할 때 신호를 없앤다.

| 상황 | 처리 |
|---|---|
| JSON 파싱 실패 · 스키마 불일치 | `outcome='error'`, `attempts += 1`, `inbox` 유지 — **텍스트 파싱 폴백 없음** |
| 등록되지 않은 프로젝트 이름 반환 | 동일 — **가장 가까운 이름으로 매칭하지 않는다** |
| `priority` 값이 범위 밖 | 동일 — `P2`로 대체하지 않는다 |
| `attempts >= max_attempts` (기본 3) | `unmatched`로 승격 → 사람이 본다 |
| 엔드포인트 연결 실패 (`network`) | 틱 실패로 stderr 로그, **아이템 상태 불변**, 다음 틱 재시도 |
| 엔드포인트 오류 응답 (`api`) | 동일 |
| `confidence < threshold` | `unmatched` + `candidates` + `reason` 기록 (정상 경로) |

마지막 두 줄의 구분이 중요하다. **연결 실패는 아이템의 문제가 아니라 환경의 문제**이므로
`attempts`를 올리지 않는다. 모델이 꺼져 있는 동안 쌓인 아이템이 전부 `unmatched`로
밀려나면 안 된다.

---

## 6. 스키마

```sql
CREATE TABLE classification (      -- 아이템당 최신 1행. 이력이 아니다.
  item_id      TEXT PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  attempted_at TEXT NOT NULL,
  outcome      TEXT NOT NULL,      -- assigned | unmatched | error
  candidates   TEXT NOT NULL,      -- JSON [{"project":"belt","score":0.41}, ...]
  reason       TEXT,               -- 모델이 남긴 판단 이유
  attempts     INTEGER NOT NULL DEFAULT 0,
  CHECK (outcome IN ('assigned','unmatched','error'))
);

CREATE TABLE golden (              -- 사람이 확정한 정답. 독립 자산이다.
  item_id        TEXT PRIMARY KEY, -- 추적용. FK 아님 — 아이템이 지워져도 남는다
  title          TEXT NOT NULL,    -- 스냅샷
  body           TEXT NOT NULL,    -- 스냅샷
  project        TEXT NOT NULL,    -- 정답
  priority       TEXT NOT NULL,
  model_project  TEXT,             -- 모델은 뭐라 했나 (NULL = unmatched 였음)
  model_priority TEXT,
  corrected_at   TEXT NOT NULL,
  CHECK (priority IN ('P0','P1','P2','P3'))
);
```

### `classification`을 이력이 아니라 최신 1행으로 두는 이유

후보정에 필요한 것은 "지금 왜 `unmatched`인가"뿐이다. 과거 시도 기록은 아무도 보지
않는다.

### `golden`에 FK를 걸지 않고 본문을 복사하는 이유

골든셋은 아이템의 부가정보가 아니라 **독립된 학습·평가 자산**이다. 보드를 정리해도,
아이템을 지워도, 프로젝트를 없애도 골든은 남아야 값을 한다. FK를 걸면 아이템 삭제가
골든셋을 함께 지운다.

### `model_project`/`model_priority`를 함께 남기는 이유

**오답 쌍이 가장 유용한 few-shot 예시**다. "모델이 A라 했는데 정답은 B"가 "정답은 B"보다
훨씬 많은 것을 가르친다. `eval`에서 개선 여부를 측정할 때도 기준선이 된다.

---

## 7. 골든셋 수집 정책

**사람이 명시적으로 손댄 것만** 기록한다.

| 행동 | 골든 기록 | 이유 |
|---|---|---|
| `assign` — `unmatched` 구제 | 기록 | 모델이 못 푼 것을 사람이 풀었다 |
| `assign` — 오분류 정정 | 기록 | 모델이 틀렸다는 명시적 신호 |
| `priority` 후보정 | 기록 | 동일 |
| 모델 판정을 그대로 둠 | 기록 안 함 | 암묵적 승인은 신호가 약하다 |

마지막 줄이 핵심이다. 모델이 스스로 만든 답을 골든셋에 넣고 그것을 다시 few-shot으로
먹이면 자기 편향이 증폭된다. 골든셋은 모델 출력으로 오염되면 안 된다.

`assign`/`priority`는 `items` 갱신과 `golden` upsert를 **한 트랜잭션**에서 처리한다.
보드는 고쳐졌는데 골든이 안 남거나 그 반대인 상태를 만들지 않는다.

### 골든셋 관리

```sh
kanban golden list
kanban golden rm <id>          # 프로젝트 설명이 바뀌어 낡아버린 골든 제거
kanban golden export --json    # 외부 평가·파인튜닝용
```

프로젝트 설명을 크게 바꾸면 기존 골든과 모순될 수 있다. 골든은 참조일 뿐이고 프로젝트
설명이 우선이므로, 충돌하는 골든은 `golden rm`으로 걷어낸다.

---

## 8. 평가 (`eval`)

```sh
kanban eval
  → project  정확도  34/40 (85%)
    priority 정확도  22/40 (55%)

    틀린 항목:
      itm-000021  belt → areum-lab-tools     (priority P1 → P2)
      itm-000033  areum-lab-tools → belt
```

골든셋 전체를 현재 프롬프트·모델로 다시 분류해 정확도를 낸다. **골든셋을 쌓기만 하고
효과를 못 재면 쌓을 이유가 없다.** 프로젝트 설명을 고쳤을 때, 모델을 바꿨을 때, few-shot
개수를 조정했을 때 좋아졌는지 나빠졌는지 판단할 수단이 이것뿐이다.

> 평가 시에는 few-shot에서 **평가 대상 아이템 자신을 제외**한다. 자기 자신을 예시로
> 보여주면 정확도가 무의미하게 부풀려진다.

우선순위 정확도가 프로젝트 정확도보다 낮게 나오는 것이 정상이다. 주관적 판단이라
사람끼리도 갈린다.

---

## 9. 후보정 흐름

```
$ kanban list --state unmatched --json
  [{ "id": "itm-000021",
     "title": "캐시가 계속 어긋남",
     "candidates": [{"project":"belt","score":0.41},
                    {"project":"areum-lab-tools","score":0.38}],
     "reason": "어느 저장소를 가리키는지 본문에 단서가 없음" }]

$ kanban assign itm-000021 --project belt --priority P1
  → backlog 이동 + golden 기록 (model_project=null, model_priority=null)

# 또는 프로젝트를 새로 등록한 뒤
$ kanban project add cache-lib --desc "공용 캐시 라이브러리..."
$ kanban reclassify itm-000021
```

`candidates`와 `reason`이 남아 있어야 사람이 빠르게 판단할 수 있다. 이것이 분류 실패
시에도 근거를 기록하는 이유다.

---

## 10. 테스트

전부 인라인 `#[cfg(test)]`. 공유 목·픽스처는 `commands/testutil.rs`가 단독 소유한다.

| 대상 | 검증 |
|---|---|
| `classify/engine.rs` | `MockClassifier`로 전 경로: 배정 / threshold 미달 → `unmatched` / 스키마 위반 → `attempts++` / `attempts >= 3` → `unmatched` 승격 / 연결 실패 시 **상태·attempts 불변** |
| `classify/prompt.rs` | few-shot이 프로젝트당 K개로 잘리는가 · 오답 쌍이 우선 채워지는가 · **스펙 위반 응답에서 폴백 없이 실패하는가** · 미등록 프로젝트 이름 거부 |
| `commands/assign.rs` | `items` 갱신과 `golden` upsert가 한 트랜잭션인가 (한쪽 실패 시 둘 다 롤백) |
| `commands/reclassify.rs` | `attempts`가 0으로 초기화되는가 |
| `commands/eval.rs` | 고정 골든셋 + mock으로 정확도 계산이 맞는가 · **평가 대상이 자기 few-shot에서 제외되는가** |

실제 로컬 엔드포인트를 때리는 E2E는 `#[ignore]`로 분리한다. 런타임이 설치된 환경에서만
`cargo test -- --ignored`로 돌린다.

---

## 11. 미결정 사항

| 항목 | 상태 |
|---|---|
| 로컬 추론 런타임 선택 (Ollama / LM Studio / llama.cpp) | 미정 — OpenAI 호환 인터페이스라 3단계에서 결정 |
| 모델 선택 및 `threshold` 실측값 | 미정 — 골든셋이 쌓인 뒤 `eval`로 조정 |
| `duplicate-of` 판정에 임베딩 유사도를 쓸지 | 미정 — 우선 LLM 판정만으로 시작 |
| 틱 간격 기본값 | 5분 — 실사용 후 조정 |
