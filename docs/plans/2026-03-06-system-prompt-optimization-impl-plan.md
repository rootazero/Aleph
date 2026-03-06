# System Prompt Architecture Optimization — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph's 23-layer prompt pipeline with standardized workspace files, inbound context layer, and 4-type hook system, referencing OpenClaw's 9-layer architecture.

**Architecture:** Additive refactor (Option B) — add 2 new layers (`InboundContextLayer`, `WorkspaceFilesLayer`), refactor 3 existing layers (`SoulLayer`, `ProfileLayer`, `CustomInstructionsLayer`), expand Hook system from 2 to 4 types. Phase 1 is non-breaking; Phase 2 migrates; Phase 3 cleans up.

**Tech Stack:** Rust, Tokio, trait-based composition, tempfile (tests)

**Design Doc:** `docs/plans/2026-03-06-system-prompt-architecture-optimization-design.md`

---

## Phase 1: Add (Non-Breaking)

### Task 1: WorkspaceFiles Data Structures

**Files:**
- Create: `core/src/thinker/workspace_files.rs`
- Modify: `core/src/thinker/mod.rs` (add `pub mod workspace_files;`)

**Step 1: Write the failing test**

In `core/src/thinker/workspace_files.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[test]
    fn workspace_file_names_match_spec() {
        assert_eq!(WORKSPACE_FILE_NAMES, &[
            "SOUL.md",
            "IDENTITY.md",
            "AGENTS.md",
            "TOOLS.md",
            "MEMORY.md",
            "HEARTBEAT.md",
            "BOOTSTRAP.md",
        ]);
    }

    #[test]
    fn load_finds_existing_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("SOUL.md"), "I am Aleph.").unwrap();
        fs::write(dir.path().join("IDENTITY.md"), "User: Alice").unwrap();

        let ws = WorkspaceFiles::load(dir.path(), &WorkspaceFilesConfig::default());
        assert_eq!(ws.files.len(), 2);
        assert_eq!(ws.files[0].name, "SOUL.md");
        assert_eq!(ws.files[0].content.as_deref(), Some("I am Aleph."));
        assert_eq!(ws.files[1].name, "IDENTITY.md");
    }

    #[test]
    fn load_skips_missing_files() {
        let dir = tempdir().unwrap();
        let ws = WorkspaceFiles::load(dir.path(), &WorkspaceFilesConfig::default());
        assert!(ws.files.is_empty());
    }

    #[test]
    fn load_skips_empty_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("SOUL.md"), "").unwrap();
        fs::write(dir.path().join("SOUL.md"), "   \n  ").unwrap();

        let ws = WorkspaceFiles::load(dir.path(), &WorkspaceFilesConfig::default());
        assert!(ws.files.is_empty());
    }

    #[test]
    fn truncates_large_files() {
        let dir = tempdir().unwrap();
        let large = "X".repeat(30_000);
        fs::write(dir.path().join("SOUL.md"), &large).unwrap();

        let cfg = WorkspaceFilesConfig { per_file_max_chars: 20_000, total_max_chars: 100_000 };
        let ws = WorkspaceFiles::load(dir.path(), &cfg);
        assert_eq!(ws.files.len(), 1);
        assert!(ws.files[0].truncated);
        assert!(ws.files[0].content.as_ref().unwrap().len() <= 21_000); // margin for marker
    }

    #[test]
    fn respects_total_budget() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("SOUL.md"), &"A".repeat(60_000)).unwrap();
        fs::write(dir.path().join("IDENTITY.md"), &"B".repeat(60_000)).unwrap();

        let cfg = WorkspaceFilesConfig { per_file_max_chars: 60_000, total_max_chars: 80_000 };
        let ws = WorkspaceFiles::load(dir.path(), &cfg);
        let total: usize = ws.files.iter()
            .filter_map(|f| f.content.as_ref())
            .map(|c| c.len())
            .sum();
        assert!(total <= 85_000); // slight margin for truncation markers
    }

    #[test]
    fn get_file_by_name() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("SOUL.md"), "I am Aleph.").unwrap();

        let ws = WorkspaceFiles::load(dir.path(), &WorkspaceFilesConfig::default());
        assert_eq!(ws.get("SOUL.md"), Some("I am Aleph."));
        assert_eq!(ws.get("MISSING.md"), None);
    }

    #[test]
    fn prefers_dot_aleph_directory() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("SOUL.md"), "root version").unwrap();
        fs::create_dir_all(dir.path().join(".aleph")).unwrap();
        fs::write(dir.path().join(".aleph").join("SOUL.md"), "aleph version").unwrap();

        let ws = WorkspaceFiles::load(dir.path(), &WorkspaceFilesConfig::default());
        assert_eq!(ws.get("SOUL.md"), Some("aleph version"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib workspace_files -- --nocapture 2>&1 | head -30`
Expected: FAIL — module does not exist

**Step 3: Write minimal implementation**

