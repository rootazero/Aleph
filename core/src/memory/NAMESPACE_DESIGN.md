# Memory Namespace Design (Personal AI Hub - Phase 4)

## Overview

The namespace column in the `memory_facts` table enables **multi-user data isolation** in Personal AI Hub. It allows the Owner and multiple Guests to maintain separate, private knowledge bases while sharing a single Aleph instance.

## Schema Design

### Table: `memory_facts` (with namespace support)

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
    invalidation_reason TEXT,
    specificity TEXT NOT NULL DEFAULT 'pattern',
    temporal_scope TEXT NOT NULL DEFAULT 'contextual',
    decay_invalidated_at INTEGER,
    namespace TEXT NOT NULL DEFAULT 'owner'  -- NEW COLUMN
);

-- Indexes for namespace filtering
CREATE INDEX idx_facts_namespace ON memory_facts(namespace);
CREATE INDEX idx_facts_namespace_valid ON memory_facts(namespace, is_valid);
```

### Namespace Values

| Namespace | Purpose | Access | Example |
|-----------|---------|--------|---------|
| `owner` | Owner's private facts | Owner only | Default for new facts |
| `guest:<guest_id>` | Guest's private facts | That guest + Owner | `guest:abc-123-def` |
| `shared` | Shared facts (future) | Based on ACL rules | Phase 4.2+ |

## Access Control Matrix

| User | Can Read | Can Create | Notes |
|------|----------|-----------|-------|
| **Owner** | All namespaces | Any namespace | Full system control |
| **Guest** | `guest:<their_id>` only | `guest:<their_id>` only | Isolated knowledge base |
| **Anonymous** | None | None | Denied access |

## Query Patterns

### For Owner (reads all facts)

```sql
-- Simple approach: read all
SELECT * FROM memory_facts WHERE is_valid = 1
ORDER BY updated_at DESC;

-- Explicit approach: show all accessible namespaces
SELECT * FROM memory_facts
WHERE is_valid = 1
  AND (namespace = 'owner'
       OR namespace LIKE 'guest:%'
       OR namespace = 'shared')
ORDER BY updated_at DESC;

-- Count facts by namespace
SELECT namespace, COUNT(*) as count
FROM memory_facts
WHERE is_valid = 1
GROUP BY namespace;
```

### For Guest (reads only their facts)

```sql
-- Read only their private facts
SELECT * FROM memory_facts
WHERE namespace = 'guest:abc-123-def'
  AND is_valid = 1
ORDER BY updated_at DESC;

-- Future: include shared facts (Phase 4.2)
SELECT * FROM memory_facts
WHERE (namespace = 'guest:abc-123-def' OR namespace = 'shared')
  AND is_valid = 1
ORDER BY updated_at DESC;
```

### For Compression (by user)

```sql
-- Compress facts for owner
INSERT INTO memory_facts (
    id, content, fact_type, embedding, source_memory_ids,
    created_at, updated_at, confidence, is_valid, specificity,
    temporal_scope, namespace
) VALUES (
    'fact-123', 'User likes coffee', 'preference', ...,
    now(), now(), 1.0, 1, 'pattern', 'contextual', 'owner'
);

-- Compress facts for guest
INSERT INTO memory_facts (
    ..., namespace
) VALUES (
    ..., 'guest:alice'
);
```

### For Retention Cleanup (per namespace)

```sql
-- Delete expired owner facts
DELETE FROM memory_facts
WHERE namespace = 'owner'
  AND created_at < (now - retention_days);

-- Delete expired guest facts (isolation maintained)
DELETE FROM memory_facts
WHERE namespace = 'guest:alice'
  AND created_at < (now - retention_days);
