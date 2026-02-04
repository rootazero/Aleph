# Part B: Agent-to-Agent Communication - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement inter-session communication tools (sessions_list, sessions_send, sessions_spawn) with A2A policy control and sandbox visibility.

**Architecture:** Create a new `tools/sessions` module with three core tools, A2A policy engine, sandbox visibility control, and sub-agent registry. Integrates with the new `routing` module from Part C.

**Tech Stack:** Rust, serde, schemars (JsonSchema), tokio, uuid

---

### Task 1: Create sessions module structure with A2A policy

**Files:**
- Create: `core/src/tools/sessions/mod.rs`
- Create: `core/src/tools/sessions/policy.rs`
- Modify: `core/src/tools/mod.rs`

**Step 1: Create `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod policy;

pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
```

**Step 2: Create `core/src/tools/sessions/policy.rs` with tests**

```rust
//! Agent-to-Agent communication policy.
//!
//! Controls which agents can communicate with each other.

use serde::{Deserialize, Serialize};

/// Agent-to-Agent policy configuration
#[derive(Debug, Clone, Default)]
pub struct AgentToAgentPolicy {
    /// Whether A2A communication is enabled
    pub enabled: bool,
    /// Allow rules
    rules: Vec<A2ARule>,
}

/// A2A allow rule
#[derive(Debug, Clone)]
pub struct A2ARule {
    pub from: RuleMatcher,
    pub to: RuleMatcher,
}

/// Rule matcher for agent IDs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleMatcher {
    /// Matches any agent ("*")
    Any,
    /// Matches specific agent ID
    Specific(String),
}

impl AgentToAgentPolicy {
    /// Create a new policy with rules
    pub fn new(enabled: bool, rules: Vec<A2ARule>) -> Self {
        Self { enabled, rules }
    }

    /// Create from config allow list
    pub fn from_allow_list(enabled: bool, allows: &[String]) -> Self {
        let rules = allows.iter().filter_map(|s| A2ARule::parse(s)).collect();
        Self { enabled, rules }
    }

    /// Check if communication from one agent to another is allowed
    pub fn is_allowed(&self, from_agent: &str, to_agent: &str) -> bool {
        // Disabled = no A2A communication
        if !self.enabled {
            return false;
        }

        // Same agent always allowed
        if from_agent.eq_ignore_ascii_case(to_agent) {
            return true;
        }

        // Check rules
        self.rules.iter().any(|rule| {
            rule.from.matches(from_agent) && rule.to.matches(to_agent)
        })
    }

    /// Add a rule
    pub fn add_rule(&mut self, rule: A2ARule) {
        self.rules.push(rule);
    }
}

impl A2ARule {
    /// Create a new rule
    pub fn new(from: RuleMatcher, to: RuleMatcher) -> Self {
        Self { from, to }
    }

    /// Parse a rule from string format: "from -> to" or "*"
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();

        // "*" means any to any
        if s == "*" {
            return Some(Self {
                from: RuleMatcher::Any,
                to: RuleMatcher::Any,
            });
        }

        // "from -> to" format
        let parts: Vec<&str> = s.split("->").map(|p| p.trim()).collect();
        match parts.as_slice() {
            [from, to] => Some(Self {
                from: RuleMatcher::parse(from),
                to: RuleMatcher::parse(to),
            }),
            _ => None,
        }
    }
}

impl RuleMatcher {
    /// Parse from string: "*" or specific agent ID
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s == "*" {
            Self::Any
        } else {
            Self::Specific(s.to_lowercase())
        }
    }

    /// Check if this matcher matches the given agent ID
    pub fn matches(&self, agent_id: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Specific(id) => id.eq_ignore_ascii_case(agent_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_disabled() {
        let policy = AgentToAgentPolicy::new(false, vec![]);
        assert!(!policy.is_allowed("main", "work"));
    }

    #[test]
    fn test_policy_same_agent_always_allowed() {
        let policy = AgentToAgentPolicy::new(true, vec![]);
        assert!(policy.is_allowed("main", "main"));
        assert!(policy.is_allowed("MAIN", "main")); // case insensitive
    }

    #[test]
    fn test_policy_no_rules_denies_cross_agent() {
        let policy = AgentToAgentPolicy::new(true, vec![]);
        assert!(!policy.is_allowed("main", "work"));
    }

    #[test]
    fn test_policy_wildcard_rule() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["*".to_string()]);
        assert!(policy.is_allowed("main", "work"));
        assert!(policy.is_allowed("work", "main"));
    }

    #[test]
    fn test_policy_specific_rule() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["main -> work".to_string()]);
        assert!(policy.is_allowed("main", "work"));
        assert!(!policy.is_allowed("work", "main")); // reverse not allowed
    }

    #[test]
    fn test_policy_wildcard_from() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["* -> monitor".to_string()]);
        assert!(policy.is_allowed("main", "monitor"));
        assert!(policy.is_allowed("work", "monitor"));
        assert!(!policy.is_allowed("main", "work"));
    }

    #[test]
    fn test_policy_wildcard_to() {
        let policy = AgentToAgentPolicy::from_allow_list(true, &["main -> *".to_string()]);
        assert!(policy.is_allowed("main", "work"));
        assert!(policy.is_allowed("main", "monitor"));
        assert!(!policy.is_allowed("work", "main"));
    }

    #[test]
    fn test_rule_parse_invalid() {
        assert!(A2ARule::parse("").is_none());
        assert!(A2ARule::parse("invalid").is_none());
        assert!(A2ARule::parse("a -> b -> c").is_none());
    }

    #[test]
    fn test_rule_matcher() {
        assert!(RuleMatcher::Any.matches("anything"));
        assert!(RuleMatcher::Specific("main".to_string()).matches("main"));
        assert!(RuleMatcher::Specific("main".to_string()).matches("MAIN"));
        assert!(!RuleMatcher::Specific("main".to_string()).matches("work"));
    }
}
```

