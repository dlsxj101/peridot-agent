use super::*;
use crate::commands::{
    OpenAiOAuthCredentials, openai_oauth_access_token_identity, read_managed_env_var,
    read_stored_openai_oauth_credentials,
};

pub(crate) async fn live_provider(
    config: &PeridotConfig,
    model: &str,
    _project_root: &Path,
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
            let credentials = match std::env::var("OPENAI_ACCESS_TOKEN").ok() {
                Some(access_token) => {
                    let identity = openai_oauth_access_token_identity(&access_token);
                    OpenAiOAuthCredentials {
                        access_token,
                        account_id: std::env::var("OPENAI_CODEX_ACCOUNT_ID")
                            .ok()
                            .or(identity.account_id),
                    }
                }
                None => read_stored_openai_oauth_credentials().await?.with_context(|| {
                    "OPENAI_ACCESS_TOKEN or peridot login openai-oauth is required for live runs"
                })?,
            };
            let account_id = credentials.account_id.with_context(|| {
                "OpenAI Codex OAuth token does not include chatgpt_account_id; rerun `peridot login openai-oauth`"
            })?;
            let base_url = if matches!(
                config.api.base_url.trim_end_matches('/'),
                "https://api.anthropic.com"
                    | "https://api.openai.com"
                    | "https://api.openai.com/v1"
            ) {
                "https://chatgpt.com/backend-api/codex".to_string()
            } else {
                config.api.base_url.clone()
            };
            Ok(Box::new(OpenAiCodexProvider::with_transport_options(
                model.to_string(),
                credentials.access_token,
                account_id,
                base_url,
                config.api.timeout_seconds,
                config.api.max_retries,
            )))
        }
        provider => anyhow::bail!(
            "live provider {provider} is not implemented yet; use claude-api, openai-api, openrouter-api, openai-oauth, or --mock-response-file for deterministic replay"
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
            reasoning_content: None,
            usage: Usage::default(),
        });
    };
    let Some(object) = value.as_object() else {
        return Ok(CompletionResponse {
            text: line,
            tool_calls: Vec::new(),
            reasoning_content: None,
            usage: Usage::default(),
        });
    };
    let usage_from = |obj: &serde_json::Map<String, serde_json::Value>| -> PeriResult<Usage> {
        obj.get("usage")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|err| PeriError::Provider(format!("invalid mock response usage: {err}")))
            .map(Option::unwrap_or_default)
    };
    // Top-level peridot action: {"action": "...", "parameters": {...}}
    // Previously the harness parsed this from response text. Since the
    // native-tool-call refactor, the LLM is expected to emit tool_calls
    // directly; the mock provider mirrors that contract so the fixtures
    // do not need to be rewritten.
    if let (Some(action), Some(parameters)) = (
        object.get("action").and_then(serde_json::Value::as_str),
        object.get("parameters"),
    ) {
        let usage = usage_from(object)?;
        return Ok(CompletionResponse {
            text: String::new(),
            tool_calls: vec![peridot_llm::ToolInvocation {
                id: format!("mock_{action}"),
                name: action.to_string(),
                arguments: parameters.clone(),
            }],
            reasoning_content: None,
            usage,
        });
    }
    let Some(text) = object.get("text").and_then(serde_json::Value::as_str) else {
        return Ok(CompletionResponse {
            text: line,
            tool_calls: Vec::new(),
            reasoning_content: None,
            usage: Usage::default(),
        });
    };
    let usage = usage_from(object)?;
    // Text envelope whose inner text is itself a peridot action JSON
    // (used by budget tests so usage rides alongside a tool call).
    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(text)
        && let Some(inner_obj) = inner.as_object()
        && let (Some(action), Some(parameters)) = (
            inner_obj.get("action").and_then(serde_json::Value::as_str),
            inner_obj.get("parameters"),
        )
    {
        return Ok(CompletionResponse {
            text: String::new(),
            tool_calls: vec![peridot_llm::ToolInvocation {
                id: format!("mock_{action}"),
                name: action.to_string(),
                arguments: parameters.clone(),
            }],
            reasoning_content: None,
            usage,
        });
    }
    Ok(CompletionResponse {
        text: text.to_string(),
        tool_calls: Vec::new(),
        reasoning_content: None,
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
