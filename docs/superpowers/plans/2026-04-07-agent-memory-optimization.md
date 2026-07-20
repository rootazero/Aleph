# Agent Memory Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce Aleph memory footprint from ~418MB to ~200MB by eliminating redundant in-memory session caches and deferring agent instantiation to first use.

**Architecture:** Two changes: (1) Remove the per-agent in-memory `HashMap<String, SessionData>` that duplicates SQLite — all session reads/writes go through `SessionManager` directly. (2) Convert `AgentRegistry` to store configs lazily, instantiating `AgentInstance` on first access instead of at startup.

**Tech Stack:** Rust, SQLite (rusqlite), tokio async

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/agent_instance.rs` | Modify | Remove `sessions` HashMap, delegate to SessionManager |
| `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Modify | Register configs lazily instead of creating all instances |

---

### Task 1: Remove In-Memory Session Cache from AgentInstance

The `sessions: Arc<RwLock<HashMap<String, SessionData>>>` field in `AgentInstance` duplicates data already persisted in SQLite via `SessionManager`. Every `add_message()` writes to both, every `get_history()` reads only from memory. This is pure redundancy — remove the in-memory copy and delegate all session operations to `SessionManager`.

**Files:**
- Modify: `src/gateway/agent_instance.rs`

- [ ] **Step 1: Remove SessionData, sessions field, and delegate session methods to SessionManager**

In `src/gateway/agent_instance.rs`:

1. Remove the `SessionData` struct (lines 146-151)
2. Remove `sessions` field from `AgentInstance` (line 140)
3. Make `session_manager` non-optional — it's always required now: `session_manager: Arc<SessionManager>`
4. Remove `AgentInstance::new()` (the no-SessionManager constructor). Keep only `with_session_manager()`, renamed to `new()`.
5. Rewrite `get_or_create_session()` to delegate to `self.session_manager.get_or_create(key)`
6. Rewrite `ensure_session()` to delegate to `self.session_manager.get_or_create(key)`
7. Rewrite `add_message()` to call only `self.session_manager.add_message(key, role_str, content)`
8. Rewrite `get_history()` to call `self.session_manager.get_history(key, limit)` and convert `StoredMessage` → `SessionMessage`
9. Rewrite `reset_session()` to delegate to `self.session_manager.reset_session(key)`
10. Rewrite `list_sessions()` to delegate to `self.session_manager.list_sessions()` filtered by agent_id
11. Remove `HashMap` import for sessions (keep if used elsewhere)

Key conversion — `StoredMessage` to `SessionMessage`:

```rust
fn stored_to_session(msg: &StoredMessage) -> SessionMessage {
    SessionMessage {
        role: match msg.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        },
        content: msg.content.clone(),
        timestamp: msg.timestamp.parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap_or_else(|_| chrono::Utc::now()),
        metadata: msg.metadata.as_ref().and_then(|m| serde_json::from_str(m).ok()),
    }
}
```

- [ ] **Step 2: Update AgentInstance::new() signature and constructor**

Replace both constructors with a single one:

```rust
pub fn new(
    config: AgentInstanceConfig,
    session_manager: Arc<SessionManager>,
) -> Result<Self, AgentInstanceError> {
    let agent_dir = config.agent_dir.clone();
    std::fs::create_dir_all(&agent_dir).map_err(|e| {
        AgentInstanceError::InitFailed(format!("Failed to create agent dir: {}", e))
    })?;
    std::fs::create_dir_all(&config.workspace).map_err(|e| {
        AgentInstanceError::InitFailed(format!("Failed to create workspace: {}", e))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        let _ = std::fs::set_permissions(&agent_dir, perms);
    }
    info!("Created agent instance '{}' at {:?}", config.agent_id, agent_dir);
    Ok(Self {
        config,
        state: Arc::new(RwLock::new(AgentState::Idle)),
        agent_dir,
        session_manager,
    })
}
```

- [ ] **Step 3: Fix all callers of the old constructors**

In `agent_init.rs`, replace:
- `AgentInstance::with_session_manager(config, session_manager.clone())` → `AgentInstance::new(config, session_manager.clone())`