**Step 3: Add sessions module to tools/mod.rs**

Find `core/src/tools/mod.rs` and add:

```rust
pub mod sessions;
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check 2>&1 | head -30`

**Step 5: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions::policy::tests 2>&1 | tail -20`

**Step 6: Commit**

```bash
git add core/src/tools/sessions/mod.rs core/src/tools/sessions/policy.rs core/src/tools/mod.rs
git commit -m "tools/sessions: add A2A policy engine with allow rules"
```

---

### Task 2: Add sandbox visibility control

**Files:**
- Create: `core/src/tools/sessions/visibility.rs`
- Modify: `core/src/tools/sessions/mod.rs`

**Step 1: Create `core/src/tools/sessions/visibility.rs`**

```rust
//! Sandbox session visibility control.
//!
//! Controls what sessions a sandboxed session can see and interact with.

use serde::{Deserialize, Serialize};

/// Session visibility policy for sandboxed sessions
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionToolsVisibility {
    /// Can see all sessions
    All,
    /// Can only see sessions spawned by self (default)
    #[default]
    Spawned,
    /// Cannot see any other sessions
    None,
}

/// Context for session visibility checks
#[derive(Debug, Clone)]
pub struct VisibilityContext {
    /// Current session's key
    pub requester_key: String,
    /// Is this a sandboxed session?
    pub sandboxed: bool,
    /// Visibility policy
    pub visibility: SessionToolsVisibility,
}

impl VisibilityContext {
    /// Create a new visibility context
    pub fn new(requester_key: impl Into<String>, sandboxed: bool, visibility: SessionToolsVisibility) -> Self {
        Self {
            requester_key: requester_key.into(),
            sandboxed,
            visibility,
        }
    }

    /// Create a non-sandboxed context (full access)
    pub fn full_access(requester_key: impl Into<String>) -> Self {
        Self {
            requester_key: requester_key.into(),
            sandboxed: false,
            visibility: SessionToolsVisibility::All,
        }
    }

    /// Check if requester can see target session
    pub fn can_see(&self, target_key: &str, spawned_by: Option<&str>) -> bool {
        // Non-sandboxed sessions can see everything
        if !self.sandboxed {
            return true;
        }

        match self.visibility {
            SessionToolsVisibility::All => true,
            SessionToolsVisibility::None => false,
            SessionToolsVisibility::Spawned => {
                // Can always see self
                if target_key == self.requester_key {
                    return true;
                }
                // Can see sessions spawned by self
                spawned_by
                    .map(|s| s == self.requester_key)
                    .unwrap_or(false)
            }
        }
    }

    /// Check if requester can send to target session
    pub fn can_send(&self, target_key: &str, spawned_by: Option<&str>) -> bool {
        self.can_see(target_key, spawned_by)
    }

    /// Check if requester can spawn sub-agents
    pub fn can_spawn(&self) -> bool {
        if !self.sandboxed {
            return true;
        }
        // Subagent sessions cannot spawn further subagents
        !is_subagent_session(&self.requester_key)
    }
}

