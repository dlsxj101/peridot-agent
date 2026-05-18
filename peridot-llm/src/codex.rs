use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

use crate::{
    AuthMethod, CompletionRequest, CompletionResponse, LlmMessage, LlmProvider, MessageRole,
    PricingTable, Usage,
};

/// OpenAI Codex provider backed by the local `codex app-server` runtime.
pub struct CodexAppServerProvider {
    model: String,
    command: String,
    cwd: PathBuf,
    timeout_seconds: u64,
}

impl CodexAppServerProvider {
    /// Creates a Codex app-server provider using the `codex` binary on PATH.
    pub fn new(model: impl Into<String>, cwd: impl Into<PathBuf>, timeout_seconds: u64) -> Self {
        Self::with_command(model, cwd, "codex", timeout_seconds)
    }

    /// Creates a Codex app-server provider with an explicit command.
    pub fn with_command(
        model: impl Into<String>,
        cwd: impl Into<PathBuf>,
        command: impl Into<String>,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            model: model.into(),
            command: command.into(),
            cwd: cwd.into(),
            timeout_seconds: timeout_seconds.max(10),
        }
    }

    /// Returns the command used to launch Codex.
    pub fn command(&self) -> &str {
        &self.command
    }
}

#[async_trait]
impl LlmProvider for CodexAppServerProvider {
    async fn complete(&self, request: CompletionRequest) -> PeriResult<CompletionResponse> {
        let model = if request.model.trim().is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };
        run_codex_turn(
            &self.command,
            &self.cwd,
            &model,
            request,
            self.timeout_seconds,
        )
        .await
    }

    fn supports_cache(&self) -> bool {
        true
    }

    fn supports_prefill(&self) -> bool {
        false
    }

    fn supports_thinking(&self) -> bool {
        true
    }

    fn pricing(&self) -> PricingTable {
        PricingTable::default()
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::OAuth
    }
}

async fn run_codex_turn(
    command: &str,
    cwd: &Path,
    model: &str,
    request: CompletionRequest,
    timeout_seconds: u64,
) -> PeriResult<CompletionResponse> {
    let mut session = CodexRpcSession::spawn(command).await?;
    let result = timeout(
        Duration::from_secs(timeout_seconds),
        session.complete(cwd, model, request),
    )
    .await
    .map_err(|_| PeriError::Provider("codex app-server turn timed out".to_string()))?;
    session.close().await;
    result
}

struct CodexRpcSession {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    stderr_tail: Arc<Mutex<Vec<String>>>,
    next_id: u64,
}

impl CodexRpcSession {
    async fn spawn(command: &str) -> PeriResult<Self> {
        let mut child = Command::new(command)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| {
                PeriError::Provider(format!(
                    "failed to spawn codex app-server with {command:?}: {err}"
                ))
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PeriError::Provider("codex app-server stdin unavailable".to_string()))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PeriError::Provider("codex app-server stdout unavailable".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            PeriError::Provider("codex app-server stderr unavailable".to_string())
        })?;
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_tail_task = Arc::clone(&stderr_tail);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut tail) = stderr_tail_task.lock() {
                    tail.push(line);
                    if tail.len() >= 20 {
                        tail.remove(0);
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
            stderr_tail,
            next_id: 1,
        })
    }

    async fn complete(
        &mut self,
        cwd: &Path,
        model: &str,
        request: CompletionRequest,
    ) -> PeriResult<CompletionResponse> {
        let init_id = self
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "peridot",
                        "title": "Peridot Agent",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {}
                }),
            )
            .await?;
        self.wait_response(init_id).await?;
        self.notify("initialized", json!({})).await?;

        let mut thread_params = json!({ "cwd": cwd.to_string_lossy() });
        if !model.trim().is_empty()
            && let Some(object) = thread_params.as_object_mut()
        {
            object.insert("model".to_string(), Value::String(model.trim().to_string()));
        }
        let thread_id = self.request("thread/start", thread_params).await?;
        let thread_response = self.wait_response(thread_id).await?;
        let thread_id = extract_thread_id(&thread_response).ok_or_else(|| {
            PeriError::Provider(format!(
                "codex app-server thread/start returned no thread id: {thread_response}"
            ))
        })?;

        let prompt = render_codex_prompt(&request);
        let mut turn_params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}]
        });
        if !model.trim().is_empty()
            && let Some(object) = turn_params.as_object_mut()
        {
            object.insert("model".to_string(), Value::String(model.trim().to_string()));
        }
        let turn_request_id = self.request("turn/start", turn_params).await?;
        self.wait_response(turn_request_id).await?;
        self.wait_turn_completed(&thread_id).await
    }

    async fn close(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    async fn request(&mut self, method: &str, params: Value) -> PeriResult<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(json!({ "id": id, "method": method, "params": params }))
            .await?;
        Ok(id)
    }

    async fn notify(&mut self, method: &str, params: Value) -> PeriResult<()> {
        self.write_json(json!({ "method": method, "params": params }))
            .await
    }

    async fn write_json(&mut self, value: Value) -> PeriResult<()> {
        let mut line = serde_json::to_vec(&value)
            .map_err(|err| PeriError::Provider(format!("failed to encode codex RPC: {err}")))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|err| PeriError::Provider(format!("failed to write codex RPC: {err}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|err| PeriError::Provider(format!("failed to flush codex RPC: {err}")))
    }

    async fn read_message(&mut self) -> PeriResult<Value> {
        let Some(line) = self.lines.next_line().await.map_err(|err| {
            PeriError::Provider(format!("failed to read codex app-server stdout: {err}"))
        })?
        else {
            return Err(PeriError::Provider(format!(
                "codex app-server exited before completing the turn{}",
                self.format_stderr_tail()
            )));
        };
        serde_json::from_str::<Value>(&line)
            .map_err(|err| PeriError::Provider(format!("invalid codex app-server JSON: {err}")))
    }

    async fn wait_response(&mut self, id: u64) -> PeriResult<Value> {
        loop {
            let message = self.read_message().await?;
            if message.get("id").and_then(Value::as_u64) == Some(id)
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                if let Some(error) = message.get("error") {
                    return Err(PeriError::Provider(format!(
                        "codex app-server RPC error: {error}{}",
                        self.format_stderr_tail()
                    )));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            self.handle_server_request(&message).await?;
        }
    }

    async fn wait_turn_completed(&mut self, thread_id: &str) -> PeriResult<CompletionResponse> {
        let mut text = String::new();
        let mut usage = Usage::default();
        loop {
            let message = self.read_message().await?;
            self.handle_server_request(&message).await?;
            let method = message.get("method").and_then(Value::as_str).unwrap_or("");
            let params = message.get("params").unwrap_or(&Value::Null);
            match method {
                "item/agentMessage/delta" => {
                    if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                        text.push_str(delta);
                    }
                }
                "item/completed" => {
                    if let Some(item) = params.get("item")
                        && item.get("type").and_then(Value::as_str) == Some("agentMessage")
                    {
                        text = item
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or(&text)
                            .to_string();
                    }
                }
                "thread/tokenUsage/updated" => {
                    if params.get("threadId").and_then(Value::as_str) == Some(thread_id)
                        && let Some(token_usage) = params.get("tokenUsage")
                    {
                        usage = parse_codex_usage(token_usage);
                    }
                }
                "turn/completed" => {
                    return Ok(CompletionResponse {
                        text,
                        tool_calls: Vec::new(),
                        reasoning_content: None,
                        usage,
                    });
                }
                "turn/failed" | "turn/cancelled" => {
                    return Err(PeriError::Provider(format!(
                        "codex app-server {method}: {params}{}",
                        self.format_stderr_tail()
                    )));
                }
                _ => {}
            }
        }
    }

    async fn handle_server_request(&mut self, message: &Value) -> PeriResult<()> {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(());
        };
        if message.get("method").and_then(Value::as_str).is_none()
            || message.get("result").is_some()
            || message.get("error").is_some()
        {
            return Ok(());
        }
        self.write_json(json!({
            "id": id,
            "result": {
                "decision": "decline",
                "text": ""
            }
        }))
        .await
    }

    fn format_stderr_tail(&self) -> String {
        let Ok(tail) = self.stderr_tail.lock() else {
            return String::new();
        };
        if tail.is_empty() {
            String::new()
        } else {
            format!("\ncodex stderr:\n{}", tail.join("\n"))
        }
    }
}

