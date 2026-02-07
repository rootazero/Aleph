# Identity Context & Security Enforcement Design

> **Status:** Approved for Implementation
> **Date:** 2026-02-07
> **Architect:** System Architect + Claude

---

## Executive Summary

This design implements **stateless security enforcement** for Aleph's Owner+Guest model by:

1. **IdentityContext** - Immutable identity snapshot carried through execution
2. **PolicyEngine Refactoring** - Stateless pure functions for permission checks
3. **CLI guests Command** - Testing harness for end-to-end validation

### Architecture Philosophy

**"Certificate of Authority"** - IdentityContext acts as a frozen credential snapshot, enabling:
- Deterministic audit trails (evidence == context)
- Race-condition-free execution (permissions locked at request time)
- Distributed system readiness (no state synchronization required)

---

## Part 1: IdentityContext Data Model

### Core Type Definition

```rust
// shared/protocol/src/auth.rs

/// Identity context for a single execution request
///
/// This is an immutable snapshot of the caller's identity and permissions
/// at the moment the request was made. It serves as audit evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityContext {
    /// Unique identifier for this request (for log correlation)
    pub request_id: String,

    /// Session key that originated this request
    pub session_key: String,

    /// User's role (Owner, Guest, Anonymous)
    pub role: Role,

    /// Specific identity ID
    /// - "owner" for Role::Owner
    /// - guest_id (UUID) for Role::Guest
    /// - "anonymous" for Role::Anonymous
    pub identity_id: String,

    /// Guest scope (only present for Role::Guest)
    /// This is the permission snapshot frozen at request time
    pub scope: Option<GuestScope>,

    /// Timestamp when this context was created (Unix seconds)
    pub created_at: i64,

    /// Source channel (e.g., "telegram", "cli", "websocket")
    pub source_channel: String,
}

impl IdentityContext {
    /// Create an Owner identity context
    pub fn owner(session_key: String, source_channel: String) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role: Role::Owner,
            identity_id: "owner".to_string(),
            scope: None,
            created_at: now_unix_timestamp(),
            source_channel,
        }
    }

    /// Create a Guest identity context
    pub fn guest(
        session_key: String,
        guest_id: String,
        scope: GuestScope,
        source_channel: String,
    ) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role: Role::Guest,
            identity_id: guest_id,
            scope: Some(scope),
            created_at: now_unix_timestamp(),
            source_channel,
        }
    }

    /// Create an Anonymous identity context (always denied)
    pub fn anonymous(session_key: String, source_channel: String) -> Self {
        Self {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role: Role::Anonymous,
            identity_id: "anonymous".to_string(),
            scope: None,
            created_at: now_unix_timestamp(),
            source_channel,
        }
    }
}
```

### Session Storage Strategy

**Approach:** Extend existing `sessions.metadata` field (TEXT) to store `SessionIdentityMeta` JSON.

```rust
// core/src/gateway/session_manager.rs

/// Session identity metadata stored in database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIdentityMeta {
    /// Role of the session owner
    pub role: Role,

    /// Identity ID ("owner" or guest_id)
    pub identity_id: String,

    /// Guest scope (frozen at session creation)
    pub scope: Option<GuestScope>,

    /// Source channel
    pub source_channel: String,

    /// Custom metadata (preserved from old format)
    #[serde(flatten)]
    pub custom: HashMap<String, serde_json::Value>,
}

impl Default for SessionIdentityMeta {
    fn default() -> Self {
        Self {
            role: Role::Owner,  // Default to Owner for backward compatibility
            identity_id: "owner".to_string(),
            scope: None,
            source_channel: "unknown".to_string(),
            custom: HashMap::new(),
        }
    }
}
```

**Migration Strategy:**
- Existing sessions with `metadata=NULL` or unparseable JSON → use `Default::default()`
- No schema change required (TEXT field already exists)
- Backward compatible

---

## Part 2: PolicyEngine Stateless Refactoring

### Before (Stateful)

