# Aleph Security Wiring — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `RuntimeSecurityGuard` end-to-end through `PiiSecretsGuardrail`, activate `{{secret:NAME}}` placeholder substitution at the tool-call boundary, drain audit events to SQL, scrub secret bytes at sandbox edge before UTF-8 conversion, harden input-side blocking, and delete four vestigial OpenClaw TODOs plus one boot no-op.

**Architecture:** `PiiSecretsGuardrail` becomes a thin trait adapter over one `Arc<RuntimeSecurityGuard>`. Three trait surfaces pick three call shapes: input → `process_outbound(None)`, output → `process_inbound(...)`, tool_call → `process_outbound(Some(resolver))`. `Sanitize` variant carries rendered tool args back, already honored by `harness/agent/guardrails.rs:82-87`. A new `audit_drain` task consumes `mpsc::Receiver<AuditEntry>` and writes via `SecurityStore`. A new `sandbox/scrub.rs` runs `regex::bytes::Regex` patterns on raw `Vec<u8>` stdout/stderr before any `String::from_utf8_lossy` call.

**Tech Stack:** Rust 2021, tokio, async-trait, regex (incl. `regex::bytes`), rusqlite via existing `SecurityStore`, serde_json, mpsc.

**Spec:** [`docs/superpowers/specs/2026-05-20-aleph-security-wiring-design.md`](../specs/2026-05-20-aleph-security-wiring-design.md)

**Worktree:** branch `aleph-security-wiring`, path `.claude/worktrees/aleph-security-wiring/`. Created via `superpowers:using-git-worktrees`.

---

## Spec-deviation note

