use super::*;

pub(crate) fn load_effective_config(
    project_root: &Path,
    explicit_config: Option<&Path>,
) -> Result<PeridotConfig> {
    load_effective_config_inner(project_root, explicit_config, true, true)
}

pub(crate) async fn maybe_run_first_launch_wizard(
    project_root: &Path,
    explicit_config: Option<&Path>,
    headless: bool,
    output: OutputFormat,
) -> Result<bool> {
    if headless || output == OutputFormat::Json || !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    let config_path = explicit_config
        .map(Path::to_path_buf)
        .unwrap_or_else(|| project_root.join(".peridot/config.toml"));
    if config_path.exists() {
        return Ok(false);
    }
    println!("No Peridot config found for this project. Let's set it up.");
    let result = init_project_config_value(project_root)?;
    run_config_wizard(&result).await?;
    Ok(true)
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

pub(crate) async fn run_config_command(
    command: &ConfigCommand,
    config: &PeridotConfig,
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    match command {
        ConfigCommand::Init => init_project_config(project_root, output).await,
        ConfigCommand::Wizard => run_config_wizard_command(project_root, output).await,
        ConfigCommand::Set { key, value } => {
            set_project_config_value(project_root, key, value, output)
        }
        ConfigCommand::Show => print_config(config, output),
        ConfigCommand::Edit => edit_project_config(project_root),
        ConfigCommand::Providers => print_provider_catalog(config, output),
        ConfigCommand::Models => print_model_catalog(config, output),
    }
}

fn print_model_catalog(config: &PeridotConfig, output: OutputFormat) -> Result<()> {
    // goal_checker and compaction always mirror `main` — show them as
    // derived (auto) so operators can see what each role will dispatch
    // to without thinking they are separately tunable.
    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "main": config.models.main,
                    "goal_checker": config.models.goal_checker(),
                    "compaction": config.models.compaction(),
                    "service_tier": config.models.service_tier,
                }))?
            );
        }
        OutputFormat::Text => {
            println!("main:         {}", config.models.main);
            println!(
                "service_tier: {}",
                config.models.service_tier.as_deref().unwrap_or("default")
            );
            println!(
                "goal_checker: {} (auto, follows main)",
                config.models.goal_checker()
            );
            println!(
                "compaction:   {} (auto, follows main)",
                config.models.compaction()
            );
        }
    }
    Ok(())
}

fn print_provider_catalog(config: &PeridotConfig, output: OutputFormat) -> Result<()> {
    let providers = [
        ("claude-api", "Anthropic Claude API"),
        ("openai-api", "OpenAI API (api.openai.com)"),
        ("openrouter-api", "OpenRouter (openrouter.ai/api)"),
        ("openai-oauth", "OpenAI OAuth direct (ChatGPT subscription)"),
    ];
    let active = config.auth.primary.as_str();
    match output {
        OutputFormat::Json => {
            let payload: Vec<_> = providers
                .iter()
                .map(|(name, description)| {
                    serde_json::json!({
                        "name": name,
                        "description": description,
                        "active": *name == active,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "active": active,
                    "providers": payload,
                }))?
            );
        }
        OutputFormat::Text => {
            println!("active provider: {active}");
            for (name, description) in providers {
                let marker = if name == active { "*" } else { " " };
                println!("  {marker} {name:<16} {description}");
            }
        }
    }
    Ok(())
}

pub(super) async fn init_project_config(project_root: &Path, output: OutputFormat) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    let configured = if output == OutputFormat::Text && std::io::stdin().is_terminal() {
        maybe_run_config_wizard(&result).await?
    } else {
        false
    };
    print_json_or_text_result(
        serde_json::json!({
            "config_path": result.config_path,
            "created_config": result.created_config,
            "updated_gitignore": result.updated_gitignore,
            "configured": configured
        }),
        if configured {
            format!(
                "initialized {} (created_config={}, updated_gitignore={}, configured=true)",
                result.peridot_dir.display(),
                result.created_config,
                result.updated_gitignore
            )
        } else {
            format!(
                "initialized {} (created_config={}, updated_gitignore={})",
                result.peridot_dir.display(),
                result.created_config,
                result.updated_gitignore
            )
        },
        output,
    )
}

pub(super) async fn run_config_wizard_command(
    project_root: &Path,
    output: OutputFormat,
) -> Result<()> {
    if output == OutputFormat::Json || !std::io::stdin().is_terminal() {
        anyhow::bail!("config wizard requires an interactive terminal");
    }
    let result = init_project_config_value(project_root)?;
    run_config_wizard(&result).await?;
    print_json_or_text_result(
        serde_json::json!({
            "config_path": result.config_path,
            "configured": true
        }),
        format!("configured {}", result.config_path.display()),
        output,
    )
}

