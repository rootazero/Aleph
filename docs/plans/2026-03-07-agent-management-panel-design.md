# Agent Management Panel Design

> Date: 2026-03-07
> Status: Approved
> Scope: Panel Agent CRUD + 6-Tab Management UI + AgentManager Service

---

## Background

Aleph has a solid agent definition system at the config/runtime level (`AgentsConfig`, `AgentDefinition`, `AgentDefinitionResolver`, `AgentRouter`), but no UI or RPC layer for users to manage agents dynamically. The panel's current Agent settings page (`/settings/agent`) only exposes behavior configuration (file ops, code exec, general settings) — not agent lifecycle management.

This design introduces a full Agent Management system: data model extensions, a service layer for TOML-based CRUD, Gateway RPC methods, and a top-level Panel UI with 6 tabs.

### Reference

OpenClaw's agent management was studied for inspiration. Key takeaways adopted:
- Tabbed agent detail view (Overview / Files / Tools / Skills / Channels)
- Cascading defaults (agent → defaults → implicit)
- Workspace-centric model with bootstrap file editing
- Deletion with trash + binding cascade cleanup

Key differences from OpenClaw:
- TOML (not JSON5) as config source — using `toml_edit` for precision edits
- Agent is a **top-level navigation item** (not nested under Settings)
- Leptos/WASM components (not TypeScript DOM)
- Aleph's DDD value objects (`AgentIdentity`, `AgentModelConfig`, `AgentParams`)

---

## 1. Navigation Architecture

### Bottom Bar

`PanelMode` gains a fourth variant:

```
Chat | Dashboard | Agents | Settings
```

- Icon: Bot-style SVG (Lucide `bot` or custom)
- Route prefix: `/agents`
- Agent removed from Settings sidebar

### Route Structure

```
/agents                          → Default agent's Overview
/agents/{id}/overview            → Overview Tab
/agents/{id}/behavior            → Behavior Tab
/agents/{id}/files               → Files Tab
/agents/{id}/skills              → Skills Tab
/agents/{id}/tools               → Tools Tab
/agents/{id}/channels            → Channels Tab
```

### Sidebar

`AgentsSidebar`:
- Top: **"+ New Agent"** button
- List: Each agent shows `emoji + name`, default agent has star badge
- Bottom: Global default agent dropdown selector

### Content Area

`AgentsRouter` added to `MainContent` alongside Chat/Dashboard/Settings, using the same CSS display toggling pattern.

---

## 2. Data Model Extensions

### New Value Objects

```rust
/// Agent identity for display (core/src/config/types/agents_def.rs)
pub struct AgentIdentity {
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,       // URL or base64
    pub theme: Option<String>,        // tagline / theme color
}

/// Model configuration with fallback chain
pub struct AgentModelConfig {
    pub primary: String,
    pub fallbacks: Vec<String>,
}

/// Per-agent inference parameters
pub struct AgentParams {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
}
```

### AgentDefinition Changes

```rust
pub struct AgentDefinition {
    // Existing fields (retained)
    pub id: String,
    pub default: bool,
    pub name: Option<String>,
    pub profile: Option<String>,
    pub skills: Option<Vec<String>>,
    pub subagents: Option<SubagentPolicy>,

    // New fields
    pub identity: Option<AgentIdentity>,
    pub model_config: Option<AgentModelConfig>,
    pub params: Option<AgentParams>,
}
```

### Backward Compatibility

Existing `model: Option<String>` retained as `#[serde(alias)]`. Resolution order in `AgentDefinitionResolver`:
1. `model_config` present → use it
2. Only `model` present → convert to `AgentModelConfig { primary: model, fallbacks: [] }`
3. Both present → `model_config` wins

### TOML Example

```toml
[[agents.list]]
id = "coder"
name = "Code Master"
default = true

[agents.list.identity]
emoji = "🧑‍💻"
description = "Full-stack coding specialist"
theme = "Write clean, efficient code"

[agents.list.model_config]
primary = "claude-opus-4"
fallbacks = ["claude-sonnet-4"]

[agents.list.params]
temperature = 0.3
max_tokens = 8192
```

---

## 3. AgentManager Service Layer

Location: `core/src/config/agent_manager.rs`

### Interface

