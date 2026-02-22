# Memory Workspace Design

> **Date**: 2026-02-22
> **Status**: Approved
> **Scope**: Memory system domain isolation via Workspace concept

---

## 1. Problem Statement

Aleph users may use the system across vastly different domains: cryptocurrency trading, novel writing, health consulting, etc. Each domain operates with independent "swarm agents" and produces domain-specific memories. Without isolation, memories from different domains pollute each other's retrieval results, leading to:

- Irrelevant context injection (trading signals appearing in novel-writing sessions)
- Reduced retrieval precision
- Inability to apply domain-specific configurations (decay policies, tools, providers)

## 2. Core Concept: Orthogonal Dual Dimensions

Workspace (domain isolation) and Namespace (access control) are two independent, orthogonal dimensions:

```
         namespace (who can see)
         ┌────────┬────────┬────────┐
         │ owner  │ guest  │ shared │
    ┌────┼────────┼────────┼────────┤
    │ ws │ my     │ guest  │ shared │
    │ A  │ trading│ trading│ trading│
w   │    │ memory │ memory │ knowledge│
o   ├────┼────────┼────────┼────────┤
r   │ ws │ my     │ guest  │ shared │
k   │ B  │ novel  │ novel  │ writing│
s   │    │ memory │ memory │ resources│
p   ├────┼────────┼────────┼────────┤
a   │ de │ daily  │ guest  │ general│
c   │ fa │ chat   │ daily  │ common │
e   │ ul │ memory │ memory │ sense  │
    │ t  │        │        │        │
    └────┴────────┴────────┴────────┘
```

- **workspace**: Answers "which domain does this memory belong to?"
- **namespace**: Answers "who owns/can access this memory?"

## 3. Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Relationship with namespace | Orthogonal dual dimensions | Clear separation of concerns; domain isolation vs access control are independent |
| Workspace entity model | Independent DDD Entity | Rich metadata, configuration, lifecycle management |
| Default behavior | Built-in "default" workspace | Zero friction; unclassified memories auto-categorized |
| Cross-workspace queries | Default isolation + explicit cross-domain | Safety by default, flexibility when needed |
| VFS relationship | Orthogonal independent | Workspace is a "filter", not a "directory"; same path can exist in different workspaces |
| Metadata storage | Facts table (fact_type=WorkspaceDefinition) | Reuse existing LanceDB infrastructure |
| Session relationship | One-to-one binding | Clear context boundaries; switching workspace = new session |
| Physical isolation | Column-level (Scalar Index) | Simplest; reuses all existing infrastructure |
| Configuration scope | Decay policy + Agent config + Tool whitelist | Full domain customization |

## 4. Data Model

### 4.1 Workspace Entity

```rust
pub struct Workspace {
    pub id: String,           // "ws-crypto-trading"
    pub name: String,         // "加密货币交易"
    pub description: String,
    pub icon: Option<String>, // "💰"
    pub config: WorkspaceConfig,
    pub is_default: bool,     // true for "default" workspace only
    pub is_archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct WorkspaceConfig {
    // Memory decay strategy
    pub decay_rate: Option<f32>,
    pub permanent_fact_types: Vec<FactType>,

    // Agent configuration overrides
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub system_prompt_override: Option<String>,

    // Tool whitelist (None = all tools available)
    pub allowed_tools: Option<Vec<String>>,
}
```

### 4.2 Workspace Storage as Fact

Workspace metadata is stored as a special Fact in the facts table:

```
fact_type:  "workspace_definition"
path:       "aleph://system/workspaces/ws-crypto-trading"
workspace:  "default"           // metadata itself belongs to default
namespace:  "owner"
content:    { JSON-serialized Workspace struct }
```

### 4.3 WorkspaceFilter

```rust
pub enum WorkspaceFilter {
    Single(String),           // Single workspace
    Multiple(Vec<String>),    // Explicit multi-workspace query
    All,                      // All workspaces ("*")
}
```

## 5. Schema Changes

All four LanceDB tables gain a `workspace` column:

### 5.1 facts table (24 → 25 columns)

```
+ workspace: String  // Scalar Index, default "default"
```

### 5.2 memories table (10 → 11 columns)

```
+ workspace: String  // Scalar Index, default "default"
```

### 5.3 graph_nodes table (8 → 9 columns)

