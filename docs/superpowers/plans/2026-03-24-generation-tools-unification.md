# Generation Tools Unification & Token Budget Self-Reflection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify all generation tools (video/audio/speech) as proper AlephTools, add `/video` `/image` `/audio` `/speech` fast-path commands, and add LLM self-reflection for token budget awareness.

**Architecture:** Three layers — (1) Upgrade video/audio to AlephTool matching image_generate.rs pattern, fix speech_generate.rs wrapping, (2) Register 4 slash commands on the fast path, (3) Add efficiency awareness to BASE_BEHAVIOR and improve hit_limit fallback message.

**Tech Stack:** Rust, serde/schemars for JSON Schema, async_trait for AlephTool

**Spec:** `docs/superpowers/specs/2026-03-24-generation-tools-unification-design.md`

---

## Task 1: Create `video_generate.rs`

**Files:**
- Create: `src/builtin_tools/generation/video_generate.rs`
- Reference: `src/builtin_tools/generation/image_generate.rs` (full pattern to follow)

- [ ] **Step 1: Create `video_generate.rs` with Args, Output, Tool struct, and AlephTool impl**

Follow `image_generate.rs` pattern exactly. Key differences: no width/height/quality/style params, add aspect_ratio. Use `duration_ms` (consistent with image/speech).

**CRITICAL**: Use `crate::sync_primitives::{Arc, RwLock}` — NOT `std::sync`. This project uses a sync_primitives module for conditional loom support.

```rust
//! Video generation tool — generates videos from text descriptions.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::{Arc, RwLock};
use std::time::Instant;
use tracing::info;

use crate::error::Result;
use crate::generation::{
    GenerationData, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::builtin_tools::error::ToolError;
use crate::tools::AlephTool;

/// Arguments for the video generation tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct VideoGenerateArgs {
    /// Text description of the video to generate
    pub prompt: String,
    /// Optional provider name (uses default video provider if not specified)
    pub provider: Option<String>,
    /// Optional aspect ratio (e.g. "16:9", "9:16", "1:1")
    pub aspect_ratio: Option<String>,
}

/// Output from the video generation tool.
#[derive(Debug, Clone, Serialize)]
pub struct VideoGenerateOutput {
    /// Location of the generated video (URL or local file path)
    pub video_location: String,
    /// Type of location: "url" or "file"
    pub location_type: String,
    /// The prompt used for generation
    pub prompt: String,
    /// Provider that generated the video
    pub provider: String,
    /// Model used for generation
    pub model: Option<String>,
    /// Wall-clock time for generation in milliseconds
    pub duration_ms: u64,
}

/// Tool for generating videos from text descriptions.
pub struct VideoGenerateTool {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
}

impl VideoGenerateTool {
    pub const NAME: &'static str = "video_generate";
    pub const DESCRIPTION: &'static str =
        "Generate a video from a text description. Provide a detailed prompt describing the scene, motion, style, and camera movement.";

    pub fn new(registry: Arc<RwLock<GenerationProviderRegistry>>) -> Self {
        Self { registry }
    }

    async fn call_impl(
        &self,
        args: VideoGenerateArgs,
    ) -> std::result::Result<VideoGenerateOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let start = Instant::now();

        // Truncate prompt for display (UTF-8 safe)
        let prompt_display = if args.prompt.chars().count() > 30 {
            let end = args.prompt
                .char_indices()
                .nth(30)
                .map(|(i, _)| i)
                .unwrap_or(args.prompt.len());
            format!("{}...", &args.prompt[..end])
        } else {
            args.prompt.clone()
        };
        notify_tool_start(Self::NAME, &format!("生成视频: {}", prompt_display));

        info!(prompt = %args.prompt, provider = ?args.provider, "Starting video generation");

        // Acquire lock in scoped block — drop before await
        let (provider_name, provider) = {
            let reg = self.registry.read().map_err(|e| {
                let error_msg = format!("Failed to acquire registry lock: {}", e);
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Execution(error_msg)
            })?;

            if let Some(ref name) = args.provider {
                let p = reg.get(name).ok_or_else(|| {
                    let error_msg = format!("Video provider '{}' not found", name);
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::InvalidArgs(error_msg)
                })?;
                if !p.supports(GenerationType::Video) {
                    let error_msg = format!("Provider '{}' does not support video generation", name);
                    notify_tool_result(Self::NAME, &error_msg, false);
                    return Err(ToolError::InvalidArgs(error_msg));
                }
                (name.clone(), p)
            } else {
                reg.first_for_type(GenerationType::Video).ok_or_else(|| {
                    let error_msg = "No video generation provider available".to_string();
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::Execution(error_msg)
                })?
            }
            // Lock is dropped here at end of block
        };

        info!(provider = %provider_name, "Using video provider");

        // Build request
        let mut request = GenerationRequest::video(&args.prompt);
        if let Some(ref ar) = args.aspect_ratio {
            request.params.aspect_ratio = Some(ar.clone());
        }

        // Execute generation
        let output = provider.generate(request).await.map_err(|e| {
            let error_msg = format!("Video generation failed: {}", e);
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::from(e)
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Process output
        let (video_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url"),
            GenerationData::LocalPath(path) => (path.clone(), "file"),
            GenerationData::Bytes(_) => {
                let error_msg = "Video provider returned raw bytes — expected URL or file path".to_string();
                notify_tool_result(Self::NAME, &error_msg, false);
                return Err(ToolError::Execution(error_msg));
            }
        };

        // Notify success
        let result_summary = format!(
            "视频生成完成 ({} ms, provider: {})",
            duration_ms, provider_name
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(VideoGenerateOutput {
            video_location: video_location.to_string(),
            location_type: location_type.to_string(),
            prompt: args.prompt,
            provider: provider_name,
            model: output.metadata.model,
            duration_ms,
        })
    }
}

impl Clone for VideoGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

#[async_trait]
impl AlephTool for VideoGenerateTool {
    const NAME: &'static str = "video_generate";
    const DESCRIPTION: &'static str =
        "Generate a video from a text description. Provide a detailed prompt describing the scene, motion, style, and camera movement.";
    type Args = VideoGenerateArgs;
    type Output = VideoGenerateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}
```

