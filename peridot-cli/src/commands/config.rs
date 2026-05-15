use super::*;

pub(crate) fn load_effective_config(
    project_root: &Path,
    explicit_config: Option<&Path>,
) -> Result<PeridotConfig> {
    load_effective_config_inner(project_root, explicit_config, true, true)
}

pub(super) fn load_effective_config_inner(
    project_root: &Path,
    explicit_config: Option<&Path>,
    include_global: bool,
    include_env: bool,
) -> Result<PeridotConfig> {
    let mut config = PeridotConfig::default();
    if include_global && let Some(global_config) = global_config_path() {
        merge_config_file(&global_config, false, &mut config)?;
    }
    apply_agents_preferences(project_root, &mut config)?;

    let project_config;
    let (path, required) = match explicit_config {
        Some(path) => (path, true),
        None => {
            project_config = project_root.join(".peridot/config.toml");
            (project_config.as_path(), false)
        }
    };
    merge_config_file(path, required, &mut config)?;
    if include_env {
        apply_env_config(&mut config)?;
    }
    Ok(config)
}

pub(super) fn merge_config_file(
    path: &Path,
    required: bool,
    config: &mut PeridotConfig,
) -> Result<()> {
    if !path.exists() {
        if required {
            anyhow::bail!("config file not found: {}", path.display());
        }
        return Ok(());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let project_config = toml::from_str::<PeridotConfig>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let raw_config = toml::from_str::<toml::Value>(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    merge_project_config(&raw_config, project_config, config);
    Ok(())
}

pub(super) fn global_config_path() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("PERIDOT_HOME") {
        return Some(PathBuf::from(home).join("config.toml"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".peridot/config.toml"))
}

pub(super) fn apply_agents_preferences(
    project_root: &Path,
    config: &mut PeridotConfig,
) -> Result<()> {
    let profile = ProjectScanner::new().scan(project_root)?;
    let preferences = profile.preferences;
    if let Some(mode) = preferences.default_mode {
        config.defaults.mode = mode;
    }
    if let Some(permission) = preferences.default_permission {
        config.defaults.permission = permission;
    }
    if let Some(ask_before_install) = preferences.ask_before_install {
        config.security.ask_before_install = ask_before_install;
    }
    if let Some(ask_before_delete) = preferences.ask_before_delete {
        config.security.ask_before_delete = ask_before_delete;
    }
    if let Some(auto_commit) = preferences.auto_commit {
        config.git.auto_commit = auto_commit;
    }
    if let Some(commit_frequency) = preferences.commit_frequency {
        config.git.commit_frequency = commit_frequency;
    }
    if let Some(branch_prefix) = preferences.branch_prefix {
        config.git.branch_prefix = branch_prefix;
    }
    Ok(())
}

pub(super) fn apply_env_config(config: &mut PeridotConfig) -> Result<()> {
    if let Ok(model) = std::env::var("PERIDOT_MODEL")
        && !model.trim().is_empty()
    {
        config.models.main = model;
    }
    if let Ok(mode) = std::env::var("PERIDOT_MODE") {
        config.defaults.mode = parse_env_mode("PERIDOT_MODE", &mode)?;
    }
    if let Ok(permission) = std::env::var("PERIDOT_PERMISSION") {
        config.defaults.permission = parse_env_permission("PERIDOT_PERMISSION", &permission)?;
    }
    if let Ok(budget) = std::env::var("PERIDOT_BUDGET") {
        config.defaults.budget_usd = budget.parse().with_context(|| {
            format!("failed to parse PERIDOT_BUDGET as a decimal number: {budget}")
        })?;
    }
    if let Ok(max_turns) = std::env::var("PERIDOT_MAX_TURNS") {
        config.defaults.max_turns = max_turns.parse().with_context(|| {
            format!("failed to parse PERIDOT_MAX_TURNS as an integer: {max_turns}")
        })?;
    }
    Ok(())
}

pub(super) fn parse_env_mode(name: &str, value: &str) -> Result<peridot_common::ExecutionMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "plan" => Ok(peridot_common::ExecutionMode::Plan),
        "execute" => Ok(peridot_common::ExecutionMode::Execute),
        "goal" => Ok(peridot_common::ExecutionMode::Goal),
        _ => anyhow::bail!("{name} must be one of plan, execute, or goal"),
    }
}

pub(super) fn parse_env_permission(
    name: &str,
    value: &str,
) -> Result<peridot_common::PermissionMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "safe" => Ok(peridot_common::PermissionMode::Safe),
        "auto" => Ok(peridot_common::PermissionMode::Auto),
        "yolo" => Ok(peridot_common::PermissionMode::Yolo),
        _ => anyhow::bail!("{name} must be one of safe, auto, or yolo"),
    }
}

