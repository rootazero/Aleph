# Model-Perceivable Ecosystem Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the LLM aware of available sub-agents and MCP tools via lightweight prompt catalog + on-demand discovery tools.

**Architecture:** Two new prompt layers (AgentCatalogLayer at priority 505 Stable, McpToolIndexLayer at priority 1065 Dynamic) inject compact indexes. Two new tools (agent_info, mcp_tool_schema) provide full details on demand. Follows existing two-stage discovery pattern (ToolsLayer + tool_index, SkillInstructionsLayer + skill_read).

**Tech Stack:** Rust, existing PromptLayer trait, existing AlephTool trait, schemars for JSON Schema.

**Note:** The working tree already has in-progress changes: `mcp_instructions.rs` (untracked new file), and modifications to `prompt_layer.rs`, `prompt_pipeline.rs`, `layers/mod.rs`. This plan builds on top of those changes.

---

### Task 1: Extend AgentDef with description and when_to_use

**Files:**
- Modify: `src/agents/types.rs`

- [ ] **Step 1: Write failing tests for new fields**

Add to the existing `tests` module in `src/agents/types.rs`:

```rust
#[test]
fn test_agent_def_description_default() {
    let agent = AgentDef::new("test", AgentMode::SubAgent);
    assert!(agent.description.is_empty());
    assert!(agent.when_to_use.is_none());
}

#[test]
fn test_with_description() {
    let agent = AgentDef::new("test", AgentMode::SubAgent)
        .with_description("A test agent");
    assert_eq!(agent.description, "A test agent");
}

#[test]
fn test_with_when_to_use() {
    let agent = AgentDef::new("test", AgentMode::SubAgent)
        .with_when_to_use("When you need testing");
    assert_eq!(agent.when_to_use.as_deref(), Some("When you need testing"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib agents::types::tests::test_agent_def_description_default`
Expected: FAIL — `description` field does not exist

- [ ] **Step 3: Add fields and builder methods to AgentDef**

In `src/agents/types.rs`, add two fields to the `AgentDef` struct after `id`:

```rust
pub struct AgentDef {
    pub id: String,
    /// One-line description for catalog index
    pub description: String,
    /// Usage trigger hint for the model
    pub when_to_use: Option<String>,
    pub mode: AgentMode,
    // ... rest unchanged
}
```

Update `AgentDef::new()` to initialize the new fields:

```rust
pub fn new(id: impl Into<String>, mode: AgentMode) -> Self {
    Self {
        id: id.into(),
        description: String::new(),
        when_to_use: None,
        mode,
        // ... rest unchanged
    }
}
```

Add builder methods after the existing `with_model_hint`:

```rust
/// Set description
pub fn with_description(mut self, desc: impl Into<String>) -> Self {
    self.description = desc.into();
    self
}

/// Set when_to_use hint
pub fn with_when_to_use(mut self, hint: impl Into<String>) -> Self {
    self.when_to_use = Some(hint.into());
    self
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::types`
Expected: ALL PASS (existing tests + 3 new tests)

- [ ] **Step 5: Commit**

```bash
git add src/agents/types.rs
git commit -m "feat(agents): add description and when_to_use fields to AgentDef"
```

---

### Task 2: Update builtin_agents() with descriptions

**Files:**
- Modify: `src/agents/registry.rs`

- [ ] **Step 1: Write failing test**

Add to existing tests in `src/agents/registry.rs`:

```rust
#[test]
fn test_builtin_subagents_have_descriptions() {
    let registry = AgentRegistry::with_builtins();
    let subagents = registry.list_subagents();
    for agent in &subagents {
        assert!(
            !agent.description.is_empty(),
            "Agent '{}' should have a description",
            agent.id
        );
    }
}

#[test]
fn test_builtin_subagents_have_when_to_use() {
    let registry = AgentRegistry::with_builtins();
    let subagents = registry.list_subagents();
    for agent in &subagents {
        assert!(
            agent.when_to_use.is_some(),
            "Agent '{}' should have when_to_use",
            agent.id
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib agents::registry::tests::test_builtin_subagents_have_descriptions`
Expected: FAIL — descriptions are empty