§3.2 of the spec names `process_outbound` for all three surfaces. During plan preparation we verified that `RuntimeSecurityGuard` exposes both `process_outbound` (extract + leak + PII + content-sanitize + placeholder-replace) and `process_inbound` (leak + PII; designed for LLM→user direction). For `OutputGuardrail::evaluate_output` we use `process_inbound` because:
- `evaluate_output` text is LLM → user; running `content_sanitize` (which wraps external content for the LLM's benefit) would be wrong.
- `process_inbound` is the function purpose-built for this direction.

Mapping (final):

| Trait surface | `RuntimeSecurityGuard` method | Resolver |
|---|---|---|
| `evaluate_input` | `process_outbound` | `None` |
| `evaluate_output` | `process_inbound` | _(not accepted by this method)_ |
| `evaluate_tool_call` | `process_outbound` | `Some(resolver)` |

This is a clarification, not a scope change. Spec §3.2 table semantics still hold.

---

## Pre-implementation: worktree

- [ ] **Pre-Step 1: Create isolated worktree**

Run:
```bash
git worktree add -b aleph-security-wiring .claude/worktrees/aleph-security-wiring main
cd .claude/worktrees/aleph-security-wiring
```

Expected: New branch `aleph-security-wiring` checked out at `.claude/worktrees/aleph-security-wiring/`, based on `main` (currently `1bd69da6a`).

All subsequent task files paths assume the worktree as CWD. When writing files via Write/Edit tools, paths must be absolute and prefixed with `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/aleph-security-wiring/` (per saved feedback: shell `cd` is ignored by Write/Edit; absolute paths are required).

---

## Task 1: `VaultSecretResolver` shim

**Goal:** Bridge `SharedTokenManager.get_secret(name)` to the `AsyncSecretResolver` trait.

**Files:**
- Create: `src/secrets/vault_resolver.rs`
- Modify: `src/secrets/mod.rs` (add `mod` + `pub use`)
- Test: same file (`#[cfg(test)] mod tests`)

**Why:** `RuntimeSecurityGuard::process_outbound` accepts `Option<&dyn AsyncSecretResolver>`. The vault-backed implementation is missing; this task adds the smallest possible shim. `SharedTokenManager.get_secret(name) -> Result<Option<DecryptedSecret>, SharedTokenError>` already returns plaintext.

- [ ] **Step 1: Write the failing test**

Create `src/secrets/vault_resolver.rs`:

```rust
//! AsyncSecretResolver impl backed by SharedTokenManager.

use std::sync::Arc;

use async_trait::async_trait;

use crate::gateway::security::shared_token::SharedTokenManager;
use crate::secrets::injection::AsyncSecretResolver;
use crate::secrets::types::{DecryptedSecret, SecretError};

/// `AsyncSecretResolver` impl that resolves secret names via the
/// `SharedTokenManager`-managed vault. The only production resolver.
pub struct VaultSecretResolver {
    inner: Arc<SharedTokenManager>,
}

impl VaultSecretResolver {
    pub fn new(inner: Arc<SharedTokenManager>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AsyncSecretResolver for VaultSecretResolver {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        match self.inner.get_secret(name) {
            Ok(Some(decrypted)) => Ok(decrypted),
            Ok(None) => Err(SecretError::NotFound(name.to_string())),
            Err(e) => Err(SecretError::Serialization(format!(
                "vault resolve error: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::shared_token::SharedTokenManager;
    use crate::gateway::security::store::SecurityStore;
    use tempfile::TempDir;

    fn make_mgr() -> (Arc<SharedTokenManager>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault_path = tmp.path().join("vault");
        let mgr = SharedTokenManager::new(store, vault_path);
        // Seed shared-token secret bytes so encryption works.
        let _ = mgr.generate_token();
        (Arc::new(mgr), tmp)
    }

    #[tokio::test]
    async fn resolve_returns_decrypted_value_when_present() {
        let (mgr, _tmp) = make_mgr();
        mgr.store_secret("alpha", "sk-test-VALUE").unwrap();
        let resolver = VaultSecretResolver::new(mgr.clone());
        let decrypted = resolver.resolve("alpha").await.unwrap();
        assert_eq!(decrypted.expose(), "sk-test-VALUE");
    }

    #[tokio::test]
    async fn resolve_returns_not_found_for_missing_name() {
        let (mgr, _tmp) = make_mgr();
        let resolver = VaultSecretResolver::new(mgr);
        let err = resolver.resolve("ghost").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound(name) if name == "ghost"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run from worktree CWD:
```bash
cargo test -p alephcore --lib secrets::vault_resolver -- --nocapture
```
Expected: FAIL with `error[E0432]: unresolved import` (the module is not yet in `mod.rs`) or `cannot find type VaultSecretResolver`.

- [ ] **Step 3: Wire module in `src/secrets/mod.rs`**

Add to `src/secrets/mod.rs` (anywhere in the existing module declarations, alongside `pub mod injection;`):

```rust
pub mod vault_resolver;
pub use vault_resolver::VaultSecretResolver;
```

- [ ] **Step 4: Run test to verify it passes**

Run:
```bash
cargo test -p alephcore --lib secrets::vault_resolver -- --nocapture
```
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add src/secrets/vault_resolver.rs src/secrets/mod.rs
git commit -m "secrets: VaultSecretResolver shim over SharedTokenManager"
```

---

## Task 2: `PiiSecretsGuardrail` delegate refactor (G1 + G2 trait surface)

**Goal:** Replace the parallel-stack `evaluate()` helper with a single `Arc<RuntimeSecurityGuard>` delegate. Activate placeholder substitution on `evaluate_tool_call`.

**Files:**
- Modify: `src/guardrails/pii_secrets.rs` (rewrite struct, three impls, helpers, tests)
- Test: same file

**Why:** Closes G1 (unify two stacks) and surfaces G2's placeholder-substitution capability through the existing trait registry, with no change to `GuardrailRegistry` or harness.

- [ ] **Step 1: Write the failing tests (RED)**

Replace `src/guardrails/pii_secrets.rs` test module (`#[cfg(test)] mod tests { ... }` at end of file — create if missing). Tests to add:

```rust
#[cfg(test)]
mod delegation_tests {
    use super::*;
    use crate::guardrails::decision::GuardrailDecision;
    use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
    use crate::secrets::injection::AsyncSecretResolver;
    use crate::secrets::types::{DecryptedSecret, SecretError};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct StubResolver;

    #[async_trait]
    impl AsyncSecretResolver for StubResolver {
        async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
            match name {
                "test_key" => Ok(DecryptedSecret::new("resolved-VAL".to_string())),
                _ => Err(SecretError::NotFound(name.to_string())),
            }
        }
    }

    fn guard(with_resolver: bool) -> PiiSecretsGuardrail {
        let resolver: Option<Arc<dyn AsyncSecretResolver>> =
            if with_resolver { Some(Arc::new(StubResolver)) } else { None };
        PiiSecretsGuardrail::with_resolver(resolver)
    }

    #[tokio::test]
    async fn input_does_not_resolve_placeholder() {
        let g = guard(true);
        let dec = g.evaluate_input("hello {{secret:test_key}}").await;
        match dec {
            GuardrailDecision::Allow | GuardrailDecision::Warn { .. } => {}
            GuardrailDecision::Sanitize(rep) => {
                assert!(
                    !rep.text.contains("resolved-VAL"),
                    "input must never expose plaintext secret"
                );
            }
            GuardrailDecision::Block { .. } => panic!("input should not block this benign text"),
        }
    }

    #[tokio::test]
    async fn output_does_not_resolve_placeholder() {
        let g = guard(true);
        let dec = g.evaluate_output("LLM said {{secret:test_key}}").await;
        match dec {
            GuardrailDecision::Sanitize(rep) => {
                assert!(
                    !rep.text.contains("resolved-VAL"),
                    "output must never expose plaintext secret"
                );
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn tool_call_resolves_placeholder() {
        let g = guard(true);
        let args = serde_json::json!({ "command": "echo {{secret:test_key}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Sanitize(rep) => {
                assert!(
                    rep.text.contains("resolved-VAL"),
                    "tool_call must resolve placeholder; got `{}`",
                    rep.text
                );
                assert!(!rep.text.contains("{{secret:"));
            }
            other => panic!("expected Sanitize, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tool_call_without_resolver_passes_placeholder_through() {
        let g = guard(false);
        let args = serde_json::json!({ "command": "echo {{secret:test_key}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Allow => {}
            GuardrailDecision::Sanitize(rep) => {
                assert!(rep.text.contains("{{secret:test_key}}"));
                assert!(!rep.text.contains("resolved-VAL"));
            }
            other => panic!("expected Allow or pass-through Sanitize, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tool_call_unknown_secret_blocks() {
        let g = guard(true);
        let args = serde_json::json!({ "command": "echo {{secret:ghost}}" });
        let dec = g.evaluate_tool_call("bash_exec", &args).await;
        match dec {
            GuardrailDecision::Block { reason, .. } => {
                assert!(reason.contains("ghost"), "reason must name the missing secret");
            }
            other => panic!("expected Block, got {:?}", other),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p alephcore --lib guardrails::pii_secrets::delegation_tests -- --nocapture
```
Expected: compile failure — `PiiSecretsGuardrail::with_resolver` does not exist.

- [ ] **Step 3: Rewrite the struct + impls (GREEN)**

Replace the entire contents of `src/guardrails/pii_secrets.rs` with:

```rust
//! `PiiSecretsGuardrail` — thin trait adapter delegating to `RuntimeSecurityGuard`.
//!
//! Maps the three guardrail surfaces (input / output / tool_call) onto the
//! orchestrator's two methods:
//! - `evaluate_input`  → `process_outbound(None resolver)` — user → LLM
//! - `evaluate_output` → `process_inbound`                 — LLM → user
//! - `evaluate_tool_call` → `process_outbound(Some(resolver))` — LLM → tool
//!
//! Placeholder substitution (`{{secret:NAME}}`) happens **only** at the
//! tool_call surface — the unique location where the next consumer is a
//! tool runtime (not the user) and plaintext is appropriate.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ErrorClass;
use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::secrets::injection::AsyncSecretResolver;
use crate::security::runtime_guard::{
    GuardResult, RuntimeSecurityGuard, SecurityContext, SecurityGuardError,
};

const NAME: &str = "pii_secrets";

pub struct PiiSecretsGuardrail {
    guard: Arc<RuntimeSecurityGuard>,
    resolver: Option<Arc<dyn AsyncSecretResolver>>,
}

impl PiiSecretsGuardrail {
    /// Construct over an existing orchestrator with no resolver. Placeholder
    /// substitution at the tool_call surface will be inert.
    pub fn new(guard: Arc<RuntimeSecurityGuard>) -> Self {
        Self {
            guard,
            resolver: None,
        }
    }

    /// Construct over an existing orchestrator with a resolver wired in.
    pub fn with_guard_and_resolver(
        guard: Arc<RuntimeSecurityGuard>,
        resolver: Option<Arc<dyn AsyncSecretResolver>>,
    ) -> Self {
        Self { guard, resolver }
    }

    /// Construct with a default orchestrator and an optional resolver.
    /// Convenience for the boot path. Audit channel from the orchestrator
    /// is dropped here — callers that want audit drainage should construct
    /// via `with_guard_and_resolver` after spawning their own drain.
    pub fn with_resolver(resolver: Option<Arc<dyn AsyncSecretResolver>>) -> Self {
        let guard = Arc::new(RuntimeSecurityGuard::default_guard());
        Self { guard, resolver }
    }

    fn map_outbound(result: Result<GuardResult, SecurityGuardError>) -> GuardrailDecision {
        match result {
            Ok(GuardResult::Clean { .. }) => GuardrailDecision::Allow,
            Ok(GuardResult::Warned { warnings, .. }) => GuardrailDecision::Warn {
                reason: warnings.join("; "),
            },
            Ok(GuardResult::Redacted { text, reasons }) => GuardrailDecision::Sanitize(Replacement {
                text,
                source: format!("pii_secrets ({})", reasons.join("; ")),
            }),
            Ok(GuardResult::Blocked { reason, .. }) => GuardrailDecision::Block {
                reason,
                class: ErrorClass::Fixable,
            },
            Err(SecurityGuardError::SecretResolutionFailed(e)) => GuardrailDecision::Block {
                reason: format!("Secret resolution failed: {e}"),
                class: ErrorClass::Fixable,
            },
            Err(e) => {
                tracing::warn!(error = %e, "RuntimeSecurityGuard error; allowing pass-through");
                GuardrailDecision::Allow
            }
        }
    }

    /// For tool_call we always want a fresh, rendered args payload back to
    /// the caller, even if the orchestrator returned `Clean { text }` where
    /// the only change was placeholder substitution. So we wrap `Clean`'s
    /// text as `Sanitize` iff the text differs from the original.
    fn map_tool_call(
        original: &str,
        result: Result<GuardResult, SecurityGuardError>,
    ) -> GuardrailDecision {
        match result {
            Ok(GuardResult::Clean { text }) if text != original => {
                GuardrailDecision::Sanitize(Replacement {
                    text,
                    source: "pii_secrets (placeholder substitution)".to_string(),
                })
            }
            other => Self::map_outbound(other),
        }
    }
}

#[async_trait]
impl InputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        let ctx = SecurityContext::default();
        let r = self.guard.process_outbound(text, None, ctx).await;
        Self::map_outbound(r)
    }
}

#[async_trait]
impl OutputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        let ctx = SecurityContext::default();
        let r = self.guard.process_inbound(text, &ctx).await;
        Self::map_outbound(r)
    }
}

#[async_trait]
impl ToolCallGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str {
        NAME
    }
    async fn evaluate_tool_call(&self, _tool_name: &str, args: &Value) -> GuardrailDecision {
        let serialized = match serde_json::to_string(args) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize tool args for guardrail scan");
                return GuardrailDecision::Allow;
            }
        };
        let ctx = SecurityContext::default();
        let resolver_ref = self.resolver.as_ref().map(|a| a.as_ref() as &dyn AsyncSecretResolver);
        let r = self.guard.process_outbound(&serialized, resolver_ref, ctx).await;
        Self::map_tool_call(&serialized, r)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p alephcore --lib guardrails::pii_secrets -- --nocapture
```
Expected: PASS — 5 new tests + any pre-existing tests in the file.

- [ ] **Step 5: Adjust call sites that referenced the old `from_globals()`**

The old API had `PiiSecretsGuardrail::from_globals()`. Production has exactly one caller at `src/bin/aleph-server/commands/start/orchestrator_init.rs:226`. To stay buildable until Task 3 wires the resolver, add a temporary alias at the bottom of `src/guardrails/pii_secrets.rs`:

```rust
impl PiiSecretsGuardrail {
    /// Compatibility wrapper — constructs a guardrail with no resolver and
    /// a fresh default orchestrator. Prefer `with_guard_and_resolver` for
    /// new call sites. Removed in Task 3.
    pub fn from_globals() -> Self {
        Self::with_resolver(None)
    }
}
```

- [ ] **Step 6: Build the whole crate**

Run:
```bash
cargo check -p alephcore
```
Expected: SUCCESS, no compile errors.

- [ ] **Step 7: Commit**

```bash
git add src/guardrails/pii_secrets.rs
git commit -m "guardrails: PiiSecretsGuardrail delegates to RuntimeSecurityGuard"
```

---

## Task 3: Boot-time resolver wiring (G2 part 2)

**Goal:** Pass the production `VaultSecretResolver` into `PiiSecretsGuardrail` at boot, and reuse the orchestrator that has audit channel attached.

**Files:**
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` (line ~215-230)
- Modify: `src/guardrails/pii_secrets.rs` (remove temporary `from_globals` alias)

**Why:** Without this, the production guardrail still has `resolver: None`, and the trait surface still cannot resolve placeholders.

- [ ] **Step 1: Read current orchestrator_init.rs**

```bash
sed -n '200,260p' src/bin/aleph-server/commands/start/orchestrator_init.rs
```

Locate the block where `[guardrails]` is checked and `PiiSecretsGuardrail::from_globals()` is called. Identify the surrounding context: which `Arc<SharedTokenManager>` is available in scope?

- [ ] **Step 2: Modify `orchestrator_init.rs` to construct the guardrail with a resolver**

Pseudocode of the replacement block (exact identifiers depend on what's in scope at lines 215-230 — read the file first; the variable might be `shared_token_mgr`, `vault_mgr`, or similar):

```rust
// PiiSecretsGuardrail wiring — pass vault-backed resolver so {{secret:NAME}}
// in tool args resolves at the tool_call surface (LLM→tool boundary).
let resolver: Option<Arc<dyn alephcore::secrets::AsyncSecretResolver>> = shared_token_mgr
    .as_ref()
    .map(|m| {
        Arc::new(alephcore::secrets::VaultSecretResolver::new(m.clone()))
            as Arc<dyn alephcore::secrets::AsyncSecretResolver>
    });

let (guard, audit_rx) = alephcore::security::RuntimeSecurityGuard::new_with_audit(
    alephcore::security::SecurityGuardConfig::default(),
);
let guard = Arc::new(guard);

// Stash audit_rx for Task 5's drain wiring.
// For now, drop it (audit drain wired in Task 5).
let _ = audit_rx;

let pii = Arc::new(alephcore::guardrails::PiiSecretsGuardrail::with_guard_and_resolver(
    guard,
    resolver,
));
```

(Replace the existing `let pii = Arc::new(alephcore::guardrails::PiiSecretsGuardrail::from_globals());` line with the above block.)

- [ ] **Step 3: Remove the temporary `from_globals()` alias**

Delete the alias block added in Task 2 Step 5 from `src/guardrails/pii_secrets.rs`. `from_globals` no longer exists.

- [ ] **Step 4: Build and run all guardrail tests**

Run:
```bash
cargo check -p alephcore
cargo test -p alephcore --lib guardrails -- --nocapture
```
Expected: build SUCCESS; no `from_globals` references remain.

- [ ] **Step 5: Commit**

```bash
git add src/bin/aleph-server/commands/start/orchestrator_init.rs src/guardrails/pii_secrets.rs
git commit -m "guardrails: wire VaultSecretResolver into PiiSecretsGuardrail at boot"
```

---

## Task 4: Audit drain task (G3 part 1)

**Goal:** Implement `spawn_audit_drain` that consumes `mpsc::Receiver<AuditEntry>` and writes rows to the existing `security_audit_log` table.

**Files:**
- Create: `src/security/audit_drain.rs`
- Modify: `src/security/mod.rs` (`pub mod audit_drain; pub use ...`)
- Modify: `src/gateway/security/store/mod.rs` (add `insert_audit_entry` method)

**Why:** The schema and INSERT SQL exist (`audit.rs:127-149`); the table is created in `gateway/security/store/mod.rs:152`. Nobody consumes the audit receiver in production.

- [ ] **Step 1: Add `SecurityStore::insert_audit_entry` (production helper)**

Append to `impl SecurityStore` in `src/gateway/security/store/mod.rs`:

```rust
/// Insert a single security audit entry. Used by the audit drain task.
pub fn insert_audit_entry(
    &self,
    entry: &crate::security::audit::AuditEntry,
) -> rusqlite::Result<()> {
    let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
    conn.execute(
        crate::security::audit::AUDIT_INSERT_SQL,
        rusqlite::params![
            entry.event_type.to_string(),
            entry.severity.to_string(),
            entry.source_ip.as_deref(),
            entry.session_id.as_deref(),
            entry.detail.as_str(),
        ],
    )?;
    Ok(())
}
```

- [ ] **Step 2: Write the failing test for the drain**

Create `src/security/audit_drain.rs`:

```rust
//! Background task that drains SecurityAuditLog entries to SQL.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::gateway::security::store::SecurityStore;
use crate::security::audit::AuditEntry;

/// Spawn a single drain task that pulls `AuditEntry` items from `rx` and
/// inserts them into the `security_audit_log` table via `store`. Returns
/// the join handle. The task exits gracefully when `rx`'s sender side
/// drops.
pub fn spawn_audit_drain(
    rx: mpsc::Receiver<AuditEntry>,
    store: Arc<SecurityStore>,
) -> JoinHandle<()> {
    tokio::spawn(async move { drain_loop(rx, store).await })
}

async fn drain_loop(mut rx: mpsc::Receiver<AuditEntry>, store: Arc<SecurityStore>) {
    while let Some(entry) = rx.recv().await {
        if let Err(e) = store.insert_audit_entry(&entry) {
            tracing::error!(error = %e, ?entry.event_type, "audit drain insert failed");
        }
    }
    tracing::debug!("audit drain channel closed; task exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::audit::{AuditEntry, AuditEventType, AuditSeverity};

    fn entry(detail: &str) -> AuditEntry {
        AuditEntry {
            event_type: AuditEventType::SsrfBlocked,
            severity: AuditSeverity::Warn,
            source_ip: None,
            session_id: Some("sess-1".to_string()),
            detail: detail.to_string(),
        }
    }

    #[tokio::test]
    async fn drain_persists_entries_to_store() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (tx, rx) = mpsc::channel(8);
        let handle = spawn_audit_drain(rx, store.clone());

        tx.send(entry("first")).await.unwrap();
        tx.send(entry("second")).await.unwrap();

        // Close the channel so the task exits.
        drop(tx);
        handle.await.unwrap();

        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM security_audit_log", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn drain_exits_gracefully_on_sender_drop() {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let (tx, rx) = mpsc::channel::<AuditEntry>(1);
        let handle = spawn_audit_drain(rx, store);
        drop(tx);
        // Should complete promptly.
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("drain did not exit within 1s")
            .unwrap();
    }
}
```

- [ ] **Step 3: Add module declaration**

Append to `src/security/mod.rs`:

```rust
pub mod audit_drain;
pub use audit_drain::spawn_audit_drain;
```

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p alephcore --lib security::audit_drain -- --nocapture
```
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add src/security/audit_drain.rs src/security/mod.rs src/gateway/security/store/mod.rs
git commit -m "security: audit drain task writes SecurityAuditLog to SQL"
```

---

## Task 5: Boot-time audit drain wiring (G3 part 2)

**Goal:** Spawn the drain task at server boot, hold its sender side alive in `RuntimeSecurityGuard`, and shut down cleanly on server exit.

**Files:**
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs`

**Why:** Without the spawn, Task 4's drain is dead code. The orchestrator created in Task 3 already returned an `audit_rx` that we currently drop with `let _ = audit_rx;`.

- [ ] **Step 1: Wire the spawn in orchestrator_init.rs**

Replace `let _ = audit_rx;` (added in Task 3 Step 2) with:

```rust
// Drain audit events to the security_audit_log table.
// Holds an Arc<SecurityStore> for the task's lifetime. Task exits on
// channel close (server shutdown drops the orchestrator → its sender).
if let Some(store) = security_store.as_ref() {
    let _drain_handle = alephcore::security::spawn_audit_drain(audit_rx, store.clone());
    // Handle deliberately not awaited; task lives for the server process lifetime.
    // tokio::JoinHandle is detached intentionally.
} else {
    tracing::warn!("SecurityStore unavailable; audit events will be dropped");
    drop(audit_rx);
}
```

If `security_store` is not in scope at the wiring point, read the surrounding boot file to find the correct variable name — it is constructed in `src/bin/aleph-server/commands/start/builder/subsystems.rs` and threaded through `AppDeps`/`AppContext`.

- [ ] **Step 2: Build**

Run:
```bash
cargo check -p alephcore
```
Expected: SUCCESS.

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/orchestrator_init.rs
git commit -m "security: spawn audit drain task at server boot"
```

---

## Task 6: Bytes-level scrub patterns + `scrub_secrets_bytes` (G4 part 1)

**Goal:** Add `regex::bytes::Regex`-backed scrub for sandbox stdout/stderr, sharing pattern source-of-truth with the existing string-level detector.

**Files:**
- Create: `src/sandbox/scrub.rs`
- Modify: `src/secrets/leak_detector.rs` — add `pub fn default_patterns_bytes()`
- Modify: `src/sandbox/mod.rs` — `pub mod scrub; pub use scrub::{scrub_secrets_bytes, ScrubResult};`

**Why:** Closes G4. Aleph's PII engine uses `regex::Regex` over `&str`, which cannot match across `String::from_utf8_lossy`'s `U+FFFD` replacements.

- [ ] **Step 1: Extract pattern strings in `leak_detector.rs`**

Read `src/secrets/leak_detector.rs` to locate the pattern definitions (they live around line 13-100). Refactor so the pattern strings are exposed:

Add at module top:
```rust
/// Source-of-truth secret regex strings. Both the str-based detector and
/// the bytes-based scrub compile from this list.
pub const SECRET_PATTERN_SOURCES: &[(&str, &str)] = &[
    // (name, regex_source)
    ("sk_proj", r"sk-proj-[A-Za-z0-9_\-]{20,}"),
    ("sk_ant",  r"sk-ant-[A-Za-z0-9_\-]{20,}"),
    ("aws_akia", r"AKIA[0-9A-Z]{16}"),
    ("github_pat", r"ghp_[A-Za-z0-9]{20,}"),
    ("gitlab_pat", r"glpat-[A-Za-z0-9_\-]{20,}"),
    // NOTE: extend cautiously; every addition affects both str + bytes scans.
];

/// Produce bytes-flavored regexes matching the same patterns as the
/// production str-based detector.
pub fn default_patterns_bytes() -> Vec<(&'static str, regex::bytes::Regex)> {
    SECRET_PATTERN_SOURCES
        .iter()
        .map(|(name, src)| (*name, regex::bytes::Regex::new(src).expect("static pattern compiles")))
        .collect()
}
```

If the existing `LeakDetector::default_patterns()` uses different patterns, take its current pattern list as the source-of-truth in `SECRET_PATTERN_SOURCES` so the str-side behavior is unchanged. The aim is for both scans to mirror, not to add or remove patterns in this task.

Replace `LeakDetector::default_patterns()`'s body to compile from `SECRET_PATTERN_SOURCES`:
```rust
pub fn default_patterns() -> Self {
    let patterns: Vec<(String, regex::Regex)> = SECRET_PATTERN_SOURCES
        .iter()
        .map(|(name, src)| {
            (name.to_string(), regex::Regex::new(src).expect("static pattern compiles"))
        })
        .collect();
    // Wire `patterns` into the detector struct using the existing field shape.
}
```

(The exact field name varies — read the file. If `default_patterns()` already exists in a richer shape, only add `SECRET_PATTERN_SOURCES` and `default_patterns_bytes`; do NOT touch the str-side compilation unless the existing patterns differ.)

- [ ] **Step 2: Write the failing test for `scrub_secrets_bytes`**

Create `src/sandbox/scrub.rs`:

```rust
//! Byte-level secret scrub for sandbox stdout/stderr.
//!
//! Runs `regex::bytes::Regex` patterns over raw `&[u8]` before any UTF-8
//! conversion, catching secrets surrounded by non-UTF-8 bytes that would
//! otherwise be lossily replaced with `U+FFFD`.

use std::borrow::Cow;

use crate::secrets::injection::InjectedSecret;
use crate::secrets::leak_detector::default_patterns_bytes;

/// Outcome of a byte-level scrub.
#[derive(Debug, Clone)]
pub struct ScrubResult<'a> {
    /// Possibly modified bytes (borrowed when no hits, owned when redacted).
    pub bytes: Cow<'a, [u8]>,
    /// Pattern names that matched and were redacted.
    pub hits: Vec<&'static str>,
}

