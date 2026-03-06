# System Prompt Architecture Optimization Design

> Reference: OpenClaw Agent System Prompt 9-Layer Architecture
> Date: 2026-03-06
> Status: Approved

## Background

Aleph's current system prompt architecture uses a trait-based `PromptLayer` pipeline with 23 composable layers, 5 assembly paths, and 3 prompt modes. While more sophisticated than OpenClaw's monolithic approach, it lacks several capabilities that OpenClaw's 9-layer model provides:

1. **Standardized Workspace Files** — OpenClaw has a defined set of user-editable files (IDENTITY.md, SOUL.md, etc.) auto-injected into system prompt
2. **Inbound Context Layer** — Per-request dynamic context (sender, channel, mentions, etc.)
3. **Rich Hook System** — 4 types of hooks for different injection scenarios

## Design Direction

**Option B: Additive Refactor** — Keep Aleph's existing trait-based pipeline architecture. Add 2 new layers, refactor 3 existing layers, expand Hook system to 4 types. Replace `SoulManifest`/`ProfileConfig` prompt loading with standardized workspace files.

---

## 1. Workspace Files Spec

### Standard File Set

Each Aleph workspace (`~/.aleph/` or custom directory) may contain:

| File | Required | Purpose | Default (missing) |
|------|----------|---------|-------------------|
| `SOUL.md` | No | Identity + personality + voice + directives | Built-in default identity |
| `IDENTITY.md` | No | User info (name, preferences, timezone, language) | No user identity injected |
| `AGENTS.md` | No | Workspace coding standards / project description | No project context |
| `TOOLS.md` | No | Custom tool usage guidelines | Tool built-in descriptions only |
| `MEMORY.md` | No | Persistent memory / knowledge cache | No memory injected |
| `HEARTBEAT.md` | No | Custom heartbeat poll prompt | Default heartbeat prompt |
| `BOOTSTRAP.md` | No | First-run onboarding (new workspace only) | Skip onboarding |

### SOUL.md Format (replaces SoulManifest)

Pure Markdown + YAML frontmatter, lowering the editing barrier:

```markdown
---
tone: warm-professional
verbosity: balanced
formatting: markdown
relationship: trusted-partner
expertise:
  - software-engineering
  - system-architecture
---

# Identity

You are Aleph, a self-hosted personal AI assistant.

# Directives

- Converse in Chinese, code comments in English
- Prefer conciseness, avoid verbosity
- Proactively ask when uncertain

# Anti-patterns

- Never delete user files without confirmation
- Never execute dangerous operations unconfirmed
```

### Loading Mechanism

```rust
pub struct WorkspaceFiles {
    pub workspace_dir: PathBuf,
    pub files: Vec<WorkspaceFile>,
}

pub struct WorkspaceFile {
    pub name: &'static str,          // "SOUL.md"
    pub content: Option<String>,      // None = file missing
    pub truncated: bool,              // exceeded per-file budget
    pub original_size: usize,
}
```

**Loading rules:**
- Scan workspace directory by filename, skip missing files
- Per-file limit: `per_file_max_chars` (default 20K)
- Total limit: `total_max_chars` (default 100K)
- Truncation: head 70% + tail 20% + truncation marker
- Sub-agent mode: inject only `AGENTS.md` + `TOOLS.md`

---

## 2. InboundContextLayer

### Position

Priority **55** (right after SoulLayer at 50). Dynamically injects current session context per request.

### Data Structures

```rust
pub struct InboundContext {
    pub sender: SenderInfo,
    pub channel: ChannelContext,
    pub session: SessionContext,
    pub message: MessageMetadata,
}

pub struct SenderInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub is_owner: bool,
}

pub struct ChannelContext {
    pub kind: String,               // "telegram" | "discord" | "cli" | "websocket"
    pub capabilities: Vec<String>,  // ["inline_buttons", "reactions", "threads"]
    pub is_group_chat: bool,
    pub is_mentioned: bool,
}

pub struct SessionContext {
    pub session_key: String,
    pub active_agent: Option<String>,
}

pub struct MessageMetadata {
    pub has_attachments: bool,
    pub attachment_types: Vec<String>,
    pub reply_to: Option<String>,
}
```

### Layer Implementation

```rust
pub struct InboundContextLayer;

impl PromptLayer for InboundContextLayer {
    fn name(&self) -> &'static str { "inbound_context" }
    fn priority(&self) -> u32 { 55 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Soul, AssemblyPath::Context, AssemblyPath::Cached]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full | PromptMode::Compact)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(inbound) = input.inbound {
            output.push_str("## Inbound Context\n");
            // Format sender, channel, session, message metadata
        }
    }
}
```

