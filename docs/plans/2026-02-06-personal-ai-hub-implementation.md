# Personal AI Hub Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement Personal AI Hub architecture with Owner+Guest user model, mDNS discovery, config sync, and data isolation

**Architecture:** Server-Client with thin client philosophy. Server holds all state (config, memory, sessions). Clients are shells + local executors. Owner has full control, Guests have scoped permissions.

**Tech Stack:** Rust (tokio, serde, dashmap, mdns-sd), rusqlite (namespaced storage), existing aleph-protocol crate

---

## Phase 1: Foundation - User Model & Authentication

### Task 1.1: Define Role Types in Protocol

**Files:**
- Create: `shared/protocol/src/auth.rs`
- Modify: `shared/protocol/src/lib.rs`

**Step 1: Write the failing test**

Create `shared/protocol/src/auth.rs`:

```rust
//! Authentication types for Owner+Guest model

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// User role in Personal AI Hub
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full system control, can manage guests
    Owner,
    /// Limited access with scoped permissions
    Guest,
    /// Unauthenticated, access denied
    Anonymous,
}

impl Default for Role {
    fn default() -> Self {
        Self::Anonymous
    }
}

/// Guest permission scope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestScope {
    /// Allowed tool names or categories
    pub allowed_tools: Vec<String>,
    /// Token expiration timestamp (Unix seconds)
    pub expires_at: Option<i64>,
    /// Human-readable name
    pub display_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_default_is_anonymous() {
        assert_eq!(Role::default(), Role::Anonymous);
    }

    #[test]
    fn test_role_serde() {
        let role = Role::Owner;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"owner\"");
        let parsed: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Role::Owner);
    }

    #[test]
    fn test_guest_scope_with_expiry() {
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: Some(1735689600),
            display_name: Some("Mom".to_string()),
        };
        let json = serde_json::to_string(&scope).unwrap();
        let parsed: GuestScope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allowed_tools, vec!["translate"]);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd shared/protocol && cargo test auth`
Expected: FAIL (module not found in lib.rs)

**Step 3: Export from lib.rs**

Modify `shared/protocol/src/lib.rs`:

```rust
mod auth;
pub use auth::{Role, GuestScope};
```

**Step 4: Run test to verify it passes**

Run: `cd shared/protocol && cargo test auth`
Expected: PASS (3 tests)

**Step 5: Commit**

```bash
cd shared/protocol
git add src/auth.rs src/lib.rs
git commit -m "feat(protocol): add Role and GuestScope for Owner+Guest model"
```

---

### Task 1.2: Add IdentityMap to Gateway Security

**Files:**
- Create: `core/src/gateway/security/identity_map.rs`
- Modify: `core/src/gateway/security/mod.rs`

**Step 1: Write the failing test**

Create `core/src/gateway/security/identity_map.rs`:

```rust
//! Maps external identities (Telegram, WhatsApp) to internal user IDs

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Internal user identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UserId {
    Owner,
    Guest(String), // guest_id
}

/// External platform identity
#[derive(Debug, Clone)]
pub struct PlatformIdentity {
    pub platform: String,      // "telegram", "whatsapp"
    pub platform_user_id: String, // "+1234567890", "user123"
}

/// Bidirectional mapping between platform identities and internal user IDs
pub struct IdentityMap {
    /// "platform:user_id" -> UserId
    external_to_internal: DashMap<String, UserId>,
    /// UserId -> Vec<"platform:user_id">
    internal_to_external: DashMap<UserId, Vec<String>>,
}

impl IdentityMap {
    pub fn new() -> Self {
        Self {
            external_to_internal: DashMap::new(),
            internal_to_external: DashMap::new(),
        }
    }

    /// Create identity key from platform and user_id
    fn make_key(platform: &str, platform_user_id: &str) -> String {
        format!("{}:{}", platform, platform_user_id)
    }

    /// Resolve external identity to internal user ID
    pub fn resolve(&self, platform: &str, platform_user_id: &str) -> Option<UserId> {
        let key = Self::make_key(platform, platform_user_id);
        self.external_to_internal.get(&key).map(|v| v.clone())
    }

    /// Add or update mapping
    pub fn add_mapping(&self, platform: &str, platform_user_id: &str, user_id: UserId) {
        let key = Self::make_key(platform, platform_user_id);
        self.external_to_internal.insert(key.clone(), user_id.clone());

        // Update reverse mapping
        self.internal_to_external
            .entry(user_id)
            .or_insert_with(Vec::new)
            .push(key);
    }

    /// Remove mapping
    pub fn remove_mapping(&self, platform: &str, platform_user_id: &str) {
        let key = Self::make_key(platform, platform_user_id);
        if let Some((_, user_id)) = self.external_to_internal.remove(&key) {
            if let Some(mut external_ids) = self.internal_to_external.get_mut(&user_id) {
                external_ids.retain(|k| k != &key);
            }
        }
    }

    /// Get all external identities for a user
    pub fn get_external_identities(&self, user_id: &UserId) -> Vec<String> {
        self.internal_to_external
            .get(user_id)
            .map(|v| v.clone())
            .unwrap_or_default()
    }
}

impl Default for IdentityMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_returns_none_for_unknown() {
        let map = IdentityMap::new();
        assert!(map.resolve("telegram", "unknown").is_none());
    }

    #[test]
    fn test_add_and_resolve_mapping() {
        let map = IdentityMap::new();
        map.add_mapping("telegram", "12345", UserId::Owner);

        let result = map.resolve("telegram", "12345");
        assert_eq!(result, Some(UserId::Owner));
    }

    #[test]
    fn test_multiple_external_ids_for_one_user() {
        let map = IdentityMap::new();
        map.add_mapping("telegram", "12345", UserId::Owner);
        map.add_mapping("whatsapp", "+1234567890", UserId::Owner);

        let external_ids = map.get_external_identities(&UserId::Owner);
        assert_eq!(external_ids.len(), 2);
        assert!(external_ids.contains(&"telegram:12345".to_string()));
    }

    #[test]
    fn test_remove_mapping() {
        let map = IdentityMap::new();
        map.add_mapping("telegram", "12345", UserId::Owner);

        map.remove_mapping("telegram", "12345");
        assert!(map.resolve("telegram", "12345").is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test identity_map`
Expected: FAIL (module not exported)

**Step 3: Export from security/mod.rs**

Modify `core/src/gateway/security/mod.rs`:

```rust
mod identity_map;
pub use identity_map::{IdentityMap, UserId, PlatformIdentity};
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test identity_map`
Expected: PASS (4 tests)

**Step 5: Commit**

```bash
cd core
git add src/gateway/security/identity_map.rs src/gateway/security/mod.rs
git commit -m "feat(gateway): add IdentityMap for external identity resolution"
```

---

### Task 1.3: Add PolicyEngine to Gateway Security

**Files:**
- Create: `core/src/gateway/security/policy_engine.rs`
- Modify: `core/src/gateway/security/mod.rs`

**Step 1: Write the failing test**

Create `core/src/gateway/security/policy_engine.rs`:

```rust
//! Permission policy engine for Owner+Guest model

use aleph_protocol::auth::{Role, GuestScope};
use dashmap::DashMap;
use std::collections::HashMap;

/// Permission check result
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionResult {
    Allowed,
    Denied { reason: String },
}

impl PermissionResult {
    pub fn is_allowed(&self) -> bool {
        matches!(self, PermissionResult::Allowed)
    }
}

/// Policy engine for checking tool execution permissions
pub struct PolicyEngine {
    /// guest_id -> GuestScope
    guest_scopes: DashMap<String, GuestScope>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self {
            guest_scopes: DashMap::new(),
        }
    }

    /// Register or update guest scope
    pub fn set_guest_scope(&self, guest_id: String, scope: GuestScope) {
        self.guest_scopes.insert(guest_id, scope);
    }

    /// Remove guest scope
    pub fn remove_guest_scope(&self, guest_id: &str) {
        self.guest_scopes.remove(guest_id);
    }

    /// Check if role can execute tool
    pub fn check_tool_permission(
        &self,
        role: &Role,
        guest_id: Option<&str>,
        tool_name: &str,
    ) -> PermissionResult {
        match role {
            Role::Owner => PermissionResult::Allowed,
            Role::Guest => {
                let Some(guest_id) = guest_id else {
                    return PermissionResult::Denied {
                        reason: "Guest ID required for Guest role".to_string(),
                    };
                };

                let Some(scope) = self.guest_scopes.get(guest_id) else {
                    return PermissionResult::Denied {
                        reason: format!("No scope found for guest '{}'", guest_id),
                    };
                };

                // Check expiry
                if let Some(expires_at) = scope.expires_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64;
                    if now > expires_at {
                        return PermissionResult::Denied {
                            reason: "Guest token expired".to_string(),
                        };
                    }
                }

                // Check tool permission
                let tool_category = tool_name.split(':').next().unwrap_or(tool_name);
                let allowed = scope.allowed_tools.iter().any(|allowed| {
                    allowed == tool_name || allowed == tool_category || allowed == "*"
                });

                if allowed {
                    PermissionResult::Allowed
                } else {
                    PermissionResult::Denied {
                        reason: format!("Tool '{}' not in guest scope", tool_name),
                    }
                }
            }
            Role::Anonymous => PermissionResult::Denied {
                reason: "Authentication required".to_string(),
            },
        }
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_always_allowed() {
        let engine = PolicyEngine::new();
        let result = engine.check_tool_permission(&Role::Owner, None, "shell:exec");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_anonymous_always_denied() {
        let engine = PolicyEngine::new();
        let result = engine.check_tool_permission(&Role::Anonymous, None, "translate");
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_guest_with_scope_allowed() {
        let engine = PolicyEngine::new();
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "translate");
        assert!(result.is_allowed());
    }

    #[test]
    fn test_guest_without_permission_denied() {
        let engine = PolicyEngine::new();
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: None,
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "shell:exec");
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_guest_expired_token_denied() {
        let engine = PolicyEngine::new();
        let past_timestamp = 1000000000; // Year 2001
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["*".to_string()],
                expires_at: Some(past_timestamp),
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "translate");
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_tool_category_matching() {
        let engine = PolicyEngine::new();
        engine.set_guest_scope(
            "guest1".to_string(),
            GuestScope {
                allowed_tools: vec!["shell".to_string()],
                expires_at: None,
                display_name: None,
            },
        );

        let result = engine.check_tool_permission(&Role::Guest, Some("guest1"), "shell:exec");
        assert!(result.is_allowed());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test policy_engine`
