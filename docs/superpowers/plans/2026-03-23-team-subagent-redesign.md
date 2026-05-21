# Team Sub-Agent Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace named Agent creation in team_create/team_launch with keep_alive Sub-Agent spawning, eliminating AgentRegistry pollution.

**Architecture:** Extend `SubAgentRun` with `persona` and `keep_alive` fields, add `Idle` state to `RunStatus`, modify team tools to spawn sub-agents instead of creating named agents, and add sub-agent cleanup to `team_disband`.

**Tech Stack:** Rust, async_trait, serde, schemars, tokio, tracing

---

### Task 1: Add `Idle` status and `persona`/`keep_alive` fields to SubAgentRun

**Files:**
- Modify: `src/agents/sub_agents/run.rs`

- [ ] **Step 1: Write failing tests for Idle status transitions**

Add these tests to the existing `tests` module in `run.rs`:

```rust
#[test]
fn test_idle_status_transitions() {
    // Running -> Idle (keep_alive completion)
    assert!(RunStatus::Running.can_transition_to(&RunStatus::Idle));
    // Idle -> Running (steer re-entry)
    assert!(RunStatus::Idle.can_transition_to(&RunStatus::Running));
    // Idle -> Completed (final completion)
    assert!(RunStatus::Idle.can_transition_to(&RunStatus::Completed));
    // Idle -> Cancelled (disband)
    assert!(RunStatus::Idle.can_transition_to(&RunStatus::Cancelled));
    // Idle is NOT terminal
    assert!(!RunStatus::Idle.is_terminal());
    // Invalid: Idle -> Failed, Idle -> Pending, Idle -> Paused
    assert!(!RunStatus::Idle.can_transition_to(&RunStatus::Failed));
    assert!(!RunStatus::Idle.can_transition_to(&RunStatus::Pending));
    assert!(!RunStatus::Idle.can_transition_to(&RunStatus::Paused));
}

#[test]
fn test_idle_serialization() {
    let status = RunStatus::Idle;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"idle\"");
    let deserialized: RunStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, RunStatus::Idle);
}

#[test]
fn test_subagent_run_persona_and_keep_alive() {
    let parent_key = SessionKey::main("main");
    let session_key = SessionKey::Subagent {
        parent_key: Box::new(parent_key.clone()),
        subagent_id: "persona-test".to_string(),
    };

    let run = SubAgentRun::new(session_key, parent_key, "Test", "explore")
        .with_persona("You are a code reviewer".to_string())
        .with_keep_alive(true);

    assert_eq!(run.persona, Some("You are a code reviewer".to_string()));
    assert!(run.keep_alive);

    // Default values
    let run2 = SubAgentRun::new(
        SessionKey::main("s"),
        SessionKey::main("p"),
        "T",
        "e",
    );
    assert_eq!(run2.persona, None);
    assert!(!run2.keep_alive);
}

#[test]
fn test_persona_not_serialized() {
    let run = SubAgentRun::new(
        SessionKey::main("s"),
        SessionKey::main("p"),
        "Task",
        "explore",
    )
    .with_persona("Secret persona".to_string());

    let json = serde_json::to_string(&run).unwrap();
    assert!(!json.contains("Secret persona"));
    assert!(!json.contains("persona"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib agents::sub_agents::run::tests -- --nocapture 2>&1 | head -40`
