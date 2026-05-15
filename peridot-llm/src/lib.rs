//! LLM provider contracts and live provider implementations.

mod anthropic;
mod openai;
mod parse;
mod provider;
mod transport;
mod types;

pub use anthropic::ClaudeProvider;
pub use openai::OpenAiProvider;
pub use parse::parse_action;
pub use provider::{LlmProvider, PricingTable};
pub use types::{
    AuthMethod, CompletionRequest, CompletionResponse, CompletionStreamChunk, LlmMessage,
    MessageRole, ParsedAction, Usage,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::{anthropic_payload, parse_anthropic_response, parse_anthropic_stream};
    use crate::openai::{openai_responses_payload, parse_openai_response, parse_openai_stream};
    use crate::transport::should_retry_status;
    use async_trait::async_trait;
    use peridot_common::PeriResult;

    #[derive(Clone, Debug)]
    struct StaticProvider;

    #[async_trait]
    impl LlmProvider for StaticProvider {
        async fn complete(&self, _request: CompletionRequest) -> PeriResult<CompletionResponse> {
            Ok(CompletionResponse {
                text: "hello".to_string(),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
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

    #[test]
    fn parses_direct_json_action() {
        let action =
            parse_action(r#"{"thinking":"ok","action":"agent_done","parameters":{"done":true}}"#)
                .unwrap();

        assert_eq!(action.thinking.as_deref(), Some("ok"));
        assert_eq!(action.tool_call.name, "agent_done");
    }

    #[test]
    fn parses_json_code_block() {
        let action = parse_action(
            r#"Here:
```json
{"action":"file_read","parameters":{"path":"README.md"}}
```"#,
        )
        .unwrap();

        assert_eq!(action.tool_call.name, "file_read");
    }

    #[test]
    fn extracts_first_json_object() {
        let action =
            parse_action(r#"noise {"action":"plan_create","parameters":{"steps":[]}} tail"#)
                .unwrap();

        assert_eq!(action.tool_call.name, "plan_create");
    }

    #[tokio::test]
    async fn default_stream_returns_single_done_chunk() {
        let provider = StaticProvider;
        let chunks = provider
            .stream(CompletionRequest {
                model: "mock".to_string(),
                system: None,
                messages: vec![LlmMessage::new(MessageRole::User, "hello")],
                max_tokens: Some(16),
                thinking: false,
            })
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
        });

        assert_eq!(payload["system"], "top\n\ninline");
        assert_eq!(payload["messages"][0]["role"], "user");
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
    fn openai_payload_uses_responses_shape() {
        let payload = openai_responses_payload(&CompletionRequest {
            model: "gpt-5.2".to_string(),
            system: Some("system".to_string()),
            messages: vec![LlmMessage::new(MessageRole::User, "hello")],
            max_tokens: Some(256),
            thinking: false,
        });

        assert_eq!(payload["model"], "gpt-5.2");
        assert_eq!(payload["instructions"], "system");
        assert_eq!(payload["max_output_tokens"], 256);
        assert_eq!(payload["input"][0]["role"], "user");
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
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.cache_creation_tokens, 2);
        assert!(response.usage.estimated_cost_usd > 0.0);
    }

    #[test]
    fn parses_anthropic_stream_chunks_and_usage() {
        let chunks = parse_anthropic_stream(
            r#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":2,"cache_read_input_tokens":3}}}

event: content_block_delta
data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"hel"}}

event: content_block_delta
data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}

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
    fn parses_openai_response_output_text() {
        let response = parse_openai_response(
            r#"{
                "output_text": "{\"action\":\"agent_done\"}",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "input_tokens_details": {"cached_tokens": 2}
                }
            }"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(response.text, "{\"action\":\"agent_done\"}");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.usage.cache_read_tokens, 2);
    }

    #[test]
    fn parses_openai_stream_chunks_and_usage() {
        let chunks = parse_openai_stream(
            r#"event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"hel"}

event: response.output_text.delta
data: {"type":"response.output_text.delta","delta":"lo"}

event: response.completed
data: {"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5,"input_tokens_details":{"cached_tokens":2}}}}

data: [DONE]
"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(chunks[0].delta, "hel");
        assert_eq!(chunks[1].delta, "lo");
        assert!(chunks.last().unwrap().done);
        let usage = chunks.last().unwrap().usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 2);
    }

    #[test]
    fn parses_openai_response_output_items() {
        let response = parse_openai_response(
            r#"{
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": "ok"}]
                }]
            }"#,
            PricingTable::default(),
        )
        .unwrap();

        assert_eq!(response.text, "ok");
    }
}
