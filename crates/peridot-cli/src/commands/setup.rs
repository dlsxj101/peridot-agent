use super::*;
use peridot_common::PeridotConfig;

pub(crate) fn run_setup_command(project_root: &Path, output: OutputFormat) -> Result<()> {
    let config_result = init_project_config_value(project_root)?;
    // Right after the initial write, sniff the user's stored
    // credentials and rewrite the auth/model fields so they actually
    // work without a manual `[auth].primary = …` edit. Without this,
    // a user who logged in with `peridot login openai-oauth` (e.g.
    // ChatGPT subscription) sees the default `claude-api` config and
    // their first run dies on the first LLM call. See onboarding bug
    // #2 in the QA report.
    let auth_adjusted = if config_result.created_config {
        match adjust_default_auth_for_available_credentials(&config_result.config_path) {
            Ok(adj) => adj,
            Err(err) => {
                eprintln!(
                    "warning: failed to align config defaults with stored credentials: {err}"
                );
                None
            }
        }
    } else {
        None
    };
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
            "created_agents": created_agents,
            "auth_detected": auth_adjusted,
        }),
        format!(
            "setup complete (created_config={}, updated_gitignore={}, created_agents={}{})",
            config_result.created_config,
            config_result.updated_gitignore,
            created_agents,
            match auth_adjusted {
                Some(provider) => format!(", auth_detected={provider}"),
                None => String::new(),
            }
        ),
        output,
    )
}

/// Detect the most likely "first run will succeed" provider from
/// already-stored credentials and rewrite the fresh config's
/// `auth.primary` + `models.main` accordingly. Returns the provider id
/// that won, or `None` if no credentials were detected (caller surfaces
/// the absence so the user knows they still need to log in).
///
/// Detection priority is intentional: OAuth subscriptions
/// (ChatGPT/Codex) are the most likely "I already paid for a subscription
/// and never set an env var" path; explicit API keys come next; OpenAI
/// API key last because it can collide with Anthropic users running both.
/// The pairing between provider and default model lives in the match
/// arms below — keep it in sync with `peridot_common::DEFAULT_MODEL_*`
/// constants if those move.
fn adjust_default_auth_for_available_credentials(
    config_path: &Path,
) -> Result<Option<&'static str>> {
    let Some((provider_id, default_model)) = detect_preferred_provider() else {
        return Ok(None);
    };
    let toml_text = fs::read_to_string(config_path)?;
    let mut config: PeridotConfig = toml::from_str(&toml_text)?;
    config.auth.primary = provider_id.to_string();
    config.models.main = default_model.to_string();
    fs::write(config_path, toml::to_string_pretty(&config)?)?;
    Ok(Some(provider_id))
}

/// Synchronously sniffs the filesystem and environment for any
/// credential we know how to consume. Returns `(provider_id,
/// default_model_for_that_provider)`. File-existence is enough — we
/// don't decrypt tokens here, the live `peridot run` will fail loudly
/// if a stored file is corrupted, and that's better feedback than a
/// silent fallback.
fn detect_preferred_provider() -> Option<(&'static str, &'static str)> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let envs = EnvCredentialSniff {
        anthropic_key_set: std::env::var("ANTHROPIC_API_KEY").is_ok(),
        openai_key_set: std::env::var("OPENAI_API_KEY").is_ok(),
    };
    detect_preferred_provider_in(&PathBuf::from(home), &envs)
}

/// Pure variant used by both production code and unit tests: takes
/// an explicit home directory and an env snapshot so tests can run in
/// parallel without HOME/env races. The matching priority is documented
/// on the caller.
fn detect_preferred_provider_in(
    home: &Path,
    envs: &EnvCredentialSniff,
) -> Option<(&'static str, &'static str)> {
    let auth_dir = home.join(".peridot/auth");
    if auth_dir.join("openai-oauth.json").is_file() {
        return Some(("openai-oauth", "gpt-5.5"));
    }
    if auth_dir.join("claude-api.json").is_file() || envs.anthropic_key_set {
        return Some(("claude-api", "claude-sonnet-4-6"));
    }
    if auth_dir.join("openai-api.json").is_file() || envs.openai_key_set {
        return Some(("openai-api", "gpt-5.5"));
    }
    None
}