In `AgentRegistry::create_dynamic()`:
- The `session_manager` parameter must become non-optional: `session_manager: Arc<SessionManager>`
- Remove the if/else branch, always pass session_manager

- [ ] **Step 4: Fix tests in agent_instance.rs**

Tests that use `AgentInstance::new(config)` without a SessionManager need a mock or real SessionManager. Create a helper:

```rust
#[cfg(test)]
fn test_session_manager() -> Arc<SessionManager> {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("test_sessions.db");
    Arc::new(SessionManager::new(&db_path).expect("test session manager"))
}
```

Update all test functions to pass `test_session_manager()` to `AgentInstance::new()`.

- [ ] **Step 5: Compile and run tests**

Run: `cargo test -p alephcore --lib -- agent_instance`
Expected: All tests pass

- [ ] **Step 6: Commit**

```
fix(memory): remove redundant in-memory session cache from AgentInstance

Session data was stored in both an in-memory HashMap and SQLite via
SessionManager. All session operations now delegate to SessionManager,
eliminating O(agents × sessions × messages) memory duplication.
```

---

### Task 2: Agent Lazy Loading in AgentRegistry

Currently all 20+ agents are fully instantiated at startup (directories created, state allocated). Convert `AgentRegistry` to store `AgentInstanceConfig` and create `AgentInstance` on first access.

**Files:**
- Modify: `src/gateway/agent_instance.rs` (AgentRegistry section)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs`

- [ ] **Step 1: Add lazy entry type to AgentRegistry**

In `agent_instance.rs`, add a new enum and update the registry:

```rust
/// Lazy-loaded agent entry: either just a config or a fully initialized instance.
enum AgentEntry {
    /// Agent registered but not yet instantiated
    Config {
        config: AgentInstanceConfig,
        session_manager: Arc<SessionManager>,
    },
    /// Fully instantiated agent
    Instance(Arc<AgentInstance>),
}

pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, AgentEntry>>>,
    default_agent: String,
}
```

- [ ] **Step 2: Add register_config() method and update get()**

```rust
impl AgentRegistry {
    /// Register an agent config for lazy instantiation
    pub async fn register_config(
        &self,
        config: AgentInstanceConfig,
        session_manager: Arc<SessionManager>,
    ) {
        let id = config.agent_id.clone();
        let mut agents = self.agents.write().await;
        agents.insert(id.clone(), AgentEntry::Config { config, session_manager });
        info!("Registered agent config (lazy): {}", id);
    }

    /// Register a pre-built instance (for backwards compat / tests)
    pub async fn register(&self, instance: AgentInstance) {
        let id = instance.id().to_string();
        let mut agents = self.agents.write().await;
        agents.insert(id.clone(), AgentEntry::Instance(Arc::new(instance)));
        info!("Registered agent: {}", id);
    }

    /// Get an agent, instantiating lazily if needed
    pub async fn get(&self, agent_id: &str) -> Option<Arc<AgentInstance>> {
        // Fast path: already instantiated
        {
            let agents = self.agents.read().await;
            if let Some(AgentEntry::Instance(inst)) = agents.get(agent_id) {
                return Some(inst.clone());
            }
        }

        // Slow path: need to instantiate from config
        let mut agents = self.agents.write().await;
        let entry = agents.get(agent_id)?;

        match entry {
            AgentEntry::Instance(inst) => Some(inst.clone()),
            AgentEntry::Config { .. } => {
                // Take the config out to instantiate
                let entry = agents.remove(agent_id)?;
                if let AgentEntry::Config { config, session_manager } = entry {
                    let id = config.agent_id.clone();
                    match AgentInstance::new(config, session_manager) {
                        Ok(instance) => {
                            let arc = Arc::new(instance);
                            agents.insert(id, AgentEntry::Instance(arc.clone()));
                            Some(arc)
                        }
                        Err(e) => {
                            warn!("Failed to lazily instantiate agent '{}': {}", id, e);
                            None
                        }
                    }
                } else {
                    unreachable!()
                }
            }
        }
    }
}
```

- [ ] **Step 3: Update list(), find_by_name(), get_allowed_links(), remove()**

These methods need to work with both `AgentEntry::Config` and `AgentEntry::Instance`:

- `list()`: return all keys from the HashMap (works as-is with both variants)
- `find_by_name()`: extract display_name from either config or instance
- `get_allowed_links()`: extract from either config or instance
- `remove()`: remove from HashMap regardless of variant

```rust
pub async fn list(&self) -> Vec<String> {
    let agents = self.agents.read().await;
    agents.keys().cloned().collect()
}