```rust
//! Standardized workspace files for system prompt injection.
//!
//! Loads user-editable files (SOUL.md, IDENTITY.md, etc.) from a workspace
//! directory and provides them to the prompt pipeline.

use std::path::{Path, PathBuf};
use crate::thinker::prompt_budget::truncate_with_head_tail;

/// Ordered list of standard workspace file names.
pub const WORKSPACE_FILE_NAMES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
];

/// Configuration for workspace file loading.
#[derive(Debug, Clone)]
pub struct WorkspaceFilesConfig {
    pub per_file_max_chars: usize,
    pub total_max_chars: usize,
}

impl Default for WorkspaceFilesConfig {
    fn default() -> Self {
        Self {
            per_file_max_chars: 20_000,
            total_max_chars: 100_000,
        }
    }
}

/// A loaded workspace file.
#[derive(Debug, Clone)]
pub struct WorkspaceFile {
    pub name: &'static str,
    pub content: Option<String>,
    pub truncated: bool,
    pub original_size: usize,
}

/// Collection of loaded workspace files.
#[derive(Debug, Clone)]
pub struct WorkspaceFiles {
    pub workspace_dir: PathBuf,
    pub files: Vec<WorkspaceFile>,
}

impl WorkspaceFiles {
    /// Load workspace files from directory, applying truncation budgets.
    pub fn load(workspace: &Path, config: &WorkspaceFilesConfig) -> Self {
        let mut files = Vec::new();
        let mut total_chars = 0;

        for &name in WORKSPACE_FILE_NAMES {
            if total_chars >= config.total_max_chars {
                break;
            }

            let path = resolve_path(workspace, name);
            let raw = match std::fs::read_to_string(&path) {
                Ok(c) if !c.trim().is_empty() => c,
                _ => continue,
            };

            let original_size = raw.len();
            let mut truncated = false;

            // Per-file truncation
            let content = if raw.len() > config.per_file_max_chars {
                truncated = true;
                truncate_with_head_tail(&raw, config.per_file_max_chars, 0.7, 0.2)
            } else {
                raw
            };

            // Total budget check
            let remaining = config.total_max_chars.saturating_sub(total_chars);
            let content = if content.len() > remaining {
                truncated = true;
                truncate_with_head_tail(&content, remaining, 0.7, 0.2)
            } else {
                content
            };

            total_chars += content.len();
            files.push(WorkspaceFile {
                name,
                content: Some(content),
                truncated,
                original_size,
            });
        }

        Self {
            workspace_dir: workspace.to_path_buf(),
            files,
        }
    }

    /// Get file content by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.files.iter()
            .find(|f| f.name == name)
            .and_then(|f| f.content.as_deref())
    }
}

/// Resolve file path: check .aleph/ first, then workspace root.
fn resolve_path(workspace: &Path, filename: &str) -> PathBuf {
    let aleph_path = workspace.join(".aleph").join(filename);
    if aleph_path.exists() {
        return aleph_path;
    }
    workspace.join(filename)
}
```

**Step 4: Register module in mod.rs**

