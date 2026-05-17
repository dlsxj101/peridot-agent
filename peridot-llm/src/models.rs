//! Static registry of model context windows.
//!
//! The harness needs to know the active model's max context window to
//! drive dynamic compaction at e.g. 90% of capacity. Each provider
//! publishes the number on its own page, so we hard-code the values
//! here and fall back to a sane default (200k) when the model name is
//! unrecognised — recent unknown models almost always have at least
//! 200k context. The match is prefix-based so versioned aliases like
//! `claude-sonnet-4-6-20251022` resolve correctly.

/// Returns the maximum input context window in tokens for `model`, or
/// `None` when the name doesn't match any known entry. Callers
/// typically `unwrap_or(200_000)` so the dynamic compaction path stays
/// active even on new models.
pub fn context_window_tokens(model: &str) -> Option<usize> {
    let lower = model.to_ascii_lowercase();
    // Anthropic
    if lower.contains("claude-opus") || lower.contains("claude-sonnet") || lower.contains("claude-haiku") {
        return Some(200_000);
    }
    if lower.starts_with("claude-3") {
        return Some(200_000);
    }
    // OpenAI / GPT
    if lower.starts_with("gpt-5") {
        return Some(400_000);
    }
    if lower.starts_with("gpt-4o") || lower.starts_with("gpt-4.1") {
        return Some(128_000);
    }
    if lower.starts_with("gpt-4-turbo") {
        return Some(128_000);
    }
    if lower.starts_with("gpt-4") {
        return Some(8_192);
    }
    if lower.starts_with("gpt-3.5-turbo") {
        return Some(16_385);
    }
    if lower.starts_with("o1-mini") || lower.starts_with("o3-mini") {
        return Some(128_000);
    }
    if lower.starts_with("o1") || lower.starts_with("o3") || lower.starts_with("o4") {
        return Some(200_000);
    }
    // Gemini
    if lower.contains("gemini-2.5") || lower.contains("gemini-1.5") {
        return Some(1_000_000);
    }
    if lower.contains("gemini") {
        return Some(128_000);
    }
    // DeepSeek / Qwen / Llama families
    if lower.contains("deepseek") {
        return Some(128_000);
    }
    if lower.contains("qwen") {
        return Some(128_000);
    }
    if lower.contains("llama-3") {
        return Some(128_000);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_claude_models_resolve_to_200k() {
        assert_eq!(context_window_tokens("claude-sonnet-4-6"), Some(200_000));
        assert_eq!(
            context_window_tokens("claude-opus-4-7-20251101"),
            Some(200_000)
        );
        assert_eq!(context_window_tokens("claude-haiku-4-5"), Some(200_000));
    }

    #[test]
    fn gpt5_resolves_to_400k() {
        assert_eq!(context_window_tokens("gpt-5"), Some(400_000));
        assert_eq!(context_window_tokens("gpt-5-mini"), Some(400_000));
    }

    #[test]
    fn old_gpt4_8k() {
        assert_eq!(context_window_tokens("gpt-4"), Some(8_192));
        assert_eq!(context_window_tokens("gpt-4-0613"), Some(8_192));
    }

    #[test]
    fn gemini_resolves_to_1m() {
        assert_eq!(
            context_window_tokens("gemini-2.5-pro"),
            Some(1_000_000)
        );
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(context_window_tokens("totally-fictional-model").is_none());
    }

    #[test]
    fn case_insensitive_match() {
        assert_eq!(
            context_window_tokens("Claude-Sonnet-4-6"),
            Some(200_000)
        );
    }
}