- [ ] **Step 2: Verify it compiles in isolation**

Run: `cargo check -p alephcore 2>&1 | head -20`

(It won't compile yet because mod.rs doesn't export it — that's fine, just verify no syntax errors in the file itself by checking the error is "module not found".)

---

## Task 2: Create `audio_generate.rs`

**Files:**
- Create: `src/builtin_tools/generation/audio_generate.rs`

- [ ] **Step 1: Create `audio_generate.rs` following the same pattern as video_generate.rs**

Same pattern as `video_generate.rs`. **CRITICAL**: Use `crate::sync_primitives::{Arc, RwLock}`, include `notify_tool_start`/`notify_tool_result`, use `crate::builtin_tools::error::ToolError`.

```rust
//! Audio/music generation tool — generates audio from text descriptions.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::sync_primitives::{Arc, RwLock};
use std::time::Instant;
use tracing::info;

use crate::error::Result;
use crate::generation::{
    GenerationData, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::builtin_tools::error::ToolError;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AudioGenerateArgs {
    /// Text description of the audio/music to generate
    pub prompt: String,
    /// Optional provider name (uses default audio provider if not specified)
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioGenerateOutput {
    pub audio_location: String,
    pub location_type: String,
    pub prompt: String,
    pub provider: String,
    pub model: Option<String>,
    pub duration_ms: u64,
}

pub struct AudioGenerateTool {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
}

impl AudioGenerateTool {
    pub const NAME: &'static str = "audio_generate";
    pub const DESCRIPTION: &'static str =
        "Generate audio or music from a text description. Provide a prompt describing the genre, mood, instruments, tempo, and style.";

    pub fn new(registry: Arc<RwLock<GenerationProviderRegistry>>) -> Self {
        Self { registry }
    }

    async fn call_impl(
        &self,
        args: AudioGenerateArgs,
    ) -> std::result::Result<AudioGenerateOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let start = Instant::now();

        let prompt_display = if args.prompt.chars().count() > 30 {
            let end = args.prompt.char_indices().nth(30).map(|(i, _)| i).unwrap_or(args.prompt.len());
            format!("{}...", &args.prompt[..end])
        } else {
            args.prompt.clone()
        };
        notify_tool_start(Self::NAME, &format!("生成音频: {}", prompt_display));

        info!(prompt = %args.prompt, provider = ?args.provider, "Starting audio generation");

        let (provider_name, provider) = {
            let reg = self.registry.read().map_err(|e| {
                let error_msg = format!("Failed to acquire registry lock: {}", e);
                notify_tool_result(Self::NAME, &error_msg, false);
                ToolError::Execution(error_msg)
            })?;

            if let Some(ref name) = args.provider {
                let p = reg.get(name).ok_or_else(|| {
                    let error_msg = format!("Audio provider '{}' not found", name);
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::InvalidArgs(error_msg)
                })?;
                if !p.supports(GenerationType::Audio) {
                    let error_msg = format!("Provider '{}' does not support audio generation", name);
                    notify_tool_result(Self::NAME, &error_msg, false);
                    return Err(ToolError::InvalidArgs(error_msg));
                }
                (name.clone(), p)
            } else {
                reg.first_for_type(GenerationType::Audio).ok_or_else(|| {
                    let error_msg = "No audio generation provider available".to_string();
                    notify_tool_result(Self::NAME, &error_msg, false);
                    ToolError::Execution(error_msg)
                })?
            }
        };

        info!(provider = %provider_name, "Using audio provider");

        let request = GenerationRequest::audio(&args.prompt);
        let output = provider.generate(request).await.map_err(|e| {
            let error_msg = format!("Audio generation failed: {}", e);
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::from(e)
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let (audio_location, location_type) = match &output.data {
            GenerationData::Url(url) => (url.clone(), "url"),
            GenerationData::LocalPath(path) => (path.clone(), "file"),
            GenerationData::Bytes(_) => {
                let error_msg = "Audio provider returned raw bytes — expected URL or file path".to_string();
                notify_tool_result(Self::NAME, &error_msg, false);
                return Err(ToolError::Execution(error_msg));
            }
        };

        let result_summary = format!("音频生成完成 ({} ms, provider: {})", duration_ms, provider_name);
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(AudioGenerateOutput {
            audio_location: audio_location.to_string(),
            location_type: location_type.to_string(),
            prompt: args.prompt,
            provider: provider_name,
            model: output.metadata.model,
            duration_ms,
        })
    }
}

impl Clone for AudioGenerateTool {
    fn clone(&self) -> Self {
        Self { registry: Arc::clone(&self.registry) }
    }
}

#[async_trait]
impl AlephTool for AudioGenerateTool {
    const NAME: &'static str = "audio_generate";
    const DESCRIPTION: &'static str =
        "Generate audio or music from a text description. Provide a prompt describing the genre, mood, instruments, tempo, and style.";
    type Args = AudioGenerateArgs;
    type Output = AudioGenerateOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}
```

---

## Task 3: Fix `speech_generate.rs` — `Arc<RwLock<>>` wrapping

**Files:**
- Modify: `src/builtin_tools/generation/speech_generate.rs:72-86` (struct + constructor)
- Modify: `src/builtin_tools/generation/speech_generate.rs:109-145` (lock acquisition in call_impl)

The existing `SpeechGenerateTool` uses `Arc<GenerationProviderRegistry>` (no RwLock). Must match `ImageGenerateTool`'s `Arc<RwLock<GenerationProviderRegistry>>` pattern since the builder stores `Option<Arc<std::sync::RwLock<GenerationProviderRegistry>>>`.

- [ ] **Step 1: Update struct definition (line 72-74)**

Change:
```rust
pub struct SpeechGenerateTool {
    registry: Arc<GenerationProviderRegistry>,
}
```
To:
```rust
pub struct SpeechGenerateTool {
    registry: Arc<RwLock<GenerationProviderRegistry>>,
}
```

- [ ] **Step 2: Fix import (line 9)**

The existing `use crate::sync_primitives::Arc;` must include RwLock. Change to:
```rust
use crate::sync_primitives::{Arc, RwLock};
```

- [ ] **Step 3: Update constructor (line 84-86)**

The `new()` method signature changes from `Arc<GenerationProviderRegistry>` to `Arc<RwLock<GenerationProviderRegistry>>`:

```rust
pub fn new(registry: Arc<RwLock<GenerationProviderRegistry>>) -> Self {
    Self { registry }
}
```

- [ ] **Step 4: Update `call_impl` provider lookup to use scoped lock (lines ~120-145)**

Replace direct registry access with scoped lock block matching image_generate.rs pattern. The current code calls `self.registry.get(name)` directly — must change to `self.registry.read().unwrap_or_else(|e| e.into_inner())` in a scoped block, then drop before await.

Find the provider lookup section and wrap it in a scoped block:
```rust
let (provider_name, provider) = {
    let reg = self.registry.read().unwrap_or_else(|e| e.into_inner());
    // ... existing provider selection logic using reg instead of self.registry ...
};
```

- [ ] **Step 5: Update tests to use `Arc<RwLock<>>`**

The existing tests in `speech_generate.rs` (lines ~254-533) create registries as `Arc<GenerationProviderRegistry>`. After changing the struct, these must wrap with `RwLock`. Find all `Arc::new(GenerationProviderRegistry::new())` and `create_test_registry()` patterns in the test module and wrap with `RwLock`:

Change: `Arc::new(GenerationProviderRegistry::new())`
To: `Arc::new(RwLock::new(GenerationProviderRegistry::new()))`

And update `create_test_registry()` helper similarly.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

---

## Task 4: Update `mod.rs` — export new modules

**Files:**
- Modify: `src/builtin_tools/generation/mod.rs`

- [ ] **Step 1: Add module declarations and pub exports**

Add after the existing `pub use speech_generate::...` line:

```rust
mod audio_generate;
mod video_generate;

pub use audio_generate::{AudioGenerateArgs, AudioGenerateOutput, AudioGenerateTool};
pub use video_generate::{VideoGenerateArgs, VideoGenerateOutput, VideoGenerateTool};
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

---

## Task 5: Update `builder.rs` — unified tool registration

**Files:**
- Modify: `src/executor/builtin_registry/builder.rs:134-138` (tool creation)
- Modify: `src/executor/builtin_registry/builder.rs:565-603` (metadata registration)

- [ ] **Step 1: Add tool fields and creation alongside image_generate_tool (after line 138)**

After the existing `image_generate_tool` creation block, add:

```rust
let video_generate_tool = config.generation_registry.as_ref().map(|registry| {
    info!("Creating VideoGenerateTool with generation registry");
    crate::builtin_tools::generation::VideoGenerateTool::new(Arc::clone(registry))
});

let audio_generate_tool = config.generation_registry.as_ref().map(|registry| {
    info!("Creating AudioGenerateTool with generation registry");
    crate::builtin_tools::generation::AudioGenerateTool::new(Arc::clone(registry))
});

let speech_generate_tool = config.generation_registry.as_ref().map(|registry| {
    info!("Creating SpeechGenerateTool with generation registry");
    crate::builtin_tools::generation::SpeechGenerateTool::new(Arc::clone(registry))
});
```

- [ ] **Step 2: Add fields to BuiltinToolRegistry struct (in registry.rs around line 41)**

After the existing `image_generate_tool` field, add:

```rust
pub(crate) video_generate_tool: Option<crate::builtin_tools::generation::VideoGenerateTool>,
pub(crate) audio_generate_tool: Option<crate::builtin_tools::generation::AudioGenerateTool>,
pub(crate) speech_generate_tool: Option<crate::builtin_tools::generation::SpeechGenerateTool>,
```

- [ ] **Step 3: Pass fields in struct initialization (builder.rs around line 384)**

After `image_generate_tool,` add:

```rust
video_generate_tool,
audio_generate_tool,
speech_generate_tool,
```

- [ ] **Step 4: Replace hardcoded video/audio metadata registration with schema-derived (lines 574-600)**

Replace the existing `generate_video` and `generate_audio` conditional blocks with unified registration:

```rust
if let Ok(reg_inner) = registry.read() {
    use crate::generation::GenerationType;

    if reg_inner.first_for_type(GenerationType::Video).is_some() {
        reg(tools, "video_generate",
            crate::builtin_tools::generation::VideoGenerateTool::DESCRIPTION,
            serde_json::to_value(schemars::schema_for!(crate::builtin_tools::generation::VideoGenerateArgs)).unwrap_or_default());
        info!("Registered video_generate tool in BuiltinToolRegistry");
    }

    if reg_inner.first_for_type(GenerationType::Audio).is_some() {
        reg(tools, "audio_generate",
            crate::builtin_tools::generation::AudioGenerateTool::DESCRIPTION,
            serde_json::to_value(schemars::schema_for!(crate::builtin_tools::generation::AudioGenerateArgs)).unwrap_or_default());
        info!("Registered audio_generate tool in BuiltinToolRegistry");
    }

    if reg_inner.first_for_type(GenerationType::Speech).is_some() {
        reg(tools, "speech_generate",
            crate::builtin_tools::generation::SpeechGenerateTool::DESCRIPTION,
            serde_json::to_value(schemars::schema_for!(crate::builtin_tools::generation::SpeechGenerateArgs)).unwrap_or_default());
        info!("Registered speech_generate tool in BuiltinToolRegistry");
    }
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

---

## Task 6: Update `registry.rs` — dispatch routing

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs:290-291`

- [ ] **Step 1: Replace legacy dispatch with AlephTool routing**

Replace the existing lines:
```rust
"generate_video" => Box::pin(async move { self.execute_video_generate(arguments).await }),
"generate_audio" => Box::pin(async move { self.execute_audio_generate(arguments).await }),
```

With:
```rust
"video_generate" => Box::pin(async move {
    let tool = self.video_generate_tool.as_ref().ok_or_else(|| {
        AlephError::tool("Video generation not available: no generation registry configured")
    })?;
    tool.call_json(arguments).await
}),
"audio_generate" => Box::pin(async move {
    let tool = self.audio_generate_tool.as_ref().ok_or_else(|| {
        AlephError::tool("Audio generation not available: no generation registry configured")
    })?;
    tool.call_json(arguments).await
}),
"speech_generate" => Box::pin(async move {
    let tool = self.speech_generate_tool.as_ref().ok_or_else(|| {
        AlephError::tool("Speech generation not available: no generation registry configured")
    })?;
    tool.call_json(arguments).await
}),
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

---

## Task 7: Delete `executors.rs` legacy code

**Files:**
- Modify or Delete: `src/executor/builtin_registry/executors.rs`
- Modify: `src/executor/builtin_registry/mod.rs` (remove `mod executors;` if deleting)

- [ ] **Step 1: Check if executors.rs contains anything besides video/audio handlers**

Read the file. If it only contains `execute_video_generate` and `execute_audio_generate`, delete it entirely and remove `mod executors;` from `mod.rs`.

If it contains other methods, only remove the two legacy methods.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

- [ ] **Step 3: Run existing tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`

- [ ] **Step 4: Commit Layer 1**

```bash
git add src/builtin_tools/generation/video_generate.rs \
       src/builtin_tools/generation/audio_generate.rs \
       src/builtin_tools/generation/speech_generate.rs \
       src/builtin_tools/generation/mod.rs \
       src/executor/builtin_registry/builder.rs \
       src/executor/builtin_registry/registry.rs \
       src/executor/builtin_registry/executors.rs \
       src/executor/builtin_registry/mod.rs
git commit -m "generation: unify video/audio/speech as AlephTools

Upgrade video_generate and audio_generate from legacy handlers to proper
AlephTool implementations matching image_generate.rs pattern. Fix
speech_generate Arc<RwLock<>> wrapping inconsistency and register it.
Delete legacy executors.rs handlers."
```

---

## Task 8: Add efficiency awareness to BASE_BEHAVIOR

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs:29-55` (BASE_BEHAVIOR constant)

- [ ] **Step 1: Append efficiency awareness section to BASE_BEHAVIOR**

Add at the end of the BASE_BEHAVIOR string (before the closing `";`):

```rust
\n\
- **EFFICIENCY: ACT BEFORE EXPLORING.** If the user's request maps directly to an available tool (image/video/audio generation, web search, file operations, etc.), call that tool IMMEDIATELY. Do not explore configuration, read guides, or verify setup first — trust that registered tools are ready to use.\n\
- **EFFICIENCY: PREFER ACTION OVER PREPARATION.** If a tool directly matches the request, call it first and explore only if it fails. When you have enough information to attempt the task, attempt it. A failed attempt with a clear error message is more useful than exhausting the token budget on preparation.";
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -10`

- [ ] **Step 3: Commit Layer 3 part 1**

```bash
git add src/agent_loop/prompt_builder.rs
git commit -m "agent_loop: add efficiency awareness to BASE_BEHAVIOR

Prompt the LLM to call matching tools immediately rather than spending
tokens exploring configuration. Prefer action over preparation."
```

---

## Task 9: Improve hit_limit fallback message

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs:249-259`

- [ ] **Step 1: Replace the generic fallback message with actionable guidance**

Find the existing fallback (lines ~256-259):
```rust
format!(
    "Sorry, I was unable to complete the task within the allowed limits ({} iterations, {} tool calls). Please try a simpler request.",
    result.iterations, result.tool_calls_made
)
```

Replace with:
```rust
format!(
    "抱歉，我在处理这个请求时用了太多步骤但没能完成（{} 次迭代，{} 次工具调用）。\n\
     请尝试更直接的指令，比如使用 /video、/image、/audio 等命令直接生成内容。\n\n\
     Sorry, I was unable to complete the task within the allowed limits ({} iterations, {} tool calls). \
     Try using direct commands like /video, /image, or /audio for generation tasks.",
    result.iterations, result.tool_calls_made,
    result.iterations, result.tool_calls_made
)
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -10`

- [ ] **Step 3: Commit Layer 3 part 2**

```bash
git add src/gateway/execution_engine/run_loop.rs
git commit -m "run_loop: improve hit_limit fallback with actionable guidance

Replace generic 'try simpler request' message with bilingual text that
directs users to fast-path slash commands (/video, /image, /audio)."
```

---

## Task 10: Add fast-path slash commands

**Files:**
- Modify: `src/gateway/execution_engine/slash_command.rs:255-322` (build_tool_arguments)
- Modify: `src/executor/builtin_registry/builder.rs` (command shorthand mapping)

- [ ] **Step 1: Add argument mapping for generation tools in `build_tool_arguments`**

In `slash_command.rs`, add a new match arm before the generic `_` fallback (before line ~307):

```rust
// Generation tools: /video <prompt>, /image <prompt>, /audio <prompt>
"video_generate" | "image_generate" | "audio_generate" => serde_json::json!({
    "prompt": args_str,
}),
// Speech tool: /speech <text>
"speech_generate" => serde_json::json!({
    "text": args_str,
}),
```

- [ ] **Step 2: Add command shorthand mappings in `try_resolve_slash_command`**

In `slash_command.rs`, find the shorthand mapping section (around line 50 where `"rename"` is mapped):

```rust
let cmd_name = match cmd_name.as_str() {
    "rename" => "session_set_topic".to_string(),
    other => other.to_string(),
};
```

Extend it to:
```rust
let cmd_name = match cmd_name.as_str() {
    "rename" => "session_set_topic".to_string(),
    "video" => "video_generate".to_string(),
    "image" => "image_generate".to_string(),
    "audio" => "audio_generate".to_string(),
    "speech" => "speech_generate".to_string(),
    other => other.to_string(),
};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -10`

- [ ] **Step 4: Commit Layer 2**

```bash
git add src/gateway/execution_engine/slash_command.rs
git commit -m "slash_command: add /video /image /audio /speech fast-path commands

Map /video, /image, /audio, /speech as shorthand commands that resolve
to their respective generation tools on the fast path (L0), bypassing
the agent loop entirely for zero-token-cost generation."
```

---

## Task 11: Update config.toml for renamed tool

**Files:**
- Check: `~/.aleph/config.toml` (if `default_video_provider` references old tool name)

- [ ] **Step 1: Check if any config references old tool names**

Search for `generate_video` or `generate_audio` in config files and code. The config field `default_video_provider` should still work (it references provider name, not tool name). But verify no tool-name references exist.

Run: `grep -r "generate_video\|generate_audio" src/ --include="*.rs" | grep -v "test" | grep -v "target/"`

If any references remain, update them to `video_generate` / `audio_generate`.

- [ ] **Step 2: Final full compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -5`

- [ ] **Step 3: Run all tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -20`

- [ ] **Step 4: Final commit if needed**

If any additional fixes were needed, commit them:
```bash
git add -A
git commit -m "generation: fix remaining references to old tool names"
```