- [ ] **Step 3: Add descriptions to all builtin sub-agents**

Update each `AgentDef::new(...)` call in `builtin_agents()` in `src/agents/registry.rs`:

```rust
pub fn builtin_agents() -> Vec<AgentDef> {
    vec![
        // Main agent - full access (no description needed, not in catalog)
        AgentDef::new("main", AgentMode::Primary)
            .with_description("Primary agent that responds directly to user"),
        // Explore agent - read-only tools
        AgentDef::new("explore", AgentMode::SubAgent)
            .with_description("Read-only codebase exploration specialist")
            .with_when_to_use(
                "When you need to search, read, or understand code without modifying anything",
            )
            .with_prompt_sections(vec!["explore_constraints".into()])
            .with_allowed_tools(vec![
                "glob".into(),
                "grep".into(),
                "read_file".into(),
                "web_fetch".into(),
                "search".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(20),
        // Coder agent - file operations
        AgentDef::new("coder", AgentMode::SubAgent)
            .with_description("Code writing specialist with file operations")
            .with_when_to_use("When you need to write, edit, or create code files")
            .with_prompt_sections(vec!["coder_guidelines".into()])
            .with_allowed_tools(vec![
                "read_file".into(),
                "write_file".into(),
                "edit_file".into(),
                "glob".into(),
                "grep".into(),
                "bash".into(),
            ])
            .with_max_iterations(30),
        // Researcher agent - search and web
        AgentDef::new("researcher", AgentMode::SubAgent)
            .with_description("Web and document research specialist")
            .with_when_to_use(
                "When you need to search the web, fetch URLs, or gather external information",
            )
            .with_prompt_sections(vec!["researcher_protocol".into()])
            .with_allowed_tools(vec![
                "search".into(),
                "web_fetch".into(),
                "read_file".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(15),
        // Default agent - general-purpose sub-agent
        AgentDef::new("default", AgentMode::SubAgent)
            .with_description("General-purpose sub-agent")
            .with_when_to_use("When no specialized agent fits the task")
            .with_context_mode(ContextMode::Summary),
        // Plan agent - read-only planner
        AgentDef::new("plan", AgentMode::SubAgent)
            .with_description("Read-only planning and analysis specialist")
            .with_when_to_use(
                "When you need to analyze requirements, design architecture, or create plans",
            )
            .with_prompt_sections(vec!["plan_protocol".into()])
            .with_allowed_tools(vec![
                "glob".into(),
                "grep".into(),
                "read_file".into(),
                "bash".into(),
            ])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into()])
            .with_max_iterations(20)
            .with_context_mode(ContextMode::Summary),
        // Verify agent - adversarial verifier (read-only)
        AgentDef::new("verify", AgentMode::SubAgent)
            .with_description("Adversarial verification specialist")
            .with_when_to_use(
                "When you need to independently verify that work was done correctly",
            )
            .with_prompt_sections(vec!["verify_protocol".into()])
            .with_allowed_tools(vec!["*".into()])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into()])
            .with_max_iterations(25)
            .with_context_mode(ContextMode::Summary),
    ]
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::registry`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src/agents/registry.rs
git commit -m "feat(agents): add descriptions and when_to_use to builtin agents"
```

---

### Task 3: Add AgentCatalogEntry and McpToolIndexEntry to prompt_layer.rs

**Files:**
- Modify: `src/thinker/prompt_layer.rs`

- [ ] **Step 1: Add AgentCatalogEntry type**

After the `McpServerInstruction` struct (around line 18), add:

```rust
/// Lightweight agent catalog entry for prompt injection.
///
/// Contains only the fields needed for the catalog index —
/// use `agent_info` tool for full details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCatalogEntry {
    pub id: String,
    pub description: String,
    pub when_to_use: Option<String>,
}
```

- [ ] **Step 2: Add McpToolIndexEntry type**

After `AgentCatalogEntry`, add:

```rust
/// Lightweight MCP tool index entry for prompt injection.
///
/// Contains only name + description — use `mcp_tool_schema`
/// tool for full parameter schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolIndexEntry {
    pub server_name: String,
    pub tool_name: String,
    pub description: String,
}
```

- [ ] **Step 3: Add fields to LayerInput**

In the `LayerInput` struct, after the `mcp_instructions` field (line 85), add:

```rust
    /// MCP tool index for prompt injection.
    ///
    /// When set, `McpToolIndexLayer` injects a per-server tool index
    /// into the system prompt (name + description only).
    pub mcp_tool_index: Option<&'a [McpToolIndexEntry]>,
