# Voice Conversation System Design

**Date**: 2026-03-22
**Status**: Draft
**Scope**: Gateway voice middleware + voice_mode_set tool + Panel voice configuration

## Summary

Add universal voice conversation capability to Aleph. Users can send voice messages through any Channel (Telegram, Discord, etc.) and receive voice replies — including intermediate status narration and final results — creating a Jarvis-like conversational experience. The feature is implemented as a Gateway-level I/O transformation layer, keeping the Agent Loop modality-agnostic.

## Requirements

1. **Hybrid trigger** — Receiving audio automatically triggers voice reply; explicit toggle (`voice_mode_set` tool) also supported
2. **Full narration** — All stages voiced: tool status broadcasts, intermediate results, final response (Jarvis-style)
3. **Smart coalescing** — Short messages (status updates) sent immediately; long replies split by natural paragraphs
4. **Gateway middleware** — STT/TTS handled at Gateway layer; Agent Loop remains text-only
5. **Provider configurable** — Reuse existing Generation Provider config; user selects default voice in Panel
6. **Graceful degradation** — Fall back to text with notification when Channel lacks audio or TTS fails
7. **Per-channel state** — Voice mode state is independent per Channel
8. **Ephemeral audio** — Session history stores text only; audio is transient I/O artifact

## Architecture

### Position in the Stack

```
Channel (I/O)
  ↕ attachments (audio files)
Gateway Voice Middleware (NEW)
  ↕ text only
Agent Loop (Brain)
  ↕ text + tools
Core Services
```

The voice middleware sits between Channel I/O and the Agent Loop, performing bidirectional modality conversion. The Agent Loop never sees or produces audio — it works exclusively with text.

### Approach: Dual Middleware Pipeline

Two middleware components inserted into the Gateway message pipeline:

- **InboundVoiceMiddleware**: Intercepts incoming audio attachments → STT → text
- **OutboundVoiceMiddleware**: Intercepts outgoing stream events → TTS → audio attachments

