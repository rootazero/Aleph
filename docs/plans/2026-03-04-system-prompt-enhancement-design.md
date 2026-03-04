# System Prompt Enhancement Design

> Inspired by OpenClaw's system prompt architecture, tailored for Aleph's PromptPipeline.

**Date**: 2026-03-04
**Status**: Approved
**Approach**: Plan A — Layer-by-layer enhancement within existing PromptPipeline

---

## Background

Aleph's PromptPipeline (22 priority-ordered Layers) is already more architecturally elegant than OpenClaw's monolithic 25-section string builder. However, OpenClaw has several runtime capabilities that Aleph lacks:

| Gap | Description | Priority |
|-----|-------------|----------|
| **Bootstrap Files** | Workspace-level context injection via user-editable files | P0 |
| **Token Budget** | System prompt budget management with smart truncation | P0 |
| **PromptMode** | Sub-agent prompt minimization (Full/Compact/Minimal) | P0 |
| **Truncation Warnings** | User notification when context is truncated | P1 |
| **Heartbeat Guidance** | Long-task progress reporting guidance | P2 |

### What Aleph Already Does Better

- **PromptPipeline architecture** — 22 composable Layers vs OpenClaw's monolithic function
- **Three-tier tool disclosure** — Full/Summary/Index progressive disclosure
- **SoulManifest** — Structured identity (voice, directives, anti_patterns, relationship)
- **ContextAggregator** — Two-phase filtering (Interaction + Security)
- **Multi-paradigm support** — CLI/WebRich/Messaging/Background/Embedded

---

## Design

### 1. PromptMode — Sub-Agent Prompt Minimization

Add `PromptMode` to control which Layers participate in assembly:

```rust
/// Prompt rendering mode — controls which layers participate
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptMode {
    /// All layers included (primary agent)
    #[default]
    Full,
    /// Essential layers only (sub-agent, saves ~60% tokens)
    /// Includes: Soul, Profile, Role, Tools, Security, ResponseFormat, Custom, Language
    /// Excludes: Bootstrap, Heartbeat, Skills, Guidelines, Thinking, POE, etc.
    Compact,
    /// Identity line only + tools + response format (ultra-lightweight)
    Minimal,
}
```

**Layer participation matrix:**

| Layer | Priority | Full | Compact | Minimal |
|-------|----------|------|---------|---------|
| SoulLayer | 50 | ✓ | ✓ | identity only |
| BootstrapLayer | 55 | ✓ | ✗ | ✗ |
| ProfileLayer | 75 | ✓ | ✓ | ✗ |
| RoleLayer | 100 | ✓ | ✓ | ✗ |
| RuntimeContextLayer | 200 | ✓ | ✗ | ✗ |
| EnvironmentLayer | 300 | ✓ | ✗ | ✗ |
| RuntimeCapabilitiesLayer | 400 | ✓ | ✗ | ✗ |
| ToolsLayer | 500 | ✓ | ✓ | ✓ |
| HydratedToolsLayer | 500 | ✓ | ✓ | ✓ |
| PoePromptLayer | 505 | ✓ | ✗ | ✗ |
| SecurityLayer | 600 | ✓ | ✓ | ✗ |
| ProtocolTokensLayer | 700 | ✓ | ✗ | ✗ |
| HeartbeatLayer | 710 | ✓ | ✗ | ✗ |
| OperationalGuidelinesLayer | 800 | ✓ | ✗ | ✗ |
| CitationStandardsLayer | 900 | ✓ | ✗ | ✗ |
| GenerationModelsLayer | 1000 | ✓ | ✗ | ✗ |
| SkillInstructionsLayer | 1050 | ✓ | ✗ | ✗ |
| SpecialActionsLayer | 1100 | ✓ | ✗ | ✗ |
| ResponseFormatLayer | 1200 | ✓ | ✓ | ✓ |
| GuidelinesLayer | 1300 | ✓ | ✗ | ✗ |
| ThinkingGuidanceLayer | 1350 | ✓ | ✗ | ✗ |
| SkillModeLayer | 1400 | ✓ | ✗ | ✗ |
| CustomInstructionsLayer | 1500 | ✓ | ✓ | ✗ |
| LanguageLayer | 1600 | ✓ | ✓ | ✓ |

**Implementation**: Add `fn supports_mode(&self, mode: PromptMode) -> bool` to the `PromptLayer` trait with a default implementation returning `true`. Override in layers that should be excluded from Compact/Minimal.

