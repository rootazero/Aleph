# LLM Provider Configuration

## Provider Protocols

| Protocol | Default base_url | Endpoint built as |
|----------|-----------------|-------------------|
| `"openai"` (default) | `https://api.openai.com/v1` | `{base}/v1/chat/completions` (or `v3` if URL contains `/v3`) |
| `"anthropic"` | `https://api.anthropic.com` | `{base}/v1/messages` |
| `"gemini"` | `https://generativelanguage.googleapis.com` | `{base}/v1beta/models/{model}:generateContent` |
| `"ollama"` | `http://localhost:11434` | `{base}/api/generate` |
| `"codex"` | `https://chatgpt.com` | `{base}/backend-api/codex/responses` |

## base_url Normalization

All protocols normalize the URL before appending the endpoint:
1. Strip trailing `/`
2. Strip trailing version suffix (`/v1`, `/v3`)
3. Re-append correct version + endpoint path

`https://api.openai.com` and `https://api.openai.com/v1` produce the same result.

**Special**: doubao/volcengine/ark URL `https://ark.cn-beijing.volces.com/api/v3` is detected as v3 and uses `v3/chat/completions`.

## Known Presets

When a provider name matches a preset, `base_url`, `protocol`, `color`, and default model auto-fill. User config overrides.

| Name | base_url | Protocol | Default Model |
|------|----------|----------|---------------|
| `openai` | `https://api.openai.com/v1` | openai | gpt-4o |
| `chatgpt` | `https://chatgpt.com` | codex | gpt-5.4 |
| `deepseek` | `https://api.deepseek.com` | openai | deepseek-chat |
| `claude` | `https://api.anthropic.com` | anthropic | claude-sonnet-4-5-20250514 |
| `gemini` | `https://generativelanguage.googleapis.com` | gemini | gemini-2.5-flash |
| `moonshot` / `kimi` | `https://api.moonshot.cn/v1` | openai | moonshot-v1-8k |
| `doubao` / `volcengine` / `ark` | `https://ark.cn-beijing.volces.com/api/v3` | openai | doubao-1.5-pro-256k |
| `siliconflow` | `https://api.siliconflow.cn/v1` | openai | deepseek-ai/DeepSeek-V3 |
| `zhipu` / `glm` | `https://open.bigmodel.cn/api/paas/v4` | openai | GLM-5 |
| `minimax` | `https://api.minimax.io/v1` | openai | MiniMax-M2.5 |
| `groq` | `https://api.groq.com/openai/v1` | openai | llama-3.3-70b-versatile |
| `openrouter` | `https://openrouter.ai/api/v1` | openai | anthropic/claude-sonnet-4-5 |
| `t8star` | `https://api.t8star.cn/v1` | openai | (none) |
| `together` | `https://api.together.xyz/v1` | openai | (none) |
| `perplexity` | `https://api.perplexity.ai` | openai | (none) |
| `mistral` | `https://api.mistral.ai/v1` | openai | (none) |
| `fireworks` | `https://api.fireworks.ai/inference/v1` | openai | (none) |

Preset providers only need `models`, `enabled = true`, and vault key:

```toml
[providers.deepseek]
models = ["deepseek-chat"]
enabled = true
```

## Complete Template (all fields)

```toml
[providers.MyProvider]
protocol = "openai"                     # REQUIRED: "openai" | "anthropic" | "gemini" | "ollama" | "codex"
base_url = "https://api.example.com/v1" # Optional: override default for protocol
models = ["model-name"]                 # REQUIRED: array of model names (first = default)
enabled = true                          # REQUIRED: default is false, must explicitly enable
verified = false                        # Optional: set after successful test
timeout_seconds = 300                   # Optional: default 300
color = "#808080"                       # Optional: hex color for UI

# Common parameters (all optional)
max_tokens = 4096                       # Max tokens in response
temperature = 0.7                       # 0.0-2.0 (OpenAI/Gemini), 0.0-1.0 (Claude)
top_p = 0.9                             # Nucleus sampling, 0.0-1.0
top_k = 40                              # Claude, Gemini, Ollama only

# Protocol-specific (all optional)
frequency_penalty = 0.0                 # OpenAI only, -2.0 to 2.0
presence_penalty = 0.0                  # OpenAI only, -2.0 to 2.0
stop_sequences = "###,END"              # Claude/Gemini/Ollama, comma-separated
thinking_level = "HIGH"                 # Gemini 3 only: "LOW" | "HIGH"
media_resolution = "HIGH"               # Gemini only: "LOW" | "MEDIUM" | "HIGH"
repeat_penalty = 1.1                    # Ollama only
system_prompt_mode = "prepend"          # "prepend" (default) | "standard"
```

## Field Pitfalls

| Correct | WRONG (silently ignored) |
|---------|--------------------------|
| `protocol = "openai"` | ~~`provider_type = "openai"`~~ (that's for generation providers) |
| `models = ["gpt-4o"]` | ~~`model = "gpt-4o"`~~ (works as alias but array is canonical) |
| `enabled = true` | (omitting it = disabled by default!) |
| `base_url = "https://..."` | ~~`url = "..."`~~, ~~`endpoint = "..."`~~ |
