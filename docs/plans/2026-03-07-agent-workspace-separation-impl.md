# Agent-Workspace Separation Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Separate agent runtime state (`~/.aleph/agents/{id}/`) from workspace content (`~/.aleph/workspaces/{id}/`) to enable future multi-agent workspace sharing.

**Architecture:** Add `agents_root` alongside existing `workspace_root` in config. `AgentDefinitionResolver` creates both directories on resolve. Lazy migration moves `sessions/` from old unified location to the new agent state directory.

**Tech Stack:** Rust, toml/toml_edit (config), std::fs (directory ops), tempfile (tests)

---

### Task 1: Add `agents_root` to `AgentDefaults`

**Files:**
- Modify: `core/src/config/types/agents_def.rs:83-108` (AgentDefaults struct)

**Step 1: Write the failing test**

Add to the existing test module in `core/src/config/types/agents_def.rs`:

```rust
#[test]
fn test_agents_root_deserialize() {
    let toml_str = r#"
        [defaults]
        agents_root = "/home/user/agents"
        workspace_root = "/home/user/workspaces"
    "#;
    let config: AgentsConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(
        config.defaults.agents_root,
        Some(PathBuf::from("/home/user/agents"))
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::types::agents_def::tests::test_agents_root_deserialize`
Expected: FAIL — field `agents_root` does not exist on `AgentDefaults`

**Step 3: Write minimal implementation**

Add to `AgentDefaults` struct in `agents_def.rs`:

```rust
/// Default agent state root directory (default: ~/.aleph/agents)
#[serde(default, skip_serializing_if = "Option::is_none")]
pub agents_root: Option<PathBuf>,
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::types::agents_def::tests::test_agents_root_deserialize`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/config/types/agents_def.rs
git commit -m "config: add agents_root field to AgentDefaults"
```

---

### Task 2: Add `agent_dir` to `ResolvedAgent` and resolve it

**Files:**
- Modify: `core/src/config/agent_resolver.rs:42-75` (ResolvedAgent struct)
- Modify: `core/src/config/agent_resolver.rs:146-174` (resolve_workspace_path area — add resolve_agent_dir)
- Modify: `core/src/config/agent_resolver.rs:177-254` (resolve_one — call both resolvers)
- Modify: `core/src/config/agent_resolver.rs:334-340` (add default_agents_root helper)

**Step 1: Write the failing test**

Add to the test module in `agent_resolver.rs`:

```rust
#[test]
fn test_resolve_creates_dual_directories() {
    let tmp = TempDir::new().unwrap();
    let workspace_root = tmp.path().join("workspaces");
    let agents_root = tmp.path().join("agents");

    let config = AgentsConfig {
        defaults: AgentDefaults {
            workspace_root: Some(workspace_root.clone()),
            agents_root: Some(agents_root.clone()),
            ..Default::default()
        },
        list: vec![AgentDefinition {
            id: "coder".to_string(),
            name: Some("Coder".to_string()),
            ..Default::default()
        }],
    };

    let profiles = HashMap::new();
    let mut resolver = AgentDefinitionResolver::new();
    let resolved = resolver.resolve_all(&config, &profiles);

    assert_eq!(resolved.len(), 1);
    let agent = &resolved[0];

    // Workspace content dir
    assert_eq!(agent.workspace_path, workspace_root.join("coder"));
    assert!(agent.workspace_path.join("memory").is_dir());
    assert!(agent.workspace_path.join("SOUL.md").exists());

    // Agent state dir
    assert_eq!(agent.agent_dir, agents_root.join("coder"));
    assert!(agent.agent_dir.join("sessions").is_dir());

    // sessions/ should NOT be in workspace
    assert!(!agent.workspace_path.join("sessions").exists());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::agent_resolver::tests::test_resolve_creates_dual_directories`
Expected: FAIL — no field `agent_dir` on `ResolvedAgent`

**Step 3: Write minimal implementation**

3a. Add `agent_dir` field to `ResolvedAgent`:

```rust
/// Resolved agent state directory path
pub agent_dir: PathBuf,
```

3b. Add `default_agents_root()` helper:

```rust
/// Default agent state root directory: `~/.aleph/agents`.
fn default_agents_root() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph")
        .join("agents")
}
```

3c. Add `resolve_agent_dir()` method to `AgentDefinitionResolver`:

```rust
/// Resolve the agent state directory path.
///
/// Uses `{agents_root}/{agent_id}` pattern.
pub fn resolve_agent_dir(
    &self,
    agent: &AgentDefinition,
    defaults: &AgentDefaults,
) -> PathBuf {
    let root = defaults
        .agents_root
        .as_ref()
        .map(|p| resolve_user_path(p))
        .unwrap_or_else(default_agents_root);
    root.join(&agent.id)
}
```

3d. Add `initialize_agent_dir()` public function:

```rust
/// Initialize agent state directory structure.
///
/// ```text
/// ~/.aleph/agents/{id}/
/// └── sessions/         # Session persistence
/// ```
pub fn initialize_agent_dir(path: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(path.join("sessions"))?;
    Ok(())
}
```

3e. Remove `sessions/` creation from `initialize_workspace()` (line 280):

Remove the line:
```rust
fs::create_dir_all(path.join("sessions"))?;
```

3f. Update `resolve_one()` to call both:

After `let workspace_path = ...` (line 184), add:

```rust
// 1b. Resolve agent state directory
let agent_dir = self.resolve_agent_dir(agent, defaults);
```

After workspace initialization (after line 198), add:

```rust
// 2b. Initialize agent state directory
if let Err(e) = initialize_agent_dir(&agent_dir) {
    tracing::warn!(
        agent_id = %agent.id,
        path = %agent_dir.display(),
        error = %e,
        "Failed to initialize agent state directory"
    );
}
```

Add `agent_dir` to the `ResolvedAgent` construction (after `workspace_path,`):

```rust
agent_dir,
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib config::agent_resolver::tests::test_resolve_creates_dual_directories`
Expected: PASS

**Step 5: Fix existing tests**

The `test_resolve_all_basic` test checks `workspace_path.join("memory").exists()` — this should still pass. But it doesn't check for `sessions/` in workspace, so no breakage expected. However, `ResolvedAgent` construction in existing tests will need `agent_dir` — but tests go through `resolve_all()` which handles this internally.

Run: `cargo test -p alephcore --lib config::agent_resolver::tests`
Expected: All PASS

**Step 6: Commit**

```bash
git add core/src/config/agent_resolver.rs
git commit -m "agent: add agent_dir to ResolvedAgent, separate state from content"
```

---

### Task 3: Lazy migration of sessions/

**Files:**
- Modify: `core/src/config/agent_resolver.rs` (resolve_one — add migration logic)

**Step 1: Write the failing test**

```rust
#[test]
fn test_lazy_migration_moves_sessions() {
    let tmp = TempDir::new().unwrap();
    let workspace_root = tmp.path().join("workspaces");
    let agents_root = tmp.path().join("agents");

    // Pre-create old unified layout: sessions/ inside workspace
    let old_sessions = workspace_root.join("migrator").join("sessions");
    fs::create_dir_all(&old_sessions).unwrap();
    fs::write(old_sessions.join("test-session.json"), "{}").unwrap();

    // Also need SOUL.md etc. to exist so initialize_workspace doesn't overwrite
    let ws = workspace_root.join("migrator");
    fs::write(ws.join("SOUL.md"), "# Migrator").unwrap();

    let config = AgentsConfig {
        defaults: AgentDefaults {
            workspace_root: Some(workspace_root.clone()),
            agents_root: Some(agents_root.clone()),
            ..Default::default()
        },
        list: vec![AgentDefinition {
            id: "migrator".to_string(),
            ..Default::default()
        }],
    };

    let profiles = HashMap::new();
    let mut resolver = AgentDefinitionResolver::new();
    let resolved = resolver.resolve_all(&config, &profiles);

    let agent = &resolved[0];

    // sessions/ should have moved to agent_dir
    assert!(agent.agent_dir.join("sessions").join("test-session.json").exists());

    // sessions/ should no longer exist in workspace
    assert!(!agent.workspace_path.join("sessions").exists());
}

