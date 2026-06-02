# F3 — Voice Input

Design doc for roadmap item **F3** (spec §21.5.10). Implementation plan,
not landed code. Lowest priority of the feature track; isolate behind a
feature flag so the core CLI stays dependency-light.

## Goal

Let an operator dictate a prompt: capture microphone audio → transcribe →
drop the text into the TUI composer (or a `peridot run` prompt) as if it
were typed.

## Current state

- Input is text-only (TUI composer, `peridot run "<task>"`).
- No audio capture, no transcription.

## Architecture

Keep voice entirely optional and out of the default build.

### 1. New optional crate `peridot-voice`

- `cargo` feature `voice` on `peridot-cli` enables a `peridot-voice`
  dependency. Default build excludes it (no audio/native deps).
- Public surface behind a trait so backends are swappable:

```rust
pub trait Transcriber {
    /// Transcribe a finished PCM buffer to text.
    fn transcribe(&self, pcm: &AudioBuffer) -> anyhow::Result<String>;
}
```

### 2. Capture (`cpal`)

- `cpal` opens the default input device, streams f32/i16 PCM into a ring
  buffer.
- **VAD** (voice-activity detection): a simple energy-gate first
  (RMS threshold + silence-timeout to end an utterance); `webrtc-vad` as a
  later upgrade. Keeps push-to-talk and auto-stop both possible.

### 3. Transcription backends

- **Local** (default for `voice` feature): `whisper-rs` (whisper.cpp
  bindings) with a small/base model path from config. Fully offline.
- **Cloud**: OpenAI Whisper API backend (`whisper-1`) for hosts without
  the model — reuses existing OpenAI auth.
- Backend chosen by `[voice] backend = "local|openai"`.

### 4. TUI integration (`peridot-tui`)

- A push-to-talk keybinding (e.g. `Ctrl+R`) toggles capture; the status
  bar shows a recording marker using the existing five-marker vocabulary
  (no new decorative icons) and a `PhraseKey` for the recording/transcribing
  states (English + Korean, per the i18n rule).
- On utterance end → transcribe → insert text at the composer caret.
  The operator edits/submits normally; nothing auto-sends.

## Integration points

- `peridot-cli`: `voice` feature flag; wire `Transcriber` into the TUI
  host; optional `peridot run --voice` to dictate the initial prompt.
- `peridot-tui`: keybinding, recording state, `PhraseKey` additions,
  caret insertion.
- Config: `[voice] backend`, `model_path`, `device`, `vad`,
  `push_to_talk_key`.

## Milestones

1. `peridot-voice` crate skeleton + `Transcriber` trait + `AudioBuffer`,
   behind the `voice` feature (no-op default).
2. `cpal` capture + energy-gate VAD (unit-test VAD on synthetic buffers).
3. Local `whisper-rs` backend (gated; model path from config).
4. OpenAI Whisper backend.
5. TUI push-to-talk + status marker + `PhraseKey` (EN/KO) + caret insert.
6. `peridot run --voice`.

## Risks / decisions

- **Native deps**: `cpal` and `whisper-rs` pull platform audio + C++
  build deps — strictly behind the `voice` feature so default builds and
  CI are unaffected.
- **Model download**: whisper models are large; document the path and do
  not bundle. Cloud backend avoids the download.
- **Latency/UX**: transcribe on utterance-end, not continuously, to keep
  it responsive and cheap.
- **Privacy**: local backend is fully offline; make the cloud backend an
  explicit opt-in.

## Testing

- VAD energy-gate unit tests on synthetic PCM (silence vs speech bursts).
- A mock `Transcriber` to test the TUI caret-insertion flow without audio
  hardware.
- Feature-flag matrix: default build (no voice) and `--features voice`
  both compile.
