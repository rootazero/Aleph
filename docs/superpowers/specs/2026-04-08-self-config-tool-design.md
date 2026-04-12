# self_config Tool Design — Aleph Self-Management Execution Layer

## Problem

LLM cannot modify its own identity files or configuration:
- `file_write` resolves relative paths to `output/documents/`, not `~/.aleph/agents/{agent_id}/`
- `config.toml` is blocked by file_ops `denied_paths`
- `SelfManageTool` only returns guidance documents, cannot execute changes
- LLM doesn't know its own `agent_id` to construct correct paths
- SKILL.md instructs LLM to use `bash` for config editing, which is unsafe and error-prone

## Solution

Add a `self_config` tool that provides structured identity file read/write and config modification. Integrates into the existing self-management ecosystem alongside `self_manage`, `read_config_guide`, and `vault_store`.

## Architecture

```
Workflow (before):
  self_manage → manual → read_config_guide → guide → bash → execute
                                                       ↑ unsafe, error-prone

Workflow (after):
  self_manage → manual → read_config_guide → guide → self_config → structured execution
                                                       ↑ safe, validated
  bash demoted to: plugin/skill install and system commands only
```

## Tool Definition

### Name & Description

```
NAME: "self_config"
DESCRIPTION: "Read and write Aleph identity files (MEMORY.md, SOUL.md, etc.) and
modify config.toml with validation. Use for all self-management file operations.
Identity files are stored in the agent's directory and injected into your context
on each turn."
```

### Args (tagged enum)

```rust
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SelfConfigArgs {
    ListFiles,
    ReadFile {
        /// Identity file name: MEMORY.md, SOUL.md, AGENTS.md, IDENTITY.md, TOOLS.md, HEARTBEAT.md
        file_name: String,
    },
    WriteFile {
        /// Identity file name (must be in allowlist)
        file_name: String,
        /// Full content to write to the file
        content: String,
    },
    ReadConfig {
        /// Dot-path config section, e.g. "memory", "providers.openai", "general"
        config_path: String,
    },
    UpdateConfig {
        /// Dot-path config section to update
        config_path: String,
        /// JSON value to deep-merge into the section
        config_value: serde_json::Value,
        /// Preview changes without persisting (default: false)
        #[serde(default)]
        dry_run: bool,
    },
}
```

Key: WriteFile's `content` is `String` (not `Option<String>`), ensuring JSON Schema marks it as required.

### Output

```rust
#[derive(Serialize)]
pub struct SelfConfigOutput {
    pub success: bool,
    pub message: String,
    /// File content (ReadFile), file list (ListFiles), or config JSON (ReadConfig/UpdateConfig)
    pub data: Option<serde_json::Value>,
}
```

## Operations

### list_files
- List all identity files in `~/.aleph/agents/{agent_id}/`
- Return name, exists (bool), size for each file in `IDENTITY_FILE_NAMES`

### read_file
- Validate `file_name` against `IDENTITY_FILE_NAMES` allowlist
- Path traversal check (reject `..`, `/`, `\`, null bytes)
- Read from `agent_dir.join(file_name)`
- Return content as string

### write_file
- Same validation as read_file
- Create `agent_dir` if needed (`fs::create_dir_all`)
- Write content to `agent_dir.join(file_name)`
- Return success message: "Written {bytes} bytes to {file_name}. Changes will take effect on the next turn."

### read_config
- Require `config_path` (dot-separated, e.g. "memory", "providers.openai")
- Read from `Arc<RwLock<Config>>` — serialize to JSON, navigate to dot-path
- Return the config subtree as JSON

### update_config
- Require `config_path` and `config_value`
- Delegate to `ConfigPatcher::apply(PatchRequest { path, patch, dry_run, .. })`
- ConfigPatcher provides: JSON Schema validation, structural validation, backup, atomic save, conflict detection
- Return diff and validation status

## Security

- Identity file allowlist: reuse `IDENTITY_FILE_NAMES` from `src/thinker/identity_files.rs`
- Path traversal prevention on file_name
- Config writes go through ConfigPatcher (already validates against JSON Schema + Config::validate())
- agent_id injected at construction from `BuiltinToolConfig.current_agent_id` — LLM cannot specify arbitrary agent IDs
- No confirmation required: identity files are LLM's own working memory; config updates have dry_run + validation

## Struct & Dependencies

```rust
#[derive(Clone)]
pub struct SelfConfigTool {
    agent_dir: PathBuf,                          // ~/.aleph/agents/{agent_id}/
    agent_id: String,
    config: Option<Arc<RwLock<Config>>>,          // from BuiltinToolConfig
    config_patcher: Option<Arc<ConfigPatcher>>,   // from BuiltinToolConfig
}
```

Constructor: `SelfConfigTool::new(agent_id)` computes `agent_dir` internally. Builder methods: `.with_config()`, `.with_patcher()`.

## Registration

In `builder.rs`, construct using existing `BuiltinToolConfig` fields:
```rust
let agent_id = config.current_agent_id.clone().unwrap_or("main".into());
let mut tool = SelfConfigTool::new(agent_id);
if let Some(ref cfg) = config.config { tool = tool.with_config(Arc::clone(cfg)); }
if let Some(ref p) = config.config_patcher { tool = tool.with_patcher(Arc::clone(p)); }
```

Register as core tool (always available, same tier as self_manage).

## SKILL.md Update

Update `~/.aleph/skills/self/SKILL.md` tools table:

```markdown
| Tool | Use for |
|------|---------|
| `self_config` | Read/write identity files (MEMORY.md, SOUL.md, etc.), read/update config.toml |
| `vault_store` | Store/delete/list API keys |
| `read_config_guide` | Load detailed guide for a domain |
| `bash` | Plugin/skill install, system commands only |

**Never use**: `file_ops` / `file_write` / `file_edit` for self-management (denied_paths + wrong path resolution).
```

Update Operation Protocol:
```markdown
1. Store secrets: `vault_store(action="store", key="<convention>", secret="<key>")`
2. Read config: `self_config(action="read_config", config_path="providers")`
3. Update config: `self_config(action="update_config", config_path="providers.new_provider", config_value={...})`
4. Write identity file: `self_config(action="write_file", file_name="MEMORY.md", content="...")`
5. Verify: `self_config(action="read_config", ...)` or `self_config(action="read_file", ...)`
6. Config auto-reloads (500ms) — no restart needed except generation providers.
```

## Files to Create/Modify

| File | Action |
|------|--------|
| `src/builtin_tools/self_config.rs` | **Create** — tool implementation |
| `src/builtin_tools/mod.rs` | Modify — add `pub mod self_config` + re-export |
| `src/executor/builtin_registry/builder.rs` | Modify — construct and register |
| `src/executor/builtin_registry/registry.rs` | Modify — add field + dispatch |
| `~/.aleph/skills/self/SKILL.md` | Modify — update tools table and operation protocol |
