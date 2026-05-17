# Borderless transcript + status absorption + mascot relocation — plan

## 왜

현재 TUI는 transcript를 box border로 감싸고 오른쪽에 status side panel을 둠. 세 가지 UX 문제:

1. **복사가 지저분함** — 드래그 selection은 사각형이라 transcript 본문만 잡으려 해도 (a) 양옆 보더 문자 `│`와 (b) 같은 행의 status 패널 내용이 같이 잡힘.
2. **스트리밍 중 빨간 보더 깜빡임** — `TranscriptKind::Error`/`ToolFail` 라인이 보더 셀에 인접하면 ratatui 0.30이 SGR reset을 안 보내서 보더 문자(`┌─┐│└┘`)가 같이 빨개짐.
3. **마스코트가 사이드 패널 안에서만 산다** — 사이드 패널을 끄면 마스코트도 같이 사라짐. peridot 정체성 손실.

Claude Code / Codex CLI는 둘 다 transcript 영역을 보더 없이 full-width로 두고, status는 위 헤더 한 줄 + 아래 상태바 한 줄로 압축. 같은 방향으로 가자.

## 무엇

### 레이아웃 변경

| 패널 | 현재 | 변경 후 |
|---|---|---|
| Header (1줄) | `peridot · model · auto · …` | **확장**: session/steps/elapsed/subagents-count 추가 |
| Tab bar | (변경 없음) | (변경 없음) |
| **Transcript** | `Block::default().borders(ALL).title(...)` | **보더 없음**, full-width. title은 transcript 위쪽 dim 한 줄로 |
| **Status side panel** | 기본 ON, 오른쪽에 박스로 표시 | **기본 OFF**. `Ctrl+]`로 토글 (opt-in). 정보는 헤더에 흡수 |
| Input | `Block::default().borders(ALL).title("prompt")` | 유지 (입력창은 시각적 경계 필요) |
| Status bar (1줄) | `● ⠴ processing  | 2 queued` | **확장**: tokens/cost/cache 추가, `●` 점은 mood indicator로 |

### 마스코트 3분산 배치

| 위치 | 상황 | 크기 |
|---|---|---|
| Welcome 화면 | 첫 진입, transcript 비어있을 때 | 풀 스프라이트 (8×4 셀) |
| 상태바 mood indicator | 항상 (채팅 중) | 1셀 (`●` 자리를 mood-driven 문자로) |
| 사이드 패널 | `Ctrl+]` 켰을 때 | 풀 스프라이트 (기존 동작) |

### 마스코트 흰배경 버그 수정 — ✅ 완료

`peridot-tui/src/mascot/render.rs`: `Pixel::Empty`일 때도 `▀` 글리프 + `Color::Reset` fg를 쓰던 게 문제. 어두운 터미널에서 fg=Reset은 "터미널 기본 fg" = 밝은 회색/흰색이라, 빈 픽셀이 흰 반쪽 블록으로 칠해짐.

수정 후 동작:
- `(Empty, Empty)` → ` ` (space, fg/bg Reset) — 진짜 투명
- `(Empty, Index)` → `▄` (lower half block, fg=색)
- `(Index, Empty)` → `▀` (upper half block, fg=색)
- `(Index, Index)` → `▀` (기존 동작)

테스트 2개 추가:
- `render_paints_sprite_pixels_and_leaves_empty_cells_transparent` — 빈 셀이 실제로 공백인지
- `empty_cells_use_reset_colors_not_terminal_default_fg` — 회귀 방지

## Before / After mockup

### Before (현재)

