use std::path::Path;
use std::process::{Child, Command, Output};
use std::{io, thread};

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, SandboxMode, ToolGroup, ToolResult};
use serde_json::Value;

use crate::path::required_str;
use crate::{Tool, ToolContext};

/// Built-in shell execution tool.
#[derive(Clone, Debug)]
pub struct ShellExecTool;

/// Built-in guarded read-only shell execution tool.
#[derive(Clone, Debug)]
pub struct ShellReadOnlyTool;

#[async_trait]
impl Tool for ShellExecTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Shell
    }

    fn description(&self) -> &str {
        "Execute a shell command from the project root after deterministic safety checks"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command line executed from the project root"
                }
            },
            "required": ["command"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let command = required_str(&params, "command")?;
        reject_hard_blocked_command(command)?;
        enforce_shell_approval_policy(command, ctx)?;
        let prepared = shell_command(command, ctx)?;
        // Dry-run: surface the resolved invocation without spawning.
        // Useful for safety drills and CI smokes — the operator can
        // confirm the docker/firejail wrapping is applied as expected.
        if ctx.security.shell_dry_run {
            let description = describe_shell_command(&prepared);
            return Ok(ToolResult::success(
                format!("dry-run: {command}"),
                serde_json::json!({
                    "dry_run": true,
                    "would_execute": description,
                    "command": command,
                    "workspace_mutated": false,
                    "mutation_basis": "dry_run",
                }),
            ));
        }
        let before_fingerprint = git_worktree_fingerprint(&ctx.project_root);
        let ctx_for_block = ctx.clone();
        let label = command.to_string();
        let output = tokio::task::spawn_blocking(move || {
            spawn_and_wait_interruptible(prepared, &ctx_for_block, &label)
        })
        .await
        .map_err(|err| PeriError::Tool(format!("shell worker failed: {err}")))??;
        let after_fingerprint = git_worktree_fingerprint(&ctx.project_root);
        let mutation = workspace_mutation_snapshot(before_fingerprint, after_fingerprint);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summary = if output.status.success() {
            format!("command exited 0: {command}")
        } else {
            format!(
                "command exited {}: {command}",
                output.status.code().unwrap_or(-1)
            )
        };
        Ok(ToolResult::success(
            summary,
            serde_json::json!({
                "status": output.status.code(),
                "stdout": stdout,
                "stderr": stderr,
                "workspace_mutated": mutation.mutated,
                "mutation_basis": mutation.basis,
                "git_status_before": mutation.before,
                "git_status_after": mutation.after,
            }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Write
    }

    fn risk_class(&self) -> peridot_common::RiskClass {
        // Shell can do anything — destroy files, push to remotes,
        // exfiltrate secrets. Permission-level is `Write` for the
        // allowlist machinery but the UI / class-based approval policy
        // needs to treat it as the most dangerous class so prompts
        // surface clearly.
        peridot_common::RiskClass::Destructive
    }

    fn can_run_concurrent(&self) -> bool {
        false
    }
}

#[async_trait]
impl Tool for ShellReadOnlyTool {
    fn name(&self) -> &str {
        "shell_readonly"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Shell
    }

    fn description(&self) -> &str {
        "Execute a tightly restricted read-only shell command from the project root. Prefer ripgrep_search, git_status, and git_diff when possible."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Read-only shell command. Allows search/list/print/git inspection commands; rejects redirects, command separators, installs, and known mutations."
                }
            },
            "required": ["command"],
            "additionalProperties": false,
        })
    }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> PeriResult<ToolResult> {
        let command = required_str(&params, "command")?;
        reject_hard_blocked_command(command)?;
        enforce_readonly_shell_policy(command)?;
        let prepared = shell_command(command, ctx)?;
        if ctx.security.shell_dry_run {
            return Ok(ToolResult::success(
                format!("dry-run readonly: {command}"),
                serde_json::json!({
                    "dry_run": true,
                    "would_execute": describe_shell_command(&prepared),
                    "command": command,
                    "workspace_mutated": false,
                    "mutation_basis": "dry_run",
                }),
            ));
        }
        let before_fingerprint = git_worktree_fingerprint(&ctx.project_root);
        let ctx_for_block = ctx.clone();
        let label = command.to_string();
        let output = tokio::task::spawn_blocking(move || {
            spawn_and_wait_interruptible(prepared, &ctx_for_block, &label)
        })
        .await
        .map_err(|err| PeriError::Tool(format!("readonly shell worker failed: {err}")))??;
        let after_fingerprint = git_worktree_fingerprint(&ctx.project_root);
        let mutation = workspace_mutation_snapshot(before_fingerprint, after_fingerprint);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let summary = if mutation.mutated {
            format!("read-only command mutated workspace: {command}")
        } else if output.status.success() {
            format!("read-only command exited 0: {command}")
        } else {
            format!(
                "read-only command exited {}: {command}",
                output.status.code().unwrap_or(-1)
            )
        };
        Ok(ToolResult {
            success: output.status.success() && !mutation.mutated,
            summary,
            output: serde_json::json!({
                "status": output.status.code(),
                "stdout": stdout,
                "stderr": stderr,
                "workspace_mutated": mutation.mutated,
                "mutation_basis": mutation.basis,
                "git_status_before": mutation.before,
                "git_status_after": mutation.after,
            }),
        })
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }
}