### Injected Format Example

```
## Inbound Context
Sender: alice (owner)
Channel: telegram | group_chat | mentioned
Capabilities: inline_buttons, reactions
Session: tg:group:-100123456
Active Agent: default
Attachments: image (1)
```

Estimated size: **200-500 bytes/request** (conversation history is already in the messages array, not repeated here).

---

## 3. Hook System (4 Types)

### Hook 1: BootstrapHook — Full control over workspace files

```rust
pub trait BootstrapHook: Send + Sync {
    fn name(&self) -> &str;
    fn on_bootstrap(&self, ctx: &mut BootstrapHookContext) -> Result<()>;
}

pub struct BootstrapHookContext {
    pub workspace_dir: PathBuf,
    pub session_key: String,
    pub channel: String,
    pub files: Vec<WorkspaceFile>,  // add, remove, modify, reorder
}
```

### Hook 2: ExtraFilesHook — Append-only (simple scenarios)

```rust
pub trait ExtraFilesHook: Send + Sync {
    fn name(&self) -> &str;
    fn extra_files(&self, ctx: &ExtraFilesContext) -> Result<Vec<ExtraFile>>;
}

pub struct ExtraFilesContext {
    pub workspace_dir: PathBuf,
    pub session_key: String,
    pub channel: String,
}

pub struct ExtraFile {
    pub path: String,
    pub content: String,
}
```

### Hook 3: PromptBuildHook — Dynamic injection / final modification

```rust
pub trait PromptBuildHook: Send + Sync {
    fn name(&self) -> &str;
    fn before_build(&self, ctx: &mut PromptBuildContext) -> Result<()> { Ok(()) }
    fn after_build(&self, prompt: &mut String) -> Result<()> { Ok(()) }
}

pub struct PromptBuildContext {
    pub config: PromptConfig,
    pub inbound: Option<InboundContext>,
    pub prepend_context: Option<String>,
    pub system_prompt_override: Option<String>,
}
```

### Hook 4: BudgetHook — Runtime budget adjustment

```rust
pub trait BudgetHook: Send + Sync {
    fn name(&self) -> &str;
    fn adjust_budget(&self, ctx: &BudgetHookContext) -> Result<BudgetOverride>;
}

pub struct BudgetHookContext {
    pub session_key: String,
    pub channel: String,
    pub current_budget: TokenBudget,
}

pub struct BudgetOverride {
    pub per_file_max_chars: Option<usize>,
    pub total_max_chars: Option<usize>,
}
```

### Hook Registry & Execution Pipeline

```rust
pub struct PromptHookRegistry {
    bootstrap_hooks: Vec<Box<dyn BootstrapHook>>,
    extra_files_hooks: Vec<Box<dyn ExtraFilesHook>>,
    prompt_build_hooks: Vec<Box<dyn PromptBuildHook>>,
    budget_hooks: Vec<Box<dyn BudgetHook>>,
}
```

Execution order:
1. `BudgetHook.adjust_budget()` — determine budget for this request
2. `BootstrapHook.on_bootstrap()` — control workspace file set
3. `ExtraFilesHook.extra_files()` — append extra files
4. `PromptBuildHook.before_build()` — inject prepend context
5. `PromptPipeline.assemble()` — layer assembly
6. `PromptBuildHook.after_build()` — final modification

### Hook Sources

| Source | Example |
|--------|---------|
| Built-in modules | Heartbeat system registers BudgetHook to reduce budget for heartbeat requests |
| Extension plugins | WASM/Node.js plugins register via Extension API |
| Config file | `aleph.toml` declares extra-files paths |

### Config-Driven Extra Files

```toml
[prompt.extra_files]
enabled = true
paths = ["docs/API.md", "docs/ARCHITECTURE.md"]
```

Equivalent to registering an `ExtraFilesHook` without writing code.

---

## 4. Layer Refactoring

### SoulLayer Refactor

**Before:** Reads `SoulManifest` Rust struct via `IdentityResolver`.
**After:** Reads `SOUL.md` from workspace files.

```rust
impl PromptLayer for SoulLayer {
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(soul_content) = input.workspace_file("SOUL.md") {
            output.push_str("# Soul\n\n");
            output.push_str(soul_content);
            output.push('\n');
        }
    }
}
```

### ProfileLayer Refactor

**Before:** Reads `ProfileConfig.system_prompt` overlay.
**After:** Reads `AGENTS.md` from workspace files.

