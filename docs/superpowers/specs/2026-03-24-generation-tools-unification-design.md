# Generation Tools Unification & Token Budget Self-Reflection

**Date**: 2026-03-24
**Status**: Approved
**Scope**: Generation tool system, slash commands, agent loop prompt

## Problem

When a user asks to generate a video via Telegram, the agent consumed 500K tokens across 20 tool calls (skill_list, web_fetch, read_config_guide, vault_store, 15x bash) but **never called `generate_video`**. The token budget was exhausted (`hit_limit=true`) and the user received no response.

Root causes:
1. **Tool quality asymmetry**: `image_generate` is a proper AlephTool; `generate_video` and `generate_audio` are legacy handlers with hardcoded JSON schema and manual argument parsing
2. **No explicit commands**: Users have no fast path to trigger generation directly
3. **No budget awareness**: The agent has no prompt-level guidance to avoid endless exploration when the task maps directly to a tool
4. **Inadequate hit_limit feedback**: When token budget is exhausted, the existing fallback message is generic and doesn't guide the user toward efficient alternatives (e.g., slash commands)

## Design

Three layers, prioritized: Layer 1 (core) → Layer 3 (budget) → Layer 2 (commands).

---

### Layer 1: Generation Tool Unification

#### 1.1 Upgrade Video/Audio/Speech to AlephTool

Create proper AlephTool implementations matching `image_generate.rs` pattern.

**All tools must use `Arc<RwLock<GenerationProviderRegistry>>`** to match the builder's storage type and `ImageGenerateTool`'s pattern. The existing `SpeechGenerateTool` uses bare `Arc<>` and must be fixed.

**All tools should include `notify_tool_start` / `notify_tool_result`** progress notifications matching `image_generate.rs` for UX consistency.

**`video_generate.rs`** (new):
```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VideoGenerateArgs {
    pub prompt: String,
    pub provider: Option<String>,
    pub aspect_ratio: Option<String>,  // "16:9", "9:16"
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoGenerateOutput {
    pub video_location: String,
    pub location_type: String,  // "url" | "file"
    pub prompt: String,
    pub provider: String,
    pub model: Option<String>,
    pub generation_duration_ms: u64,  // wall-clock time for generation, NOT content duration
}

impl AlephTool for VideoGenerateTool {
    const NAME: &'static str = "video_generate";
    const DESCRIPTION: &'static str = "Generate videos from text descriptions";
    type Args = VideoGenerateArgs;
    type Output = VideoGenerateOutput;
}
```

**`audio_generate.rs`** (new):
```rust
pub struct AudioGenerateArgs {
    pub prompt: String,
    pub provider: Option<String>,
}
// Output: audio_location, location_type, prompt, provider, model, generation_duration_ms
```

**`speech_generate.rs`** (existing AlephTool impl — fix `Arc<RwLock<>>` wrapping + register):

Note: `SpeechGenerateTool` already has a complete AlephTool impl. Changes needed:
- Fix internal registry type from `Arc<GenerationProviderRegistry>` to `Arc<RwLock<GenerationProviderRegistry>>`
- Add registration in `builder.rs` and dispatch routing in `registry.rs`
- Keep existing field name `text` (not `prompt`) — semantically correct for text-to-speech

```rust
pub struct SpeechGenerateArgs {
    pub text: String,         // text to convert to speech (not a creative prompt)
    pub provider: Option<String>,
    pub voice: Option<String>,
}
```

#### 1.2 Naming Unification

| Old Name | New Name |
|----------|----------|
| `image_generate` | `image_generate` (unchanged) |
| `generate_video` | `video_generate` |
| `generate_audio` | `audio_generate` |
| (unregistered) | `speech_generate` |

#### 1.3 Registration Unification in builder.rs

Replace conditional hardcoded JSON schema registration with unified AlephTool registration:

```rust
// All four types follow the same pattern as image_generate
for gen_type in [Image, Video, Audio, Speech] {
    if reg_inner.first_for_type(gen_type).is_some() {
        // Register corresponding AlephTool
    }
}
```

#### 1.4 Cleanup

- Delete `execute_video_generate()` and `execute_audio_generate()` from `executors.rs`
- Delete `executors.rs` if it becomes empty
- Update `registry.rs` tool dispatch to use `AlephTool::call_json` for all four types

---

### Layer 2: Explicit Commands `/video`, `/image`, `/audio`, `/speech`

#### 2.1 Fast Path Execution

Four commands registered as builtin tools with `dispatch_mode = Direct`, going through the **Fast Path (L0)** — zero LLM token consumption:

