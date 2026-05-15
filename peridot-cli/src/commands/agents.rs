use super::*;

pub(crate) fn run_agents_command(
    command: &AgentsCommand,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        AgentsCommand::Init => {
            let path = project_root.join("AGENTS.md");
            let created = if path.exists() {
                false
            } else {
                let profile = ProjectScanner::new().scan(project_root)?;
                fs::write(&path, agents_draft(&profile))?;
                true
            };
            print_json_or_text_result(
                serde_json::json!({"path": path, "created": created}),
                format!("AGENTS.md created={created}"),
                output,
            )
        }
        AgentsCommand::Show => {
            let path = find_agents_instruction(project_root)
                .with_context(|| "no AGENTS.md-compatible instruction file found")?;
            let content = fs::read_to_string(&path)?;
            match output {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "path": path,
                        "content": content
                    }))?
                ),
                OutputFormat::Text => print!("{content}"),
            }
            Ok(())
        }
    }
}

pub(super) fn find_agents_instruction(project_root: &Path) -> Option<PathBuf> {
    [
        ".peridot/AGENTS.md",
        "AGENTS.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
    ]
    .into_iter()
    .map(|path| project_root.join(path))
    .find(|path| path.exists())
}

pub(super) fn agents_draft(profile: &ProjectProfile) -> String {
    let build = profile.commands.build.as_deref().unwrap_or("");
    let test = profile.commands.test.as_deref().unwrap_or("");
    let lint = profile.commands.lint.as_deref().unwrap_or("");
    let format = profile.commands.format.as_deref().unwrap_or("");
    format!(
        "# Peridot Agent Instructions\n\n\
## project\n\
name: {}\n\
description: Generated Peridot project guidance draft.\n\n\
## commands\n\
build: {}\n\
test: {}\n\
lint: {}\n\
format: {}\n\n\
## style\n\
- Keep changes scoped and buildable.\n\
- Add or update tests for behavior changes.\n\n\
## boundaries\n\
- DO NOT modify generated files without explicit approval.\n\
- DO NOT commit secrets or local memory databases.\n\n\
## preferences\n\
default_mode: execute\n\
default_permission: auto\n",
        profile.name, build, test, lint, format
    )
}
