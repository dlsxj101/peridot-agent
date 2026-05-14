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
    /// Creates a hook runner.
    pub fn new(project_root: impl Into<PathBuf>, hooks: HooksConfig) -> Self {
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
        for hook in hooks
            .iter()
            .filter(|hook| hook.event == event && hook_path_matches(hook, variables))
        {
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
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&command)
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
                    command,
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
                    command,
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

/// Builds standard tool-hook variables.
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
}
