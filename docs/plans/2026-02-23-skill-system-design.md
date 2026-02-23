# Skill System Design: Domain-Driven Skill-First Architecture

> Date: 2026-02-23
> Status: Approved
> Reference: OpenClaw Skills analysis + Aleph DDD conventions

---

## Motivation

Aleph's current extension system has a **dual-track architecture** — Static Markdown skills and Runtime plugins (Node.js/WASM). While functional, it lacks several capabilities that OpenClaw's Skills system demonstrates:

| Capability | OpenClaw | Aleph Current |
|------------|----------|---------------|
| Runtime eligibility gating | Binary/env/OS/config checks | None — skills loaded unconditionally |
| Dependency installation | brew/npm/go/uv/download auto-install | None |
| Session snapshot caching | Per-session snapshot with version invalidation | Per-request registry scan |
| Environment injection | Turn-scoped env var management | None |
| Skill status reporting | Rich eligibility dashboard | None |
| Agent-level skill filtering | Per-agent skill whitelist | None |
| Slash command direct dispatch | `/skill` can bypass LLM → call tool directly | Commands always go through LLM |

This design introduces a **Skill-First unified architecture** where Skills are first-class citizens and Plugins are packaging units for Skills.

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Skill-First unified | Plugin is a packaging unit for Skills, Tools, Hooks |
| Approach | Domain rebuild (DDD) | New `SkillManifest` aggregate root, fits Aleph's DDD conventions |
| Eligibility scope | Server-Only | All execution on Server; consistent with "brain in cloud" principle |
| Dependency install | Implement with Exec approval | Reuse existing approval workflow for safety |
| Snapshot strategy | Global singleton | All sessions share one snapshot; version bump on file change |

---

## 1. Domain Model

### New Bounded Context: Skill

```
┌─────────────────────────────────────────────────────┐
│                Skill Bounded Context                 │
├─────────────────────────────────────────────────────┤
│                                                      │
│  SkillManifest (AggregateRoot)                       │
│  ├── id: SkillId                                     │
│  ├── name: String                                    │
│  ├── plugin: Option<PluginId>                        │
│  ├── description: String                             │
│  ├── content: SkillContent                           │
│  ├── scope: PromptScope                              │
│  ├── eligibility: EligibilitySpec                    │
│  ├── install_specs: Vec<InstallSpec>                 │
│  ├── invocation: InvocationPolicy                    │
│  ├── source: SkillSource                             │
│  └── priority: u8                                    │
│                                                      │
│  EligibilitySpec (ValueObject)                       │
│  ├── os: Option<Vec<Os>>                             │
│  ├── required_bins: Vec<String>                      │
│  ├── any_bins: Vec<String>                           │
│  ├── required_env: Vec<String>                       │
│  ├── required_config: Vec<String>                    │
│  ├── always: bool                                    │
│  └── enabled: Option<bool>                           │
│                                                      │
│  InstallSpec (ValueObject)                           │
│  ├── id: String                                      │
│  ├── kind: InstallKind                               │
│  ├── package: String                                 │
│  ├── bins: Vec<String>                               │
│  └── url: Option<String>                             │
│                                                      │
│  InvocationPolicy (ValueObject)                      │
│  ├── user_invocable: bool                            │
│  ├── disable_model_invocation: bool                  │
│  └── command_dispatch: Option<DispatchSpec>           │
│                                                      │
│  SkillSnapshot (Entity)                              │
│  ├── version: u64                                    │
│  ├── prompt_xml: String                              │
│  ├── eligible: Vec<SkillId>                          │
│  ├── ineligible: HashMap<SkillId, Vec<Reason>>       │
│  ├── skill_commands: Vec<SkillCommandSpec>            │
│  └── built_at: Instant                               │
│                                                      │
└─────────────────────────────────────────────────────┘
```

### Relationship to Existing Contexts

```
Skill Context ──uses──→ Dispatcher Context  (SkillId → tool routing)
Skill Context ──uses──→ POE Context         (SkillManifest → success contract)
Extension     ──migrates──→ Skill Context   (ExtensionSkill → SkillManifest)
```

### Rust Traits

```rust
// domain/skill.rs

/// Unique identifier for a skill. Format: "plugin:name" or just "name"
#[derive(Clone, Eq, PartialEq, Hash, Display)]
pub struct SkillId(String);

impl Entity for SkillManifest {
    type Id = SkillId;
    fn id(&self) -> &Self::Id { &self.id }
}

impl AggregateRoot for SkillManifest {}

impl ValueObject for EligibilitySpec {}
impl ValueObject for InstallSpec {}
impl ValueObject for InvocationPolicy {}
```

---

## 2. Eligibility Engine

### EligibilityService

The eligibility engine evaluates whether a Skill is usable in the current Server environment.