```

- [ ] **Step 4: Update all LayerInput constructors to initialize the new field**

In `basic()`, `hydration()`, `soul()`, and `context()` constructors, add after `mcp_instructions: None,`:

```rust
            mcp_tool_index: None,
```

- [ ] **Step 5: Add with_mcp_tool_index builder method**

After the `with_mcp_instructions` method (line 232), add:

```rust
    /// Attach MCP tool index for prompt injection.
    pub fn with_mcp_tool_index(mut self, index: &'a [McpToolIndexEntry]) -> Self {
        self.mcp_tool_index = Some(index);
        self
    }
```

- [ ] **Step 6: Add available_agents to PromptConfig**

In `src/thinker/prompt_builder/mod.rs`, add after `eligible_skills` field (line 96):

```rust
    /// Available agent catalog entries for AgentCatalogLayer.
    pub available_agents: Option<Vec<crate::thinker::prompt_layer::AgentCatalogEntry>>,
```

And in the `Default` impl, add after `eligible_skills: None,`:

```rust
            available_agents: None,
```

- [ ] **Step 7: Run cargo check to verify compilation**

Run: `cargo check -p alephcore`
Expected: PASS (all new fields are Option with None defaults)

- [ ] **Step 8: Commit**

```bash
git add src/thinker/prompt_layer.rs src/thinker/prompt_builder/mod.rs
git commit -m "feat(thinker): add AgentCatalogEntry, McpToolIndexEntry types and LayerInput fields"
```

---

### Task 4: Implement AgentCatalogLayer

**Files:**
- Create: `src/thinker/layers/agent_catalog.rs`
- Modify: `src/thinker/layers/mod.rs`

- [ ] **Step 1: Create agent_catalog.rs with tests**

Create `src/thinker/layers/agent_catalog.rs`:

```rust
//! AgentCatalogLayer — sub-agent catalog index for primary agent awareness (priority 505)

use crate::thinker::prompt_layer::{AgentCatalogEntry, AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct AgentCatalogLayer;

impl PromptLayer for AgentCatalogLayer {
    fn name(&self) -> &'static str {
        "agent_catalog"
    }
    fn priority(&self) -> u32 {
        505
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let agents = match input.config.available_agents {
            Some(ref agents) if !agents.is_empty() => agents,
            _ => return,
        };

        // Filter: only agents with non-empty description
        let visible: Vec<&AgentCatalogEntry> =
            agents.iter().filter(|a| !a.description.is_empty()).collect();

        if visible.is_empty() {
            return;
        }

        output.push_str("## Available Agents\n\n");
        output.push_str(
            "You can delegate tasks to specialized sub-agents using the `delegate` tool.\n",
        );
        output.push_str(
            "Use `agent_info(agent_id)` to get detailed capabilities before delegating.\n\n",
        );
        output.push_str(&build_agent_catalog_xml(&visible));
        output.push_str("\n\n");
    }
}