```rust
pub struct PolicyEngine {
    guest_scopes: DashMap<String, GuestScope>,  // ❌ Internal state
}

impl PolicyEngine {
    pub fn check_tool_permission(
        &self,
        role: &Role,
        guest_id: Option<&str>,
        tool_name: &str,
    ) -> PermissionResult {
        // ❌ Queries internal DashMap
    }
}
```

### After (Stateless)

```rust
// core/src/gateway/security/policy_engine.rs

/// Stateless policy engine for tool permission evaluation
///
/// All permission checks are pure functions based on IdentityContext.
/// No internal state is maintained.
pub struct PolicyEngine;

impl PolicyEngine {
    /// Check if identity can execute a tool (pure function)
    pub fn check_tool_permission(
        identity: &IdentityContext,
        tool_name: &str,
    ) -> PermissionResult {
        match identity.role {
            Role::Owner => PermissionResult::Allowed,

            Role::Guest => {
                let Some(ref scope) = identity.scope else {
                    return PermissionResult::Denied {
                        reason: format!(
                            "Guest '{}' has no permission scope",
                            identity.identity_id
                        ),
                    };
                };

                // Check expiration
                if let Some(expires_at) = scope.expires_at {
                    if identity.created_at > expires_at {
                        return PermissionResult::Denied {
                            reason: "Guest token expired".to_string(),
                        };
                    }
                }

                Self::check_guest_scope(scope, tool_name, &identity.identity_id)
            }

            Role::Anonymous => PermissionResult::Denied {
                reason: "Authentication required".to_string(),
            },
        }
    }

    /// Check if a tool is allowed by guest scope (pure function)
    fn check_guest_scope(
        scope: &GuestScope,
        tool_name: &str,
        guest_id: &str,
    ) -> PermissionResult {
        let tool_category = tool_name.split(':').next().unwrap_or(tool_name);

        let allowed = scope.allowed_tools.iter().any(|allowed| {
            allowed == tool_name       // Exact match
            || allowed == tool_category // Category match
            || allowed == "*"           // Wildcard
        });

        if allowed {
            PermissionResult::Allowed
        } else {
            PermissionResult::Denied {
                reason: format!(
                    "Tool '{}' not in guest '{}' scope (allowed: {:?})",
                    tool_name, guest_id, scope.allowed_tools
                ),
            }
        }
    }
}
```

### Removed APIs

- `PolicyEngine::new()` → Marked as `#[deprecated]`, returns empty struct
- `PolicyEngine::set_guest_scope()` → Deleted (scope now in Session)
- `PolicyEngine::remove_guest_scope()` → Deleted

---

## Part 3: CLI `guests` Command

### Command Structure

```rust
// clients/cli/src/main.rs

#[derive(Subcommand)]
enum Commands {
    /// Manage guest invitations and permissions
    Guests {
        #[command(subcommand)]
        action: GuestsAction,
    },
}
```

### Subcommands

```rust
// clients/cli/src/commands/guests.rs

#[derive(Subcommand)]
pub enum GuestsAction {
    /// Create a new guest invitation
    Invite {
        #[arg(short, long)]
        name: String,

        #[arg(short, long)]
        tools: String,  // Comma-separated

        #[arg(long)]
        ttl: Option<String>,  // Invitation expiry (e.g., "1h")

        #[arg(long)]
        session_ttl: Option<String>,  // Session expiry (e.g., "30d")

        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },

    /// List pending (non-activated) invitations
    List {
        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },

    /// Revoke a guest invitation or active session
    Revoke {
        guest_id: String,

        #[arg(short, long)]
        force: bool,
    },

    /// Show detailed information about a guest
    Info {
        guest_id: String,

        #[arg(short, long, default_value = "text")]
        format: OutputFormat,
    },
}
```

### Usage Examples

