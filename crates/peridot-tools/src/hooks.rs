//! User hook execution with deterministic safety boundaries.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use peridot_common::{HookConfig, HookFailureMode, HooksConfig, PeriError, PeriResult};
use serde_json::Value;

/// Runtime variable map used when rendering hook command templates.
pub type HookVariables = BTreeMap<String, String>;

/// Result from one hook execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookOutcome {
    /// Hook event name.
    pub event: String,
    /// Rendered command.
    pub command: String,
    /// Failure policy.
    pub on_failure: HookFailureMode,
    /// Process exit code, or -1 when timed out or unavailable.
    pub exit_code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr or validation error.
    pub stderr: String,
}

impl HookOutcome {
    fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Executes configured hooks from a project root.
#[derive(Clone, Debug)]
pub struct HookRunner {
    project_root: PathBuf,
    hooks: HooksConfig,
}

impl HookRunner {
    /// Creates a hook runner. Auto-discovers any scripts in
    /// `.peridot/hooks/` whose filename matches the convention
    /// (`pre-<tool>.sh`, `post-<tool>.sh`, `event-<name>.sh`,
    /// `lifecycle-<name>.sh`) and merges them with the explicitly
    /// configured `hooks`. Config-declared entries take precedence
    /// when the same event already exists.
    pub fn new(project_root: impl Into<PathBuf>, hooks: HooksConfig) -> Self {
        let project_root = project_root.into();
        let mut merged = hooks;
        merge_discovered_hooks(&project_root, &mut merged);
        Self {
            project_root,
            hooks: merged,
        }
    }

    /// Creates a hook runner without scanning the filesystem.
    /// Useful for tests and minimal harnesses that want exact control
    /// over which hooks fire.
    pub fn new_without_discovery(project_root: impl Into<PathBuf>, hooks: HooksConfig) -> Self {
        Self {
            project_root: project_root.into(),
            hooks,
        }
    }

    /// Runs tool hooks for a concrete event, such as pre:file_write.
    pub fn run_tool_hooks(
        &self,
        event: &str,
        variables: &HookVariables,
    ) -> PeriResult<Vec<HookOutcome>> {
        self.run_matching(&self.hooks.tool, event, variables)
    }

    /// Runs system event hooks.
    pub fn run_event_hooks(
        &self,
        event: &str,
        variables: &HookVariables,
    ) -> PeriResult<Vec<HookOutcome>> {
        self.run_matching(&self.hooks.event, event, variables)
    }

    /// Runs lifecycle hooks.
    pub fn run_lifecycle_hooks(
        &self,
        event: &str,
        variables: &HookVariables,
    ) -> PeriResult<Vec<HookOutcome>> {
        self.run_matching(&self.hooks.lifecycle, event, variables)
    }

    fn run_matching(
        &self,
        hooks: &[HookConfig],
        event: &str,
        variables: &HookVariables,
    ) -> PeriResult<Vec<HookOutcome>> {
        let mut outcomes = Vec::new();
        for hook in hooks.iter().filter(|hook| {
            hook_event_matches(&hook.event, event) && hook_path_matches(hook, variables)
        }) {
            let outcome = self.run_one(hook, variables);
            match outcome {
                Ok(outcome) if outcome.success() => outcomes.push(outcome),
                Ok(outcome) => {
                    let should_block = outcome.on_failure == HookFailureMode::Block;
                    let message = format!(
                        "hook {} failed with {}: {}",
                        outcome.event, outcome.exit_code, outcome.stderr
                    );
                    outcomes.push(outcome);
                    if should_block {
                        return Err(PeriError::PermissionDenied(message));
                    }
                }
                Err(err) if hook.on_failure == HookFailureMode::Block => return Err(err),
                Err(err) => outcomes.push(HookOutcome {
                    event: hook.event.clone(),
                    command: hook.run.clone(),
                    on_failure: hook.on_failure,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: err.to_string(),
                }),
            }
        }
        Ok(outcomes)
    }

    fn run_one(&self, hook: &HookConfig, variables: &HookVariables) -> PeriResult<HookOutcome> {
        let command = render_template(&hook.run, variables);
        validate_hook_command(&self.project_root, &command)?;
        for attempt in 0..3 {
            let outcome = self.run_rendered_command(hook, &command)?;
            if attempt < 2 && hook_text_file_busy(&outcome) {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            return Ok(outcome);
        }
        unreachable!("bounded hook retry loop always returns")
    }

    fn run_rendered_command(&self, hook: &HookConfig, command: &str) -> PeriResult<HookOutcome> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.project_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| PeriError::Tool(format!("failed to run hook: {err}")))?;

