# ConfigManager and Memory Namespace Design

**Date**: 2026-02-07  
**Version**: v1.0  
**Status**: Ready for Implementation

---

## Overview

This design implements two core foundational features for Personal AI Hub:

1. **ConfigManager (SDK)**: Client-side configuration management with 4-layer stack
2. **Memory Namespace**: Type-safe multi-user data isolation in VectorDatabase

## Architecture Decision: Hybrid Mode

### ConfigManager Philosophy

**Tier Classification**:
- **Tier 1** (Critical): Source of Truth = Gateway
  - Examples: `auth.identity`, `security.policies`, `tools.whitelist`
  - Behavior: Read-only on client, modifications via RPC
  
- **Tier 2** (Preferences): Source of Truth = Gateway (but cacheable)
  - Examples: `ui.theme`, `skills.enabled`, `shortcuts`
  - Behavior: Synced from server, can have local overrides
  
- **Tier 3** (Ephemeral): Source of Truth = Local SDK
  - Examples: `window.position`, `log.level`, `gateway.url`
  - Behavior: Never synced to server, local persistence only

**Configuration Stack** (Priority: High → Low):
```
Layer 3: Session Override (volatile, debug only)
    ↓
Layer 2: Server Synced (Tier 1/2 from Gateway)
    ↓
Layer 1: Local Persistent (Tier 2/3 from local file)
    ↓
Layer 0: Defaults (hardcoded in code)
```

### Memory Namespace Philosophy

**Type-Driven Security**:
- Use `NamespaceScope` enum instead of raw strings
- Enforce namespace at database layer (compile-time guarantee)
- Prevent parameter confusion via strong typing

**Logical Isolation** (Soft Partition):
- All facts in single table with `namespace` column
- Owner can query across all namespaces
- Guest restricted to `guest:<id>` via SQL filter

---

## Design Part 1: Overall Architecture

```
┌─────────────────────────────────────────────────┐
│              Client (SDK)                        │
│  ┌──────────────────────────────────────┐       │
│  │   ConfigManager                      │       │
│  │  - Local: theme, gateway_url         │       │
│  │  - Server: identity_map, policies ───┼───┐   │
│  │  - Session: log_level (temp)         │   │   │
│  └──────────────────────────────────────┘   │   │
└───────────────────────────────────────────┼──┼──┘
                                            │  │
                        WebSocket RPC       │  │
                        config.get          │  │
                        config.patch        │  │
                                            ▼  ▼
┌─────────────────────────────────────────────────┐
│              Gateway (Core)                      │
│  ┌──────────────────────────────────────┐       │
│  │   Config System                      │       │
│  │  - Load from ~/.aleph/config.toml    │       │
│  │  - Validate & Save                   │       │
│  │  - Broadcast config.changed event────┼───┐   │
│  └──────────────────────────────────────┘   │   │
│                                              │   │
│  ┌──────────────────────────────────────┐   │   │
│  │   VectorDatabase                     │   │   │
│  │  - search(scope: NamespaceScope) ────┼───┘   │
│  │  - insert(fact, namespace)           │       │
│  │  - SQL: WHERE namespace = ?          │       │
│  └──────────────────────────────────────┘       │
└─────────────────────────────────────────────────┘
```

---

## Design Part 2: NamespaceScope Type System

### Core Type Definition

**Location**: `core/src/memory/namespace.rs`

```rust
/// Namespace scope - Type-safe security boundary
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamespaceScope {
    /// Owner has global access
    Owner,
    /// Guest can only access their own namespace
    Guest(String),  // guest_id
    /// Shared public knowledge base (Phase 4.2+)
    Shared,
}

impl NamespaceScope {
    /// Convert to SQL WHERE clause
    pub fn to_sql_filter(&self) -> (String, Vec<String>) {
        match self {
            NamespaceScope::Owner => {
                ("1=1".to_string(), vec![])
            }
            NamespaceScope::Guest(id) => {
                ("namespace = ?".to_string(), vec![format!("guest:{}", id)])
            }
            NamespaceScope::Shared => {
                ("namespace = ?".to_string(), vec!["shared".to_string()])
            }
        }
    }
    
    /// Convert to namespace value for storage
    pub fn to_namespace_value(&self) -> String {
        match self {
            NamespaceScope::Owner => "owner".to_string(),
            NamespaceScope::Guest(id) => format!("guest:{}", id),
            NamespaceScope::Shared => "shared".to_string(),
        }
    }
    
    /// Construct from auth context (prevents bypass)
    pub fn from_auth_context(role: &Role, guest_id: Option<&str>) -> Result<Self, String> {
        match role {
            Role::Owner => Ok(NamespaceScope::Owner),
            Role::Guest => {
                let id = guest_id.ok_or("Guest role requires guest_id")?;
                Ok(NamespaceScope::Guest(id.to_string()))
            }
            Role::Anonymous => Err("Anonymous users cannot access memory".to_string()),
        }
    }
}
```