```
+ workspace: String  // Scalar Index, default "default"
```

### 5.4 graph_edges table (11 → 12 columns)

```
+ workspace: String  // Scalar Index, default "default"
```

## 6. Domain Model Changes

### 6.1 MemoryFact — Add workspace and namespace fields

```rust
pub struct MemoryFact {
    // ... existing fields ...
    pub namespace: String,   // Promoted from DB-only to domain model
    pub workspace: String,   // New field
}
```

### 6.2 MemoryEntry — Add workspace and namespace fields

```rust
pub struct MemoryEntry {
    // ... existing fields ...
    pub namespace: String,   // Promoted from DB-only to domain model
    pub workspace: String,   // New field
}
```

### 6.3 SearchFilter — Add workspace field

```rust
pub struct SearchFilter {
    pub namespace: Option<NamespaceScope>,
    pub workspace: Option<WorkspaceFilter>,  // New
    // ... existing fields ...
}
```

### 6.4 MemoryFilter — Add workspace field

```rust
pub struct MemoryFilter {
    pub namespace: Option<NamespaceScope>,
    pub workspace: Option<WorkspaceFilter>,  // New
    // ... existing fields ...
}
```

## 7. Arrow Serialization Fix

Current hardcoded namespace in `arrow_convert.rs` must be fixed:

```rust
// Before (hardcoded):
let namespace_arr = StringArray::from_iter_values(
    facts.iter().map(|_| "owner")
);

// After (from domain model):
let namespace_arr = StringArray::from_iter_values(
    facts.iter().map(|f| f.namespace.as_str())
);
let workspace_arr = StringArray::from_iter_values(
    facts.iter().map(|f| f.workspace.as_str())
);
```

## 8. Store Trait Changes

### 8.1 Strategy: Minimal Signature Changes

Most `MemoryStore` methods already accept `SearchFilter`, which now includes `workspace`. Only methods without filter parameters need signature changes.

### 8.2 GraphStore — All methods gain workspace parameter

```rust
trait GraphStore {
    async fn upsert_node(&self, node: &GraphNode, workspace: &str) -> Result<()>;
    async fn get_node(&self, name: &str, workspace: &str) -> Result<Option<GraphNode>>;
    async fn upsert_edge(&self, edge: &GraphEdge, workspace: &str) -> Result<()>;
    async fn get_edges_for_node(&self, node_id: &str, workspace: &str) -> Result<Vec<GraphEdge>>;
    async fn resolve_entity(&self, name: &str, workspace: &str) -> Result<Option<GraphNode>>;
    async fn count_edges_in_context(&self, context_key: &str, workspace: &str) -> Result<usize>;
    async fn apply_decay(&self, workspace: &str) -> Result<()>;
}
```

### 8.3 MemoryStore — list_by_path gains workspace parameter

```rust
trait MemoryStore {
    async fn list_by_path(
        &self,
        parent_path: &str,
        ns: &NamespaceScope,
        workspace: &str,    // New parameter
    ) -> Result<Vec<PathEntry>>;
    // Other methods: workspace flows through SearchFilter or MemoryFact.workspace
}
```

## 9. Runtime Context

### 9.1 WorkspaceContext

```rust
pub struct WorkspaceContext {
    pub workspace_id: String,
    pub namespace: NamespaceScope,
}

impl WorkspaceContext {
    pub fn from_session(session: &Session) -> Self { ... }

    pub fn to_search_filter(&self) -> SearchFilter {
        SearchFilter {
            workspace: Some(WorkspaceFilter::Single(self.workspace_id.clone())),
            namespace: Some(self.namespace.clone()),
            ..Default::default()
        }
    }
}
```

### 9.2 Flow Through Agent Loop

```
Session creation
  │
  ├─ workspace_id = "crypto-trading" (user-specified or "default")
  │
  ▼
WorkspaceContext { workspace_id, namespace }
  │
  ├─→ Agent Loop (observe/think/act)
  │     └─ Every memory operation implicitly injects workspace
  │
  ├─→ Thinker (LLM calls)
  │     └─ Load workspace's system_prompt_override
  │     └─ Use workspace's default_provider/model
  │
  ├─→ Dispatcher (tool orchestration)
  │     └─ Filter available tools by workspace.config.allowed_tools
  │
  └─→ Memory operations
        └─ All insert/search auto-inject workspace
```

## 10. Gateway RPC Methods

