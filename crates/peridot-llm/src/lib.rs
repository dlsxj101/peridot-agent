//! LLM provider contracts and live provider implementations.

mod anthropic;
pub mod catalog;
mod models;
mod openai;
mod openai_codex;
mod provider;
mod transport;
mod types;

pub use anthropic::ClaudeProvider;
pub use models::context_window_tokens;
pub use openai::OpenAiProvider;
pub use openai_codex::OpenAiCodexProvider;
pub use provider::{LlmProvider, PricingTable};
pub use types::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmMessage,
    MessageRole, ToolChoice, ToolDefinition, ToolInvocation, Usage,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::{
        anthropic_payload, anthropic_payload_with_cache, anthropic_stream_payload,
        parse_anthropic_response, parse_anthropic_stream,
    };
    use crate::openai::{
        openai_chat_payload, openai_stream_payload, parse_openai_response, parse_openai_stream,
    };
    use crate::openai_codex::{
        codex_model_and_service_tier, openai_codex_payload, openai_codex_responses_url,
    };
    use crate::transport::should_retry_status;
    use async_trait::async_trait;
    use peridot_common::PeriResult;
    use serde_json::json;
    use std::io::{Read, Write};

    fn request(messages: Vec<LlmMessage>) -> CompletionRequest {
        CompletionRequest {
            model: "mock".to_string(),
            system: None,
            messages,
            max_tokens: Some(16),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        }
    }

    fn tool_request(messages: Vec<LlmMessage>, tools: Vec<ToolDefinition>) -> CompletionRequest {
        CompletionRequest {
            model: "mock".to_string(),
            system: None,
            messages,
            max_tokens: Some(64),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools,
            tool_choice: ToolChoice::Auto,
        }
    }

    fn file_read_tool() -> ToolDefinition {
        ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }
    }

    fn assert_json_snapshot(value: &serde_json::Value, expected: &str) {
        let actual = serde_json::to_string_pretty(value).unwrap();
        assert_eq!(actual, expected.trim());
    }

    #[derive(Clone, Debug)]
    struct StaticProvider;

    #[async_trait]
    impl LlmProvider for StaticProvider {
        async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
            Ok(CompletionResponse {
                text: "hello".to_string(),
                tool_calls: Vec::new(),
                reasoning_content: None,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    reasoning_output_tokens: 0,
                    estimated_cost_usd: 0.01,
                },
            })
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

    #[tokio::test]
    async fn default_stream_returns_single_done_chunk() {
        let provider = StaticProvider;
        let chunks = provider
            .stream(request(vec![LlmMessage::new(MessageRole::User, "hello")]))
            .await
            .unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].delta, "hello");
        assert!(chunks[0].done);
        assert_eq!(chunks[0].usage.unwrap().output_tokens, 2);
    }

    #[test]
    fn anthropic_payload_moves_system_to_top_level() {
        let payload = anthropic_payload(&CompletionRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            system: Some("top".to_string()),
            messages: vec![
                LlmMessage::new(MessageRole::System, "inline"),
                LlmMessage::new(MessageRole::User, "hello"),
            ],
            max_tokens: Some(128),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        });

        assert_eq!(payload["system"], "top\n\ninline");
        assert_eq!(payload["messages"][0]["role"], "user");
        assert!(payload.get("tools").is_none());
    }

    #[test]
    fn anthropic_payload_with_cache_marks_three_breakpoints() {
        // System + tools + a multi-turn conversation so all three
        // breakpoints (tools, system, history) can be exercised.
        let tool = ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let assistant = LlmMessage::assistant_with_tool_calls(
            "Reading.",
            vec![ToolInvocation {
                id: "toolu_1".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "README.md"}),
            }],
        );
        let request = CompletionRequest {
            model: "claude-sonnet-4-6".to_string(),
            system: Some("you are peridot".to_string()),
            messages: vec![
                LlmMessage::new(MessageRole::User, "first prompt"),
                assistant,
                LlmMessage::tool_result("toolu_1", "# Peridot"),
                LlmMessage::new(MessageRole::User, "trailing user prompt"),
            ],
            max_tokens: Some(128),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: vec![tool],
            tool_choice: ToolChoice::Auto,
        };
        let payload = anthropic_payload_with_cache(&request, true);

        // Breakpoint 1: last tool definition carries cache_control.
        let tools = payload["tools"].as_array().expect("tools present");
        assert_eq!(
            tools.last().unwrap()["cache_control"],
            json!({ "type": "ephemeral" })
        );

        // Breakpoint 2: system rendered as a single block with cache_control.
        let system_blocks = payload["system"].as_array().expect("system as array");
        assert_eq!(system_blocks.len(), 1);
        assert_eq!(
            system_blocks[0]["cache_control"],
            json!({ "type": "ephemeral" })
        );
        assert_eq!(system_blocks[0]["text"], "you are peridot");

        // Breakpoint 3: cache_control on the tool_result content block (the
        // entry just before the trailing user prompt). Confirms the trailing
        // user turn itself is NOT marked, keeping new prompts off-cache.
        let messages = payload["messages"].as_array().expect("messages present");
        assert_eq!(messages.last().unwrap()["role"], "user");
        assert!(
            messages.last().unwrap()["content"].is_string(),
            "trailing user prompt must remain unmarked plain string"
        );
        let tool_result_msg = &messages[messages.len() - 2];
        let tool_result_blocks = tool_result_msg["content"]
            .as_array()
            .expect("tool_result content is a block array");
        assert_eq!(tool_result_blocks[0]["type"], "tool_result");
        assert_eq!(
            tool_result_blocks
                .last()
                .and_then(|block| block.get("cache_control")),
            Some(&json!({ "type": "ephemeral" }))
        );
    }

    #[test]
    fn anthropic_payload_skips_cache_when_provider_disables() {
        // cache_enabled = false must produce the exact legacy wire shape:
        // system is a plain string, tools/history have no cache_control.
        let tool = ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let request = CompletionRequest {
            model: "claude-sonnet-4-6".to_string(),
            system: Some("plain system".to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, "hi")],
            max_tokens: Some(128),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: vec![tool],
            tool_choice: ToolChoice::Auto,
        };
        let payload = anthropic_payload_with_cache(&request, false);

        assert_eq!(payload["system"], "plain system");
        let tools = payload["tools"].as_array().unwrap();
        assert!(tools.last().unwrap().get("cache_control").is_none());
        // legacy default also unchanged for callers that call anthropic_payload directly.
        let legacy = anthropic_payload(&request);
        assert_eq!(legacy, payload);
    }

    #[test]
    fn anthropic_payload_emits_tools_when_provided() {
        let tool = ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let payload = anthropic_payload(&tool_request(
            vec![LlmMessage::new(MessageRole::User, "read README")],
            vec![tool],
        ));
        assert_eq!(payload["tools"][0]["name"], "file_read");
        assert_eq!(payload["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(payload["tool_choice"]["type"], "auto");
    }

    #[test]
    fn providers_store_transport_retry_options() {
        let claude = ClaudeProvider::with_transport_options(
            "claude-sonnet-4-20250514",
            Some("key".to_string()),
            "https://api.anthropic.com",
            5,
            7,
        );
        let openai = OpenAiProvider::with_transport_options(
            "gpt-5.2",
            Some("key".to_string()),
            "https://api.openai.com",
            AuthMethod::ApiKey,
            6,
            8,
        );

        assert_eq!(claude.max_retries(), 7);
        assert_eq!(openai.max_retries(), 8);
    }

    #[test]
    fn retry_status_only_includes_transient_failures() {
        assert!(should_retry_status(reqwest::StatusCode::REQUEST_TIMEOUT));
        assert!(should_retry_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(should_retry_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!should_retry_status(reqwest::StatusCode::BAD_REQUEST));
    }

    #[test]
    fn openai_payload_uses_chat_completions_shape() {
        let payload = openai_chat_payload(&CompletionRequest {
            model: "gpt-5.2".to_string(),
            system: Some("system".to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, "hello")],
            max_tokens: Some(256),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        });

        assert_eq!(payload["model"], "gpt-5.2");
        assert_eq!(payload["max_tokens"], 256);
        assert_eq!(payload["messages"][0]["role"], "system");
        assert_eq!(payload["messages"][0]["content"], "system");
        assert_eq!(payload["messages"][1]["role"], "user");
        assert!(payload.get("tools").is_none());
    }

    #[test]
    fn openai_codex_payload_uses_responses_shape() {
        let tool = ToolDefinition {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        };
        let payload = openai_codex_payload(&CompletionRequest {
            model: "gpt-5.4".to_string(),
            system: Some("system".to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, "hello")],
            max_tokens: Some(256),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Medium,
            service_tier: None,
            tools: vec![tool],
            tool_choice: ToolChoice::Auto,
        });

        assert_eq!(payload["model"], "gpt-5.4");
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["instructions"], "system");
        assert!(payload.get("max_output_tokens").is_none());
        assert_eq!(payload["parallel_tool_calls"], false);
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(payload["tools"][0]["type"], "function");
        assert_eq!(payload["reasoning"]["effort"], "medium");
        assert_eq!(payload["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn openai_codex_endpoint_normalizes_backend_urls() {
        assert_eq!(
            openai_codex_responses_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            openai_codex_responses_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            openai_codex_responses_url("https://chatgpt.com/backend-api/codex/responses"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn openai_payload_emits_native_tool_calls_and_tool_messages() {
        // Round-trip an assistant turn that carries tool calls plus the matching
        // tool result so the wire format mirrors the OpenAI canonical protocol.
        let assistant = LlmMessage::assistant_with_tool_calls(
            "Reading the file now.",
            vec![ToolInvocation {
                id: "call_abc".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "README.md"}),
            }],
        );
        let tool = LlmMessage::tool_result("call_abc", "# Peridot");
        let payload = openai_chat_payload(&CompletionRequest {
            model: "gpt-5.2".to_string(),
            system: None,
            messages: vec![
                LlmMessage::new(MessageRole::User, "read README"),
                assistant,
                tool,
                LlmMessage::new(MessageRole::User, "summarise it"),
            ],
            max_tokens: Some(128),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        });

        assert_eq!(payload["messages"][0]["role"], "user");
        assert_eq!(payload["messages"][1]["role"], "assistant");
        assert_eq!(payload["messages"][1]["content"], "Reading the file now.");
        assert_eq!(payload["messages"][1]["tool_calls"][0]["id"], "call_abc");
        assert_eq!(
            payload["messages"][1]["tool_calls"][0]["function"]["name"],
            "file_read"
        );
        // OpenAI requires arguments as a JSON-encoded string, not an object.
        assert_eq!(
            payload["messages"][1]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"README.md\"}"
        );
        assert_eq!(payload["messages"][2]["role"], "tool");
        assert_eq!(payload["messages"][2]["tool_call_id"], "call_abc");
        assert_eq!(payload["messages"][2]["content"], "# Peridot");
        assert_eq!(payload["messages"][3]["role"], "user");
    }

    #[test]
    fn openai_payload_emits_null_content_for_pure_tool_call_assistant() {
        // OpenAI's validator rejects `{role: assistant, content: "", tool_calls:
        // [...]}`. When the model returned only tool calls we must emit
        // `content: null` instead of the empty string.
        let assistant = LlmMessage::assistant_with_tool_calls(
            "",
            vec![ToolInvocation {
                id: "call_x".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "."}),
            }],
        );
        let payload = openai_chat_payload(&CompletionRequest {
            model: "gpt-5.2".to_string(),
            system: None,
            messages: vec![assistant],
            max_tokens: None,
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        });
        assert!(payload["messages"][0]["content"].is_null());
        assert_eq!(payload["messages"][0]["tool_calls"][0]["id"], "call_x");
    }

    #[test]
    fn anthropic_payload_emits_tool_use_and_tool_result_blocks() {
        let assistant = LlmMessage::assistant_with_tool_calls(
            "Reading the file.",
            vec![ToolInvocation {
                id: "toolu_1".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "README.md"}),
            }],
        );
        let tool = LlmMessage::tool_result("toolu_1", "# Peridot");
        let payload = anthropic_payload(&CompletionRequest {
            model: "claude-sonnet-4-6".to_string(),
            system: None,
            messages: vec![
                LlmMessage::new(MessageRole::User, "read README"),
                assistant,
                tool,
            ],
            max_tokens: Some(128),
            thinking: false,
            reasoning_effort: peridot_common::ReasoningEffort::Off,
            service_tier: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        });

        assert_eq!(payload["messages"][1]["role"], "assistant");
        // Anthropic uses content blocks: a text block followed by a tool_use block.
        assert_eq!(payload["messages"][1]["content"][0]["type"], "text");
        assert_eq!(
            payload["messages"][1]["content"][0]["text"],
            "Reading the file."
        );
        assert_eq!(payload["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(payload["messages"][1]["content"][1]["id"], "toolu_1");
        assert_eq!(
            payload["messages"][1]["content"][1]["input"]["path"],
            "README.md"
        );
        // Tool result goes back on a user turn as a tool_result content block.
        assert_eq!(payload["messages"][2]["role"], "user");
        assert_eq!(payload["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            payload["messages"][2]["content"][0]["tool_use_id"],
            "toolu_1"
        );
        assert_eq!(payload["messages"][2]["content"][0]["content"], "# Peridot");
    }

    #[test]
    fn openai_payload_emits_tools_when_provided() {
        let payload = openai_chat_payload(&tool_request(
            vec![LlmMessage::new(MessageRole::User, "read README")],
            vec![file_read_tool()],
        ));
        assert_eq!(payload["tools"][0]["type"], "function");
        assert_eq!(payload["tools"][0]["function"]["name"], "file_read");
        assert_eq!(payload["tool_choice"], "auto");
    }

    #[test]
    fn openai_api_payload_snapshot_covers_reasoning_tools_and_streaming() {
        let mut request = tool_request(
            vec![LlmMessage::new(MessageRole::User, "read README")],
            vec![file_read_tool()],
        );
        request.model = "gpt-5.5".to_string();
        request.system = Some("system".to_string());
        request.max_tokens = Some(256);
        request.reasoning_effort = peridot_common::ReasoningEffort::High;
        request.tool_choice = ToolChoice::Required;

        assert_json_snapshot(
            &openai_stream_payload(&request),
            r##"
{
  "max_tokens": 256,
  "messages": [
    {
      "content": "system",
      "role": "system"
    },
    {
      "content": "read README",
      "role": "user"
    }
  ],
  "model": "gpt-5.5",
  "reasoning": {
    "effort": "high"
  },
  "stream": true,
  "stream_options": {
    "include_usage": true
  },
  "tool_choice": "required",
  "tools": [
    {
      "function": {
        "description": "Read a file",
        "name": "file_read",
        "parameters": {
          "properties": {
            "path": {
              "type": "string"
            }
          },
          "required": [
            "path"
          ],
          "type": "object"
        }
      },
      "type": "function"
    }
  ]
}
"##,
        );
    }

    #[test]
    fn openai_payloads_forward_xhigh_reasoning_effort() {
        let mut request = request(vec![LlmMessage::new(MessageRole::User, "solve hard bug")]);
        request.model = "gpt-5.5".to_string();
        request.reasoning_effort = peridot_common::ReasoningEffort::XHigh;

        assert_eq!(
            openai_stream_payload(&request)["reasoning"]["effort"],
            "xhigh"
        );
        assert_eq!(
            openai_codex_payload(&request)["reasoning"]["effort"],
            "xhigh"
        );
    }

    #[test]
    fn openai_codex_oauth_payload_snapshot_covers_responses_tool_linkage() {
        let assistant = LlmMessage::assistant_with_tool_calls(
            "Reading.",
            vec![ToolInvocation {
                id: "call_1".to_string(),
                name: "file_read".to_string(),
                arguments: json!({"path": "README.md"}),
            }],
        );
        let mut request = tool_request(
            vec![
                LlmMessage::new(MessageRole::User, "read README"),
                assistant,
                LlmMessage::tool_result("call_1", "# Peridot"),
                LlmMessage::new(MessageRole::User, "summarize"),
            ],
            vec![file_read_tool()],
        );
        request.model = "gpt-5.5".to_string();
        request.system = Some("system".to_string());
        request.reasoning_effort = peridot_common::ReasoningEffort::Medium;

        let payload = openai_codex_payload(&request);
        assert!(payload.get("max_output_tokens").is_none());
        assert_json_snapshot(
            &payload,
            r##"
{
  "include": [
    "reasoning.encrypted_content"
  ],
  "input": [
    {
      "content": [
        {
          "text": "read README",
          "type": "input_text"
        }
      ],
      "role": "user",
      "type": "message"
    },
    {
      "content": [
        {
          "text": "Reading.",
          "type": "output_text"
        }
      ],
      "role": "assistant",
      "type": "message"
    },
    {
      "arguments": "{\"path\":\"README.md\"}",
      "call_id": "call_1",
      "name": "file_read",
      "type": "function_call"
    },
    {
      "call_id": "call_1",
      "output": "# Peridot",
      "type": "function_call_output"
    },
    {
      "content": [
        {
          "text": "summarize",
          "type": "input_text"
        }
      ],
      "role": "user",
      "type": "message"
    }
  ],
  "instructions": "system",
  "model": "gpt-5.5",
  "parallel_tool_calls": false,
  "reasoning": {
    "effort": "medium",
    "summary": "auto"
  },
  "store": false,
  "stream": true,
  "text": {
    "verbosity": "low"
  },
  "tool_choice": "auto",
  "tools": [
    {
      "description": "Read a file",
      "name": "file_read",
      "parameters": {
        "properties": {
          "path": {
            "type": "string"
          }
        },
        "required": [
          "path"
        ],
        "type": "object"
      },
      "type": "function"
    }
  ]
}
"##,
        );
    }

    #[test]
    fn openai_codex_fast_model_alias_sets_priority_service_tier() {
        let mut request = request(vec![LlmMessage::new(MessageRole::User, "hi")]);
        request.model = "gpt-5.5-fast".to_string();

        let payload = openai_codex_payload(&request);

        assert_eq!(payload["model"], "gpt-5.5");
        assert_eq!(payload["service_tier"], "priority");
        assert_eq!(
            codex_model_and_service_tier("gpt-5.5", Some("fast")),
            ("gpt-5.5".to_string(), Some("priority"))
        );
    }

    #[test]
    fn anthropic_payload_snapshot_covers_thinking_tools_and_streaming() {
        let mut request = tool_request(
            vec![LlmMessage::new(MessageRole::User, "read README")],
            vec![file_read_tool()],
        );
        request.model = "claude-sonnet-4-6".to_string();
        request.system = Some("system".to_string());
        request.max_tokens = Some(512);
        request.reasoning_effort = peridot_common::ReasoningEffort::Low;
        request.tool_choice = ToolChoice::None;

        assert_json_snapshot(
            &anthropic_stream_payload(&request),
            r#"
{
  "max_tokens": 512,
  "messages": [
    {
      "content": "read README",
      "role": "user"
    }
  ],
  "model": "claude-sonnet-4-6",
  "stream": true,
  "system": "system",
  "thinking": {
    "budget_tokens": 1024,
    "type": "enabled"
  },
  "tool_choice": {
    "type": "none"
  },
  "tools": [
    {
      "description": "Read a file",
      "input_schema": {
        "properties": {
          "path": {
            "type": "string"
          }
        },
        "required": [
          "path"
        ],
        "type": "object"
      },
      "name": "file_read"
    }
  ]
}
"#,
        );
    }

    #[tokio::test]
    async fn openai_provider_posts_to_chat_completions_endpoint() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]);

            assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
            assert!(request.contains("authorization: Bearer test-key"));
            assert!(request.contains("\"model\":\"test-model\""));

            let body = r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let provider = OpenAiProvider::with_transport_options(
            "test-model",
            Some("test-key".to_string()),
            format!("http://{address}"),
            AuthMethod::ApiKey,
            5,
            0,
        );

        let response = provider
            .complete(CompletionRequest {
                model: "test-model".to_string(),
                system: None,
                messages: vec![LlmMessage::new(MessageRole::User, "hello")],
                max_tokens: Some(16),
                thinking: false,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                tools: Vec::new(),
                tool_choice: ToolChoice::Auto,
            })
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(response.text, "ok");
        assert_eq!(response.usage.output_tokens, 2);
    }

    #[tokio::test]
    async fn openai_compatible_stream_uses_openrouter_api_prefix() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]);

            assert!(request.starts_with("POST /api/v1/chat/completions HTTP/1.1"));
            assert!(request.contains("authorization: Bearer openrouter-key"));
            assert!(request.contains("accept: text/event-stream"));
            assert!(request.contains("\"stream\":true"));
            assert!(request.contains("\"stream_options\":{\"include_usage\":true}"));

            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                "data: {\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let provider = OpenAiProvider::with_transport_options(
            "openai/gpt-5.5",
            Some("openrouter-key".to_string()),
            format!("http://{address}/api"),
            AuthMethod::ApiKey,
            5,
            0,
        );

        let chunks = provider
            .stream(CompletionRequest {
                model: "openai/gpt-5.5".to_string(),
                system: None,
                messages: vec![LlmMessage::new(MessageRole::User, "hello")],
                max_tokens: Some(16),
                thinking: false,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                tools: Vec::new(),
                tool_choice: ToolChoice::Auto,
            })
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(chunks[0].delta, "ok");
        assert_eq!(chunks.last().unwrap().usage.unwrap().output_tokens, 2);
    }

    #[tokio::test]
    async fn openai_codex_oauth_stream_sends_required_headers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]);

            assert!(request.starts_with("POST /backend-api/codex/responses HTTP/1.1"));
            assert!(request.contains("authorization: Bearer oauth-token"));
            assert!(request.contains("chatgpt-account-id: account-1"));
            assert!(request.contains("originator: peridot"));
            assert!(request.contains("openai-beta: responses=experimental"));
            assert!(request.contains("\"parallel_tool_calls\":false"));
            assert!(request.contains("\"include\":[\"reasoning.encrypted_content\"]"));
            assert!(!request.contains("max_output_tokens"));

            let body = concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2},\"output\":[]}}\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let provider = OpenAiCodexProvider::with_transport_options(
            "gpt-5.5",
            "oauth-token",
            "account-1",
            format!("http://{address}/backend-api/codex"),
            5,
            0,
        );

        let chunks = provider
            .stream(CompletionRequest {
                model: "gpt-5.5".to_string(),
                system: None,
                messages: vec![LlmMessage::new(MessageRole::User, "hello")],
                max_tokens: Some(16),
                thinking: false,
                reasoning_effort: peridot_common::ReasoningEffort::Off,
                service_tier: None,
                tools: Vec::new(),
                tool_choice: ToolChoice::Auto,
            })
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(chunks[0].delta, "ok");
        assert_eq!(chunks.last().unwrap().usage.unwrap().output_tokens, 2);
    }

    #[test]
    fn parses_anthropic_usage_and_text() {
        let response = parse_anthropic_response(
            r#"{
                "content":[{"type":"text","text":"hello"}],
                "usage":{
                    "input_tokens":10,
                    "cache_creation_input_tokens":2,
                    "cache_read_input_tokens":3,
                    "output_tokens":4
                }
            }"#,
            PricingTable {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: 0.30,
            },
        )
        .unwrap();

        assert_eq!(response.text, "hello");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.cache_creation_tokens, 2);
        assert!(response.usage.estimated_cost_usd > 0.0);
    }

    #[test]
    fn parses_anthropic_tool_use_blocks() {
        let response = parse_anthropic_response(
            r#"{
                "content":[
                    {"type":"text","text":"calling tool"},
                    {"type":"tool_use","id":"toolu_1","name":"file_read","input":{"path":"README.md"}}
                ],
                "usage":{"input_tokens":1,"output_tokens":2}
            }"#,
            PricingTable::default(),
        )
        .unwrap();
        assert_eq!(response.text, "calling tool");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "toolu_1");
        assert_eq!(response.tool_calls[0].name, "file_read");
        assert_eq!(response.tool_calls[0].arguments["path"], "README.md");
    }

    #[test]
    fn parses_anthropic_stream_chunks_and_usage() {
        let chunks = parse_anthropic_stream(
            r#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}

event: message_delta
data: {"type":"message_delta","usage":{"output_tokens":4}}

event: message_stop
data: {"type":"message_stop"}
"#,
            PricingTable {
                input_per_million: 3.0,
                output_per_million: 15.0,
                cache_read_per_million: 0.30,
            },
        )
        .unwrap();

        assert_eq!(chunks[0].delta, "hel");
        assert_eq!(chunks[1].delta, "lo");
        assert!(chunks.last().unwrap().done);
        let usage = chunks.last().unwrap().usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.cache_creation_tokens, 2);
    }

    #[test]
    fn parses_anthropic_stream_tool_use_blocks() {
        let chunks = parse_anthropic_stream(
            r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_2","name":"file_read"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":":\"README.md\"}"}}