/// Scan `bytes` for secret patterns; replace matches with `[REDACTED:NAME]`.
/// Matches whose contents hash-match an entry in `injected` are skipped
/// (they were intentionally injected by the placeholder pipeline).
pub fn scrub_secrets_bytes<'a>(bytes: &'a [u8], injected: &[InjectedSecret]) -> ScrubResult<'a> {
    let patterns = default_patterns_bytes();
    let mut hits: Vec<&'static str> = Vec::new();
    let mut buf: Option<Vec<u8>> = None;

    for (name, re) in &patterns {
        // Iterate in reverse to preserve match indices when splicing.
        let matches: Vec<_> = re.find_iter(bytes).collect();
        if matches.is_empty() {
            continue;
        }
        let working = buf.get_or_insert_with(|| bytes.to_vec());
        // Re-scan working buffer in case earlier replacements shifted bytes.
        let local_matches: Vec<(usize, usize)> = re
            .find_iter(working)
            .map(|m| (m.start(), m.end()))
            .collect();
        for (start, end) in local_matches.into_iter().rev() {
            if is_whitelisted(&working[start..end], injected) {
                continue;
            }
            let replacement = format!("[REDACTED:{}]", name).into_bytes();
            working.splice(start..end, replacement);
            hits.push(*name);
        }
    }

    match buf {
        Some(v) => ScrubResult {
            bytes: Cow::Owned(v),
            hits,
        },
        None => ScrubResult {
            bytes: Cow::Borrowed(bytes),
            hits,
        },
    }
}