```
SkillManifest.eligibility
        │
        ▼
EligibilityService::evaluate(spec, ctx) → EligibilityResult
        │
        ├── always == true         → Eligible (skip all checks)
        ├── enabled == false       → Ineligible("disabled by config")
        ├── os check               → Ineligible("requires darwin, got linux")
        ├── required_bins check    → Ineligible("missing binary: ffmpeg")
        ├── any_bins check         → Ineligible("need one of: chrome, chromium")
        ├── required_env check     → Ineligible("missing env: OPENAI_API_KEY")
        ├── required_config check  → Ineligible("config key not set")
        └── all pass               → Eligible
```

### EligibilityContext

```rust
pub struct EligibilityContext {
    pub os: Os,                           // darwin | linux | windows
    pub arch: Arch,                       // aarch64 | x86_64
    pub available_bins: HashSet<String>,   // executables on PATH
    pub env_vars: HashSet<String>,         // set env var names (not values)
    pub config: Arc<AlephConfig>,          // current config
}
```

Design notes:
- `available_bins` scanned once at Server start, refreshed on PATH changes or after installs
- `env_vars` stores names only (no values) for security
- Entire context is `Clone + Send + Sync`

### EligibilityResult

```rust
pub enum EligibilityResult {
    Eligible,
    Ineligible(Vec<IneligibilityReason>),
}

pub struct IneligibilityReason {
    pub kind: ReasonKind,       // MissingBinary | MissingEnv | WrongOs | ConfigNotSet | Disabled
    pub message: String,
    pub install_hint: Option<InstallSpec>,
}
```

When `Ineligible(MissingBinary)` + matching `InstallSpec` exists → status report shows "installable" action.

---

## 3. Global Snapshot System

### SkillSnapshot

```rust
pub struct SkillSnapshot {
    pub version: u64,
    pub prompt_xml: String,                                    // <available_skills> XML
    pub eligible: Vec<SkillId>,
    pub ineligible: HashMap<SkillId, Vec<IneligibilityReason>>,
    pub skill_commands: Vec<SkillCommandSpec>,
    pub built_at: Instant,
}
```

### Lifecycle

```
Server start
    │
    ▼
SkillSnapshotManager::build_initial()
    ├── ExtensionManager::load_all()
    ├── EligibilityService::evaluate_all()
    ├── format_prompt_xml()
    └── store in Arc<RwLock<SkillSnapshot>>
    │
    ▼
Watcher detects file/config change
    │
    ▼
SkillSnapshotManager::rebuild()
    ├── reload changed Skills
    ├── re-evaluate eligibility
    ├── version += 1
    └── emit GlobalBus: SkillSnapshotUpdated(version)
    │
    ▼
Agent Loop starts new turn
    │
    ▼
SkillSnapshotManager::current() → Arc<SkillSnapshot>
    └── prompt_xml injected into system prompt
```

### Integration with Existing Watcher

Reuses `extension/watcher.rs` `ExtensionChangeEvent`:

```rust
match event {
    ExtensionChangeEvent::Skill(_) |
    ExtensionChangeEvent::Plugin(_) => {
        snapshot_manager.rebuild().await;
    }
    _ => {} // commands, agents don't affect skill snapshot
}
```

---

## 4. Dependency Installation

### SkillInstaller

```rust
pub struct SkillInstaller {
    exec_engine: Arc<ExecEngine>,
}

impl SkillInstaller {
    pub async fn install(
        &self,
        skill_id: &SkillId,
        spec: &InstallSpec,
    ) -> Result<InstallResult> {
        let command = spec.to_shell_command();

        // Reuse Exec approval workflow — user must confirm
        let approval = self.exec_engine
            .request_approval(&command, ApprovalContext::SkillInstall {
                skill: skill_id.clone(),
                package: spec.package.clone(),
            })
            .await?;

        match approval {
            Approved => {
                let output = self.exec_engine.execute(&command).await?;
                // Refresh EligibilityContext.available_bins after install
                Ok(InstallResult::Success(output))
            }
            Denied => Ok(InstallResult::Denied),
        }
    }
}
```

### InstallSpec → Shell Command Mapping

| Kind | Command |
|------|---------|
| Brew | `brew install {package}` |
| Cargo | `cargo install {package}` |
| Uv | `uv tool install {package}` |
| Download | `curl -fsSL {url} \| tar xz -C ~/.aleph/tools/{id}/` |

### SKILL.md Frontmatter Extension

```yaml
---
name: browser-automation
description: Automate browser tasks with Playwright
scope: system
eligibility:
  os: [darwin, linux]
  required_bins: [npx]
  required_env: []
install:
  - id: playwright
    kind: uv
    package: playwright
    bins: [npx]
---
```

### Security Scanner (Interface Reserved)

```rust
pub trait SkillSecurityScanner: Send + Sync {
    async fn scan(&self, skill_dir: &Path) -> Vec<SecurityFinding>;
}

// Default: check frontmatter URLs only
pub struct BasicSkillScanner;
```

Full code scanning deferred — Aleph Skills are Markdown, not executable code.

---

## 5. Skill Invocation Flow

### Path A: LLM Auto-Invocation

