# self_config Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `self_config` tool that gives the LLM structured read/write access to its identity files (`~/.aleph/agents/{agent_id}/`) and config.toml, replacing unsafe bash-based config editing.

**Architecture:** Single new file `src/builtin_tools/self_config.rs` implementing `AlephTool` with a tagged enum Args (`#[serde(tag = "action")]`). Uses existing `ConfigPatcher` for config modifications and `IDENTITY_FILE_NAMES` for file allowlisting. Registered as a core tool alongside `self_manage`.

**Tech Stack:** Rust, schemars (JsonSchema), async-trait, serde tagged enum, ConfigPatcher

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/builtin_tools/self_config.rs` | Create | SelfConfigTool — AlephTool impl with 5 operations |
| `src/builtin_tools/mod.rs` | Modify | Add `pub mod self_config` + re-export |
| `src/executor/builtin_registry/builder.rs` | Modify | Construct SelfConfigTool with agent_id, config, patcher |
| `src/executor/builtin_registry/registry.rs` | Modify | Add field + dispatch arm |
| `~/.aleph/skills/self/SKILL.md` | Modify | Update tools table and operation protocol |

---

### Task 1: Create SelfConfigTool

Create the tool with all 5 operations: list_files, read_file, write_file, read_config, update_config.

**Files:**
- Create: `src/builtin_tools/self_config.rs`

**Context to read first:**
- `src/thinker/identity_files.rs` — `IDENTITY_FILE_NAMES` constant (line 11), used for allowlist
- `src/tools/traits.rs` — `AlephTool` trait definition
- `src/config/patcher.rs` — `ConfigPatcher` struct, `PatchRequest` struct, `apply()` method
- `src/executor/builtin_registry/config.rs` — `BuiltinToolConfig` fields: `config`, `config_patcher`, `current_agent_id`
- `src/builtin_tools/self_manage.rs` — existing pattern for self-management tools

- [ ] **Step 1: Create `src/builtin_tools/self_config.rs`**

Implement the full tool. Key design points:

**Args (tagged enum):**
```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SelfConfigArgs {
    ListFiles,
    ReadFile { file_name: String },
    WriteFile { file_name: String, content: String },
    ReadConfig { config_path: String },
    UpdateConfig { config_path: String, config_value: serde_json::Value, #[serde(default)] dry_run: bool },
}
```

**Struct:**
```rust
#[derive(Clone)]
pub struct SelfConfigTool {
    agent_dir: PathBuf,
    agent_id: String,
    config: Option<Arc<RwLock<Config>>>,
    config_patcher: Option<Arc<ConfigPatcher>>,
}
```

**Constructor:** `SelfConfigTool::new(agent_id: impl Into<String>)` computes `agent_dir` as `dirs::home_dir().join(".aleph/agents").join(&agent_id)`. Builder methods: `.with_config(Arc<RwLock<Config>>)`, `.with_patcher(Arc<ConfigPatcher>)`.

**Security validation helper:**
```rust
fn validate_file_name(name: &str) -> Result<(), ToolError> {
    // Import from thinker
    use crate::thinker::identity_files::IDENTITY_FILE_NAMES;
    if !IDENTITY_FILE_NAMES.contains(&name) {
        return Err(ToolError::InvalidArgs(format!(
            "Invalid file name '{}'. Allowed: {:?}", name, IDENTITY_FILE_NAMES
        )));
    }
    // Path traversal check
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(ToolError::InvalidArgs("Invalid characters in file name".into()));
    }
    Ok(())
}
```

**Operation implementations:**

1. `ListFiles`: iterate `IDENTITY_FILE_NAMES`, check `agent_dir.join(name)` exists/size, return JSON array of `{name, exists, size}`.

2. `ReadFile`: validate file_name, read `agent_dir.join(file_name)`, return content as string in `data` field.

3. `WriteFile`: validate file_name, `fs::create_dir_all(&self.agent_dir)`, write to `agent_dir.join(file_name)`, return bytes written. Message: "Written N bytes to {file_name}. Changes will take effect on the next turn."

4. `ReadConfig`: require `self.config`, serialize config to JSON value via `serde_json::to_value(&*config.read().await)`, navigate dot-path (split by `.`, index into nested objects), return subtree.

5. `UpdateConfig`: require `self.config_patcher`, create `PatchRequest { path: config_path, patch: config_value, dry_run, .. }`, call `self.config_patcher.apply(request).await`, return result with diffs. If patcher is None, return error "Config updates not available".

**AlephTool impl:**
- `NAME = "self_config"`
- `DESCRIPTION`: Explain all 5 operations, mention identity file names, mention config dot-path syntax
- `call()`: match on `SelfConfigArgs` variant and dispatch

**Tests** (`#[cfg(test)] mod tests`):
- `test_list_files` — create tempdir agent_dir, write SOUL.md, verify list shows it
- `test_read_write_file` — write MEMORY.md, read back, verify content
- `test_write_file_rejects_invalid_name` — try "../../etc/passwd", expect error
- `test_write_file_creates_dir` — write to non-existent agent_dir, verify dir created
- `test_read_config` — create SelfConfigTool with a real Config, read "general" section

- [ ] **Step 2: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo test -p alephcore --lib -- self_config`

- [ ] **Step 3: Commit**

```
feat(tools): add self_config tool for identity files and config management

Tagged enum args ensure write_file's content is a required String.
Uses ConfigPatcher for validated, atomic config updates. Identity
files allowlisted via IDENTITY_FILE_NAMES constant.
```

---

### Task 2: Register SelfConfigTool in Builtin Registry

Wire the tool into the registry so LLM can use it.

**Files:**
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/builder.rs`
- Modify: `src/executor/builtin_registry/registry.rs`

