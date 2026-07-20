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
5. **Provider configurable** — Reuse existing Generation Provider config; user selects default voice in Panel. Custom providers (OpenAI/Azure proxies) also support voice selection via protocol-based voice lists
6. **Graceful degradation** — Fall back to text with notification when Channel lacks audio or TTS fails
7. **Per-channel state** — Voice mode state is independent per Channel
8. **Ephemeral audio** — Session history stores text only; audio is transient I/O artifact

## Architecture

### Position in the Stack

```
Channel (I/O)
  ↕ attachments (audio files)
Gateway Voice Layer (NEW)
  ↕ text only
Agent Loop (Brain)
  ↕ text + tools
Core Services
```

The voice layer sits between Channel I/O and the Agent Loop, performing bidirectional modality conversion. The Agent Loop never sees or produces audio — it works exclusively with text.

### Approach: Inbound Middleware + ReplyEmitter Voice Mode

Two integration points in the Gateway message pipeline:

- **InboundVoiceMiddleware**: Intercepts incoming audio attachments → STT → text (new component)
- **ReplyEmitter voice mode**: Extends the existing `ReplyEmitter` with a third output mode (`Voice`) alongside `Typewriter` and `Instant`. The ReplyEmitter already buffers `ResponseChunk` events and sends text to channels — adding TTS generation here is a natural extension, avoiding a separate outbound middleware.