```rust
impl PromptLayer for ProfileLayer {
    fn inject(&self, output: &mut String, input: &LayerInput) {
        if let Some(agents_content) = input.workspace_file("AGENTS.md") {
            output.push_str("# Project Context\n\n");
            output.push_str(agents_content);
            output.push('\n');
        }
    }
}
```

Non-prompt capabilities remain in `ProfileConfig`:
```rust
pub struct ProfileConfig {
    // Removed: pub system_prompt: Option<String>,
    pub tool_whitelist: Option<Vec<String>>,
    pub tool_blacklist: Option<Vec<String>>,
    pub prompt_mode: Option<PromptMode>,
}
```

### CustomInstructionsLayer Deprecation

Responsibilities absorbed by `IDENTITY.md` + `AGENTS.md`.
- Mark as `#[deprecated]` during transition period
- If both `custom_instructions` config and `IDENTITY.md` exist, file takes priority, config is fallback
- Remove in next major version

### LayerInput Extension

```rust
pub struct LayerInput<'a> {
    // Existing fields retained
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub poe: Option<&'a PoePromptContext>,
    pub profile: Option<&'a ProfileConfig>,
    pub mode: PromptMode,

    // Deprecated (kept during transition)
    pub soul: Option<&'a SoulManifest>,

    // New
    pub inbound: Option<&'a InboundContext>,
    pub workspace: Option<&'a WorkspaceFiles>,
}

impl<'a> LayerInput<'a> {
    /// Get workspace file content by name
    pub fn workspace_file(&self, name: &str) -> Option<&str> {
        self.workspace
            .and_then(|ws| ws.files.iter()
                .find(|f| f.name == name)
                .and_then(|f| f.content.as_deref()))
    }
}
```

---

## 5. Final Layer List (24 Layers)

| Priority | Layer | Change |
|----------|-------|--------|
| 50 | SoulLayer | **Refactored** — reads SOUL.md |
| **55** | **InboundContextLayer** | **New** |
| 75 | ProfileLayer | **Refactored** — reads AGENTS.md |
| 100 | RoleLayer | Unchanged |
| 200 | RuntimeContextLayer | Unchanged |
| 300 | EnvironmentLayer | Unchanged |
| 400 | RuntimeCapabilitiesLayer | Unchanged |
| 500 | ToolsLayer | Unchanged |
| 501 | HydratedToolsLayer | Unchanged |
| 505 | PoePromptLayer | Unchanged |
| 600 | SecurityLayer | Unchanged |
| 700 | ProtocolTokensLayer | Unchanged |
| 710 | HeartbeatLayer | Unchanged |
| 800 | OperationalGuidelinesLayer | Unchanged |
| 900 | CitationStandardsLayer | Unchanged |
| 1000 | GenerationModelsLayer | Unchanged |
| 1050 | SkillInstructionsLayer | Unchanged |
| 1100 | SpecialActionsLayer | Unchanged |
| 1200 | ResponseFormatLayer | Unchanged |
| 1300 | GuidelinesLayer | Unchanged |
| 1350 | ThinkingGuidanceLayer | Unchanged |
| 1400 | SkillModeLayer | Unchanged |
| 1500 | CustomInstructionsLayer | **Deprecated** (transition) |
| **1550** | **WorkspaceFilesLayer** | **New** — loads IDENTITY/TOOLS/MEMORY/HEARTBEAT/BOOTSTRAP |
| 1600 | LanguageLayer | Unchanged |

---

## 6. Migration Strategy

### Phase 1: Add (non-breaking)
- Implement `WorkspaceFiles` loading mechanism
- Add `WorkspaceFilesLayer` and `InboundContextLayer`
- Extend `LayerInput` with `workspace` and `inbound` fields
- Implement 4 Hook types and `PromptHookRegistry`
- Existing SoulManifest/ProfileConfig/CustomInstructions continue to work

### Phase 2: Migrate (soft deprecation)
- Refactor `SoulLayer` to prefer `SOUL.md` over `SoulManifest`
- Refactor `ProfileLayer` to prefer `AGENTS.md` over `ProfileConfig.system_prompt`
- Mark `CustomInstructionsLayer` as `#[deprecated]`
- Add migration tool: `aleph migrate workspace` to generate workspace files from existing config

### Phase 3: Clean up (next major version)
- Remove `SoulManifest` struct and `IdentityResolver`
- Remove `ProfileConfig.system_prompt` field
- Remove `CustomInstructionsLayer`
- Remove deprecated `soul` field from `LayerInput`
