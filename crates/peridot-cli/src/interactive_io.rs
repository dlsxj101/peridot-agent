use super::*;
use std::io::{IsTerminal, Read};

pub(super) fn run_tui_lifecycle_hooks(
    state: &TuiState,
    config: &PeridotConfig,
    project_root: &Path,
) -> Result<()> {
    let runner = HookRunner::new(project_root, config.hooks.clone());
    for event in &state.lifecycle_events {
        let mut variables = HookVariables::new();
        variables.insert(
            "project_root".to_string(),
            project_root.display().to_string(),
        );
        variables.insert("workspace".to_string(), project_root.display().to_string());
        match event.event.as_str() {
            "mode_switch" => {
                variables.insert("from_mode".to_string(), event.from.clone());
                variables.insert("to_mode".to_string(), event.to.clone());
            }
            "permission_switch" => {
                variables.insert("from_permission".to_string(), event.from.clone());
                variables.insert("to_permission".to_string(), event.to.clone());
            }
            _ => {}
        }
        variables.insert("from".to_string(), event.from.clone());
        variables.insert("to".to_string(), event.to.clone());
        runner.run_lifecycle_hooks(&event.event, &variables)?;
    }
    Ok(())
}

pub(super) fn read_piped_task() -> Result<Option<String>> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut task = String::new();
    stdin.lock().read_to_string(&mut task)?;
    let task = task.trim().to_string();
    if task.is_empty() {
        Ok(None)
    } else {
        Ok(Some(task))
    }
}