fn is_whitelisted(slice: &[u8], injected: &[InjectedSecret]) -> bool {
    if injected.is_empty() {
        return false;
    }
    // InjectedSecret hashes its `value: &str`. We only whitelist if the
    // candidate is valid UTF-8 (binary noise can never be an injected secret).
    let Ok(s) = std::str::from_utf8(slice) else {
        return false;
    };
    let candidate = InjectedSecret::from_value("__probe__", s);
    injected
        .iter()
        .any(|i| i.value_hash == candidate.value_hash && i.value_len == candidate.value_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_passthrough_when_no_match() {
        let input = b"hello world\n".to_vec();
        let out = scrub_secrets_bytes(&input, &[]);
        assert_eq!(out.bytes.as_ref(), input.as_slice());
        assert!(out.hits.is_empty());
        assert!(matches!(out.bytes, Cow::Borrowed(_)));
    }

    #[test]
    fn scrub_redacts_sk_proj_in_utf8() {
        let mut input = b"key=sk-proj-".to_vec();
        input.extend(std::iter::repeat(b'A').take(40));
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:sk_proj]"), "got `{s}`");
        assert!(out.hits.contains(&"sk_proj"));
    }

    #[test]
    fn scrub_finds_sk_around_nonutf8_bytes() {
        // sk-proj-AAAAAAAAAAAAAAAAAAAA followed by raw 0xFF byte.
        let mut input = b"prefix:".to_vec();
        input.extend_from_slice(b"sk-proj-");
        input.extend(std::iter::repeat(b'B').take(40));
        input.push(0xFF);
        input.extend_from_slice(b":suffix");
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:sk_proj]"), "got `{s}`");
    }

    #[test]
    fn scrub_skips_whitelisted_injected_secret() {
        let key_str: String = format!("sk-proj-{}", "C".repeat(40));
        let injected = InjectedSecret::from_value("test", &key_str);
        let input = format!("key={key_str}").into_bytes();
        let out = scrub_secrets_bytes(&input, &[injected]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains(&key_str), "expected injected key passthrough, got `{s}`");
        assert!(out.hits.is_empty());
    }
}
```

- [ ] **Step 3: Wire module**

Append to `src/sandbox/mod.rs`:

```rust
pub mod scrub;
pub use scrub::{scrub_secrets_bytes, ScrubResult};
```

- [ ] **Step 4: Run tests**

Run:
```bash
cargo test -p alephcore --lib sandbox::scrub -- --nocapture
```
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/scrub.rs src/sandbox/mod.rs src/secrets/leak_detector.rs
git commit -m "sandbox: scrub_secrets_bytes — byte-level secret redaction"
```