#[test]
fn test_no_migration_when_agent_dir_exists() {
    let tmp = TempDir::new().unwrap();
    let workspace_root = tmp.path().join("workspaces");
    let agents_root = tmp.path().join("agents");

    // Pre-create both directories
    let old_sessions = workspace_root.join("stable").join("sessions");
    fs::create_dir_all(&old_sessions).unwrap();
    fs::write(old_sessions.join("old.json"), "old").unwrap();

    let new_sessions = agents_root.join("stable").join("sessions");
    fs::create_dir_all(&new_sessions).unwrap();
    fs::write(new_sessions.join("new.json"), "new").unwrap();

    let config = AgentsConfig {
        defaults: AgentDefaults {
            workspace_root: Some(workspace_root.clone()),
            agents_root: Some(agents_root.clone()),
            ..Default::default()
        },
        list: vec![AgentDefinition {
            id: "stable".to_string(),
            ..Default::default()
        }],
    };

    let profiles = HashMap::new();
    let mut resolver = AgentDefinitionResolver::new();
    resolver.resolve_all(&config, &profiles);

    // Old sessions should still be in workspace (not moved, since agent_dir already existed)
    assert!(old_sessions.join("old.json").exists());
    // New sessions untouched
    assert!(new_sessions.join("new.json").exists());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib config::agent_resolver::tests::test_lazy_migration`
Expected: FAIL

**Step 3: Write minimal implementation**

Add migration logic in `resolve_one()`, after both directories are initialized:

```rust
// 2c. Lazy migration: move sessions/ from workspace to agent_dir
let old_sessions = workspace_path.join("sessions");
if !agent_dir.exists() || !agent_dir.join("sessions").exists() {
    if old_sessions.is_dir() {
        // Only migrate if agent_dir/sessions doesn't already exist
        let new_sessions = agent_dir.join("sessions");
        if !new_sessions.exists() {
            tracing::info!(
                agent_id = %agent.id,
                "Migrating sessions/ from workspace to agent state directory"
            );
            // Ensure agent_dir exists
            let _ = fs::create_dir_all(&agent_dir);
            if let Err(e) = fs::rename(&old_sessions, &new_sessions) {
                tracing::warn!(
                    agent_id = %agent.id,
                    error = %e,
                    "Failed to migrate sessions/, copying instead"
                );
                // Fallback: sessions/ stays in both places
            }
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib config::agent_resolver::tests`
Expected: All PASS

**Step 5: Commit**

```bash
git add core/src/config/agent_resolver.rs
git commit -m "agent: add lazy migration for sessions/ from workspace to agent_dir"
```

---

### Task 4: Update AgentManager to use dual directories

**Files:**
- Modify: `core/src/config/agent_manager.rs:74-78` (AgentManager struct — add agents_root)
- Modify: `core/src/config/agent_manager.rs:86-127` (new — accept agents_root)
- Modify: `core/src/config/agent_manager.rs:153-181` (create — create both dirs)
- Modify: `core/src/config/agent_manager.rs:282-343` (delete — trash both dirs)

**Step 1: Write the failing test**

Update the `setup()` helper and add a test:

```rust
fn setup(config_content: &str) -> (TempDir, AgentManager) {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    let workspace_root = dir.path().join("workspaces");
    let agents_root = dir.path().join("agents");
    let trash_root = dir.path().join("trash");

    fs::create_dir_all(&workspace_root).unwrap();
    fs::create_dir_all(&agents_root).unwrap();
    fs::create_dir_all(&trash_root).unwrap();
    fs::write(&config_path, config_content).unwrap();

    let manager = AgentManager::new(config_path, workspace_root, agents_root, trash_root);
    (dir, manager)
}

#[test]
fn test_create_creates_both_directories() {
    let (_dir, mgr) = setup(base_config());
    let def = AgentDefinition {
        id: "dual".to_string(),
        name: Some("Dual Agent".to_string()),
        ..Default::default()
    };

    mgr.create(def).unwrap();

    // Workspace content dir
    assert!(mgr.workspace_root.join("dual").join("SOUL.md").exists());
    assert!(mgr.workspace_root.join("dual").join("memory").is_dir());

    // Agent state dir
    assert!(mgr.agents_root.join("dual").join("sessions").is_dir());

    // sessions/ should NOT be in workspace
    assert!(!mgr.workspace_root.join("dual").join("sessions").exists());
}

#[test]
fn test_delete_trashes_both_directories() {
    let (_dir, mgr) = setup(base_config());

    // Pre-create both dirs for coder
    fs::create_dir_all(mgr.workspace_root.join("coder")).unwrap();
    fs::write(mgr.workspace_root.join("coder").join("SOUL.md"), "test").unwrap();
    fs::create_dir_all(mgr.agents_root.join("coder").join("sessions")).unwrap();

    mgr.delete("coder").unwrap();

    assert!(!mgr.workspace_root.join("coder").exists());
    assert!(!mgr.agents_root.join("coder").exists());

    // Both should be in trash
    let trash_entries: Vec<_> = fs::read_dir(&mgr.trash_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    // workspace trash + agent_dir trash = 2 entries
    assert_eq!(trash_entries.len(), 2);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::agent_manager::tests::test_create_creates_both`
Expected: FAIL — `AgentManager::new` doesn't accept `agents_root`

**Step 3: Write minimal implementation**

3a. Add `agents_root` field to `AgentManager`:

```rust
pub struct AgentManager {
    config_path: PathBuf,
    workspace_root: PathBuf,
    agents_root: PathBuf,
    trash_root: PathBuf,
}
```

3b. Update `new()` signature:

```rust
pub fn new(
    config_path: PathBuf,
    workspace_root: PathBuf,
    agents_root: PathBuf,
    trash_root: PathBuf,
) -> Self {
    let mgr = Self {
        config_path,
        workspace_root,
        agents_root,
        trash_root,
    };
    // ... rest unchanged
}
```

3c. Update `create()` to initialize agent_dir:

After the `initialize_workspace` call, add:

```rust
// Initialize agent state directory
let agent_state_dir = self.agents_root.join(&def.id);
initialize_agent_dir(&agent_state_dir).map_err(|e| {
    AlephError::IoError(format!(
        "Failed to initialize agent state dir for '{}': {}",
        def.id, e
    ))
})?;
```

Add the import at the top:
```rust
use crate::config::agent_resolver::{initialize_workspace, initialize_agent_dir};
```

3d. Update `delete()` to trash both directories:

After the existing workspace trash logic (lines 324-339), add:

```rust
// Move agent state directory to trash
let agent_state_dir = self.agents_root.join(id);
if agent_state_dir.exists() {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let trash_name = format!("{}_agent_{}", id, timestamp);
    let trash_dir = self.trash_root.join(trash_name);
    fs::create_dir_all(&self.trash_root).map_err(|e| {
        AlephError::IoError(format!("Failed to create trash dir: {}", e))
    })?;
    fs::rename(&agent_state_dir, &trash_dir).map_err(|e| {
        AlephError::IoError(format!(
            "Failed to move agent state dir to trash: {}",
            e
        ))
    })?;
    info!("Moved agent state to trash: {}", trash_dir.display());
}
```

**Step 4: Run all agent_manager tests**

Run: `cargo test -p alephcore --lib config::agent_manager::tests`
Expected: All PASS

**Step 5: Commit**

```bash
git add core/src/config/agent_manager.rs core/src/config/agent_resolver.rs
git commit -m "agent: AgentManager creates/deletes dual directories"
```

---

### Task 5: Update AgentInstanceConfig to include agent_dir

**Files:**
- Modify: `core/src/gateway/agent_instance.rs:18-52` (AgentInstanceConfig struct + from_resolved)

**Step 1: Add `agent_dir` field and update `from_resolved`**

Add to `AgentInstanceConfig`:
```rust
/// Agent state directory (sessions, runtime state)
pub agent_dir: PathBuf,
```

Update `Default`:
```rust
agent_dir: dirs::home_dir()
    .unwrap_or_else(|| PathBuf::from("/tmp"))
    .join(".aleph/agents/main"),
```

Update `from_resolved()`:
```rust
agent_dir: agent.agent_dir.clone(),
```

**Step 2: Run build**

Run: `cargo check -p alephcore`
Expected: PASS (or shows other callers that need updating — fix them)

**Step 3: Commit**

```bash
git add core/src/gateway/agent_instance.rs
git commit -m "agent: add agent_dir to AgentInstanceConfig"
```

---

### Task 6: Update server startup to pass agents_root

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs:1437-1441` (AgentManager::new call)

**Step 1: Update the AgentManager construction**

Change from:
```rust
let agent_manager = Arc::new(alephcore::AgentManager::new(
    alephcore::Config::default_path(),
    dirs::home_dir().unwrap_or_default().join(".aleph/workspaces"),
    dirs::home_dir().unwrap_or_default().join(".aleph/trash"),
));
```

To:
```rust
let agent_manager = Arc::new(alephcore::AgentManager::new(
    alephcore::Config::default_path(),
    dirs::home_dir().unwrap_or_default().join(".aleph/workspaces"),
    dirs::home_dir().unwrap_or_default().join(".aleph/agents"),
    dirs::home_dir().unwrap_or_default().join(".aleph/trash"),
));
```

**Step 2: Update agent_create builtin tool**

In `core/src/builtin_tools/agent_manage/create.rs`, update the workspace path section to also create agent_dir:

After the `initialize_workspace` call (line 157), add:
```rust
// Initialize agent state directory
let agents_dir = dirs::home_dir()
    .unwrap_or_else(|| PathBuf::from("/tmp"))
    .join(".aleph/agents");
let agent_state_dir = agents_dir.join(&args.id);
initialize_agent_dir(&agent_state_dir)
    .map_err(|e| crate::error::AlephError::other(format!(
        "Failed to initialize agent state dir for '{}': {}",
        args.id, e
    )))?;
```

Add the import:
```rust
use crate::config::agent_resolver::{initialize_workspace, initialize_agent_dir};
```

**Step 3: Build and verify**

Run: `cargo check -p alephcore && cargo check -p aleph-server`
Expected: PASS

**Step 4: Commit**

```bash
git add core/src/bin/aleph/commands/start/mod.rs core/src/builtin_tools/agent_manage/create.rs
git commit -m "agent: wire agents_root through server startup and builtin tools"
```

---

### Task 7: Run full test suite and fix breakage

**Files:**
- Potentially any file that constructs `AgentManager::new` or `ResolvedAgent` directly

**Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | head -100`

**Step 2: Fix any compilation or test failures**

Common fixes needed:
- Any direct `AgentManager::new(path, ws, trash)` calls need the 4th `agents_root` arg
- Any direct `ResolvedAgent { ... }` construction needs `agent_dir` field
- `test_workspace_initialization` may need updating if `sessions/` creation was removed

**Step 3: Run again to confirm**

Run: `cargo test -p alephcore --lib`
Expected: All PASS (excluding pre-existing failures in `markdown_skill`)

**Step 4: Commit**

```bash
git add -u
git commit -m "agent: fix test breakage from agent-workspace separation"
```

---

### Task 8: Final integration commit

**Step 1: Run full build**

Run: `cargo build -p alephcore`
Expected: PASS

**Step 2: Verify directory structure manually**

Run: `cargo run --bin aleph -- start --help` (just to verify binary compiles)

**Step 3: Final commit if any remaining changes**

```bash
git add -u
git commit -m "agent: complete agent-workspace separation implementation"
```
