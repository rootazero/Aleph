# Agent Files Directory Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix agent identity files (SOUL.md, etc.) being written to `~/.aleph/workspaces/` instead of `~/.aleph/agents/`, and fix Panel agent files API to read from the correct directory.

**Architecture:** Three-layer fix: (1) rename and re-target the file operations module, (2) fix all `initialize_workspace()` call sites, (3) fix orphan reconciliation to scan the correct dir.

**Tech Stack:** Rust, TOML config, filesystem operations

**Spec:** `docs/superpowers/specs/2026-03-19-agent-files-directory-fix-design.md`

---

### Task 1: Rename `workspace_files.rs` → `agent_files.rs` and re-target to `agents_root`

**Files:**
- Rename: `src/config/agent_manager/workspace_files.rs` → `src/config/agent_manager/agent_files.rs`
- Modify: `src/config/agent_manager/mod.rs:10`

- [ ] **Step 1: Rename the file**

```bash
cd /Users/zouguojun/Workspace/Aleph
mv src/config/agent_manager/workspace_files.rs src/config/agent_manager/agent_files.rs
```

- [ ] **Step 2: Update mod.rs**

In `src/config/agent_manager/mod.rs`, change line 10:

```rust
// OLD:
mod workspace_files;

// NEW:
mod agent_files;
```

- [ ] **Step 3: Re-target `agent_files.rs` from `workspace_root` to `agents_root`**

Replace the entire content of `src/config/agent_manager/agent_files.rs` with:

```rust
//! Agent identity file operations — list, read, write, delete files in agent identity directories

use std::fs;

use crate::error::{AlephError, Result};

use super::{AgentManager, WorkspaceFile, BOOTSTRAP_FILES};

impl AgentManager {
    /// List files in an agent's identity directory
    pub fn list_files(&self, agent_id: &str) -> Result<Vec<WorkspaceFile>> {
        let agent_dir = self.agents_root.join(agent_id);
        if !agent_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        let entries = fs::read_dir(&agent_dir).map_err(|e| {
            AlephError::IoError(format!("Failed to read agent dir: {}", e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AlephError::IoError(format!("Failed to read dir entry: {}", e))
            })?;
            let metadata = entry.metadata().map_err(|e| {
                AlephError::IoError(format!("Failed to read metadata: {}", e))
            })?;

            if !metadata.is_file() {
                continue;
            }

            let filename = entry.file_name().to_string_lossy().to_string();
            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            files.push(WorkspaceFile {
                is_bootstrap: BOOTSTRAP_FILES.contains(&filename.as_str()),
                filename,
                size_bytes: metadata.len(),
                modified_at,
            });
        }

        files.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(files)
    }

    /// Read a file from an agent's identity directory
    pub fn read_file(&self, agent_id: &str, filename: &str) -> Result<String> {
        self.validate_filename(filename)?;
        let path = self.agents_root.join(agent_id).join(filename);
        fs::read_to_string(&path).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to read file '{}': {}",
                path.display(),
                e
            ))
        })
    }

    /// Write a file to an agent's identity directory
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()> {
        self.validate_filename(filename)?;
        let agent_dir = self.agents_root.join(agent_id);
        fs::create_dir_all(&agent_dir).map_err(|e| {
            AlephError::IoError(format!("Failed to create agent dir: {}", e))
        })?;
        let path = agent_dir.join(filename);
        fs::write(&path, content).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to write file '{}': {}",
                path.display(),
                e
            ))
        })
    }

    /// Delete a file from an agent's identity directory
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()> {
        self.validate_filename(filename)?;
        let path = self.agents_root.join(agent_id).join(filename);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AlephError::IoError(format!(
                    "Failed to delete file '{}': {}",
                    path.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS (no external consumers reference `workspace_files` by module name)

- [ ] **Step 5: Commit**

```bash
git add src/config/agent_manager/agent_files.rs src/config/agent_manager/mod.rs
git add src/config/agent_manager/workspace_files.rs  # shows as deleted
git commit -m "agent_manager: rename workspace_files → agent_files, re-target to agents_root"
```

---

### Task 2: Fix `crud.rs` — `initialize_workspace` and `reconcile_orphan_workspaces`

**Files:**
- Modify: `src/config/agent_manager/crud.rs`

- [ ] **Step 1: Fix `create()` — write identity files to `agents_root`**

In `src/config/agent_manager/crud.rs`, replace lines 227-245 (the workspace block in `create()`):

```rust
        // OLD (lines 227-240):
        // Workspace directory for project files (SOUL.md, AGENTS.md, etc.)
        let ws_dir = self.workspace_root.join(&def.id);
        std::fs::create_dir_all(&ws_dir).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to create workspace for '{}': {}",
                def.id, e
            ))
        })?;
        initialize_workspace(&ws_dir, agent_name).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to initialize workspace files for '{}': {}",
                def.id, e
            ))
        })?;

        info!(
            "Created agent '{}' with workspace at {}",
            def.id,
            ws_dir.display()
        );

        // NEW:
        // Identity files (SOUL.md, AGENTS.md, etc.) go in agent state directory
        initialize_workspace(&agent_state_dir, agent_name).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to initialize identity files for '{}': {}",
                def.id, e
            ))
        })?;

        // Workspace directory for tool output (separate from identity)
        let ws_dir = self.workspace_root.join(&def.id);
        std::fs::create_dir_all(&ws_dir).map_err(|e| {
            AlephError::IoError(format!(
                "Failed to create workspace for '{}': {}",
                def.id, e
            ))
        })?;

        info!(
            "Created agent '{}' (identity: {}, workspace: {})",
            def.id,
            agent_state_dir.display(),
            ws_dir.display()
        );