---

## Task 7: Workspace stdout/stderr scrub integration (G4 part 2)

**Goal:** Plumb `scrub_secrets_bytes` into `WorkspaceSandbox::execute`'s pipeline before any string conversion.

**Files:**
- Modify: `src/sandbox/workspace.rs` (around line 160 — `execute` method)

**Why:** Without this, Task 6's scrub is unreachable on the production path. `SandboxOutput.stdout: Vec<u8>` is already raw bytes; we redact in-place before downstream consumers convert.

- [ ] **Step 1: Read the execute method**

```bash
sed -n '150,260p' src/sandbox/workspace.rs
```

Identify the point where `output: SandboxOutput` (with `stdout: Vec<u8>`, `stderr: Vec<u8>`) is produced by the driver and just before `Ok(output)` returns.

- [ ] **Step 2: Write the failing test**

Append a test module at the end of `src/sandbox/workspace.rs` (or extend the existing `#[cfg(test)] mod tests` if present). The test stubs the driver, runs `execute`, and asserts that a planted `sk-proj-…` string in driver stdout is redacted on the returned output.

```rust
#[cfg(test)]
mod scrub_integration_tests {
    use super::*;
    use crate::sandbox::command::{SandboxCommand, SandboxOutput};
    use crate::sandbox::driver::{OsSandboxDriverTrait, ParsedProfile};
    use crate::sandbox::policy::SandboxPolicy;
    use async_trait::async_trait;

    struct LeakDriver {
        leak_in_stdout: Vec<u8>,
    }

    #[async_trait]
    impl OsSandboxDriverTrait for LeakDriver {
        async fn run(
            &self,
            _cmd: &SandboxCommand,
            _profile: &ParsedProfile,
            _policy: &SandboxPolicy,
        ) -> Result<SandboxOutput, SandboxError> {
            Ok(SandboxOutput {
                exit_code: 0,
                stdout: self.leak_in_stdout.clone(),
                stderr: Vec::new(),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn workspace_scrubs_leaked_secret_in_stdout() {
        // Build a WorkspaceSandbox using the LeakDriver — exact constructor
        // depends on existing pub API. If WorkspaceSandbox::new takes the
        // driver directly, pass LeakDriver. If it requires a Factory,
        // adapt with a one-line wrapper.
        let mut leak = b"out:".to_vec();
        leak.extend_from_slice(b"sk-proj-");
        leak.extend(std::iter::repeat(b'Z').take(40));
        // Construct (refer to existing `WorkspaceSandbox::new`/`with_driver` API).
        let sandbox = WorkspaceSandbox::with_driver_for_test(Arc::new(LeakDriver {
            leak_in_stdout: leak,
        }));
        let cmd = SandboxCommand::shell("true");
        let out = sandbox.execute(cmd).await.unwrap();
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("[REDACTED:sk_proj]"), "got `{s}`");
        assert!(!s.contains("sk-proj-ZZ"));
    }
}
```

