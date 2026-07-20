# Gateway Evolution P4: Security Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist DM policies to SQLite, emit security events to audit log + event bus, add brute-force detection for pairing, and add SHA-256 hash verification for plugin installs.

**Architecture:** Extend SecurityStore (schema v8) with channel_policies table. Emit SecurityAuditLog events at pairing/permission check points. Add hash field to marketplace manifest and verify on install.

**Tech Stack:** SQLite (rusqlite), SecurityAuditLog, GatewayEventBus, sha2 crate

**Spec:** `docs/superpowers/specs/2026-03-25-gateway-evolution-design.md` (Phase 4)

---

## Key Discovery: 80% Infrastructure Exists

| Component | Status | Location |
|-----------|--------|----------|
| SecurityStore SQLite | ✅ v7, migrations ready | `gateway/security/store.rs` |
| SecurityAuditLog | ✅ 10 event types, async channel | `security/audit.rs` |
| Approval system | ✅ 10 action types, glob matching | `approval/` |
| DmPolicy enum | ✅ Open/Allowlist/Pairing/Disabled | `inbound_router/types.rs` |
| Pairing flow | ✅ 5-min expiry, capacity limits | `gateway/security/pairing.rs` |
| Event bus | ✅ broadcast + global bus | `event/bus.rs` |
| channel_policies DB table | ❌ Missing | — |
| Security event emission | ❌ Not wired | — |
| Brute-force detection | ❌ Missing | — |
| Plugin hash verification | ❌ Missing | — |

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/gateway/security/store.rs` | Add channel_policies table (v8 migration) |
| Modify | `src/gateway/inbound_router/permission.rs` | Read DM policy from DB |
| Modify | `src/security/audit.rs` | Add new event types for pairing/permission |
| Create | `src/gateway/security/brute_force.rs` | Pairing brute-force detection |
| Modify | `src/gateway/security/mod.rs` | Export new module |
| Modify | `src/extension/marketplace/manifest.rs` | Add sha256 field |
| Modify | `src/extension/marketplace/installer.rs` | Verify hash on install |

---

### Task 1: DM Policy persistence (channel_policies table)

**Files:**
- Modify: `src/gateway/security/store.rs` — v8 migration + CRUD methods

- [ ] **Step 1: Read store.rs to find exact migration location**

Read `src/gateway/security/store.rs` to find:
- SCHEMA_VERSION constant (should be 7)
- Location of v7 migration block
- Pattern for adding new migration

- [ ] **Step 2: Add v8 migration with channel_policies table**

Bump `SCHEMA_VERSION` from 7 to 8. Add v8 migration block after v7:

```rust
// v8: Channel policies persistence
if current_version < 8 {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS channel_policies (
            channel_id TEXT NOT NULL,
            policy_type TEXT NOT NULL,
            policy TEXT NOT NULL,
            allowlist TEXT,
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (channel_id, policy_type)
        );
        CREATE INDEX IF NOT EXISTS idx_channel_policies_channel ON channel_policies(channel_id);
        PRAGMA user_version = 8;"
    )?;
}
```

- [ ] **Step 3: Add CRUD methods for channel_policies**

Add to `impl SecurityStore`:

```rust
    /// Get DM policy for a channel. Returns None if not persisted (use config default).
    pub fn get_channel_dm_policy(&self, channel_id: &str) -> Result<Option<(String, Option<String>)>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT policy, allowlist FROM channel_policies WHERE channel_id = ?1 AND policy_type = 'dm'"
        )?;
        let result = stmt.query_row(params![channel_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        });
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Set DM policy for a channel.
    pub fn set_channel_dm_policy(
        &self,
        channel_id: &str,
        policy: &str,
        allowlist: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT OR REPLACE INTO channel_policies (channel_id, policy_type, policy, allowlist, updated_at)
             VALUES (?1, 'dm', ?2, ?3, ?4)",
            params![channel_id, policy, allowlist, now],
        )?;
        Ok(())
    }