Expected: compilation errors (Idle variant, persona/keep_alive fields don't exist yet)

- [ ] **Step 3: Add `Idle` variant to `RunStatus` enum**

In `run.rs`, add `Idle` to the `RunStatus` enum (after `Paused`):

```rust
/// Run is idle, waiting for next steer (keep_alive mode)
Idle,
```

- [ ] **Step 4: Update `can_transition_to()` with Idle transitions**

Add these arms to the `match` in `can_transition_to()`, **before** the existing `_ => false` catchall (keep `_ => false` as-is):

```rust
// Running can also go to Idle (keep_alive completion)
(RunStatus::Running, RunStatus::Idle) => true,

// Idle can resume (Running), complete (Completed), or be cancelled
(RunStatus::Idle, RunStatus::Running) => true,
(RunStatus::Idle, RunStatus::Completed) => true,
(RunStatus::Idle, RunStatus::Cancelled) => true,
```

The existing `_ => false` catchall will correctly handle all invalid Idle transitions (Idle→Failed, Idle→Pending, Idle→Paused). Do NOT remove it.

- [ ] **Step 5: Add `persona` and `keep_alive` fields to `SubAgentRun`**

Add these fields to `SubAgentRun` struct (after `cleanup_policy`):

```rust
/// Persona prompt for team sub-agents (injected as system prompt prefix)
/// Skipped during serialization to avoid leaking into MemoryFact persistence.
#[serde(skip)]
pub persona: Option<String>,
/// When true, the sub-agent stays alive (Idle) after completing a task,
/// waiting for the next steer. Used by team members.
#[serde(default)]
pub keep_alive: bool,
```

Initialize in `SubAgentRun::new()`:
```rust
persona: None,
keep_alive: false,
```

Add builder methods:
```rust
/// Set the persona prompt
pub fn with_persona(mut self, persona: String) -> Self {
    self.persona = Some(persona);
    self
}

/// Set keep-alive mode
pub fn with_keep_alive(mut self, keep_alive: bool) -> Self {
    self.keep_alive = keep_alive;
    self
}
```

- [ ] **Step 6: Update existing Idle-related test assertions**

The existing `test_run_status_is_terminal` test needs to add: `assert!(!RunStatus::Idle.is_terminal());`

The existing `test_run_status_transitions` test needs to add Idle transition assertions.

- [ ] **Step 7: Run all tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::sub_agents::run::tests -- --nocapture`
Expected: all tests PASS

- [ ] **Step 8: Commit**

```bash
git add src/agents/sub_agents/run.rs
git commit -m "agents: add Idle status and persona/keep_alive fields to SubAgentRun"
```

---

### Task 2: Update SubAgentRegistry for Idle status

**Files:**
- Modify: `src/agents/sub_agents/registry.rs`

- [ ] **Step 1: Write failing tests for Idle in registry**

Add to `tests` module in `registry.rs`:

```rust
#[tokio::test]
async fn test_idle_status_in_stats() {
    let registry = SubAgentRegistry::new_in_memory();
    let run = SubAgentRun::new(
        make_subagent_key("p1", "s1"),
        make_session_key("p1"),
        "Task",
        "explore",
    ).with_keep_alive(true);
    let run_id = run.run_id.clone();
    registry.register(run).await.unwrap();

    registry.transition(&run_id, RunStatus::Running).await.unwrap();
    registry.transition(&run_id, RunStatus::Idle).await.unwrap();

    let stats = registry.stats().await;
    assert_eq!(stats.idle, 1);
    assert_eq!(stats.running, 0);

    // Idle runs are active (non-terminal)
    let active = registry.get_active_runs().await;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].status, RunStatus::Idle);
}