fn render_codex_prompt(request: &CompletionRequest) -> String {
    let mut sections = Vec::new();
    if let Some(system) = request.system.as_deref()
        && !system.trim().is_empty()
    {
        sections.push(format!(
            "System instructions:\n{}\n\nProvider constraint: do not use Codex tools directly. Return exactly one Peridot JSON action object and no markdown.",
            system.trim()
        ));
    }
    sections.push("Conversation transcript:".to_string());
    for message in &request.messages {
        sections.push(format_message(message));
    }
    sections.join("\n\n")
}

fn format_message(message: &LlmMessage) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    format!("{role}:\n{}", message.content)
}

fn extract_thread_id(result: &Value) -> Option<String> {
    result
        .get("thread")
        .and_then(|thread| {
            thread
                .get("id")
                .or_else(|| thread.get("sessionId"))
                .and_then(Value::as_str)
        })
        .or_else(|| result.get("threadId").and_then(Value::as_str))
        .or_else(|| result.get("sessionId").and_then(Value::as_str))
        .map(str::to_string)
}

fn parse_codex_usage(token_usage: &Value) -> Usage {
    let usage = token_usage
        .get("last")
        .or_else(|| token_usage.get("total"))
        .unwrap_or(&Value::Null);
    let input_tokens = usage
        .get("inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("outputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cachedInputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_output_tokens = usage
        .get("reasoningOutputTokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens: 0,
        reasoning_output_tokens,
        estimated_cost_usd: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_codex_thread_id_from_thread_object() {
        let value = json!({"thread": {"id": "thread-1"}});
        assert_eq!(extract_thread_id(&value).as_deref(), Some("thread-1"));
    }

    #[test]
    fn parses_codex_token_usage_last_turn() {
        let usage = parse_codex_usage(&json!({
            "total": {"inputTokens": 1000, "cachedInputTokens": 100, "outputTokens": 50},
            "last": {
                "inputTokens": 20,
                "cachedInputTokens": 3,
                "outputTokens": 4,
                "reasoningOutputTokens": 2
            }
        }));
        assert_eq!(usage.input_tokens, 20);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.reasoning_output_tokens, 2);
    }

    #[test]
    fn codex_prompt_keeps_peridot_json_contract_visible() {
        let prompt = render_codex_prompt(&CompletionRequest {
            model: "gpt-5.5".to_string(),
            system: Some("Respond with JSON containing action and parameters.".to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, "do work")],
            max_tokens: Some(1000),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: crate::ToolChoice::Auto,
        });
        assert!(prompt.contains("Return exactly one Peridot JSON action object"));
        assert!(prompt.contains("user:\ndo work"));
    }
}
