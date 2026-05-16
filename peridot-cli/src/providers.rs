use super::*;
use crate::commands::read_managed_env_var;

pub(super) async fn live_provider(
    config: &PeridotConfig,
    model: &str,
    project_root: &Path,
) -> Result<Box<dyn LlmProvider>> {
    match config.auth.primary.as_str() {
        "claude-api" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .or_else(|| read_stored_api_key(AuthProvider::ClaudeApi).ok().flatten())
                .with_context(
                    || "ANTHROPIC_API_KEY or peridot login claude-api is required for live runs",
                )?;
            Ok(Box::new(ClaudeProvider::with_transport_options(
                model.to_string(),
                Some(api_key),
                config.api.base_url.clone(),
                config.api.timeout_seconds,
                config.api.max_retries,
            )))
        }
        "openai-api" => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .ok()
                .or_else(|| read_stored_api_key(AuthProvider::OpenaiApi).ok().flatten())
                .with_context(
                    || "OPENAI_API_KEY or peridot login openai-api is required for live runs",
                )?;
            let base_url = if config.api.base_url == "https://api.anthropic.com" {
                "https://api.openai.com".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiProvider::with_transport_options(
                model.to_string(),
                Some(api_key),
                base_url,
                AuthMethod::ApiKey,
                config.api.timeout_seconds,
                config.api.max_retries,
            )))
        }
        "openrouter-api" => {
            let api_key = std::env::var("OPENROUTER_API_KEY")
                .ok()
                .or_else(|| read_managed_env_var("OPENROUTER_API_KEY").ok().flatten())
                .with_context(
                    || {
                        "OPENROUTER_API_KEY, peridot env set OPENROUTER_API_KEY, or peridot login openrouter-api is required for live runs"
                    },
                )?;
            let base_url = if config.api.base_url == "https://api.anthropic.com" {
                "https://openrouter.ai/api".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiProvider::with_transport_options(
                model.to_string(),
                Some(api_key),
                base_url,
                AuthMethod::ApiKey,
                config.api.timeout_seconds,
                config.api.max_retries,
            )))
        }
        "openai-oauth" => {
            let access_token = match std::env::var("OPENAI_ACCESS_TOKEN").ok() {
                Some(access_token) => Some(access_token),
                None => read_stored_openai_oauth_access_token().await?,
            }
            .with_context(
                || "OPENAI_ACCESS_TOKEN or peridot login openai-oauth is required for live runs",
            )?;
            let base_url = if config.api.base_url == "https://api.anthropic.com" {
                "https://api.openai.com".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiProvider::with_transport_options(
                model.to_string(),
                Some(access_token),
                base_url,
                AuthMethod::OAuth,
                config.api.timeout_seconds,
                config.api.max_retries,
            )))
        }
        "codex" => {
            let command =
                std::env::var("PERIDOT_CODEX_COMMAND").unwrap_or_else(|_| "codex".to_string());
            Ok(Box::new(CodexAppServerProvider::with_command(
                model.to_string(),
                project_root.to_path_buf(),
                command,
                config.api.timeout_seconds,
            )))
        }
        provider => anyhow::bail!(
            "live provider {provider} is not implemented yet; use claude-api, openai-api, openrouter-api, codex, openai-oauth, or --mock-response-file for deterministic replay"
        ),
    }
}

pub(super) struct FileMockProvider {
    responses: std::sync::Mutex<Vec<String>>,
}

impl FileMockProvider {
    pub(super) fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let responses = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .rev()
            .collect();
        Ok(Self {
            responses: std::sync::Mutex::new(responses),
        })
    }
}

pub(super) fn parse_mock_completion_response(line: String) -> PeriResult<CompletionResponse> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
        return Ok(CompletionResponse {
            text: line,
            tool_calls: Vec::new(),
            usage: Usage::default(),
        });
    };
    let Some(object) = value.as_object() else {
        return Ok(CompletionResponse {
            text: line,
            tool_calls: Vec::new(),
            usage: Usage::default(),
        });
    };
    let Some(text) = object.get("text").and_then(serde_json::Value::as_str) else {
        return Ok(CompletionResponse {
            text: line,
            tool_calls: Vec::new(),
            usage: Usage::default(),
        });
    };
    let usage = object
        .get("usage")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|err| PeriError::Provider(format!("invalid mock response usage: {err}")))?
        .unwrap_or_default();
    Ok(CompletionResponse {
        text: text.to_string(),
        tool_calls: Vec::new(),
        usage,
    })
}

#[async_trait]
impl LlmProvider for FileMockProvider {
    async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let text = self
            .responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| PeriError::Provider("mock response file exhausted".to_string()))?;
        parse_mock_completion_response(text)
    }

    fn supports_cache(&self) -> bool {
        false
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        false
    }

    fn pricing(&self) -> PricingTable {
        PricingTable::default()
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::NotConfigured
    }
}
