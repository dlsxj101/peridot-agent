# Phase 1.1 인수인계 — Peridot Daemon Async + `session.start`

> 이 문서는 Phase 1.1a + 1.1b를 다른 환경(예: Codex)에서 이어서 작업할
> 때 그대로 읽고 시작할 수 있도록 만들어졌다. **작업이 끝나면 이 파일을
> repo에서 삭제할 것.**

---

## 컨텍스트

Peridot는 Rust 기반 자율 코딩 agent로 TUI / CLI / headless 모드를 지원한다.
v0.7.10까지 진행됐고, 별도 VS Code / Cursor 익스텐션
(`extensions/vscode/`, publisher `dlsxj101.peridot-vscode` v0.0.2)도 Phase 0
(scaffold + JSON-RPC 검증)이 끝난 상태다.

Phase 0 검증은 Cursor의 WSL2 Remote-WSL backend에서 **실 동작 확인 완료**:

- `Peridot: Hello` → "extension installed correctly" 토스트 ✓
- `Peridot: Check Daemon Version` → "Peridot daemon 0.7.10 (extension 0.0.2)" 토스트 ✓

이번 인수인계 작업은 **Phase 1.1a + 1.1b 결합**: `peridot daemon`
서브커맨드를 tokio async runtime으로 갈아엎고, 새 JSON-RPC 메서드
`session.start` / `session.cancel`을 추가해서 익스텐션이 실제 agent loop를
띄울 수 있게 만드는 것.

---

## 현재 상태

| 항목 | 값 |
|---|---|
| 마지막 commit | `e55da7e` (origin/main + origin/claude/review-codebase-features-P6AN0) |
| 작업 브랜치 | `claude/review-codebase-features-P6AN0` (push 시 main에도 같이) |
| Workspace 버전 | 0.7.10 (`Cargo.toml`) |
| 익스텐션 버전 | 0.0.2 (`extensions/vscode/package.json`) |

기존 `daemon.rs`는 sync 버전 (std::io::stdin 블로킹 loop, `peridot.version` /
`peridot.echo` / `shutdown` 세 메서드만 처리). 이번에 통째로 재작성한다.

> **참고**: 이전에 Phase 1.1a 작업 한 번 진행하다가 사용자 머신의 WDAC
> 차단 때문에 Phase 0 검증부터 끝내고 와야 해서 revert됐다. 그래서
> branch HEAD는 깨끗하다 — 새로 처음부터 작성한다.

---

## 이번 작업 목표 — Phase 1.1a + 1.1b 함께

### 1.1a — daemon async 변환 + 메서드 인터페이스

**`crates/peridot-cli/src/commands/daemon.rs`를 다음 구조로 재작성:**

1. **`pub(crate) async fn run_daemon_command(project_root: &Path, ...) -> Result<()>`**
   - workspace `Cargo.toml`의 tokio features에 **`"io-std"` 추가** 필요
     (`tokio::io::stdin`/`stdout`이 `io-std` feature gate임)
   - **writer task**: `tokio::io::stdout`로 `mpsc<String>` drain (단일
     writer라 동시 task들의 JSON-RPC frame 인터리브 방지)
   - **reader task**: `tokio::task::spawn_blocking` 안에서 std::io::stdin
     line iteration → `mpsc<io::Result<String>>`로 메인 task에 전달
     (Windows 콘솔 호환성 때문에 sync stdin이 안전)
   - **메인 loop**: mpsc recv → JSON-RPC dispatch → response/notification emit

2. **`DaemonState` struct** (Clone)
   ```rust
   #[derive(Clone)]
   struct DaemonState {
       sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
       next_session_id: Arc<Mutex<u64>>,
       project_root: Arc<PathBuf>,
       out: mpsc::UnboundedSender<String>,
       // agent 부트스트랩 정적 인자 — daemon 시작 시 한 번 빌드
       run_config: Arc<PeridotConfig>,
       run_template: Arc<AgentTaskOptions>,
   }
   ```
   매 `session.start`마다 다시 빌드하지 않도록 정적 인자는 Arc로 보관.