pub async fn find_by_name(&self, name: &str) -> Option<String> {
    let agents = self.agents.read().await;
    let name_lower = name.to_lowercase();
    let mut matched_id: Option<String> = None;

    for (id, entry) in agents.iter() {
        let display = match entry {
            AgentEntry::Instance(inst) => inst.display_name().to_lowercase(),
            AgentEntry::Config { config, .. } => config
                .display_name
                .as_deref()
                .unwrap_or(&config.agent_id)
                .to_lowercase(),
        };
        if display == name_lower || display.contains(&name_lower) || name_lower.contains(&display) {
            if matched_id.is_some() {
                if display == name_lower {
                    matched_id = Some(id.clone());
                }
            } else {
                matched_id = Some(id.clone());
            }
        }
    }
    matched_id
}

pub async fn get_allowed_links(&self, agent_id: &str) -> Option<Option<Vec<String>>> {
    let agents = self.agents.read().await;
    match agents.get(agent_id)? {
        AgentEntry::Instance(inst) => Some(inst.config().allowed_links.clone()),
        AgentEntry::Config { config, .. } => Some(config.allowed_links.clone()),
    }
}
```

- [ ] **Step 4: Update agent_init.rs to use register_config()**

Replace the agent registration loop (lines 1069-1120) to use `register_config()` instead of creating instances:

```rust
// In the new agents path (line 1069):
for agent in &resolved_agents {
    let config = alephcore::gateway::AgentInstanceConfig::from_resolved(agent);
    let agent_id = config.agent_id.clone();
    agent_registry.register_config(config, session_manager.clone()).await;
    if !daemon {
        println!("  Registered agent: {} (lazy)", agent_id);
    }
}

// In the legacy path (line 1101):
for agent_config in full_config.get_agent_instance_configs() {
    let agent_id = agent_config.agent_id.clone();
    agent_registry.register_config(agent_config, session_manager.clone()).await;
    if !daemon {
        println!("  Registered agent: {} (lazy)", agent_id);
    }
}
```

- [ ] **Step 5: Update create_dynamic() for new API**

```rust
pub async fn create_dynamic(
    &self,
    id: &str,
    soul_content: &str,
    session_manager: Arc<SessionManager>,
) -> Result<Arc<AgentInstance>, AgentInstanceError> {
    // Check existence in the map (both Config and Instance variants)
    {
        let agents = self.agents.read().await;
        if agents.contains_key(id) {
            return Err(AgentInstanceError::InitFailed(format!(
                "Agent '{}' already exists", id
            )));
        }
    }
    // ... rest unchanged, but use AgentInstance::new(config, session_manager)
}
```

- [ ] **Step 6: Fix tests and compile**

Run: `cargo test -p alephcore --lib -- agent_instance`
Expected: All tests pass

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 7: Commit**

```
perf(agents): lazy-load agent instances on first access

AgentRegistry now stores AgentInstanceConfig at startup and defers
AgentInstance creation to first get(). With 20+ agents configured,
this avoids ~20 directory creations, state allocations, and HashMap
initializations that were previously done eagerly at startup.
```

---

### Task 3: Build, Measure, Verify

- [ ] **Step 1: Full build**

```bash
just build
```

- [ ] **Step 2: Restart and measure memory**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
nohup target/release/aleph-server start > /tmp/aleph-server.log 2>&1 &
sleep 5
vmmap --summary $(pgrep -f "target/release/aleph-server") 2>/dev/null | head -5
```

Expected: Physical footprint significantly lower than 418MB.

- [ ] **Step 3: Functional verification**

Send a test message to verify agent lazy loading + session persistence works:
- Agent should be instantiated on first request
- Messages should persist across restarts (SQLite)
- Session history should be retrievable

- [ ] **Step 4: Final commit if any fixes needed**
