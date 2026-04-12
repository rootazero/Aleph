# Transcription Provider Separation Design

**Date**: 2026-04-03
**Status**: Approved
**Scope**: Separate Whisper/STT from Speech/TTS into an independent generation type

## Problem

Speech providers currently bundle TTS (text-to-speech) and STT (speech-to-text) into a single configuration. The system auto-derives the STT endpoint from the TTS base URL (e.g., `https://ai.t8star.cn/v1/audio/speech` → `https://ai.t8star.cn/v1/audio/transcriptions`). However, not all providers offer both capabilities — some only provide TTS, others only STT. This coupling forces an assumption that doesn't hold universally.

## Solution

Introduce `Transcription` as the fifth `GenerationType`, peer to Image / Video / Audio / Speech. Each type gets its own independent provider configuration and Panel UI tab.

## Design

### 1. Data Model Changes

#### 1.1 GenerationType enum

**File**: `src/generation/types/generation_type.rs`

Add `Transcription` variant:

```rust
pub enum GenerationType {
    Image,
    Video,
    Audio,
    Speech,
    Transcription,  // NEW
}
```

Update helper methods:
- `supports_language()` → returns `true` for `Transcription` (and `Speech` if needed)
- `display_name()` → returns `"Transcription"`
- `is_long_running()` → `false` for Transcription (typically fast)
- `supports_voice()` → `false` for Transcription

#### 1.2 GenerationConfig

**File**: `src/config/types/generation/config.rs`

Add two fields:

```rust
pub default_transcription_provider: Option<String>,
pub transcription_providers: HashMap<String, GenerationProviderConfig>,
```

Update all methods that iterate typed maps:
- `get_default_provider()` — add `Transcription` match arm
- `get_provider()` — include `transcription_providers` in lookup chain
- `get_enabled_providers()` — include `transcription_providers`
- `get_providers_for_type()` — map `Transcription` to `transcription_providers`
- `merged_providers()` — add `transcription_providers` iteration block
- `validate()` — validate `default_transcription_provider` and all `transcription_providers` entries

#### 1.3 GenerationDefaults

**File**: `src/config/types/generation/defaults.rs`

Remove `stt_model` field — Transcription providers use their own `model` field in `GenerationProviderConfig`.

Keep `language` — shared by both Speech and Transcription contexts.

### 2. Service Layer Changes

#### 2.1 TranscriptionService configuration source

**File**: `src/bin/aleph-server/commands/start/builder/agent_init.rs` (lines ~920-958)

Replace current logic that borrows API key from LLM providers:

**Before**: Find an OpenAI-compatible LLM provider and use its `api_key` + `base_url`.

**After**: Read from `app_config.generation.transcription_providers` using `default_transcription_provider` to select the provider. Construct `WhisperTranscription` from the provider's own `api_key`, `base_url`, and `model`.

If no transcription provider is configured, `TranscriptionService` remains `None` (graceful degradation).

#### 2.2 OpenAiTtsProvider cleanup

**File**: `src/generation/providers/openai_tts.rs`

- Delete `stt_url()` method
- Remove any `secondary_endpoint(GenerationType::Speech)` logic

Speech providers are now purely TTS — they have no knowledge of STT.

### 3. Panel UI Changes

**File**: `interfaces/webchat/src/views/settings/generation_providers.rs`

#### 3.1 New Transcription tab

Add a fifth category tab alongside Image / Video / Audio / Speech. The tab label is "Transcription".

#### 3.2 Transcription provider form

Minimal field set:
- **API Key** — SecretInput
- **Base URL** — text input
- **Model** — text input (default: `whisper-1`)
- **Language** — text input (optional, e.g., `zh`, `en`)
- **Timeout** — number input
- **Enable** — toggle

Does NOT include: voice, speed, format (TTS-only fields).

#### 3.3 Speech provider form: voices URL field

Add a **Voices URL** input field to the Speech provider form. This endpoint is used to fetch available voices for the TTS provider.

URL completion rules (consistent with other URL input fields in the system):
- **Default**: auto-complete from base URL → `{base_url}/v1/audio/voices`
- **Full URL override**: if the user enters a complete URL (starts with `http://` or `https://`), use it as-is without appending any path

Example: if base URL is `https://ai.t8star.cn`, the voices URL defaults to `https://ai.t8star.cn/v1/audio/voices`. If the user explicitly enters `https://other-service.com/custom/voices`, that full URL is used directly.

The `voices_url` field is stored in `GenerationProviderConfig` (optional, `Option<String>`).

#### 3.4 Speech provider form cleanup

Remove from Speech provider detail view:
- `stt_model` input field
- Any STT-related configuration UI

#### 3.5 API layer

The existing Panel API client routes CRUD operations by `GenerationType`. Adding `Transcription` to the enum should naturally extend the API without new endpoints.

### 4. Configuration Format

Example `~/.aleph/config.toml`:

```toml
[generation]
default_speech_provider = "t8star"
default_transcription_provider = "t8star-stt"

# Speech (TTS only)
[generation.speech_providers.t8star]
provider_type = "openai_compat"
api_key = "sk-xxx"
base_url = "https://ai.t8star.cn/v1/audio/speech"
model = "tts-1"
voices_url = "https://ai.t8star.cn/v1/audio/voices"  # optional, auto-derived from base_url if omitted
enabled = true

# Transcription (STT, independent)
[generation.transcription_providers.t8star-stt]
provider_type = "openai_compat"
api_key = "sk-xxx"
base_url = "https://ai.t8star.cn/v1/audio/transcriptions"
model = "whisper-1"
enabled = true
```

### 5. Change Summary

| Layer | File | Change |
|-------|------|--------|
| Type | `generation_type.rs` | Add `Transcription` variant + helper methods |
| Config | `config.rs` | Add `default_transcription_provider` + `transcription_providers` |
| Config | `defaults.rs` | Remove `stt_model` field |
| Config | `provider.rs` | Add `voices_url: Option<String>` field |
| Provider | `openai_tts.rs` | Remove `stt_url()` method, use `voices_url` for voice list |
| Service | `agent_init.rs` | Read config from transcription provider instead of LLM provider |
| Panel | `generation_providers.rs` | Add Transcription tab + form, clean Speech form |

### 6. Non-Goals

- No migration logic (project not yet released, manual config edit only)
- No advanced transcription parameters (response format, temperature) — keep minimal
- No new `GenerationProvider` trait implementation for transcription — `WhisperTranscription` remains a `TranscriptionService` impl, just with a different config source