pub(super) fn merge_project_config(
    raw_config: &toml::Value,
    project_config: PeridotConfig,
    config: &mut PeridotConfig,
) {
    if raw_config.get("auth").is_some() {
        config.auth = project_config.auth;
    }
    if raw_config.get("models").is_some() {
        config.models = project_config.models;
    }
    if raw_config.get("api").is_some() {
        config.api = project_config.api;
    }
    if raw_config.get("context").is_some() {
        config.context = project_config.context;
    }
    if raw_config.get("memory").is_some() {
        config.memory = project_config.memory;
    }
    if raw_config.get("tui").is_some() {
        config.tui = project_config.tui;
    }
    if raw_config.get("mcp").is_some() {
        config.mcp = project_config.mcp;
    }
    if raw_config.get("hooks").is_some() {
        config.hooks = project_config.hooks;
    }
    if raw_config.get("git").is_some() {
        config.git = project_config.git;
    }
    if raw_config.get("updates").is_some() {
        config.updates = project_config.updates;
    }
    if let Some(defaults) = raw_config.get("defaults").and_then(toml::Value::as_table) {
        if defaults.contains_key("mode") {
            config.defaults.mode = project_config.defaults.mode;
        }
        if defaults.contains_key("permission") {
            config.defaults.permission = project_config.defaults.permission;
        }
        if defaults.contains_key("max_turns") {
            config.defaults.max_turns = project_config.defaults.max_turns;
        }
        if defaults.contains_key("budget_usd") {
            config.defaults.budget_usd = project_config.defaults.budget_usd;
        }
        if defaults.contains_key("budget_warning_pct") {
            config.defaults.budget_warning_pct = project_config.defaults.budget_warning_pct;
        }
    }
    if let Some(security) = raw_config.get("security").and_then(toml::Value::as_table) {
        if security.contains_key("sandbox") {
            config.security.sandbox = project_config.security.sandbox;
        }
        if security.contains_key("docker_image") {
            config.security.docker_image = project_config.security.docker_image;
        }
        if security.contains_key("docker_network") {
            config.security.docker_network = project_config.security.docker_network;
        }
        if security.contains_key("ask_before_install") {
            config.security.ask_before_install = project_config.security.ask_before_install;
        }
        if security.contains_key("ask_before_delete") {
            config.security.ask_before_delete = project_config.security.ask_before_delete;
        }
    }
}

pub(crate) fn run_config_command(
    command: &ConfigCommand,
    config: &PeridotConfig,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        ConfigCommand::Init => init_project_config(project_root, output),
        ConfigCommand::Show => print_config(config, output),
        ConfigCommand::Edit => edit_project_config(project_root),
    }
}

pub(super) fn init_project_config(project_root: &Path, output: OutputFormat) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    print_json_or_text_result(
        serde_json::json!({
            "config_path": result.config_path,
            "created_config": result.created_config,
            "updated_gitignore": result.updated_gitignore
        }),
        format!(
            "initialized {} (created_config={}, updated_gitignore={})",
            result.peridot_dir.display(),
            result.created_config,
            result.updated_gitignore
        ),
        output,
    )
}

pub(super) fn edit_project_config(project_root: &Path) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = Command::new(&editor)
        .arg(&result.config_path)
        .status()
        .with_context(|| format!("failed to launch editor `{editor}`"))?;
    if !status.success() {
        anyhow::bail!("editor `{editor}` exited with {status}");
    }
    Ok(())
}

pub(super) struct ConfigInitResult {
    pub(super) peridot_dir: PathBuf,
    pub(super) config_path: PathBuf,
    pub(super) created_config: bool,
    pub(super) updated_gitignore: bool,
}

pub(super) fn init_project_config_value(project_root: &Path) -> Result<ConfigInitResult> {
    let peridot_dir = project_root.join(".peridot");
    fs::create_dir_all(peridot_dir.join("hooks"))?;
    fs::create_dir_all(peridot_dir.join("skills"))?;
    let config_path = peridot_dir.join("config.toml");
    let created_config = if config_path.exists() {
        false
    } else {
        let config = toml::to_string_pretty(&PeridotConfig::default())?;
        fs::write(&config_path, config)?;
        true
    };
    let gitignore_path = project_root.join(".gitignore");
    let managed_entries = [
        ".peridot/memory.db",
        ".peridot/mem/",
        ".peridot/sessions/",
        ".peridot/skills/auto/",
        ".peridot/logs/",
    ];
    let mut gitignore = fs::read_to_string(&gitignore_path).unwrap_or_default();
    let mut changed_gitignore = false;
    for entry in managed_entries {
        if !gitignore.lines().any(|line| line.trim() == entry) {
            if !gitignore.ends_with('\n') && !gitignore.is_empty() {
                gitignore.push('\n');
            }
            gitignore.push_str(entry);
            gitignore.push('\n');
            changed_gitignore = true;
        }
    }
    if changed_gitignore {
        fs::write(&gitignore_path, gitignore)?;
    }
    Ok(ConfigInitResult {
        peridot_dir,
        config_path,
        created_config,
        updated_gitignore: changed_gitignore,
    })
}

pub(super) fn print_config(config: &PeridotConfig, output: OutputFormat) -> Result<()> {
    match output {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(config)?),
        OutputFormat::Text => {
            println!("auth.primary = {}", config.auth.primary);
            println!("models.main = {}", config.models.main);
            println!("defaults.mode = {}", config.defaults.mode);
            println!("defaults.permission = {}", config.defaults.permission);
            println!("defaults.max_turns = {}", config.defaults.max_turns);
            println!("defaults.budget_usd = {}", config.defaults.budget_usd);
            println!("security.sandbox = {}", config.security.sandbox);
            println!("security.docker_image = {}", config.security.docker_image);
            println!(
                "security.docker_network = {}",
                config.security.docker_network
            );
            println!(
                "security.ask_before_install = {}",
                config.security.ask_before_install
            );
            println!(
                "security.ask_before_delete = {}",
                config.security.ask_before_delete
            );
            println!("git.auto_commit = {}", config.git.auto_commit);
            println!("git.commit_frequency = {}", config.git.commit_frequency);
            println!("git.branch_prefix = {}", config.git.branch_prefix);
            println!("git.auto_branch = {}", config.git.auto_branch);
            println!("updates.auto_check = {}", config.updates.auto_check);
            println!(
                "updates.auto_check_interval = {}",
                config.updates.auto_check_interval
            );
            println!("updates.auto_install = {}", config.updates.auto_install);
        }
    }
    Ok(())
}
