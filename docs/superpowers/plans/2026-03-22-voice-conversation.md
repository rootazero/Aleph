# Voice Conversation System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add universal voice conversation capability — STT inbound, TTS outbound, Jarvis-style narration — as a Gateway-level I/O transformation layer.

**Architecture:** Dual integration: InboundVoiceMiddleware transcribes audio before the Agent Loop; ReplyEmitter gains a Voice output mode that converts text to speech after the Agent Loop. VoiceState per-channel, voice_mode_set tool for LLM control, prompt layer for voice-appropriate output style.

**Tech Stack:** Rust, async-trait, tokio, existing MediaPipeline (Whisper STT), existing GenerationProviderRegistry (OpenAI TTS / ElevenLabs), Leptos (Panel UI)

**Spec:** `docs/superpowers/specs/2026-03-22-voice-conversation-design.md`

---

### Task 1: VoiceState Data Model

**Files:**
- Create: `src/gateway/voice/mod.rs`
- Create: `src/gateway/voice/state.rs`
- Modify: `src/gateway/mod.rs` — add `pub mod voice;`
- Modify: `src/gateway/channel_registry.rs:43` — add `voice_states` field

- [ ] **Step 1: Create voice module with VoiceState**

```rust
// src/gateway/voice/mod.rs
pub mod state;
pub use state::VoiceState;
```

```rust
// src/gateway/voice/state.rs
use serde::{Deserialize, Serialize};

/// Per-channel voice mode state
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VoiceState {
    /// Whether voice mode is explicitly enabled
    pub enabled: bool,
    /// TTS provider override (None = use global default speech provider)
    pub provider: Option<String>,
    /// Voice ID override (None = use provider's default voice)
    pub voice: Option<String>,
    /// Consecutive TTS failure count (auto-disable at 3)
    pub consecutive_failures: u8,
}

impl VoiceState {
    /// Check if voice output is active (explicitly enabled)
    pub fn is_active(&self) -> bool {
        self.enabled
    }

    /// Record a TTS success, resetting failure counter
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }

    /// Record a TTS failure, auto-disable after 3 consecutive failures.
    /// Returns true if voice mode was auto-disabled.
    pub fn record_failure(&mut self) -> bool {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= 3 {
            self.enabled = false;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let state = VoiceState::default();
        assert!(!state.is_active());
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn record_success_resets_failures() {
        let mut state = VoiceState { enabled: true, consecutive_failures: 2, ..Default::default() };
        state.record_success();
        assert_eq!(state.consecutive_failures, 0);
        assert!(state.is_active());
    }

    #[test]
    fn auto_disable_after_three_failures() {
        let mut state = VoiceState { enabled: true, ..Default::default() };
        assert!(!state.record_failure()); // 1
        assert!(!state.record_failure()); // 2
        assert!(state.record_failure());  // 3 → auto-disabled
        assert!(!state.is_active());
    }
}
```

- [ ] **Step 2: Register voice module in gateway**

Add to `src/gateway/mod.rs`:
```rust
pub mod voice;
```

- [ ] **Step 3: Add voice_states to ChannelRegistry**

In `src/gateway/channel_registry.rs`, add field to ChannelRegistry struct (around line 43):
```rust
use std::collections::HashMap;
use crate::sync_primitives::RwLock;
use super::voice::VoiceState;
use super::channel::ChannelId;

// Add to ChannelRegistry struct:
voice_states: RwLock<HashMap<String, VoiceState>>,
```

Add methods:
```rust
/// Get voice state for a channel (returns default if not set)
pub async fn get_voice_state(&self, channel_id: &str) -> VoiceState {
    self.voice_states.read().await
        .get(channel_id)
        .cloned()
        .unwrap_or_default()
}

/// Set voice state for a channel
pub async fn set_voice_state(&self, channel_id: &str, state: VoiceState) {
    self.voice_states.write().await
        .insert(channel_id.to_string(), state);
}

/// Update voice state with a closure
pub async fn update_voice_state<F>(&self, channel_id: &str, f: F)
where F: FnOnce(&mut VoiceState) {
    let mut states = self.voice_states.write().await;
    let state = states.entry(channel_id.to_string()).or_default();
    f(state);
}
```

Initialize in constructor:
```rust
voice_states: RwLock::new(HashMap::new()),
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib voice::state`
Expected: 3 tests pass

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 6: Commit**

```bash
git add src/gateway/voice/ src/gateway/mod.rs src/gateway/channel_registry.rs
git commit -m "voice: add VoiceState data model and ChannelRegistry integration"
```

---

### Task 2: Gateway InboundContext — voice_reply_hint

**Files:**
- Modify: `src/gateway/inbound_context.rs:49-84` — add `voice_reply_hint` field

- [ ] **Step 1: Write test**

Add to existing tests in `src/gateway/inbound_context.rs`:
```rust
#[test]
fn test_voice_reply_hint_default_false() {
    let msg = make_test_message();
    let route = ReplyRoute::new(
        ChannelId::new("telegram"),
        ConversationId::new("chat-1"),
    );
    let session_key = SessionKey::main("main");
    let ctx = InboundContext::new(msg, route, session_key);
    assert!(!ctx.voice_reply_hint);
}

#[test]
fn test_with_voice_reply_hint() {
    let msg = make_test_message();
    let route = ReplyRoute::new(
        ChannelId::new("telegram"),
        ConversationId::new("chat-1"),
    );
    let session_key = SessionKey::main("main");
    let ctx = InboundContext::new(msg, route, session_key)
        .with_voice_reply_hint(true);
    assert!(ctx.voice_reply_hint);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::inbound_context::tests::test_voice_reply_hint`