This approach was chosen over Channel decoration (can't intercept StreamEvents) and EventBus-driven processing (ordering guarantees too complex).

## Data Model

### VoiceState (per-channel)

```rust
// src/gateway/voice/state.rs
pub struct VoiceState {
    pub enabled: bool,              // Explicit toggle
    pub provider: Option<String>,   // TTS provider override, None = global default
    pub voice: Option<String>,      // Voice ID override
    pub consecutive_failures: u8,   // Auto-disable after 3 consecutive TTS failures
}
```

Storage: `ChannelRegistry` holds `HashMap<ChannelId, VoiceState>`. Non-persistent — defaults to disabled on restart, re-enabled via conversation (R9).

### voice_reply_hint

`InboundMessage` has no `metadata` field. The `voice_reply_hint` (indicating the user sent audio and should receive a voice reply) is carried on the `InboundContext` struct, which already flows through the request pipeline carrying routing and authorization context:

```rust
// Added to InboundContext
pub voice_reply_hint: bool,  // true when inbound audio was transcribed
```

This hint is read by the `ReplyEmitter` to decide whether to activate voice mode for the current request (even if the channel's `VoiceState.enabled` is false).

### InboundMessage

No structural changes. The inbound middleware:
- Writes transcribed text into `message.text`
- Sets `context.voice_reply_hint = true` on the `InboundContext`
- Removes processed audio attachments from `attachments`

### OutboundMessage

No structural changes. TTS audio appended as `Attachment { mime_type: "audio/mp3", data, filename }`. Text always preserved in `text` field — audio is additive, never a replacement.

## Inbound Voice Middleware

Location: `src/gateway/voice/inbound.rs`

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
      4. Set context.voice_reply_hint = true on InboundContext
      5. Remove processed audio attachments
```

### Error Handling

- STT failure → Keep original audio attachment intact, append "[Voice transcription failed, please resend or use text]" to text, set voice_reply_hint = false
- Network timeout → Same degradation as STT failure

### Boundaries

- Does NOT modify session_key routing
- Does NOT persist audio files
- Does NOT process non-audio attachments (images, documents pass through)

## Outbound Voice: ReplyEmitter Voice Mode

Location: Extend existing `src/gateway/reply_emitter.rs`

The `ReplyEmitter` already implements typewriter and instant modes for `StreamEvent::ResponseChunk` events. Voice mode is added as a third mode that:
1. Receives `ResponseChunk` events (with `content`, `is_final`, `is_intermediate` fields)
2. Applies smart coalescing to determine when to generate TTS
3. Calls TTS provider and attaches audio to the outbound message

### Voice Reply Decision Logic (priority order)

1. `ChannelCapabilities.audio == false` → no voice
2. No TTS provider configured → no voice, notify degradation
3. `VoiceState.enabled == true` (explicit toggle) → voice
4. `voice_reply_hint == true` (user sent audio this request) → voice
5. None of above → no voice

### Smart Coalescing Strategy

The ReplyEmitter voice mode processes `StreamEvent::ResponseChunk` events:

| Condition | Voice Strategy | Example |
|-----------|---------------|---------|
| `is_intermediate: true` (short status) | Immediate TTS + send | "Let me look that up..." |
| `is_intermediate: false, is_final: false` (streaming content) | Accumulate by paragraph, TTS each completed paragraph | Multi-paragraph explanation |
| `is_final: true` | TTS remaining buffer + send | Final paragraph |
| `StreamEvent::Error` | Immediate short TTS | "Something went wrong..." |

The `is_intermediate` flag already distinguishes short status updates from substantive content — this maps naturally to the "immediate vs. accumulate" coalescing decision.

Note: The LLM naturally produces status narration text (guided by voice mode prompt injection). These arrive as `ResponseChunk` events — the ReplyEmitter does not need to fabricate status messages from `ToolStart`/`ToolComplete` events.

### TTS Execution

- Calls `GenerationProviderRegistry` for the configured default speech provider
- Voice selection priority: VoiceState.voice override > provider's GenerationDefaults.voice > provider's first available voice
- Output: audio bytes wrapped as `Attachment`
- Sends via `Channel.send(OutboundMessage { text, attachments: [audio] })`

### Ordering Guarantee

- TTS requests queued via `tokio::mpsc` channel within the ReplyEmitter, processed sequentially
- Ensures voice messages arrive in correct order matching text flow

### Text Always Sent

Regardless of TTS success or failure, text content is always delivered. Voice is an enhancement layer, not a replacement.

## Tool: voice_mode_set

Location: `src/builtin_tools/voice_tools/voice_mode_set.rs`

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

Location: `src/agent_loop/prompt_builder.rs`

The `PromptBuilder` receives voice mode status via `InboundContext`, which already flows into `LayerInput.inbound` during prompt construction. When the inbound middleware processes a request, it sets `voice_mode_active: bool` on `InboundContext` (true if `VoiceState.enabled` OR `voice_reply_hint`). The `PromptBuilder` reads this flag to conditionally inject the voice prompt layer.

```rust
// Added to InboundContext (alongside voice_reply_hint)
pub voice_mode_active: bool,  // true when voice output is active for this request
```

When voice mode is active, inject into system prompt:

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
| TTS timeout (>10s for short text, scales with length) | Treated as single failure |

### Consecutive Failure Counter

Stored in `VoiceState.consecutive_failures`. Reset to 0 on any successful TTS call. At 3, `enabled` is set to `false` and user is notified: "Voice mode auto-disabled due to repeated failures."

### TTS Timeout Scaling

Base timeout 10s for short text (< 100 chars). For longer text, scale linearly: `timeout = 10s + (char_count / 100) * 5s`, capped at 30s. This accommodates ElevenLabs and other high-quality providers that may take longer for paragraphs.

## Panel Configuration Improvements

### 1. GenerationDefaults — Voice Fields Already Exist

The `GenerationDefaults` struct already contains `voice: Option<String>`, `speed: Option<f32>`, and `format: Option<String>`. **No schema changes needed** — only the Panel UI needs to expose these existing fields for Speech providers.

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

### Voice List Resolution

Each TTS provider implements a `list_voices()` method on the `GenerationProvider` trait (extending the existing trait, not a new one). Voice lists are resolved by **protocol**, not by provider instance:

- **Built-in providers** (OpenAI TTS, ElevenLabs): hardcoded voice lists in the provider implementation
- **Custom providers** (user-added, typically OpenAI/Azure proxies): voice list is determined by the provider's `protocol` field. A custom provider with `protocol: "openai_tts"` returns the same voice list as the built-in OpenAI TTS provider. This means users who configure a proxy/relay endpoint get the same voice dropdown as built-in providers — no manual voice ID entry needed.

Protocol-to-voice-list mapping:

| Protocol | Voice List Source |
|----------|------------------|
| `openai_tts` | OpenAI voices (alloy, echo, fable, onyx, nova, shimmer) |
| `elevenlabs` | ElevenLabs voices (rachel, domi, bella, antoni, etc.) |
| `azure_tts` | Azure Neural voices (future, extensible) |
| Unknown protocol | Empty list — user must manually enter voice ID in GenerationDefaults |

### 3. Provider Detail Form — Voice Configuration

When a provider's capabilities include `Speech`, the right-side detail panel adds:

- **Default Voice** — dropdown populated from `generation_providers.voices`, showing name + gender. Works for both built-in and custom providers (resolved by protocol)
- **Default Speed** — slider, range 0.25-4.0, default 1.0
- **Default Format** — dropdown (mp3/opus/aac/flac), filtered by provider support
- **Test Voice** button — generates a test sentence with selected voice+speed, plays in browser via `<audio>` element

### 4. Generation Settings Page

Verify the Default Speech Provider selector works correctly in `views/settings/generation.rs`. Display the currently configured default provider's voice summary.

## Module Structure

### New Files

```
src/gateway/voice/
├── mod.rs              // Module entry, VoiceState definition
└── inbound.rs          // InboundVoiceMiddleware

src/builtin_tools/voice_tools/
├── mod.rs              // Tool registration
└── voice_mode_set.rs   // voice_mode_set tool
```

### Modified Files

| File | Change |
|------|--------|
| `gateway/channel_registry.rs` | Add `voice_states: HashMap<ChannelId, VoiceState>`, call InboundVoiceMiddleware before routing |
| `gateway/reply_emitter.rs` | Add `Voice` output mode with smart coalescing + TTS generation |
| `gateway/inbound_router/executor.rs` | Thread `voice_reply_hint` from InboundContext to ReplyEmitter |
| `builtin_tools/mod.rs` | Register `voice_mode_set` tool |
| `agent_loop/prompt_builder.rs` | Inject voice mode prompt when active; access voice state via AgentEnv |
| `gateway/handlers/generation_providers.rs` | Add `generation_providers.voices` RPC handler |
| `generation/providers/openai_tts.rs` | Implement `list_voices()` on GenerationProvider trait |
| `generation/providers/elevenlabs.rs` | Implement `list_voices()` on GenerationProvider trait |
| `interfaces/webchat/src/views/settings/providers.rs` | Add voice config UI (dropdown, speed slider, test button) to detail panel |
| `interfaces/webchat/src/api/generation_providers.rs` | Add voices API client |

### Unchanged

- **Channel trait and all Channel implementations** — already support attachments
- **Agent Loop core logic** — remains modality-agnostic
- **SessionManager** — history stays text-only
- **MediaPipeline / generation providers** — called but not modified (except adding `list_voices()` to trait)
- **GenerationDefaults schema** — voice/speed/format fields already exist

## Dependency Flow

```
voice_mode_set tool ──writes──→ ChannelRegistry.voice_states
InboundVoiceMiddleware ──reads──→ voice_states
                       ──sets───→ InboundContext.voice_reply_hint
                       ──calls──→ MediaPipeline (STT)
ReplyEmitter (voice mode) ──reads──→ voice_states + InboundContext.voice_reply_hint
                           ──calls──→ GenerationProviderRegistry (TTS)
                           ──sends──→ Channel.send()
PromptBuilder ──reads──→ InboundContext.voice_mode_active
Panel UI ──calls──→ generation_providers.voices RPC (protocol-based resolution)
         ──writes──→ GenerationDefaults (voice, speed, format)
```

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Gateway layer, not Agent Loop | Voice is I/O modality conversion, not reasoning (R1) |
| ReplyEmitter voice mode, not separate middleware | ReplyEmitter already handles typewriter/instant modes; voice is a natural third mode. Avoids ordering complexity of a separate component |
| Per-channel state, not per-session | Different channels have different audio capabilities |
| Non-persistent VoiceState | Restart = clean slate; re-enable via conversation (R9) |
| Text always sent alongside audio | Voice is enhancement, not replacement; graceful degradation |
| LLM produces narration via prompt | R8 (LLM Sovereignty) + R10 (Intelligence in Prompt) |
| No agent-level voice config | YAGNI; voice is infrastructure, not agent personality |
| Protocol-based voice lists for custom providers | Custom providers (OpenAI/Azure proxies) share voice lists with their upstream protocol, no manual voice ID entry needed |
| `list_voices()` on existing GenerationProvider trait | Avoids a new trait; not all providers implement it (default returns empty) |
| voice_reply_hint on InboundContext | InboundMessage has no metadata field; InboundContext already flows through pipeline |
| Scaled TTS timeout | Long paragraphs need more time; fixed 10s too aggressive for high-quality providers |
| Sequential TTS queue | Guarantees playback order matches text order |