```

- [ ] **Step 2: Fix `reconcile_orphan_workspaces` — scan `agents_root`**

In `crud.rs`, replace lines 73-76:

```rust
// OLD:
    /// Scan workspace_root for directories that have no matching config entry
    /// and register them as minimal agent definitions.
    fn reconcile_orphan_workspaces(&self) {
        let entries = match fs::read_dir(&self.workspace_root) {

// NEW:
    /// Scan agents_root for directories that have no matching config entry
    /// and register them as minimal agent definitions.
    fn reconcile_orphan_workspaces(&self) {
        let entries = match fs::read_dir(&self.agents_root) {
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/config/agent_manager/crud.rs
git commit -m "agent_manager: write identity files to agents_root, scan agents_root for orphans"
```

---

### Task 3: Fix `agent_resolver.rs` — `initialize_workspace` call site

**Files:**
- Modify: `src/config/agent_resolver.rs:234-242`

- [ ] **Step 1: Fix the call site**

In `src/config/agent_resolver.rs`, replace lines 234-242:

```rust
        // OLD:
        // Workspace files (SOUL.md, AGENTS.md, etc.) go in workspace_path for user editing
        if let Err(e) = initialize_workspace(&workspace_path, agent_name) {
            tracing::warn!(
                agent_id = %agent.id,
                path = %workspace_path.display(),
                error = %e,
                "Failed to initialize workspace identity files"
            );
        }

        // NEW:
        // Identity files (SOUL.md, AGENTS.md, etc.) go in agent_dir
        if let Err(e) = initialize_workspace(&agent_dir, agent_name) {
            tracing::warn!(
                agent_id = %agent.id,
                path = %agent_dir.display(),
                error = %e,
                "Failed to initialize agent identity files"
            );
        }
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/config/agent_resolver.rs
git commit -m "agent_resolver: write identity files to agent_dir instead of workspace_path"
```

---

### Task 4: Fix `create.rs` (AgentCreateTool) — identity files path

**Files:**
- Modify: `src/builtin_tools/agent_manage/create.rs:231-312`

- [ ] **Step 1: Fix `create.rs` — identity files to agents dir, workspace stays for tool output**

In `src/builtin_tools/agent_manage/create.rs`, replace lines 231-312:

```rust
        // 3. Determine paths
        let agents_state_root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph/agents");
        let agent_state_dir = agents_state_root.join(&args.id);

        let workspaces_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph/workspaces");
        let workspace_path = workspaces_dir.join(&args.id);

        // 4. Initialize agent identity directory (SOUL.md, AGENTS.md, etc.)
        let display_name = args.name.as_deref().unwrap_or(&args.id);
        initialize_workspace(&agent_state_dir, display_name)
            .map_err(|e| crate::error::AlephError::other(format!(
                "Failed to initialize identity files for '{}': {}",
                args.id, e
            )))?;

        // Initialize agent state directory (sessions/)
        crate::config::agent_resolver::initialize_agent_dir(&agent_state_dir)
            .map_err(|e| crate::error::AlephError::other(format!(
                "Failed to initialize agent state dir for '{}': {}",
                args.id, e
            )))?;

        // Create workspace directory for tool output
        std::fs::create_dir_all(&workspace_path).map_err(|e| {
            crate::error::AlephError::other(format!(
                "Failed to create workspace for '{}': {}",
                args.id, e
            ))
        })?;

        // 5. Write custom system_prompt to AGENTS.md if provided
        if let Some(ref prompt) = args.system_prompt {
            let agents_md = agent_state_dir.join("AGENTS.md");
            let content = format!(
                "# {} Workspace\n\n\
                 ## System Prompt\n\n\
                 {}\n\n\
                 ## Instructions\n\n\
                 Add workspace-specific instructions here.\n",
                display_name, prompt
            );
            std::fs::write(&agents_md, content).map_err(|e| {
                crate::error::AlephError::other(format!(
                    "Failed to write AGENTS.md: {}", e
                ))
            })?;
        }

        // 6. Generate template files (non-fatal if write fails)
        let soul_path = agent_state_dir.join("SOUL.md");
        if !soul_path.exists() {
            let soul_content = if let Some(ref prompt) = args.system_prompt {
                prompt.clone()
            } else {
                let soul_name = args.name.as_deref().unwrap_or(&args.id);
                let specialized = match args.description.as_deref() {
                    Some(desc) => format!(" specialized in {}", desc),
                    None => String::new(),
                };
                format!(
                    "You are {}{}.\n\n\
                     ## Tone\n\
                     - Professional, friendly, concise\n\n\
                     ## Boundaries\n\
                     - Focus on your area of expertise\n\
                     - Suggest switching to another agent for out-of-scope requests\n",
                    soul_name, specialized
                )
            };
            let _ = std::fs::write(&soul_path, soul_content);
        }

        let identity_path = agent_state_dir.join("IDENTITY.md");
        if !identity_path.exists() {
            let identity_name = args.name.as_deref().unwrap_or(&args.id);
            let identity_content = format!(
                "- Name: {}\n- Emoji: \u{1f916}\n- Theme: professional\n",
                identity_name
            );
            let _ = std::fs::write(&identity_path, identity_content);
        }

        let tools_path = agent_state_dir.join("TOOLS.md");
        if !tools_path.exists() {
            let tools_content = "# Tool Notes\n\nRecord your tool usage preferences and notes here.\n";
            let _ = std::fs::write(&tools_path, tools_content);
        }
```

Note: Lines after 312 (AgentInstance creation, registry, output) remain unchanged. The `workspace_path` variable still exists and is used for `AgentInstanceConfig.workspace` (tool execution dir) — this is correct.

- [ ] **Step 2: Compile check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/builtin_tools/agent_manage/create.rs
git commit -m "agent_create: write identity files to agents dir, workspace for tool output only"
```

---

### Task 5: Update tests

**Files:**
- Modify: `src/config/agent_manager/tests.rs`

- [ ] **Step 1: Fix `test_create_agent` — check SOUL.md in agents_root**

Replace lines 97-103:

```rust
    // OLD:
    // Verify workspace directory was created
    let ws_dir = mgr.workspace_root.join("researcher");
    assert!(ws_dir.exists());

    // Verify SOUL.md was created
    let soul = fs::read_to_string(ws_dir.join("SOUL.md")).unwrap();
    assert!(soul.contains("Research Agent"));

    // NEW:
    // Verify agent identity directory was created with SOUL.md
    let agent_dir = mgr.agents_root.join("researcher");
    assert!(agent_dir.exists());

    let soul = fs::read_to_string(agent_dir.join("SOUL.md")).unwrap();
    assert!(soul.contains("Research Agent"));
```

- [ ] **Step 2: Fix `test_create_creates_both_directories` — identity files in agents_root**

Replace lines 227-235:

```rust
    // OLD:
    // Workspace content dir
    assert!(mgr.workspace_root.join("dual").join("SOUL.md").exists());
    assert!(mgr.workspace_root.join("dual").join("MEMORY.md").exists());

    // Agent state dir
    assert!(mgr.agents_root.join("dual").join("sessions").is_dir());

    // sessions/ should NOT be in workspace
    assert!(!mgr.workspace_root.join("dual").join("sessions").exists());

    // NEW:
    // Agent identity dir has identity files + sessions
    assert!(mgr.agents_root.join("dual").join("SOUL.md").exists());
    assert!(mgr.agents_root.join("dual").join("MEMORY.md").exists());
    assert!(mgr.agents_root.join("dual").join("sessions").is_dir());

    // Identity files should NOT be in workspace
    assert!(!mgr.workspace_root.join("dual").join("SOUL.md").exists());
```

- [ ] **Step 3: Fix `test_delete_trashes_both_directories` — create SOUL.md in agents_root**

Replace lines 242-249:

```rust
    // OLD:
    // Pre-create both dirs for coder
    fs::create_dir_all(mgr.workspace_root.join("coder")).unwrap();
    fs::write(
        mgr.workspace_root.join("coder").join("SOUL.md"),
        "test",
    )
    .unwrap();
    fs::create_dir_all(mgr.agents_root.join("coder").join("sessions")).unwrap();

    // NEW:
    // Pre-create both dirs for coder
    fs::create_dir_all(mgr.workspace_root.join("coder")).unwrap();
    fs::create_dir_all(mgr.agents_root.join("coder").join("sessions")).unwrap();
    fs::write(
        mgr.agents_root.join("coder").join("SOUL.md"),
        "test",
    )
    .unwrap();
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib agent_manager`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/config/agent_manager/tests.rs
git commit -m "agent_manager: update tests to expect identity files in agents_root"
```

---

### Task 6: Full verification

- [ ] **Step 1: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: PASS (pre-existing `markdown_skill::loader` failures are known and unrelated)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | head -50`
Expected: No new warnings

- [ ] **Step 3: Final commit (if any fixes needed)**

Only if clippy or tests revealed issues.