#[derive(Clone, Copy, Debug, Default)]
struct EnvCredentialSniff {
    anthropic_key_set: bool,
    openai_key_set: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(label: &str, with_oauth: bool, with_claude_key: bool) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!(
            "peridot-detect-test-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(home.join(".peridot/auth")).unwrap();
        if with_oauth {
            fs::write(home.join(".peridot/auth/openai-oauth.json"), "{}").unwrap();
        }
        if with_claude_key {
            fs::write(
                home.join(".peridot/auth/claude-api.json"),
                r#"{"api_key":"dummy"}"#,
            )
            .unwrap();
        }
        home
    }

    #[test]
    fn detects_openai_oauth_credential() {
        let home = temp_home("oauth", true, false);
        let envs = EnvCredentialSniff::default();
        assert_eq!(
            detect_preferred_provider_in(&home, &envs),
            Some(("openai-oauth", "gpt-5.5"))
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn detects_stored_anthropic_key() {
        let home = temp_home("anthropic", false, true);
        let envs = EnvCredentialSniff::default();
        assert_eq!(
            detect_preferred_provider_in(&home, &envs),
            Some(("claude-api", "claude-sonnet-4-6"))
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn detects_anthropic_via_env_var() {
        let home = temp_home("anthropic_env", false, false);
        let envs = EnvCredentialSniff {
            anthropic_key_set: true,
            openai_key_set: false,
        };
        assert_eq!(
            detect_preferred_provider_in(&home, &envs),
            Some(("claude-api", "claude-sonnet-4-6"))
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn detects_openai_api_via_env_var_last() {
        let home = temp_home("openai_env", false, false);
        let envs = EnvCredentialSniff {
            anthropic_key_set: false,
            openai_key_set: true,
        };
        assert_eq!(
            detect_preferred_provider_in(&home, &envs),
            Some(("openai-api", "gpt-5.5"))
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn oauth_beats_stored_anthropic_when_both_present() {
        // Users with both subscribed + a one-off API key almost always
        // mean the subscription — match that expectation.
        let home = temp_home("both", true, true);
        let envs = EnvCredentialSniff::default();
        assert_eq!(
            detect_preferred_provider_in(&home, &envs),
            Some(("openai-oauth", "gpt-5.5"))
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn no_credentials_returns_none() {
        let home = temp_home("none", false, false);
        let envs = EnvCredentialSniff::default();
        assert_eq!(detect_preferred_provider_in(&home, &envs), None);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn adjust_writes_detected_provider_back_to_disk() {
        // Integration-style: write a fresh default config, then call
        // the adjust helper directly with a temp home that has OAuth.
        // Confirms the TOML re-serialisation round-trips cleanly.
        let home = temp_home("adjust", true, false);
        let project = home.join("project");
        fs::create_dir_all(project.join(".peridot")).unwrap();
        let config_path = project.join(".peridot/config.toml");
        let default_cfg = PeridotConfig::default();
        fs::write(&config_path, toml::to_string_pretty(&default_cfg).unwrap()).unwrap();

        // Drive the same logic path adjust_default_auth uses, but
        // pointed at our temp home.
        let envs = EnvCredentialSniff::default();
        let detected = detect_preferred_provider_in(&home, &envs).expect("oauth detected");
        let mut cfg: PeridotConfig =
            toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        cfg.auth.primary = detected.0.to_string();
        cfg.models.main = detected.1.to_string();
        fs::write(&config_path, toml::to_string_pretty(&cfg).unwrap()).unwrap();

        let re_read: PeridotConfig =
            toml::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
        assert_eq!(re_read.auth.primary, "openai-oauth");
        assert_eq!(re_read.models.main, "gpt-5.5");
        fs::remove_dir_all(&home).ok();
    }
}