### VectorDatabase Integration

**Modified**: `core/src/memory/database/core.rs`

```rust
impl VectorDatabase {
    /// Semantic search (mandatory namespace filtering)
    pub async fn search(
        &self,
        query: &str,
        scope: NamespaceScope,  // Mandatory parameter
        limit: usize,
    ) -> Result<Vec<Fact>, DatabaseError> {
        let (filter, params) = scope.to_sql_filter();
        
        let sql = format!(
            "SELECT * FROM memory_facts 
             WHERE is_valid = 1 AND {}
             ORDER BY updated_at DESC 
             LIMIT ?",
            filter
        );
        
        // Execute query...
    }
    
    /// Insert fact (auto-set namespace)
    pub async fn insert_fact(
        &self,
        fact: &Fact,
        scope: NamespaceScope,
    ) -> Result<()> {
        let namespace = scope.to_namespace_value();
        
        let sql = "INSERT INTO memory_facts (..., namespace) VALUES (..., ?)";
        // Execute insert...
    }
}
```

**Key Points**:
- ✅ All queries require `NamespaceScope` - compiler enforced
- ✅ `from_auth_context()` is the only construction method
- ✅ SQL injection safe (parameterized queries)

---

## Design Part 3: Database Schema Migration

### Migration Strategy

**Location**: `core/src/memory/database/migration.rs`

```rust
/// Memory namespace migration (adds namespace column)
pub async fn migrate_add_namespace(conn: &Connection) -> Result<()> {
    // Step 1: Check if already migrated
    let has_namespace = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('memory_facts') WHERE name='namespace'",
        [],
        |row| row.get::<_, i64>(0)
    )?;
    
    if has_namespace > 0 {
        info!("Namespace column already exists, skipping migration");
        return Ok(());
    }
    
    info!("Starting namespace migration for memory_facts");
    
    // Step 2: Add namespace column (default 'owner')
    conn.execute(
        "ALTER TABLE memory_facts ADD COLUMN namespace TEXT NOT NULL DEFAULT 'owner'",
        []
    )?;
    
    // Step 3: Create indexes
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_facts_namespace ON memory_facts(namespace)",
        []
    )?;
    
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_facts_namespace_valid 
         ON memory_facts(namespace, is_valid)",
        []
    )?;
    
    info!("Namespace migration completed successfully");
    Ok(())
}
```

### Schema Update

**Existing Table**:
```sql
CREATE TABLE memory_facts (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    fact_type TEXT NOT NULL DEFAULT 'other',
    embedding BLOB,
    source_memory_ids TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    confidence REAL NOT NULL DEFAULT 1.0,
    is_valid INTEGER NOT NULL DEFAULT 1,
    -- ... other fields
);
```

**After Migration**:
```sql
CREATE TABLE memory_facts (
    -- ... all existing fields ...
    namespace TEXT NOT NULL DEFAULT 'owner'  -- NEW
);

-- New indexes
CREATE INDEX idx_facts_namespace ON memory_facts(namespace);
CREATE INDEX idx_facts_namespace_valid ON memory_facts(namespace, is_valid);
```

### Migration Invocation

Auto-execute in `VectorDatabase::new()`:

```rust
impl VectorDatabase {
    pub async fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        
        // Run all migrations
        migration::migrate_add_namespace(&conn).await?;
        
        Ok(Self { conn, ... })
    }
}
```

**Key Points**:
- ✅ Idempotent: safe to run multiple times
- ✅ Backward compatible: existing data defaults to 'owner'
- ✅ Performance optimized: compound index `(namespace, is_valid)`

---

## Design Part 4: ConfigManager Implementation

### SDK Structure

**Location**: `clients/shared/src/config/manager.rs`

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use serde_json::Value;
use tokio::sync::RwLock;

/// Configuration manager - 4-layer stack
pub struct ConfigManager {
    /// Layer 0: Hardcoded defaults
    defaults: HashMap<String, Value>,
    
    /// Layer 1: Local persistent config (Tier 2/3)
    local: RwLock<HashMap<String, Value>>,
    local_path: PathBuf,
    
    /// Layer 2: Server synced config (Tier 1)
    server: RwLock<HashMap<String, Value>>,
    
    /// Layer 3: Session temporary override
    session_override: RwLock<HashMap<String, Value>>,
    
    /// Change subscribers
    subscribers: RwLock<Vec<Box<dyn Fn(ConfigChangedEvent) + Send + Sync>>>,
}

impl ConfigManager {
    pub fn new(local_path: PathBuf) -> Self { ... }
    