```rust
pub struct AgentManager {
    config_path: PathBuf,       // aleph.toml
    workspace_root: PathBuf,    // ~/.aleph/agents/
    trash_root: PathBuf,        // ~/.aleph/trash/
}

impl AgentManager {
    // CRUD
    pub fn list(&self) -> Result<Vec<AgentDefinition>>;
    pub fn get(&self, id: &str) -> Result<AgentDefinition>;
    pub fn create(&self, def: AgentDefinition) -> Result<()>;
    pub fn update(&self, id: &str, patch: AgentPatch) -> Result<()>;
    pub fn delete(&self, id: &str) -> Result<()>;
    pub fn set_default(&self, id: &str) -> Result<()>;

    // Workspace files
    pub fn list_files(&self, agent_id: &str) -> Result<Vec<WorkspaceFile>>;
    pub fn read_file(&self, agent_id: &str, filename: &str) -> Result<FileContent>;
    pub fn write_file(&self, agent_id: &str, filename: &str, content: &str) -> Result<()>;
    pub fn delete_file(&self, agent_id: &str, filename: &str) -> Result<()>;
}
```

### Key Behaviors

**Create**:
1. Validate id uniqueness (alphanumeric + hyphens, ≤32 chars)
2. Append `[[agents.list]]` entry via `toml_edit` (preserves comments/formatting)
3. Create workspace directory: `{workspace_root}/{id}/`
4. Initialize default SOUL.md template

**Delete**:
1. Reject if it's the only agent (at least one must remain)
2. Reject if it's the default agent (must switch default first)
3. Remove entry from `[[agents.list]]` via `toml_edit`
4. Move workspace to `{trash_root}/{id}_{timestamp}/`
5. Cascade: remove routing rules referencing this agent

**Update (AgentPatch)**:
```rust
pub struct AgentPatch {
    pub name: Option<String>,
    pub identity: Option<AgentIdentity>,
    pub model_config: Option<AgentModelConfig>,
    pub params: Option<AgentParams>,
    pub skills: Option<Vec<String>>,
    pub subagents: Option<SubagentPolicy>,
}
```

### TOML Edit Internals

```rust
// Internal methods — toml_edit not exposed
fn load_document(&self) -> Result<DocumentMut>;
fn save_document(&self, doc: &DocumentMut) -> Result<()>;  // atomic: tmp + rename
fn find_agent_index(&self, doc: &Document, id: &str) -> Option<usize>;
```

Atomic write: write to `aleph.toml.tmp`, then `fs::rename` for crash safety.

`ConfigWatcher` picks up the change automatically → hot-reload → `AgentDefinitionResolver` re-resolves.

---

## 4. Gateway RPC Methods

New handler: `core/src/gateway/handlers/agents.rs`

### Method Table

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `agents.list` | — | `{agents: AgentSummary[], default_id}` | Lightweight list |
| `agents.get` | `{id}` | `AgentDetail` | Full definition + resolved fields |
| `agents.create` | `{id, name?, identity?, ...}` | `{success, id}` | Create agent |
| `agents.update` | `{id, patch}` | `{success}` | Partial update |
| `agents.delete` | `{id}` | `{success}` | Delete + trash |
| `agents.set_default` | `{id}` | `{success}` | Switch default |
| `agents.files.list` | `{agent_id}` | `{files: WorkspaceFile[]}` | List workspace files |
| `agents.files.get` | `{agent_id, filename}` | `{content, modified_at}` | Read file |
| `agents.files.set` | `{agent_id, filename, content}` | `{success}` | Write file |
| `agents.files.delete` | `{agent_id, filename}` | `{success}` | Delete file |

### Response Types

```rust
pub struct AgentSummary {
    pub id: String,
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,       // Resolved primary model
    pub is_default: bool,
}

pub struct AgentDetail {
    pub definition: AgentDefinition,
    pub resolved_model: String,
    pub workspace_path: String,
    pub file_count: usize,
}

pub struct WorkspaceFile {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_at: i64,            // Unix timestamp
    pub is_bootstrap: bool,          // SOUL.md, AGENTS.md, etc.
}
```

### Event Broadcasting

All write operations broadcast via `GatewayEventBus`:

```rust
GatewayEvent::ConfigChanged(ConfigChangedEvent {
    section: Some("agents"),
    value: json!({"action": "created", "agent_id": "coder"}),
    ..
})
```