```

## Implementation Phases

### Phase 4.1: Schema Documentation (CURRENT)
- Add `namespace TEXT NOT NULL DEFAULT 'owner'` column
- Add indexes: `idx_facts_namespace`, `idx_facts_namespace_valid`
- Document namespace semantics and query patterns
- **No data migration needed** (default to 'owner' for backward compatibility)

### Phase 4.2: Query Integration (Future)
- Update `get_all_facts()` to filter by user's namespace
- Update `search_facts()` to filter by namespace before vector search
- Update compression to set namespace based on current user
- Add `get_guest_facts(guest_id)` method for admin views

### Phase 4.3: Guest Management Integration (Future)
- `InvitationManager` creates guest with UUID
- Guest activation sets namespace = `guest:<guest_uuid>`
- `PolicyEngine` checks role + namespace during queries
- Gateway router passes user context to memory system

### Phase 4.4: Sharing Layer (Future)
- Implement `shared` namespace with ACL table
- Owner can mark facts as shareable
- Guest can optionally see shared facts
- Audit log tracks who accessed shared facts

## Data Isolation Guarantees

### Strong Isolation (Current)
- Each guest's facts are completely isolated
- Owner can see all facts but cannot accidentally share
- No cross-namespace data leakage possible

### Weak Isolation (After Phase 4.4)
- Owner explicitly marks facts as `shared`
- Guest can optionally retrieve shared facts
- ACL table controls who sees what

## Migration Path

### Step 1: Add Column (Phase 4.1)
```sql
ALTER TABLE memory_facts ADD COLUMN namespace TEXT NOT NULL DEFAULT 'owner';
CREATE INDEX idx_facts_namespace ON memory_facts(namespace);
CREATE INDEX idx_facts_namespace_valid ON memory_facts(namespace, is_valid);
```

**Note:** All existing facts default to `'owner'`, so no data transformation needed.

### Step 2: Update Code (Phase 4.2)
```rust
// Before: All facts are owner's
pub async fn get_all_facts(&self) -> Result<Vec<MemoryFact>, AlephError> {
    // ... select from memory_facts ...
}

// After: Filter by namespace
pub async fn get_all_facts(&self, user_id: &UserId) -> Result<Vec<MemoryFact>, AlephError> {
    let namespace = match user_id {
        UserId::Owner => "owner",  // Owner sees all
        UserId::Guest(id) => format!("guest:{}", id),
    };
    // ... select from memory_facts where namespace ...
}
```

### Step 3: InvitationManager Integration (Phase 4.3)
```rust
// When guest is created
let guest_id = Uuid::new_v4().to_string();
let namespace = format!("guest:{}", guest_id);

// When facts are created for this guest
fact.namespace = Some(namespace);
```

## Performance Considerations

### Index Strategy

| Index | Purpose | Used By |
|-------|---------|---------|
| `idx_facts_namespace` | Single-user fact queries | Get all facts for user |
| `idx_facts_namespace_valid` | Combined filter | Get valid facts for user |
| `idx_facts_type` | Fact type filtering | Fact categorization |
| `idx_facts_updated` | Time-based queries | Sorting, retention |
| `idx_facts_decay_invalidated` | Recycle bin queries | Decay engine |

### Query Performance

**Without namespace filtering:**
- All queries scan entire `memory_facts` table
- As guest count increases, performance degrades
- Expected: O(n) where n = total facts in system

**With namespace filtering:**
- Queries hit `idx_facts_namespace` first
- Reduces result set to single user's facts
- Expected: O(log n + m) where m = user's facts
- Much faster for large deployments

## Testing Strategy

### Unit Tests (schema level)
```rust
#[test]
fn test_facts_table_has_namespace_column() {
    // Verify column exists and defaults to 'owner'
}

#[test]
fn test_namespace_indexes_created() {
    // Verify both indexes exist for query optimization
}
```

### Integration Tests (Phase 4.2+)
```rust
#[test]
async fn test_guest_cannot_see_owner_facts() {
    // Create owner fact in 'owner' namespace
    // Create guest fact in 'guest:alice' namespace
    // Query as guest, verify only guest fact returned
}

#[test]
async fn test_owner_sees_all_facts() {
    // Create facts in multiple namespaces
    // Query as owner, verify all returned
}
```

## Future Extensions

### Multi-Tenant Support (Phase 10)
- Add `tenant_id` column
- Namespace becomes: `<tenant>/<role>/<user_id>`
- Example: `acme/owner`, `acme/guest:alice`, `others/owner`

### Data Residency Compliance (Phase 11)
- Store namespace's location preference
- Example: `guest:alice@eu`, `guest:bob@us`
- Enforce data stays in specified region

### Fact Expiration per Role (Phase 12)
- Owner's facts expire after 365 days
- Guest facts expire after 30 days
- Retention policy is per-namespace

## References

- **InvitationManager**: `core/src/gateway/security/invitation_manager.rs`
- **PolicyEngine**: `core/src/gateway/security/policy_engine.rs`
- **Schema**: `core/src/memory/database/core.rs:fn schema_sql()`
- **CRUD Operations**: `core/src/memory/database/facts/crud.rs`
- **Personal AI Hub Plan**: `docs/plans/2026-02-06-personal-ai-hub-implementation.md`
