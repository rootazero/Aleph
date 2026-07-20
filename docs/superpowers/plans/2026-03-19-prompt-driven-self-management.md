# Prompt-Driven Self-Management Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace complex Rust builtin tools for self-management with LLM-driven file operations guided by on-demand configuration knowledge prompts.

**Architecture:** A `read_config_guide(topic)` tool provides progressive disclosure of config knowledge from pre-written Markdown guides. A `vault_store` tool wraps `SharedTokenManager` for encrypted secret management. Existing tools (`config_read`, `config_update`, `soul_update`, `profile_update`, `skill_reader`) are unregistered. The `OperationalGuidelinesLayer` prompt is updated to enable LLM self-management.

**Tech Stack:** Rust, schemars (JSON Schema), tokio async, TOML/Markdown file I/O

**Spec:** `docs/superpowers/specs/2026-03-19-prompt-driven-self-management-design.md`

---

## Chunk 1: New Tools

### Task 1: Create `vault_store` builtin tool

**Files:**
- Create: `src/builtin_tools/vault_store.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create the vault_store tool module**

Create `src/builtin_tools/vault_store.rs`:

```rust
//! VaultStoreTool — manage encrypted secret vault

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::gateway::security::SharedTokenManager;
use crate::tools::traits::AlephTool;
use super::{notify_tool_start, notify_tool_result};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct VaultStoreArgs {
    /// Action to perform
    #[schemars(description = "Action: 'store' to save a secret, 'delete' to remove one, 'list' to see all key names")]
    pub action: VaultAction,
    /// Secret key name (e.g., "provider:openai", "gen:stability"). Required for store/delete.
    #[schemars(description = "Key name for the secret. Convention: provider:{name}, gen:{name}, channel:{type}:{id}")]
    pub key: Option<String>,
    /// Secret value. Required for 'store' action only.
    #[schemars(description = "The secret value to store. Only used with 'store' action.")]
    pub secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VaultAction {
    Store,
    Delete,
    List,
}

#[derive(Debug, Serialize)]
pub struct VaultStoreOutput {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct VaultStoreTool {
    manager: Arc<SharedTokenManager>,
}

impl VaultStoreTool {
    pub fn new(manager: Arc<SharedTokenManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for VaultStoreTool {
    const NAME: &'static str = "vault_store";
    const DESCRIPTION: &'static str = "Manage encrypted secret vault. API keys and sensitive credentials must be stored via this tool, never written directly to config files. Use 'store' to save, 'delete' to remove, 'list' to see key names (values are never returned).";

    type Args = VaultStoreArgs;
    type Output = VaultStoreOutput;

    fn requires_confirmation(&self) -> bool { true }

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"vault_store(action="store", key="provider:openai", secret="sk-...")"#.into(),
            r#"vault_store(action="delete", key="provider:openai")"#.into(),
            r#"vault_store(action="list")"#.into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> anyhow::Result<Self::Output> {
        notify_tool_start(Self::NAME, &format!("{:?}", args.action));

        let result = match args.action {
            VaultAction::Store => {
                let key = args.key.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'key' is required for store action"))?;
                let secret = args.secret.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'secret' is required for store action"))?;
                self.manager.store_secret(key, secret)?;
                VaultStoreOutput {
                    success: true,
                    message: format!("Secret '{}' stored successfully", key),
                    keys: None,
                }
            }
            VaultAction::Delete => {
                let key = args.key.as_deref()
                    .ok_or_else(|| anyhow::anyhow!("'key' is required for delete action"))?;
                let deleted = self.manager.delete_secret(key)?;
                VaultStoreOutput {
                    success: deleted,
                    message: if deleted {
                        format!("Secret '{}' deleted", key)
                    } else {
                        format!("Secret '{}' not found", key)
                    },
                    keys: None,
                }
            }
            VaultAction::List => {
                let names = self.manager.list_secret_names()?;
                VaultStoreOutput {
                    success: true,
                    message: format!("{} secrets stored", names.len()),
                    keys: Some(names),
                }
            }
        };

        notify_tool_result(Self::NAME, &result.message, result.success);
        Ok(result)
    }
}
```

- [ ] **Step 2: Add module and exports to mod.rs**

In `src/builtin_tools/mod.rs`, add:
- Module declaration: `pub mod vault_store;`
- Export: `pub use vault_store::{VaultAction, VaultStoreArgs, VaultStoreOutput, VaultStoreTool};`

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles cleanly

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/vault_store.rs src/builtin_tools/mod.rs
git commit -m "tools: add vault_store builtin tool for encrypted secret management"
```

---

### Task 2: Create `read_config_guide` builtin tool

**Files:**
- Create: `src/builtin_tools/config_guide.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create the config_guide tool module**

Create `src/builtin_tools/config_guide.rs`:

```rust
//! ReadConfigGuideTool — progressive disclosure of configuration knowledge

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::tools::traits::AlephTool;
use super::{notify_tool_start, notify_tool_result};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadConfigGuideArgs {
    /// Topic to get configuration guide for
    #[schemars(description = "Configuration domain: overview (all domains + file paths), providers (LLM provider config + vault), mcp (MCP server config), skills (skill install + format), agents (agent workspace + SOUL.md), general (general/memory/policies), generation (image/speech/video providers), channels (Telegram/Discord config), cron (scheduled tasks)")]
    pub topic: GuideTopic,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuideTopic {
    Overview,
    Providers,
    Mcp,
    Skills,
    Agents,
    General,
    Generation,
    Channels,
    Cron,
}

impl GuideTopic {
    fn filename(&self) -> &'static str {
        match self {
            Self::Overview => "overview.md",
            Self::Providers => "providers.md",
            Self::Mcp => "mcp.md",
            Self::Skills => "skills.md",
            Self::Agents => "agents.md",
            Self::General => "general.md",
            Self::Generation => "generation.md",
            Self::Channels => "channels.md",
            Self::Cron => "cron.md",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReadConfigGuideOutput {
    pub success: bool,
    pub topic: String,
    pub content: String,
}

#[derive(Clone)]
pub struct ReadConfigGuideTool {
    guides_dir: PathBuf,
}

impl ReadConfigGuideTool {
    pub fn new(guides_dir: PathBuf) -> Self {
        Self { guides_dir }
    }

    /// Default guides directory: ~/.aleph/guides/
    pub fn default_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph")
            .join("guides")
    }
}

impl Default for ReadConfigGuideTool {
    fn default() -> Self {
        Self::new(Self::default_dir())
    }
}

#[async_trait]
impl AlephTool for ReadConfigGuideTool {
    const NAME: &'static str = "read_config_guide";
    const DESCRIPTION: &'static str = "Get Aleph configuration manual. Call when user needs to modify config, install plugins/skills, configure API keys, manage agents, or other self-management operations. Returns structure, steps, and caveats for the domain.";

    type Args = ReadConfigGuideArgs;
    type Output = ReadConfigGuideOutput;

    async fn call(&self, args: Self::Args) -> anyhow::Result<Self::Output> {
        let topic_name = format!("{:?}", args.topic).to_lowercase();
        notify_tool_start(Self::NAME, &topic_name);

        let file_path = self.guides_dir.join(args.topic.filename());

        let content = match tokio::fs::read_to_string(&file_path).await {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("Guide '{}' not found at {}: {}", topic_name, file_path.display(), e);
                notify_tool_result(Self::NAME, &msg, false);
                return Ok(ReadConfigGuideOutput {
                    success: false,
                    topic: topic_name,
                    content: msg,
                });
            }
        };

        notify_tool_result(Self::NAME, &format!("loaded {} guide", topic_name), true);
        Ok(ReadConfigGuideOutput {
            success: true,
            topic: topic_name,
            content,
        })
    }
}
```

- [ ] **Step 2: Add module and exports to mod.rs**

In `src/builtin_tools/mod.rs`, add:
- Module declaration: `pub mod config_guide;`
- Export: `pub use config_guide::{GuideTopic, ReadConfigGuideArgs, ReadConfigGuideOutput, ReadConfigGuideTool};`

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles cleanly

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/config_guide.rs src/builtin_tools/mod.rs
git commit -m "tools: add read_config_guide builtin tool for progressive config knowledge disclosure"
```

---

## Chunk 2: Wire Up New Tools & Remove Old Tools

### Task 3: Register new tools in the executor

**Files:**
- Modify: `src/executor/builtin_registry/config.rs` — add `shared_token_manager` field
- Modify: `src/executor/builtin_registry/builder.rs` — create tool instances
- Modify: `src/executor/builtin_registry/registry.rs` — add execution routes
- Modify: `src/executor/builtin_registry/definitions.rs` — add tool definitions
- Modify: `src/bin/aleph/commands/start/builder/agent_init.rs` — pass SharedTokenManager

- [ ] **Step 1: Add `shared_token_manager` to BuiltinToolConfig**

In `src/executor/builtin_registry/config.rs`, add field to `BuiltinToolConfig`:

```rust
pub shared_token_manager: Option<Arc<SharedTokenManager>>,
```

Add the import for `SharedTokenManager` at the top of the file.

- [ ] **Step 2: Create tool instances in builder.rs**

In `src/executor/builtin_registry/builder.rs`, inside `with_config()`:

```rust
// Config guide tool (no dependencies, always available)
let config_guide_tool = ReadConfigGuideTool::default();

// Vault store tool (requires SharedTokenManager)
let vault_store_tool = config.shared_token_manager.as_ref().map(|mgr| {
    info!("Creating VaultStoreTool");
    VaultStoreTool::new(Arc::clone(mgr))
});
```

Add corresponding fields to the `BuiltinToolRegistry` struct:
```rust
pub(crate) config_guide_tool: ReadConfigGuideTool,
pub(crate) vault_store_tool: Option<VaultStoreTool>,
```

- [ ] **Step 3: Add execution routes in registry.rs**

In `src/executor/builtin_registry/registry.rs`, inside `execute_tool()`, add match arms:

```rust
"read_config_guide" => {
    let tool = self.config_guide_tool.clone();
    Box::pin(async move { tool.call_json(arguments).await })
}
"vault_store" => {
    let tool = self.vault_store_tool.as_ref().ok_or_else(|| {
        AlephError::tool("vault_store not available: no SharedTokenManager configured")
    })?.clone();
    Box::pin(async move { tool.call_json(arguments).await })
}
```

- [ ] **Step 4: Add tool definitions AND create_tool_boxed entries in definitions.rs**

In `src/executor/builtin_registry/definitions.rs`:

Add to `BUILTIN_TOOL_DEFINITIONS`:

```rust
BuiltinToolDefinition {
    name: "read_config_guide",
    description: "Get Aleph configuration manual for self-management operations",
    requires_config: false,
},
BuiltinToolDefinition {
    name: "vault_store",
    description: "Manage encrypted secret vault (store/delete/list API keys)",
    requires_config: true,
},
```

Add imports at top:
```rust
use crate::builtin_tools::{ReadConfigGuideTool, VaultStoreTool};
```

Add match arms to `create_tool_boxed()`:
```rust
"read_config_guide" => Some(Box::new(ReadConfigGuideTool::default())),
"vault_store" => {
    config.and_then(|c| c.shared_token_manager.as_ref()).map(|mgr| {
        Box::new(VaultStoreTool::new(Arc::clone(mgr))) as Box<dyn AlephToolDyn>
    })
}
```

- [ ] **Step 5: Pass SharedTokenManager in agent_init.rs**

In `src/bin/aleph/commands/start/builder/agent_init.rs`, find the `BuiltinToolConfig` construction (around line 268) and add:

```rust
shared_token_manager: Some(shared_token_mgr.clone()),
```

Note: `shared_token_mgr` is available from `auth_bundle.auth_ctx.shared_token_mgr` — trace the variable to find its exact name in scope.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore`
Expected: compiles cleanly (may need to also check the binary crate)

Run: `cargo check --bin aleph`
Expected: compiles cleanly

- [ ] **Step 7: Commit**

```bash
git add src/executor/builtin_registry/ src/bin/aleph/commands/start/builder/agent_init.rs
git commit -m "tools: wire up vault_store and read_config_guide in executor registry"
```

---

### Task 4: Unregister removed tools

**Files:**
- Modify: `src/executor/builtin_registry/definitions.rs` — remove definitions
- Modify: `src/executor/builtin_registry/builder.rs` — remove tool creation
- Modify: `src/executor/builtin_registry/registry.rs` — remove execution routes
- Modify: `src/tools/builtin.rs` — remove `with_config_read()`, `with_config_update()` methods
- Modify: `src/builtin_tools/mod.rs` — remove exports (keep modules for now)

Tools to unregister: `config_read`, `config_update`, `soul_update`, `profile_update`, `read_skill`

- [ ] **Step 1: Remove tool definitions AND create_tool_boxed entries from definitions.rs**

In `BUILTIN_TOOL_DEFINITIONS`: remove entries for `config_read`, `config_update`, `read_skill`. (`soul_update` and `profile_update` are not in this array — they're registered elsewhere.)

Keep `list_skills` — it's independent from `read_skill`.

In `create_tool_boxed()`: remove match arms for `"config_read"` (lines 294-301), `"config_update"` (lines 302-309), and `"read_skill"` (line 290).

Remove imports: `ConfigReadTool`, `ConfigUpdateTool`, `ReadSkillTool` from the `use` block at line 21-24.

Update test `test_all_tools_defined()` — remove `assert!(names.contains(&"read_skill".to_string()));` and any assertions for removed tools.

- [ ] **Step 2: Remove tool creation from builder.rs**

Remove the lines that create `ConfigReadTool`, `ConfigUpdateTool`, `SoulUpdateTool`, `ProfileUpdateTool`, `ReadSkillTool` instances. Remove corresponding struct fields from `BuiltinToolRegistry`.

Also remove `reg()` calls for `soul_update`, `profile_update`, and `read_skill` in `register_core_tools()` (in builder.rs).

- [ ] **Step 3: Remove execution routes from registry.rs**

Remove match arms for `"config_read"`, `"config_update"`, `"soul_update"`, `"profile_update"`, `"read_skill"` in `execute_tool()`.

- [ ] **Step 4: Remove builder methods from tools/builtin.rs**

Remove `with_config_read()` and `with_config_update()` methods from `impl AlephToolServer`.

- [ ] **Step 5: Remove exports from builtin_tools/mod.rs**

Remove `pub use` lines for:
- `config_read::{ConfigReadArgs, ConfigReadOutput, ConfigReadTool}`
- `config_update::{ConfigUpdateArgs, ConfigUpdateOutput, ConfigUpdateTool}`
- `profile_update::{ProfileField, ProfileOperation, ProfileUpdateArgs, ProfileUpdateOutput, ProfileUpdateTool}`
- `soul_update::{SoulField, SoulOperation, SoulUpdateArgs, SoulUpdateOutput, SoulUpdateTool}`
- `skill_reader::{ReadSkillArgs, ReadSkillOutput, ReadSkillTool, ...}` (keep ListSkills exports)

Keep the `pub mod` declarations so the source files remain compilable — they'll be deleted later after validation.

- [ ] **Step 6: Fix compilation errors**

Run: `cargo check -p alephcore`

Fix any references to removed types across the codebase. Common places:
- Panel API handlers that reference these tool types
- Test files
- Gateway handlers

For each error: remove the reference or replace with the new tool.

- [ ] **Step 7: Verify full build**

Run: `cargo check --bin aleph`
Expected: compiles cleanly

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "tools: unregister config_read, config_update, soul_update, profile_update, read_skill"
```

---

## Chunk 3: Update System Prompt

### Task 5: Update OperationalGuidelinesLayer

**Files:**
- Modify: `src/thinker/layers/operational_guidelines.rs`

- [ ] **Step 1: Update the prompt text**

The current layer says "What You Must NEVER Do Autonomously - Modify configuration files". This contradicts the new design. Replace the entire `inject()` body after the paradigm check with:

```rust
output.push_str("## System Operational Awareness\n\n");
output.push_str(
    "You are aware of your own runtime environment and can monitor it proactively.\n\n",
);

output.push_str("### Diagnostic Capabilities (read-only, always allowed)\n");
output.push_str("- Check disk space: `df -h`\n");
output.push_str("- Check memory usage: `vm_stat` / `free -h`\n");
output.push_str("- Check running Aleph processes: `ps aux | grep aleph`\n");
output.push_str(
    "- Check configuration validity: read config files and validate structure\n",
);
output.push_str("- Check Desktop Bridge status: query UDS socket availability\n");
output.push_str("- Check LanceDB health: verify database file accessibility\n\n");

output.push_str("### Self-Management\n");
output.push_str("You can manage all Aleph configuration. When needed, call read_config_guide(topic) ");
output.push_str("to get the configuration manual for the relevant domain, then use file read/write ");
output.push_str("tools to make changes.\n");
output.push_str("- Always backup config files before modification (cp file file.bak)\n");
output.push_str("- Show planned changes to the user and confirm before writing\n");
output.push_str("- After writing, read the file back to verify the format is valid\n");
output.push_str("- API keys must be stored via vault_store tool, never written to config files\n\n");

output.push_str("### When You Detect Issues\n");
output.push_str(
    "If you notice configuration conflicts, database issues, disconnected bridges,\n",
);
output.push_str("abnormal resource usage, or runtime capability degradation:\n\n");
output.push_str("**Action**: Report to the user with:\n");
output.push_str("1. What you observed (specific evidence)\n");
output.push_str("2. Potential impact\n");
output.push_str("3. Suggested remediation steps\n\n");

output.push_str("### What You Must NEVER Do Autonomously\n");
output.push_str("- Restart Aleph services\n");
output.push_str("- Delete or compact databases\n");
output.push_str("- Kill processes\n");
output.push_str("- Change system settings\n\n");
```

Key changes:
1. Added "Self-Management" section with backup/confirm/verify/vault rules
2. Removed "Modify configuration files" from the NEVER list
3. Removed "Do NOT execute remediation without explicit user approval" (now LLM can self-manage with user confirmation)

- [ ] **Step 2: Update tests if needed**

The existing tests only check that injection is empty without context, and that paths are correct. These should still pass unchanged.

Run: `cargo test -p alephcore --lib operational_guidelines`

- [ ] **Step 3: Commit**

```bash
git add src/thinker/layers/operational_guidelines.rs
git commit -m "prompt: update operational guidelines to enable LLM self-management"
```

---

## Chunk 4: Guide Files

### Task 6: Create guide files

**Files:**
- Create: `docs/guides/overview.md`
- Create: `docs/guides/providers.md`
- Create: `docs/guides/mcp.md`
- Create: `docs/guides/skills.md`
- Create: `docs/guides/agents.md`
- Create: `docs/guides/general.md`
- Create: `docs/guides/generation.md`
- Create: `docs/guides/channels.md`
- Create: `docs/guides/cron.md`

Each guide follows the template from the spec. Content must be derived from current config structures in the codebase.

- [ ] **Step 1: Read current config structures**

Read these files to extract accurate field information:
- `src/config/structs.rs` — all config sections
- `src/config/types/` — type definitions for each section
- `src/mcp/manager/config.rs` — MCP config format
- `src/thinker/soul.rs` — SoulManifest structure
- `src/config/agent_resolver.rs` — agent workspace layout

- [ ] **Step 2: Create `docs/guides/overview.md`**

Content: file map of all config files, operation model (backup→edit→verify→auto-reload), backup rules, hot-reload behavior, vault_store usage pattern.

- [ ] **Step 3: Create `docs/guides/providers.md`**

Content: `[providers.*]` TOML structure, vault key naming `provider:{name}`, model binding fields, how to add/modify/delete a provider.

- [ ] **Step 4: Create `docs/guides/mcp.md`**

Content: `mcp_config.json` structure, server entry fields (command, args, env, cwd, timeout_seconds, enabled, triggers), env var expansion `${VAR}`, how to add/remove MCP servers, note that `mcp_manage` tool needed after editing to restart servers.

- [ ] **Step 5: Create `docs/guides/skills.md`**

Content: `~/.aleph/skills/` directory layout, SKILL.md format, per-skill directory structure, manual install steps, ClawHub tool for registry installs, project-level skills (`.aleph/skills/`).

- [ ] **Step 6: Create `docs/guides/agents.md`**

Content: `~/.aleph/agents/{id}/` layout, SOUL.md YAML schema (identity, voice, directives, anti_patterns, relationship, expertise), MEMORY.md format and 20K char limit, agent entry in config.toml `[agents]`.

- [ ] **Step 7: Create `docs/guides/general.md`**

Content: `[general]` (hotkey, log_retention, language), `[memory]` (enabled, embedding_model, retention_days, similarity_threshold), `[policies]` subsections, `[dispatcher]`.

- [ ] **Step 8: Create `docs/guides/generation.md`**

Content: `[generation]` provider config, vault key naming `gen:{name}`, supported provider types, how to add image/speech/video generation providers.

- [ ] **Step 9: Create `docs/guides/channels.md`**

Content: `[channels]` opaque JSON config per channel type (telegram, discord, etc.), how to enable/disable channels, vault key naming `channel:{type}:{id}`.

- [ ] **Step 10: Create `docs/guides/cron.md`**

Content: `[cron]` config structure, note that runtime operations (create/delete/enable/disable jobs) use `cron_manage` tool, config only controls global cron behavior.

- [ ] **Step 11: Verify all guides are ≤ 1500 tokens each**

Rough check: each file should be under ~6000 characters.

Run: `wc -c docs/guides/*.md`

- [ ] **Step 12: Commit**

```bash
git add docs/guides/
git commit -m "docs: add configuration guide files for LLM self-management"
```

---

### Task 7: Deploy guides on server start

**Files:**
- Modify: `src/bin/aleph/commands/start/builder/` — find the startup initialization and add guide file deployment

- [ ] **Step 1: Find the startup init point**

Look for where `~/.aleph/` subdirectories are created during first run (e.g., `~/.aleph/skills/`, `~/.aleph/agents/`). The guides deployment should go in the same place.

- [ ] **Step 2: Add guide deployment logic**

At server startup, copy guides from the embedded location to `~/.aleph/guides/`. Embed guide files at compile time using `include_str!()` with paths relative to `Cargo.toml` (since this code lives in `alephcore` crate):

```rust
// In src/config/guides.rs (new file)
use std::path::Path;

const GUIDES: &[(&str, &str)] = &[
    ("overview.md", include_str!("../../docs/guides/overview.md")),
    ("providers.md", include_str!("../../docs/guides/providers.md")),
    ("mcp.md", include_str!("../../docs/guides/mcp.md")),
    ("skills.md", include_str!("../../docs/guides/skills.md")),
    ("agents.md", include_str!("../../docs/guides/agents.md")),
    ("general.md", include_str!("../../docs/guides/general.md")),
    ("generation.md", include_str!("../../docs/guides/generation.md")),
    ("channels.md", include_str!("../../docs/guides/channels.md")),
    ("cron.md", include_str!("../../docs/guides/cron.md")),
];

pub fn deploy_guides(aleph_dir: &Path) -> std::io::Result<()> {
    let guides_dir = aleph_dir.join("guides");
    std::fs::create_dir_all(&guides_dir)?;
    for (name, content) in GUIDES {
        std::fs::write(guides_dir.join(name), content)?;
    }
    Ok(())
}
```

Note: `include_str!()` paths are relative to the file's location. Since this file is at `src/config/guides.rs`, `../../docs/guides/` reaches the repo root's `docs/guides/`. Verify the path is correct by checking `Cargo.toml` position relative to `docs/`.

Call `deploy_guides()` during server startup in the binary crate, before tool registry creation. Add `pub mod guides;` to `src/config/mod.rs`.

- [ ] **Step 3: Verify compilation and startup**

Run: `cargo check --bin aleph`

- [ ] **Step 4: Commit**

```bash
git add src/bin/aleph/
git commit -m "startup: deploy config guide files to ~/.aleph/guides/ on server start"
```

---

## Chunk 5: Integration Testing & Cleanup

### Task 8: Integration test

**Files:**
- No new test files — use manual testing via running server

- [ ] **Step 1: Build and start the server**

```bash
pkill -f "target/debug/aleph" 2>/dev/null; sleep 2
cargo run --bin aleph -- start
```

- [ ] **Step 2: Verify guide deployment**

```bash
ls ~/.aleph/guides/
# Expected: 9 .md files
cat ~/.aleph/guides/overview.md
# Expected: valid guide content
```

- [ ] **Step 3: Test via LLM conversation**

Send a message like "帮我查看当前的 provider 配置" and verify:
1. LLM calls `read_config_guide("providers")`
2. Guide content is returned
3. LLM reads config.toml and reports provider info

- [ ] **Step 4: Test vault_store**

Send "帮我保存一个测试 API key" and verify:
1. LLM calls `vault_store(action="store", key="provider:test", secret="test-key")`
2. Confirmation is requested
3. Secret is stored successfully
4. `vault_store(action="list")` shows the new key
5. `vault_store(action="delete", key="provider:test")` removes it

- [ ] **Step 5: Verify removed tools are gone**

Send "用 config_update 修改配置" — LLM should NOT have access to `config_update` tool. It should instead use `read_config_guide` + file editing.

### Task 9: Clean up removed tool source files (optional, after validation)

**Files:**
- Delete: `src/builtin_tools/config_read.rs`
- Delete: `src/builtin_tools/config_update.rs`
- Delete: `src/builtin_tools/soul_update.rs`
- Delete: `src/builtin_tools/profile_update.rs`
- Modify: `src/builtin_tools/mod.rs` — remove `pub mod` declarations

- [ ] **Step 1: Remove source files**

Only do this after successful integration testing. Remove the 4 files and their `pub mod` declarations.

- [ ] **Step 2: Fix any remaining compilation errors**

Run: `cargo check -p alephcore && cargo check --bin aleph`

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "cleanup: remove unregistered self-management tool source files"
```