Expected: FAIL (module not exported)

**Step 3: Export from security/mod.rs**

Modify `core/src/gateway/security/mod.rs`:

```rust
mod policy_engine;
pub use policy_engine::{PolicyEngine, PermissionResult};
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test policy_engine`
Expected: PASS (6 tests)

**Step 5: Commit**

```bash
cd core
git add src/gateway/security/policy_engine.rs src/gateway/security/mod.rs
git commit -m "feat(gateway): add PolicyEngine for permission checks"
```

---

## Phase 2: Guest Management - Invitation System

### Task 2.1: Add Invitation Types to Protocol

**Files:**
- Create: `shared/protocol/src/invitation.rs`
- Modify: `shared/protocol/src/lib.rs`

**Step 1: Write the failing test**

Create `shared/protocol/src/invitation.rs`:

```rust
//! Guest invitation types

use crate::auth::GuestScope;
use serde::{Deserialize, Serialize};

/// Request to create guest invitation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvitationRequest {
    /// Guest display name
    pub guest_name: String,
    /// Permission scope
    pub scope: GuestScope,
}

/// Created invitation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    /// Encrypted invitation token
    pub token: String,
    /// Invitation URL
    pub url: String,
    /// Guest ID
    pub guest_id: String,
    /// Expiry timestamp
    pub expires_at: Option<i64>,
}

/// Request to activate invitation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivateInvitationRequest {
    /// Invitation token
    pub token: String,
}

/// Activated guest token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestToken {
    /// JWT-style token
    pub token: String,
    /// Guest ID
    pub guest_id: String,
    /// Scope
    pub scope: GuestScope,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_invitation_request_serde() {
        let req = CreateInvitationRequest {
            guest_name: "Mom".to_string(),
            scope: GuestScope {
                allowed_tools: vec!["translate".to_string()],
                expires_at: Some(1735689600),
                display_name: Some("Mom".to_string()),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: CreateInvitationRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.guest_name, "Mom");
    }

    #[test]
    fn test_invitation_serde() {
        let inv = Invitation {
            token: "encrypted_token".to_string(),
            url: "https://aleph.local/join?t=xxx".to_string(),
            guest_id: "guest1".to_string(),
            expires_at: Some(1735689600),
        };
        let json = serde_json::to_string(&inv).unwrap();
        let parsed: Invitation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.guest_id, "guest1");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd shared/protocol && cargo test invitation`
Expected: FAIL (module not found)

**Step 3: Export from lib.rs**

Modify `shared/protocol/src/lib.rs`:

```rust
mod invitation;
pub use invitation::{
    CreateInvitationRequest, Invitation,
    ActivateInvitationRequest, GuestToken,
};
```

**Step 4: Run test to verify it passes**