/// Build XML catalog of available agents.
fn build_agent_catalog_xml(agents: &[&AgentCatalogEntry]) -> String {
    let mut buf = String::from("<available_agents>\n");
    for agent in agents {
        buf.push_str("  <agent>\n");
        buf.push_str("    <id>");
        buf.push_str(&escape_xml(&agent.id));
        buf.push_str("</id>\n");
        buf.push_str("    <description>");
        buf.push_str(&escape_xml(&agent.description));
        buf.push_str("</description>\n");
        if let Some(ref when) = agent.when_to_use {
            buf.push_str("    <when>");
            buf.push_str(&escape_xml(when));
            buf.push_str("</when>\n");
        }
        buf.push_str("  </agent>\n");
    }
    buf.push_str("</available_agents>");
    buf
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    fn entry(id: &str, desc: &str, when: Option<&str>) -> AgentCatalogEntry {
        AgentCatalogEntry {
            id: id.to_string(),
            description: desc.to_string(),
            when_to_use: when.map(|s| s.to_string()),
        }
    }

    #[test]
    fn injects_agent_catalog() {
        let layer = AgentCatalogLayer;
        let agents = vec![
            entry("explore", "Read-only explorer", Some("When searching code")),
            entry("coder", "Code writer", None),
        ];
        let config = PromptConfig {
            available_agents: Some(agents),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Available Agents"));
        assert!(out.contains("<id>explore</id>"));
        assert!(out.contains("<description>Read-only explorer</description>"));
        assert!(out.contains("<when>When searching code</when>"));
        assert!(out.contains("<id>coder</id>"));
        assert!(!out.contains("<when></when>")); // no empty when tag for coder
    }

    #[test]
    fn filters_empty_descriptions() {
        let layer = AgentCatalogLayer;
        let agents = vec![
            entry("visible", "Has desc", None),
            entry("hidden", "", None),
        ];
        let config = PromptConfig {
            available_agents: Some(agents),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("visible"));
        assert!(!out.contains("hidden"));
    }

    #[test]
    fn empty_agents_no_output() {
        let layer = AgentCatalogLayer;
        let config = PromptConfig {
            available_agents: Some(vec![]),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn none_agents_no_output() {
        let layer = AgentCatalogLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn xml_escaping() {
        let layer = AgentCatalogLayer;
        let agents = vec![entry("a&b", "Uses <tags>", Some("When & if"))];
        let config = PromptConfig {
            available_agents: Some(agents),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("a&amp;b"));
        assert!(out.contains("&lt;tags&gt;"));
        assert!(out.contains("When &amp; if"));
    }

    #[test]
    fn priority_is_505() {
        assert_eq!(AgentCatalogLayer.priority(), 505);
    }

    #[test]
    fn full_mode_only() {
        assert!(AgentCatalogLayer.supports_mode(PromptMode::Full));
        assert!(!AgentCatalogLayer.supports_mode(PromptMode::Compact));
        assert!(!AgentCatalogLayer.supports_mode(PromptMode::Minimal));
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib thinker::layers::agent_catalog`
Expected: ALL PASS

- [ ] **Step 3: Commit**

```bash
git add src/thinker/layers/agent_catalog.rs
git commit -m "feat(thinker): implement AgentCatalogLayer with XML catalog generation"
```

---

### Task 5: Implement McpToolIndexLayer

**Files:**
- Create: `src/thinker/layers/mcp_tool_index.rs`

- [ ] **Step 1: Create mcp_tool_index.rs with tests**

Create `src/thinker/layers/mcp_tool_index.rs`:

```rust
//! McpToolIndexLayer — MCP server tool index injection (priority 1065)

use crate::thinker::prompt_layer::{
    AssemblyPath, LayerInput, LayerStability, McpToolIndexEntry, PromptLayer,
};
use crate::thinker::prompt_mode::PromptMode;
use std::collections::BTreeMap;

pub struct McpToolIndexLayer;

impl PromptLayer for McpToolIndexLayer {
    fn name(&self) -> &'static str {
        "mcp_tool_index"
    }
    fn priority(&self) -> u32 {
        1065
    }
    fn stability(&self) -> LayerStability {
        LayerStability::Dynamic
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let entries = match input.mcp_tool_index {
            Some(items) if !items.is_empty() => items,
            _ => return,
        };

        // Group by server_name using BTreeMap for deterministic ordering
        let mut by_server: BTreeMap<&str, Vec<&McpToolIndexEntry>> = BTreeMap::new();
        for entry in entries {
            by_server
                .entry(&entry.server_name)
                .or_default()
                .push(entry);
        }

        output.push_str("## MCP Server Tools\n\n");
        output.push_str(
            "The following tools are provided by connected MCP servers.\n\
             Use `mcp_tool_schema(tool_name)` to get full parameter schema before calling.\n\n",
        );

        for (server, tools) in &by_server {
            output.push_str("### ");
            output.push_str(server);
            output.push('\n');
            for tool in tools {
                output.push_str("- ");
                output.push_str(&tool.tool_name);
                output.push_str(" — ");
                output.push_str(&tool.description);
                output.push('\n');
            }
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    fn entry(server: &str, tool: &str, desc: &str) -> McpToolIndexEntry {
        McpToolIndexEntry {
            server_name: server.to_string(),
            tool_name: tool.to_string(),
            description: desc.to_string(),
        }
    }

    #[test]
    fn injects_grouped_by_server() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let entries = vec![
            entry("github", "github:create_issue", "Create an issue"),
            entry("github", "github:list_pulls", "List pull requests"),
            entry("slack", "slack:send_message", "Send a message"),
        ];
        let input = LayerInput::basic(&config, &[]).with_mcp_tool_index(&entries);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## MCP Server Tools"));
        assert!(out.contains("### github"));
        assert!(out.contains("- github:create_issue — Create an issue"));
        assert!(out.contains("- github:list_pulls — List pull requests"));
        assert!(out.contains("### slack"));
        assert!(out.contains("- slack:send_message — Send a message"));
    }

    #[test]
    fn servers_sorted_alphabetically() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let entries = vec![
            entry("slack", "slack:send", "Send"),
            entry("github", "github:list", "List"),
        ];
        let input = LayerInput::basic(&config, &[]).with_mcp_tool_index(&entries);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        let github_pos = out.find("### github").unwrap();
        let slack_pos = out.find("### slack").unwrap();
        assert!(github_pos < slack_pos, "github should come before slack");
    }

    #[test]
    fn empty_entries_no_output() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let entries: Vec<McpToolIndexEntry> = vec![];
        let input = LayerInput::basic(&config, &[]).with_mcp_tool_index(&entries);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn none_entries_no_output() {
        let layer = McpToolIndexLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn stability_is_dynamic() {
        assert_eq!(McpToolIndexLayer.stability(), LayerStability::Dynamic);
    }

    #[test]
    fn priority_is_1065() {
        assert_eq!(McpToolIndexLayer.priority(), 1065);
    }

    #[test]
    fn full_mode_only() {
        assert!(McpToolIndexLayer.supports_mode(PromptMode::Full));
        assert!(!McpToolIndexLayer.supports_mode(PromptMode::Compact));
        assert!(!McpToolIndexLayer.supports_mode(PromptMode::Minimal));
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib thinker::layers::mcp_tool_index`
Expected: ALL PASS

- [ ] **Step 3: Commit**

```bash
git add src/thinker/layers/mcp_tool_index.rs
git commit -m "feat(thinker): implement McpToolIndexLayer with server-grouped tool index"
```

---

### Task 6: Register new layers in mod.rs and pipeline

**Files:**
- Modify: `src/thinker/layers/mod.rs`
- Modify: `src/thinker/prompt_pipeline.rs`

- [ ] **Step 1: Add to layers/mod.rs**

In `src/thinker/layers/mod.rs`, add under the `// --- Config-gated layers ---` section (after `mod skill_instructions;`):

```rust
mod agent_catalog;
mod mcp_tool_index;
```

Add to the re-exports section at the bottom:

```rust
pub use agent_catalog::AgentCatalogLayer;
pub use mcp_tool_index::McpToolIndexLayer;
```

- [ ] **Step 2: Add to prompt_pipeline.rs default_layers()**

In `src/thinker/prompt_pipeline.rs`, in the `default_layers()` function, add after `Box::new(HydratedToolsLayer),`:

```rust
            Box::new(AgentCatalogLayer),
```

And add after `Box::new(McpInstructionsLayer),`:

```rust
            Box::new(McpToolIndexLayer),
```

- [ ] **Step 3: Update default_layers() doc comment**

Update the doc comment for `default_layers()` to include the new layers:

Add `///  505  AgentCatalogLayer` after the `///  500  ToolsLayer + HydratedToolsLayer` line.

Add `/// 1065  McpToolIndexLayer` after the `/// 1705  McpInstructionsLayer` line (or wherever McpInstructionsLayer is listed).

- [ ] **Step 4: Update test expectations**

In `src/thinker/prompt_pipeline.rs`, update:

1. `test_default_layers_count`: Change expected count from 30 to 32 (or whatever the current count is + 2).

2. `compact_mode_excludes_heavy_layers`: Add `"agent_catalog"` and `"mcp_tool_index"` to the `excluded_in_compact` array.

3. `dynamic_layers_are_correctly_classified`: Add `"mcp_tool_index"` to the `dynamic_names` assertions and update the expected count from 7 to 8 (or current + 1).

- [ ] **Step 5: Run all pipeline tests**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline`
Expected: ALL PASS

- [ ] **Step 6: Run full cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/thinker/layers/mod.rs src/thinker/prompt_pipeline.rs
git commit -m "feat(thinker): register AgentCatalogLayer and McpToolIndexLayer in pipeline"
```

---

### Task 7: Implement agent_info tool

**Files:**
- Create: `src/builtin_tools/agent_manage/info.rs`
- Modify: `src/builtin_tools/agent_manage/mod.rs`

- [ ] **Step 1: Create info.rs**

Create `src/builtin_tools/agent_manage/info.rs`:

```rust
//! AgentInfoTool — return full agent definition details for a given agent ID.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::AgentRegistry;
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for agent_info tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentInfoArgs {
    /// Agent ID to look up (e.g., "explore", "coder", "researcher")
    pub agent_id: String,
}

/// Detailed agent information returned by agent_info.
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfoOutput {
    pub id: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    pub mode: String,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
    pub context_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u32>,
}

impl fmt::Display for AgentInfoOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Agent: {} ({})", self.id, self.mode)?;
        writeln!(f, "  description: {}", self.description)?;
        if let Some(ref when) = self.when_to_use {
            writeln!(f, "  when_to_use: {}", when)?;
        }
        writeln!(f, "  allowed_tools: {}", self.allowed_tools.join(", "))?;
        if !self.denied_tools.is_empty() {
            writeln!(f, "  denied_tools: {}", self.denied_tools.join(", "))?;
        }
        if let Some(max) = self.max_iterations {
            writeln!(f, "  max_iterations: {}", max)?;
        }
        writeln!(f, "  context_mode: {}", self.context_mode)?;
        Ok(())
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that returns full details about a registered agent definition.
#[derive(Clone)]
pub struct AgentInfoTool {
    registry: Arc<AgentRegistry>,
}

impl AgentInfoTool {
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl AlephTool for AgentInfoTool {
    const NAME: &'static str = "agent_info";
    const DESCRIPTION: &'static str =
        "Get detailed capabilities and configuration of a registered agent. \
         Returns allowed/denied tools, iteration limits, context mode, and usage hints.";

    type Args = AgentInfoArgs;
    type Output = AgentInfoOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"agent_info({"agent_id": "explore"})"#.to_string(),
            r#"agent_info({"agent_id": "coder"})"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(agent_id = %args.agent_id, "agent_info requested");

        let agent_def = self.registry.get(&args.agent_id).ok_or_else(|| {
            let available = self.registry.list_ids().join(", ");
            AlephError::NotFound(format!(
                "Agent '{}' not found. Available agents: {}",
                args.agent_id, available
            ))
        })?;

        Ok(AgentInfoOutput {
            id: agent_def.id,
            description: agent_def.description,
            when_to_use: agent_def.when_to_use,
            mode: format!("{:?}", agent_def.mode),
            allowed_tools: agent_def.allowed_tools,
            denied_tools: agent_def.denied_tools,
            max_iterations: agent_def.max_iterations,
            context_mode: format!("{:?}", agent_def.context_mode),
            model_hint: agent_def.model_hint,
            token_budget: agent_def.token_budget,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{builtin_agents, AgentRegistry};

    fn test_registry() -> Arc<AgentRegistry> {
        Arc::new(AgentRegistry::with_builtins())
    }

    #[tokio::test]
    async fn test_info_existing_agent() {
        let tool = AgentInfoTool::new(test_registry());
        let result = tool
            .call(AgentInfoArgs {
                agent_id: "explore".to_string(),
            })
            .await;
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.id, "explore");
        assert!(!info.description.is_empty());
        assert!(info.when_to_use.is_some());
        assert_eq!(info.mode, "SubAgent");
        assert!(info.allowed_tools.contains(&"glob".to_string()));
        assert!(info.denied_tools.contains(&"bash".to_string()));
    }

    #[tokio::test]
    async fn test_info_not_found() {
        let tool = AgentInfoTool::new(test_registry());
        let result = tool
            .call(AgentInfoArgs {
                agent_id: "nonexistent".to_string(),
            })
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"));
        assert!(err.contains("explore")); // available agents listed
    }

    #[test]
    fn test_tool_definition() {
        let tool = AgentInfoTool::new(test_registry());
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "agent_info");
        assert!(!def.requires_confirmation);
    }
}
```

- [ ] **Step 2: Register in mod.rs**

In `src/builtin_tools/agent_manage/mod.rs`, add:

```rust
pub mod info;
```

And add to the re-exports:

```rust
pub use info::{AgentInfoArgs, AgentInfoOutput, AgentInfoTool};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::agent_manage::info`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/agent_manage/info.rs src/builtin_tools/agent_manage/mod.rs
git commit -m "feat(tools): implement agent_info tool for on-demand agent discovery"
```

---

### Task 8: Implement mcp_tool_schema tool

**Files:**
- Create: `src/builtin_tools/mcp_discover.rs`
- Modify: `src/builtin_tools/mod.rs` (register module)

- [ ] **Step 1: Create mcp_discover.rs**

Create `src/builtin_tools/mcp_discover.rs`:

```rust
//! McpToolSchemaTool — return full MCP tool schema for on-demand discovery.

use std::fmt;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{AlephError, Result};
use crate::mcp::McpClient;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for mcp_tool_schema tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct McpToolSchemaArgs {
    /// Full tool name (e.g., "github:create_issue")
    pub tool_name: String,
}

/// Full MCP tool schema returned by mcp_tool_schema.
#[derive(Debug, Clone, Serialize)]
pub struct McpToolSchemaOutput {
    pub tool_name: String,
    pub server_name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub requires_confirmation: bool,
}

impl fmt::Display for McpToolSchemaOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Tool: {} (server: {})", self.tool_name, self.server_name)?;
        writeln!(f, "  description: {}", self.description)?;
        writeln!(
            f,
            "  schema: {}",
            serde_json::to_string_pretty(&self.input_schema).unwrap_or_default()
        )?;
        writeln!(f, "  requires_confirmation: {}", self.requires_confirmation)?;
        Ok(())
    }
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that returns full parameter schema for an MCP tool.
#[derive(Clone)]
pub struct McpToolSchemaTool {
    mcp_client: Arc<McpClient>,
}

impl McpToolSchemaTool {
    pub fn new(mcp_client: Arc<McpClient>) -> Self {
        Self { mcp_client }
    }
}

#[async_trait]
impl AlephTool for McpToolSchemaTool {
    const NAME: &'static str = "mcp_tool_schema";
    const DESCRIPTION: &'static str =
        "Get the full parameter schema for an MCP server tool. \
         Returns the tool's JSON Schema input definition so you can call it correctly.";

    type Args = McpToolSchemaArgs;
    type Output = McpToolSchemaOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"mcp_tool_schema({"tool_name": "github:create_issue"})"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(tool_name = %args.tool_name, "mcp_tool_schema requested");

        let tools = self.mcp_client.list_tools().await;
        let tool = tools.iter().find(|t| t.name == args.tool_name).ok_or_else(|| {
            let available: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            AlephError::NotFound(format!(
                "MCP tool '{}' not found. Available MCP tools: {}",
                args.tool_name,
                available.join(", ")
            ))
        })?;

        // Extract server name from tool name prefix (e.g., "github:create_issue" → "github")
        let server_name = args
            .tool_name
            .split(':')
            .next()
            .unwrap_or(&args.tool_name)
            .to_string();

        Ok(McpToolSchemaOutput {
            tool_name: tool.name.clone(),
            server_name,
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            requires_confirmation: tool.requires_confirmation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definition() {
        // McpToolSchemaTool requires an McpClient which needs async setup.
        // Test the output serialization instead.
        let output = McpToolSchemaOutput {
            tool_name: "github:create_issue".to_string(),
            server_name: "github".to_string(),
            description: "Create an issue".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" }
                }
            }),
            requires_confirmation: false,
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("github:create_issue"));
        assert!(json.contains("Create an issue"));
        assert!(json.contains("\"type\":\"object\""));
    }

    #[test]
    fn test_server_name_extraction() {
        // The server name is extracted from the tool_name prefix
        let name = "github:create_issue";
        let server = name.split(':').next().unwrap_or(name);
        assert_eq!(server, "github");

        // Edge case: no colon
        let name2 = "standalone_tool";
        let server2 = name2.split(':').next().unwrap_or(name2);
        assert_eq!(server2, "standalone_tool");
    }

    #[test]
    fn test_output_display() {
        let output = McpToolSchemaOutput {
            tool_name: "slack:send".to_string(),
            server_name: "slack".to_string(),
            description: "Send a message".to_string(),
            input_schema: serde_json::json!({}),
            requires_confirmation: true,
        };
        let display = format!("{}", output);
        assert!(display.contains("slack:send"));
        assert!(display.contains("requires_confirmation: true"));
    }
}
```

- [ ] **Step 2: Register in builtin_tools/mod.rs**

In `src/builtin_tools/mod.rs`, add the module declaration:

```rust
pub mod mcp_discover;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib builtin_tools::mcp_discover`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/mcp_discover.rs src/builtin_tools/mod.rs
git commit -m "feat(tools): implement mcp_tool_schema tool for on-demand MCP discovery"
```

---

### Task 9: Integration verification

**Files:** None (read-only verification)

- [ ] **Step 1: Run full cargo check**

Run: `cargo check -p alephcore`
Expected: PASS — zero errors

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: PASS — zero warnings

- [ ] **Step 4: Verify prompt pipeline integration**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline`
Expected: ALL PASS — layer counts, mode filtering, stability tests all pass

- [ ] **Step 5: Final commit (if any fixes needed)**

```bash
git add -A
git commit -m "fix: address clippy warnings and test adjustments for model-perceivable ecosystem"
```