3. **`SessionEntry` struct**
   ```rust
   struct SessionEntry {
       cancel: CancelToken,            // peridot_common::CancelToken
       _task: tokio::task::JoinHandle<()>,
   }
   ```

4. **JSON-RPC 메서드** (기존 + 새것)

   | Method | 동작 |
   |---|---|
   | `peridot.version` | `{ "version": CARGO_PKG_VERSION }` 반환 |
   | `peridot.echo` | `{ "echo": params.text }` 반환 |
   | `session.start` | 아래 1.1b 참고 |
   | `session.cancel` | `params.session_id`의 CancelToken.cancel() 호출, `{ "cancelled": bool, "session_id": string }` 반환 |
   | `shutdown` | 루프 종료. `id` 있으면 `{ "shutdown": true }` ack, 없으면 무응답 |

   에러 코드:
   - 알 수 없는 메서드 → -32601 method not found
   - `jsonrpc != "2.0"` → -32600 invalid request
   - parse 실패 → -32700 parse error
   - 잘못된 params → -32602 invalid params

5. **Wire format** (매 메시지는 `\n` 종료 단일 라인 JSON):

   ```text
   C→S {"jsonrpc":"2.0","id":1,"method":"session.start","params":{"task":"fix the bug"}}
   S→C {"jsonrpc":"2.0","id":1,"result":{"session_id":"session-<pid>-<n>"}}
   S→C {"jsonrpc":"2.0","method":"event","params":{"session_id":"...","event":{"kind":"started",...}}}
   ...
   S→C {"jsonrpc":"2.0","method":"event","params":{"session_id":"...","event":{"kind":"finished",...}}}

   C→S {"jsonrpc":"2.0","id":2,"method":"session.cancel","params":{"session_id":"..."}}
   S→C {"jsonrpc":"2.0","id":2,"result":{"cancelled":true,"session_id":"..."}}
   ```

### 1.1b — `session.start`에서 실제 agent loop 호출

`peridot-cli/src/run_loop.rs`의 진입점:

```rust
pub(super) async fn run_task_with_events<F>(
    task: String,
    mode: ExecutionMode,
    options: AgentTaskOptions,
    config: PeridotConfig,
    project_root: PathBuf,
    cancel: Option<peridot_core::CancelToken>,
    compact_request: Option<Arc<AtomicBool>>,
    context_snapshot_path: Option<PathBuf>,
    ask_user_port: Option<Arc<dyn peridot_tools::AskUserPort>>,
    message_bus: MessageBusHookup,
    events: F,
) -> Result<AgentRunSummary>
where F: FnMut(AgentRunEvent)
```

이걸 daemon이 호출하도록 한다.

**`session.start` 핸들러 의사 코드:**