Expected: FAIL — field doesn't exist

- [ ] **Step 3: Add voice_reply_hint field and builder method**

In `InboundContext` struct (after line 66):
```rust
/// Whether this request should receive voice reply
/// (set by InboundVoiceMiddleware when audio was transcribed)
pub voice_reply_hint: bool,
```

In `InboundContext::new()` (after line 83):
```rust
voice_reply_hint: false,
```

Add builder method (after line 103):
```rust
/// Set voice reply hint (when inbound audio was transcribed)
pub fn with_voice_reply_hint(mut self, hint: bool) -> Self {
    self.voice_reply_hint = hint;
    self
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib gateway::inbound_context`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/gateway/inbound_context.rs
git commit -m "voice: add voice_reply_hint to gateway InboundContext"
```

---

### Task 3: Thinker InboundContext — voice_mode_active + Prompt Layer

**Files:**
- Modify: `src/thinker/inbound_context.rs:57-62` — add `voice_mode_active` field
- Create: `src/thinker/layers/voice_mode.rs` — new prompt layer
- Modify: `src/thinker/layers/mod.rs` — register VoiceModeLayer

- [ ] **Step 1: Write test for voice_mode_active on InboundContext**

Add to `src/thinker/inbound_context.rs` tests:
```rust
#[test]
fn voice_mode_active_included_in_prompt() {
    let ctx = InboundContext {
        sender: SenderInfo {
            id: "u1".to_string(),
            display_name: Some("User".to_string()),
            is_owner: true,
        },
        channel: ChannelContext {
            kind: "telegram".to_string(),
            capabilities: vec![],
            is_group_chat: false,
            is_mentioned: false,
        },
        session: SessionContext::default(),
        message: MessageMetadata::default(),
        voice_mode_active: true,
    };
    let output = ctx.format_for_prompt();
    assert!(output.contains("Voice Mode: active"));
}

#[test]
fn voice_mode_inactive_not_in_prompt() {
    let ctx = InboundContext::default();
    let output = ctx.format_for_prompt();
    assert!(!output.contains("Voice Mode"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib thinker::inbound_context::tests::voice_mode`
Expected: FAIL — field doesn't exist

- [ ] **Step 3: Add voice_mode_active field**

In `InboundContext` struct (after line 61):
```rust
/// Whether voice mode is active for this request
pub voice_mode_active: bool,
```

In `format_for_prompt()` (before the final `lines.join`):
```rust
// Voice mode
if self.voice_mode_active {
    lines.push("Voice Mode: active".to_string());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib thinker::inbound_context`
Expected: All pass

- [ ] **Step 5: Create VoiceModeLayer**

```rust
// src/thinker/layers/voice_mode.rs
//! VoiceModeLayer — injects voice mode guidelines when active (priority 1710)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, LayerStability, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct VoiceModeLayer;

const VOICE_MODE_PROMPT: &str = r#"## Voice Mode

Current Channel has voice mode enabled. Your replies will be converted to speech. Guidelines:

1. Narrate your actions briefly before and after tool use (e.g., "Let me check that...", "Found it")
2. Use conversational, spoken-language style — avoid markdown, code blocks, tables
3. Organize long replies in natural paragraphs, keep each concise
4. Express numbers and URLs in spoken form ("about three thousand five hundred" not "3,500")
"#;

impl PromptLayer for VoiceModeLayer {
    fn name(&self) -> &'static str { "voice_mode" }
    fn priority(&self) -> u32 { 1710 } // Just after inbound_context (1700)
    fn stability(&self) -> LayerStability { LayerStability::Dynamic }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let active = input.inbound
            .map(|ctx| ctx.voice_mode_active)
            .unwrap_or(false);
        if active {
            output.push_str(VOICE_MODE_PROMPT);
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::inbound_context::InboundContext;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn metadata() {
        let layer = VoiceModeLayer;
        assert_eq!(layer.name(), "voice_mode");
        assert_eq!(layer.priority(), 1710);
    }

    #[test]
    fn injects_when_voice_active() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let inbound = InboundContext {
            voice_mode_active: true,
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]).with_inbound(&inbound);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.contains("## Voice Mode"));
        assert!(out.contains("spoken-language style"));
    }

    #[test]
    fn skips_when_voice_inactive() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let inbound = InboundContext::default();
        let input = LayerInput::basic(&config, &[]).with_inbound(&inbound);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_no_inbound() {
        let layer = VoiceModeLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
```

- [ ] **Step 6: Register VoiceModeLayer in layers/mod.rs**

Add to `src/thinker/layers/mod.rs`:
```rust
pub mod voice_mode;
```

Register `VoiceModeLayer` in the pipeline (find where layers are collected and add `Box::new(voice_mode::VoiceModeLayer)`).

- [ ] **Step 7: Run tests**

Run: `cargo test -p alephcore --lib thinker::layers::voice_mode`
Expected: All 4 tests pass

- [ ] **Step 8: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 9: Commit**

```bash
git add src/thinker/inbound_context.rs src/thinker/layers/voice_mode.rs src/thinker/layers/mod.rs
git commit -m "voice: add VoiceModeLayer prompt injection for voice-appropriate LLM output"
```

---

### Task 4: InboundVoiceMiddleware — STT Processing

**Files:**
- Create: `src/gateway/voice/inbound.rs`
- Modify: `src/gateway/voice/mod.rs` — add `pub mod inbound;`

- [ ] **Step 1: Write test for audio detection**

```rust
// src/gateway/voice/inbound.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{
        Attachment, ChannelId, ConversationId, InboundMessage, MessageId, UserId,
    };
    use chrono::Utc;

    fn make_text_message() -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("telegram"),
            conversation_id: ConversationId::new("chat-1"),
            sender_id: UserId::new("user-1"),
            sender_name: Some("Test".to_string()),
            text: "Hello".to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        }
    }

    fn make_voice_message() -> InboundMessage {
        let mut msg = make_text_message();
        msg.text = String::new();
        msg.attachments = vec![Attachment {
            mime_type: "audio/ogg".to_string(),
            filename: Some("voice.ogg".to_string()),
            size: Some(1024),
            url: Some("https://example.com/voice.ogg".to_string()),
            path: None,
            data: None,
        }];
        msg
    }

    #[test]
    fn has_audio_attachment_detects_audio() {
        let msg = make_voice_message();
        assert!(has_audio_attachment(&msg));
    }

    #[test]
    fn has_audio_attachment_ignores_non_audio() {
        let mut msg = make_text_message();
        msg.attachments = vec![Attachment {
            mime_type: "image/png".to_string(),
            filename: Some("photo.png".to_string()),
            size: Some(2048),
            url: None,
            path: None,
            data: None,
        }];
        assert!(!has_audio_attachment(&msg));
    }

    #[test]
    fn has_audio_attachment_empty() {
        let msg = make_text_message();
        assert!(!has_audio_attachment(&msg));
    }
}
```

- [ ] **Step 2: Implement audio detection**

```rust
// src/gateway/voice/inbound.rs
//! Inbound Voice Middleware — transcribes audio attachments via STT