**Context to read first:**
- `src/executor/builtin_registry/builder.rs` — find `self_manage_tool` construction (line 87) and `current_agent_id` extraction (line 406). Follow the same pattern.
- `src/executor/builtin_registry/registry.rs` — find struct fields and dispatch match
- `src/builtin_tools/mod.rs` — existing re-exports

- [ ] **Step 1: Update `src/builtin_tools/mod.rs`**

Add:
```rust
pub mod self_config;
pub use self_config::SelfConfigTool;
```

- [ ] **Step 2: Update `src/executor/builtin_registry/builder.rs`**

Near line 87 (after `self_manage_tool` construction), add:

```rust
let self_config_tool = {
    let agent_id = config.current_agent_id.clone().unwrap_or_else(|| "main".to_string());
    let mut tool = SelfConfigTool::new(agent_id);
    if let Some(ref cfg) = config.config {
        tool = tool.with_config(std::sync::Arc::clone(cfg));
    }
    if let Some(ref patcher) = config.config_patcher {
        tool = tool.with_patcher(std::sync::Arc::clone(patcher));
    }
    tool
};
```

Add the import: `use crate::builtin_tools::SelfConfigTool;`

Add `self_config_tool` to the struct fields list (near line 737 where `self_manage_tool` is assigned).

Register tool metadata in `register_core_tools()` — follow the same `reg()` pattern used for other tools. Use `AlephTool::definition()` to get the schema.

- [ ] **Step 3: Update `src/executor/builtin_registry/registry.rs`**

Add field to the struct:
```rust
pub(crate) self_config_tool: crate::builtin_tools::self_config::SelfConfigTool,
```

Add dispatch arm in the `execute_tool` match:
```rust
"self_config" => Box::pin(async move { self.self_config_tool.call_json(arguments).await }),
```

- [ ] **Step 4: Compile and test**

Run: `cargo check -p alephcore`
Run: `cargo check --bin aleph-server`

- [ ] **Step 5: Commit**

```
feat(tools): register self_config in builtin tool registry

Constructed with current_agent_id, config handle, and config_patcher
from BuiltinToolConfig. Registered as core tool (always available).
```

---

### Task 3: Update SKILL.md

Update the self-management skill to reference `self_config` instead of `bash` for config editing.

**Files:**
- Modify: `~/.aleph/skills/self/SKILL.md`

- [ ] **Step 1: Update tools table**

Replace the existing tools table (lines 19-27 of SKILL.md) with:

```markdown
## Tools

| Tool | Use for |
|------|---------|
| `self_config` | Read/write identity files (MEMORY.md, SOUL.md, etc.), read/update config.toml |
| `vault_store` | Store/delete/list API keys |
| `read_config_guide` | Load detailed guide for a domain (see topics below) |
| `bash` | Plugin/skill install, system commands only |

**Never use**: `file_ops` / `file_write` / `file_edit` for self-management (denied_paths + wrong path resolution).
```

- [ ] **Step 2: Update Operation Protocol**

Replace the existing protocol (lines 30-36) with:

```markdown
## Operation Protocol

1. Store secrets: `vault_store(action="store", key="<convention>", secret="<key>")`
2. Read config: `self_config(action="read_config", config_path="providers")`
3. Update config: `self_config(action="update_config", config_path="providers.new_one", config_value={...})`
4. Write identity file: `self_config(action="write_file", file_name="MEMORY.md", content="...")`
5. Read identity file: `self_config(action="read_file", file_name="MEMORY.md")`
6. List identity files: `self_config(action="list_files")`
7. Verify: `self_config(action="read_config", ...)` or `self_config(action="read_file", ...)`
8. Config auto-reloads (fswatch 500ms) — no restart needed except generation providers.
```

- [ ] **Step 3: Update Critical Rules**

Replace rule 1 (line 10) from:
```
1. **Use `bash` for config files** — `file_ops` cannot access `~/.aleph/config.toml` (denied_paths).
```
To:
```
1. **Use `self_config` for config and identity files** — `file_ops`/`file_write` cannot access `~/.aleph/` (denied_paths). Use `self_config(action="update_config", ...)` for config.toml and `self_config(action="write_file", ...)` for identity files.
```

- [ ] **Step 4: Commit**

```
docs(self): update SKILL.md to use self_config instead of bash for config editing
```

---

### Task 4: Build, Restart, Test

Full build and production test via Panel.

- [ ] **Step 1: Full release build**

```bash
just build
```

- [ ] **Step 2: Restart server**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2
nohup target/release/aleph-server start > /tmp/aleph-server.log 2>&1 &
```

- [ ] **Step 3: Test identity file write via Panel**

Open Panel, new session, send: "请把我叫张三写入MEMORY.md"

Verify in logs:
- `self_config` tool called with `action: "write_file"`
- `file_name: "MEMORY.md"`, `content` present
- Write succeeds to `~/.aleph/agents/main/MEMORY.md`

- [ ] **Step 4: Test identity file read**

Send: "读取我的MEMORY.md"

Verify `self_config` called with `action: "read_file"`, returns content.

- [ ] **Step 5: Test config read**

Send: "显示当前的 memory 配置"

Verify `self_config` called with `action: "read_config"`, returns JSON.

- [ ] **Step 6: Test list files**

Send: "列出我的身份文件"

Verify `self_config` called with `action: "list_files"`, returns file list.

- [ ] **Step 7: Final commit if fixes needed**