Run: `cd shared/protocol && cargo test invitation`
Expected: PASS (2 tests)

**Step 5: Commit**

```bash
cd shared/protocol
git add src/invitation.rs src/lib.rs
git commit -m "feat(protocol): add invitation types for guest management"
```

---

### Task 2.2: Add InvitationManager to Gateway

**Files:**
- Create: `core/src/gateway/security/invitation_manager.rs`
- Modify: `core/src/gateway/security/mod.rs`

**Step 1: Write the failing test**

Create `core/src/gateway/security/invitation_manager.rs`:

```rust
//! Manages guest invitations and activation

use aleph_protocol::auth::GuestScope;
use aleph_protocol::invitation::{Invitation, GuestToken};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Pending invitation
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingInvitation {
    guest_id: String,
    guest_name: String,
    scope: GuestScope,
    created_at: i64,
    invitation_expires_at: i64, // 15 minutes
}

/// Manages invitations
pub struct InvitationManager {
    /// token -> PendingInvitation
    pending: DashMap<String, PendingInvitation>,
    /// HMAC secret for token signing
    secret: String,
}

impl InvitationManager {
    pub fn new(secret: String) -> Self {
        Self {
            pending: DashMap::new(),
            secret,
        }
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    /// Create invitation
    pub fn create_invitation(
        &self,
        guest_name: String,
        scope: GuestScope,
        base_url: &str,
    ) -> Invitation {
        let guest_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string(); // Simple token for now
        let now = Self::now();
        let invitation_expires_at = now + 900; // 15 minutes

        let pending = PendingInvitation {
            guest_id: guest_id.clone(),
            guest_name,
            scope: scope.clone(),
            created_at: now,
            invitation_expires_at,
        };

        self.pending.insert(token.clone(), pending);

        Invitation {
            token: token.clone(),
            url: format!("{}/join?t={}", base_url, token),
            guest_id,
            expires_at: scope.expires_at,
        }
    }

    /// Activate invitation
    pub fn activate_invitation(&self, token: &str) -> Result<GuestToken, String> {
        let Some((_, pending)) = self.pending.remove(token) else {
            return Err("Invalid or expired invitation token".to_string());
        };

        let now = Self::now();
        if now > pending.invitation_expires_at {
            return Err("Invitation expired".to_string());
        }

        // Generate guest JWT token (simplified)
        let guest_token = format!("guest_{}_{}", pending.guest_id, Uuid::new_v4());

        Ok(GuestToken {
            token: guest_token,
            guest_id: pending.guest_id,
            scope: pending.scope,
        })
    }

    /// List active invitations
    pub fn list_pending(&self) -> Vec<Invitation> {
        let now = Self::now();
        self.pending
            .iter()
            .filter(|entry| entry.value().invitation_expires_at > now)
            .map(|entry| Invitation {
                token: entry.key().clone(),
                url: format!("https://aleph.local/join?t={}", entry.key()),
                guest_id: entry.value().guest_id.clone(),
                expires_at: entry.value().scope.expires_at,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_invitation() {
        let manager = InvitationManager::new("test_secret".to_string());
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: None,
            display_name: Some("Test".to_string()),
        };

        let inv = manager.create_invitation("Test".to_string(), scope, "https://aleph.local");
        assert!(inv.url.contains("/join?t="));
    }

    #[test]
    fn test_activate_valid_invitation() {
        let manager = InvitationManager::new("test_secret".to_string());
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: None,
            display_name: Some("Test".to_string()),
        };

        let inv = manager.create_invitation("Test".to_string(), scope, "https://aleph.local");
        let result = manager.activate_invitation(&inv.token);
        assert!(result.is_ok());

        let guest_token = result.unwrap();
        assert_eq!(guest_token.guest_id, inv.guest_id);
    }

    #[test]
    fn test_activate_invalid_token() {
        let manager = InvitationManager::new("test_secret".to_string());
        let result = manager.activate_invitation("invalid_token");
        assert!(result.is_err());
    }

    #[test]
    fn test_invitation_one_time_use() {
        let manager = InvitationManager::new("test_secret".to_string());
        let scope = GuestScope {
            allowed_tools: vec!["translate".to_string()],
            expires_at: None,
            display_name: None,
        };

        let inv = manager.create_invitation("Test".to_string(), scope, "https://aleph.local");
        manager.activate_invitation(&inv.token).unwrap();

        // Second activation should fail
        let result = manager.activate_invitation(&inv.token);
        assert!(result.is_err());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test invitation_manager`