```

- [ ] **Step 4: Add tests**

```rust
    #[test]
    fn test_channel_dm_policy_crud() {
        let store = SecurityStore::open_in_memory().unwrap();

        // No policy initially
        assert!(store.get_channel_dm_policy("telegram").unwrap().is_none());

        // Set policy
        store.set_channel_dm_policy("telegram", "pairing", None).unwrap();
        let (policy, allowlist) = store.get_channel_dm_policy("telegram").unwrap().unwrap();
        assert_eq!(policy, "pairing");
        assert!(allowlist.is_none());

        // Set with allowlist
        store.set_channel_dm_policy("discord", "allowlist", Some("[\"user1\",\"user2\"]")).unwrap();
        let (policy, allowlist) = store.get_channel_dm_policy("discord").unwrap().unwrap();
        assert_eq!(policy, "allowlist");
        assert_eq!(allowlist.unwrap(), "[\"user1\",\"user2\"]");

        // Update existing
        store.set_channel_dm_policy("telegram", "open", None).unwrap();
        let (policy, _) = store.get_channel_dm_policy("telegram").unwrap().unwrap();
        assert_eq!(policy, "open");
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib store::tests -- --nocapture`

- [ ] **Step 6: Commit**

```bash
git add src/gateway/security/store.rs
git commit -m "security: add channel_policies SQLite table (schema v8) with DM policy CRUD"
```

---

### Task 2: Security event emission + brute-force detection

**Files:**
- Modify: `src/security/audit.rs` — add PairingAttempt, PermissionDenied event types
- Create: `src/gateway/security/brute_force.rs` — rate-based detection
- Modify: `src/gateway/security/mod.rs` — export
- Modify: `src/gateway/inbound_router/permission.rs` — emit events

- [ ] **Step 1: Add new event types to SecurityAuditLog**

In `src/security/audit.rs`, add to `AuditEventType` enum:

```rust
    PairingAttempt,
    PairingBruteForce,
    PermissionDenied,
    GuestSessionCreated,
```

- [ ] **Step 2: Create brute_force.rs**

```rust
//! Brute-force detection for pairing attempts.
//!
//! Tracks failed pairing attempts per (channel, sender) and temporarily
//! blocks senders who exceed the threshold.

use dashmap::DashMap;
use std::time::{Duration, Instant};

/// Default: 5 failures in 5 minutes triggers a 30-minute block.
const MAX_FAILURES: u32 = 5;
const WINDOW: Duration = Duration::from_secs(300);
const BLOCK_DURATION: Duration = Duration::from_secs(1800);

struct AttemptRecord {
    failures: u32,
    first_failure: Instant,
    blocked_until: Option<Instant>,
}

/// Brute-force detector for pairing attempts.
pub struct BruteForceDetector {
    /// Key: "channel:sender_id"
    records: DashMap<String, AttemptRecord>,
}

impl BruteForceDetector {
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
        }
    }

    /// Check if a sender is currently blocked.
    pub fn is_blocked(&self, channel: &str, sender: &str) -> bool {
        let key = format!("{}:{}", channel, sender);
        if let Some(record) = self.records.get(&key) {
            if let Some(blocked_until) = record.blocked_until {
                if Instant::now() < blocked_until {
                    return true;
                }
            }
        }
        false
    }

    /// Record a failed pairing attempt. Returns true if the sender is now blocked.
    pub fn record_failure(&self, channel: &str, sender: &str) -> bool {
        let key = format!("{}:{}", channel, sender);
        let mut entry = self.records.entry(key).or_insert_with(|| AttemptRecord {
            failures: 0,
            first_failure: Instant::now(),
            blocked_until: None,
        });

        let record = entry.value_mut();

        // Reset window if expired
        if record.first_failure.elapsed() > WINDOW {
            record.failures = 0;
            record.first_failure = Instant::now();
            record.blocked_until = None;
        }

        record.failures += 1;

        if record.failures >= MAX_FAILURES {
            record.blocked_until = Some(Instant::now() + BLOCK_DURATION);
            return true; // Now blocked
        }

        false
    }

    /// Record a successful pairing (resets the failure counter).
    pub fn record_success(&self, channel: &str, sender: &str) {
        let key = format!("{}:{}", channel, sender);
        self.records.remove(&key);
    }

    /// Prune expired records. Returns count pruned.
    pub fn prune(&self) -> usize {
        let mut pruned = 0;
        self.records.retain(|_, record| {
            // Keep if actively blocked or has recent failures
            if let Some(blocked_until) = record.blocked_until {
                if Instant::now() >= blocked_until {
                    pruned += 1;
                    return false;
                }
                return true;
            }
            if record.first_failure.elapsed() > WINDOW {
                pruned += 1;
                return false;
            }
            true
        });
        pruned
    }
}