This approach was chosen over Channel decoration (can't intercept StreamEvents) and EventBus-driven processing (ordering guarantees too complex).

## Data Model

### VoiceState (per-channel)

```rust
// core/src/gateway/voice/state.rs
pub struct VoiceState {
    pub enabled: bool,              // Explicit toggle
    pub provider: Option<String>,   // TTS provider override, None = global default
    pub voice: Option<String>,      // Voice ID override
    pub consecutive_failures: u8,   // Auto-disable after 3 consecutive TTS failures
}
```

Storage: `ChannelRegistry` holds `HashMap<ChannelId, VoiceState>`. Non-persistent — defaults to disabled on restart, re-enabled via conversation (R9).

### InboundMessage

No structural changes. The middleware:
- Writes transcribed text into `message.text`
- Sets `metadata["voice_input"] = true` (signals outbound middleware to auto-reply with voice)
- Removes processed audio attachments from `attachments`

### OutboundMessage

No structural changes. TTS audio appended as `Attachment { mime_type: "audio/mp3", data, filename }`. Text always preserved in `text` field — audio is additive, never a replacement.

## Inbound Voice Middleware

Location: `core/src/gateway/voice/inbound.rs`

### Processing Flow

```
InboundMessage arrives
  → Scan attachments for audio/* MIME types
  → No audio: pass through unchanged
  → Has audio:
      1. Read audio data (already cached locally by Channel or available via URL)
      2. Call MediaPipeline STT (reuse audio_transcribe provider infrastructure)
      3. Write transcription to message.text
         - If text was non-empty: prepend "[Voice] transcription" + original text
      4. Set metadata voice_input = true
      5. Remove processed audio attachments
      6. Set voice_reply_hint = true for this request
```

### Error Handling

- STT failure → Keep original audio attachment intact, append "[Voice transcription failed, please resend or use text]" to text, set voice_reply_hint = false
- Network timeout → Same degradation as STT failure

### Boundaries

- Does NOT modify session_key routing
- Does NOT persist audio files
- Does NOT process non-audio attachments (images, documents pass through)

## Outbound Voice Middleware

Location: `core/src/gateway/voice/outbound.rs`

### Voice Reply Decision Logic (priority order)

1. `ChannelCapabilities.audio == false` → no voice
2. No TTS provider configured → no voice, notify degradation
3. `VoiceState.enabled == true` (explicit toggle) → voice
4. `voice_reply_hint == true` (user sent audio this request) → voice
5. None of above → no voice

### Smart Coalescing Strategy

The middleware processes StreamEvent flow from the Agent Loop:

| Event | Voice Strategy | Example |
|-------|---------------|---------|
| `BlockReply(is_final: false)` short | Immediate TTS + send | "Let me look that up..." |
| `BlockReply(is_final: false)` long | Accumulate by paragraph, TTS each | Multi-paragraph explanation |
| `BlockReply(is_final: true)` | TTS remaining buffer + send | Final paragraph |
| `Error` | Immediate short TTS | "Something went wrong..." |

Note: The LLM naturally produces status narration text (guided by voice mode prompt injection). These arrive as `BlockReply` events — the middleware does not need to fabricate status messages from `ToolStart`/`ToolComplete` events.

### TTS Execution

- Calls `GenerationProviderRegistry` for the configured default speech provider
- Voice selection priority: VoiceState.voice override > provider's GenerationDefaults.voice > provider's first available voice
- Output: audio bytes wrapped as `Attachment`
- Sends via `Channel.send(OutboundMessage { text, attachments: [audio] })`

### Ordering Guarantee

- TTS requests queued via `tokio::mpsc` channel, processed sequentially
- Ensures voice messages arrive in correct order matching text flow

### Text Always Sent

Regardless of TTS success or failure, text content is always delivered. Voice is an enhancement layer, not a replacement.

## Tool: voice_mode_set

Location: `core/src/builtin_tools/voice_tools/voice_mode_set.rs`

### Interface

```rust
pub struct VoiceModeSetInput {
    pub enabled: bool,
    pub channel_id: Option<String>,  // None = current channel
    pub provider: Option<String>,    // Override default TTS provider
    pub voice: Option<String>,       // Override default voice
}

pub struct VoiceModeSetOutput {
    pub success: bool,
    pub channel_id: String,
    pub state: VoiceState,
    pub message: String,  // e.g., "Voice mode enabled for Telegram" / "Channel does not support audio"
}
```

Triggered by natural language: "turn on voice mode", "switch to voice replies", "use ElevenLabs voice".

Returns failure with explanation when Channel lacks audio capability or no TTS provider is configured.

## Prompt Integration

Location: `core/src/agent_loop/prompt_builder.rs`

When voice mode is active for the current channel, inject into system prompt:

```
## Voice Mode

Current Channel has voice mode enabled. Your replies will be converted to speech. Guidelines:

1. Narrate your actions briefly before and after tool use (e.g., "Let me check that...", "Found it")
2. Use conversational, spoken-language style — avoid markdown, code blocks, tables
3. Organize long replies in natural paragraphs, keep each concise
4. Express numbers and URLs in spoken form ("about three thousand five hundred" not "3,500")
```

This leverages R8 (LLM Sovereignty) and R10 (Intelligence in the Prompt) — the LLM naturally produces voice-appropriate content without hardcoded templates.

## Degradation Strategy

| Scenario | Behavior |
|----------|----------|
| Channel doesn't support audio | voice_mode_set returns error with explanation |
| No TTS provider configured | voice_mode_set returns error: "Configure a Speech provider in Panel first" |
| Single TTS failure | That message falls back to text-only, appends "[Voice generation failed]" |
| 3 consecutive TTS failures | Auto-disable voice mode for that channel, notify user |
| STT failure | Keep original audio attachment, add failure notice to text |
| TTS timeout (>10s) | Treated as single failure |

### Consecutive Failure Counter

Stored in `VoiceState.consecutive_failures`. Reset to 0 on any successful TTS call. At 3, `enabled` is set to `false` and user is notified: "Voice mode auto-disabled due to repeated failures."

## Panel Configuration Improvements

### 1. Extend GenerationDefaults

```rust
// core/src/config/types/generation/defaults.rs
pub struct GenerationDefaults {
    // Existing fields...
    pub width: Option<u32>,
    pub height: Option<u32>,
    // ...

    // New voice fields
    pub voice: Option<String>,        // Default voice ID (e.g., "alloy", "rachel")
    pub speed: Option<f32>,           // Default speed 0.25-4.0
    pub audio_format: Option<String>, // Default output format (mp3, opus, etc.)
}
```

### 2. Voice Enumeration RPC Endpoint

New method `generation_providers.voices`:

```json
// Request
{ "method": "generation_providers.voices", "params": { "provider_id": "openai-tts" } }

// Response
{
  "voices": [
    { "id": "alloy", "name": "Alloy", "gender": "neutral", "description": "Neutral, balanced" },
    { "id": "nova", "name": "Nova", "gender": "female", "description": "Warm, friendly" },
    { "id": "onyx", "name": "Onyx", "gender": "male", "description": "Deep, authoritative" }
  ]
}
```

Each TTS provider implements `list_voices()`. Hardcoded initially (voice lists are stable); extensible to dynamic API queries later.

### 3. Provider Detail Form — Voice Configuration

When a provider's capabilities include `Speech`, the right-side detail panel adds:

- **Default Voice** — dropdown populated from `generation_providers.voices`, showing name + gender
- **Default Speed** — slider, range 0.25-4.0, default 1.0
- **Default Format** — dropdown (mp3/opus/aac/flac), filtered by provider support
- **Test Voice** button — generates a test sentence with selected voice+speed, plays in browser via `<audio>` element

### 4. Generation Settings Page

Verify the Default Speech Provider selector works correctly in `views/settings/generation.rs`. Display the currently configured default provider's voice summary.

## Module Structure

### New Files

```
core/src/gateway/voice/
├── mod.rs              // Module entry, VoiceState definition
├── inbound.rs          // InboundVoiceMiddleware
├── outbound.rs         // OutboundVoiceMiddleware + smart coalescing
└── chunker.rs          // Voice segmentation (extend existing BlockReplyChunker)

core/src/builtin_tools/voice_tools/
├── mod.rs              // Tool registration
└── voice_mode_set.rs   // voice_mode_set tool
```

### Modified Files

| File | Change |
|------|--------|
| `gateway/channel_registry.rs` | Add `voice_states: HashMap<ChannelId, VoiceState>`, call InboundVoiceMiddleware before routing |
| `gateway/event_emitter.rs` | Route outbound events through OutboundVoiceMiddleware before Channel.send() |
| `builtin_tools/mod.rs` | Register `voice_mode_set` tool |
| `agent_loop/prompt_builder.rs` | Inject voice mode prompt when active |
| `config/types/generation/defaults.rs` | Add voice/speed/audio_format fields |
| `gateway/handlers/generation_providers.rs` | Add `generation_providers.voices` RPC handler |
| `generation/providers/openai_tts.rs` | Implement `list_voices()` |
| `generation/providers/elevenlabs.rs` | Implement `list_voices()` |
| `interfaces/webchat/src/views/settings/providers.rs` | Add voice config UI to detail panel |
| `interfaces/webchat/src/api/generation_providers.rs` | Add voices API client |

### Unchanged

- **Channel trait and all Channel implementations** — already support attachments
- **Agent Loop core logic** — remains modality-agnostic
- **SessionManager** — history stays text-only
- **MediaPipeline / generation providers** — called but not modified

## Dependency Flow

```
voice_mode_set tool ──writes──→ ChannelRegistry.voice_states
InboundVoiceMiddleware ──reads──→ voice_states (sets hint)
                       ──calls──→ MediaPipeline (STT)
OutboundVoiceMiddleware ──reads──→ voice_states + hint
                        ──calls──→ GenerationProviderRegistry (TTS)
                        ──sends──→ Channel.send()
PromptBuilder ──reads──→ voice_states (injects prompt)
Panel UI ──calls──→ generation_providers.voices RPC
         ──writes──→ GenerationDefaults (voice, speed, format)
```

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Gateway middleware, not Agent Loop | Voice is I/O modality conversion, not reasoning (R1) |
| Per-channel state, not per-session | Different channels have different audio capabilities |
| Non-persistent VoiceState | Restart = clean slate; re-enable via conversation (R9) |
| Text always sent alongside audio | Voice is enhancement, not replacement; graceful degradation |
| LLM produces narration via prompt | R8 (LLM Sovereignty) + R10 (Intelligence in Prompt) |
| No agent-level voice config | YAGNI; voice is infrastructure, not agent personality |
| Hardcoded voice lists | Voice options rarely change; avoid unnecessary API calls |
| Sequential TTS queue | Guarantees playback order matches text order |