### 2. Bootstrap File System

Workspace-level context files that auto-inject into system prompt, complementing SoulManifest's code-level identity.

**File conventions** (scanned from workspace root):

| File | Purpose | Priority |
|------|---------|----------|
| `CONTEXT.md` | Project context (architecture, tech stack, conventions) | 1 (highest) |
| `INSTRUCTIONS.md` | Custom instructions (workflow habits, preferences) | 2 |
| `TOOLS.md` | Tool usage guide (which tools, how to use) | 3 |
| `MEMORY.md` | Persistent memory (cross-session) | 4 (lowest) |

**Design decision**: No SOUL.md/IDENTITY.md — Aleph's identity is managed by `SoulManifest` struct, not file injection.

**BootstrapLayer (priority 55):**

```rust
pub struct BootstrapLayer {
    workspace: PathBuf,
    max_chars_per_file: usize,  // default: 20_000
    max_chars_total: usize,     // default: 100_000
}

impl PromptLayer for BootstrapLayer {
    fn priority(&self) -> u32 { 55 }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn render(&self, ctx: &LayerContext) -> Option<String> {
        // 1. Scan workspace for bootstrap files in priority order
        // 2. Load each file, apply per-file truncation (70% head + 20% tail)
        // 3. Enforce total budget (sequential, stop when depleted)
        // 4. Format as markdown sections with file attribution
        // 5. Track truncation stats in LayerContext for warning system
    }
}
```

**Truncation algorithm:**
- File exceeds `max_chars_per_file` → keep 70% head + 20% tail, mark middle with `[... N chars truncated ...]`
- Total exceeds `max_chars_total` → process in priority order, stop loading when budget exhausted
- Character-safe truncation: use `char_indices()` to avoid UTF-8 boundary splits

**Output format:**
```markdown
## Workspace Context

### CONTEXT.md
[file content, possibly truncated]

### INSTRUCTIONS.md
[file content, possibly truncated]
```

### 3. Token Budget Management

Global budget guard for assembled system prompt, preventing context window squeeze.

**TokenBudget configuration:**

```rust
pub struct TokenBudget {
    /// Maximum total characters for assembled system prompt
    /// Default: 80_000 chars (~20K tokens)
    pub max_total_chars: usize,

    /// Bootstrap section budget (subset of total, managed by BootstrapLayer)
    pub max_bootstrap_chars: usize,  // default: 100_000

    /// Per-bootstrap-file limit (managed by BootstrapLayer)
    pub max_per_file_chars: usize,   // default: 20_000

    /// Warning mode for truncation events
    pub truncation_warning: TruncationWarning,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum TruncationWarning {
    Off,
    #[default]
    Once,
    Always,
}
```

**Budget enforcement in Pipeline:**

```rust
impl PromptPipeline {
    pub fn assemble(&self, mode: PromptMode, budget: &TokenBudget) -> PromptResult {
        // 1. Run all layers that support the mode, in priority order
        let sections: Vec<(u32, String)> = self.layers
            .iter()
            .filter(|l| l.supports_mode(mode))
            .filter_map(|l| l.render(&ctx).map(|s| (l.priority(), s)))
            .collect();

        // 2. Merge all sections
        let raw_prompt = sections.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("\n\n");

        // 3. If over budget, trim from lowest priority
        let (final_prompt, stats) = if raw_prompt.len() > budget.max_total_chars {
            self.trim_to_budget(&sections, budget.max_total_chars)
        } else {
            (raw_prompt, vec![])
        };

        PromptResult { prompt: final_prompt, truncation_stats: stats, mode }
    }
}
```

**Trimming strategy:**
- Remove sections from lowest priority (highest number) first
- Core layers are never trimmed: Soul(50), Role(100), Tools(500), ResponseFormat(1200)
- Partial trimming not supported — a section is either fully included or fully removed
- This keeps the prompt coherent (no half-rendered sections)

**PromptResult:**

```rust
pub struct PromptResult {
    pub prompt: String,
    pub truncation_stats: Vec<TruncationStat>,
    pub mode: PromptMode,
}

pub struct TruncationStat {
    pub layer_name: String,
    pub original_chars: usize,
    pub final_chars: usize,  // 0 if fully removed
    pub fully_removed: bool,
}
```

### 4. Truncation Warning System

Notify users when bootstrap files or system prompt sections are truncated.

**Integration point**: Thinker, after assembling prompt via Pipeline.

