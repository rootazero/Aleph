# Generation Provider Configuration

## provider_type Values (EXACT — unknown values cause startup error)

| provider_type | Aliases | Category | Notes |
|--------------|---------|----------|-------|
| `"openai"` | `"openai_image"`, `"dalle"` | Image | OpenAI DALL-E |
| `"stability"` | `"stability_image"`, `"sdxl"` | Image | Stability AI |
| `"google"` | `"google_imagen"`, `"imagen"` | Image | Google Imagen |
| `"midjourney"` | `"mj"` | Image | Midjourney API |
| `"replicate"` | — | Image | Replicate API |
| `"openai_compat"` | — | Any | Generic OpenAI-compatible (`base_url` REQUIRED) |
| `"openai_tts"` | `"tts"` | Speech | OpenAI TTS |
| `"elevenlabs"` | — | Speech | ElevenLabs TTS |
| `"google_veo"` | `"veo"` | Video | Google Veo |

Unknown `provider_type` causes startup error with supported values listed.

## base_url: ResolvedUrl Rules

**Rule**: Strip scheme (`https://`). No `/` in remaining string, or ends with `/v1` → auto-append endpoint. Anything else → use as-is.

| URL | Detection | Result |
|-----|-----------|--------|
| `https://api.example.com` | No `/` after scheme | **Standard** — auto-append `/v1/images/generations` etc. |
| `https://api.example.com/v1` | Ends with `/v1` | **Standard** — strip `/v1`, then auto-append |
| `https://api.example.com/v1/` | Trailing `/` stripped, then ends `/v1` | **Standard** |
| `https://ai.t8star.cn/v2/videos/generations` | Has `/v2/...` | **Custom** — use as-is |
| `https://api.example.com/custom/tts` | Has `/custom/...` | **Custom** — use as-is |
| `https://api.example.com/v1beta/models` | Has `/v1beta/...` (not `/v1`) | **Custom** — use as-is |

**Simple rule**: bare domain or domain+`/v1` = auto-append. Any other path = full URL, no change.

### Standard auto-appended paths

| GenerationType | Primary | Secondary |
|---------------|---------|-----------|
| Image | `/v1/images/generations` | `/v1/images/edits` |
| Video | `/v1/videos/generations` | (none) |
| Speech | `/v1/audio/speech` | `/v1/audio/transcriptions` |
| Audio | `/v1/audio/generations` | (none) |

Custom URLs have no secondary endpoint (no image edit, no STT).

## TOML Field Reference

| Correct field | WRONG (silently ignored) |
|---------------|--------------------------|
| `provider_type = "openai_image"` | ~~`provider = "..."`~~, ~~`type = "..."`~~, ~~`protocol = "..."`~~ |
| `models = ["model-name"]` | ~~`model = "..."`~~ (alias works but array is canonical) |
| `capabilities = ["image"]` | ~~`type = "image"`~~, ~~`capability = "image"`~~ |
| `base_url = "https://..."` | ~~`url = "..."`~~, ~~`endpoint = "..."`~~ |
| `edit_url = "https://..."` | Only for `openai_compat` |
| `default_video_provider = "name"` | ~~`default_video = "..."`~~ |
| `default_image_provider = "name"` | ~~`default_image = "..."`~~ |
| `default_speech_provider = "name"` | ~~`default_speech = "..."`~~ |
| `default_audio_provider = "name"` | ~~`default_audio = "..."`~~ |

`capabilities` values: `"image"`, `"video"`, `"speech"`, `"audio"` (only these 4).

## Complete Template

```toml
[generation.providers.MyProvider]
provider_type = "openai_image"          # REQUIRED: see table above
base_url = "https://api.example.com/v1" # Optional (required for openai_compat)
models = ["dall-e-3"]                   # Recommended: array
capabilities = ["image"]                # Recommended: "image"|"video"|"speech"|"audio"
enabled = true                          # Default true (unlike LLM providers!)
timeout_seconds = 120                   # Default 120 (unlike LLM 300)
color = "#808080"                       # Hex #RGB or #RRGGBB
verified = false
edit_url = "https://..."                # Only for openai_compat image edit

[generation.providers.MyProvider.defaults]
width = 1024                            # Pixels (1-8192)
height = 1024
aspect_ratio = "16:9"
quality = "hd"                          # "standard", "hd"
style = "vivid"                         # "vivid", "natural"
n = 1                                   # Outputs (1-10)
format = "png"                          # "png", "webp", "mp4"
duration_seconds = 5.0                  # Video (>0)
fps = 24                                # Video (1-120)
voice = "alloy"                         # TTS voice ID
speed = 1.0                             # TTS (0.25-4.0)
language = "en"
guidance_scale = 7.5                    # CFG (0.0-30.0)
steps = 50                              # Inference (1-150)
```

## Field Pitfalls

| Issue | Detail |
|-------|--------|
| `enabled` default differs | LLM: `false`, Generation: `true` |
| `timeout_seconds` default differs | LLM: 300s, Generation: 120s |
| `provider_type` vs `protocol` | Generation uses `provider_type`, LLM uses `protocol`. Never mix! |
| `capabilities` must be array | `["image"]` not `"image"` |
| `openai_compat` requires `base_url` | Startup error without it |
| Color format | Must be `#RGB` or `#RRGGBB` |

## Typed Provider Maps (alternative to legacy)

Auto-set capabilities by section name. Typed maps take priority over `providers` on name collision.

```toml
[generation.image_providers.dalle]
provider_type = "openai_image"
models = ["dall-e-3"]
# capabilities auto-set to ["image"]

[generation.video_providers.veo]
provider_type = "google_veo"
models = ["veo-2"]

[generation.speech_providers.openai-tts]
provider_type = "openai_tts"
models = ["tts-1-hd"]

[generation.audio_providers.my-audio]
provider_type = "openai_compat"
base_url = "https://api.example.com/v1/audio/generations"
models = ["audio-model"]
```

## Step-by-step: Add a generation provider

**Step 1**: Store API key
```
vault_store(action="store", key="gen:<Name>", secret="<key>")
```

**Step 2**: Find insertion point
```
bash(grep -n '^\[generation\]\|^\[group_chat\]\|^\[tools\]\|^\[mcp\]' ~/.aleph/config.toml)
```

**Step 3**: Insert with python3
```
bash(python3 -c "
import re
with open('$HOME/.aleph/config.toml', 'r') as f:
    content = f.read()
new_section = '''
[generation.providers.<NAME>]
provider_type = \"<TYPE>\"
base_url = \"<URL>\"
models = [\"<MODEL>\"]
capabilities = [\"<CAPABILITY>\"]
enabled = true
timeout_seconds = 120
color = \"#808080\"
'''
pattern = r'(\[(?:group_chat|tools|mcp)\])'
match = re.search(pattern, content)
if match:
    pos = match.start()
    content = content[:pos] + new_section + '\n' + content[pos:]
else:
    content += new_section
with open('$HOME/.aleph/config.toml', 'w') as f:
    f.write(content)
print('Done')
")
```

**Step 4**: Verify
```
bash(grep -A8 '<NAME>' ~/.aleph/config.toml)
```

Tell user: "Please restart Aleph for the new generation provider to take effect."
