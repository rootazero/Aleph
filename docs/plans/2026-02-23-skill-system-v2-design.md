# Skill System v2: Complete DDD Rebuild

> Date: 2026-02-23
> Status: Approved
> Supersedes: 2026-02-23-skill-system-design.md (original design preserved)
> Reference: OpenClaw Skills analysis + Aleph DDD conventions

---

## Context

The previous implementation (Phase 1: 14 tasks, Phase 2: partial) was lost when a git worktree was deleted. This design rebuilds the Skill System from scratch using the same architectural decisions, with refinements based on lessons learned.

### Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | Complete DDD, independent `core/src/skill/` module | Clean bounded context, testable in isolation |
| Integration | `ExtensionManager` holds `Option<SkillSystem>` | Cross-reference without tight coupling |
| Frontmatter | Full YAML with EligibilitySpec, InstallSpec, InvocationPolicy | Feature parity with OpenClaw, DDD ValueObjects |
| Snapshot | Global version-invalidated cache | All sessions share one snapshot, fast reads |
| Clone | `Arc<Inner>` pattern | Cheap sharing across Gateway handlers, ExecutionEngine |

---

## 1. Domain Model (`core/src/domain/skill.rs`)

### AggregateRoot

```rust
pub struct SkillManifest {
    id: SkillId,
    name: String,
    plugin: Option<PluginId>,
    description: String,
    content: SkillContent,        // raw markdown body
    scope: PromptScope,
    eligibility: EligibilitySpec,
    install_specs: Vec<InstallSpec>,
    invocation: InvocationPolicy,
    source: SkillSource,
    priority: u8,
}
```

Implements `Entity<Id = SkillId>` + `AggregateRoot` traits per `core/src/domain/` conventions.

### ValueObjects

```rust
// Newtype pattern per DESIGN_PATTERNS.md
pub struct SkillId(String);  // "plugin:skill-name" or "skill-name"
pub struct PluginId(String);
pub struct SkillContent(String);  // markdown body

pub enum PromptScope { System, Tool, Standalone, Disabled }

pub struct EligibilitySpec {
    pub os: Option<Vec<Os>>,
    pub required_bins: Vec<String>,
    pub any_bins: Vec<String>,
    pub required_env: Vec<String>,
    pub required_config: Vec<String>,
    pub always: bool,
    pub enabled: Option<bool>,
}

pub enum Os { Darwin, Linux, Windows }

pub struct InstallSpec {
    pub id: String,
    pub kind: InstallKind,
    pub package: String,
    pub bins: Vec<String>,
    pub os: Option<Vec<Os>>,
    pub url: Option<String>,
}

pub enum InstallKind { Brew, Apt, Npm, Uv, Go, Download }

pub struct InvocationPolicy {
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub command_dispatch: Option<DispatchSpec>,
}

pub struct DispatchSpec {
    pub tool_name: String,
    pub arg_mode: ArgMode,
}

pub enum ArgMode { Raw, Parsed }

pub enum SkillSource {
    Bundled,
    Global,
    Workspace,
    Plugin(PluginId),
}
```

All ValueObjects implement `Eq + Clone` per `ValueObject` trait.

---

## 2. Module Structure (`core/src/skill/`)

```
core/src/skill/
├── mod.rs              # SkillSystem (Arc<Inner>), public API facade
├── manifest.rs         # SKILL.md parser: YAML frontmatter → SkillManifest
├── eligibility.rs      # EligibilityService: runtime checks (OS/bins/env/config)
├── snapshot.rs         # SkillSnapshot: version-invalidated global cache
├── registry.rs         # SkillRegistry: HashMap<SkillId, SkillManifest> + query
├── prompt.rs           # XML prompt generation from eligible skills
├── installer.rs        # dependency installation via Exec approval workflow
├── status.rs           # SkillStatusReport for RPC handlers
└── commands.rs         # slash command resolution (/skill-name → dispatch)
```

### SkillSystem API

```rust
#[derive(Clone)]
pub struct SkillSystem {
    inner: Arc<Inner>,
}

struct Inner {
    registry: RwLock<SkillRegistry>,
    snapshot: RwLock<SkillSnapshot>,
    skill_dirs: RwLock<Vec<PathBuf>>,
    eligibility: EligibilityService,
}

impl SkillSystem {
    pub fn new() -> Self;

    // Lifecycle
    pub async fn init(&self, dirs: Vec<PathBuf>) -> Result<()>;
    pub async fn rebuild(&self) -> Result<()>;
    pub async fn reload_file(&self, path: &Path) -> Result<()>;

    // Queries
    pub async fn current_snapshot(&self) -> SkillSnapshot;
    pub async fn get_skill(&self, id: &SkillId) -> Option<SkillManifest>;
    pub async fn list_skills(&self) -> Vec<SkillManifest>;
    pub async fn skill_status(&self) -> Vec<SkillStatusReport>;

    // Execution
    pub async fn resolve_command(&self, name: &str) -> Option<SkillCommandSpec>;
    pub async fn install_deps(&self, id: &SkillId) -> Result<InstallResult>;
}
```

