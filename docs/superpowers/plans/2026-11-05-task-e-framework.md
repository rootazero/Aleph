# Task E: SessionCapability Registration - Implementation Framework

**Status:** Preparation complete, waiting for Task B design

---

## Implementation Pattern (from ToolBackendAdapter analysis)

```rust
// Pattern discovered in src/capability/backend.rs + src/tools/registry.rs

pub struct SessionCapabilityBackend {
    // Single source of truth for session capabilities
    registry: /* Session registry */,
}

impl CapabilityBackend for SessionCapabilityBackend {
    fn lookup(&self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        // Return descriptor if session exists
    }
    
    fn enumerate(&self, scope: &Scope) -> Vec<CapabilityDescriptor> {
        // List all active sessions matching scope
    }
    
    fn snapshot(&self, scope: &Scope) -> CapabilitySnapshot {
        // Atomic snapshot of all sessions + revision
    }
}
```

---

## Key Files to Modify

1. **Create new file:**
   - `src/capability/session_backend.rs` - SessionCapabilityBackend impl
   - `desktop/nsh-shell/src/capability.rs` - NSH-specific Capability registration

2. **Modify existing:**
   - `src/capability/mod.rs` - export session_backend module
   - `src/bin/aleph-server/commands/start/mod.rs` - mount SessionCapabilityBackend

---

## Waiting For

- **Task B**: SessionCapability design spec (CapabilityKind, Facets, Lifetime)
- **Task C**: Server RPC plan (affects capability methods)
- **Task D**: UI implementation (can proceed in parallel)

---

## Next Steps (after subagent deliverables)

1. Read Task B design doc
2. Implement SessionCapabilityBackend 
3. Write integration tests
4. Verify with graphify query