In `core/src/thinker/mod.rs`, add:
```rust
pub mod workspace_files;
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib workspace_files -- --nocapture`
Expected: All 7 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/workspace_files.rs core/src/thinker/mod.rs
git commit -m "thinker: add WorkspaceFiles data structures and loader"
```

---

### Task 2: InboundContext Data Structures

**Files:**
- Create: `core/src/thinker/inbound_context.rs`
- Modify: `core/src/thinker/mod.rs` (add `pub mod inbound_context;`)

**Step 1: Write the failing test**

In `core/src/thinker/inbound_context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sender_is_not_owner() {
        let sender = SenderInfo::default();
        assert!(!sender.is_owner);
        assert!(sender.display_name.is_none());
    }

    #[test]
    fn channel_context_defaults() {
        let ctx = ChannelContext::default();
        assert_eq!(ctx.kind, "unknown");
        assert!(!ctx.is_group_chat);
        assert!(!ctx.is_mentioned);
        assert!(ctx.capabilities.is_empty());
    }

    #[test]
    fn format_for_prompt_basic() {
        let ctx = InboundContext {
            sender: SenderInfo {
                id: "user123".into(),
                display_name: Some("Alice".into()),
                is_owner: true,
            },
            channel: ChannelContext {
                kind: "telegram".into(),
                capabilities: vec!["reactions".into()],
                is_group_chat: false,
                is_mentioned: false,
            },
            session: SessionContext {
                session_key: "tg:dm:123".into(),
                active_agent: Some("default".into()),
            },
            message: MessageMetadata::default(),
        };

        let formatted = ctx.format_for_prompt();
        assert!(formatted.contains("Alice"));
        assert!(formatted.contains("owner"));
        assert!(formatted.contains("telegram"));
        assert!(formatted.contains("tg:dm:123"));
    }

    #[test]
    fn format_for_prompt_with_attachments() {
        let ctx = InboundContext {
            sender: SenderInfo { id: "u1".into(), ..Default::default() },
            channel: ChannelContext {
                kind: "discord".into(),
                is_group_chat: true,
                is_mentioned: true,
                ..Default::default()
            },
            session: SessionContext { session_key: "dc:guild:456".into(), ..Default::default() },
            message: MessageMetadata {
                has_attachments: true,
                attachment_types: vec!["image".into()],
                reply_to: Some("msg_789".into()),
            },
        };

        let formatted = ctx.format_for_prompt();
        assert!(formatted.contains("group_chat"));
        assert!(formatted.contains("mentioned"));
        assert!(formatted.contains("image"));
        assert!(formatted.contains("msg_789"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib inbound_context -- --nocapture 2>&1 | head -20`
Expected: FAIL — module does not exist

**Step 3: Write minimal implementation**

```rust
//! Inbound context — per-request dynamic context for the prompt pipeline.
//!
//! Provides sender identity, channel info, session state, and message
//! metadata so the LLM knows "who is talking, where, and how to respond."

/// Sender identity information.
#[derive(Debug, Clone, Default)]
pub struct SenderInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub is_owner: bool,
}

/// Channel context for the current request.
#[derive(Debug, Clone)]
pub struct ChannelContext {
    pub kind: String,
    pub capabilities: Vec<String>,
    pub is_group_chat: bool,
    pub is_mentioned: bool,
}

impl Default for ChannelContext {
    fn default() -> Self {
        Self {
            kind: "unknown".into(),
            capabilities: Vec::new(),
            is_group_chat: false,
            is_mentioned: false,
        }
    }
}

/// Session context.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub session_key: String,
    pub active_agent: Option<String>,
}

/// Message metadata.
#[derive(Debug, Clone, Default)]
pub struct MessageMetadata {
    pub has_attachments: bool,
    pub attachment_types: Vec<String>,
    pub reply_to: Option<String>,
}

/// Complete inbound context for a single request.
#[derive(Debug, Clone, Default)]
pub struct InboundContext {
    pub sender: SenderInfo,
    pub channel: ChannelContext,
    pub session: SessionContext,
    pub message: MessageMetadata,
}

impl InboundContext {
    /// Format for injection into system prompt.
    pub fn format_for_prompt(&self) -> String {
        let mut lines = Vec::with_capacity(8);

        // Sender
        let sender_display = self.sender.display_name.as_deref().unwrap_or(&self.sender.id);
        let owner_tag = if self.sender.is_owner { " (owner)" } else { "" };
        lines.push(format!("Sender: {}{}", sender_display, owner_tag));

        // Channel
        let mut channel_parts = vec![self.channel.kind.clone()];
        if self.channel.is_group_chat {
            channel_parts.push("group_chat".into());
        }
        if self.channel.is_mentioned {
            channel_parts.push("mentioned".into());
        }
        lines.push(format!("Channel: {}", channel_parts.join(" | ")));

        // Capabilities
        if !self.channel.capabilities.is_empty() {
            lines.push(format!("Capabilities: {}", self.channel.capabilities.join(", ")));
        }

        // Session
        lines.push(format!("Session: {}", self.session.session_key));
        if let Some(ref agent) = self.session.active_agent {
            lines.push(format!("Active Agent: {}", agent));
        }

        // Attachments
        if self.message.has_attachments {
            lines.push(format!("Attachments: {} ({})",
                self.message.attachment_types.join(", "),
                self.message.attachment_types.len()));
        }

        // Reply
        if let Some(ref reply_to) = self.message.reply_to {
            lines.push(format!("Reply To: {}", reply_to));
        }

        lines.join("\n")
    }
}
```

**Step 4: Register module**

In `core/src/thinker/mod.rs`, add:
```rust
pub mod inbound_context;
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib inbound_context -- --nocapture`
Expected: All 4 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/inbound_context.rs core/src/thinker/mod.rs
git commit -m "thinker: add InboundContext data structures"
```

---

### Task 3: Extend LayerInput with workspace and inbound fields

**Files:**
- Modify: `core/src/thinker/prompt_layer.rs:33-95`

**Step 1: Write the failing test**

Add to `core/src/thinker/prompt_layer.rs` tests:

```rust
#[cfg(test)]
mod workspace_tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::workspace_files::{WorkspaceFiles, WorkspaceFile};
    use crate::thinker::inbound_context::{InboundContext, SenderInfo};

    #[test]
    fn layer_input_workspace_file_access() {
        let config = PromptConfig::default();
        let tools = vec![];
        let ws = WorkspaceFiles {
            workspace_dir: "/tmp".into(),
            files: vec![WorkspaceFile {
                name: "SOUL.md",
                content: Some("I am Aleph.".into()),
                truncated: false,
                original_size: 11,
            }],
        };
        let input = LayerInput::basic(&config, &tools).with_workspace(&ws);
        assert_eq!(input.workspace_file("SOUL.md"), Some("I am Aleph."));
        assert_eq!(input.workspace_file("MISSING.md"), None);
    }

    #[test]
    fn layer_input_inbound_access() {
        let config = PromptConfig::default();
        let tools = vec![];
        let inbound = InboundContext {
            sender: SenderInfo { id: "u1".into(), is_owner: true, ..Default::default() },
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &tools).with_inbound(&inbound);
        assert!(input.inbound.is_some());
        assert!(input.inbound.unwrap().sender.is_owner);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib prompt_layer::workspace_tests -- --nocapture 2>&1 | head -20`
Expected: FAIL — `with_workspace` and `with_inbound` don't exist

**Step 3: Add fields and methods to LayerInput**

In `core/src/thinker/prompt_layer.rs`, modify `LayerInput`:

```rust
// Add imports at top:
use crate::thinker::workspace_files::WorkspaceFiles;
use crate::thinker::inbound_context::InboundContext;

pub struct LayerInput<'a> {
    pub config: &'a PromptConfig,
    pub tools: Option<&'a [ToolInfo]>,
    pub hydration: Option<&'a HydrationResult>,
    pub soul: Option<&'a SoulManifest>,
    pub context: Option<&'a ResolvedContext>,
    pub poe: Option<&'a PoePromptContext>,
    pub profile: Option<&'a crate::config::ProfileConfig>,
    pub mode: PromptMode,
    // New fields
    pub inbound: Option<&'a InboundContext>,
    pub workspace: Option<&'a WorkspaceFiles>,
}

// Update ALL existing constructors to include new fields as None:
// basic(), hydration(), soul(), context()

// Add new builder methods:
impl<'a> LayerInput<'a> {
    // ... existing methods ...

    /// Attach inbound context.
    pub fn with_inbound(mut self, inbound: &'a InboundContext) -> Self {
        self.inbound = Some(inbound);
        self
    }

    /// Attach workspace files.
    pub fn with_workspace(mut self, workspace: &'a WorkspaceFiles) -> Self {
        self.workspace = Some(workspace);
        self
    }

    /// Get workspace file content by name.
    pub fn workspace_file(&self, name: &str) -> Option<&str> {
        self.workspace
            .and_then(|ws| ws.get(name))
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib prompt_layer -- --nocapture`
Expected: All tests PASS (existing + new)

**Step 5: Commit**

```bash
git add core/src/thinker/prompt_layer.rs
git commit -m "thinker: extend LayerInput with workspace and inbound fields"
```

---

### Task 4: InboundContextLayer Implementation

**Files:**
- Create: `core/src/thinker/layers/inbound_context.rs`
- Modify: `core/src/thinker/layers/mod.rs` (add module + re-export)

**Step 1: Write the failing test**

In `core/src/thinker/layers/inbound_context.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::PromptLayer as _;
    use crate::thinker::inbound_context::*;

    #[test]
    fn layer_metadata() {
        let layer = InboundContextLayer;
        assert_eq!(layer.name(), "inbound_context");
        assert_eq!(layer.priority(), 55);
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn layer_paths() {
        let paths = InboundContextLayer.paths();
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
        assert!(!paths.contains(&AssemblyPath::Basic));
    }

    #[test]
    fn injects_when_inbound_present() {
        let config = PromptConfig::default();
        let tools = vec![];
        let inbound = InboundContext {
            sender: SenderInfo {
                id: "alice".into(),
                display_name: Some("Alice".into()),
                is_owner: true,
            },
            channel: ChannelContext {
                kind: "telegram".into(),
                is_group_chat: true,
                is_mentioned: true,
                capabilities: vec!["reactions".into()],
            },
            session: SessionContext {
                session_key: "tg:group:123".into(),
                active_agent: Some("default".into()),
            },
            message: MessageMetadata::default(),
        };
        let input = LayerInput::basic(&config, &tools).with_inbound(&inbound);
        let mut out = String::new();
        InboundContextLayer.inject(&mut out, &input);

        assert!(out.contains("## Inbound Context"));
        assert!(out.contains("Alice"));
        assert!(out.contains("telegram"));
        assert!(out.contains("tg:group:123"));
    }

    #[test]
    fn skips_when_no_inbound() {
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        InboundContextLayer.inject(&mut out, &input);

        assert!(out.is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib layers::inbound_context -- --nocapture 2>&1 | head -20`
Expected: FAIL — module does not exist

**Step 3: Write implementation**

```rust
//! InboundContextLayer — per-request dynamic context injection (priority 55)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

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
        let inbound = match input.inbound {
            Some(ctx) => ctx,
            None => return,
        };

        output.push_str("## Inbound Context\n");
        output.push_str(&inbound.format_for_prompt());
        output.push_str("\n\n");
    }
}
```

**Step 4: Register in layers/mod.rs**

Add to `core/src/thinker/layers/mod.rs`:
```rust
// --- Inbound context layer ---
mod inbound_context;
pub use inbound_context::InboundContextLayer;
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib layers::inbound_context -- --nocapture`
Expected: All 4 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/layers/inbound_context.rs core/src/thinker/layers/mod.rs
git commit -m "thinker: add InboundContextLayer (priority 55)"
```

---

### Task 5: WorkspaceFilesLayer Implementation

**Files:**
- Create: `core/src/thinker/layers/workspace_files.rs`
- Modify: `core/src/thinker/layers/mod.rs` (add module + re-export)

**Step 1: Write the failing test**

In `core/src/thinker/layers/workspace_files.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::PromptLayer as _;
    use crate::thinker::workspace_files::*;

    fn make_workspace(files: Vec<(&'static str, &str)>) -> WorkspaceFiles {
        WorkspaceFiles {
            workspace_dir: "/tmp".into(),
            files: files.into_iter().map(|(name, content)| WorkspaceFile {
                name,
                content: Some(content.to_string()),
                truncated: false,
                original_size: content.len(),
            }).collect(),
        }
    }

    #[test]
    fn layer_metadata() {
        let layer = WorkspaceFilesLayer;
        assert_eq!(layer.name(), "workspace_files");
        assert_eq!(layer.priority(), 1550);
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn injects_remaining_files() {
        // SOUL.md and AGENTS.md are handled by SoulLayer/ProfileLayer,
        // so WorkspaceFilesLayer injects the rest.
        let ws = make_workspace(vec![
            ("SOUL.md", "identity"),       // handled by SoulLayer
            ("AGENTS.md", "project rules"), // handled by ProfileLayer
            ("IDENTITY.md", "User: Alice"),
            ("TOOLS.md", "Use git carefully"),
            ("MEMORY.md", "Key fact: X=42"),
        ]);
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools).with_workspace(&ws);
        let mut out = String::new();
        WorkspaceFilesLayer.inject(&mut out, &input);

        // Should include IDENTITY, TOOLS, MEMORY but NOT SOUL, AGENTS
        assert!(out.contains("IDENTITY.md"));
        assert!(out.contains("User: Alice"));
        assert!(out.contains("TOOLS.md"));
        assert!(out.contains("MEMORY.md"));
        assert!(!out.contains("### SOUL.md"));
        assert!(!out.contains("### AGENTS.md"));
    }

    #[test]
    fn skips_when_no_workspace() {
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        WorkspaceFilesLayer.inject(&mut out, &input);
        assert!(out.is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib layers::workspace_files -- --nocapture 2>&1 | head -20`
Expected: FAIL

**Step 3: Write implementation**

```rust
//! WorkspaceFilesLayer — inject remaining workspace files (priority 1550)
//!
//! SOUL.md is handled by SoulLayer (50), AGENTS.md by ProfileLayer (75).
//! This layer injects: IDENTITY.md, TOOLS.md, MEMORY.md, HEARTBEAT.md, BOOTSTRAP.md.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Files handled by other layers (excluded from this layer's injection).
const HANDLED_ELSEWHERE: &[&str] = &["SOUL.md", "AGENTS.md"];

pub struct WorkspaceFilesLayer;

impl PromptLayer for WorkspaceFilesLayer {
    fn name(&self) -> &'static str { "workspace_files" }
    fn priority(&self) -> u32 { 1550 }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let workspace = match input.workspace {
            Some(ws) => ws,
            None => return,
        };

        let mut sections = Vec::new();
        for file in &workspace.files {
            if HANDLED_ELSEWHERE.contains(&file.name) {
                continue;
            }
            if let Some(ref content) = file.content {
                sections.push(format!("### {}\n{}", file.name, content));
            }
        }

        if !sections.is_empty() {
            output.push_str("## Workspace Files\n\n");
            output.push_str(&sections.join("\n\n"));
            output.push_str("\n\n");
        }
    }
}
```

**Step 4: Register in layers/mod.rs**

```rust
// --- Workspace files layer ---
mod workspace_files;
pub use workspace_files::WorkspaceFilesLayer;
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib layers::workspace_files -- --nocapture`
Expected: All 3 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/layers/workspace_files.rs core/src/thinker/layers/mod.rs
git commit -m "thinker: add WorkspaceFilesLayer (priority 1550)"
```

---

### Task 6: Hook System — 4 Trait Definitions

**Files:**
- Create: `core/src/thinker/prompt_hooks_v2.rs`
- Modify: `core/src/thinker/mod.rs` (add `pub mod prompt_hooks_v2;`)

**Step 1: Write the failing test**

In `core/src/thinker/prompt_hooks_v2.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_budget::TokenBudget;
    use std::path::PathBuf;

    // --- Stub implementations ---

    struct TestBootstrapHook;
    impl BootstrapHook for TestBootstrapHook {
        fn name(&self) -> &str { "test_bootstrap" }
        fn on_bootstrap(&self, ctx: &mut BootstrapHookContext) -> crate::error::Result<()> {
            ctx.files.push(crate::thinker::workspace_files::WorkspaceFile {
                name: "TOOLS.md", // Note: this is &'static str
                content: Some("Injected by hook".into()),
                truncated: false,
                original_size: 16,
            });
            Ok(())
        }
    }

    struct TestExtraFilesHook;
    impl ExtraFilesHook for TestExtraFilesHook {
        fn name(&self) -> &str { "test_extra" }
        fn extra_files(&self, _ctx: &ExtraFilesContext) -> crate::error::Result<Vec<ExtraFile>> {
            Ok(vec![ExtraFile {
                path: "docs/API.md".into(),
                content: "# API Docs".into(),
            }])
        }
    }

    struct TestBudgetHook;
    impl BudgetHook for TestBudgetHook {
        fn name(&self) -> &str { "test_budget" }
        fn adjust_budget(&self, _ctx: &BudgetHookContext) -> crate::error::Result<BudgetOverride> {
            Ok(BudgetOverride {
                per_file_max_chars: Some(10_000),
                total_max_chars: None,
            })
        }
    }

    #[test]
    fn registry_registers_and_counts() {
        let mut registry = PromptHookRegistry::new();
        registry.register_bootstrap(Box::new(TestBootstrapHook));
        registry.register_extra_files(Box::new(TestExtraFilesHook));
        registry.register_budget(Box::new(TestBudgetHook));
        assert_eq!(registry.bootstrap_hooks.len(), 1);
        assert_eq!(registry.extra_files_hooks.len(), 1);
        assert_eq!(registry.budget_hooks.len(), 1);
    }

    #[test]
    fn bootstrap_hook_modifies_files() {
        let hook = TestBootstrapHook;
        let mut ctx = BootstrapHookContext {
            workspace_dir: PathBuf::from("/tmp"),
            session_key: "test".into(),
            channel: "cli".into(),
            files: Vec::new(),
        };
        hook.on_bootstrap(&mut ctx).unwrap();
        assert_eq!(ctx.files.len(), 1);
        assert_eq!(ctx.files[0].content.as_deref(), Some("Injected by hook"));
    }

    #[test]
    fn extra_files_hook_returns_files() {
        let hook = TestExtraFilesHook;
        let ctx = ExtraFilesContext {
            workspace_dir: PathBuf::from("/tmp"),
            session_key: "test".into(),
            channel: "cli".into(),
        };
        let files = hook.extra_files(&ctx).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "docs/API.md");
    }

    #[test]
    fn budget_hook_adjusts_budget() {
        let hook = TestBudgetHook;
        let ctx = BudgetHookContext {
            session_key: "test".into(),
            channel: "cli".into(),
            current_budget: TokenBudget::default(),
        };
        let override_ = hook.adjust_budget(&ctx).unwrap();
        assert_eq!(override_.per_file_max_chars, Some(10_000));
        assert!(override_.total_max_chars.is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib prompt_hooks_v2 -- --nocapture 2>&1 | head -20`
Expected: FAIL

**Step 3: Write implementation**

```rust
//! Prompt Hook System v2 — 4 types of hooks for prompt customization.
//!
//! 1. BootstrapHook — full control over workspace files
//! 2. ExtraFilesHook — append-only (simple scenarios)
//! 3. PromptBuildHook — dynamic injection / final modification
//! 4. BudgetHook — runtime budget adjustment

use std::path::PathBuf;
use crate::error::Result;
use crate::thinker::prompt_builder::PromptConfig;
use crate::thinker::prompt_budget::TokenBudget;
use crate::thinker::workspace_files::WorkspaceFile;
use crate::thinker::inbound_context::InboundContext;

// ── Hook 1: Bootstrap ──────────────────────────────────────────────

/// Full control over workspace files (add, remove, modify, reorder).
pub trait BootstrapHook: Send + Sync {
    fn name(&self) -> &str;
    fn on_bootstrap(&self, ctx: &mut BootstrapHookContext) -> Result<()>;
}

pub struct BootstrapHookContext {
    pub workspace_dir: PathBuf,
    pub session_key: String,
    pub channel: String,
    pub files: Vec<WorkspaceFile>,
}

// ── Hook 2: Extra Files ────────────────────────────────────────────

/// Append-only file injection (simple scenarios).
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

// ── Hook 3: Prompt Build ───────────────────────────────────────────

/// Dynamic injection before/after prompt assembly.
pub trait PromptBuildHook: Send + Sync {
    fn name(&self) -> &str;
    fn before_build(&self, _ctx: &mut PromptBuildContext) -> Result<()> { Ok(()) }
    fn after_build(&self, _prompt: &mut String) -> Result<()> { Ok(()) }
}

pub struct PromptBuildContext {
    pub config: PromptConfig,
    pub inbound: Option<InboundContext>,
    pub prepend_context: Option<String>,
    pub system_prompt_override: Option<String>,
}

// ── Hook 4: Budget ─────────────────────────────────────────────────

/// Runtime budget adjustment.
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

// ── Registry ───────────────────────────────────────────────────────

/// Central registry for all 4 hook types.
pub struct PromptHookRegistry {
    pub bootstrap_hooks: Vec<Box<dyn BootstrapHook>>,
    pub extra_files_hooks: Vec<Box<dyn ExtraFilesHook>>,
    pub prompt_build_hooks: Vec<Box<dyn PromptBuildHook>>,
    pub budget_hooks: Vec<Box<dyn BudgetHook>>,
}

impl PromptHookRegistry {
    pub fn new() -> Self {
        Self {
            bootstrap_hooks: Vec::new(),
            extra_files_hooks: Vec::new(),
            prompt_build_hooks: Vec::new(),
            budget_hooks: Vec::new(),
        }
    }

    pub fn register_bootstrap(&mut self, hook: Box<dyn BootstrapHook>) {
        self.bootstrap_hooks.push(hook);
    }

    pub fn register_extra_files(&mut self, hook: Box<dyn ExtraFilesHook>) {
        self.extra_files_hooks.push(hook);
    }

    pub fn register_prompt_build(&mut self, hook: Box<dyn PromptBuildHook>) {
        self.prompt_build_hooks.push(hook);
    }

    pub fn register_budget(&mut self, hook: Box<dyn BudgetHook>) {
        self.budget_hooks.push(hook);
    }
}

impl Default for PromptHookRegistry {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 4: Register module**

In `core/src/thinker/mod.rs`:
```rust
pub mod prompt_hooks_v2;
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib prompt_hooks_v2 -- --nocapture`
Expected: All 4 tests PASS

**Step 6: Commit**

```bash
git add core/src/thinker/prompt_hooks_v2.rs core/src/thinker/mod.rs
git commit -m "thinker: add 4-type hook system (PromptHookRegistry v2)"
```

---

### Task 7: Register New Layers in PromptPipeline

**Files:**
- Modify: `core/src/thinker/prompt_pipeline.rs:132-158`
- Modify: `core/src/thinker/prompt_pipeline.rs` (tests)

**Step 1: Update default_layers()**

In `core/src/thinker/prompt_pipeline.rs`, update `default_layers()`:

```rust
pub fn default_layers() -> Self {
    Self::new(vec![
        Box::new(SoulLayer),
        Box::new(InboundContextLayer),  // NEW: priority 55
        Box::new(ProfileLayer),
        Box::new(RoleLayer),
        Box::new(RuntimeContextLayer),
        Box::new(EnvironmentLayer),
        Box::new(RuntimeCapabilitiesLayer),
        Box::new(ToolsLayer),
        Box::new(HydratedToolsLayer),
        Box::new(crate::poe::PoePromptLayer),
        Box::new(SecurityLayer),
        Box::new(ProtocolTokensLayer),
        Box::new(HeartbeatLayer),
        Box::new(OperationalGuidelinesLayer),
        Box::new(CitationStandardsLayer),
        Box::new(GenerationModelsLayer),
        Box::new(SkillInstructionsLayer),
        Box::new(SpecialActionsLayer),
        Box::new(ResponseFormatLayer),
        Box::new(GuidelinesLayer),
        Box::new(ThinkingGuidanceLayer),
        Box::new(SkillModeLayer),
        Box::new(CustomInstructionsLayer),
        Box::new(WorkspaceFilesLayer),  // NEW: priority 1550
        Box::new(LanguageLayer),
    ])
}
```

Add import: `use super::layers::InboundContextLayer;` and `use super::layers::WorkspaceFilesLayer;`

**Step 2: Update test assertion**

Update `test_default_layers_count`:
```rust
assert_eq!(pipeline.layer_count(), 25); // was 23
```

Also add `55` to the protected priorities in `assemble()`:
```rust
let protected = &[50u32, 55, 75, 100, 500, 501, 1200];
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib prompt_pipeline -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_pipeline.rs
git commit -m "thinker: register InboundContextLayer and WorkspaceFilesLayer in pipeline"
```

---

## Phase 2: Migrate (Soft Deprecation)

### Task 8: Refactor SoulLayer to Prefer SOUL.md

**Files:**
- Modify: `core/src/thinker/layers/soul.rs`

**Step 1: Write failing test for new behavior**

Add test:
```rust
#[test]
fn prefers_workspace_soul_over_manifest() {
    let layer = SoulLayer;
    let config = PromptConfig::default();
    let tools = vec![];
    let soul = SoulManifest {
        identity: "I am old Aleph.".to_string(),
        ..Default::default()
    };
    let ws = crate::thinker::workspace_files::WorkspaceFiles {
        workspace_dir: "/tmp".into(),
        files: vec![crate::thinker::workspace_files::WorkspaceFile {
            name: "SOUL.md",
            content: Some("I am new Aleph from workspace.".into()),
            truncated: false,
            original_size: 30,
        }],
    };
    let input = LayerInput::soul(&config, &tools, &soul).with_workspace(&ws);
    let mut out = String::new();
    layer.inject(&mut out, &input);

    // Should use workspace SOUL.md, not SoulManifest
    assert!(out.contains("I am new Aleph from workspace."));
    assert!(!out.contains("I am old Aleph."));
}

#[test]
fn falls_back_to_manifest_when_no_workspace_soul() {
    let layer = SoulLayer;
    let config = PromptConfig::default();
    let tools = vec![];
    let soul = SoulManifest {
        identity: "I am Aleph.".to_string(),
        directives: vec!["Be helpful".into()],
        ..Default::default()
    };
    let ws = crate::thinker::workspace_files::WorkspaceFiles {
        workspace_dir: "/tmp".into(),
        files: vec![], // no SOUL.md
    };
    let input = LayerInput::soul(&config, &tools, &soul).with_workspace(&ws);
    let mut out = String::new();
    layer.inject(&mut out, &input);

    // Should fall back to SoulManifest
    assert!(out.contains("I am Aleph."));
}
```

**Step 2: Run test to verify it fails**

Expected: First test FAIL (still uses SoulManifest)

**Step 3: Modify SoulLayer.inject()**

```rust
fn inject(&self, output: &mut String, input: &LayerInput) {
    // Priority 1: workspace SOUL.md
    if let Some(soul_content) = input.workspace_file("SOUL.md") {
        output.push_str("# Soul\n\n");
        output.push_str(soul_content);
        output.push_str("\n\n---\n\n");
        return;
    }

    // Priority 2: SoulManifest (legacy fallback)
    let soul = match input.soul {
        Some(s) => s,
        None => return,
    };
    // ... existing SoulManifest rendering logic unchanged ...
}
```

**Step 4: Run all soul tests**

Run: `cargo test -p alephcore --lib layers::soul -- --nocapture`
Expected: All tests PASS

**Step 5: Commit**

```bash
git add core/src/thinker/layers/soul.rs
git commit -m "thinker: SoulLayer prefers workspace SOUL.md over SoulManifest"
```

---

### Task 9: Refactor ProfileLayer to Prefer AGENTS.md

**Files:**
- Modify: `core/src/thinker/layers/profile.rs`

**Step 1: Write failing test**

```rust
#[test]
fn prefers_workspace_agents_over_profile_prompt() {
    let layer = ProfileLayer;
    let config = PromptConfig::default();
    let tools = vec![];
    let soul = SoulManifest::default();
    let profile = ProfileConfig {
        system_prompt: Some("Old profile prompt.".into()),
        ..Default::default()
    };
    let ws = crate::thinker::workspace_files::WorkspaceFiles {
        workspace_dir: "/tmp".into(),
        files: vec![crate::thinker::workspace_files::WorkspaceFile {
            name: "AGENTS.md",
            content: Some("# Project Rules\nAlways run tests.".into()),
            truncated: false,
            original_size: 35,
        }],
    };
    let input = LayerInput::soul(&config, &tools, &soul)
        .with_profile(Some(&profile))
        .with_workspace(&ws);
    let mut out = String::new();
    layer.inject(&mut out, &input);

    assert!(out.contains("Project Rules"));
    assert!(!out.contains("Old profile prompt."));
}
```

**Step 2: Modify ProfileLayer.inject()**

```rust
fn inject(&self, output: &mut String, input: &LayerInput) {
    // Priority 1: workspace AGENTS.md
    if let Some(agents_content) = input.workspace_file("AGENTS.md") {
        output.push_str("## Project Context\n\n");
        output.push_str(agents_content);
        output.push_str("\n\n");
        return;
    }

    // Priority 2: ProfileConfig.system_prompt (legacy fallback)
    let profile = match input.profile {
        Some(p) => p,
        None => return,
    };
    let prompt = match profile.system_prompt.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };
    output.push_str("## Current Role Context\n\n");
    output.push_str(prompt);
    output.push_str("\n\n");
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib layers::profile -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/thinker/layers/profile.rs
git commit -m "thinker: ProfileLayer prefers workspace AGENTS.md over ProfileConfig.system_prompt"
```

---

### Task 10: Deprecate CustomInstructionsLayer

**Files:**
- Modify: `core/src/thinker/layers/custom_instructions.rs`

**Step 1: Add deprecation logic**

The layer should check: if workspace has `IDENTITY.md`, skip `custom_instructions` (it's handled by WorkspaceFilesLayer).

```rust
fn inject(&self, output: &mut String, input: &LayerInput) {
    // If workspace IDENTITY.md exists, skip — handled by WorkspaceFilesLayer
    if input.workspace_file("IDENTITY.md").is_some() {
        return;
    }

    // Legacy fallback
    if let Some(instructions) = &input.config.custom_instructions {
        let instructions = sanitize_for_prompt(instructions, SanitizeLevel::Moderate);
        let instructions = sanitize_for_prompt(&instructions, SanitizeLevel::Light);
        output.push_str("## Additional Instructions\n");
        output.push_str(&instructions);
        output.push_str("\n\n");
    }
}
```

**Step 2: Add test**

```rust
#[test]
fn skips_when_workspace_identity_exists() {
    let layer = CustomInstructionsLayer;
    let config = PromptConfig {
        custom_instructions: Some("Old instructions.".to_string()),
        ..Default::default()
    };
    let tools = vec![];
    let ws = crate::thinker::workspace_files::WorkspaceFiles {
        workspace_dir: "/tmp".into(),
        files: vec![crate::thinker::workspace_files::WorkspaceFile {
            name: "IDENTITY.md",
            content: Some("User preferences from file.".into()),
            truncated: false,
            original_size: 27,
        }],
    };
    let input = LayerInput::basic(&config, &tools).with_workspace(&ws);
    let mut out = String::new();
    layer.inject(&mut out, &input);

    assert!(out.is_empty()); // skipped because IDENTITY.md exists
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib layers::custom_instructions -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/thinker/layers/custom_instructions.rs
git commit -m "thinker: deprecate CustomInstructionsLayer when IDENTITY.md exists"
```

---

### Task 11: Update Existing BootstrapLayer File List

**Files:**
- Modify: `core/src/thinker/layers/bootstrap.rs:12-19`

**Step 1: Align BootstrapLayer with new workspace file spec**

Update `BOOTSTRAP_FILES` constant to match the new standard:

```rust
const BOOTSTRAP_FILES: &[&str] = &[
    "SOUL.md",
    "IDENTITY.md",
    "AGENTS.md",
    "TOOLS.md",
    "MEMORY.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
];
```

This aligns BootstrapLayer (which loads raw files from disk) with the `WorkspaceFiles` spec. Note: BootstrapLayer is used when `with_bootstrap()` is called on the pipeline (separate from WorkspaceFilesLayer which reads from `LayerInput.workspace`).

**Step 2: Update tests**

Update `loads_user_and_agents_files` test to match new file names. Add test for new files.

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib layers::bootstrap -- --nocapture`
Expected: All tests PASS

**Step 4: Commit**

```bash
git add core/src/thinker/layers/bootstrap.rs
git commit -m "thinker: align BootstrapLayer file list with workspace files spec"
```

---

### Task 12: Wire InboundContext from Gateway

**Files:**
- Modify: Gateway handler that calls Thinker (find via `grep "build_prompt" core/src/thinker/mod.rs`)
- Modify: `core/src/thinker/mod.rs` — `ThinkerConfig` + `build_prompt()`

**Step 1: Add `inbound_context` to ThinkerConfig**

In `core/src/thinker/mod.rs`:

```rust
pub struct ThinkerConfig {
    // ... existing fields ...
    pub soul: Option<soul::SoulManifest>,
    pub active_profile: Option<crate::config::ProfileConfig>,
    pub bootstrap_workspace: Option<std::path::PathBuf>,
    // New
    pub workspace_files: Option<workspace_files::WorkspaceFiles>,
    pub inbound_context: Option<inbound_context::InboundContext>,
}
```

**Step 2: Update build_prompt() to pass through**

In `Thinker::build_prompt()`, pass `workspace_files` and `inbound_context` through to `LayerInput`:

```rust
let input = LayerInput::soul(&config, &tools, &soul)
    .with_profile(self.config.active_profile.as_ref())
    .with_workspace(self.config.workspace_files.as_ref())  // NEW
    .with_inbound(self.config.inbound_context.as_ref());    // NEW (unwrap Option)
```

Note: The `with_workspace` and `with_inbound` methods take `&T`, so we need to handle the `Option`. Add convenience methods that accept `Option<&T>`:

```rust
pub fn with_workspace_opt(mut self, workspace: Option<&'a WorkspaceFiles>) -> Self {
    self.workspace = workspace;
    self
}

pub fn with_inbound_opt(mut self, inbound: Option<&'a InboundContext>) -> Self {
    self.inbound = inbound;
    self
}
```

**Step 3: Run compilation check**

Run: `cargo check -p alephcore`
Expected: PASS (no test changes needed — Gateway wiring is additive)

**Step 4: Commit**

```bash
git add core/src/thinker/mod.rs core/src/thinker/prompt_layer.rs
git commit -m "thinker: wire WorkspaceFiles and InboundContext through ThinkerConfig"
```

---

### Task 13: Config-Driven Extra Files

**Files:**
- Modify: `core/src/config/types/` — add `prompt` config section
- Create: `core/src/thinker/hooks/config_extra_files.rs` (built-in hook)

**Step 1: Add config section**

In the appropriate config types file, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct PromptExtraFilesConfig {
    pub enabled: bool,
    pub paths: Vec<String>,
}
```

**Step 2: Implement ConfigExtraFilesHook**

```rust
pub struct ConfigExtraFilesHook {
    workspace_dir: PathBuf,
    paths: Vec<String>,
}

impl ExtraFilesHook for ConfigExtraFilesHook {
    fn name(&self) -> &str { "config_extra_files" }
    fn extra_files(&self, _ctx: &ExtraFilesContext) -> Result<Vec<ExtraFile>> {
        let mut files = Vec::new();
        for path in &self.paths {
            let full_path = self.workspace_dir.join(path);
            if let Ok(content) = std::fs::read_to_string(&full_path) {
                if !content.trim().is_empty() {
                    files.push(ExtraFile {
                        path: path.clone(),
                        content,
                    });
                }
            }
        }
        Ok(files)
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p alephcore --lib hooks -- --nocapture`
Expected: PASS

**Step 4: Commit**

```bash
git add -A
git commit -m "config: add prompt.extra_files config + built-in ExtraFilesHook"
```

---

## Phase 3: Clean Up (Future — Document Only)

> These tasks are for the next major version. Do NOT implement now.

### Task 14 (Future): Remove SoulManifest struct

- Delete `core/src/thinker/soul.rs` (entire file, 908 lines)
- Delete `core/src/thinker/identity.rs` (entire file, 321 lines)
- Remove `soul` field from `LayerInput`
- Remove `soul` field from `ThinkerConfig`
- Update all callers to use workspace `SOUL.md` exclusively

### Task 15 (Future): Remove ProfileConfig.system_prompt

- Remove `system_prompt` field from `ProfileConfig`
- Remove legacy fallback path in `ProfileLayer`
- Update all callers to use workspace `AGENTS.md` exclusively

### Task 16 (Future): Remove CustomInstructionsLayer

- Delete `core/src/thinker/layers/custom_instructions.rs`
- Remove from `layers/mod.rs`
- Remove from `PromptPipeline::default_layers()`
- Remove `custom_instructions` field from `PromptConfig`

### Task 17 (Future): Remove old PromptHook trait

- Delete `core/src/thinker/prompt_hooks.rs`
- Rename `prompt_hooks_v2.rs` → `prompt_hooks.rs`
- Update all callers
