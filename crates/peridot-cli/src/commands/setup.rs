use super::*;

pub(crate) fn run_setup_command(project_root: &Path, output: OutputFormat) -> Result<()> {
    let config_result = init_project_config_value(project_root)?;
    let agents_path = project_root.join("AGENTS.md");
    let created_agents = if find_agents_instruction(project_root).is_none() {
        let profile = ProjectScanner::new().scan(project_root)?;
        fs::write(&agents_path, agents_draft(&profile))?;
        true
    } else {
        false
    };
    print_json_or_text_result(
        serde_json::json!({
            "config_path": config_result.config_path,
            "created_config": config_result.created_config,
            "updated_gitignore": config_result.updated_gitignore,
            "agents_path": agents_path,
            "created_agents": created_agents
        }),
        format!(
            "setup complete (created_config={}, updated_gitignore={}, created_agents={})",
            config_result.created_config, config_result.updated_gitignore, created_agents
        ),
        output,
    )
}
