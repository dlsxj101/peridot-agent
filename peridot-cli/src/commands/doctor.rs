//! `peridot doctor` — single-shot health audit.
//!
//! Walks the four most common failure surfaces (config validity,
//! provider auth, MCP server health, AGENTS metadata presence) and
//! reports each check's status. Used during onboarding and as a CI
//! smoke target ("does my Peridot install still work after the
//! upgrade?"). Returns non-zero when any check is `fail` so it
//! composes with shell pipelines.

use std::time::Duration;

use anyhow::Result;
use peridot_common::PeridotConfig;
use peridot_mcp::McpClient;

use super::{
    AuthProvider, OutputFormat, mcp::mcp_target, mcp::validate_mcp_server,
    output::print_json_or_text_result, read_managed_env_var, read_stored_api_key,
    read_stored_openai_oauth_credentials,
};
use std::path::Path;

#[derive(Clone, Debug)]
struct DoctorCheck {
    name: &'static str,
    status: DoctorStatus,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl DoctorStatus {
    fn label(self) -> &'static str {
        match self {
            DoctorStatus::Pass => "pass",
            DoctorStatus::Warn => "warn",
            DoctorStatus::Fail => "fail",
        }
    }
}

pub(crate) async fn run_doctor_command(
    project_root: &Path,
    config: &PeridotConfig,
    output: OutputFormat,
) -> Result<()> {
    let mut checks: Vec<DoctorCheck> = Vec::new();

    checks.push(check_workspace_layout(project_root));
    if config.auth.primary == "openai-oauth" {
        checks.push(check_openai_oauth_credentials().await);
    } else {
        checks.push(check_primary_provider(config));
    }
    checks.push(check_models_config(config));
    checks.push(check_agents_metadata(project_root));
    checks.extend(check_mcp_servers(config).await);
    checks.push(check_security_posture(config));

    let any_fail = checks.iter().any(|c| c.status == DoctorStatus::Fail);
    let any_warn = checks.iter().any(|c| c.status == DoctorStatus::Warn);

    let text_summary = checks
        .iter()
        .map(|c| format!("{}\t{}\t{}", c.status.label(), c.name, c.message))
        .collect::<Vec<_>>()
        .join("\n");

    let json_payload = serde_json::json!({
        "overall": if any_fail { "fail" } else if any_warn { "warn" } else { "pass" },
        "checks": checks.iter().map(|c| serde_json::json!({
            "name": c.name,
            "status": c.status.label(),
            "message": c.message,
        })).collect::<Vec<_>>(),
    });

    print_json_or_text_result(json_payload, text_summary, output)?;

    if any_fail {
        std::process::exit(1);
    }
    Ok(())
}

fn check_workspace_layout(project_root: &Path) -> DoctorCheck {
    let peridot_dir = project_root.join(".peridot");
    if peridot_dir.exists() {
        DoctorCheck {
            name: "workspace",
            status: DoctorStatus::Pass,
            message: format!(".peridot/ initialised at {}", peridot_dir.display()),
        }
    } else {
        DoctorCheck {
            name: "workspace",
            status: DoctorStatus::Warn,
            message: format!(
                ".peridot/ missing — run `peridot config init` in {}",
                project_root.display()
            ),
        }
    }
}

fn check_primary_provider(config: &PeridotConfig) -> DoctorCheck {
    let primary = config.auth.primary.clone();
    if primary == "claude-api" {
        return check_api_key_credential(
            "claude-api",
            "ANTHROPIC_API_KEY",
            AuthProvider::ClaudeApi,
        );
    }
    if primary == "openai-api" {
        return check_api_key_credential("openai-api", "OPENAI_API_KEY", AuthProvider::OpenaiApi);
    }
    if primary == "openrouter-api" {
        return check_api_key_credential(
            "openrouter-api",
            "OPENROUTER_API_KEY",
            AuthProvider::OpenrouterApi,
        );
    }
    // openai-oauth handled at the caller because the credential read is
    // async; falling through here means we synthesise a placeholder
    // here and the caller overrides it via `check_openai_oauth_credentials_async`.
    if primary == "openai-oauth" {
        return DoctorCheck {
            name: "provider:openai-oauth",
            status: DoctorStatus::Pass,
            message: "OAuth check pending (resolved async)".to_string(),
        };
    }
    DoctorCheck {
        name: "provider:unknown",
        status: DoctorStatus::Fail,
        message: format!(
            "unknown `auth.primary = {primary}`; expected claude-api / openai-api / openrouter-api / openai-oauth"
        ),
    }
}