```rust
async fn handle_session_start(state: &DaemonState, id: Value, params: Option<Value>) {
    let task: String = params.as_ref()
        .and_then(|v| v.get("task")).and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| /* -32602 emit */ return)?;
    let mode: ExecutionMode = params.as_ref()
        .and_then(|v| v.get("mode")).and_then(Value::as_str)
        .map(parse_execution_mode)
        .unwrap_or(/* daemon default */);
    let permission: PermissionMode = params.as_ref()
        .and_then(|v| v.get("permission")).and_then(Value::as_str)
        .map(parse_permission_mode)
        .unwrap_or(/* daemon default */);
    let model: Option<String> = params.as_ref()
        .and_then(|v| v.get("model")).and_then(Value::as_str)
        .map(String::from);

    let session_id = state.next_id().await;
    let cancel = CancelToken::new();
    let cancel_for_task = cancel.clone();
    let state_for_task = state.clone();
    let session_id_for_task = session_id.clone();

    let handle = tokio::spawn(async move {
        // 1. 시작 알림
        emit_event(&state_for_task, &session_id_for_task, json!({
            "kind": "started", "task": task,
        }));

        // 2. options 빌드 (model 오버라이드 등 적용)
        let mut options = (*state_for_task.run_template).clone();
        if let Some(m) = model { options.model = m; }
        options.permission = permission;
        // ... 다른 override 필요한 만큼

        // 3. run_task_with_events 호출. events 콜백에서 매 AgentRunEvent를
        //    JSON-RPC notification으로 forward.
        let session_id_inner = session_id_for_task.clone();
        let state_inner = state_for_task.clone();
        let result = run_task_with_events(
            task,
            mode,
            options,
            (*state_for_task.run_config).clone(),
            state_for_task.project_root.as_ref().clone(),
            Some(cancel_for_task),
            /* compact_request */ None,
            /* context_snapshot_path */ None,
            /* ask_user_port */ None,        // 1.2에서 daemon 자체 AskUserPort
            /* message_bus */ MessageBusHookup::None,
            move |event: AgentRunEvent| {
                let value = serde_json::to_value(&event).unwrap_or(Value::Null);
                emit_event(&state_inner, &session_id_inner, value);
            },
        ).await;

        // 4. 결과 알림
        match result {
            Ok(summary) => emit_event(&state_for_task, &session_id_for_task, json!({
                "kind": "finished",
                "stopped_reason": format!("{:?}", summary.stopped_reason),
                "turns": summary.turns.len(),
            })),
            Err(err) => emit_event(&state_for_task, &session_id_for_task, json!({
                "kind": "error", "message": err.to_string(),
            })),
        }

        // 5. registry에서 자기 자신 제거 (메모리 누수 방지)
        state_for_task.sessions.lock().await.remove(&session_id_for_task);
    });

    state.sessions.lock().await.insert(session_id.clone(), SessionEntry {
        cancel,
        _task: handle,
    });

    emit_response(&state, id, json!({ "session_id": session_id }));
}
```

#### ⚠️ 핵심 주의사항

- **`AgentRunEvent`는 이미 `Serialize` 구현됨** (peridot-core 라이브러리).
  `serde_json::to_value(&event)` 한 줄로 JSON Value 변환 가능. 직접 확인 권장.
- `run_task_with_events`에 필요한 정적 인자들 (`AgentTaskOptions`,
  `PeridotConfig`, `MessageBusHookup` 등)을 **daemon 시작 시 미리** 만들어서
  `DaemonState`에 보관. `main.rs`의 `Some(Command::Daemon)` 핸들러에서
  빌드해서 `run_daemon_command`에 넘겨주기.
- `register_configured_mcp_tools` 등은 `run_task_with_events` 내부에서
  알아서 처리한다.
- `ask_user_port`는 1.2에서 daemon 자체의 AskUserPort 구현으로 채울 예정.
  지금은 `None` — agent가 ask_user 호출하면 default 처리 (`skipped`).
- agent task가 panic하면 stdout으로 panic 메시지가 새어 나가서 JSON-RPC
  frame이 깨질 수 있다. 가능하면 `tokio::spawn`된 task body를
  `std::panic::catch_unwind` 또는 비슷한 가드로 감싸기. 필수는 아니지만
  견고함 +1.

### `main.rs` 변경

```rust
Some(Command::Daemon) => {
    // 정적 인자들 빌드 (config / options template 등)
    let template = AgentTaskOptions::default_for_daemon(/* ... */);
    commands::run_daemon_command(&project_root, &config, template).await?;
    return Ok(());
}
```

`run_daemon_command`는 이제 async. `main` fn은 이미 `#[tokio::main]`이라
`await` OK.

---

## 테스트 요구사항

`crates/peridot-cli/src/commands/daemon.rs`의 `#[cfg(test)] mod tests`에:

- `#[tokio::test]` 매크로 사용
- 헬퍼: `dispatch_and_collect(line: &str) -> Vec<Value>`
  - `DaemonState` 만들고 `dispatch` 호출
  - 짧은 sleep (50ms 정도) 후 mpsc receiver drain
  - 받은 line들을 `serde_json::Value`로 파싱해 반환

**필수 케이스**:

| # | 케이스 | 기대 |
|---|---|---|
| 1 | `peridot.version` | `result.version == CARGO_PKG_VERSION` |
| 2 | `peridot.echo` with text | `result.echo == "..."` |
| 3 | `peridot.echo` with non-object params | `error.code == -32602` |
| 4 | unknown method | `error.code == -32601` |
| 5 | `session.start` without `task` | `error.code == -32602` |
| 6 | `session.start` with task | response에 `session_id` 포함 + 매칭되는 `started` 이벤트 emit |
| 7 | `session.cancel` unknown id | `result.cancelled == false` |
| 8 | `shutdown` with id | `result.shutdown == true` |

> agent 실제 호출 테스트는 mock provider 없이는 어렵다. **1.1b 실 통합
> 검증은 빌드 + clippy + 기존 e2e** (`crates/peridot-cli/tests/e2e.rs`)의
> `daemon_responds_to_version_echo_and_shutdown`만 통과시키면 OK.
> 새 e2e 케이스 추가는 선택 (mock 환경에서 session.start이 실패하지 않고
> finished 이벤트를 emit하는 정도까지).

---

## 빌드 검증 절차

작업 끝나면 순서대로:

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all && cargo fmt --all --check
cargo test -p peridot-cli daemon
cargo test --workspace --exclude peridot-cli --exclude peridot-agents
```

마지막 두 줄은 daemon 로컬 테스트 + 나머지 워크스페이스.

> **알려진 환경 의존 실패** — sandbox에서 git commit signing이 안 될 때
> 아래 테스트들이 실패한다. 환경 문제니까 무시:
> - `peridot-cli::tests::auto_commit_run_commits_dirty_worktree`
> - `peridot-agents::tests::local_runner_creates_worktree_for_task`
> - `peridot-agents::tests::creates_branch_and_commit_in_temp_repo` (있다면)

---

## 버전 / 문서 업데이트

1. `Cargo.toml`: `version = "0.7.10"` → `"0.8.0"`
   (minor bump — `session.start`은 새 public surface)

2. `CHANGELOG.md` 상단에 `## [0.8.0] — 2026-05-20` 섹션 추가:
   - Added — `peridot daemon` async runtime (tokio)
   - Added — `session.start` / `session.cancel` JSON-RPC 메서드
   - 1.1a + 1.1b 변경 사항 자세히 (이전 `## [0.7.x]` 섹션들 톤 참고)

3. `README.md`의 "Status" 섹션 — Current version 갱신, "What's new in v0.8.0"
   섹션 추가.

---

## Commit / Push

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(daemon): tokio async runtime + session.start/cancel, v0.8.0

[자세한 커밋 메시지 — 이전 v0.7.x 커밋 톤 참고]
EOF
)"
git push -u origin claude/review-codebase-features-P6AN0
git push origin claude/review-codebase-features-P6AN0:main
```

작업 브랜치 + main 둘 다 fast-forward update.

---

## 사용자 환경 메모

- Windows 11 회사 PC + WDAC enforce (`CodeIntegrityPolicyEnforcementStatus = 2`)
- Phase 0 검증은 Cursor + WSL2 Ubuntu Remote-WSL backend로 수행
- 익스텐션 settings의 `peridot.binaryPath`에 `/home/yhchoi/.local/bin/peridot`
  절대경로를 박아 둠
- 한국어 사용자. 채팅은 한국어, **코드 주석은 영어**로 유지.

---

## 작업 안 할 것 / 다음 phase로 보류

| Phase | 내용 |
|---|---|
| 1.2 | `approval.respond` / `ask_user.respond` 메서드 + daemon 측 AskUserPort 구현 |
| 1.3 | VS Code 사이드바 view container + WebView chat panel |
| 1.4 | AgentRunEvent → WebView 렌더 (ToolStarted / AssistantDelta / FileDiff / ApprovalRequested) |
| 1.5 | ChatGPT OAuth login command (`peridot login openai-oauth` spawn + 브라우저) |

이번 작업은 **오로지 1.1a + 1.1b** — daemon async + `session.start` +
`session.cancel` + 실 agent loop 연결 + AgentRunEvent forward까지만.

---

## 핸드오프 끝

작업 완료 후:
1. 이 파일 (`HANDOFF.md`) 삭제하고 그것도 같은 commit에 포함
2. main + 작업 브랜치 둘 다 push
3. 사용자에게 v0.8.0 push 알림