/// Spawns the prepared shell command as a child and waits for it to exit
/// while polling the agent loop's `cancel` token. When the operator hits
/// Esc the loop receives a fresh cancellation and we send the child a
/// kill signal, then return an `interrupted` error rather than blocking
/// forever inside `Command::output()`. Without a cancel token attached
/// to the context we fall back to a simple blocking `output()` so the
/// behaviour is unchanged outside the live TUI.
pub(crate) fn spawn_and_wait_interruptible(
    mut command: std::process::Command,
    ctx: &ToolContext,
    label: &str,
) -> PeriResult<std::process::Output> {
    let cancel = ctx.cancel.clone();
    let timeout_seconds = ctx.security.shell_command_timeout_seconds;
    // Child stdin = /dev/null on every path. Otherwise the child
    // inherits the TUI's tty stdin, racing the operator for
    // keystrokes; on Unix it also lets the child reach the
    // controlling terminal directly (npm / vite / spinner libs send
    // escape sequences that reset keypad mode), which corrupts the
    // TUI textarea after the command exits — arrow keys then
    // arrive as raw `[A` / `[B` / `[5~` instead of typed events.
    command.stdin(std::process::Stdio::null());
    // Fast path: no cancel token attached and no timeout configured →
    // keep the legacy blocking output() behaviour so non-TUI callers
    // (tests, headless smokes) see the same shape as before.
    if cancel.is_none() && timeout_seconds == 0 {
        return command
            .output()
            .map_err(|err| PeriError::Tool(format!("failed to run command: {err}")));
    }
    configure_interruptible_process_group(&mut command);
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| PeriError::Tool(format!("failed to spawn command: {err}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PeriError::Tool("failed to capture command stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PeriError::Tool("failed to capture command stderr".to_string()))?;
    let stdout_reader = read_pipe_in_background(stdout);
    let stderr_reader = read_pipe_in_background(stderr);
    let started = std::time::Instant::now();
    let deadline = if timeout_seconds == 0 {
        None
    } else {
        Some(started + std::time::Duration::from_secs(timeout_seconds))
    };
    let status;
    loop {
        match child
            .try_wait()
            .map_err(|err| PeriError::Tool(format!("failed to poll command: {err}")))?
        {
            Some(_status) => {
                status = _status;
                break;
            }
            None => {
                if let Some(token) = cancel.as_ref()
                    && token.is_cancelled()
                {
                    // Best-effort kill; ignore the error so we never
                    // double-report a failure that the cancellation
                    // already explains.
                    terminate_child_tree(&mut child);
                    let _ = child.wait();
                    let _ = collect_pipe_output("stdout", stdout_reader);
                    let _ = collect_pipe_output("stderr", stderr_reader);
                    return Err(PeriError::Tool(format!(
                        "{label}: interrupted by user before completion"
                    )));
                }
                if let Some(due) = deadline
                    && std::time::Instant::now() >= due
                {
                    // Same kill+wait dance, but report the deadline so the
                    // model gets a recoverable error instead of a generic
                    // "interrupted". Goal-mode loops use this to detect
                    // runaway commands without operator intervention.
                    terminate_child_tree(&mut child);
                    let _ = child.wait();
                    let _ = collect_pipe_output("stdout", stdout_reader);
                    let _ = collect_pipe_output("stderr", stderr_reader);
                    return Err(PeriError::Tool(format!(
                        "{label}: timed out after {timeout_seconds}s (security.shell_command_timeout_seconds)"
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    let stdout = collect_pipe_output("stdout", stdout_reader)?;
    let stderr = collect_pipe_output("stderr", stderr_reader)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn configure_interruptible_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_interruptible_process_group(_command: &mut Command) {}

fn terminate_child_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-KILL")
            .arg("--")
            .arg(format!("-{}", child.id()))
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status();
    }
    let _ = child.kill();
}

/// Per-pipe ceiling on captured bytes. A command that emits unbounded
/// output (`yes`, `cat /dev/zero`) would otherwise OOM the agent via an
/// unbounded `read_to_end`. Matches the web tools' `MAX_FETCH_BYTES`.
const MAX_PIPE_CAPTURE_BYTES: usize = 5 * 1024 * 1024;

fn read_pipe_in_background<R>(mut reader: R) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    R: io::Read + Send + 'static,
{
    thread::spawn(move || {
        // Read up to the cap, then keep draining to EOF (discarding the
        // overflow) so the child isn't blocked on a full pipe — we just
        // stop appending. A truncation marker is added so the model knows
        // the captured output is incomplete.
        let mut output = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        let mut truncated = false;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if output.len() < MAX_PIPE_CAPTURE_BYTES {
                let take = n.min(MAX_PIPE_CAPTURE_BYTES - output.len());
                output.extend_from_slice(&buf[..take]);
                if take < n {
                    truncated = true;
                }
            } else {
                truncated = true;
            }
        }
        if truncated {
            output.extend_from_slice(
                format!("\n\n[output truncated at {MAX_PIPE_CAPTURE_BYTES} bytes]").as_bytes(),
            );
        }
        Ok(output)
    })
}

fn collect_pipe_output(
    name: &str,
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> PeriResult<Vec<u8>> {
    reader
        .join()
        .map_err(|_| PeriError::Tool(format!("shell {name} reader panicked")))?
        .map_err(|err| PeriError::Tool(format!("failed to read command {name}: {err}")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MutationSnapshot {
    mutated: bool,
    basis: &'static str,
    before: Option<String>,
    after: Option<String>,
}

fn git_worktree_fingerprint(project_root: &Path) -> Option<String> {
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !inside.status.success() || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
        return None;
    }
    let output = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn workspace_mutation_snapshot(before: Option<String>, after: Option<String>) -> MutationSnapshot {
    let mutated = matches!((&before, &after), (Some(left), Some(right)) if left != right);
    let basis = match (&before, &after) {
        (Some(_), Some(_)) => "git_status",
        _ => "git_status_unavailable",
    };
    MutationSnapshot {
        mutated,
        basis,
        before,
        after,
    }
}

pub(crate) fn reject_hard_blocked_command(command: &str) -> PeriResult<()> {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");

    if pipes_remote_download_into_shell(&normalized) {
        return Err(PeriError::PermissionDenied(
            "piping remote download output into a shell is blocked".to_string(),
        ));
    }
    if has_recursive_force_root_remove(&normalized)
        || normalized.contains("mkfs.")
        || normalized.contains("dd if=/dev/zero")
        || is_fork_bomb(&normalized)
        || normalized.contains("chmod -R 777 /")
    {
        return Err(PeriError::PermissionDenied(format!(
            "hard-blocked shell command pattern: {command}"
        )));
    }
    Ok(())
}

/// Detects `curl|wget|fetch ... | <shell>` (and command/process substitution
/// like `sh -c "$(curl …)"` / `bash <(curl …)`), independent of spacing or
/// which shell receives the download — the previous check only caught the two
/// exact `curl | sh` / `wget | bash` spellings.
fn pipes_remote_download_into_shell(normalized: &str) -> bool {
    const DOWNLOADERS: [&str; 3] = ["curl", "wget", "fetch"];
    const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "ash"];
    let has_downloader = DOWNLOADERS.iter().any(|d| normalized.contains(d));
    if !has_downloader {
        return false;
    }
    // Command/process substitution feeding a shell, e.g. sh -c "$(curl …)".
    for sub in ["$(curl", "$(wget", "$(fetch", "<(curl", "<(wget", "<(fetch"] {
        if normalized.contains(sub) {
            return true;
        }
    }
    // A pipe whose downstream segment invokes a shell interpreter. Compare the
    // first token of each post-pipe segment so spacing (`x|sh` vs `x | sh`) and
    // the shell choice don't matter.
    let mut segments = normalized.split('|');
    let _ = segments.next(); // the producing segment (the downloader side)
    for segment in segments {
        if let Some(program) = segment.split_whitespace().next() {
            let basename = program.rsplit('/').next().unwrap_or(program);
            if SHELLS.contains(&basename) {
                return true;
            }
        }
    }
    false
}

/// Whitespace-insensitive classic fork-bomb detector (`:(){ :|:& };:` and its
/// reflowed variants).
fn is_fork_bomb(normalized: &str) -> bool {
    let stripped: String = normalized.chars().filter(|c| !c.is_whitespace()).collect();
    stripped.contains(":(){:|:&};:")
}

fn enforce_readonly_shell_policy(command: &str) -> PeriResult<()> {
    let normalized = normalize_shell_command(command);
    if is_install_command(&normalized) || is_destructive_shell_command(&normalized) {
        return Err(PeriError::PermissionDenied(
            "read-only shell rejects install or destructive commands".to_string(),
        ));
    }
    if contains_shell_write_syntax(&normalized) {
        return Err(PeriError::PermissionDenied(
            "read-only shell rejects redirects, backgrounding, and command separators".to_string(),
        ));
    }
    for segment in normalized.split('|') {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return Err(PeriError::PermissionDenied(
                "read-only shell rejects empty pipeline segments".to_string(),
            ));
        }
        if !is_allowed_readonly_segment(trimmed) {
            return Err(PeriError::PermissionDenied(format!(
                "read-only shell command is not on the inspection allowlist: {trimmed}. \
                 Use a dedicated read-only tool or an allowlisted inspection command; \
                 if this shell form is required, retry with shell_exec so the normal \
                 permission approval flow applies."
            )));
        }
    }
    Ok(())
}

fn contains_shell_write_syntax(command: &str) -> bool {
    command.contains(">>")
        || command.contains('>')
        || command.contains('<')
        || command.contains("&&")
        || command.contains("||")
        || command.contains(';')
        || command.contains('`')
        || command.contains("$(")
        || command.ends_with('&')
}

fn is_allowed_readonly_segment(segment: &str) -> bool {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let Some(program) = tokens.first().map(|token| clean_shell_token(token)) else {
        return false;
    };
    let basename = program.rsplit('/').next().unwrap_or(program);
    if basename == "git" {
        let Some(subcommand) = tokens.get(1).map(|token| clean_shell_token(token)) else {
            return false;
        };
        return matches!(
            subcommand,
            "status"
                | "diff"
                | "log"
                | "show"
                | "grep"
                | "branch"
                | "ls-files"
                | "rev-parse"
                | "describe"
        );
    }
    // `find` is allowlisted for traversal/printing only — its action
    // primitives let it execute or delete arbitrary files, which is not
    // read-only. Reject any command-execution / mutation actions.
    if basename == "find" {
        const FIND_EXEC_ACTIONS: [&str; 7] = [
            "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprintf", "-fprint",
        ];
        if tokens
            .iter()
            .any(|token| FIND_EXEC_ACTIONS.contains(&clean_shell_token(token)))
        {
            return false;
        }
    }
    // `sed` can execute shell commands (`e`), write files (`w`/`W`), and
    // read files (`r`), in addition to in-place editing (`-i`). None of
    // those are read-only, so reject the dangerous script commands and the
    // in-place flags. Plain substitution / print scripts still pass.
    if basename == "sed" {
        if tokens.iter().any(|token| {
            let cleaned = clean_shell_token(token);
            cleaned == "-i"
                || cleaned == "--in-place"
                || cleaned.starts_with("-i")
                || cleaned.starts_with("--in-place=")
        }) {
            return false;
        }
        if tokens
            .iter()
            .any(|token| sed_script_has_dangerous_command(clean_shell_token(token)))
        {
            return false;
        }
    }
    // `awk` (and gawk/mawk) can shell out via `system(...)`, read commands
    // through a pipe, and write to files via output redirection inside the
    // program text. There's no safe-subset parse here, so drop them from
    // the allowlist entirely — the model can use `rg`/`grep`/`cut` instead,
    // or fall back to `shell_exec` for genuine awk needs.
    if matches!(basename, "awk" | "gawk" | "mawk" | "nawk") {
        return false;
    }
    matches!(
        basename,
        "rg" | "grep"
            | "find"
            | "ls"
            | "pwd"
            | "cat"
            | "nl"
            | "head"
            | "tail"
            | "sed"
            | "wc"
            | "sort"
            | "uniq"
            | "cut"
    )
}

/// Detects sed script tokens that step outside read-only behaviour: the
/// `e` (execute), `w`/`W` (write file), and `r`/`R` (read file) commands.
/// Conservative scan over the script text — flags the command letters when
/// they appear as standalone sed commands (optionally after an address /
/// separator). Plain `s/a/b/`, `p`, `d`, `-n`, etc. are unaffected.
fn sed_script_has_dangerous_command(script: &str) -> bool {
    // Skip option flags — those are handled separately.
    if script.starts_with('-') {
        return false;
    }
    let bytes = script.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if matches!(c, 'e' | 'w' | 'W' | 'r' | 'R') {
            // Only treat it as a command when it begins a command slot:
            // start of string or immediately after a command separator
            // (`;`, newline, `{`). This avoids flagging the letter when it
            // appears inside a substitution pattern/replacement.
            let prev = if i == 0 {
                None
            } else {
                Some(bytes[i - 1] as char)
            };
            if matches!(prev, None | Some(';') | Some('\n') | Some('{') | Some(' ')) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn has_recursive_force_root_remove(command: &str) -> bool {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        if !is_rm_command_token(token) {
            continue;
        }
        let mut recursive = false;
        let mut force = false;
        for next in tokens.iter().skip(index + 1) {
            let cleaned = clean_shell_token(next);
            if is_shell_command_separator(cleaned) {
                break;
            }
            update_rm_flags(cleaned, &mut recursive, &mut force);
            if recursive && force && is_root_target(cleaned) {
                return true;
            }
        }
    }
    false
}

fn is_rm_command_token(token: &str) -> bool {
    let cleaned = clean_shell_token(token);
    cleaned == "rm" || cleaned.ends_with("/rm")
}

fn is_shell_command_separator(token: &str) -> bool {
    matches!(token, "&&" | "||" | ";" | "|")
}

fn clean_shell_token(token: &str) -> &str {
    token
        .trim_matches(|ch| matches!(ch, '"' | '\''))
        .trim_end_matches(';')
}

fn update_rm_flags(token: &str, recursive: &mut bool, force: &mut bool) {
    match token {
        "--recursive" => *recursive = true,
        "--force" => *force = true,
        _ if token.starts_with('-') && !token.starts_with("--") => {
            for flag in token.chars().skip(1) {
                match flag {
                    'r' | 'R' => *recursive = true,
                    'f' => *force = true,
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn is_root_target(token: &str) -> bool {
    // Filesystem root and globs.
    matches!(token, "/" | "/*" | "/." | "/./" | "/..")
        // Home directory wipes.
        || matches!(token, "~" | "~/" | "~/*" | "$HOME" | "${HOME}" | "$HOME/" | "$HOME/*")
        // Critical system directories that should never be recursively removed.
        || matches!(
            token.trim_end_matches('/'),
            "/usr" | "/etc" | "/bin" | "/sbin" | "/lib" | "/lib64" | "/var" | "/boot"
                | "/sys" | "/proc" | "/dev" | "/root" | "/home" | "/opt" | "/srv"
        )
}

pub(crate) fn enforce_shell_approval_policy(command: &str, ctx: &ToolContext) -> PeriResult<()> {
    let normalized = normalize_shell_command(command);
    if shell_command_is_approved(&normalized, ctx) {
        return Ok(());
    }
    if ctx.security.ask_before_install && is_install_command(&normalized) {
        return Err(PeriError::PermissionDenied(
            "dependency installation requires explicit user approval".to_string(),
        ));
    }
    if ctx.security.ask_before_delete && is_destructive_shell_command(&normalized) {
        return Err(PeriError::PermissionDenied(
            "destructive shell command requires explicit user approval".to_string(),
        ));
    }
    Ok(())
}

fn normalize_shell_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn shell_command_is_approved(command: &str, ctx: &ToolContext) -> bool {
    ctx.security
        .approved_shell_commands
        .iter()
        .any(|approved| normalize_shell_command(approved) == command)
        || (is_destructive_shell_command(command)
            && command_targets_within_scope(command, &ctx.security.approved_shell_path_scopes))
}

/// Returns true when one of `command`'s path arguments equals an approved scope
/// or lives under it. The previous implementation used a raw whole-command
/// `contains(scope)`, which over-approved: an approved scope of `src` would
/// auto-approve a destructive `rm -rf src_backup` (or any command that merely
/// mentioned the string). Matching whole path tokens (and descendants via
/// `scope/`) keeps the approval scoped to the path the operator actually
/// granted.
fn command_targets_within_scope(command: &str, scopes: &[String]) -> bool {
    let scopes: Vec<&str> = scopes
        .iter()
        .map(|s| s.trim().trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .collect();
    if scopes.is_empty() {
        return false;
    }
    command.split_whitespace().any(|raw| {
        let token = clean_shell_token(raw).trim_end_matches('/');
        scopes
            .iter()
            .any(|scope| token == *scope || token.starts_with(&format!("{scope}/")))
    })
}

fn is_install_command(command: &str) -> bool {
    let padded = format!(" {command} ");
    [
        " cargo add ",
        " cargo install ",
        " npm install ",
        " npm i ",
        " npm ci ",
        " pnpm add ",
        " pnpm install ",
        " yarn add ",
        " yarn install ",
        " pip install ",
        " pip3 install ",
        " python -m pip install ",
        " python3 -m pip install ",
        " uv add ",
        " uv pip install ",
        " poetry add ",
        " apt install ",
        " apt-get install ",
        " dnf install ",
        " yum install ",
        " brew install ",
    ]
    .iter()
    .any(|pattern| padded.contains(pattern))
}

fn is_destructive_shell_command(command: &str) -> bool {
    let padded = format!(" {command} ");
    command.starts_with("rm ")
        || padded.contains(" && rm ")
        || padded.contains(" ; rm ")
        || padded.contains(" | xargs rm ")
        || padded.contains(" find ") && padded.contains(" -delete ")
        || padded.contains(" git clean ")
        || padded.contains(" git reset --hard ")
        || padded.contains(" git push --force ")
        || padded.contains(" git push -f ")
}

pub(crate) fn shell_command(command: &str, ctx: &ToolContext) -> PeriResult<Command> {
    match ctx.security.sandbox {
        SandboxMode::None => {
            let mut process = Command::new("sh");
            process
                .arg("-c")
                .arg(command)
                .current_dir(&ctx.project_root);
            Ok(process)
        }
        SandboxMode::Docker => {
            let mut process = Command::new("docker");
            process.args(docker_shell_args(
                &ctx.project_root,
                command,
                &ctx.security.docker_image,
                ctx.security.docker_network,
                ctx.security.docker_read_only_rootfs,
                &ctx.security.docker_memory_limit,
            ));
            Ok(process)
        }
        SandboxMode::Firejail => {
            let mut process = Command::new("firejail");
            process
                .args(firejail_shell_args(
                    &ctx.project_root,
                    command,
                    ctx.security.docker_network,
                ))
                .current_dir(&ctx.project_root);
            Ok(process)
        }
    }
}

pub(crate) fn docker_shell_args(
    project_root: &Path,
    command: &str,
    image: &str,
    network: bool,
    read_only_rootfs: bool,
    memory_limit: &str,
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "-v".to_string(),
        format!("{}:/workspace", project_root.display()),
        "-w".to_string(),
        "/workspace".to_string(),
    ];
    if !network {
        args.extend(["--network".to_string(), "none".to_string()]);
    }
    if read_only_rootfs {
        // Lock the container fs read-only and provide a small tmpfs at
        // /tmp so tooling that writes scratch files (cargo, npm, pip,
        // gcc, etc.) keeps working. The workspace mount stays
        // read-write because it was added above.
        args.push("--read-only".to_string());
        args.extend(["--tmpfs".to_string(), "/tmp:rw,size=64m".to_string()]);
    }
    let trimmed_memory = memory_limit.trim();
    if !trimmed_memory.is_empty() {
        args.extend(["--memory".to_string(), trimmed_memory.to_string()]);
    }
    args.extend([
        image.to_string(),
        "sh".to_string(),
        "-lc".to_string(),
        command.to_string(),
    ]);
    args
}

pub(crate) fn firejail_shell_args(
    project_root: &Path,
    command: &str,
    network: bool,
) -> Vec<String> {
    let mut args = vec![
        "--quiet".to_string(),
        "--noprofile".to_string(),
        "--private-dev".to_string(),
        "--private-tmp".to_string(),
        format!("--whitelist={}", project_root.display()),
        format!("--read-write={}", project_root.display()),
    ];
    if !network {
        args.push("--net=none".to_string());
    }
    args.extend(["sh".to_string(), "-lc".to_string(), command.to_string()]);
    args
}

/// Renders a human-readable description of a prepared shell `Command`.
/// Used by `shell_dry_run` so the model and the operator can see
/// exactly which program + args + cwd would have run, without
/// actually launching the process.
pub(crate) fn describe_shell_command(command: &std::process::Command) -> String {
    let program = command.get_program().to_string_lossy().to_string();
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect();
    let cwd = command
        .get_current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<inherited>".to_string());
    format!("[dry-run] cwd={cwd} cmd={program} args={args:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn readonly_allows_plain_inspection_commands() {
        assert!(enforce_readonly_shell_policy("find . -name x").is_ok());
        assert!(enforce_readonly_shell_policy("sed 's/a/b/' f").is_ok());
        assert!(enforce_readonly_shell_policy("sed -n '1,5p' f").is_ok());
        assert!(enforce_readonly_shell_policy("rg pattern src").is_ok());
        assert!(enforce_readonly_shell_policy("cat README.md").is_ok());
        assert!(enforce_readonly_shell_policy("git log").is_ok());
    }

    #[test]
    fn readonly_rejects_find_exec_family() {
        // `+`-terminated forms avoid the `;` separator so the find-specific
        // action allowlist (not the generic write-syntax check) is what
        // rejects them.
        for cmd in [
            "find . -exec rm {} +",
            "find . -execdir cat {} +",
            "find . -delete",
            "find . -fprintf /tmp/x %p",
            "find . -fprint /tmp/x",
        ] {
            assert!(
                enforce_readonly_shell_policy(cmd).is_err(),
                "expected rejection: {cmd}"
            );
        }
    }

    #[test]
    fn is_allowed_readonly_segment_rejects_find_actions_directly() {
        // Exercises the find branch in isolation, independent of the
        // generic shell-write-syntax pre-check.
        assert!(!is_allowed_readonly_segment("find . -exec rm {} +"));
        assert!(!is_allowed_readonly_segment("find . -ok rm {} +"));
        assert!(!is_allowed_readonly_segment("find . -delete"));
        assert!(is_allowed_readonly_segment("find . -name x"));
    }

    #[test]
    fn readonly_rejects_sed_exec_write_read_and_inplace() {
        for cmd in [
            "sed -i s/a/b/ f",
            "sed --in-place s/a/b/ f",
            "sed 'e cat /etc/passwd' f",
            "sed 'w /tmp/out' f",
            "sed 'r /etc/passwd' f",
        ] {
            assert!(
                enforce_readonly_shell_policy(cmd).is_err(),
                "expected rejection: {cmd}"
            );
        }
    }

    #[test]
    fn readonly_rejects_awk_family() {
        // awk/gawk/mawk are removed from the allowlist entirely.
        assert!(!is_allowed_readonly_segment("awk 'BEGIN{system(\"id\")}'"));
        assert!(!is_allowed_readonly_segment("gawk '{print}'"));
        assert!(!is_allowed_readonly_segment("mawk '{print}'"));
        assert!(!is_allowed_readonly_segment("nawk '{print}'"));
        // Even a benign-looking awk invocation is now rejected end-to-end.
        assert!(enforce_readonly_shell_policy("awk '{print $1}' f").is_err());
    }

    #[test]
    fn pipe_reader_caps_and_marks_truncation() {
        let big = vec![b'a'; MAX_PIPE_CAPTURE_BYTES + 1024];
        let handle = read_pipe_in_background(Cursor::new(big));
        let out = handle.join().unwrap().unwrap();
        // Captured bytes are capped (plus the appended truncation marker),
        // never the full oversized input.
        assert!(out.len() < MAX_PIPE_CAPTURE_BYTES + 1024);
        let tail = String::from_utf8_lossy(&out);
        assert!(tail.contains("[output truncated"));
    }

    #[test]
    fn pipe_reader_passthrough_when_small() {
        let handle = read_pipe_in_background(Cursor::new(b"hello".to_vec()));
        let out = handle.join().unwrap().unwrap();
        assert_eq!(out, b"hello");
    }
}