```bash
# Create invitation for Mom (translate only, 30-day session)
$ aleph guests invite --name "Mom" --tools translate --session-ttl 30d

✓ Invitation created successfully!

  Guest ID: 6ba7b810-9dad-11d1-80b4-00c04fd430c8
  Token: 550e8400-e29b-41d4-a716-446655440000
  Expires at: 2026-02-07 18:45:00 UTC
  Activation URL: https://aleph.local/join?t=550e8400...

# List all pending invitations
$ aleph guests list

# JSON output for scripting
$ aleph guests list --format json
```

---

## Implementation Impact Analysis

### Files to Modify

| File | Change | Complexity |
|------|--------|-----------|
| `shared/protocol/src/auth.rs` | Add `IdentityContext` type | Low |
| `core/src/gateway/session_manager.rs` | Add `SessionIdentityMeta` | Medium |
| `core/src/gateway/security/policy_engine.rs` | Refactor to stateless | Medium |
| `core/src/executor/single_step.rs` | Inject `IdentityContext` into `execute()` | High |
| `core/src/executor/routed_executor.rs` | Inject `IdentityContext` into `execute()` | High |
| `core/src/agent_loop/mod.rs` | Pass `IdentityContext` to executor | Medium |
| `clients/cli/src/main.rs` | Add `Guests` command | Low |
| `clients/cli/src/commands/guests.rs` | Implement handlers | Medium |

### New Files to Create

- `clients/cli/src/commands/guests.rs` - CLI command implementation
- Tests for `IdentityContext` construction
- Integration tests for permission enforcement

### Backward Compatibility

1. **Session metadata migration** - Use `Default::default()` for old sessions
2. **PolicyEngine deprecation** - Old API marked `#[deprecated]` but functional
3. **CLI addition** - No breaking changes to existing commands

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_owner_always_allowed() {
    let identity = IdentityContext::owner("session:1".into(), "cli".into());
    let result = PolicyEngine::check_tool_permission(&identity, "shell:exec");
    assert!(result.is_allowed());
}

#[test]
fn test_guest_denied_without_scope() {
    let identity = IdentityContext {
        role: Role::Guest,
        identity_id: "guest1".into(),
        scope: None,  // ❌ No scope
        // ... other fields
    };
    let result = PolicyEngine::check_tool_permission(&identity, "translate");
    assert!(!result.is_allowed());
}

#[test]
fn test_guest_allowed_with_matching_tool() {
    let scope = GuestScope {
        allowed_tools: vec!["translate".into()],
        expires_at: None,
        display_name: None,
    };
    let identity = IdentityContext::guest(
        "session:guest1".into(),
        "guest1".into(),
        scope,
        "telegram".into(),
    );
    let result = PolicyEngine::check_tool_permission(&identity, "translate");
    assert!(result.is_allowed());
}

#[test]
fn test_guest_denied_expired_token() {
    let scope = GuestScope {
        allowed_tools: vec!["*".into()],
        expires_at: Some(1000),  // Past timestamp
        display_name: None,
    };
    let identity = IdentityContext::guest(
        "session:guest1".into(),
        "guest1".into(),
        scope,
        "telegram".into(),
    );
    // Ensure identity.created_at > scope.expires_at
    let mut identity_expired = identity.clone();
    identity_expired.created_at = 2000;

    let result = PolicyEngine::check_tool_permission(&identity_expired, "translate");
    assert!(!result.is_allowed());
}
```

### Integration Tests

```bash
# 1. Create guest invitation via CLI
$ aleph guests invite --name "TestGuest" --tools translate