pub(super) fn set_project_config_value(
    project_root: &Path,
    key: &str,
    value: &str,
    output: OutputFormat,
) -> Result<()> {
    let result = init_project_config_value(project_root)?;
    let mut config = toml::from_str::<PeridotConfig>(&fs::read_to_string(&result.config_path)?)?;
    set_config_key(&mut config, key, value)?;
    fs::write(&result.config_path, toml::to_string_pretty(&config)?)?;
    print_json_or_text_result(
        serde_json::json!({
            "config_path": result.config_path,
            "key": key,
            "value": value
        }),
        format!("set {key} = {value}"),
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

async fn maybe_run_config_wizard(result: &ConfigInitResult) -> Result<bool> {
    if !result.created_config
        && !prompt_yes_no(
            "A Peridot config already exists. Update provider and model settings?",
            false,
        )?
    {
        return Ok(false);
    }
    run_config_wizard(result).await?;
    Ok(true)
}

async fn run_config_wizard(result: &ConfigInitResult) -> Result<()> {
    println!();
    println!("Welcome to Peridot.");
    println!("Choose how this project should talk to models.");
    println!();

    let provider = prompt_choice(
        "Provider",
        &[
            "OpenRouter API key",
            "OpenAI OAuth direct / ChatGPT login",
            "Claude API key",
            "OpenAI API key",
        ],
        default_provider_choice(),
    )?;
    let profile = match provider {
        1 => {
            if read_managed_env_var("OPENROUTER_API_KEY")?.is_none() {
                println!();
                println!(
                    "OpenRouter is selected. Store a key with: peridot env set OPENROUTER_API_KEY sk-or-..."
                );
            }
            let model = prompt_model_choice(
                "OpenRouter main model",
                &[
                    "openai/gpt-4o-mini",
                    "openai/gpt-5.2",
                    "anthropic/claude-sonnet-4.5",
                ],
                "openai/gpt-4o-mini",
            )?;
            ConfigWizardProfile {
                auth_primary: "openrouter-api".to_string(),
                api_base_url: "https://openrouter.ai/api".to_string(),
                main_model: model,
            }
        }
        2 => {
            println!();
            run_login_command(AuthProvider::OpenaiOauth, OutputFormat::Text).await?;
            println!();
            let model = prompt_model_choice(
                "OpenAI OAuth main model",
                &["gpt-5.5", "gpt-5.5-fast", "gpt-5.4", "gpt-5.4-mini"],
                "gpt-5.5",
            )?;
            ConfigWizardProfile {
                auth_primary: "openai-oauth".to_string(),
                api_base_url: "https://chatgpt.com/backend-api/codex".to_string(),
                main_model: model,
            }
        }
        3 => {
            let model = prompt_model_choice(
                "Claude main model",
                &["claude-sonnet-4-6", "claude-haiku-4-5"],
                "claude-sonnet-4-6",
            )?;
            ConfigWizardProfile {
                auth_primary: "claude-api".to_string(),
                api_base_url: "https://api.anthropic.com".to_string(),
                main_model: model,
            }
        }
        4 => {
            let model =
                prompt_model_choice("OpenAI main model", &["gpt-5.2", "gpt-4o-mini"], "gpt-5.2")?;
            ConfigWizardProfile {
                auth_primary: "openai-api".to_string(),
                api_base_url: "https://api.openai.com".to_string(),
                main_model: model,
            }
        }
        _ => unreachable!("prompt_choice only returns listed choices"),
    };
    let config = config_from_wizard_profile(profile);
    fs::write(&result.config_path, toml::to_string_pretty(&config)?)?;
    println!();
    println!("Wrote {}", result.config_path.display());
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ConfigWizardProfile {
    pub(super) auth_primary: String,
    pub(super) api_base_url: String,
    pub(super) main_model: String,
}

pub(super) fn config_from_wizard_profile(profile: ConfigWizardProfile) -> PeridotConfig {
    let mut config = PeridotConfig::default();
    config.auth.primary = profile.auth_primary;
    config.api.base_url = profile.api_base_url;
    // `main` is the single model knob — goal_checker and compaction both
    // read from it via `ModelsConfig::goal_checker()` / `::compaction()`.
    config.models.main = profile.main_model;
    config
}

pub(super) fn set_config_key(config: &mut PeridotConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "auth.primary" => config.auth.primary = value.to_string(),
        "api.base_url" => config.api.base_url = value.to_string(),
        "api.timeout_seconds" => {
            config.api.timeout_seconds = value
                .parse()
                .with_context(|| "api.timeout_seconds must be an integer")?;
        }
        "api.max_retries" => {
            config.api.max_retries = value
                .parse()
                .with_context(|| "api.max_retries must be an integer")?;
        }
        "models.main" => config.models.main = value.to_string(),
        "models.service_tier" => config.models.service_tier = parse_service_tier(value)?,
        "models.goal_checker" | "models.compaction" => {
            // These roles deliberately track `models.main` so a single
            // switch reroutes every internal call. Refuse the write so
            // operators don't think they configured it independently
            // when in fact the value will be ignored at read time.
            anyhow::bail!(
                "`{key}` is not separately configurable — it always follows `models.main`. \
                 Set `models.main = \"{value}\"` instead."
            );
        }
        "defaults.mode" => config.defaults.mode = parse_env_mode("defaults.mode", value)?,
        "defaults.permission" => {
            config.defaults.permission = parse_env_permission("defaults.permission", value)?;
        }
        "defaults.max_turns" => {
            config.defaults.max_turns = value
                .parse()
                .with_context(|| "defaults.max_turns must be an integer")?;
        }
        "defaults.budget_usd" => {
            config.defaults.budget_usd = value
                .parse()
                .with_context(|| "defaults.budget_usd must be a decimal number")?;
        }
        "security.sandbox" => config.security.sandbox = parse_sandbox_mode(value)?,
        "git.auto_commit" => config.git.auto_commit = parse_bool_value("git.auto_commit", value)?,
        "git.auto_branch" => config.git.auto_branch = parse_bool_value("git.auto_branch", value)?,
        "git.branch_prefix" => config.git.branch_prefix = value.to_string(),
        "updates.auto_check" => {
            config.updates.auto_check = parse_bool_value("updates.auto_check", value)?;
        }
        "updates.auto_install" => {
            config.updates.auto_install = parse_bool_value("updates.auto_install", value)?;
        }
        _ => anyhow::bail!(
            "unsupported config key `{key}`; supported examples: auth.primary, api.base_url, models.main, models.service_tier, defaults.mode"
        ),
    }
    Ok(())
}

fn parse_service_tier(value: &str) -> Result<Option<String>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "off" | "none" | "default" | "standard" => Ok(None),
        "fast" | "priority" => Ok(Some("fast".to_string())),
        other => anyhow::bail!(
            "models.service_tier must be one of fast, priority, standard, default, off, or none (got {other})"
        ),
    }
}