```
peridot · gpt-4o · auto · session foo
─── main ─────────────────────────────────────────────────────
┌── chat · 5 turns ───────────────────────────────┬── Status ──┐
│                                                 │ ▄▄▄▄▄▄░░░░ │  ← 마스코트
│ > 안녕                                          │ █▀▄▄▄▄█░░░ │     주변 흰색
│                                                 │ █▀▄▄▄▄█░░░ │     (Color::Reset
│ 코드베이스를 분석하겠습니다.                     │  ▀▀▀▀▀▀░░░ │      → 터미널
│                                                 │            │      기본 fg)
│ ✔ file_read  read peridot-core/src/agent.rs     │ id: 1779…  │
│                                                 │ agent:run  │
│ ⚠ error: rate-limited                            │ steps: 12  │  ← ⚠ 라인이
│                                                 │ elapsed:8s │     빨강이면
│ ⠴ processing                                     │            │     양쪽 보더
│                                                 │ Subagents  │     같이 빨개짐
│                                                 │ <none>     │
└─────────────────────────────────────────────────┴────────────┘
┌── prompt ──────────────────────────────────────────────────┐
│ > _                                                        │
└────────────────────────────────────────────────────────────┘
● ⠴ processing  | 2 queued
```

### After (D + 마스코트 분산)

#### 진입 직후 — welcome 화면

```
peridot · gpt-4o · auto · ◔ idle
─── welcome ──────────────────────────────────────────────────────


           ▄▄▄▄▄▄
          █▀▄▄▄▄█             Welcome back dlsxj101!
          █▀▄▄▄▄█             Peridot is ready for an agent run.
           ▀▀▀▀▀▀
                              /help          show available commands
                              /goal start    run a long-form task
                              Ctrl+]         toggle status panel


┌── prompt ────────────────────────────────────────────────────────────────┐
│ > _                                                                      │
└──────────────────────────────────────────────────────────────────────────┘
◔ idle · ready
```

#### 채팅 중 — 사이드 패널 OFF (기본)

```
peridot · gpt-4o · auto · session 1779011508 · steps 12 · 8s · subagents 0
─── chat · 5 turns ─────────────────────────────────────────────────────────

  > 안녕 너의 코드베이스를 분석해줘

  코드베이스를 분석하겠습니다. 먼저 디렉토리 구조와 핵심 모듈을 살펴볼게요.

  ✔ file_read  read peridot-core/src/agent.rs
  ✔ file_list  listed peridot-tools/src/tools

  ⚠ error: rate-limited, retrying in 2s

  ⠴ processing...

┌── prompt ────────────────────────────────────────────────────────────────┐
│ > _                                                                      │
└──────────────────────────────────────────────────────────────────────────┘
◑ ⠴ processing · 2 queued · tokens 12.3k · cost $0.45 · cache 87%
```

- 드래그 selection 깨끗함 — transcript 옆에 잡것 없음
- `⚠ error` 라인 빨개져도 인접 셀은 빈 공백 → 빨강 SGR 누수 사라짐
- 헤더에 session id / steps / elapsed / subagents-count 압축
- 상태바 `◑` = thinking mood (전엔 `●`)

#### 채팅 중 — `Ctrl+]`로 사이드 패널 ON

```
peridot · gpt-4o · auto · session 1779011508 · steps 12 · 8s · subagents 0
─── chat · 5 turns ─────────────────────────────────────────┬─── Status ───
                                                            │
  > 안녕                                                    │   ▄▄▄▄▄▄
                                                            │   █▀▄▄▄▄█      ← 흰배경
  코드베이스를 분석하겠습니다.                              │   █▀▄▄▄▄█        없음
                                                            │    ▀▀▀▀▀▀
  ✔ file_read  read peridot-core/src/agent.rs              │
                                                            │   id: 1779…
  ⠴ processing...                                            │   agent: run
                                                            │   steps: 12
                                                            │   subagents 0
                                                            │
┌── prompt ─────────────────────────────────────────────────┴───────────────┐
│ > _                                                                       │
└───────────────────────────────────────────────────────────────────────────┘
◑ ⠴ processing · 2 queued
```

- 사이드 패널 보더는 왼쪽 `│` 하나만 (양쪽 보더 4개 → 1개로 감소)
- 마스코트 주변 흰색 사라짐 (✅ 이미 수정됨)

### 상태바 mood indicator

| 상태 | 글리프 | 색 |
|---|---|---|
| Idle | `◔` | 회색 |
| Thinking (streaming) | `◑` | 노랑 |
| ToolRunning | `◉` | 시안 |
| ApprovalWait | `◕` | 오렌지 |
| AskUser | `◔` | 보라 |
| Done | `◉` | 초록 |
| Failed | `◓` | 빨강 |
| Interrupted | `◔` | 마젠타 |