async fn check_openai_oauth_credentials() -> DoctorCheck {
    match read_stored_openai_oauth_credentials().await {
        Ok(Some(_)) => DoctorCheck {
            name: "provider:openai-oauth",
            status: DoctorStatus::Pass,
            message: "stored OAuth credentials are present".to_string(),
        },
        Ok(None) => DoctorCheck {
            name: "provider:openai-oauth",
            status: DoctorStatus::Fail,
            message: "OAuth credentials missing — run `peridot login openai`".to_string(),
        },
        Err(err) => DoctorCheck {
            name: "provider:openai-oauth",
            status: DoctorStatus::Fail,
            message: format!("failed to read OAuth store: {err}"),
        },
    }
}

fn check_api_key_credential(
    label: &'static str,
    env_var: &str,
    stored: AuthProvider,
) -> DoctorCheck {
    if std::env::var(env_var).is_ok() {
        return DoctorCheck {
            name: label,
            status: DoctorStatus::Pass,
            message: format!("{env_var} is set"),
        };
    }
    if read_managed_env_var(env_var).is_ok() {
        return DoctorCheck {
            name: label,
            status: DoctorStatus::Pass,
            message: format!("{env_var} sourced via managed env"),
        };
    }
    match read_stored_api_key(stored) {
        Ok(Some(_)) => DoctorCheck {
            name: label,
            status: DoctorStatus::Pass,
            message: "stored API key is present".to_string(),
        },
        Ok(None) | Err(_) => DoctorCheck {
            name: label,
            status: DoctorStatus::Fail,
            message: format!("{env_var} not set and no stored credential — run `peridot login`"),
        },
    }
}

fn check_models_config(config: &PeridotConfig) -> DoctorCheck {
    let main = config.models.main.trim();
    if main.is_empty() {
        return DoctorCheck {
            name: "models",
            status: DoctorStatus::Fail,
            message: "models.main is empty — set it via `peridot config set models.main <name>`"
                .to_string(),
        };
    }
    DoctorCheck {
        name: "models",
        status: DoctorStatus::Pass,
        message: format!("models.main = {main}"),
    }
}

fn check_agents_metadata(project_root: &Path) -> DoctorCheck {
    let candidates = [
        ".peridot/AGENTS.md",
        "AGENTS.md",
        "CLAUDE.md",
        ".github/copilot-instructions.md",
    ];
    for relative in candidates {
        let path = project_root.join(relative);
        if path.exists() {
            return DoctorCheck {
                name: "agents_md",
                status: DoctorStatus::Pass,
                message: format!("found {}", path.display()),
            };
        }
    }
    DoctorCheck {
        name: "agents_md",
        status: DoctorStatus::Warn,
        message: "no AGENTS metadata file in project — `peridot agents init` to scaffold one"
            .to_string(),
    }
}

async fn check_mcp_servers(config: &PeridotConfig) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    if config.mcp.is_empty() {
        return checks;
    }
    for server in &config.mcp {
        let validation = validate_mcp_server(server);
        if let Err(err) = validation {
            checks.push(DoctorCheck {
                name: "mcp",
                status: DoctorStatus::Fail,
                message: format!("server `{}` invalid: {err}", server.name),
            });
            continue;
        }
        let client = McpClient::with_timeout(
            server.clone(),
            Duration::from_secs(server.timeout_seconds.max(1)),
        );
        match client.health_check().await {
            Ok(duration) => checks.push(DoctorCheck {
                name: "mcp",
                status: DoctorStatus::Pass,
                message: format!(
                    "{} ({}) reachable in {}ms",
                    server.name,
                    mcp_target(server),
                    duration.as_millis()
                ),
            }),
            Err(err) => checks.push(DoctorCheck {
                name: "mcp",
                status: DoctorStatus::Fail,
                message: format!("{}: health probe failed: {err}", server.name),
            }),
        }
    }
    checks
}

fn check_security_posture(config: &PeridotConfig) -> DoctorCheck {
    let sandbox = config.security.sandbox.to_string();
    let mode = sandbox.as_str();
    // None sandbox + ask_before_install false is a footgun — flag as warn.
    if mode == "none" && !config.security.ask_before_install && !config.security.ask_before_delete {
        return DoctorCheck {
            name: "security",
            status: DoctorStatus::Warn,
            message: "sandbox=none AND ask_before_install/delete=false — every shell command runs unconfined and unannounced. Consider enabling ask_before_* or a sandbox.".to_string(),
        };
    }
    DoctorCheck {
        name: "security",
        status: DoctorStatus::Pass,
        message: format!(
            "sandbox={mode}, ask_before_install={}, ask_before_delete={}",
            config.security.ask_before_install, config.security.ask_before_delete
        ),
    }
}