fn parse_bool_value(name: &str, value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Ok(true),
        "false" | "no" | "0" | "off" => Ok(false),
        _ => anyhow::bail!("{name} must be true or false"),
    }
}

fn parse_sandbox_mode(value: &str) -> Result<peridot_common::SandboxMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(peridot_common::SandboxMode::None),
        "docker" => Ok(peridot_common::SandboxMode::Docker),
        "firejail" => Ok(peridot_common::SandboxMode::Firejail),
        _ => anyhow::bail!("security.sandbox must be one of none, docker, or firejail"),
    }
}

fn default_provider_choice() -> usize {
    if read_managed_env_var("OPENROUTER_API_KEY")
        .ok()
        .flatten()
        .is_some()
    {
        1
    } else if openai_oauth_credentials_path().exists() {
        2
    } else {
        1
    }
}

fn openai_oauth_credentials_path() -> PathBuf {
    if let Some(home) = std::env::var_os("PERIDOT_HOME") {
        return PathBuf::from(home).join("auth/openai-oauth.json");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".peridot/auth/openai-oauth.json")
}

fn prompt_model_choice(label: &str, options: &[&str], default: &str) -> Result<String> {
    let mut choices = options.to_vec();
    choices.push("Custom");
    let choice = prompt_choice(label, &choices, 1)?;
    if choice == choices.len() {
        prompt_text(&format!("{label} id"), default)
    } else {
        Ok(choices[choice - 1].to_string())
    }
}

fn prompt_choice(label: &str, options: &[&str], default: usize) -> Result<usize> {
    anyhow::ensure!(!options.is_empty(), "prompt options cannot be empty");
    anyhow::ensure!(
        (1..=options.len()).contains(&default),
        "default prompt choice is out of range"
    );
    loop {
        println!("{label}:");
        for (index, option) in options.iter().enumerate() {
            println!("  {}. {}", index + 1, option);
        }
        print!("Choose [{default}]: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let answer = answer.trim();
        if answer.is_empty() {
            return Ok(default);
        }
        if let Ok(choice) = answer.parse::<usize>()
            && (1..=options.len()).contains(&choice)
        {
            return Ok(choice);
        }
        println!("Enter a number from 1 to {}.", options.len());
    }
}

fn prompt_yes_no(question: &str, default: bool) -> Result<bool> {
    let hint = if default { "Y/n" } else { "y/N" };
    loop {
        print!("{question} [{hint}]: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        match answer.as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("Enter y or n."),
        }
    }
}

fn prompt_text(question: &str, default: &str) -> Result<String> {
    loop {
        print!("{question} [{default}]: ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let answer = answer.trim();
        if answer.is_empty() {
            return Ok(default.to_string());
        }
        if !answer.is_empty() {
            return Ok(answer.to_string());
        }
    }
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
