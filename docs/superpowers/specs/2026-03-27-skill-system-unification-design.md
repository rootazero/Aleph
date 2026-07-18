# Skill System Unification & Enhancement Design

## Overview

Unify Aleph's four parallel skill systems into a single domain-driven architecture, add missing capabilities (install execution, API key management, status reporting), redesign the Panel UI for full skill lifecycle management, and expose skill operations as LLM Tools.

Reference: OpenClaw's skill system (install metadata, Control UI tabs, detail dialog). Goal: learn from OpenClaw, surpass it with Aleph's Rust type safety, Vault security, and LLM-first philosophy.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| System unification | Full convergence to `skill/` module | Eliminates fragmentation; `SkillManifest` already covers all fields |
| API Key storage | Vault (encrypted) | Consistent with Provider keys; security over convenience |
| Non-sensitive config | TOML file (`~/.aleph/data/skills.toml`) | Human-readable, matches existing config patterns |
| Install execution | CLI + Panel + LLM Tool | R9 (Everything is a Tool); R6 (AI Comes to You) |
| Panel UI | Status tabs + detail dialog + install/toggle/apikey | Feature parity with OpenClaw, plus scope control |

## Section 1: Unified Data Model

### Extended SkillManifest

`SkillManifest` becomes the **only skill representation**. All sources (file scan, plugin registration, runtime loading) convert to `SkillManifest` at registration time.

New fields added to existing struct in `src/domain/skill.rs`:

```rust
pub struct SkillManifest {
    // === Existing fields (unchanged) ===
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub plugin: Option<PluginId>,
    pub content: SkillContent,
    pub scope: PromptScope,
    pub bound_tool: Option<String>,
    pub eligibility: EligibilitySpec,
    pub invocation: InvocationPolicy,
    pub source: SkillSource,
    pub install_specs: Vec<InstallSpec>,

    // === New fields ===
    pub primary_env: Option<String>,     // API Key env var name (e.g. "OPENAI_API_KEY")
    pub homepage: Option<String>,        // External docs/key-acquisition URL
    pub emoji: Option<String>,           // UI icon
}
```

### Deprecated Types

| Deprecated | Replaced By |
|------------|-------------|
| `extension/types/skills.rs::ExtensionSkill` | `SkillManifest` |
| `tools/markdown_skill/spec.rs::AlephSkillSpec` | `SkillManifest` |
| `skills/mod.rs` (legacy module) | Deleted |

### Conversion Paths

- **Plugin skills**: `ExtensionManager` loads plugin → `ExtensionSkill` → `SkillManifest` (source=Plugin) → `SkillRegistry`
- **Markdown skills**: `AlephSkillSpec` parsed → `SkillManifest` (OpenClaw compat field mapping) → `SkillRegistry`
- **File skills**: Already `SkillManifest`, no change

**Boot sequence dependency direction**: `ExtensionManager` is the caller. After it finishes loading plugins and converting `ExtensionSkill` → `SkillManifest`, it calls `skill_system.register_external(converted_manifests)` to push them into the unified registry. `SkillSystem` never pulls from `ExtensionManager` — the dependency is one-way: `ExtensionManager → SkillSystem`.

## Section 2: Unified SkillSystem + Config Persistence

### SkillSystem as the Only Facade

Extended responsibilities on existing `skill/mod.rs`:

```rust
impl SkillSystem {
    // === Existing (unchanged) ===
    pub async fn init(dirs: &SkillDirs) -> Self;
    pub async fn rebuild(&self);
    pub fn list_skills(&self) -> Vec<SkillManifest>;
    pub fn get_skill(&self, id: &SkillId) -> Option<SkillManifest>;
    pub fn current_snapshot(&self) -> SkillSnapshot;
    pub fn resolve_command(&self, name: &str) -> Option<SkillCommandSpec>;

    // === New ===
    pub fn register_external(&self, manifests: Vec<SkillManifest>);
    pub async fn full_status(&self) -> Vec<SkillStatusEntry>;
    pub async fn update_config(&self, id: &SkillId, update: SkillConfigUpdate);
    pub async fn install_dependency(&self, id: &SkillId, spec_id: &str) -> InstallResult;
}
```

### Config Persistence: SkillsConfig

New file `skill/config.rs`. Storage: `~/.aleph/data/skills.toml`.