/// Check if a session key is a subagent session
fn is_subagent_session(key: &str) -> bool {
    key.contains(":subagent:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_sandboxed_sees_all() {
        let ctx = VisibilityContext::full_access("agent:main:main");
        assert!(ctx.can_see("agent:work:main", None));
        assert!(ctx.can_see("agent:main:subagent:task1", None));
    }

    #[test]
    fn test_sandboxed_visibility_all() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::All);
        assert!(ctx.can_see("agent:work:main", None));
    }

    #[test]
    fn test_sandboxed_visibility_none() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::None);
        assert!(!ctx.can_see("agent:work:main", None));
        assert!(!ctx.can_see("agent:main:main", None)); // Even self is hidden
    }

    #[test]
    fn test_sandboxed_visibility_spawned_sees_self() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        assert!(ctx.can_see("agent:main:main", None));
    }

    #[test]
    fn test_sandboxed_visibility_spawned_sees_children() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        // Can see session spawned by self
        assert!(ctx.can_see("agent:main:subagent:task1", Some("agent:main:main")));
        // Cannot see session spawned by others
        assert!(!ctx.can_see("agent:work:subagent:task2", Some("agent:work:main")));
        // Cannot see session with no spawner
        assert!(!ctx.can_see("agent:work:main", None));
    }

    #[test]
    fn test_can_send_follows_can_see() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        assert!(ctx.can_send("agent:main:main", None));
        assert!(!ctx.can_send("agent:work:main", None));
    }

    #[test]
    fn test_non_sandboxed_can_spawn() {
        let ctx = VisibilityContext::full_access("agent:main:main");
        assert!(ctx.can_spawn());
    }

    #[test]
    fn test_sandboxed_main_can_spawn() {
        let ctx = VisibilityContext::new("agent:main:main", true, SessionToolsVisibility::Spawned);
        assert!(ctx.can_spawn());
    }

    #[test]
    fn test_subagent_cannot_spawn() {
        let ctx = VisibilityContext::new("agent:main:subagent:task1", true, SessionToolsVisibility::Spawned);
        assert!(!ctx.can_spawn());
    }

    #[test]
    fn test_is_subagent_session() {
        assert!(is_subagent_session("agent:main:subagent:task1"));
        assert!(is_subagent_session("agent:main:main:subagent:nested"));
        assert!(!is_subagent_session("agent:main:main"));
        assert!(!is_subagent_session("agent:work:peer:user123"));
    }
}
```

**Step 2: Update `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod policy;
pub mod visibility;

pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/tools/sessions/visibility.rs core/src/tools/sessions/mod.rs
git commit -m "tools/sessions: add sandbox visibility control"
```

---

### Task 3: Add session types and helpers

**Files:**
- Create: `core/src/tools/sessions/types.rs`
- Modify: `core/src/tools/sessions/mod.rs`

**Step 1: Create `core/src/tools/sessions/types.rs`**

```rust
//! Shared types for session tools.

use serde::{Deserialize, Serialize};

/// Session kind for filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Main,
    Dm,
    Group,
    Task,
    Subagent,
    Ephemeral,
}

impl SessionKind {
    /// Parse from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "main" => Some(Self::Main),
            "dm" | "direct" | "directmessage" => Some(Self::Dm),
            "group" | "channel" => Some(Self::Group),
            "task" | "cron" | "webhook" | "scheduled" => Some(Self::Task),
            "subagent" | "sub" => Some(Self::Subagent),
            "ephemeral" => Some(Self::Ephemeral),
            _ => None,
        }
    }

    /// Get kind from session key string
    pub fn from_session_key(key: &str) -> Self {
        let parts: Vec<&str> = key.split(':').collect();
        if parts.len() < 3 {
            return Self::Main;
        }

        match parts.get(2..) {
            Some(["main"]) | Some([]) => Self::Main,
            Some(["dm", ..]) => Self::Dm,
            Some([_, "dm", ..]) => Self::Dm,
            Some([_, "group", ..]) | Some([_, "channel", ..]) => Self::Group,
            Some(["cron", ..]) | Some(["webhook", ..]) | Some(["scheduled", ..]) => Self::Task,
            Some(["subagent", ..]) | Some([_, "subagent", ..]) => Self::Subagent,
            Some(["ephemeral", ..]) => Self::Ephemeral,
            _ => Self::Main,
        }
    }
}

/// A message in a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub timestamp: Option<i64>,
}

/// Session list row for sessions_list result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListRow {
    pub key: String,
    pub kind: SessionKind,
    pub agent_id: String,
    pub channel: Option<String>,
    pub label: Option<String>,
    pub updated_at: Option<i64>,
    pub model: Option<String>,
    pub messages: Option<Vec<SessionMessage>>,
    pub spawned_by: Option<String>,
}

