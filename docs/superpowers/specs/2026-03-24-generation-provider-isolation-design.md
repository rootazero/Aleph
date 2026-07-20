# Generation Provider Isolation & URL Normalization Design

**Date**: 2026-03-24
**Status**: Approved
**Scope**: Generation provider config restructure (4-category isolation) + URL auto-completion

## Problem

1. **Category confusion**: All providers configured under `[generation.providers.*]` with `capabilities = [...]` array. Easy to misconfigure (e.g., speech provider gets video URL).
2. **URL construction inconsistent**: `openai_compat` has smart normalization, `openai_tts`/`openai_image` have hardcoded path append — no shared logic.
3. **No multi-endpoint support**: Each type may need multiple endpoints (image: generate + edit; speech: TTS + STT), but current design stores one URL per provider.

## Design Decisions

1. **Config split into 4 typed sections**: `image_providers`, `video_providers`, `speech_providers`, `audio_providers`
2. **Type determined by section name**: No `capabilities` field needed in new format
3. **Backward compatible**: Old `[generation.providers.*]` + `capabilities` still works, auto-mapped
4. **Smart URL resolution**: Standard base URL → auto-derive all operation endpoints; custom full URL → use as-is
5. **URL auto-complete rule**: domain-only or domain+`/v1` = standard → auto-complete; anything else = custom → don't touch

## Config Structure

### New format (preferred)

```toml
[generation]
default_image_provider = "T8StariMage"
default_video_provider = "T8StarVideo"
default_speech_provider = "T8Star"
default_audio_provider = "SunoAI"

[generation.image_providers.T8StariMage]
base_url = "https://ai.t8star.cn"
provider_type = "openai_image"
models = ["nano-banana-2-4k"]

[generation.video_providers.T8StarVideo]
base_url = "https://ai.t8star.cn/v2/videos/generations"
provider_type = "openai_compat"
models = ["veo3.1-pro-4k"]
timeout_seconds = 300

[generation.speech_providers.T8Star]
base_url = "https://ai.t8star.cn"
provider_type = "openai_tts"
models = ["tts-1-hd"]
voice = "alloy"

[generation.audio_providers.SunoAI]
base_url = "https://api.suno.com"
provider_type = "openai_compat"
models = ["suno-v4"]
```

### Old format (backward compatible, deprecated)

```toml
[generation.providers.T8Star]
base_url = "https://ai.t8star.cn"
capabilities = ["speech"]
provider_type = "openai_tts"
```

Old format parsed → mapped to typed section based on `capabilities[0]` → registered normally.

## URL Resolution

### Auto-complete rule

```rust
fn needs_auto_complete(url: &str) -> bool {
    let trimmed = url.trim_end_matches('/');
    let after_scheme = trimmed
        .strip_prefix("https://").or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    // Standard: domain-only (no /) or domain+/v1
    !after_scheme.contains('/') || after_scheme.ends_with("/v1")
}
```

| User input | `needs_auto_complete` | Result |
|---|---|---|
| `https://api.example.com` | `true` | Standard base |
| `https://api.example.com/v1` | `true` | Standard base |
| `https://api.example.com/v1/` | `true` | (trim `/` first) |
| `https://api.example.com/v2/videos/generations` | `false` | Custom URL |
| `https://custom.api.com/my/tts` | `false` | Custom URL |

### ResolvedUrl type

```rust
pub enum ResolvedUrl {
    /// Standard OpenAI-compatible base URL. All operation endpoints derived automatically.
    Standard(String),
    /// Custom full URL. Used as-is for primary operation only.
    Custom(String),
}

pub fn resolve_base_url(url: &str) -> ResolvedUrl {
    let trimmed = url.trim_end_matches('/');
    if needs_auto_complete(trimmed) {
        let base = trimmed.trim_end_matches("/v1").trim_end_matches('/');
        ResolvedUrl::Standard(base.to_string())
    } else {
        ResolvedUrl::Custom(trimmed.to_string())
    }
}
```

### Per-type endpoint derivation

**Image provider** (Standard base → 2 endpoints):
- `{base}/v1/images/generations` — image generation
- `{base}/v1/images/edits` — image editing

**Video provider** (Standard base → 1 endpoint):
- `{base}/v1/videos/generations` — video generation

**Speech provider** (Standard base → 2 endpoints):
- `{base}/v1/audio/speech` — TTS
- `{base}/v1/audio/transcriptions` — STT

**Audio provider** (Standard base → 1 endpoint):
- `{base}/v1/audio/generations` — audio/music generation

Custom URL → primary operation only, secondary operations unavailable.

## Registration Flow

```
Config parsed
    ↓
Old format: [generation.providers.*] + capabilities
    → Map to typed section by capabilities[0]
    ↓
New format: [generation.image_providers.*] etc.
    → Type determined by section name
    ↓
For each provider in each typed section:
    1. resolve_base_url(base_url) → ResolvedUrl
    2. create_provider(name, config, gen_type, resolved_url)
    3. registry.register(provider) with forced GenerationType
```

## Files to Create/Modify

| Action | File | Description |
|--------|------|-------------|
| Create | `src/generation/providers/url_normalize.rs` | `ResolvedUrl`, `resolve_base_url()`, `needs_auto_complete()` |
| Modify | `src/config/types/generation/*.rs` | Add `image_providers`, `video_providers`, `speech_providers`, `audio_providers` fields |
| Modify | `src/generation/providers/factory.rs` | Accept `GenerationType` param, use `ResolvedUrl` |
| Modify | `src/generation/providers/openai_tts.rs` | Use `ResolvedUrl` for `speech_url()` and new `stt_url()` |
| Modify | `src/generation/providers/openai_image.rs` | Use `ResolvedUrl` for `generations_url()` and new `edits_url()` |
| Modify | `src/generation/providers/openai_compat/builder.rs` | Replace private `normalize_endpoint()` with shared `resolve_base_url()` |
| Modify | `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Iterate 4 typed sections + legacy merge |

## YAGNI

- No Panel UI changes in this spec (config structure change is backend-only; Panel reads from config)
- No new provider types
- No migration tool for old → new config format (both work simultaneously)