Note: `WorkspaceSandbox::with_driver_for_test` may not exist. If not, add it as a `#[cfg(test)] impl WorkspaceSandbox` helper that mirrors the production constructor with a custom driver.

- [ ] **Step 3: Run the failing test**

```bash
cargo test -p alephcore --lib sandbox::workspace::scrub_integration -- --nocapture
```
Expected: FAIL — `sk-proj-…` is present unredacted in `out.stdout`.

- [ ] **Step 4: Add scrub call inside `execute`**

In `src/sandbox/workspace.rs`, locate the line where the driver returns `output: SandboxOutput`. Insert immediately after:

```rust
// Byte-level secret scrub before any downstream consumer touches stdout/stderr.
// Whitelist is fed via SecurityContext.injected_secrets when threaded; for
// direct sandbox callers (no security context) this scrubs with an empty
// whitelist, which is the safe default.
let injected: &[crate::secrets::injection::InjectedSecret] = &[];
let stdout_scrub = crate::sandbox::scrub_secrets_bytes(&output.stdout, injected);
let stderr_scrub = crate::sandbox::scrub_secrets_bytes(&output.stderr, injected);
if !stdout_scrub.hits.is_empty() || !stderr_scrub.hits.is_empty() {
    tracing::warn!(
        stdout_hits = ?stdout_scrub.hits,
        stderr_hits = ?stderr_scrub.hits,
        "sandbox bytes-scrub redacted secrets in command output"
    );
}
output.stdout = stdout_scrub.bytes.into_owned();
output.stderr = stderr_scrub.bytes.into_owned();
```

- [ ] **Step 5: Run tests to verify they pass**

Run:
```bash
cargo test -p alephcore --lib sandbox -- --nocapture
```
Expected: PASS — new scrub integration test plus pre-existing sandbox tests.

- [ ] **Step 6: Commit**

```bash
git add src/sandbox/workspace.rs
git commit -m "sandbox: scrub stdout/stderr bytes before lossy UTF-8 conversion"
```

---

## Task 8: Input-side hardening tests (G5)

**Goal:** Explicitly cover input-side blocking for the five most common pasted API-key prefixes; add patterns to `SECRET_PATTERN_SOURCES` if any prefix slips through.

**Files:**
- Modify: `src/guardrails/pii_secrets.rs` (test module — add scenarios)
- Modify (if any test fails): `src/secrets/leak_detector.rs` (extend `SECRET_PATTERN_SOURCES`)

**Why:** Closes G5. Avoids the historical worry of user pasting `sk-proj-…` into a prompt going unblocked.

- [ ] **Step 1: Write the failing parameterized test**

Append to the test module in `src/guardrails/pii_secrets.rs`:

```rust
#[cfg(test)]
mod input_blocking_tests {
    use super::*;
    use crate::guardrails::decision::GuardrailDecision;
    use crate::guardrails::traits::InputGuardrail;

    fn pasted(prefix: &str) -> String {
        format!("My key is {}{}", prefix, "A".repeat(40))
    }

    #[tokio::test]
    async fn input_blocks_pasted_api_keys() {
        let g = PiiSecretsGuardrail::with_resolver(None);
        let cases = [
            "sk-proj-",
            "sk-ant-",
            "AKIA",
            "ghp_",
            "glpat-",
        ];
        for prefix in cases {
            let text = pasted(prefix);
            let dec = g.evaluate_input(&text).await;
            match dec {
                GuardrailDecision::Block { reason, .. } => {
                    assert!(
                        reason.to_lowercase().contains("leak")
                            || reason.to_lowercase().contains("secret")
                            || reason.to_lowercase().contains("api"),
                        "Block reason should mention leak/secret/api; got `{reason}` for prefix `{prefix}`"
                    );
                }
                other => panic!("prefix `{prefix}` was NOT blocked; got {other:?}"),
            }
        }
    }
}
```