event: message_stop
data: {"type":"message_stop"}
"#,
            PricingTable::default(),
        )
        .unwrap();
        let last = chunks.last().unwrap();
        assert!(last.done);
        assert_eq!(last.tool_calls.len(), 1);
        assert_eq!(last.tool_calls[0].id, "toolu_2");
        assert_eq!(last.tool_calls[0].name, "file_read");
        assert_eq!(last.tool_calls[0].arguments["path"], "README.md");
    }

    #[test]
    fn parses_openai_response_text_and_tool_calls() {
        let response = parse_openai_response(
            r#"{
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "ok",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {"name": "file_read", "arguments": "{\"path\":\"README.md\"}"}
                        }]
                    }
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "prompt_tokens_details": {"cached_tokens": 2},
                    "completion_tokens_details": {"reasoning_tokens": 3}
                }
            }"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(response.text, "ok");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_1");
        assert_eq!(response.tool_calls[0].name, "file_read");
        assert_eq!(response.tool_calls[0].arguments["path"], "README.md");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.cache_read_tokens, 2);
        assert_eq!(response.usage.reasoning_output_tokens, 3);
    }

    #[test]
    fn parses_openai_stream_chunks_and_usage() {
        let chunks = parse_openai_stream(
            r#"data: {"choices":[{"delta":{"content":"hel"}}]}

data: {"choices":[{"delta":{"content":"lo"}}]}

data: {"usage":{"prompt_tokens":10,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":2},"completion_tokens_details":{"reasoning_tokens":3}}}

data: [DONE]
"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(chunks[0].delta, "hel");
        assert_eq!(chunks[1].delta, "lo");
        let last = chunks.last().unwrap();
        assert!(last.done);
        let usage = last.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 2);
        assert_eq!(usage.reasoning_output_tokens, 3);
    }

    #[test]
    fn parses_openai_stream_tool_call_deltas() {
        let chunks = parse_openai_stream(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_2","function":{"name":"file_read","arguments":"{\"path\":\""}}]}}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"README.md\"}"}}]}}]}

data: [DONE]
"#,
            PricingTable::default(),
        )
        .unwrap();
        let last = chunks.last().unwrap();
        assert!(last.done);
        assert_eq!(last.tool_calls.len(), 1);
        assert_eq!(last.tool_calls[0].id, "call_2");
        assert_eq!(last.tool_calls[0].name, "file_read");
        assert_eq!(last.tool_calls[0].arguments["path"], "README.md");
    }
}
