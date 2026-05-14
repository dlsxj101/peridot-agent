---
name: llm-provider-implementation
description: Implement or modify Peridot LLM provider behavior. Use for LlmProvider trait changes, Claude or OpenAI provider work, authentication, streaming, prompt caching, response prefill, parsing fallback, pricing, token usage, or provider error handling.
---

# LLM Provider Implementation

## Workflow
1. Re-read spec section 6 before changing provider behavior.
2. Keep `LlmProvider` generic over provider capabilities: cache, prefill, thinking, pricing, and auth method.
3. Preserve deterministic request serialization for cacheable prefixes.
4. Keep timestamps and volatile values out of stable system prompt sections.
5. Implement parsing fallback in ordered stages and test each stage.
6. Classify provider errors into retryable, compaction-triggering, auth, budget, and terminal errors.

## Claude
- Preserve prompt caching breakpoints.
- Treat response prefill/tool masking as a Claude-optimized path, not a generic provider guarantee.
- Keep thinking settings stable within a session.

## OpenAI
- Keep API key and Codex OAuth paths separated.
- Isolate unofficial OAuth/Codex compatibility code so policy or protocol changes are localized.

## Tests
- Use mock responses for streaming chunks, retries, bad JSON, usage accounting, and parse fallback.
- Avoid real API tests outside explicit E2E feature gates.