impl Default for BruteForceDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_blocked_initially() {
        let detector = BruteForceDetector::new();
        assert!(!detector.is_blocked("telegram", "user1"));
    }

    #[test]
    fn test_block_after_threshold() {
        let detector = BruteForceDetector::new();
        for i in 0..4 {
            let blocked = detector.record_failure("telegram", "user1");
            assert!(!blocked, "Should not be blocked after {} failures", i + 1);
        }
        // 5th failure triggers block
        let blocked = detector.record_failure("telegram", "user1");
        assert!(blocked, "Should be blocked after 5 failures");
        assert!(detector.is_blocked("telegram", "user1"));
    }

    #[test]
    fn test_success_resets() {
        let detector = BruteForceDetector::new();
        for _ in 0..4 {
            detector.record_failure("telegram", "user1");
        }
        detector.record_success("telegram", "user1");
        assert!(!detector.is_blocked("telegram", "user1"));

        // Should take 5 more failures to block again
        for _ in 0..4 {
            assert!(!detector.record_failure("telegram", "user1"));
        }
        assert!(detector.record_failure("telegram", "user1"));
    }

    #[test]
    fn test_different_senders_independent() {
        let detector = BruteForceDetector::new();
        for _ in 0..5 {
            detector.record_failure("telegram", "user1");
        }
        assert!(detector.is_blocked("telegram", "user1"));
        assert!(!detector.is_blocked("telegram", "user2"));
    }

    #[test]
    fn test_prune_expired() {
        let detector = BruteForceDetector::new();
        // Insert a record that will immediately be considered expired
        // (since WINDOW is 300s and we can't easily fast-forward time,
        // just verify prune runs without panic and returns 0 for fresh records)
        detector.record_failure("telegram", "user1");
        let pruned = detector.prune();
        assert_eq!(pruned, 0); // Fresh record, not yet expired
    }
}
```

- [ ] **Step 3: Export in gateway/security/mod.rs**

Add `pub mod brute_force;`

- [ ] **Step 4: Wire audit logging into permission.rs**

In `src/gateway/inbound_router/permission.rs`, where pairing requests fail or permissions are denied, add audit log calls. This requires passing the audit log into the InboundMessageRouter or using a global reference.

Simplest approach: add an `audit_log: Option<SecurityAuditLog>` field to InboundMessageRouter and emit events:

```rust
// After pairing failure (in the DmPolicy::Pairing branch):
if let Some(ref audit) = self.audit_log {
    audit.log_event(
        AuditEventType::PairingAttempt,
        AuditSeverity::Info,
        &format!("Pairing attempt from {} on channel {}", sender_id, channel_id),
    );
}

// After permission denied:
if let Some(ref audit) = self.audit_log {
    audit.log_event(
        AuditEventType::PermissionDenied,
        AuditSeverity::Warn,
        &format!("Permission denied for {} on channel {} (policy: {:?})", sender_id, channel_id, policy),
    );
}
```

NOTE: If integrating the audit log into InboundMessageRouter is too invasive in terms of field plumbing, create a standalone `emit_security_event()` helper that checks a global/static audit log. Alternatively, just add the brute_force detector and audit event types — the wiring can be done incrementally.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib brute_force -- --nocapture`

- [ ] **Step 6: Commit**

```bash
git add src/gateway/security/ src/security/audit.rs
git commit -m "security: add brute-force detection and security audit event types"
```

---

### Task 3: Plugin integrity verification

**Files:**
- Modify: `src/extension/marketplace/manifest.rs` — add sha256 field
- Modify: `src/extension/marketplace/installer.rs` — verify hash on install

- [ ] **Step 1: Add sha256 field to MarketplacePluginEntry**

In `manifest.rs`, add to `MarketplacePluginEntry`:

```rust
    /// SHA-256 hash of the plugin archive (hex-encoded)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
```

- [ ] **Step 2: Add hash verification to installer**

In `installer.rs`, add a verification function:

```rust
/// Verify the SHA-256 hash of a directory by hashing all files recursively.
/// Returns Ok(()) if hash matches or if expected_hash is None (no verification).
pub fn verify_plugin_integrity(
    source_path: &Path,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    let Some(expected) = expected_hash else {
        return Ok(()); // No hash to verify
    };

    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();

    // Collect and sort files for deterministic ordering
    let mut files: Vec<_> = walkdir::WalkDir::new(source_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.path().components().any(|c| c.as_os_str() == ".git"))
        .collect();
    files.sort_by(|a, b| a.path().cmp(b.path()));

    for entry in files {
        let relative = entry.path().strip_prefix(source_path).unwrap_or(entry.path());
        hasher.update(relative.to_string_lossy().as_bytes());
        let content = std::fs::read(entry.path())
            .map_err(|e| format!("Failed to read {}: {}", entry.path().display(), e))?;
        hasher.update(&content);
    }

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!(
            "Plugin integrity check failed: expected {}, got {}",
            expected, actual
        ));
    }

    Ok(())
}
```

Call this in `install_plugin_from_cache()` before copying:

```rust
// Before the copy operation:
if let Some(ref hash) = expected_hash {
    verify_plugin_integrity(source_path, Some(hash))?;
}
```

NOTE: This requires `sha2` and `walkdir` crates. Check if they're already in Cargo.toml. If not, add them. If adding dependencies is too heavy for this phase, create the function but gate it behind a TODO or feature flag.

- [ ] **Step 3: Run compile check**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add src/extension/marketplace/
git commit -m "security: add SHA-256 integrity verification for plugin installs"
```

---

### Task 4: Final validation

- [ ] **Step 1: Run full test suite**

Run: `cargo test -p alephcore --lib`

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -p alephcore -- -W clippy::all`

- [ ] **Step 3: Final commit if needed**

```bash
git add -A && git commit -m "security: fix clippy warnings in P4 changes"
```