        let deadline = Instant::now() + Duration::from_secs(self.hooks.timeout_seconds);
        loop {
            if child
                .try_wait()
                .map_err(|err| PeriError::Tool(format!("failed to poll hook: {err}")))?
                .is_some()
            {
                let output = child
                    .wait_with_output()
                    .map_err(|err| PeriError::Tool(format!("failed to collect hook: {err}")))?;
                return Ok(HookOutcome {
                    event: hook.event.clone(),
                    command: command.to_string(),
                    on_failure: hook.on_failure,
                    exit_code: output.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                });
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(HookOutcome {
                    event: hook.event.clone(),
                    command: command.to_string(),
                    on_failure: hook.on_failure,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: "hook timed out".to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn hook_text_file_busy(outcome: &HookOutcome) -> bool {
    outcome.exit_code == 126 && outcome.stderr.contains("Text file busy")
}

/// Builds standard tool-hook variables.
/// Scans `.peridot/hooks/` and appends auto-discovered entries to
/// `hooks`. The convention is filename-prefix-based:
///
/// - `pre-<tool>.sh`        → tool hook with `event = "pre:<tool>"`
/// - `post-<tool>.sh`       → tool hook with `event = "post:<tool>"`
/// - `event-<name>.sh`      → event hook with `event = "<name>"`
/// - `lifecycle-<name>.sh`  → lifecycle hook with `event = "<name>"`
///
/// Files that don't match the convention (utility scripts shared by
/// real hooks, `.md` notes, etc.) are silently ignored. Entries whose
/// event is already declared explicitly in `hooks` are skipped, so
/// `config.toml` declarations always win.
pub fn merge_discovered_hooks(project_root: &Path, hooks: &mut HooksConfig) {
    let hook_dir = project_root.join(".peridot").join("hooks");
    let entries = match std::fs::read_dir(&hook_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut tool_seen: std::collections::HashSet<String> =
        hooks.tool.iter().map(|h| h.event.clone()).collect();
    let mut event_seen: std::collections::HashSet<String> =
        hooks.event.iter().map(|h| h.event.clone()).collect();
    let mut lifecycle_seen: std::collections::HashSet<String> =
        hooks.lifecycle.iter().map(|h| h.event.clone()).collect();

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(parsed) = parse_hook_filename(name) else {
            continue;
        };
        let relative_run = format!(".peridot/hooks/{name}");
        let new_hook = HookConfig {
            event: parsed.event_name.clone(),
            run: relative_run,
            description: Some(format!("auto-discovered from {name}")),
            on_failure: HookFailureMode::Warn,
            only_paths: Vec::new(),
        };
        match parsed.kind {
            DiscoveredKind::Tool => {
                if tool_seen.insert(parsed.event_name) {
                    hooks.tool.push(new_hook);
                }
            }
            DiscoveredKind::Event => {
                if event_seen.insert(parsed.event_name) {
                    hooks.event.push(new_hook);
                }
            }
            DiscoveredKind::Lifecycle => {
                if lifecycle_seen.insert(parsed.event_name) {
                    hooks.lifecycle.push(new_hook);
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiscoveredKind {
    Tool,
    Event,
    Lifecycle,
}

struct DiscoveredHook {
    kind: DiscoveredKind,
    event_name: String,
}

/// Returns `Some(DiscoveredHook)` when `filename` matches one of the
/// four conventions, or `None` for utility scripts / non-hook files.
/// The trailing `.sh` is stripped before matching; case-sensitive.
fn parse_hook_filename(filename: &str) -> Option<DiscoveredHook> {
    let stem = filename
        .strip_suffix(".sh")
        .or_else(|| filename.strip_suffix(".bash"))
        .unwrap_or(filename);
    if let Some(rest) = stem.strip_prefix("pre-") {
        if rest.is_empty() {
            return None;
        }
        return Some(DiscoveredHook {
            kind: DiscoveredKind::Tool,
            event_name: format!("pre:{rest}"),
        });
    }
    if let Some(rest) = stem.strip_prefix("post-") {
        if rest.is_empty() {
            return None;
        }
        return Some(DiscoveredHook {
            kind: DiscoveredKind::Tool,
            event_name: format!("post:{rest}"),
        });
    }
    if let Some(rest) = stem.strip_prefix("event-") {
        if rest.is_empty() {
            return None;
        }
        return Some(DiscoveredHook {
            kind: DiscoveredKind::Event,
            event_name: rest.to_string(),
        });
    }
    if let Some(rest) = stem.strip_prefix("lifecycle-") {
        if rest.is_empty() {
            return None;
        }
        return Some(DiscoveredHook {
            kind: DiscoveredKind::Lifecycle,
            event_name: rest.to_string(),
        });
    }
    None
}

pub fn tool_hook_variables(tool: &str, params: &Value) -> HookVariables {
    let mut variables = HookVariables::new();
    variables.insert("tool".to_string(), tool.to_string());
    variables.insert("params_json".to_string(), params.to_string());
    if let Some(path) = params.get("path").and_then(Value::as_str) {
        variables.insert("path".to_string(), path.to_string());
    }
    if let Some(command) = params.get("command").and_then(Value::as_str) {
        variables.insert("command".to_string(), command.to_string());
    }
    variables
}

/// Builds standard lifecycle-hook variables.
pub fn lifecycle_hook_variables(
    session_id: &str,
    mode: &str,
    permission: &str,
    project_root: &Path,
    status: &str,
    summary: &str,
) -> HookVariables {
    let mut variables = HookVariables::new();
    variables.insert("session_id".to_string(), session_id.to_string());
    variables.insert("mode".to_string(), mode.to_string());
    variables.insert("permission".to_string(), permission.to_string());
    variables.insert(
        "project_root".to_string(),
        project_root.display().to_string(),
    );
    variables.insert("workspace".to_string(), project_root.display().to_string());
    variables.insert("status".to_string(), status.to_string());
    variables.insert("summary".to_string(), summary.to_string());
    variables
}

fn render_template(template: &str, variables: &HookVariables) -> String {
    let mut rendered = template.to_string();
    for (key, value) in variables {
        rendered = rendered.replace(&format!("{{{key}}}"), value);
    }
    rendered
}

fn validate_hook_command(project_root: &Path, command: &str) -> PeriResult<()> {
    let script = command
        .split_whitespace()
        .next()
        .ok_or_else(|| PeriError::Config("hook command is empty".to_string()))?;
    if !script.starts_with(".peridot/hooks/") || script.contains("..") {
        return Err(PeriError::PermissionDenied(format!(
            "hook command must start with .peridot/hooks/: {script}"
        )));
    }
    let hooks_root = project_root
        .join(".peridot/hooks")
        .canonicalize()
        .map_err(|_| {
            PeriError::PermissionDenied("hook root .peridot/hooks does not exist".to_string())
        })?;
    let script_path = project_root.join(script).canonicalize().map_err(|_| {
        PeriError::PermissionDenied(format!("hook script does not exist: {script}"))
    })?;
    if !script_path.starts_with(hooks_root) {
        return Err(PeriError::PermissionDenied(format!(
            "hook script escapes .peridot/hooks/: {script}"
        )));
    }
    Ok(())
}

fn hook_path_matches(hook: &HookConfig, variables: &HookVariables) -> bool {
    if hook.only_paths.is_empty() {
        return true;
    }
    let Some(path) = variables.get("path") else {
        return false;
    };
    hook.only_paths.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix("/**") {
            path == prefix || path.starts_with(&format!("{prefix}/"))
        } else {
            path == pattern
        }
    })
}

fn hook_event_matches(pattern: &str, event: &str) -> bool {
    pattern == event
        || pattern
            .strip_suffix("*")
            .is_some_and(|prefix| event.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("peridot-hooks-{name}-{}", std::process::id()))
    }

    #[test]
    fn runs_project_hook_script() {
        let root = temp_root("success");
        let hooks_dir = root.join(".peridot/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        let script = hooks_dir.join("echo.sh");
        fs::write(&script, "#!/bin/sh\necho hook:$1\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let runner = HookRunner::new(
            &root,
            HooksConfig {
                tool: vec![HookConfig {
                    event: "pre:file_write".to_string(),
                    run: ".peridot/hooks/echo.sh {path}".to_string(),
                    description: None,
                    on_failure: HookFailureMode::Warn,
                    only_paths: vec!["src/**".to_string()],
                }],
                ..HooksConfig::default()
            },
        );
        let mut variables = HookVariables::new();
        variables.insert("path".to_string(), "src/lib.rs".to_string());

        let outcomes = runner.run_tool_hooks("pre:file_write", &variables).unwrap();

        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].stdout.contains("hook:src/lib.rs"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blocks_disallowed_hook_location() {
        let root = temp_root("blocked");
        fs::create_dir_all(root.join(".peridot/hooks")).unwrap();
        let runner = HookRunner::new(
            &root,
            HooksConfig {
                tool: vec![HookConfig {
                    event: "pre:file_write".to_string(),
                    run: "echo unsafe".to_string(),
                    description: None,
                    on_failure: HookFailureMode::Block,
                    only_paths: Vec::new(),
                }],
                ..HooksConfig::default()
            },
        );

        let error = runner
            .run_tool_hooks("pre:file_write", &HookVariables::new())
            .unwrap_err();

        assert!(matches!(error, PeriError::PermissionDenied(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wildcard_event_matches_tool_hook() {
        let hook = HookConfig {
            event: "pre:*".to_string(),
            run: ".peridot/hooks/check.sh".to_string(),
            description: None,
            on_failure: HookFailureMode::Warn,
            only_paths: Vec::new(),
        };

        assert!(hook_event_matches(&hook.event, "pre:file_write"));
        assert!(!hook_event_matches(&hook.event, "post:file_write"));
    }

    #[test]
    fn lifecycle_variables_include_session_fields() {
        let variables = lifecycle_hook_variables(
            "session-1",
            "execute",
            "auto",
            Path::new("/workspace"),
            "done",
            "ok",
        );

        assert_eq!(variables["session_id"], "session-1");
        assert_eq!(variables["mode"], "execute");
        assert_eq!(variables["permission"], "auto");
        assert_eq!(variables["status"], "done");
    }

    #[test]
    fn parse_hook_filename_recognises_all_four_kinds() {
        let pre = super::parse_hook_filename("pre-file_write.sh").unwrap();
        assert_eq!(pre.kind, super::DiscoveredKind::Tool);
        assert_eq!(pre.event_name, "pre:file_write");

        let post = super::parse_hook_filename("post-shell_exec.sh").unwrap();
        assert_eq!(post.kind, super::DiscoveredKind::Tool);
        assert_eq!(post.event_name, "post:shell_exec");

        let event = super::parse_hook_filename("event-context_compacted.sh").unwrap();
        assert_eq!(event.kind, super::DiscoveredKind::Event);
        assert_eq!(event.event_name, "context_compacted");

        let lifecycle = super::parse_hook_filename("lifecycle-session_start.sh").unwrap();
        assert_eq!(lifecycle.kind, super::DiscoveredKind::Lifecycle);
        assert_eq!(lifecycle.event_name, "session_start");
    }

    #[test]
    fn parse_hook_filename_rejects_utility_scripts() {
        assert!(super::parse_hook_filename("common.sh").is_none());
        assert!(super::parse_hook_filename("README.md").is_none());
        assert!(super::parse_hook_filename("helpers.bash").is_none());
        // Edge case: bare prefix with nothing after.
        assert!(super::parse_hook_filename("pre-.sh").is_none());
    }

    #[test]
    fn parse_hook_filename_accepts_bash_extension() {
        let pre = super::parse_hook_filename("pre-file_write.bash").unwrap();
        assert_eq!(pre.event_name, "pre:file_write");
    }

    #[test]
    fn merge_discovered_hooks_picks_up_filesystem_scripts() {
        let root =
            std::env::temp_dir().join(format!("peridot-hooks-discovery-{}", std::process::id()));
        let hook_dir = root.join(".peridot").join("hooks");
        std::fs::create_dir_all(&hook_dir).unwrap();
        std::fs::write(hook_dir.join("pre-file_write.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(
            hook_dir.join("event-context_compacted.sh"),
            "#!/bin/sh\necho hi\n",
        )
        .unwrap();
        std::fs::write(hook_dir.join("common.sh"), "# utility, not a hook\n").unwrap();

        let mut hooks = HooksConfig::default();
        super::merge_discovered_hooks(&root, &mut hooks);

        assert_eq!(hooks.tool.len(), 1);
        assert_eq!(hooks.tool[0].event, "pre:file_write");
        assert_eq!(hooks.tool[0].run, ".peridot/hooks/pre-file_write.sh");
        assert_eq!(hooks.event.len(), 1);
        assert_eq!(hooks.event[0].event, "context_compacted");
        assert!(hooks.lifecycle.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn merge_discovered_hooks_does_not_override_config_declarations() {
        let root =
            std::env::temp_dir().join(format!("peridot-hooks-precedence-{}", std::process::id()));
        let hook_dir = root.join(".peridot").join("hooks");
        std::fs::create_dir_all(&hook_dir).unwrap();
        std::fs::write(hook_dir.join("pre-file_write.sh"), "#!/bin/sh\necho fs\n").unwrap();

        let mut hooks = HooksConfig {
            tool: vec![HookConfig {
                event: "pre:file_write".to_string(),
                run: "echo from-config".to_string(),
                description: None,
                on_failure: HookFailureMode::Block,
                only_paths: Vec::new(),
            }],
            ..HooksConfig::default()
        };
        super::merge_discovered_hooks(&root, &mut hooks);

        // Config-declared entry wins; filesystem-discovered hook is
        // not duplicated.
        assert_eq!(hooks.tool.len(), 1);
        assert_eq!(hooks.tool[0].run, "echo from-config");
        assert_eq!(hooks.tool[0].on_failure, HookFailureMode::Block);

        std::fs::remove_dir_all(root).unwrap();
    }
}
