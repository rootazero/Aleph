# Generation Providers Configuration Guide

## File Path
- Main: `~/.aleph/config.toml` section `[generation]`
- API keys: encrypted vault (key: `gen:{name}`)

## Operation Rules
1. Before modification: `cp ~/.aleph/config.toml ~/.aleph/config.toml.bak`
2. API keys via `vault_store(action="store", key="gen:<name>", secret="<key>")`
3. After modification: auto-reloads via fswatch

## Structure

```toml
[generation]
# Default provider per generation type (NOTE: full `_provider` suffix)
default_image_provider = "stability"
default_speech_provider = "openai-tts"
default_video_provider = "veo"
default_audio_provider = "my-audio"

[generation.providers.stability]
provider_type = "stability"          # REQUIRED, exact id (NOT `type` / `provider`)
base_url = "https://api.stability.ai/v2beta"
models = ["sd3-large"]               # array (NOT singular `model`)
capabilities = ["image"]             # array: "image"|"video"|"speech"|"audio"
enabled = true                       # default true (unlike LLM providers)
timeout_seconds = 120                # default 120 (unlike LLM 300)
# api_key — DO NOT SET HERE, use vault_store with key "gen:stability"

[generation.providers.openai-tts]
provider_type = "openai_tts"         # aliases: tts
models = ["tts-1-hd"]
capabilities = ["speech"]

[generation.providers.openai-tts.defaults]
voice = "alloy"            # alloy | echo | fable | onyx | nova | shimmer

[generation.providers.openai-dall-e]
provider_type = "openai"             # aliases: openai_image, dalle
models = ["dall-e-3"]
capabilities = ["image"]

[generation.providers.openai-dall-e.defaults]
width = 1024
height = 1024
quality = "hd"            # standard | hd
style = "vivid"           # vivid | natural
```

> Typed maps (`[generation.image_providers.<name>]`, `video_providers`,
> `speech_providers`, `audio_providers`) are an alternative to the generic
> `[generation.providers.<name>]` map and auto-set `capabilities` from the
> section name. See [references/generation-providers.md](../../skills/self/references/generation-providers.md)
> for the exact `provider_type` values and per-type `defaults` fields.

## Common Operations

### Add image generation provider
1. Add `[generation.providers.<name>]` with `provider_type = "<type>"` and `capabilities = ["image"]`
2. Store API key: `vault_store(action="store", key="gen:<name>", secret="...")`
3. Set as default: `generation.default_image_provider = "<name>"`

### Add speech provider
1. Add `[generation.providers.<name>]` with `provider_type = "<type>"` and `capabilities = ["speech"]`
2. Store API key via vault_store
3. Set as default: `generation.default_speech_provider = "<name>"`

### Remove a generation provider
1. Remove section from config.toml
2. Delete API key: `vault_store(action="delete", key="gen:<name>")`

## Caveats
- `provider_type` is REQUIRED and must be a non-empty exact id; `type` / `provider` are silently ignored. Config load only checks it is non-empty — an unknown id is rejected later, when the provider is first built/used (a `GenerationError`), not at startup.
- `models` is an array; singular `model` works as an alias but the array is canonical.
- `capabilities` is an array — one or more of `"image"`, `"video"`, `"speech"`, `"audio"`, `"transcription"`.
- Default-selection fields carry the full `_provider` suffix: `default_image_provider`, `default_speech_provider`, `default_video_provider`, `default_audio_provider`, `default_transcription_provider` (NOT `default_image` etc.).
- API keys use `gen:{name}` vault convention (not `ai:{name}`).
- Generation providers need a restart after adding/modifying.