```
User: "Help me analyze this code"
    │
    ▼
PromptBuilder::build_system_prompt()
    ├── SkillSnapshot.prompt_xml → <available_skills> XML
    ├── Injected into skill tool description
    └── LLM sees: <skill><name>code-review</name>...</skill>
    │
    ▼
LLM calls skill tool: { "name": "code-review", "arguments": "analyze quality" }
    │
    ▼
SkillExecutor::invoke(skill_id, arguments, ctx)
    ├── Load SkillManifest from SkillRegistry
    ├── Check permission (Allow/Ask/Deny)
    ├── Render template: content.replace("$ARGUMENTS", arguments)
    └── Return rendered Markdown as tool result
    │
    ▼
LLM follows Skill instructions
```

### Path B: Slash Command

```
User: "/code-review src/main.rs"
    │
    ▼
Gateway::handle_message()
    ├── Detect "/" prefix
    ├── Match against SkillSnapshot.skill_commands
    └── Found SkillCommandSpec { skill_id, dispatch }
    │
    ├── dispatch == None (standard):
    │     Rewrite message: "Use the 'code-review' skill for: src/main.rs"
    │     → Normal Agent Loop
    │
    └── dispatch == Some(Tool { tool_name }):
          Direct tool call, bypass LLM
          → Return tool result
```

### Path C: Agent-Level Filtering

```
Config:
  agents:
    coding-bot:
      skills: [code-review, github, testing]

Agent Loop starts (agent_id = "coding-bot")
    │
    ▼
SkillSnapshotManager::filtered_snapshot(agent_id)
    ├── Take global snapshot
    ├── Intersect: eligible ∩ agent.skills
    └── Regenerate prompt_xml with filtered skills only
```

---

## 6. Module Structure

### File Layout

```
core/src/
├── domain/
│   ├── mod.rs           # Existing: Entity, AggregateRoot, ValueObject
│   └── skill.rs         # NEW: SkillEntity trait, SkillId, EligibilitySpec...
├── skill/               # NEW module
│   ├── mod.rs           # SkillSystem entry point
│   ├── manifest.rs      # SkillManifest implementation
│   ├── eligibility.rs   # EligibilityService + EligibilityContext
│   ├── snapshot.rs      # SkillSnapshotManager
│   ├── installer.rs     # SkillInstaller
│   ├── registry.rs      # SkillRegistry
│   ├── executor.rs      # SkillExecutor (invoke, permission check)
│   ├── prompt.rs        # prompt_xml generation, skill tool description
│   ├── frontmatter.rs   # YAML frontmatter parsing (eligibility/install fields)
│   ├── status.rs        # SkillStatusReport generation
│   └── commands.rs      # SkillCommandSpec, slash command resolution
├── extension/           # Existing (gradual migration)
│   ├── mod.rs           # ExtensionManager delegates to SkillSystem internally
│   ├── discovery/       # Retained: file discovery logic reused
│   ├── watcher.rs       # Retained: file monitoring reused
│   ├── runtime/         # Retained: Node.js/WASM runtime
│   └── ...
```

### Integration Points

| Integration Point | Change |
|-------------------|--------|
| `ExtensionManager` | Hold `SkillSystem` internally, delegate skill operations |
| `PromptBuilder` | Read `prompt_xml` from `SkillSnapshot` instead of traversing registry |
| `AgentLoop` | Get current snapshot from `SkillSnapshotManager` at turn start |
| `Gateway handlers` | New RPC: `skills.status`, `skills.install_dep` |
| `GlobalBus` | New event: `SkillSnapshotUpdated` |

### Migration Strategy

```
Phase 1: Add domain/skill.rs + skill/ module alongside extension
Phase 2: ExtensionManager delegates skill operations to SkillSystem
Phase 3: PromptBuilder / AgentLoop switch to new interfaces
Phase 4: Deprecate skill-related code in extension module
```

---

## 7. What We Learn From OpenClaw, What We Surpass

### Learned

| Feature | OpenClaw's Insight |
|---------|-------------------|
| Eligibility gating | Runtime environment checks before including skills in prompt |
| Snapshot caching | Build once per session, version-invalidate on change |
| Install specs | Declarative dependency installation in frontmatter |
| Slash command dispatch | Direct tool bypass without LLM for deterministic operations |
| Agent-level filtering | Per-agent skill whitelists for focused behavior |

### Surpassed

| Dimension | OpenClaw | Aleph |
|-----------|----------|-------|
| Type safety | TypeScript types, runtime validation | Rust traits + schemars, compile-time guarantees |
| Domain modeling | Flat data structures | DDD aggregate roots, value objects, bounded contexts |
| Extension runtime | None (markdown only) | WASM sandbox + Node.js IPC + MCP |
| Prompt scope | Binary (included or not) | Four-level: System/Tool/Standalone/Disabled |
| Security | Pre-install code scanning | Exec approval workflow + security scanner interface |
| Architecture | Single-process | Server-Client distributed execution |
| Skill content | Read via file tool (extra LLM call) | Direct injection as tool result (zero extra calls) |

---

## 8. Non-Goals (This Iteration)

- Skill composition / chaining (future consideration)
- Remote node eligibility (Client-side binary detection)
- Full code security scanning (only URL checks)
- Skill marketplace / registry
- Skill versioning and rollback