---

## 3. Integration Points

### ExtensionManager

```rust
pub struct ExtensionManager {
    // ...existing fields...
    skill_system: Option<SkillSystem>,
}

impl ExtensionManager {
    pub fn skill_system(&self) -> Option<&SkillSystem>;
    pub async fn init_skill_system(&mut self, dirs: Vec<PathBuf>) -> Result<()>;
}
```

### PromptBuilder (existing modification preserved)

```rust
pub struct PromptConfig {
    // ...existing fields...
    pub skill_instructions: Option<String>,
}
```

### ExecutionEngine

Reads `SkillSystem::current_snapshot().prompt_xml` at agent turn start, injects into `ThinkerConfig.prompt.skill_instructions`.

### Gateway RPC Handlers

| Method | Description |
|--------|-------------|
| `skills.list` | Returns all skills with eligibility status |
| `skills.status` | Returns detailed eligibility check results |
| `skills.install` | Triggers dependency installation for a skill |
| `skills.reload` | Force rescan all skill directories |

---

## 4. Data Flows

### 4.1 Startup Loading

```
Server Start
  → ExtensionManager::init_skill_system(dirs)
    → SkillSystem::init(dirs)
      → scan dirs for SKILL.md → parse → SkillManifest
      → registry.register_all(manifests)
      → eligibility.evaluate_all(registry)
      → snapshot.rebuild(registry, eligibility_results)
```

### 4.2 Agent Turn Injection

```
ExecutionEngine::run_agent()
  → skill_system.current_snapshot()
  → ThinkerConfig { skill_instructions: snapshot.prompt_xml }
  → PromptBuilder::append_skill_instructions()
  → LLM sees <available_skills> in system prompt
```

### 4.3 Hot-Reload (single file)

```
reload_file(path)
  → re-parse manifest
  → registry.update(id, manifest)
  → eligibility.evaluate(id)
  → snapshot.rebuild()  // version++
```

---

## 5. Skill Directory Priority (high → low)

| Priority | Source | Path |
|----------|--------|------|
| 4 | Workspace | `./.aleph/skills/` |
| 3 | Project plugins | `./.aleph/extensions/*/skills/` |
| 2 | Global | `~/.aleph/skills/` |
| 1 | Bundled | `apps/macos/Resources/skills/` |

Same-name skills: higher priority wins.

---

## 6. Eligibility Engine

```rust
pub struct EligibilityService;

impl EligibilityService {
    pub fn evaluate(&self, manifest: &SkillManifest) -> EligibilityResult;
    pub fn evaluate_all(&self, registry: &SkillRegistry) -> HashMap<SkillId, EligibilityResult>;
}

pub enum EligibilityResult {
    Eligible,
    Ineligible(Vec<IneligibilityReason>),
}

pub enum IneligibilityReason {
    Disabled,
    OsNotSupported(Os),
    MissingBinary(String),
    MissingAnyBinary(Vec<String>),
    MissingEnv(String),
    MissingConfig(String),
}
```

Check order: `always` flag → `enabled` override → OS → binaries → env → config.

---

## 7. SKILL.md Format

```yaml
---
name: github
description: GitHub CLI operations
scope: system
user-invocable: true
disable-model-invocation: false
eligibility:
  os: [darwin, linux]
  required_bins: [gh]
  any_bins: []
  required_env: [GITHUB_TOKEN]
  required_config: []
  always: false
install:
  - id: brew-gh
    kind: brew
    package: gh
    bins: [gh]
    os: [darwin]
  - id: apt-gh
    kind: apt
    package: gh
    bins: [gh]
---
# GitHub Skill

Instructions for the LLM when this skill is active...
```

---

## 8. Scope Boundary

### In Scope

- Domain model (all ValueObjects and AggregateRoot)
- SKILL.md parser with full frontmatter
- Eligibility engine
- Snapshot manager with version invalidation
- Registry with priority-based dedup
- Prompt XML generation
- Installer (InstallSpec → shell command via Exec)
- Status reporting
- Slash command resolution
- ExtensionManager integration
- Gateway RPC handlers
- Unit and integration tests

### Out of Scope

- File watcher (hot-reload via fs events)
- Agent-level skill filtering
- Skill Evolution/Solidification
- Skill Sandboxing
- Turn-scoped environment injection

---

## 9. Cleanup

| File | Action |
|------|--------|
| `core/src/extension/skill_system.rs` | Delete (replaced by `core/src/skill/`) |
| `core/src/thinker/prompt_builder.rs` diff | Preserve (`skill_instructions` field) |
| `core/src/gateway/execution_engine.rs` diff | Rewrite (use formal SkillSystem API) |

---

## 10. Dependencies

- `serde` + `serde_yaml`: frontmatter parsing (existing)
- `which`: binary checks (existing)
- `chrono`: timestamps (existing)
- No new external dependencies