Expected: FAIL (module not exported)

**Step 3: Export from security/mod.rs**

Modify `core/src/gateway/security/mod.rs`:

```rust
mod invitation_manager;
pub use invitation_manager::InvitationManager;
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test invitation_manager`
Expected: PASS (4 tests)

**Step 5: Commit**

```bash
cd core
git add src/gateway/security/invitation_manager.rs src/gateway/security/mod.rs
git commit -m "feat(gateway): add InvitationManager for guest invitations"
```

---

## Phase 3: Configuration Sync - Server as Source of Truth

### Task 3.1: Add Config Events to Protocol

**Files:**
- Modify: `shared/protocol/src/events.rs`

**Step 1: Write the failing test**

Add to `shared/protocol/src/events.rs`:

```rust
/// Configuration changed event
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChangedEvent {
    /// Changed section path (e.g., "ui.theme")
    pub section: Option<String>,
    /// New config value (full config if section is None)
    pub value: Value,
    /// Change timestamp
    pub timestamp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_changed_event_serde() {
        let event = ConfigChangedEvent {
            section: Some("ui.theme".to_string()),
            value: json!({"color": "dark"}),
            timestamp: 1735689600,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ConfigChangedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.section, Some("ui.theme".to_string()));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd shared/protocol && cargo test config_changed_event`
Expected: FAIL (ConfigChangedEvent not exported)

**Step 3: Export from events module**

Add to exports in `shared/protocol/src/events.rs` or `lib.rs`:

```rust
pub use events::ConfigChangedEvent;
```

**Step 4: Run test to verify it passes**

Run: `cd shared/protocol && cargo test config_changed_event`
Expected: PASS (1 test)

**Step 5: Commit**

```bash
cd shared/protocol
git add src/events.rs
git commit -m "feat(protocol): add ConfigChangedEvent for config sync"
```

---

## Phase 4: Memory Facts Namespacing

### Task 4.1: Add Namespace Column to Facts Table

**Files:**
- Modify: `core/src/memory/database/schema.rs` (or equivalent)
- Create migration script

**Step 1: Write migration SQL**

Create migration (note: exact location depends on current migration system):

```sql
-- Add namespace column to facts table
ALTER TABLE facts ADD COLUMN namespace TEXT NOT NULL DEFAULT 'owner';

-- Create index for namespace filtering
CREATE INDEX IF NOT EXISTS idx_facts_namespace ON facts(namespace);

-- Create index for namespace+embedding queries
CREATE INDEX IF NOT EXISTS idx_facts_namespace_embedding ON facts(namespace, id);
```

**Step 2: Update schema documentation**

Document in `core/src/memory/database/schema.rs` or README:

```rust
/// Facts table schema with namespace support
///
/// Namespace values:
/// - "owner": Owner's private facts
/// - "guest:<guest_id>": Guest's private facts
/// - "shared": Facts shared with specific guests
```

**Step 3: Test migration**

Run: `cd core && cargo test memory::database`
Expected: Existing tests still pass

**Step 4: Commit**

```bash
git add core/src/memory/database/
git commit -m "feat(memory): add namespace column for data isolation"
```

---

## Phase 5: Discovery - mDNS Foundation

### Task 5.1: Add mDNS Dependency

**Files:**
- Modify: `Cargo.toml` (workspace root or shared/sdk if exists)

**Step 1: Add mdns-sd dependency**

Add to appropriate `Cargo.toml`:

```toml
[dependencies]
mdns-sd = "0.11"
```

**Step 2: Verify compilation**

Run: `cargo check`
Expected: No errors

**Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "deps: add mdns-sd for service discovery"
```

---

### Task 5.2: Create mDNS Scanner Stub

**Files:**
- Create: `shared/protocol/src/discovery.rs` (or appropriate location)
- Modify: `shared/protocol/src/lib.rs`

**Step 1: Write the failing test**

Create `shared/protocol/src/discovery.rs`:

```rust
//! Service discovery types

use serde::{Deserialize, Serialize};