# 2. Activate invitation (simulate WebSocket connect with token)
# 3. Attempt to execute tool via AgentLoop
# 4. Verify PolicyEngine blocks unauthorized tools
```

### E2E Test Flow

1. **Setup**: Start Gateway, connect CLI
2. **Create Invitation**: `aleph guests invite --name "Test" --tools translate`
3. **Activate Invitation**: Simulate guest WebSocket connection
4. **Test Allowed Tool**: Guest calls `translate` → Success
5. **Test Denied Tool**: Guest calls `shell:exec` → Denied with reason
6. **Verify Audit Log**: Check logs for IdentityContext in execution records

---

## Security Guarantees

### What This Design Achieves

1. **Immutable Permissions** - Scope frozen at session creation, immune to mid-execution changes
2. **Audit Trail** - Every `IdentityContext` has `request_id` for log correlation
3. **Explicit Denial** - Guests see clear error messages (not ghost failures)
4. **Race-Condition Free** - No dependency on mutable PolicyEngine state

### What This Design Does NOT Cover (Future Work)

- Data isolation (Memory namespace filtering)
- Session history isolation (Guest can't see Owner's messages)
- Dynamic permission updates (requires session reset)
- Tool-level argument filtering (e.g., restrict `file:read` to specific paths)

---

## Implementation Phases

### Phase 1: Foundation (Days 1-2)
- [ ] Add `IdentityContext` to `shared/protocol/src/auth.rs`
- [ ] Add `SessionIdentityMeta` to `session_manager.rs`
- [ ] Refactor `PolicyEngine` to stateless
- [ ] Write unit tests for `PolicyEngine`

### Phase 2: Executor Integration (Days 3-4)
- [ ] Modify `SingleStepExecutor::execute()` signature
- [ ] Modify `RoutedExecutor::execute()` signature
- [ ] Update `AgentLoop` to construct and pass `IdentityContext`
- [ ] Update `SessionManager::get_or_create()` to build `IdentityContext`

### Phase 3: CLI & Testing (Days 5-6)
- [ ] Implement `clients/cli/src/commands/guests.rs`
- [ ] Add `Guests` command to CLI main
- [ ] Write integration tests
- [ ] E2E validation with real guest sessions

### Phase 4: Documentation & Cleanup (Day 7)
- [ ] Update `docs/SECURITY.md` with new enforcement model
- [ ] Add examples to `docs/ARCHITECTURE.md`
- [ ] Deprecation warnings for old `PolicyEngine` APIs
- [ ] Final testing and bug fixes

---

## Success Criteria

- [ ] Guest with `allowed_tools: ["translate"]` can execute `translate`
- [ ] Same guest **cannot** execute `shell:exec` (returns `PermissionDenied`)
- [ ] Owner can execute all tools regardless of scope
- [ ] Expired guest sessions are rejected with clear error message
- [ ] CLI `aleph guests invite` creates functional invitation
- [ ] CLI `aleph guests list` shows pending invitations
- [ ] All existing tests pass (backward compatibility)
- [ ] New integration tests cover guest permission scenarios

---

## Appendix: Decision Rationale

### Why Stateless PolicyEngine?

**Stateful Approach Problems:**
1. Requires state sync in distributed systems
2. Race conditions if scope updated mid-execution
3. Audit trail requires querying historical PolicyEngine state

**Stateless Approach Benefits:**
1. IdentityContext is self-contained evidence
2. Pure functions enable property-based testing
3. Trivial to replay/debug with logged IdentityContext

### Why Extend Session Metadata Instead of New Table?

**Alternative Considered:** Create `guest_sessions` table

**Chosen Approach Advantages:**
1. No schema migration needed (TEXT field already exists)
2. Single source of truth for session identity
3. Backward compatible (NULL → Default)
4. Simpler query logic (no JOINs needed)

### Why CLI First Before UI?

**Testing Pyramid:** CLI is the fastest way to validate RPC layer without UI complexity.

**Benefit:** Debugging permission denial is easier in terminal (JSON output, grep-able logs) than in Tauri/macOS UI.

---

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Breaking existing Executor calls | High | High | Use `#[deprecated]` + compilation warnings |
| Session migration bugs | Medium | Medium | Extensive unit tests for `Default::default()` |
| CLI parsing edge cases (TTL) | Medium | Low | Comprehensive input validation tests |
| Guest bypasses via direct Thinker call | Low | Critical | Ensure AgentLoop **always** constructs IdentityContext |

---

**Document Version:** 1.0
**Next Review:** After Phase 2 completion
**Approval Status:** ✅ Approved for Implementation