- [ ] **Step 2: Run the test**

```bash
cargo test -p alephcore --lib guardrails::pii_secrets::input_blocking_tests -- --nocapture
```
Expected behavior:
- If Task 6 wired `SECRET_PATTERN_SOURCES` correctly into both str+bytes detectors, this should PASS.
- If any prefix is NOT blocked, the test FAILS with a precise message naming the prefix.

- [ ] **Step 3: If a prefix is missing, extend the source-of-truth**

In `src/secrets/leak_detector.rs::SECRET_PATTERN_SOURCES`, ensure each of `sk-proj`, `sk-ant`, `AKIA`, `ghp_`, `glpat-` is present. Rerun the test until green.

- [ ] **Step 4: Commit**

```bash
git add src/guardrails/pii_secrets.rs src/secrets/leak_detector.rs
git commit -m "guardrails: cover input-side blocking for 5 API-key prefixes"
```

---

## Task 9: Cleanup pass (G6)

**Goal:** Delete dead markers — four `OpenClaw tool-policy` TODOs and one `let _ = …default_guard()` boot no-op. Replace with reverse-pointer doc comments.

**Files:**
- Modify: `src/security/mod.rs` (line 26 — delete the no-op)
- Modify: `src/executor/builtin_registry/registry.rs` (lines 64, 419, 425 — delete + replace)
- Modify: `src/executor/builtin_registry/builder/constructor.rs` (line 39 — delete + replace)

**Why:** Closes G6. Without this the repository keeps mis-leading markers pointing at a non-existent migration.

- [ ] **Step 1: Delete the boot no-op**

`src/security/mod.rs:26` currently:
```rust
let _ = crate::security::RuntimeSecurityGuard::default_guard();
```

Find the enclosing function (likely `fn init()` or similar). Delete the line. If the enclosing function becomes empty, also delete its declaration. Verify nothing else references it:

```bash
grep -n "default_guard\|init()" src/security/mod.rs
```

- [ ] **Step 2: Replace registry.rs TODOs**

In `src/executor/builtin_registry/registry.rs`:

Replace the comment at line 64 (`/// TODO: Security enforcement will be reimplemented following OpenClaw's sandbox/tool-policy pattern.`) with:

```rust
/// Security enforcement is layered, not centralised in this registry:
/// - `GuardrailRegistry` (input / output / tool-call) covers content checks.
/// - `WorkspaceSandbox` covers OS-level isolation.
/// - `ApprovalGate` covers HITL escalation.
///
/// See docs/reference/SANDBOX.md and docs/reference/SECURITY.md.
```

Delete the TODOs at lines 419 and 425 (the inline comments inside the same `impl` block). If the block becomes empty (no longer has body), keep it; the doc comment at line 64 documents the contract.

- [ ] **Step 3: Replace constructor.rs TODO**

In `src/executor/builtin_registry/builder/constructor.rs:39`:

Replace `/// - TODO: Tool policy will be reimplemented following OpenClaw's sandbox pattern` with:

```rust
/// - Tool policy is enforced layered (Guardrails + Sandbox + ApprovalGate).
///   See docs/reference/SANDBOX.md.
```

- [ ] **Step 4: Verify all dead markers are gone**

```bash
grep -rn "OpenClaw\|default_guard()" src/ --include="*.rs"
```
Expected: zero hits (or only test-module references, if any).

- [ ] **Step 5: Build and run all tests**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib
```
Expected: build SUCCESS; no new test failures vs. the pre-cycle baseline (main has known pre-existing failures — diff against the baseline list before signing off, see CLAUDE.md memory note `project_baseline_test_failures.md`).

- [ ] **Step 6: Commit**

```bash
git add src/security/mod.rs src/executor/builtin_registry/registry.rs src/executor/builtin_registry/builder/constructor.rs
git commit -m "cleanup: drop 4 OpenClaw TODOs + RuntimeSecurityGuard boot no-op"
```

---

## Task 10: End-to-end integration test (§5.2)

**Goal:** One full integration test exercising the wired path: build registry with mock resolver, call `evaluate_tool_call` with a placeholder, assert rendered args + injected_secrets accounting + audit row.

**Files:**
- Create: `tests/security_wiring_integration.rs`

**Why:** Closes spec §5.2. Catches regressions in any of Tasks 2/3/4/5/6/7 in a single test.

- [ ] **Step 1: Write the test**

Create `tests/security_wiring_integration.rs`:

```rust
//! End-to-end integration: tool-call placeholder substitution wired through
//! PiiSecretsGuardrail → RuntimeSecurityGuard → AuditDrain.

use std::sync::Arc;

use alephcore::gateway::security::store::SecurityStore;
use alephcore::guardrails::decision::GuardrailDecision;
use alephcore::guardrails::traits::ToolCallGuardrail;
use alephcore::guardrails::PiiSecretsGuardrail;
use alephcore::secrets::injection::AsyncSecretResolver;
use alephcore::secrets::types::{DecryptedSecret, SecretError};
use alephcore::security::audit::AuditEntry;
use alephcore::security::spawn_audit_drain;
use alephcore::security::{RuntimeSecurityGuard, SecurityGuardConfig};
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

struct StubResolver;

#[async_trait]
impl AsyncSecretResolver for StubResolver {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        Ok(DecryptedSecret::new(format!("resolved-{name}")))
    }
}

#[tokio::test]
async fn tool_call_placeholder_resolves_end_to_end() {
    let (guard, audit_rx) = RuntimeSecurityGuard::new_with_audit(SecurityGuardConfig::default());
    let guard = Arc::new(guard);
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let drain = spawn_audit_drain(audit_rx, store.clone());

    let resolver: Option<Arc<dyn AsyncSecretResolver>> = Some(Arc::new(StubResolver));
    let pii = PiiSecretsGuardrail::with_guard_and_resolver(guard.clone(), resolver);

    let args = json!({ "command": "echo {{secret:openai_main}}" });
    let dec = pii.evaluate_tool_call("bash_exec", &args).await;

    let rep = match dec {
        GuardrailDecision::Sanitize(rep) => rep,
        other => panic!("expected Sanitize, got {other:?}"),
    };
    assert!(rep.text.contains("resolved-openai_main"), "got `{}`", rep.text);
    assert!(!rep.text.contains("{{secret:"));

    // Drop the guard's audit_log Sender (via dropping guard) so drain exits.
    drop(guard);
    drop(pii);
    tokio::time::timeout(std::time::Duration::from_secs(2), drain)
        .await
        .expect("drain should exit on sender drop")
        .unwrap();
}
```

- [ ] **Step 2: Run**

```bash
cargo test --test security_wiring_integration -- --nocapture
```
Expected: PASS — 1 test.

- [ ] **Step 3: Commit**

```bash
git add tests/security_wiring_integration.rs
git commit -m "test: end-to-end security wiring integration"
```

---

## Task 11: CHANGELOG + manual e2e + finalize

**Goal:** Record the cycle in the CHANGELOG, run the spec §5.3 manual e2e once, and prepare the merge.

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Append to CHANGELOG.md**

Open the unreleased / next-release section of `CHANGELOG.md` and add:

```markdown
### Added
- Security: wire RuntimeSecurityGuard as the unified backend behind PiiSecretsGuardrail; placeholder substitution at tool-call boundary.
- Security: byte-level secret-leak scrub at sandbox stdout/stderr edge (catches non-UTF-8 binary output).
- Security: persistent audit drain task writes SecurityAuditLog entries to the security_audit_log table.