/// Discovered Aleph instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredInstance {
    /// Instance name
    pub name: String,
    /// Hostname (e.g., "aleph.local")
    pub hostname: String,
    /// Port
    pub port: u16,
    /// IP addresses
    pub addresses: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_instance_serde() {
        let instance = DiscoveredInstance {
            name: "Home Aleph".to_string(),
            hostname: "aleph.local".to_string(),
            port: 18789,
            addresses: vec!["192.168.1.100".to_string()],
        };
        let json = serde_json::to_string(&instance).unwrap();
        let parsed: DiscoveredInstance = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.port, 18789);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd shared/protocol && cargo test discovery`
Expected: FAIL (module not found)

**Step 3: Export from lib.rs**

Modify `shared/protocol/src/lib.rs`:

```rust
mod discovery;
pub use discovery::DiscoveredInstance;
```

**Step 4: Run test to verify it passes**

Run: `cd shared/protocol && cargo test discovery`
Expected: PASS (1 test)

**Step 5: Commit**

```bash
cd shared/protocol
git add src/discovery.rs src/lib.rs
git commit -m "feat(protocol): add discovery types for mDNS"
```

---

## Phase 6: Integration - Wire Everything Together

### Task 6.1: Add RPC Handlers for Guest Management

**Files:**
- Create: `core/src/gateway/handlers/guests.rs`
- Modify: `core/src/gateway/handlers/mod.rs`

**Step 1: Create handler skeleton**

Create `core/src/gateway/handlers/guests.rs`:

```rust
//! Guest management RPC handlers

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::security::{InvitationManager, PolicyEngine, IdentityMap};
use aleph_protocol::invitation::CreateInvitationRequest;
use serde_json::{json, Value};
use std::sync::Arc;

/// Handle guests.create_invitation
pub async fn handle_create_invitation(
    req: &JsonRpcRequest,
    invitation_mgr: Arc<InvitationManager>,
) -> JsonRpcResponse {
    let params: CreateInvitationRequest = match serde_json::from_value(
        req.params.clone().unwrap_or(Value::Null)
    ) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(req.id.clone(), -32602, format!("Invalid params: {}", e));
        }
    };

    let invitation = invitation_mgr.create_invitation(
        params.guest_name,
        params.scope,
        "https://aleph.local", // TODO: Get from config
    );

    JsonRpcResponse::success(req.id.clone(), serde_json::to_value(invitation).unwrap())
}

/// Handle guests.list
pub async fn handle_list_guests(
    req: &JsonRpcRequest,
    invitation_mgr: Arc<InvitationManager>,
) -> JsonRpcResponse {
    let invitations = invitation_mgr.list_pending();
    JsonRpcResponse::success(req.id.clone(), serde_json::to_value(invitations).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::auth::GuestScope;

    #[tokio::test]
    async fn test_create_invitation_handler() {
        let invitation_mgr = Arc::new(InvitationManager::new("test".to_string()));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "guests.create_invitation".to_string(),
            params: Some(json!({
                "guest_name": "Test",
                "scope": {
                    "allowed_tools": ["translate"],
                    "expires_at": null,
                    "display_name": "Test"
                }
            })),
            id: Some(Value::String("1".to_string())),
        };

        let response = handle_create_invitation(&req, invitation_mgr).await;
        assert!(response.result.is_some());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test guests::handle`
Expected: FAIL (module not found)

**Step 3: Export from handlers/mod.rs**

Modify `core/src/gateway/handlers/mod.rs`:

```rust
mod guests;
pub use guests::{handle_create_invitation, handle_list_guests};
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test guests::handle`
Expected: PASS (1 test)

**Step 5: Commit**

```bash
cd core
git add src/gateway/handlers/guests.rs src/gateway/handlers/mod.rs
git commit -m "feat(gateway): add RPC handlers for guest management"
```

---

## Summary

This plan implements Personal AI Hub in 6 phases:

1. **Foundation (3 tasks)**: Role types, IdentityMap, PolicyEngine
2. **Guest Management (2 tasks)**: Invitation types, InvitationManager
3. **Config Sync (1 task)**: ConfigChangedEvent
4. **Memory Isolation (1 task)**: Namespace column
5. **Discovery (2 tasks)**: mDNS dependency, discovery types
6. **Integration (1 task)**: RPC handlers

**Total**: 10 tasks, ~50 steps

Each task follows TDD: test → implement → verify → commit

**Next Steps After Plan**:
- Wire handlers into Gateway's message router
- Implement mDNS broadcaster in Gateway
- Implement mDNS scanner in SDK
- Add ConfigManager to SDK
- Full integration testing

**Testing Strategy**:
- Unit tests per module (done in each task)
- Integration tests after Phase 6
- End-to-end test: Create invitation → Activate → Check permissions

**Rollout Strategy**:
- Deploy Phase 1-3 first (authentication foundation)
- Then Phase 4 (data isolation)
- Finally Phase 5-6 (discovery + integration)
