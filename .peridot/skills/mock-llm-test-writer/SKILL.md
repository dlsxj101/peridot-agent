---
name: mock-llm-test-writer
description: Write deterministic Peridot tests with a mock LLM server. Use for agent loop integration tests, provider parsing tests, tool-call sequences, recovery tests, session persistence tests, or any behavior that should not call real LLM APIs.
---

# Mock LLM Test Writer

## Pattern
1. Define ordered mock responses.
2. Record requests for assertions.
3. Assert tool calls, sequence, retry count, and final state.
4. Keep fixture projects tiny and disposable.
5. Test failure and recovery paths, not only successful happy paths.

## Useful Assertions
- Stable system prompt and tool definition serialization.
- Expected tool call names and parameters.
- Correct handling of malformed JSON, code-block JSON, partial JSON, and natural-language fallback.
- Goal Checker isolation from main agent context.
- No file modifications in Plan Mode.

## Avoid
- Do not use real API keys for ordinary tests.
- Do not assert on long natural-language output unless the exact text is product behavior.
- Do not make mock tests depend on wall-clock timing unless timeout behavior is the subject.
