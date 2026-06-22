//! Static registry of model context windows.
//!
//! The harness needs to know the active model's max context window to
//! drive dynamic compaction at e.g. 90% of capacity. Each provider
//! publishes the number on its own page, so we hard-code the values
//! here and fall back to a sane default (200k) when the model name is
//! unrecognised — recent unknown models almost always have at least
//! 200k context. The match is prefix-based so versioned aliases like
//! `claude-sonnet-4-6-20251022` resolve correctly.
//!
//! For OpenRouter slugs the runtime overlays this table with the
//! catalog returned by [`crate::catalog::openrouter_context_lengths`]
//! so the operator gets exact values (and picks up new models without
//! waiting for a peridot release). Other providers (Anthropic, OpenAI)
//! don't expose context length over their public APIs so this table
//! stays authoritative for them.

/// Returns the maximum input context window in tokens for `model`, or
/// `None` when the name doesn't match any known entry. Callers
/// typically `unwrap_or(200_000)` so the dynamic compaction path stays
/// active even on new models.
pub fn context_window_tokens(model: &str) -> Option<usize> {
    let lower = model.to_ascii_lowercase();
    // Anthropic. The 1M-context families (Opus 4.6/4.7/4.8, Sonnet 4.6, Fable 5,
    // Mythos 5) must be matched before the generic 200K fallback, which still
    // covers the standard-context models (Haiku 4.5, Sonnet/Opus 4.5 and older).
    if lower.contains("opus-4-6")
        || lower.contains("opus-4-7")
        || lower.contains("opus-4-8")
        || lower.contains("sonnet-4-6")
        || lower.contains("fable")
        || lower.contains("mythos")
    {
        return Some(1_000_000);
    }
    if lower.contains("claude-opus")
        || lower.contains("claude-sonnet")
        || lower.contains("claude-haiku")
    {
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
    // DeepSeek / Qwen / Llama / other community models
    if lower.contains("deepseek") {
        return Some(128_000);
    }
    if lower.contains("qwen3") {
        return Some(256_000);
    }
    if lower.contains("qwen") {
        return Some(128_000);
    }
    if lower.contains("llama-4") {
        return Some(200_000);
    }
    if lower.contains("llama-3") {
        return Some(128_000);
    }
    if lower.contains("mistral-large") || lower.contains("mistral-medium") {
        return Some(128_000);
    }
    if lower.contains("command-r") {
        return Some(128_000);
    }
    if lower.contains("grok-2") || lower.contains("grok-3") || lower.contains("grok-4") {
        return Some(131_072);
    }
    None
}

/// Returns whether `model` accepts image input (multimodal). Conservative:
/// unknown models return `false` so we never attach images to a model we
/// can't confirm supports them. Used by the vision-routing path (feature F2)
/// to decide between inlining image blocks and a text-only fallback.
///
/// The match is prefix/substring based so versioned aliases like
/// `gpt-4o-2024-08-06` resolve correctly.
pub fn model_supports_vision(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    // Anthropic: Claude 3 and every later family accept images.
    if lower.contains("claude-opus")
        || lower.contains("claude-sonnet")
        || lower.contains("claude-haiku")
        || lower.starts_with("claude-3")
    {
        return true;
    }
    // OpenAI: 4o / 4.1 / 5 / 4-turbo are multimodal; base gpt-4 and
    // gpt-3.5 are text-only.
    if lower.starts_with("gpt-4o")
        || lower.starts_with("gpt-4.1")
        || lower.starts_with("gpt-5")
        || lower.starts_with("gpt-4-turbo")
    {
        return true;
    }
    // o-series reasoning models accept images, except text-only o1-mini.
    if lower.starts_with("o1-mini") {
        return false;
    }
    if lower.starts_with("o1") || lower.starts_with("o3") || lower.starts_with("o4") {
        return true;
    }
    // Gemini: all current models are multimodal.
    if lower.contains("gemini") {
        return true;
    }
    // Llama 4 is natively multimodal.
    if lower.contains("llama-4") {
        return true;
    }
    // Grok 4 and the explicit vision variants.
    if lower.contains("grok-4") || lower.contains("grok-2-vision") || lower.contains("grok-vision")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_context_windows_resolve_by_family() {
        // 1M-context families.
        assert_eq!(context_window_tokens("claude-sonnet-4-6"), Some(1_000_000));
        assert_eq!(
            context_window_tokens("claude-opus-4-7-20251101"),
            Some(1_000_000)
        );
        assert_eq!(context_window_tokens("claude-opus-4-8"), Some(1_000_000));
        assert_eq!(context_window_tokens("claude-fable-5"), Some(1_000_000));
        // Standard 200K-context families (Haiku 4.5, Sonnet/Opus 4.5 and older).
        assert_eq!(context_window_tokens("claude-haiku-4-5"), Some(200_000));
        assert_eq!(context_window_tokens("claude-sonnet-4-5"), Some(200_000));
        assert_eq!(
            context_window_tokens("claude-3-opus-20240229"),
            Some(200_000)
        );
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
        assert_eq!(context_window_tokens("gemini-2.5-pro"), Some(1_000_000));
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(context_window_tokens("totally-fictional-model").is_none());
    }

    #[test]
    fn case_insensitive_match() {
        assert_eq!(context_window_tokens("Claude-Sonnet-4-6"), Some(1_000_000));
    }

    #[test]
    fn vision_capable_models() {
        assert!(model_supports_vision("claude-opus-4-8"));
        assert!(model_supports_vision("claude-3-haiku-20240307"));
        assert!(model_supports_vision("gpt-4o-2024-08-06"));
        assert!(model_supports_vision("gpt-4.1-mini"));
        assert!(model_supports_vision("gpt-5"));
        assert!(model_supports_vision("o3"));
        assert!(model_supports_vision("gemini-2.5-pro"));
        assert!(model_supports_vision("llama-4-scout"));
    }

    #[test]
    fn text_only_models_reject_vision() {
        assert!(!model_supports_vision("gpt-4")); // base gpt-4, not turbo/4o
        assert!(!model_supports_vision("gpt-3.5-turbo"));
        assert!(!model_supports_vision("o1-mini"));
        assert!(!model_supports_vision("totally-fictional-model"));
    }

    #[test]
    fn vision_match_is_case_insensitive() {
        assert!(model_supports_vision("GPT-4o"));
    }
}