```rust
pub struct SkillsConfig {
    pub install_preferences: InstallPreferences,
    pub entries: HashMap<SkillId, SkillEntryConfig>,
}

pub struct InstallPreferences {
    pub prefer_brew: bool,
    pub node_manager: NodeManager,  // npm | pnpm | yarn | bun
}

pub struct SkillEntryConfig {
    pub enabled: Option<bool>,              // None = auto, Some(false) = user disabled
    pub scope_override: Option<PromptScope>,
}

pub enum SkillConfigUpdate {
    SetEnabled(bool),
    SetScope(PromptScope),
    StoreApiKey(String),     // Routes to Vault
    DeleteApiKey,            // Routes to Vault
}
```

- TOML format, human-readable and editable
- API Keys never in this file — `StoreApiKey` delegates to Vault with key `skill:{skill_id}`
- `enabled: Option<bool>` — `None` = follow eligibility auto-detection; `Some(false)` = user disabled; `Some(true)` = user wants it enabled
- Atomic file writes on every `update_config`

### extension/skill_ops.rs Evolution

Thinned to delegation layer, then removed after callers migrate:

```rust
impl ExtensionManager {
    pub fn get_all_skills(&self) -> Vec<SkillManifest> {
        self.skill_system.list_skills()  // delegate
    }
    pub async fn execute_skill(&self, name: &str, args: &str) -> String {
        self.skill_system.execute(name, args).await  // delegate
    }
}
```

## Section 3: SkillStatusEntry — Full Status Report

### Structure

Replaces existing `SkillStatusReport` in `skill/status.rs`:

```rust
pub struct SkillStatusEntry {
    // Identity
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
    pub source: SkillSource,
    pub homepage: Option<String>,

    // Status triple
    pub eligible: bool,
    pub disabled: bool,
    pub missing: MissingRequirements,

    // Install
    pub install_options: Vec<InstallOption>,

    // API Key
    pub primary_env: Option<String>,
    pub api_key_set: bool,      // Vault has key (value never exposed)

    // User config
    pub scope: PromptScope,     // Effective scope (may be overridden)
    pub user_invocable: bool,
}

pub struct MissingRequirements {
    pub bins: Vec<String>,
    pub env: Vec<String>,
    pub config: Vec<String>,
}

pub struct InstallOption {
    pub id: String,
    pub kind: InstallKind,
    pub label: String,       // "Install git (brew)"
    pub bins: Vec<String>,
}
```

### Status Classification (UI Tab Logic)

```rust
pub enum SkillStatusFilter { All, Ready, NeedsSetup, Disabled }

impl SkillStatusEntry {
    pub fn matches_filter(&self, filter: &SkillStatusFilter) -> bool {
        match filter {
            All => true,
            Ready => self.eligible && !self.disabled,
            NeedsSetup => !self.eligible && !self.disabled,
            Disabled => self.disabled,
        }
    }
}
```

### Build Flow

```
SkillSystem::full_status():
  1. Iterate all SkillManifest in SkillRegistry
  2. For each manifest:
     a. EligibilityService::evaluate() → eligible + missing
     b. SkillsConfig::get(id) → enabled override, scope override
     c. SkillsConfig::has_api_key(id, vault) → api_key_set
     d. filter_install_specs_for_current_os() → install_options
     e. If primary_env set && api_key_set=false → add to missing.env
  3. Assemble SkillStatusEntry
```

## Section 4: Install Execution Pipeline

### Architecture

```
Trigger (CLI / Panel UI / LLM Tool)
    ↓ RPC: skills.install_dep { id, spec_id }
SkillSystem::install_dependency()
    ↓
InstallExecutor::run(spec)
    ↓
    ├── Validate package name safety (existing logic)
    ├── Select install command (build_install_command)
    ├── Bootstrap dependencies (auto-install uv if needed)
    ├── Execute + timeout + output capture
    └── Re-evaluate eligibility → update snapshot
```

### InstallExecutor

Extended in existing `skill/installer.rs`:

```rust
pub struct InstallExecutor;

impl InstallExecutor {
    pub async fn run(spec: &InstallSpec, prefs: &InstallPreferences) -> InstallResult {
        // 1. Safety validation (no shell metacharacters — existing)
        // 2. Dependency bootstrap (uv/go not found → install uv first)
        // 3. Build command
        // 4. tokio::process::Command with 300s timeout
        // 5. Capture stdout/stderr
    }
}

pub struct InstallResult {
    pub success: bool,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}
```