```
/video a person reading a book near a window
  → Fast Path → video_generate(prompt="a person reading...")

/image a cute cat in watercolor style
  → Fast Path → image_generate(prompt="a cute cat...")

/audio lofi hip hop beat, calm and relaxing
  → Fast Path → audio_generate(prompt="lofi hip hop...")

/speech Hello, welcome to our presentation
  → Fast Path → speech_generate(text="Hello, welcome...")
```

#### 2.2 Registration in builder.rs

Register as builtin tools with metadata:
- `dispatch_mode = Direct` → fast path
- `param_hint = "<prompt>"` or `"<text>"` (for speech) → UI hint
- `routing_strip_prefix = true` → strip command prefix, remaining text becomes prompt/text

#### 2.3 Fast Path Argument Mapping

In `slash_command.rs`, add parameter mapping. Note: `image_generate` currently also lacks explicit mapping (falls through to generic `_` branch), so this fixes all four:

```rust
"video_generate" | "image_generate" | "audio_generate" => {
    json!({"prompt": args_text})
}
"speech_generate" => {
    json!({"text": args_text})
}
```

#### 2.4 Natural Language Compatibility

Users can still say "generate a video of X" without commands — the Agent Loop handles this via LLM tool selection. Commands are shortcuts, not the only entry point.

---

### Layer 3: Token Budget Self-Reflection + hit_limit Safety Net

#### 3.1 LLM Self-Reflection (Prompt Layer)

Add efficiency awareness rules to BASE_BEHAVIOR in `prompt_builder.rs`:

```
## Efficiency Awareness

- If the user's request maps directly to an available tool (image/video/audio generation,
  web search, file operations, etc.), call that tool IMMEDIATELY. Do not explore configuration,
  read guides, or verify setup first — trust that registered tools are ready to use.

- Prefer action over preparation. If a tool directly matches the request, call it first and
  explore only if it fails. Endless preparation is not productive.

- When you have enough information to attempt the task, attempt it. A failed attempt with a clear
  error message is more useful than exhausting the token budget on preparation.
```

Design principles:
- Pure prompt guidance, no hardcoded limits in code
- Does not restrict complex tasks (debugging, research) that genuinely need multi-step exploration
- Only accelerates requests that directly map to a tool
- Compliant with R8 (LLM Sovereignty) and R10 (Intelligence Lives in the Prompt)

#### 3.2 hit_limit Safety Net (Code Layer — Message Improvement)

A generic hit_limit fallback already exists in `run_loop.rs`. This change **improves the existing message** to guide users toward efficient alternatives:

```rust
// Existing generic message → improved with actionable guidance
if result.hit_limit && result.final_text.as_ref().map(|t| t.is_empty()).unwrap_or(true) {
    result.final_text = Some(format!(
        "抱歉，我在处理这个请求时用了太多步骤但没能完成（{} 次迭代，{} 次工具调用）。\
         请尝试更直接的指令，比如使用 /video、/image、/audio 等命令。",
        result.iterations, result.tool_calls_made
    ));
}
```

Bilingual fallback based on user's message language.

#### 3.3 Explicit Non-Goals

- **No hard tool call count limit** — complex tasks need multiple steps
- **No token counter injection into prompt** — adds complexity, LLM can't count precisely
- **No middleware interception layer** — violates R8 (LLM Sovereignty)

---

## Files to Modify

| File | Change |
|------|--------|
| `src/builtin_tools/generation/video_generate.rs` | **New** — AlephTool impl |
| `src/builtin_tools/generation/audio_generate.rs` | **New** — AlephTool impl |
| `src/builtin_tools/generation/speech_generate.rs` | **Modify** — fix `Arc<RwLock<>>` wrapping + add registration |
| `src/builtin_tools/generation/mod.rs` | **Modify** — export new modules |
| `src/executor/builtin_registry/builder.rs` | **Modify** — unified registration + command registration |
| `src/executor/builtin_registry/registry.rs` | **Modify** — route to AlephTool::call_json |
| `src/executor/builtin_registry/executors.rs` | **Delete or empty** — remove legacy handlers |
| `src/gateway/execution_engine/slash_command.rs` | **Modify** — add fast path arg mapping |
| `src/agent_loop/prompt_builder.rs` | **Modify** — add efficiency awareness to BASE_BEHAVIOR |
| `src/gateway/execution_engine/run_loop.rs` | **Modify** — hit_limit safety net |

## Implementation Priority

1. **Layer 1** — Generation tool unification (fixes root cause)
2. **Layer 3** — Token self-reflection + hit_limit safety net (prevents recurrence)
3. **Layer 2** — Explicit commands (user convenience)