    /// Load local config file
    pub async fn load_local(&self) -> Result<(), String> { ... }
    
    /// Sync from Gateway (called after connection)
    pub async fn sync_from_server(&self, server_config: HashMap<String, Value>) { ... }
    
    /// Get config value (4-layer priority)
    pub async fn get(&self, key: &str) -> Option<Value> {
        // Layer 3: Session override
        if let Some(v) = self.session_override.read().await.get(key) {
            return Some(v.clone());
        }
        
        // Layer 2: Server synced
        if let Some(v) = self.server.read().await.get(key) {
            return Some(v.clone());
        }
        
        // Layer 1: Local persistent
        if let Some(v) = self.local.read().await.get(key) {
            return Some(v.clone());
        }
        
        // Layer 0: Defaults
        self.defaults.get(key).cloned()
    }
    
    /// Set local config and persist
    pub async fn set_local(&self, key: &str, value: Value) -> Result<(), String> { ... }
    
    /// Set session temporary override
    pub async fn set_session(&self, key: &str, value: Value) -> Result<(), String> {
        // Security check: prevent Tier 1 override
        if is_tier1_key(key) {
            return Err(format!("Cannot override Tier 1 config: {}", key));
        }
        
        self.session_override.write().await.insert(key.to_string(), value);
        Ok(())
    }
    
    /// Clear all session overrides
    pub async fn clear_session_overrides(&self) { ... }
    
    /// Subscribe to config changes
    pub async fn subscribe<F>(&self, callback: F) { ... }
}

/// Check if key is Tier 1
fn is_tier1_key(key: &str) -> bool {
    key.starts_with("auth.") 
        || key.starts_with("security.") 
        || key.starts_with("identity.")
}
```

**Key Points**:
- ✅ 4-layer config stack with clear priority
- ✅ Async-safe (RwLock)
- ✅ Local persistence (JSON format)
- ✅ Event subscription mechanism

---

## Design Part 5: Gateway RPC Handlers

### RPC Methods

**Location**: `core/src/gateway/handlers/config.rs` (extend existing)

```rust
/// config.get - Get full config (called after client connects)
pub async fn handle_get_full_config(
    req: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let config_snapshot = config.read().await.clone();
    
    // Convert to JSON (Tier 1/2 only)
    let config_json = serde_json::to_value(&config_snapshot)
        .unwrap_or(Value::Object(serde_json::Map::new()));
    
    JsonRpcResponse::success(req.id, json!({
        "config": config_json
    }))
}

/// config.patch - Apply config changes
pub async fn handle_patch_config(
    req: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let patch: HashMap<String, Value> = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INVALID_PARAMS, 
                format!("Invalid patch format: {}", e));
        }
    };
    
    // Apply patch and save
    {
        let mut cfg = config.write().await;
        // Apply changes...
        cfg.save().await?;
    }
    
    // Broadcast change event
    event_bus.publish(GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: None,
        value: Value::Object(patch.clone().into_iter().collect()),
        timestamp: chrono::Utc::now().timestamp(),
    })).await;
    
    JsonRpcResponse::success(req.id, json!({ "status": "ok" }))
}
```

### Event Broadcasting

**Use existing GatewayEventBus**:

```rust
// In core/src/gateway/event_bus.rs
#[derive(Debug, Clone, Serialize)]
pub enum GatewayEvent {
    // ... existing events ...
    ConfigChanged(ConfigChangedEvent),
}
```

**Client subscription**:

```rust
// SDK subscribes via WebSocket
pub async fn subscribe_config_changes(&self, config_manager: Arc<ConfigManager>) {
    self.websocket.subscribe("config.changed", move |event: ConfigChangedEvent| {
        let config_manager = config_manager.clone();
        tokio::spawn(async move {
            config_manager.sync_from_server(...).await;
            config_manager.notify_subscribers(event).await;
        });
    }).await;
}
```

---

## Design Part 6: Testing and Acceptance Criteria

### Memory Namespace Tests

**Unit Tests**: `core/src/memory/namespace.rs`

```rust
#[test]
fn test_owner_scope_no_filter() {
    let scope = NamespaceScope::Owner;
    let (filter, params) = scope.to_sql_filter();
    assert_eq!(filter, "1=1");
    assert!(params.is_empty());
}

#[test]
fn test_guest_scope_filters_correctly() {
    let scope = NamespaceScope::Guest("abc-123".to_string());
    let (filter, params) = scope.to_sql_filter();
    assert_eq!(filter, "namespace = ?");
    assert_eq!(params, vec!["guest:abc-123"]);
}