use crate::gateway::channel::InboundMessage;

/// Check if message has any audio/* attachment
pub fn has_audio_attachment(msg: &InboundMessage) -> bool {
    msg.attachments.iter().any(|a| a.mime_type.starts_with("audio/"))
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib gateway::voice::inbound`
Expected: 3 tests pass

- [ ] **Step 4: Add transcription processing function**

```rust
use crate::sync_primitives::Arc;
use tracing::{debug, warn};

/// Result of processing inbound voice
pub struct VoiceProcessResult {
    /// Updated message with transcription in text field
    pub message: InboundMessage,
    /// Whether audio was successfully transcribed
    pub transcribed: bool,
}

/// Process inbound message: if it has audio attachments, transcribe them via STT.
///
/// On success: replaces text with transcription, removes audio attachments, returns transcribed=true.
/// On failure: keeps original message, appends error notice to text, returns transcribed=false.
pub async fn process_inbound_voice(
    mut msg: InboundMessage,
    media_pipeline: &Arc<crate::media::pipeline::MediaPipeline>,
) -> VoiceProcessResult {
    if !has_audio_attachment(&msg) {
        return VoiceProcessResult { message: msg, transcribed: false };
    }

    // Find audio attachments
    let (audio_attachments, other_attachments): (Vec<_>, Vec<_>) =
        msg.attachments.into_iter().partition(|a| a.mime_type.starts_with("audio/"));

    // Try to transcribe the first audio attachment
    let audio = &audio_attachments[0];
    let transcription = transcribe_attachment(audio, media_pipeline).await;

    match transcription {
        Ok(text) => {
            debug!("Voice transcribed: {} chars", text.len());
            // Set transcription as message text
            if msg.text.is_empty() {
                msg.text = text;
            } else {
                msg.text = format!("[Voice] {}\n{}", text, msg.text);
            }
            // Keep non-audio attachments only
            msg.attachments = other_attachments;
            VoiceProcessResult { message: msg, transcribed: true }
        }
        Err(e) => {
            warn!("Voice transcription failed: {}", e);
            // Restore all attachments
            msg.attachments = audio_attachments.into_iter().chain(other_attachments).collect();
            // Append error notice
            if msg.text.is_empty() {
                msg.text = "[Voice transcription failed, please resend or use text]".to_string();
            } else {
                msg.text.push_str("\n[Voice transcription failed]");
            }
            VoiceProcessResult { message: msg, transcribed: false }
        }
    }
}

async fn transcribe_attachment(
    attachment: &crate::gateway::channel::Attachment,
    media_pipeline: &Arc<crate::media::pipeline::MediaPipeline>,
) -> Result<String, String> {
    use crate::media::types::{MediaInput, MediaType};

    // Determine input source
    let input = if let Some(ref path) = attachment.path {
        MediaInput::FilePath(std::path::PathBuf::from(path))
    } else if let Some(ref url) = attachment.url {
        MediaInput::Url(url.clone())
    } else if let Some(ref data) = attachment.data {
        MediaInput::Base64(data.clone())
    } else {
        return Err("No audio source available (no path, url, or data)".to_string());
    };

    let media_type = MediaType::Audio { duration_secs: None };
    let result = media_pipeline.process(input, media_type, None).await
        .map_err(|e| format!("STT pipeline error: {}", e))?;

    match result {
        crate::media::types::MediaOutput::Text(text) => Ok(text),
        other => Err(format!("Unexpected STT output type: {:?}", other)),
    }
}
```

- [ ] **Step 5: Update mod.rs**

In `src/gateway/voice/mod.rs`:
```rust
pub mod inbound;
pub mod state;
pub use state::VoiceState;
```

- [ ] **Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors (adjust types if `Attachment` fields differ from expected)

- [ ] **Step 7: Commit**

```bash
git add src/gateway/voice/
git commit -m "voice: add InboundVoiceMiddleware for STT transcription"
```

---

### Task 5: ReplyEmitter Voice Mode — TTS Output

**Files:**
- Modify: `src/gateway/reply_emitter.rs` — add voice mode to emit()
- Create: `src/gateway/voice/outbound.rs` — TTS generation helper

- [ ] **Step 1: Create outbound voice helper**

```rust
// src/gateway/voice/outbound.rs
//! Outbound voice processing — TTS generation for ReplyEmitter voice mode

use crate::gateway::channel::{Attachment, ChannelCapabilities, ChannelId};
use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::voice::VoiceState;
use crate::sync_primitives::Arc;
use tracing::{debug, warn};

/// Determines whether voice output should be used for this response.
pub async fn should_use_voice(
    channel_registry: &ChannelRegistry,
    channel_id: &ChannelId,
    voice_reply_hint: bool,
) -> bool {
    // Check channel capabilities
    let caps = channel_registry.get_capabilities(channel_id).await;
    if let Some(caps) = caps {
        if !caps.audio {
            return false;
        }
    }

    // Check voice state
    let state = channel_registry.get_voice_state(channel_id.as_str()).await;
    if state.is_active() {
        return true;
    }

    // Auto-voice when user sent audio
    voice_reply_hint
}

/// Generate TTS audio from text.
/// Returns audio attachment on success, or None on failure.
pub async fn generate_tts(
    text: &str,
    voice_state: &VoiceState,
    generation_registry: &Arc<crate::generation::GenerationProviderRegistry>,
) -> Option<Attachment> {
    use crate::generation::types::{GenerationRequest, GenerationParams, GenerationType};

    if text.trim().is_empty() {
        return None;
    }

    let mut params = GenerationParams::default();
    params.voice = voice_state.voice.clone();

    let request = GenerationRequest {
        generation_type: GenerationType::Speech,
        prompt: text.to_string(),
        params,
        provider_id: voice_state.provider.clone(),
        ..Default::default()
    };

    match generation_registry.generate(request).await {
        Ok(output) => {
            // Extract audio data from generation output
            if let Some(data) = output.data {
                let format = output.metadata
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mp3");
                Some(Attachment {
                    mime_type: format!("audio/{}", format),
                    filename: Some(format!("voice_reply.{}", format)),
                    size: Some(data.len() as u64),
                    url: None,
                    path: None,
                    data: Some(data),
                })
            } else if let Some(ref url) = output.url {
                Some(Attachment {
                    mime_type: "audio/mp3".to_string(),
                    filename: Some("voice_reply.mp3".to_string()),
                    size: None,
                    url: Some(url.clone()),
                    path: None,
                    data: None,
                })
            } else {
                warn!("TTS output has no data or URL");
                None
            }
        }
        Err(e) => {
            warn!("TTS generation failed: {}", e);
            None
        }
    }
}

/// Compute TTS timeout based on text length.
/// Base 10s for short text, scales linearly, capped at 30s.
pub fn tts_timeout_ms(text: &str) -> u64 {
    let char_count = text.chars().count();
    let base_ms = 10_000u64;
    let extra_ms = (char_count as u64 / 100) * 5_000;
    (base_ms + extra_ms).min(30_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_short_text() {
        assert_eq!(tts_timeout_ms("hello"), 10_000);
    }

    #[test]
    fn timeout_medium_text() {
        let text = "a".repeat(200);
        assert_eq!(tts_timeout_ms(&text), 20_000); // 10 + (200/100)*5
    }

    #[test]
    fn timeout_long_text_capped() {
        let text = "a".repeat(10000);
        assert_eq!(tts_timeout_ms(&text), 30_000); // capped
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib gateway::voice::outbound`
Expected: 3 timeout tests pass

- [ ] **Step 3: Add voice fields to ReplyEmitter**

In `src/gateway/reply_emitter.rs`, add to `ReplyEmitterConfig`:
```rust
/// Whether voice mode is active for this request
pub voice_enabled: bool,
/// Whether the inbound message was a voice message (auto-voice reply)
pub voice_reply_hint: bool,
```

Add to `ReplyEmitter` struct:
```rust
/// Generation provider registry for TTS
generation_registry: Option<Arc<crate::generation::GenerationProviderRegistry>>,
/// Voice state for the target channel
voice_state: VoiceState,
```

Update constructors to accept and store these. Default `voice_enabled: false`, `voice_reply_hint: false`, `generation_registry: None`, `voice_state: VoiceState::default()`.

Add method:
```rust
/// Check if voice output should be used
fn should_voice(&self) -> bool {
    self.config.voice_enabled || self.config.voice_reply_hint
}

/// Send text as voice to channel (TTS + audio attachment)
async fn send_as_voice(&self, text: &str) {
    if let Some(ref gen_reg) = self.generation_registry {
        let timeout = Duration::from_millis(
            super::voice::outbound::tts_timeout_ms(text)
        );
        match tokio::time::timeout(
            timeout,
            super::voice::outbound::generate_tts(text, &self.voice_state, gen_reg)
        ).await {
            Ok(Some(audio_attachment)) => {
                // Record success
                self.channel_registry.update_voice_state(
                    self.route.channel_id.as_str(),
                    |s| s.record_success()
                ).await;

                // Send with audio attachment
                let message = OutboundMessage {
                    conversation_id: self.route.conversation_id.clone(),
                    text: text.to_string(),
                    attachments: vec![audio_attachment],
                    reply_to: None,
                    inline_keyboard: None,
                    metadata: Default::default(),
                };
                let _ = self.channel_registry.send(&self.route.channel_id, message).await;
                return;
            }
            Ok(None) | Err(_) => {
                // TTS failed or timed out
                let auto_disabled = {
                    let mut disabled = false;
                    self.channel_registry.update_voice_state(
                        self.route.channel_id.as_str(),
                        |s| { disabled = s.record_failure(); }
                    ).await;
                    disabled
                };
                if auto_disabled {
                    warn!("Voice mode auto-disabled for channel {} after 3 failures",
                        self.route.channel_id);
                }
            }
        }
    }
    // Fallback: send as text
    self.send_to_channel(text).await;
}
```

- [ ] **Step 4: Integrate voice into emit() method**

In the `emit()` method, modify the `ResponseChunk` handler:

For `is_intermediate` messages — when `should_voice()`, call `send_as_voice()` instead of `send_to_channel()`.

For `RunComplete` — when `should_voice()`, send the final accumulated text via `send_as_voice()` instead of `send_to_channel()` / `send_typewriter()`.

For `RunError` — when `should_voice()`, send error via `send_as_voice()`.

Key pattern: replace `self.send_to_channel(&content)` with:
```rust
if self.should_voice() {
    self.send_as_voice(&content).await;
} else {
    self.send_to_channel(&content).await;
}
```

And similarly for `send_typewriter` calls.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors (may need to adjust GenerationProviderRegistry type paths)

- [ ] **Step 6: Commit**

```bash
git add src/gateway/reply_emitter.rs src/gateway/voice/outbound.rs src/gateway/voice/mod.rs
git commit -m "voice: add ReplyEmitter voice mode with TTS output"
```

---

### Task 6: voice_mode_set Tool

**Files:**
- Create: `src/builtin_tools/voice_tools/mod.rs`
- Create: `src/builtin_tools/voice_tools/voice_mode_set.rs`
- Modify: `src/builtin_tools/mod.rs` — register module

- [ ] **Step 1: Create voice_mode_set tool**

```rust
// src/builtin_tools/voice_tools/voice_mode_set.rs
//! voice_mode_set — Toggle voice mode on/off for a channel (R9: Everything is a Tool)

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gateway::channel_registry::ChannelRegistry;
use crate::gateway::voice::VoiceState;
use crate::sync_primitives::Arc;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VoiceModeSetArgs {
    /// Whether to enable or disable voice mode
    pub enabled: bool,
    /// Channel ID to set voice mode for. Defaults to current channel if omitted.
    #[serde(default)]
    pub channel_id: Option<String>,
    /// Override the default TTS provider (e.g., "openai-tts", "elevenlabs")
    #[serde(default)]
    pub provider: Option<String>,
    /// Override the default voice ID (e.g., "alloy", "nova", "rachel")
    #[serde(default)]
    pub voice: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoiceModeSetOutput {
    pub success: bool,
    pub channel_id: String,
    pub enabled: bool,
    pub message: String,
}

pub struct VoiceModeSetTool {
    pub channel_registry: Arc<ChannelRegistry>,
}

impl VoiceModeSetTool {
    pub fn new(channel_registry: Arc<ChannelRegistry>) -> Self {
        Self { channel_registry }
    }

    pub async fn execute(
        &self,
        args: VoiceModeSetArgs,
        current_channel_id: Option<&str>,
    ) -> VoiceModeSetOutput {
        let channel_id = args.channel_id
            .as_deref()
            .or(current_channel_id)
            .unwrap_or("unknown");

        // Check channel capabilities
        let channel_id_typed = crate::gateway::channel::ChannelId::new(channel_id);
        if let Some(caps) = self.channel_registry.get_capabilities(&channel_id_typed).await {
            if !caps.audio {
                return VoiceModeSetOutput {
                    success: false,
                    channel_id: channel_id.to_string(),
                    enabled: false,
                    message: format!("Channel '{}' does not support audio", channel_id),
                };
            }
        }

        // TODO: Check if TTS provider is configured (generation registry)

        // Update voice state
        self.channel_registry.update_voice_state(channel_id, |state| {
            state.enabled = args.enabled;
            if let Some(provider) = args.provider {
                state.provider = Some(provider);
            }
            if let Some(voice) = args.voice {
                state.voice = Some(voice);
            }
            if args.enabled {
                state.consecutive_failures = 0;
            }
        }).await;

        let action = if args.enabled { "enabled" } else { "disabled" };
        VoiceModeSetOutput {
            success: true,
            channel_id: channel_id.to_string(),
            enabled: args.enabled,
            message: format!("Voice mode {} for channel '{}'", action, channel_id),
        }
    }
}
```

- [ ] **Step 2: Create mod.rs**

```rust
// src/builtin_tools/voice_tools/mod.rs
pub mod voice_mode_set;
pub use voice_mode_set::{VoiceModeSetTool, VoiceModeSetArgs, VoiceModeSetOutput};
```

- [ ] **Step 3: Register in builtin_tools/mod.rs**

Add to `src/builtin_tools/mod.rs`:
```rust
pub mod voice_tools;
```

- [ ] **Step 4: Register tool in tool registry**

Find where `SpeechGenerateTool` and other builtin tools are registered in the `BuiltinRegistry` (in `src/executor/builtin_registry/registry.rs`). Add `VoiceModeSetTool` registration following the same pattern. The tool name should be `"voice_mode_set"`, description: `"Enable or disable voice mode for a channel. When enabled, all replies will be converted to speech audio."`.

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/voice_tools/ src/builtin_tools/mod.rs src/executor/builtin_registry/
git commit -m "voice: add voice_mode_set tool (R9: Everything is a Tool)"
```

---

### Task 7: Wire Inbound Middleware into Gateway Pipeline

**Files:**
- Modify: `src/gateway/channel_registry.rs` — call InboundVoiceMiddleware in message forwarder
- Modify: `src/gateway/inbound_router/executor.rs` — pass voice state to ReplyEmitter

- [ ] **Step 1: Add voice processing in channel message forwarder**

In `src/gateway/channel_registry.rs`, find `start_message_forwarder()` (around line 323). In the message forwarding loop where `InboundMessage` is received from channels, add voice processing before forwarding:

```rust
// After receiving msg from channel, before forwarding:
let voice_result = crate::gateway::voice::inbound::process_inbound_voice(
    msg,
    &media_pipeline,  // need to pass MediaPipeline Arc
).await;
let msg = voice_result.message;
let voice_reply_hint = voice_result.transcribed;
// Store voice_reply_hint for this message ID (to be picked up by executor)
```

Note: The exact integration point depends on how `start_message_forwarder` currently processes messages. The voice processing should happen AFTER media download (if any) but BEFORE routing to the agent loop.

- [ ] **Step 2: Pass voice state to ReplyEmitter in executor**

In `src/gateway/inbound_router/executor.rs`, where `ReplyEmitter::with_config()` is called (around line 99):

```rust
// Read voice state for this channel
let voice_state = channel_registry.get_voice_state(ctx.reply_route.channel_id.as_str()).await;
let voice_enabled = voice_state.is_active() || ctx.voice_reply_hint;

let reply_config = ReplyEmitterConfig {
    // ... existing config ...
    voice_enabled,
    voice_reply_hint: ctx.voice_reply_hint,
};

let emitter = ReplyEmitter::with_config(
    channel_registry.clone(),
    ctx.reply_route.clone(),
    run_id.clone(),
    reply_config,
)
.with_voice(voice_state, generation_registry.clone());
```

Add `with_voice()` builder method to ReplyEmitter:
```rust
pub fn with_voice(
    mut self,
    voice_state: VoiceState,
    generation_registry: Arc<GenerationProviderRegistry>,
) -> Self {
    self.voice_state = voice_state;
    self.generation_registry = Some(generation_registry);
    self
}
```

- [ ] **Step 3: Thread voice_mode_active to thinker InboundContext**

Find where the thinker's `InboundContext` is constructed from the gateway's context (likely in the agent loop setup or context aggregation). Set `voice_mode_active` based on the gateway InboundContext's `voice_reply_hint` and the channel's `VoiceState.enabled`.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 5: Integration test**

Start aleph-server, send a voice message via Telegram (if configured), verify:
1. Audio is transcribed to text
2. Agent responds with both text and audio attachment
3. Voice mode can be toggled via natural language

- [ ] **Step 6: Commit**

```bash
git add src/gateway/
git commit -m "voice: wire InboundVoiceMiddleware and ReplyEmitter voice mode into gateway pipeline"
```

---

### Task 8: GenerationProvider list_voices() + RPC Endpoint

**Files:**
- Modify: `src/generation/mod.rs` — add `list_voices()` to trait
- Modify: `src/generation/providers/openai_tts.rs` — implement `list_voices()`
- Modify: `src/generation/providers/elevenlabs.rs` — implement `list_voices()`
- Modify: `src/gateway/handlers/generation_providers.rs` — add RPC handler

- [ ] **Step 1: Define VoiceInfo and add list_voices() to trait**

In `src/generation/mod.rs`, add:
```rust
/// Voice information for TTS providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub gender: String,      // "male", "female", "neutral"
    pub description: String,
}

// Add to GenerationProvider trait (with default implementation):
fn list_voices(&self) -> Vec<VoiceInfo> {
    vec![] // Non-speech providers return empty
}
```

- [ ] **Step 2: Implement for OpenAI TTS**

In `src/generation/providers/openai_tts.rs`:
```rust
fn list_voices(&self) -> Vec<VoiceInfo> {
    vec![
        VoiceInfo { id: "alloy".into(), name: "Alloy".into(), gender: "neutral".into(), description: "Neutral, balanced".into() },
        VoiceInfo { id: "echo".into(), name: "Echo".into(), gender: "male".into(), description: "Warm, conversational".into() },
        VoiceInfo { id: "fable".into(), name: "Fable".into(), gender: "neutral".into(), description: "Expressive, animated".into() },
        VoiceInfo { id: "onyx".into(), name: "Onyx".into(), gender: "male".into(), description: "Deep, authoritative".into() },
        VoiceInfo { id: "nova".into(), name: "Nova".into(), gender: "female".into(), description: "Warm, friendly".into() },
        VoiceInfo { id: "shimmer".into(), name: "Shimmer".into(), gender: "female".into(), description: "Clear, bright".into() },
    ]
}
```

- [ ] **Step 3: Implement for ElevenLabs**

In `src/generation/providers/elevenlabs.rs`:
```rust
fn list_voices(&self) -> Vec<VoiceInfo> {
    vec![
        VoiceInfo { id: "rachel".into(), name: "Rachel".into(), gender: "female".into(), description: "Calm, young".into() },
        VoiceInfo { id: "domi".into(), name: "Domi".into(), gender: "female".into(), description: "Strong, confident".into() },
        VoiceInfo { id: "bella".into(), name: "Bella".into(), gender: "female".into(), description: "Soft, warm".into() },
        VoiceInfo { id: "antoni".into(), name: "Antoni".into(), gender: "male".into(), description: "Crisp, well-rounded".into() },
        VoiceInfo { id: "elli".into(), name: "Elli".into(), gender: "female".into(), description: "Emotional, young".into() },
        VoiceInfo { id: "josh".into(), name: "Josh".into(), gender: "male".into(), description: "Deep, warm".into() },
        VoiceInfo { id: "arnold".into(), name: "Arnold".into(), gender: "male".into(), description: "Crisp, bold".into() },
        VoiceInfo { id: "adam".into(), name: "Adam".into(), gender: "male".into(), description: "Deep, narrative".into() },
        VoiceInfo { id: "sam".into(), name: "Sam".into(), gender: "male".into(), description: "Raspy, young".into() },
    ]
}
```

- [ ] **Step 4: Add protocol-based fallback for custom providers**

For custom providers that specify a `protocol` matching a known TTS type, return the voice list of that protocol. This is done in the `GenerationProviderRegistry` or in the RPC handler:

```rust
/// Get voices for a provider, falling back to protocol-based list
pub fn get_voices_for_provider(&self, provider_id: &str) -> Vec<VoiceInfo> {
    // Try the provider's own list_voices()
    if let Some(provider) = self.get_provider(provider_id) {
        let voices = provider.list_voices();
        if !voices.is_empty() {
            return voices;
        }
    }
    // Fallback: check provider config protocol
    if let Some(config) = self.get_provider_config(provider_id) {
        match config.provider_type.as_str() {
            "openai_tts" => return OpenAiTts::static_voice_list(),
            "elevenlabs" => return ElevenLabs::static_voice_list(),
            _ => {}
        }
    }
    vec![]
}
```

- [ ] **Step 5: Add RPC handler**

In `src/gateway/handlers/generation_providers.rs`, add handler for `generation_providers.voices`:

```rust
"generation_providers.voices" => {
    let provider_id = params.get("provider_id")
        .and_then(|v| v.as_str())
        .ok_or("provider_id required")?;
    let voices = generation_registry.get_voices_for_provider(provider_id);
    Ok(serde_json::to_value(voices)?)
}
```

- [ ] **Step 6: Compile check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 7: Commit**

```bash
git add src/generation/ src/gateway/handlers/generation_providers.rs
git commit -m "voice: add list_voices() to GenerationProvider trait and RPC endpoint"
```

---

### Task 9: Panel UI — Voice Configuration

**Files:**
- Modify: `interfaces/webchat/src/views/settings/providers.rs` — add voice config to detail panel
- Modify: `interfaces/webchat/src/api/generation_providers.rs` — add voices API

**Note:** This task involves Leptos/WASM code. Read the existing provider detail form code carefully before modifying.

- [ ] **Step 1: Add voices API client**

In `interfaces/webchat/src/api/generation_providers.rs`, add:
```rust
/// Fetch available voices for a provider
pub async fn voices(state: &AppState, provider_id: &str) -> Result<Vec<VoiceInfo>, ApiError> {
    let params = serde_json::json!({ "provider_id": provider_id });
    state.rpc_call("generation_providers.voices", Some(params)).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub gender: String,
    pub description: String,
}
```

- [ ] **Step 2: Add voice config section to provider detail form**

In the provider detail panel (right side of providers.rs), when the selected provider has `Speech` capability, add UI controls:

1. **Default Voice dropdown** — populated from `generation_providers.voices` API
2. **Default Speed slider** — range 0.25 to 4.0, step 0.25
3. **Default Format dropdown** — mp3, opus, aac, flac (filtered by provider)
4. **Test Voice button** — calls `speech_generate` and plays result in `<audio>` tag

These controls read/write the provider's `defaults.voice`, `defaults.speed`, and `defaults.format` fields via the existing `generation_providers.update` RPC call.

Follow the existing pattern for other settings controls (dropdowns, sliders) in the same file.

- [ ] **Step 3: Build WASM**

Run: `just build-wasm` (or equivalent)
Expected: Compiles successfully

- [ ] **Step 4: Manual test in browser**

1. Open Panel → Settings → Providers
2. Select a Speech provider (OpenAI TTS or ElevenLabs)
3. Verify voice dropdown shows available voices with name + gender
4. Select a voice, set speed, save
5. Click "Test Voice" — verify audio plays in browser

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/
git commit -m "panel: add voice configuration UI to speech provider detail form"
```

---

### Task 10: End-to-End Integration Test

**Files:** No new files — integration testing

- [ ] **Step 1: Build full project**

Run: `just build`
Expected: Clean build

- [ ] **Step 2: Run all tests**

Run: `just test-all`
Expected: All existing tests pass + new voice tests pass

- [ ] **Step 3: Manual E2E test**

1. Start aleph-server
2. Configure a Speech provider in Panel (OpenAI TTS with API key)
3. Set default voice in Panel
4. Via Telegram: send a voice message → verify text reply + voice reply
5. Via Telegram: say "turn on voice mode" → verify LLM calls voice_mode_set tool
6. Via Telegram: send text → verify both text and voice reply
7. Via Telegram: say "turn off voice mode" → verify text-only replies resume
8. Verify intermediate status messages are voiced (tool use narration)

- [ ] **Step 4: Test degradation**

1. Disable TTS provider → verify text-only fallback with notification
2. Use a channel without audio capability → verify voice_mode_set returns error
3. Simulate TTS failure → verify auto-disable after 3 failures

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "voice: complete voice conversation system integration"
```

---

## Appendix: API Corrections

The code snippets above use simplified API signatures. The implementer MUST adjust for the actual APIs below. These corrections apply across multiple tasks.

### A1. Attachment requires `id` field

All `Attachment` constructions must include `id: String`:
```rust
Attachment {
    id: uuid::Uuid::new_v4().to_string(), // or meaningful ID
    mime_type: "audio/mp3".to_string(),
    // ... rest of fields
}
```

### A2. MediaInput uses named-field variants

```rust
// WRONG: MediaInput::FilePath(PathBuf)
// CORRECT:
MediaInput::FilePath { path: PathBuf::from(path) }
MediaInput::Url { url: url.clone() }
MediaInput::Base64 { data: data.clone(), media_type: MediaType::Audio { format: AudioFormat::Ogg, duration_secs: None } }
```

### A3. MediaType::Audio requires `format` field

```rust
// WRONG: MediaType::Audio { duration_secs: None }
// CORRECT:
MediaType::Audio { format: AudioFormat::Ogg, duration_secs: None }
```

Detect AudioFormat from MIME type:
```rust
fn audio_format_from_mime(mime: &str) -> AudioFormat {
    match mime {
        "audio/mp3" | "audio/mpeg" => AudioFormat::Mp3,
        "audio/ogg" => AudioFormat::Ogg,
        "audio/wav" => AudioFormat::Wav,
        "audio/flac" => AudioFormat::Flac,
        "audio/m4a" | "audio/mp4" => AudioFormat::M4a,
        _ => AudioFormat::Mp3, // fallback
    }
}
```

### A4. MediaOutput::Text uses named field

```rust
// WRONG: MediaOutput::Text(text) => Ok(text)
// CORRECT:
MediaOutput::Text { text } => Ok(text),
```

### A5. GenerationProviderRegistry — no `.generate()` method

Get provider first, then call generate:
```rust
// WRONG: generation_registry.generate(request).await
// CORRECT:
let provider_id = voice_state.provider.as_deref().unwrap_or("default-speech");
let provider = generation_registry.get(provider_id)
    .ok_or("No speech provider configured")?;
let output = provider.generate(request).await?;
```

Note: The caller must determine which provider ID to use. Check `GenerationConfig` for `default_speech_provider` if `VoiceState.provider` is None.

### A6. GenerationRequest has no `provider_id` field

```rust
// CORRECT construction:
let request = GenerationRequest {
    generation_type: GenerationType::Speech,
    prompt: text.to_string(),
    params,
    request_id: None,
    user_id: None,
};
```

### A7. GenerationOutput.data is a `GenerationData` enum, not Option

```rust
// WRONG: if let Some(data) = output.data { ... }
// CORRECT:
match output.data {
    GenerationData::Bytes(bytes) => {
        let format = output.metadata.format.as_deref().unwrap_or("mp3");
        Some(Attachment {
            id: uuid::Uuid::new_v4().to_string(),
            mime_type: format!("audio/{}", format),
            filename: Some(format!("voice_reply.{}", format)),
            size: Some(bytes.len() as u64),
            url: None,
            path: None,
            data: Some(bytes),
        })
    }
    GenerationData::Url(url) => {
        Some(Attachment {
            id: uuid::Uuid::new_v4().to_string(),
            mime_type: "audio/mp3".to_string(),
            filename: Some("voice_reply.mp3".to_string()),
            size: None,
            url: Some(url),
            path: None,
            data: None,
        })
    }
    GenerationData::LocalPath(path) => {
        Some(Attachment {
            id: uuid::Uuid::new_v4().to_string(),
            mime_type: "audio/mp3".to_string(),
            filename: Some("voice_reply.mp3".to_string()),
            size: None,
            url: None,
            path: Some(path),
            data: None,
        })
    }
}
```

### A8. ChannelRegistry has no `get_capabilities()` method

Add this method to `ChannelRegistry`:
```rust
/// Get channel capabilities (if channel exists)
pub async fn get_capabilities(&self, channel_id: &ChannelId) -> Option<ChannelCapabilities> {
    let channels = self.channels.read().await;
    channels.get(channel_id.as_str())
        .map(|handle| handle.channel.capabilities())
}
```

Or use the existing channel handle access pattern in the codebase.

### A9. ChannelRegistry uses `tokio::sync::RwLock`, not `crate::sync_primitives::RwLock`

```rust
// WRONG: use crate::sync_primitives::RwLock;
// CORRECT: use tokio::sync::RwLock; (already imported in channel_registry.rs)
// voice_states field uses the same RwLock as channels field
voice_states: tokio::sync::RwLock<HashMap<String, VoiceState>>,
```

### A10. MediaPipeline.process() takes references

```rust
// WRONG: media_pipeline.process(input, media_type, None).await
// CORRECT:
media_pipeline.process(&input, &media_type, None).await
```
