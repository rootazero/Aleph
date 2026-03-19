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
# Default providers per generation type
default_image = "stability"
default_speech = "openai-tts"
default_video = "runway"
default_audio = "suno"

[generation.providers.stability]
type = "image"
provider = "stability"     # Provider implementation
base_url = "https://api.stability.ai/v2beta"
model = "sd3-large"
# api_key — DO NOT SET HERE, use vault_store with key "gen:stability"

[generation.providers.openai-tts]
type = "speech"
provider = "openai"
model = "tts-1-hd"
voice = "alloy"            # alloy | echo | fable | onyx | nova | shimmer

[generation.providers.openai-dall-e]
type = "image"
provider = "openai"
model = "dall-e-3"
size = "1024x1024"         # 256x256 | 512x512 | 1024x1024 | 1792x1024 | 1024x1792
quality = "hd"             # standard | hd
```

## Common Operations

### Add image generation provider
1. Add `[generation.providers.<name>]` with `type = "image"`
2. Store API key: `vault_store(action="store", key="gen:<name>", secret="...")`
3. Set as default: `generation.default_image = "<name>"`

### Add speech provider
1. Add `[generation.providers.<name>]` with `type = "speech"`
2. Store API key via vault_store
3. Set as default: `generation.default_speech = "<name>"`

### Remove a generation provider
1. Remove section from config.toml
2. Delete API key: `vault_store(action="delete", key="gen:<name>")`

## Caveats
- Generation types: "image", "speech", "audio", "video"
- Each type can have multiple providers; `default_*` sets which is used
- API keys use `gen:{name}` vault convention (not `provider:{name}`)