#[test]
fn test_from_auth_context_guest_requires_id() {
    let result = NamespaceScope::from_auth_context(&Role::Guest, None);
    assert!(result.is_err());
}
```

**Integration Tests**: `core/tests/memory_namespace_isolation.rs`

```rust
#[tokio::test]
async fn test_guest_cannot_read_owner_facts() {
    let db = create_test_db().await;
    
    db.insert_fact(&fact1, NamespaceScope::Owner).await.unwrap();
    
    let results = db.search("test", NamespaceScope::Guest("guest-1".into()), 10)
        .await.unwrap();
    
    assert_eq!(results.len(), 0);
}

#[tokio::test]
async fn test_owner_can_read_all_namespaces() {
    let db = create_test_db().await;
    
    db.insert_fact(&fact1, NamespaceScope::Owner).await.unwrap();
    db.insert_fact(&fact2, NamespaceScope::Guest("guest-1".into())).await.unwrap();
    
    let results = db.search("test", NamespaceScope::Owner, 10).await.unwrap();
    
    assert_eq!(results.len(), 2);
}
```

### ConfigManager Tests

**Unit Tests**: `clients/shared/src/config/manager.rs`

```rust
#[tokio::test]
async fn test_config_priority_layers() {
    let manager = ConfigManager::new(PathBuf::from("/tmp/test_config.json"));
    
    // Layer 0: Default
    let theme = manager.get("ui.theme").await;
    assert_eq!(theme, Some(Value::String("system".to_string())));
    
    // Layer 1: Local override
    manager.set_local("ui.theme", Value::String("dark".into())).await.unwrap();
    let theme = manager.get("ui.theme").await;
    assert_eq!(theme, Some(Value::String("dark".to_string())));
    
    // Layer 3: Session override
    manager.set_session("ui.theme", Value::String("light".into())).await.unwrap();
    let theme = manager.get("ui.theme").await;
    assert_eq!(theme, Some(Value::String("light".to_string())));
}

#[tokio::test]
async fn test_tier1_cannot_be_overridden() {
    let manager = ConfigManager::new(PathBuf::from("/tmp/test_config.json"));
    
    let result = manager.set_session("auth.token", Value::String("fake".into())).await;
    assert!(result.is_err());
}
```

### Acceptance Criteria

**Scenario 1: Memory Namespace Isolation**
```
Given: 
  - Owner created 3 facts
  - Guest "alice" created 2 facts
When:
  - Guest "alice" executes search()
Then:
  - Returns only her 2 facts
  - Owner's facts are invisible
```

**Scenario 2: Config Sync**
```
Given:
  - macOS Client connects to Gateway
  - Gateway has config { "ui.theme": "dark" }
When:
  - macOS Client calls sync_from_server()
Then:
  - config_manager.get("ui.theme") returns "dark"
  - Local subscribers receive ConfigChangedEvent
```

**Scenario 3: Migration Idempotency**
```
Given:
  - memory_facts table has 100 rows (no namespace column)
When:
  - Run migrate_add_namespace() first time
Then:
  - namespace column added, all data defaults to "owner"
When:
  - Run migrate_add_namespace() second time
Then:
  - No duplicate column, data intact
```

**Key Metrics**:
- ✅ 100% of VectorDatabase query methods require NamespaceScope
- ✅ After migration, old data is queryable
- ✅ ConfigManager's 4-layer priority works correctly
- ✅ Tier 1 config cannot be session-overridden

---

## Implementation Checklist

### Phase 1: Memory Namespace (Week 1)
- [ ] Create `core/src/memory/namespace.rs` with NamespaceScope
- [ ] Add migration logic in `core/src/memory/database/migration.rs`
- [ ] Update VectorDatabase methods to require NamespaceScope
- [ ] Update all call sites in `memory_ops.rs`
- [ ] Write unit tests for NamespaceScope
- [ ] Write integration tests for isolation

### Phase 2: ConfigManager SDK (Week 2)
- [ ] Create `clients/shared/src/config/manager.rs`
- [ ] Implement 4-layer config stack
- [ ] Add local persistence (JSON)
- [ ] Add subscription mechanism
- [ ] Write unit tests for ConfigManager

### Phase 3: Gateway Integration (Week 3)
- [ ] Add `config.get` RPC handler
- [ ] Add `config.patch` RPC handler
- [ ] Wire handlers in Gateway startup
- [ ] Add ConfigChanged to GatewayEventBus
- [ ] Test config sync end-to-end

### Phase 4: Integration Testing (Week 4)
- [ ] Test memory isolation scenarios
- [ ] Test config sync scenarios
- [ ] Test migration idempotency
- [ ] Performance benchmarks
- [ ] Documentation updates

---

## References

- [Personal AI Hub Architecture](2026-02-06-personal-ai-hub-architecture.md)
- [Memory Namespace Design](../memory/NAMESPACE_DESIGN.md)
- [Server-Client Architecture](2026-02-06-server-client-architecture-design.md)