### 10.1 Workspace Management

```jsonc
// Create workspace
{ "method": "workspace.create", "params": {
    "name": "加密货币交易",
    "description": "比特币、以太坊交易策略",
    "icon": "💰",
    "config": {
      "decay_rate": 0.05,
      "default_provider": "deepseek",
      "allowed_tools": ["web_search", "calculator"]
    }
}}

// List all workspaces
{ "method": "workspace.list" }

// Get workspace details
{ "method": "workspace.get", "params": { "id": "ws-crypto-trading" }}

// Update workspace
{ "method": "workspace.update", "params": { "id": "...", "config": {...} }}

// Archive workspace
{ "method": "workspace.archive", "params": { "id": "..." }}

// Switch session workspace (creates new session)
{ "method": "workspace.switch", "params": { "workspace_id": "..." }}
```

### 10.2 Session Extension

```jsonc
// session.start gains workspace_id parameter
{ "method": "session.start", "params": {
    "workspace_id": "crypto-trading",  // New, defaults to "default"
    // ... existing params ...
}}
```

## 11. Migration Strategy

### Phase 1: Schema Migration

- Add `workspace` column to all 4 tables
- Set all existing data to `"default"`
- Create Scalar Index on workspace column

### Phase 2: Create Built-in Default Workspace

- Insert WorkspaceDefinition Fact:
  - path: `aleph://system/workspaces/default`
  - content: `{ name: "Default", is_default: true }`

### Phase 3: Fix Namespace Hardcoding

- `arrow_convert.rs`: Change hardcoded "owner" to read from domain model

### Backward Compatibility

- All existing data falls into `default` workspace — behavior unchanged
- API calls without workspace default to `default`
- Existing `NamespaceScope` functionality fully preserved
- `SearchFilter.workspace` is `Option` — `None` behaves as querying `default`

## 12. Affected Files

```
core/src/memory/
  ├─ context.rs              # MemoryFact, MemoryEntry: add fields
  ├─ namespace.rs            # Add WorkspaceFilter enum
  ├─ workspace.rs            # 【New】Workspace Entity, WorkspaceConfig
  ├─ store/
  │   ├─ mod.rs              # Store traits: GraphStore signature changes
  │   ├─ types.rs            # SearchFilter, MemoryFilter: add workspace
  │   └─ lance/
  │       ├─ schema.rs       # All 4 table schemas: add workspace column
  │       ├─ arrow_convert.rs # Fix hardcoding + add workspace serialization
  │       ├─ facts.rs        # list_by_path etc: adapt
  │       ├─ graph.rs        # All methods: add workspace parameter
  │       └─ memories.rs     # Adapt workspace field
  ├─ retrieval.rs            # Inject workspace filter
  ├─ fact_retrieval.rs       # Adapt workspace
  ├─ hybrid_retrieval/       # Adapt workspace
  ├─ dreaming.rs             # DreamDaemon workspace-aware
  ├─ compression/            # Compression workspace-aware
  ├─ evolution/              # Contradiction detection workspace-aware
  ├─ ripple/                 # Graph exploration workspace-aware
  └─ vfs/                    # VFS browsing workspace-aware

core/src/gateway/
  ├─ handlers/               # New workspace.* RPC handlers
  └─ session.rs              # Session: add workspace_id field

core/src/agent_loop/         # WorkspaceContext injection
core/src/thinker/            # Workspace config overrides
core/src/dispatcher/         # Tool whitelist filtering
```

## 13. Testing Strategy

### Unit Tests

- Workspace CRUD (create/get/list/update/archive)
- SearchFilter workspace filter generation
- Arrow serialization/deserialization (workspace field)
- WorkspaceContext construction and propagation
- Default behavior (None workspace → "default")

### Integration Tests

- Cross-workspace isolation (ws-A data doesn't appear in ws-B queries)
- Cross-domain queries (explicit multi-workspace)
- Global queries (workspace = "*")
- Graph isolation (graph_nodes/edges isolated by workspace)
- Session-Workspace binding validation

### Migration Tests

- Existing data migrated to "default" workspace
- Pre/post migration query result consistency

## 14. Out of Scope (YAGNI)

- Cross-workspace data copy/move
- Workspace-level permissions (beyond namespace)
- Workspace templates
- Workspace analytics dashboard
- AI auto-classification of workspace