```rust
impl Thinker {
    fn maybe_emit_truncation_warning(
        &self,
        result: &PromptResult,
        session: &SessionState,
    ) {
        if result.truncation_stats.is_empty() { return; }

        match self.config.prompt.token_budget.truncation_warning {
            TruncationWarning::Off => {},
            TruncationWarning::Once => {
                let sig = hash_truncation_stats(&result.truncation_stats);
                if !session.has_seen_truncation_signature(sig) {
                    session.record_truncation_signature(sig);
                    self.emit_warning(&result.truncation_stats);
                }
            },
            TruncationWarning::Always => {
                self.emit_warning(&result.truncation_stats);
            },
        }
    }
}
```

**Warning format:**
```
[System] Context truncated: CONTEXT.md 45000→20000 chars (-56%),
         GuidelinesLayer fully removed (budget exhausted)
```

**Simplification vs OpenClaw**: Use session-level `HashSet<u64>` for signature dedup instead of OpenClaw's 32-entry history cache. Simpler, naturally cleaned up with session lifecycle.

### 5. Heartbeat Layer

Lightweight progress reporting guidance for long-running tasks.

**HeartbeatLayer (priority 710):**

```rust
pub struct HeartbeatLayer;

impl PromptLayer for HeartbeatLayer {
    fn priority(&self) -> u32 { 710 }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }

    fn render(&self, _ctx: &LayerContext) -> Option<String> {
        Some(r#"## Progress Reporting

For long-running tasks (multi-step plans, large file operations):
- Report progress after completing each major step
- Use structured progress format: [step N/total] description
- If a step takes unusually long, report intermediate status"#.to_string())
    }
}
```

**Design decision**: Unlike OpenClaw's complex heartbeat protocol (ACK/NACK/interval), Aleph provides behavioral guidance only. Actual progress tracking is handled by the Resilience system's `TaskGraph`. This follows P6 (KISS).

---

## Architecture Summary

```
PromptPipeline (existing, enhanced)
├── PromptMode filter (new)     ← Full/Compact/Minimal
├── Layer execution (existing)
│   ├── SoulLayer (50)          ← existing
│   ├── BootstrapLayer (55)     ← NEW
│   ├── ProfileLayer (75)       ← existing
│   ├── ... (existing layers)
│   ├── HeartbeatLayer (710)    ← NEW
│   └── ... (existing layers)
├── Budget enforcement (new)    ← trim from low-priority if over budget
└── PromptResult (new)          ← carries truncation stats
         ↓
    Thinker (existing, enhanced)
    └── Truncation warning (new) ← emit to UI via event system
```

**New files to create:**
- `core/src/thinker/prompt/bootstrap.rs` — BootstrapLayer + file loading + truncation
- `core/src/thinker/prompt/heartbeat.rs` — HeartbeatLayer
- `core/src/thinker/prompt/budget.rs` — TokenBudget + enforcement + PromptResult
- `core/src/thinker/prompt/mode.rs` — PromptMode enum

**Files to modify:**
- `core/src/thinker/prompt/mod.rs` — Add PromptMode to Pipeline, assemble() signature
- `core/src/thinker/prompt/layer.rs` — Add `supports_mode()` to PromptLayer trait
- `core/src/thinker/mod.rs` — Add truncation warning emission
- `core/src/config/` — Add TokenBudget to PromptConfig

---

## Configuration

All new settings live under existing `PromptConfig`:

```rust
pub struct PromptConfig {
    // ... existing fields ...

    /// Token budget for system prompt assembly
    pub token_budget: TokenBudget,

    /// Bootstrap file workspace path (auto-detected if None)
    pub bootstrap_workspace: Option<PathBuf>,
}
```

Runtime configuration via `aleph.toml`:

```toml
[prompt]
max_total_chars = 80000
max_bootstrap_chars = 100000
max_per_file_chars = 20000
truncation_warning = "once"  # "off" | "once" | "always"
```

---

## Non-Goals

- **No SOUL.md/IDENTITY.md** — Identity managed by SoulManifest, not files
- **No complex heartbeat protocol** — Behavioral guidance only, Resilience handles tracking
- **No bootstrap hooks** — OpenClaw's `agent.bootstrap` hook is over-engineering for Aleph
- **No workspace state tracking** — OpenClaw's `.openclaw/workspace-state.json` unnecessary
- **No prompt report telemetry** — Can be added later if needed