### Install Preference Selection

```rust
pub fn select_best_install(
    specs: &[InstallSpec],
    prefs: &InstallPreferences,
) -> Option<&InstallSpec> {
    // 1. Filter current OS
    // 2. Priority: prefer_brew=true → Brew > Uv > Npm > Go > Apt > Download
    //             prefer_brew=false → Uv > Npm > Brew > Go > Apt > Download
    // 3. Return first match
}
```

### Dependency Bootstrap

```rust
async fn bootstrap_if_needed(kind: &InstallKind) -> Result<()> {
    match kind {
        Uv if !has_binary("uv") => { /* curl uv installer */ }
        Go if !has_binary("go") => { return Err(NeedManualInstall("go")) }
        _ => Ok(())
    }
}
```

### Post-Install Refresh

1. `EligibilityService::evaluate()` re-checks the skill
2. `SkillSystem::rebuild()` rebuilds snapshot
3. Returns `InstallResult` + updated `SkillStatusEntry`

## Section 5: RPC Layer + Vault Integration

### Unified RPC Endpoints

```rust
// gateway/handlers/skills.rs — rewritten

"skills.status"       // → Vec<SkillStatusEntry>
"skills.update"       // { skill_id, enabled?, scope?, api_key? } → SkillStatusEntry
"skills.install_dep"  // { skill_id, spec_id? } → { result: InstallResult, skill: SkillStatusEntry }
"skills.add"          // { source: String } → Vec<SkillStatusEntry>  (URL or base64 zip)
"skills.remove"       // { skill_id } → { ok: bool }
```

### Deprecated Endpoints

| Deprecated | Replaced By |
|------------|-------------|
| `skills.list` | `skills.status` |
| `skills.install` | `skills.add` |
| `skills.installFromZip` | `skills.add` |
| `markdown_skills.list` | `skills.status` |
| `markdown_skills.install` | `skills.add` |
| `markdown_skills.reload` | `skills.status` (auto-refresh) |
| `markdown_skills.unload` | `skills.remove` |

### Vault Integration

Note: `SharedTokenManager` methods are **synchronous** (`fn`, not `async fn`). `get_secret` returns `Result<Option<DecryptedSecret>>`, not `Result<String>`.

```rust
// skill/config.rs
impl SkillsConfig {
    pub fn store_api_key(&self, skill_id: &SkillId, value: &str, vault: &SharedTokenManager) {
        let _ = vault.store_secret(&format!("skill:{}", skill_id), value);
    }
    pub fn has_api_key(&self, skill_id: &SkillId, vault: &SharedTokenManager) -> bool {
        matches!(vault.get_secret(&format!("skill:{}", skill_id)), Ok(Some(_)))
    }
    pub fn delete_api_key(&self, skill_id: &SkillId, vault: &SharedTokenManager) {
        let _ = vault.delete_secret(&format!("skill:{}", skill_id));
    }
}
```

### Runtime API Key Injection

Before skill execution, if `primary_env` is set and Vault has the key, inject it into the **child process environment** via `Command::env()`, NOT via `std::env::set_var` (which is unsound in multi-threaded Rust since 1.66+).

```rust
// In InstallExecutor / skill execution context
let mut cmd = tokio::process::Command::new(command);
if let Some(env_name) = &manifest.primary_env {
    if let Ok(Some(secret)) = vault.get_secret(&format!("skill:{}", manifest.id)) {
        cmd.env(env_name, secret.expose());
    }
}
```

This way each child process gets its own isolated environment. No data races, no cross-skill contamination.

## Section 6: Panel UI Redesign

### Overall Structure

```
┌─────────────────────────────────────────┐
│ Skills                        [Refresh] │
│                                         │
│ [All (12)] [Ready (8)] [Needs Setup (3)] [Disabled (1)] │
│                                         │
│ ┌─ Aleph ──────────────────────────────┐│
│ │ code-review      Ready        [toggle]││
│ │ web-search       Needs Setup  [toggle]││
│ └──────────────────────────────────────┘│
│ ┌─ Official ───────────────────────────┐│
│ │ email-agent      Ready        [toggle]││
│ └──────────────────────────────────────┘│
│ ┌─ Plugin ─────────────────────────────┐│
│ │ ...                                  ││
│ └──────────────────────────────────────┘│
│              [+ Add Skill]              │
└─────────────────────────────────────────┘
```