기존 `●` 한 셀을 mood별 글리프로 치환. 같은 좌표라 레이아웃 영향 없음.

## 구현 변경 범위

| 파일 | 변경 |
|---|---|
| `peridot-tui/src/mascot/render.rs` | ✅ **완료** — 빈 픽셀 투명화, 회귀 테스트 2개 추가 |
| `peridot-tui/src/render.rs:1098-1100` | `body_block` 보더 제거 (`borders(Borders::NONE)`), title을 dim Line으로 prepend |
| `peridot-tui/src/render.rs:1087-1097` | 사이드 패널 활성 조건은 그대로지만, **default 값**을 OFF로 (`TuiConfig::show_subagent_panel` 기본값 변경) |
| `peridot-tui/src/render.rs:1145-1146` | `inner_width` / `inner_height`의 `saturating_sub(2)`를 `saturating_sub(1)`로 (보더 없으니 padding만) |
| `peridot-tui/src/render.rs:1204` | `side_block` 보더를 `Borders::LEFT`로 (왼쪽 dim 구분자만) |
| `peridot-tui/src/render.rs` (header 라인) | session id / steps / elapsed / subagents count 추가 — `render_header_line` 확장 |
| `peridot-tui/src/render.rs:847-873` | `render_status_bar`에서 `●` 자리를 mood glyph로 치환 |
| `peridot-tui/src/render.rs:232-…` | `render_welcome`에 풀 마스코트 8×4 영역 추가 (현재 텍스트만 출력 중) |
| `peridot-common/src/lib.rs` | `TuiConfig::show_subagent_panel` 기본값 `true` → `false` |
| `peridot-tui/src/tests.rs` | 스냅샷 fixture 갱신 (보더 제거 / 헤더 확장 반영) |

대략 100~150줄 정도의 수정. overlay (`approval`, `branch_picker`, `ask_user`, `menu`)는 보더 유지.

## 위험 / 트레이드오프

- **사이드 패널 default OFF** — 기존 사용자에게 변화. 마이그레이션 노트로 "Ctrl+] to bring it back" 안내 필요.
- **헤더가 좁은 터미널에서 길어짐** — width < 100일 때 자동으로 우선순위 낮은 정보 (subagents count, elapsed) 생략하도록 fallback 추가.
- **테스트 fixture 다수 갱신** — `fixture_scenarios_render_through_ratatui_backend`가 ratatui buffer 기준 스냅샷이라 보더/패널 변경 시 의도된 diff 검토.
- **mood glyph가 일부 터미널에서 누락 가능** — `◔◑◉◕◓` 등이 unicode 범위. WSL conpty는 OK이지만 안전을 위해 fallback `●` 옵션 검토.

## 옵션 (재확인)

| 옵션 | 결과 |
|---|---|
| A. Transcript 보더만 제거 | 복사 시 status 패널 내용은 여전히 딸려 옴. 불완전. |
| B. Transcript 보더 제거 + 사이드 패널 보더 제거 | 복사 시 status 내용 여전히 딸려 옴. 불완전. |
| C. 보더 유지하고 `Color::Reset`만 명시 | 빨간 보더만 해결, 복사 문제는 그대로 |
| **D. + 마스코트 분산** ← 권장 | 복사 깨끗 + 빨간 보더 사라짐 + 마스코트 항상 mood 노출 |

## 다음 단계

1. ✅ 마스코트 흰배경 수정 (완료)
2. `TuiConfig::show_subagent_panel` 기본값 false
3. transcript 보더 제거 + title을 dim Line으로 prepend
4. side panel `Borders::LEFT`만
5. header 확장 (session/steps/elapsed/subagents count)
6. status bar `●` → mood glyph
7. welcome 화면에 풀 마스코트 추가
8. 테스트 fixture 갱신
9. `cargo build --release` + 직접 띄워 시각 검증
10. 커밋