/// Send message status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SendStatus {
    /// Message sent and reply received
    Ok,
    /// Message accepted (fire-and-forget)
    Accepted,
    /// Timeout waiting for reply
    Timeout,
    /// Permission denied
    Forbidden,
    /// Error occurred
    Error,
}

/// Spawn status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpawnStatus {
    /// Spawn accepted
    Accepted,
    /// Permission denied
    Forbidden,
    /// Error occurred
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_kind_parse() {
        assert_eq!(SessionKind::parse("main"), Some(SessionKind::Main));
        assert_eq!(SessionKind::parse("dm"), Some(SessionKind::Dm));
        assert_eq!(SessionKind::parse("direct"), Some(SessionKind::Dm));
        assert_eq!(SessionKind::parse("group"), Some(SessionKind::Group));
        assert_eq!(SessionKind::parse("cron"), Some(SessionKind::Task));
        assert_eq!(SessionKind::parse("subagent"), Some(SessionKind::Subagent));
        assert_eq!(SessionKind::parse("invalid"), None);
    }

    #[test]
    fn test_session_kind_from_key() {
        assert_eq!(SessionKind::from_session_key("agent:main:main"), SessionKind::Main);
        assert_eq!(SessionKind::from_session_key("agent:main:dm:user1"), SessionKind::Dm);
        assert_eq!(SessionKind::from_session_key("agent:main:telegram:dm:user1"), SessionKind::Dm);
        assert_eq!(SessionKind::from_session_key("agent:main:discord:group:guild1"), SessionKind::Group);
        assert_eq!(SessionKind::from_session_key("agent:main:cron:daily"), SessionKind::Task);
        assert_eq!(SessionKind::from_session_key("agent:main:subagent:task1"), SessionKind::Subagent);
        assert_eq!(SessionKind::from_session_key("agent:main:ephemeral:uuid"), SessionKind::Ephemeral);
    }
}
```

**Step 2: Update `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod policy;
pub mod types;
pub mod visibility;

pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
pub use types::{SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/tools/sessions/types.rs core/src/tools/sessions/mod.rs
git commit -m "tools/sessions: add session types and helpers"
```

---

### Task 4: Add sub-agent registry

**Files:**
- Create: `core/src/tools/sessions/registry.rs`
- Modify: `core/src/tools/sessions/mod.rs`

**Step 1: Create `core/src/tools/sessions/registry.rs`**

```rust
//! Sub-agent run registry.
//!
//! Tracks active sub-agent runs for cleanup and result announcement.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Information about a spawned sub-agent run
#[derive(Debug, Clone)]
pub struct SubagentRun {
    /// Unique run ID
    pub run_id: String,
    /// Child session key
    pub child_session_key: String,
    /// Parent (requester) session key
    pub requester_session_key: String,
    /// Task description
    pub task: String,
    /// Cleanup policy: "keep" or "delete"
    pub cleanup: String,
    /// Optional label
    pub label: Option<String>,
    /// Start timestamp
    pub started_at: i64,
}

/// Registry for tracking active sub-agent runs
#[derive(Debug, Clone, Default)]
pub struct SubagentRegistry {
    runs: Arc<RwLock<HashMap<String, SubagentRun>>>,
}

impl SubagentRegistry {
    /// Create a new registry
    pub fn new() -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new sub-agent run
    pub async fn register(&self, run: SubagentRun) {
        let mut runs = self.runs.write().await;
        runs.insert(run.run_id.clone(), run);
    }

    /// Get a run by ID
    pub async fn get(&self, run_id: &str) -> Option<SubagentRun> {
        let runs = self.runs.read().await;
        runs.get(run_id).cloned()
    }

    /// Remove and return a run by ID
    pub async fn remove(&self, run_id: &str) -> Option<SubagentRun> {
        let mut runs = self.runs.write().await;
        runs.remove(run_id)
    }

    /// List runs by requester session key
    pub async fn list_by_requester(&self, requester_key: &str) -> Vec<SubagentRun> {
        let runs = self.runs.read().await;
        runs.values()
            .filter(|r| r.requester_session_key == requester_key)
            .cloned()
            .collect()
    }

    /// List all active runs
    pub async fn list_all(&self) -> Vec<SubagentRun> {
        let runs = self.runs.read().await;
        runs.values().cloned().collect()
    }

    /// Get count of active runs
    pub async fn count(&self) -> usize {
        let runs = self.runs.read().await;
        runs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_run(run_id: &str, requester: &str) -> SubagentRun {
        SubagentRun {
            run_id: run_id.to_string(),
            child_session_key: format!("agent:main:subagent:{}", run_id),
            requester_session_key: requester.to_string(),
            task: "Test task".to_string(),
            cleanup: "keep".to_string(),
            label: None,
            started_at: 0,
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = SubagentRegistry::new();
        let run = test_run("run1", "agent:main:main");

        registry.register(run.clone()).await;

        let retrieved = registry.get("run1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().run_id, "run1");
    }

    #[tokio::test]
    async fn test_remove() {
        let registry = SubagentRegistry::new();
        registry.register(test_run("run1", "agent:main:main")).await;

        let removed = registry.remove("run1").await;
        assert!(removed.is_some());

        let retrieved = registry.get("run1").await;
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_list_by_requester() {
        let registry = SubagentRegistry::new();
        registry.register(test_run("run1", "agent:main:main")).await;
        registry.register(test_run("run2", "agent:main:main")).await;
        registry.register(test_run("run3", "agent:work:main")).await;

        let main_runs = registry.list_by_requester("agent:main:main").await;
        assert_eq!(main_runs.len(), 2);

        let work_runs = registry.list_by_requester("agent:work:main").await;
        assert_eq!(work_runs.len(), 1);
    }

    #[tokio::test]
    async fn test_count() {
        let registry = SubagentRegistry::new();
        assert_eq!(registry.count().await, 0);

        registry.register(test_run("run1", "agent:main:main")).await;
        assert_eq!(registry.count().await, 1);

        registry.register(test_run("run2", "agent:main:main")).await;
        assert_eq!(registry.count().await, 2);
    }
}
```

**Step 2: Update `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod policy;
pub mod registry;
pub mod types;
pub mod visibility;

pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
pub use registry::{SubagentRegistry, SubagentRun};
pub use types::{SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/tools/sessions/registry.rs core/src/tools/sessions/mod.rs
git commit -m "tools/sessions: add sub-agent run registry"
```

---

### Task 5: Implement sessions_list tool params and result

**Files:**
- Create: `core/src/tools/sessions/list.rs`
- Modify: `core/src/tools/sessions/mod.rs`

**Step 1: Create `core/src/tools/sessions/list.rs`**

```rust
//! sessions_list tool implementation.
//!
//! Lists visible sessions with filtering options.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::{SessionKind, SessionListRow};

/// Parameters for sessions_list tool
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SessionsListParams {
    /// Filter by session kinds: main, dm, group, task, subagent, ephemeral
    #[serde(default)]
    pub kinds: Option<Vec<String>>,

    /// Maximum sessions to return (default: 50)
    #[serde(default)]
    pub limit: Option<u32>,

    /// Only sessions active within N minutes
    #[serde(default)]
    pub active_minutes: Option<u32>,

    /// Include last N messages (0-20, default: 0)
    #[serde(default)]
    pub message_limit: Option<u32>,
}

impl SessionsListParams {
    /// Get limit with default
    pub fn get_limit(&self) -> u32 {
        self.limit.unwrap_or(50).min(200)
    }

    /// Get message limit with bounds
    pub fn get_message_limit(&self) -> u32 {
        self.message_limit.unwrap_or(0).min(20)
    }

    /// Parse kinds filter
    pub fn get_kinds(&self) -> Option<Vec<SessionKind>> {
        self.kinds.as_ref().map(|kinds| {
            kinds.iter().filter_map(|s| SessionKind::parse(s)).collect()
        })
    }
}

/// Result of sessions_list tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsListResult {
    /// Total count of matching sessions
    pub count: usize,
    /// Session list
    pub sessions: Vec<SessionListRow>,
}

impl SessionsListResult {
    /// Create an empty result
    pub fn empty() -> Self {
        Self {
            count: 0,
            sessions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_defaults() {
        let params = SessionsListParams::default();
        assert_eq!(params.get_limit(), 50);
        assert_eq!(params.get_message_limit(), 0);
        assert!(params.get_kinds().is_none());
    }

    #[test]
    fn test_params_limit_bounds() {
        let params = SessionsListParams {
            limit: Some(1000),
            ..Default::default()
        };
        assert_eq!(params.get_limit(), 200); // Capped at 200
    }

    #[test]
    fn test_params_message_limit_bounds() {
        let params = SessionsListParams {
            message_limit: Some(100),
            ..Default::default()
        };
        assert_eq!(params.get_message_limit(), 20); // Capped at 20
    }

    #[test]
    fn test_params_kinds_filter() {
        let params = SessionsListParams {
            kinds: Some(vec!["main".to_string(), "dm".to_string(), "invalid".to_string()]),
            ..Default::default()
        };
        let kinds = params.get_kinds().unwrap();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&SessionKind::Main));
        assert!(kinds.contains(&SessionKind::Dm));
    }
}
```

**Step 2: Update `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod list;
pub mod policy;
pub mod registry;
pub mod types;
pub mod visibility;

pub use list::{SessionsListParams, SessionsListResult};
pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
pub use registry::{SubagentRegistry, SubagentRun};
pub use types::{SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/tools/sessions/list.rs core/src/tools/sessions/mod.rs
git commit -m "tools/sessions: add sessions_list params and result types"
```

---

### Task 6: Implement sessions_send tool params and result

**Files:**
- Create: `core/src/tools/sessions/send.rs`
- Modify: `core/src/tools/sessions/mod.rs`

**Step 1: Create `core/src/tools/sessions/send.rs`**

```rust
//! sessions_send tool implementation.
//!
//! Sends a message to another session.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::SendStatus;

/// Parameters for sessions_send tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsSendParams {
    /// Target session key (mutually exclusive with label)
    #[serde(default)]
    pub session_key: Option<String>,

    /// Target session label (mutually exclusive with session_key)
    #[serde(default)]
    pub label: Option<String>,

    /// Agent ID for label lookup (optional, defaults to current agent)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Message to send
    pub message: String,

    /// Timeout in seconds (0 = fire-and-forget, default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 {
    30
}

impl SessionsSendParams {
    /// Validate params
    pub fn validate(&self) -> Result<(), String> {
        if self.session_key.is_some() && self.label.is_some() {
            return Err("Provide either session_key or label, not both".into());
        }
        if self.session_key.is_none() && self.label.is_none() {
            return Err("session_key or label is required".into());
        }
        if self.message.trim().is_empty() {
            return Err("message cannot be empty".into());
        }
        Ok(())
    }

    /// Check if this is a fire-and-forget request
    pub fn is_fire_and_forget(&self) -> bool {
        self.timeout_seconds == 0
    }
}

/// Result of sessions_send tool
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionsSendResult {
    /// Send status
    pub status: SendStatus,
    /// Run ID for tracking
    pub run_id: Option<String>,
    /// Resolved session key
    pub session_key: Option<String>,
    /// Reply from target session (if waited)
    pub reply: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
}

impl Default for SendStatus {
    fn default() -> Self {
        Self::Error
    }
}

impl SessionsSendResult {
    /// Create success result with reply
    pub fn ok(run_id: String, session_key: String, reply: Option<String>) -> Self {
        Self {
            status: SendStatus::Ok,
            run_id: Some(run_id),
            session_key: Some(session_key),
            reply,
            error: None,
        }
    }

    /// Create accepted result (fire-and-forget)
    pub fn accepted(run_id: String, session_key: String) -> Self {
        Self {
            status: SendStatus::Accepted,
            run_id: Some(run_id),
            session_key: Some(session_key),
            reply: None,
            error: None,
        }
    }

    /// Create forbidden result
    pub fn forbidden(error: impl Into<String>) -> Self {
        Self {
            status: SendStatus::Forbidden,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Create error result
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            status: SendStatus::Error,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Create timeout result
    pub fn timeout(run_id: String, session_key: String) -> Self {
        Self {
            status: SendStatus::Timeout,
            run_id: Some(run_id),
            session_key: Some(session_key),
            reply: None,
            error: Some("Timeout waiting for reply".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_validation() {
        // Valid with session_key
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: None,
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_ok());

        // Valid with label
        let params = SessionsSendParams {
            session_key: None,
            label: Some("my-session".into()),
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_ok());

        // Invalid: both session_key and label
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: Some("my-session".into()),
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_err());

        // Invalid: neither session_key nor label
        let params = SessionsSendParams {
            session_key: None,
            label: None,
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_err());

        // Invalid: empty message
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: None,
            agent_id: None,
            message: "   ".into(),
            timeout_seconds: 30,
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_fire_and_forget() {
        let params = SessionsSendParams {
            session_key: Some("agent:main:main".into()),
            label: None,
            agent_id: None,
            message: "Hello".into(),
            timeout_seconds: 0,
        };
        assert!(params.is_fire_and_forget());
    }

    #[test]
    fn test_result_constructors() {
        let ok = SessionsSendResult::ok("run1".into(), "key1".into(), Some("reply".into()));
        assert_eq!(ok.status, SendStatus::Ok);
        assert!(ok.reply.is_some());

        let accepted = SessionsSendResult::accepted("run2".into(), "key2".into());
        assert_eq!(accepted.status, SendStatus::Accepted);

        let forbidden = SessionsSendResult::forbidden("Not allowed");
        assert_eq!(forbidden.status, SendStatus::Forbidden);
        assert!(forbidden.error.is_some());
    }
}
```

**Step 2: Update `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod list;
pub mod policy;
pub mod registry;
pub mod send;
pub mod types;
pub mod visibility;

pub use list::{SessionsListParams, SessionsListResult};
pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
pub use registry::{SubagentRegistry, SubagentRun};
pub use send::{SessionsSendParams, SessionsSendResult};
pub use types::{SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/tools/sessions/send.rs core/src/tools/sessions/mod.rs
git commit -m "tools/sessions: add sessions_send params and result types"
```

---

### Task 7: Implement sessions_spawn tool params and result

**Files:**
- Create: `core/src/tools/sessions/spawn.rs`
- Modify: `core/src/tools/sessions/mod.rs`

**Step 1: Create `core/src/tools/sessions/spawn.rs`**

```rust
//! sessions_spawn tool implementation.
//!
//! Spawns a sub-agent to execute a task.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::SpawnStatus;

/// Parameters for sessions_spawn tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsSpawnParams {
    /// Task description for the sub-agent
    pub task: String,

    /// Optional label for the child session
    #[serde(default)]
    pub label: Option<String>,

    /// Target agent ID (defaults to current agent)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Model override for child session
    #[serde(default)]
    pub model: Option<String>,

    /// Thinking level override (off, minimal, low, medium, high, xhigh)
    #[serde(default)]
    pub thinking: Option<String>,

    /// Run timeout in seconds (0 = no timeout)
    #[serde(default)]
    pub run_timeout_seconds: Option<u32>,

    /// Cleanup policy: "keep" or "delete" (default: "keep")
    #[serde(default = "default_cleanup")]
    pub cleanup: String,
}

fn default_cleanup() -> String {
    "keep".to_string()
}

impl SessionsSpawnParams {
    /// Validate params
    pub fn validate(&self) -> Result<(), String> {
        if self.task.trim().is_empty() {
            return Err("task cannot be empty".into());
        }
        if !["keep", "delete"].contains(&self.cleanup.as_str()) {
            return Err("cleanup must be 'keep' or 'delete'".into());
        }
        Ok(())
    }

    /// Check if cleanup is delete
    pub fn should_delete(&self) -> bool {
        self.cleanup == "delete"
    }
}

/// Result of sessions_spawn tool
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionsSpawnResult {
    /// Spawn status
    pub status: SpawnStatus,
    /// Run ID for tracking
    pub run_id: Option<String>,
    /// Child session key
    pub child_session_key: Option<String>,
    /// Error message if failed
    pub error: Option<String>,
}

impl Default for SpawnStatus {
    fn default() -> Self {
        Self::Error
    }
}

impl SessionsSpawnResult {
    /// Create accepted result
    pub fn accepted(run_id: String, child_session_key: String) -> Self {
        Self {
            status: SpawnStatus::Accepted,
            run_id: Some(run_id),
            child_session_key: Some(child_session_key),
            error: None,
        }
    }

    /// Create forbidden result
    pub fn forbidden(error: impl Into<String>) -> Self {
        Self {
            status: SpawnStatus::Forbidden,
            error: Some(error.into()),
            ..Default::default()
        }
    }

    /// Create error result
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            status: SpawnStatus::Error,
            error: Some(error.into()),
            ..Default::default()
        }
    }
}

/// Build system prompt for sub-agent
pub fn build_subagent_system_prompt(
    requester_key: &str,
    child_key: &str,
    label: Option<&str>,
    task: &str,
) -> String {
    let label_info = label
        .map(|l| format!("\nSession label: {}", l))
        .unwrap_or_default();

    format!(
        r#"You are a sub-agent spawned to execute a specific task.

Spawned by: {}
Your session: {}{}

## Task

{}

## Guidelines

1. Focus exclusively on the task above
2. Complete the task and report results
3. You may use available tools to accomplish the task
4. Do not spawn further sub-agents
5. Keep responses concise and focused

When complete, summarize what was accomplished."#,
        requester_key, child_key, label_info, task
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_params_validation() {
        // Valid
        let params = SessionsSpawnParams {
            task: "Do something".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "keep".into(),
        };
        assert!(params.validate().is_ok());

        // Invalid: empty task
        let params = SessionsSpawnParams {
            task: "   ".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "keep".into(),
        };
        assert!(params.validate().is_err());

        // Invalid: bad cleanup
        let params = SessionsSpawnParams {
            task: "Do something".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "invalid".into(),
        };
        assert!(params.validate().is_err());
    }

    #[test]
    fn test_should_delete() {
        let keep = SessionsSpawnParams {
            task: "task".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "keep".into(),
        };
        assert!(!keep.should_delete());

        let delete = SessionsSpawnParams {
            task: "task".into(),
            label: None,
            agent_id: None,
            model: None,
            thinking: None,
            run_timeout_seconds: None,
            cleanup: "delete".into(),
        };
        assert!(delete.should_delete());
    }

    #[test]
    fn test_result_constructors() {
        let accepted = SessionsSpawnResult::accepted("run1".into(), "child1".into());
        assert_eq!(accepted.status, SpawnStatus::Accepted);
        assert!(accepted.run_id.is_some());
        assert!(accepted.child_session_key.is_some());

        let forbidden = SessionsSpawnResult::forbidden("Not allowed");
        assert_eq!(forbidden.status, SpawnStatus::Forbidden);
        assert!(forbidden.error.is_some());
    }

    #[test]
    fn test_build_subagent_system_prompt() {
        let prompt = build_subagent_system_prompt(
            "agent:main:main",
            "agent:main:subagent:task1",
            Some("research"),
            "Find information about Rust async",
        );
        assert!(prompt.contains("agent:main:main"));
        assert!(prompt.contains("agent:main:subagent:task1"));
        assert!(prompt.contains("research"));
        assert!(prompt.contains("Find information about Rust async"));
    }
}
```

**Step 2: Update `core/src/tools/sessions/mod.rs`**

```rust
//! Inter-session communication tools.
//!
//! Provides tools for agent-to-agent communication:
//! - `sessions_list`: List visible sessions
//! - `sessions_send`: Send message to another session
//! - `sessions_spawn`: Spawn a sub-agent task

pub mod list;
pub mod policy;
pub mod registry;
pub mod send;
pub mod spawn;
pub mod types;
pub mod visibility;

pub use list::{SessionsListParams, SessionsListResult};
pub use policy::{AgentToAgentPolicy, A2ARule, RuleMatcher};
pub use registry::{SubagentRegistry, SubagentRun};
pub use send::{SessionsSendParams, SessionsSendResult};
pub use spawn::{build_subagent_system_prompt, SessionsSpawnParams, SessionsSpawnResult};
pub use types::{SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus};
pub use visibility::{SessionToolsVisibility, VisibilityContext};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/tools/sessions/spawn.rs core/src/tools/sessions/mod.rs
git commit -m "tools/sessions: add sessions_spawn params and result types"
```

---

### Task 8: Full test pass and module exports

**Files:**
- Modify: `core/src/lib.rs` (add exports)

**Step 1: Run full test suite for sessions module**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo test --lib tools::sessions 2>&1 | tail -40`
Expected: All tests PASS

**Step 2: Add exports to lib.rs**

Find the tools exports section in `core/src/lib.rs` and add:

```rust
// Session tools exports (A2A communication)
pub use crate::tools::sessions::{
    // Policy
    AgentToAgentPolicy, A2ARule, RuleMatcher,
    // Visibility
    SessionToolsVisibility, VisibilityContext,
    // Types
    SendStatus, SessionKind, SessionListRow, SessionMessage, SpawnStatus,
    // Registry
    SubagentRegistry, SubagentRun,
    // Tool params/results
    SessionsListParams, SessionsListResult,
    SessionsSendParams, SessionsSendResult,
    SessionsSpawnParams, SessionsSpawnResult,
    build_subagent_system_prompt,
};
```

**Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph/core && cargo check 2>&1 | tail -20`

**Step 4: Final commit**

```bash
git add core/src/lib.rs
git commit -m "tools/sessions: export A2A communication types from lib.rs"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | A2A policy engine | `tools/sessions/mod.rs`, `policy.rs` |
| 2 | Sandbox visibility | `visibility.rs` |
| 3 | Session types/helpers | `types.rs` |
| 4 | Sub-agent registry | `registry.rs` |
| 5 | sessions_list params/result | `list.rs` |
| 6 | sessions_send params/result | `send.rs` |
| 7 | sessions_spawn params/result | `spawn.rs` |
| 8 | Full test + exports | `lib.rs` |

**Note:** This plan creates the types, policy, and infrastructure. The actual tool execution logic (connecting to gateway, running agents) will be implemented when the gateway protocol is ready.