### Status Filter Tabs

Four tabs with dynamic counts, computed client-side from `skills.status` response:

- **All** — everything
- **Ready** — `eligible && !disabled`
- **Needs Setup** — `!eligible && !disabled`
- **Disabled** — `disabled`

### Skill Detail Dialog (click skill name)

```
┌─────────────────────────────────────┐
│ code-review                      ✕  │
│ [Ready] [Bundled] [System scope]    │
│ Description text...                 │
│                                     │
│ ── Requirements ──────────────────  │
│ ✅ git    ✅ gh    ⚠️ rg (missing)  │
│     [Install rg (brew)]            │
│                                     │
│ ── API Key ───────────────────────  │
│ OPENAI_API_KEY  [●●●●●●] [Save]   │
│ ↗ Get your key at openai.com      │
│                                     │
│ ── Settings ──────────────────────  │
│ Enabled  [toggle]                   │
│ Scope    [System ▾]                │
│                                     │
│ ── Info ──────────────────────────  │
│ Source: Bundled                      │
│ ID: aleph:code-review              │
│ ↗ Homepage                         │
└─────────────────────────────────────┘
```

### Dialog Interactions

| Area | Action | RPC Call |
|------|--------|----------|
| Install button | Install missing deps | `skills.install_dep` |
| API Key input | Save to Vault | `skills.update { api_key }` |
| Enabled toggle | Toggle enable | `skills.update { enabled }` |
| Scope dropdown | Change injection mode | `skills.update { scope }` |
| Homepage link | External navigation | Frontend only |

### Status Feedback

- Installing: spinner + "Installing..."
- Install success: green checkmark + auto-refresh status
- Install failure: red error + stderr summary
- API Key saved: brief "Saved" toast

### Leptos Component Split

```
views/settings/skills.rs → split into:
  skills/mod.rs            — SkillsView main + tab bar
  skills/skill_list.rs     — List + source grouping
  skills/skill_card.rs     — Single skill row
  skills/skill_detail.rs   — Detail dialog
  skills/skill_install.rs  — Install interaction (button + progress + result)
  skills/add_skill.rs      — "Add Skill" dialog (URL/zip input)
```

## Section 7: LLM Tools

Three new Tools for conversational skill management:

```rust
// builtin_tools/skill_install.rs
"skill_install" — Install skill dependencies
// { skill_id: String, spec_id?: String }

// builtin_tools/skill_manage.rs
"skill_manage" — Toggle/configure skills
// { skill_id: String, enabled?: bool, scope?: String }

// builtin_tools/skill_status.rs
"skill_status" — Query skill status
// { filter?: "all" | "ready" | "needs_setup" | "disabled" }
```

API Key storage reuses existing `vault_store` Tool — no new Tool needed.

## Section 8: Code Cleanup

| Delete | Reason |
|--------|--------|
| `src/skills/` entire directory | Legacy, fully replaced by `skill/` |
| `src/tools/markdown_skill/` | Loading logic migrated to `skill/manifest.rs` |
| `ExtensionSkill` type alias in `extension/types/skills.rs` | Use `SkillManifest` directly |
| `extension/skill_ops.rs` | After callers migrate to `SkillSystem` |
| `markdown_skills.*` RPC handlers | Unified into `skills.*` |
| `InstallSkillDialog` calling `markdown_skills.install` | Unified into `skills.add` |

## Where Aleph Surpasses OpenClaw

| Dimension | OpenClaw | Aleph |
|-----------|----------|-------|
| **Secret storage** | Plaintext config JSON | Vault encrypted, runtime env injection |
| **Type safety** | TypeScript runtime types | Rust compile-time `SkillManifest` guarantees |
| **LLM self-service** | None — user must use UI | 3 Tools: LLM can install/configure/query |
| **Snapshot caching** | Re-evaluates every query | `SkillSnapshot` with version, incremental rebuild |
| **Scope control** | Enable/disable only | System/Tool/Standalone/Disabled fine-grained control |
| **Priority dedup** | Last source wins | Explicit priority: Workspace > Plugin > Global > Bundled |
| **Linux support** | brew/npm/uv/go/download | +apt for Linux server deployments |
| **Scope override** | None | Per-skill prompt injection mode override |
| **Philosophy** | UI-driven management | Conversation-driven: LLM detects missing deps, offers to install (R6) |