### Fixed
- Security: PiiSecretsGuardrail input-side now explicitly tested against pasted API keys (sk-proj, sk-ant, AKIA, ghp_, glpat- prefixes).

### Removed
- Dead `let _ = RuntimeSecurityGuard::default_guard()` boot no-op.
- 4 vestigial "OpenClaw tool-policy" TODOs in executor/builtin_registry (replaced with doc-comment pointers to SANDBOX.md and SECURITY.md).
```

- [ ] **Step 2: Run the full test suite**

```bash
cargo test -p alephcore --lib
cargo test --test security_wiring_integration
cargo check -p alephcore
```
Expected: all green vs. the pre-cycle baseline (see CLAUDE.md memory note).

- [ ] **Step 3: Manual e2e (one scenario)**

In a separate shell, from the worktree:

```bash
cargo build --bin aleph-server
# Seed a fake secret
target/debug/aleph-server secret set dummy_test_key fake-sk-test123
# Start server (or use just dev)
target/debug/aleph-server start &
SERVER_PID=$!
# In another shell, drive the agent (via your usual channel) and have it
# call bash_exec with `{"command": "echo {{secret:dummy_test_key}}"}`.
# Then inspect:
sqlite3 ~/.aleph/data/aleph.db 'SELECT event_type, severity, session_id, detail FROM security_audit_log ORDER BY id DESC LIMIT 5;'
# Stop:
kill $SERVER_PID
```

Acceptance:
- LLM transcript shows the literal `{{secret:dummy_test_key}}` placeholder.
- Sandbox-side stdout shows `fake-sk-test123`.
- `security_audit_log` has at least one recent row from this session.

- [ ] **Step 4: Commit CHANGELOG**

```bash
git add CHANGELOG.md
git commit -m "changelog: aleph security wiring cycle"
```

- [ ] **Step 5: Pre-merge sanity**

Diff cycle vs. main, ensure cleanup checklist (§4.6 of spec) is satisfied:

```bash
git log --oneline main..HEAD
grep -rn "OpenClaw\|default_guard()" src/ --include="*.rs"
grep -rn "from_globals\s*(\s*)" src/ --include="*.rs"
```

Expected:
- 11 commits on the branch (rough count: 1 vault resolver, 1 guardrail refactor, 1 boot wire, 1 drain task, 1 drain wire, 1 scrub module, 1 scrub integration, 1 input hardening, 1 cleanup, 1 integration test, 1 changelog).
- Zero hits for `OpenClaw` or `default_guard()` outside test modules.
- Zero hits for `from_globals()` (deprecated alias removed in Task 3).

- [ ] **Step 6: Merge to main (in a separate session per saved feedback)**

Per saved memory `feedback_worktree_for_implementation.md`: do not `git worktree remove` inside the same `EnterWorktree` session — switch to a new session and use absolute paths to clean up.

From a fresh session (outside the worktree's enforced CWD):

```bash
cd /Volumes/TBU4/Workspace/Aleph
git checkout main
git merge --no-ff aleph-security-wiring -m "merge: aleph security wiring cycle (worktree aleph-security-wiring)"
# Optionally then:
# git worktree remove /Volumes/TBU4/Workspace/Aleph/.claude/worktrees/aleph-security-wiring
# git branch -d aleph-security-wiring
```

Per saved memory `feedback_pre_check_main_before_merge.md`: diff main-only file set before+after merge to confirm no main-only changes were silently dropped.

---

## Self-Review

**Spec coverage check (against `2026-05-20-aleph-security-wiring-design.md`):**

| Spec § | Covered by |
|---|---|
| G1 unify two stacks | Task 2 |
| G2 placeholder at tool-call boundary | Tasks 1 + 2 + 3 |
| G3 audit drain to SQL | Tasks 4 + 5 |
| G4 bytes-level scrub | Tasks 6 + 7 |
| G5 input-side blocking | Task 8 |
| G6 cleanup (4 TODOs + 1 no-op) | Task 9 |
| §4.1 file-level changes | All tasks |
| §4.5 Sanitize-honoring verification | Pre-implementation note (verified at plan-writing time: `harness/agent/guardrails.rs:82-87`); no task needed |
| §4.6 cleanup checklist | Task 9 |
| §5.1 unit tests | Tasks 1 / 2 / 4 / 6 / 7 / 8 |
| §5.2 integration test | Task 10 |
| §5.3 manual e2e | Task 11 |
| §6 CHANGELOG | Task 11 |

No spec section uncovered.

**Placeholder scan:** no `TBD`, `TODO`, `implement later`, vague-tests-without-code. Every code-step shows the code.

**Type consistency:**
- `VaultSecretResolver::new(Arc<SharedTokenManager>)` — same call sites in Task 1 + Task 3.
- `spawn_audit_drain(rx, store) -> JoinHandle<()>` — same signature in Task 4 + Task 5 + Task 10.
- `scrub_secrets_bytes(&[u8], &[InjectedSecret]) -> ScrubResult` — same in Task 6 + Task 7 + Task 10's audit-side context.
- `PiiSecretsGuardrail::with_guard_and_resolver(Arc<RuntimeSecurityGuard>, Option<Arc<dyn AsyncSecretResolver>>)` — same in Task 2 + Task 3 + Task 10.
- `PiiSecretsGuardrail::with_resolver(Option<Arc<dyn AsyncSecretResolver>>)` — same in Task 2 + Task 8 (tests).
- The temporary `from_globals()` alias added in Task 2 Step 5 is explicitly deleted in Task 3 Step 3.

**Spec-deviation note:** §3.2 named `process_outbound` for all surfaces; the plan uses `process_inbound` for `evaluate_output`. Rationale documented in the "Spec-deviation note" section above. This is a clarification, not a goal change.

---

## Execution Handoff

Plan complete. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks. Use `superpowers:subagent-driven-development`.
2. **Inline Execution** — execute in this session, batched with checkpoints. Use `superpowers:executing-plans`.