### Compatibility

- Existing `agent.run/status/cancel` handlers unchanged
- Existing `agent_config.*` handlers retained as-is for Phase 1

---

## 5. Panel UI Components

### File Structure

```
apps/panel/src/
├── views/agents/
│   ├── mod.rs              # AgentsView main frame + AgentsRouter
│   ├── overview.rs         # Overview Tab
│   ├── behavior.rs         # Behavior Tab (migrated from settings/agent.rs)
│   ├── files.rs            # Files Tab
│   ├── skills.rs           # Skills Tab
│   ├── tools.rs            # Tools Tab
│   └── channels.rs         # Channels Tab
├── components/
│   ├── agents_sidebar.rs   # Agent list sidebar
│   ├── bottom_bar.rs       # +Agents mode
│   └── mode_sidebar.rs     # +Agents branch
└── api/
    └── agents.rs           # agents.* RPC calls
```

### AgentsSidebar Layout

```
┌─────────────────────┐
│  [+ New Agent]      │
├─────────────────────┤
│  🧑‍💻 Code Master  ⭐ │
│  🔍 Researcher      │
│  📝 Writer          │
├─────────────────────┤
│  Default: [▼ dropdown]│
└─────────────────────┘
```

### AgentsView Layout

```
┌──────────────────────────────────────────┐
│  🧑‍💻 Code Master                 [Delete] │
├────────┬──────────┬───────┬───────┬──────┤
│Overview│ Behavior │ Files │Skills │Tools │Channels│
├──────────────────────────────────────────┤
│              (Tab Content)               │
└──────────────────────────────────────────┘
```

### Tab Details

| Tab | Content | RPC |
|-----|---------|-----|
| **Overview** | Identity editor, model selector (primary dropdown + fallbacks), params (temperature/max_tokens sliders), subagent policy | `agents.get`, `agents.update` |
| **Behavior** | Migrated FileOps / CodeExec / General sections | `agent_config.get/update` (Phase 1 reuse) |
| **Files** | File list (bootstrap badge) + markdown textarea editor + create/delete | `agents.files.*` |
| **Skills** | Global skill list + per-agent toggles + search filter | `agents.get/update` |
| **Tools** | Display current tool permissions (Phase 1 read-only from skills field) | `agents.get` |
| **Channels** | Bound channels list + add/remove binding rules | Reuse routing rules or new `agents.bindings.*` |

---

## 6. Migration & Boundaries

### Migration from Settings

**Remove**:
- `settings/agent.rs` → delete
- `settings/mod.rs` → remove `pub mod agent`, `pub use agent::AgentView`
- `SettingsRouter` → remove `/settings/agent` route
- Settings sidebar → remove Agent menu item

**Retain (Phase 1)**:
- `api/agent.rs` (`AgentConfigApi`) — Behavior Tab reuses it
- `agent_config.*` RPC handlers — Behavior Tab depends on them

### New Dependency

- `toml_edit` crate in `core/Cargo.toml`

### Explicit Non-Goals (Phase 1)

| Not doing | Reason |
|-----------|--------|
| Agent templates/presets | YAGNI |
| Agent clone/copy | Phase 2 |
| Agent import/export | Phase 2 |
| Drag-to-reorder | Unnecessary complexity |
| Agent run status in panel | Already in Dashboard Trace |
| Per-agent behavior config | Phase 1 shares global config; Phase 2 splits |
| `tools.catalog` RPC | Tools Tab Phase 1 shows skills field only |

### Implementation Phases

```
Phase 1: Infrastructure
  ├── Extend AgentDefinition (new fields + backward compat)
  ├── Implement AgentManager (toml_edit CRUD + workspace + trash)
  └── Add agents.* RPC handlers

Phase 2: Panel Navigation
  ├── PanelMode::Agents + BottomBar
  ├── AgentsSidebar
  ├── AgentsRouter + AgentsView main frame
  └── Remove Agent from Settings

Phase 3: Tab Implementation (parallelizable)
  ├── Overview Tab (identity + model + params)
  ├── Behavior Tab (migrate existing components)
  ├── Files Tab (editor + file management)
  ├── Skills Tab (per-agent toggles)
  ├── Tools Tab (Phase 1: display current config)
  └── Channels Tab (routing bindings + default selector)
```