#[tokio::test]
async fn test_idle_to_running_reentry() {
    let registry = SubAgentRegistry::new_in_memory();
    let run = SubAgentRun::new(
        make_subagent_key("p1", "s1"),
        make_session_key("p1"),
        "Task",
        "explore",
    ).with_keep_alive(true);
    let run_id = run.run_id.clone();
    registry.register(run).await.unwrap();

    // Pending -> Running -> Idle -> Running -> Idle -> Completed
    registry.transition(&run_id, RunStatus::Running).await.unwrap();
    registry.transition(&run_id, RunStatus::Idle).await.unwrap();
    registry.transition(&run_id, RunStatus::Running).await.unwrap();
    registry.transition(&run_id, RunStatus::Idle).await.unwrap();
    registry.transition(&run_id, RunStatus::Completed).await.unwrap();

    let run = registry.get(&run_id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Completed);
    assert!(run.ended_at.is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib agents::sub_agents::registry::tests -- --nocapture 2>&1 | head -20`
Expected: compilation errors (`idle` field missing from `RegistryStats`)

- [ ] **Step 3: Add `idle` field to `RegistryStats`**

```rust
pub struct RegistryStats {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub paused: usize,
    pub idle: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
}
```

- [ ] **Step 4: Update `stats()` method to count Idle**

Add `RunStatus::Idle => stats.idle += 1,` to the match in `stats()`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::sub_agents::registry::tests -- --nocapture`
Expected: all PASS

- [ ] **Step 6: Commit**

```bash
git add src/agents/sub_agents/registry.rs
git commit -m "agents: update SubAgentRegistry stats for Idle status"
```

---

### Task 3: Update persistence to handle persona skip

**Files:**
- Modify: `src/agents/sub_agents/persistence.rs`

- [ ] **Step 1: Write test verifying persona is excluded from serialization**

Add to `tests` module in `persistence.rs`:

```rust
#[test]
fn test_persona_excluded_from_fact() {
    let run = SubAgentRun::new(
        SessionKey::main("s1"),
        SessionKey::main("p1"),
        "Test task",
        "explore",
    )
    .with_persona("Secret persona content".to_string())
    .with_keep_alive(true);

    let fact = SubAgentRunFact::from_run(&run);
    // persona should NOT appear in serialized content
    assert!(!fact.content.contains("Secret persona"));
    // keep_alive should appear (it's not skipped)
    assert!(fact.content.contains("keep_alive"));
}

#[test]
fn test_roundtrip_without_persona() {
    let run = SubAgentRun::new(
        SessionKey::main("s1"),
        SessionKey::main("p1"),
        "Test task",
        "explore",
    )
    .with_persona("My persona".to_string())
    .with_keep_alive(true);

    let fact = SubAgentRunFact::from_run(&run);
    let restored = SubAgentRunFact::to_run(&fact).unwrap();

    // persona is lost after roundtrip (expected — #[serde(skip)])
    assert_eq!(restored.persona, None);
    // keep_alive survives roundtrip
    assert!(restored.keep_alive);
}
```

- [ ] **Step 2: Run tests to verify they pass** (should already pass since `#[serde(skip)]` was added in Task 1)

Run: `cargo test -p alephcore --lib agents::sub_agents::persistence::tests -- --nocapture`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/agents/sub_agents/persistence.rs
git commit -m "agents: add tests verifying persona exclusion from persistence"
```

---

### Task 4: Add `persona` and `keep_alive` to SessionsSpawnArgs

**Files:**
- Modify: `src/builtin_tools/sessions/spawn_tool.rs`

- [ ] **Step 1: Write failing test for new args**

Add to `tests` module in `spawn_tool.rs`:

```rust
#[test]
fn test_args_with_persona_and_keep_alive() {
    let args: SessionsSpawnArgs = serde_json::from_str(
        r#"{
            "task": "Review code",
            "persona": "You are a senior code reviewer with expertise in Rust.",
            "keep_alive": true
        }"#,
    )
    .unwrap();

    assert_eq!(args.task, "Review code");
    assert_eq!(
        args.persona,
        Some("You are a senior code reviewer with expertise in Rust.".to_string())
    );
    assert!(args.keep_alive);
}

#[test]
fn test_args_persona_defaults_to_none() {
    let args: SessionsSpawnArgs =
        serde_json::from_str(r#"{"task": "Do something"}"#).unwrap();
    assert_eq!(args.persona, None);
    assert!(!args.keep_alive);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib builtin_tools::sessions::spawn_tool::tests -- --nocapture 2>&1 | head -20`
Expected: compilation errors (persona/keep_alive fields don't exist)

- [ ] **Step 3: Add `persona` and `keep_alive` fields to `SessionsSpawnArgs`**

Add after the `cleanup` field:

```rust
/// Optional persona prompt for the sub-agent
///
/// When provided, this text is prepended to the sub-agent's system prompt,
/// giving it a distinct identity. Used by team members.
#[serde(default)]
pub persona: Option<String>,

/// Keep the sub-agent alive after task completion
///
/// When true, the sub-agent enters Idle state instead of Completed,
/// allowing subsequent steer commands. Used by team members.
#[serde(default)]
pub keep_alive: bool,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib builtin_tools::sessions::spawn_tool::tests -- --nocapture`
Expected: all PASS

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/sessions/spawn_tool.rs
git commit -m "tools: add persona and keep_alive params to sessions_spawn"
```

---

### Task 5: Update `TeamMember` to use `run_id` instead of `agent_id`

**Files:**
- Modify: `src/agents/swarm/tasks/mod.rs`

- [ ] **Step 1: Update `TeamMember` struct**

Change `TeamMember` from:
```rust
pub struct TeamMember {
    pub agent_id: AgentId,
    pub role: String,
    pub joined_at: u64,
}
```
to:
```rust
pub struct TeamMember {
    pub agent_id: AgentId,
    pub role: String,
    pub joined_at: u64,
    /// Sub-agent run ID (for team members spawned as sub-agents)
    #[serde(default)]
    pub run_id: Option<String>,
    /// Persona prompt used for this team member
    #[serde(default)]
    pub persona: Option<String>,
}
```

Note: We keep `agent_id` for backward compat with SQLite store and template-based members. The `run_id` field is the new sub-agent tracking handle.

- [ ] **Step 2: Fix all places that construct `TeamMember`**

There are multiple construction sites that will break. Fix all of them by adding `run_id: None, persona: None`:

**`src/builtin_tools/team_manage/launch.rs`** (~line 252):
```rust
let tm = TeamMember {
    agent_id: member.name.clone(),
    role: member.role.clone(),
    joined_at: now,
    run_id: None,
    persona: None,
};
```

**`src/agents/swarm/tasks/store.rs`** (~lines 141, 894, 905, 966):

Line ~141 (in the SQLite query mapping):
```rust
Ok(TeamMember {
    agent_id: row.get(0)?,
    role: row.get(1)?,
    joined_at: row.get(2)?,
    run_id: None,
    persona: None,
})
```

Lines ~894, ~905, ~966 (in test code) — add `run_id: None, persona: None` to each `TeamMember { ... }` literal.

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore`
Expected: success

- [ ] **Step 4: Commit**

```bash
git add src/agents/swarm/tasks/mod.rs src/agents/swarm/tasks/store.rs src/builtin_tools/team_manage/launch.rs
git commit -m "agents: add run_id and persona fields to TeamMember"
```

---

### Task 6: Wire `SubAgentRegistry` into `BuiltinToolConfig` and builder

**Files:**
- Modify: `src/executor/builtin_registry/config.rs`
- Modify: `src/executor/builtin_registry/builder.rs`

The team tools need `SubAgentRegistry` and session context. These are constructed in `builder.rs` via `BuiltinToolConfig`. We must add the necessary fields.

- [ ] **Step 1: Add fields to `BuiltinToolConfig`**

In `config.rs`, add after the `agent_message_bus` field:

```rust
/// Sub-agent registry for team management tools
pub sub_agent_registry: Option<Arc<crate::agents::sub_agents::registry::SubAgentRegistry>>,
/// Current agent ID for team tools (leader identity)
pub current_agent_id: Option<String>,
/// Current session key for team tools (parent session for spawned sub-agents)
pub current_session_key: Option<crate::routing::SessionKey>,
```

- [ ] **Step 2: Update builder.rs to pass new deps to team tools**

In `builder.rs` (~lines 314-317), update the team tool construction:

```rust
let team_create = TeamCreateTool::new(
    Arc::clone(store),
    config.sub_agent_registry.as_ref().map(Arc::clone).unwrap_or_else(|| Arc::new(SubAgentRegistry::new_in_memory())),
    config.current_agent_id.clone().unwrap_or_else(|| "main".to_string()),
    config.current_session_key.clone().unwrap_or_else(|| SessionKey::main("main")),
);
let team_launch = TeamLaunchTool::new(
    Arc::clone(store),
    config.sub_agent_registry.as_ref().map(Arc::clone).unwrap_or_else(|| Arc::new(SubAgentRegistry::new_in_memory())),
    config.current_agent_id.clone().unwrap_or_else(|| "main".to_string()),
    config.current_session_key.clone().unwrap_or_else(|| SessionKey::main("main")),
);
let team_list = TeamListTool::new(Arc::clone(store));
let team_disband = TeamDisbandTool::new(
    Arc::clone(store),
    config.sub_agent_registry.as_ref().map(Arc::clone).unwrap_or_else(|| Arc::new(SubAgentRegistry::new_in_memory())),
);
```

Add the necessary imports at the top of `builder.rs`:
```rust
use crate::agents::sub_agents::registry::SubAgentRegistry;
use crate::routing::SessionKey;
```

- [ ] **Step 3: Find where `BuiltinToolConfig` is populated and pass the new fields**

Run: `rg "BuiltinToolConfig" src/ --files-with-matches` to find all construction sites. For each, add:
```rust
sub_agent_registry: Some(Arc::clone(&sub_agent_registry)),
current_agent_id: Some(agent_id.clone()),
current_session_key: Some(session_key.clone()),
```

Since these are `Option` fields with `Default`, existing call sites that don't set them will use `None` and the builder will fall back to in-memory defaults.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: success

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "executor: wire SubAgentRegistry into BuiltinToolConfig for team tools"
```

---

### Task 7: Rewrite `TeamCreateTool` to spawn sub-agents (uses Task 6 wiring)

**Files:**
- Modify: `src/builtin_tools/team_manage/create.rs`

- [ ] **Step 1: Update `TeamCreateArgs` to accept members with persona**

Replace the current `TeamCreateArgs`:

```rust
/// Arguments for creating a coordination team.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamCreateArgs {
    /// Human-readable team name
    pub name: String,
    /// Description of the team's purpose
    #[serde(default)]
    pub description: Option<String>,
    /// Agent ID of the team leader (defaults to current agent)
    #[serde(default)]
    pub leader: Option<String>,
    /// Team members to spawn as sub-agents
    #[serde(default)]
    pub members: Vec<TeamMemberSpec>,
}

/// Specification for a team member to be spawned
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamMemberSpec {
    /// Role name for this member (e.g., "code-reviewer", "tester")
    pub role: String,
    /// Persona prompt — defines the member's identity and expertise
    pub persona: String,
}
```

- [ ] **Step 2: Update `TeamCreateOutput` to include spawned run_ids**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct TeamCreateOutput {
    pub team_id: String,
    pub name: String,
    pub leader: String,
    pub members: Vec<SpawnedMember>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpawnedMember {
    pub role: String,
    pub run_id: String,
}
```

- [ ] **Step 3: Update `TeamCreateTool` struct to hold SubAgentRegistry**

```rust
use crate::agents::sub_agents::registry::SubAgentRegistry;
use crate::agents::sub_agents::run::SubAgentRun;
use crate::routing::SessionKey;

#[derive(Clone)]
pub struct TeamCreateTool {
    store: Arc<dyn CoordTaskStore>,
    sub_registry: Arc<SubAgentRegistry>,
    current_agent_id: String,
    current_session_key: SessionKey,
}

impl TeamCreateTool {
    pub fn new(
        store: Arc<dyn CoordTaskStore>,
        sub_registry: Arc<SubAgentRegistry>,
        current_agent_id: String,
        current_session_key: SessionKey,
    ) -> Self {
        Self { store, sub_registry, current_agent_id, current_session_key }
    }
}
```

- [ ] **Step 4: Rewrite `call()` to spawn sub-agents instead of creating named agents**

The new `call()` implementation:
1. Create team in CoordTaskStore (unchanged)
2. For each member in `args.members`:
   a. Create a `SubAgentRun` with `.with_persona(member.persona)` and `.with_keep_alive(true)`
   b. Register in `sub_registry`
   c. Add `TeamMember` with `run_id: Some(run_id)` to store
3. Return `TeamCreateOutput` with spawned member info

```rust
async fn call(&self, args: Self::Args) -> Result<Self::Output> {
    let team_id = generate_team_id(&args.name);
    let leader = args.leader.unwrap_or_else(|| self.current_agent_id.clone());

    // Enforce member limit
    if args.members.len() > 8 {
        return Err(AlephError::Other {
            message: "Team cannot have more than 8 members".to_string(),
            suggestion: None,
        });
    }

    let new_team = NewTeam {
        id: team_id.clone(),
        name: args.name.clone(),
        description: args.description.unwrap_or_default(),
        leader: leader.clone(),
    };

    let team = self.store.create_team(new_team).await?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut spawned_members = Vec::new();

    for member_spec in &args.members {
        // Create sub-agent run with persona
        let subagent_id = format!("team-{}-{}", team_id, member_spec.role);
        let session_key = SessionKey::Subagent {
            parent_key: Box::new(self.current_session_key.clone()),
            subagent_id: subagent_id.clone(),
        };

        let run = SubAgentRun::new(
            session_key,
            self.current_session_key.clone(),
            format!("Team member: {}", member_spec.role),
            "team",
        )
        .with_persona(member_spec.persona.clone())
        .with_keep_alive(true)
        .with_label(format!("{}/{}", team_id, member_spec.role));

        let run_id = self.sub_registry.register(run).await?;

        // Add to team in store
        let tm = TeamMember {
            agent_id: subagent_id.clone(),
            role: member_spec.role.clone(),
            joined_at: now,
            run_id: Some(run_id.clone()),
            persona: Some(member_spec.persona.clone()),
        };
        self.store.add_member(&team_id, tm).await?;

        spawned_members.push(SpawnedMember {
            role: member_spec.role.clone(),
            run_id,
        });
    }

    info!(
        team_id = %team.id,
        name = %team.name,
        members = spawned_members.len(),
        "Team created with sub-agent members"
    );

    Ok(TeamCreateOutput {
        message: format!(
            "Team '{}' created (id: {}) with {} sub-agent members",
            team.name, team.id, spawned_members.len()
        ),
        team_id: team.id,
        name: team.name,
        leader,
        members: spawned_members,
    })
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Expected: may have errors at construction sites of `TeamCreateTool` — find and fix them.

- [ ] **Step 6: Find and update all `TeamCreateTool::new()` call sites**

Search for `TeamCreateTool::new(` and update to pass the new parameters.

Run: `rg "TeamCreateTool::new\(" src/ --files-with-matches`

Update each call site to pass `sub_registry`, `current_agent_id`, and `current_session_key`.

- [ ] **Step 7: Verify compilation passes**

Run: `cargo check -p alephcore`
Expected: success

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "tools: rewrite team_create to spawn sub-agents instead of named agents"
```

---

### Task 8: Update `TeamLaunchTool` to spawn sub-agents

**Files:**
- Modify: `src/builtin_tools/team_manage/launch.rs`

- [ ] **Step 1: Update `TeamLaunchTool` struct**

Same pattern as Task 6 — add `sub_registry`, `current_agent_id`, `current_session_key` fields.

```rust
#[derive(Clone)]
pub struct TeamLaunchTool {
    store: Arc<dyn CoordTaskStore>,
    sub_registry: Arc<SubAgentRegistry>,
    current_agent_id: String,
    current_session_key: SessionKey,
}
```

- [ ] **Step 2: Rewrite member registration in `call()`**

Replace the member registration loop (lines ~250-259) to spawn sub-agents:

```rust
// 4. Spawn sub-agent members
let now = now_secs();
for member in &tmpl.members {
    let subagent_id = format!("team-{}-{}", team_id, member.name);
    let session_key = SessionKey::Subagent {
        parent_key: Box::new(self.current_session_key.clone()),
        subagent_id: subagent_id.clone(),
    };

    let persona = member.description.clone().unwrap_or_else(|| {
        format!("You are {} with role: {}", member.name, member.role)
    });

    let run = SubAgentRun::new(
        session_key,
        self.current_session_key.clone(),
        format!("Team member: {}", member.name),
        "team",
    )
    .with_persona(persona.clone())
    .with_keep_alive(true)
    .with_label(format!("{}/{}", team_id, member.name));

    let run_id = self.sub_registry.register(run).await?;

    let tm = TeamMember {
        agent_id: subagent_id,
        role: member.role.clone(),
        joined_at: now,
        run_id: Some(run_id),
        persona: Some(persona),
    };
    if let Err(e) = self.store.add_member(&team_id, tm).await {
        warn!(member = %member.name, error = %e, "Failed to add member, cleaning up");
        cleanup_team(self.store.as_ref(), &team_id).await;
        return Err(e);
    }
}
```

- [ ] **Step 3: Find and update `TeamLaunchTool::new()` call sites**

Run: `rg "TeamLaunchTool::new\(" src/ --files-with-matches`

Update each to pass new parameters.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: success

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "tools: rewrite team_launch to spawn sub-agents instead of named agents"
```

---

### Task 9: Add sub-agent cleanup to `TeamDisbandTool`

**Files:**
- Modify: `src/builtin_tools/team_manage/disband.rs`

- [ ] **Step 1: Update `TeamDisbandTool` struct to hold SubAgentRegistry**

```rust
use crate::agents::sub_agents::registry::SubAgentRegistry;
use crate::agents::sub_agents::run::RunStatus;

#[derive(Clone)]
pub struct TeamDisbandTool {
    store: Arc<dyn CoordTaskStore>,
    sub_registry: Arc<SubAgentRegistry>,
}

impl TeamDisbandTool {
    pub fn new(store: Arc<dyn CoordTaskStore>, sub_registry: Arc<SubAgentRegistry>) -> Self {
        Self { store, sub_registry }
    }
}
```

- [ ] **Step 2: Add sub-agent kill logic to `call()` (best-effort)**

After step 3 ("Mark team as disbanded"), add:

```rust
// 4. Kill keep_alive sub-agents (best-effort — errors are logged, not propagated)
if let Ok(Some(team_data)) = self.store.get_team(&args.team_id).await {
    for member in &team_data.members {
        if let Some(run_id) = &member.run_id {
            if let Err(e) = self.sub_registry.transition(run_id, RunStatus::Cancelled).await {
                tracing::warn!(
                    run_id = %run_id,
                    role = %member.role,
                    error = %e,
                    "Failed to cancel sub-agent during team disband (will be cleaned up at session end)"
                );
            }
        }
    }
}
```

- [ ] **Step 3: Find and update `TeamDisbandTool::new()` call sites**

Run: `rg "TeamDisbandTool::new\(" src/ --files-with-matches`

Update to pass `sub_registry`.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: success

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "tools: add sub-agent cleanup to team_disband (best-effort)"
```

---

### Task 10: Inject persona into agent loop system prompt

**Files:**
- Modify: `src/agent_loop/prompt_builder.rs`
- Modify: `src/agent_loop/factory.rs` (or wherever sub-agent loop is constructed)

This is the critical runtime step — without it, persona is stored but never used.

- [ ] **Step 1: Add `persona_prefix` field to `PromptBuilder`**

In `prompt_builder.rs`, add to the `PromptBuilder` struct:

```rust
persona_prefix: Option<String>,
```

Initialize to `None` in `new()`. Add builder method:

```rust
/// Set a persona prefix (prepended before all other content).
/// Used by team sub-agents for distinct identity.
pub fn with_persona_prefix(mut self, persona: &str) -> Self {
    self.persona_prefix = Some(persona.to_string());
    self
}
```

- [ ] **Step 2: Inject persona at the start of `build()` output**

In the `build()` method of `PromptBuilder`, prepend the persona before the identity section:

```rust
pub fn build(&self, ...) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Persona prefix — highest priority, defines "who you are"
    if let Some(persona) = &self.persona_prefix {
        sections.push(persona.clone());
    }

    // ... rest of existing build logic unchanged
}
```

- [ ] **Step 3: Thread persona from SubAgentRun to PromptBuilder**

Find where the sub-agent's agent loop is constructed (likely in `factory.rs` or the execution engine). When building the `PromptBuilder` for a sub-agent run, check if `run.persona.is_some()` and call `.with_persona_prefix()`.

This requires:
1. Finding the code path: `sessions_spawn` → execution engine → agent loop factory → prompt builder
2. Passing `persona` through that chain

Run: `rg "PromptBuilder" src/agent_loop/ --files-with-matches` to find the exact wiring point.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: success

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "agent_loop: inject persona prefix into sub-agent system prompt"
```

---

### Task 11: Verify full compilation and run tests

**Files:** None (verification only)

- [ ] **Step 1: Run full compile check**

Run: `cargo check -p alephcore`
Expected: success, zero errors

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -20`
Expected: all tests pass (note: pre-existing `tools::markdown_skill::loader::tests` failures are known)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -20`
Expected: no new warnings

- [ ] **Step 4: Commit any fixups**

```bash
git add -A
git commit -m "agents: fix clippy warnings from team sub-agent redesign"
```
