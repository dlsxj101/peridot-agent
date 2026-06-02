# F2 — Multimodal Image Input (Vision Routing)

Design doc for roadmap item **F2** (spec §21.5.2). This is an
implementation plan, not landed code. The attachment *capture* surface is
already done; F2 is the model-side routing that makes attached images
actually reach a vision-capable model.

## Goal

Send images the operator attaches (`/attach <path>`, VS Code paste/drop,
`.peridot/attachments/`) to a vision-capable model as real image content,
instead of recording placeholder metadata, with a graceful fallback for
text-only models.

## Current state

- ✅ Attach pipeline: `/attach`, `/attachments`, `/detach`, VS Code
  paste/drop, `.peridot/attachments/` storage, session-local inventory.
- ✅ Images are recorded as attachment metadata (path + placeholder),
  surfaced as cards.
- ✅ **LLM layer (landed, first increment)**: `peridot-llm` carries images.
  `LlmMessage` has an additive `images: Vec<ImageContent>` field (empty on
  the text-only path, so every existing call site is unchanged) and a
  `user_with_images` builder. The Anthropic (`image`/`base64` block),
  OpenAI Chat (`image_url` data-URL part), and OpenAI Codex/Responses
  (`input_image` part) adapters all serialize images. A
  `model_supports_vision(model)` capability gate distinguishes vision
  models from text-only ones (conservative: unknown → false). Unit-tested
  on all three wire formats + the capability table.
- ❌ **Not yet wired end-to-end**: attached images are still recorded as
  text placeholders in `ContextManager::to_messages`; nothing yet reads
  the image bytes, base64-encodes them, and emits `user_with_images`. That
  resolver + the vision-model routing + OCR fallback are the remaining
  milestones below.

## Architecture

### 1. Content model (`peridot-common` / `peridot-llm`)

Extend the message content type from "string" to a content-part list:

```rust
pub enum ContentPart {
    Text(String),
    Image { media_type: String, data: ImageData }, // base64 or url
}
pub enum ImageData { Base64(String), Url(String) }
```

Keep a `From<String>` so existing text-only call sites are unchanged
(additive). Messages serialize to each provider's native shape.

### 2. Provider adapters (`peridot-llm/src/{anthropic,openai}.rs`)

- **Anthropic**: `content: [{type:"image", source:{type:"base64",
  media_type, data}}, {type:"text", text}]` on the Messages API.
- **OpenAI**: `content: [{type:"image_url", image_url:{url}}, ...]` on
  Chat Completions (data-URL or hosted URL).
- A per-provider `supports_vision(model)` capability check. Route the
  request to a vision-capable model id when images are present.

### 3. Capability + fallback (`peridot-core`)

- When a turn carries image attachments and the active model is
  vision-capable → inline the image parts.
- When the active model is text-only → **OCR fallback**: extract text
  from the image and inject it as a tagged text block
  (`<image-ocr path=...>...</image-ocr>`), preserving the
  external-content tagging rule. OCR backend behind a trait
  (`ImageTextExtractor`) so it is swappable (Tesseract via `leptess`, or a
  cloud OCR) and optional at build time.

### 4. Attachment → content wiring (`peridot-context`)

- Attachment PlanReminders for image paths already exist. Add a resolver
  that, at request-build time, reads the image bytes, infers
  `media_type`, base64-encodes, and emits a `ContentPart::Image` for the
  next turn (bounded by a size cap; downscale large images).

## Integration points

- `peridot-llm` provider request builders (the main change).
- `peridot-core` turn assembly (decide inline vs OCR).
- Config: `[vision] enabled`, `max_image_bytes`, `ocr = "off|tesseract"`,
  optional `vision_model` override (defaults to a capable model of the
  active provider).
- TUI/VS Code already render attachment cards; add an "image sent to
  model" vs "OCR text" indicator.

## Milestones

1. ✅ Image content model + serialization, text path unchanged (`images`
   field + `user_with_images`; tests assert text calls unaffected).
2. ✅ Anthropic vision adapter + capability gate (`model_supports_vision`).
3. ✅ OpenAI Chat + Codex/Responses vision adapters.
4. ⬜ Attachment→image-part resolver with size cap/downscale, wired into
   request assembly and gated by `model_supports_vision`.
5. ⬜ OCR fallback behind a feature flag + trait (text-only models).
6. ⬜ Config knobs + surface indicators.

The remaining work is the resolver (milestone 4): in the request-build
path, parse image-attachment plan reminders, read the bytes (size cap +
downscale), base64-encode, and replace the placeholder user turn with a
`user_with_images` message — only when `model_supports_vision(active)` is
true; otherwise fall back to OCR text (milestone 5) or the existing
placeholder.

## Risks / decisions

- **Cost**: vision tokens are expensive — gate behind explicit config and
  surface added cost in the usage HUD. (Open decision: default on or off?)
- **Size**: enforce a byte cap and downscale; never inline multi-MB
  images raw.
- **Provider drift**: keep the capability table in one place.
- **OCR weight**: Tesseract is a native dep; keep it optional so the core
  CLI stays light.

## Testing

- Mock-provider tests asserting image parts serialize to each provider's
  shape.
- A text-only-model test asserting OCR-fallback injects tagged text.
- Size-cap/downscale unit tests on the resolver.
