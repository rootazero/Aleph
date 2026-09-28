# Discord R2 — Line 1: Audit Hooks + Startup Audit + mark_event Wiring

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close R1 audit/wiring debt (D2, D3, D6) — `audit_hooks.rs` actually called from `manager.rs`, `startup_audit.rs` actually called from `start()`, and `ReconnectCoordinator::mark_event` actually called on every serenity gateway event.

**Architecture:** Three small wiring tasks in `src/gateway/interfaces/discord/` + `src/exec/manager.rs`. No new modules — only call-sites for code R1 already shipped. All three tasks touch `mod.rs` so they must be committed sequentially in one worktree (`discord-r2-line1`) but each as its own commit.

**Tech Stack:** Rust, tokio, thiserror, security/audit crate (existing)

**Spec:** `docs/superpowers/specs/2026-09-27-discord-r2-backlog.md` (D2 + D3 + D6 sections)

**Hard rule from R1 lesson:** every task ends with `cargo test --lib <new module>` — not just `cargo check`. `cargo check` does not compile `#[cfg(test)]` code; R1 line2 missed 4 broken tests because of this.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/interfaces/discord/security/audit_hooks.rs` | Verify exists (R1 line1) | 4 sync `tokio::spawn` audit helpers (`record_requested`/`record_blocked`/`record_resolved`/`record_draft`) |
| `src/gateway/interfaces/discord/security/startup_audit.rs` | Verify exists (R1 line1) | `for_guild(guild_id, healthy)` + `for_config(cfg)` builders |
| `src/exec/manager.rs` | Modify (D2) | Add `_ = audit_hooks::record_resolved(...)` line at end of `resolve_with_reason` |
| `src/gateway/interfaces/discord/mod.rs::start` | Modify (D3) | After channel initialised, loop over `allowed_guilds` and call `startup_audit::for_guild(...).log().await` |
| `src/gateway/interfaces/discord/mod.rs::Handler` | Modify (D6) | Add `self.reconnect.mark_event()` in `message`, `interaction_create`, and `ready` (3 call-sites) |
| `src/gateway/interfaces/discord/reconnect.rs` | Verify (R1 line2) | `mark_event()` already exists — fix last bug from R1 (`is_zombie` now uses nanos) |

---

## Task 1: D2 — Wire `audit_hooks::record_resolved` into `manager.rs::resolve_with_reason`

**Files:**
- Modify: `src/exec/manager.rs` — append 3-5 lines at end of `resolve_with_reason` (the one confluence point for by-id/cascade resolution)
- Verify: `src/gateway/interfaces/discord/security/audit_hooks.rs::record_resolved` already exists

**Interface contract (already defined by R1 line1):**
```rust
pub fn record_resolved(
    actor_user: &str,
    action: &str,
    decision: AuditDecision,    // Allowed | Blocked
    reason: Option<&str>,
    correlation_id: Option<&str>,
);
```

- [ ] **Step 1:** Read `src/exec/manager.rs::resolve_with_reason` end of function — identify the local variables holding actor_user, decision, reason, correlation_id.
- [ ] **Step 2:** Add `use crate::gateway::interfaces::discord::security::audit_hooks::record_resolved;` import (or full path inline).
- [ ] **Step 3:** Append at function end:
  ```rust
  record_resolved(
      actor_user,
      "exec.resolve_with_reason",
      if approved { AuditDecision::Allowed } else { AuditDecision::Blocked },
      reason.as_deref(),
      correlation_id.as_deref(),
  );
  ```
- [ ] **Step 4:** Verify `cargo test --lib exec::manager::tests::resolve_with_reason_*` still green.
- [ ] **Step 5:** Verify `cargo test --lib gateway::interfaces::discord::security::audit_hooks` green.
- [ ] **Step 6:** Commit: `discord(r2-line1): D2 wire audit_hooks::record_resolved into manager.rs::resolve_with_reason`

---

## Task 2: D3 — Wire `startup_audit::for_guild` into `DiscordChannel::start`

**Files:**
- Modify: `src/gateway/interfaces/discord/mod.rs::start` — after guilds initialised, loop and call `startup_audit::for_guild(guild_id, healthy).log().await` for each allowed guild

**Interface contract (already defined by R1 line1):**
```rust
pub struct StartupAuditEntry { ... }
impl StartupAuditEntry {
    pub fn for_guild(guild_id: u64, healthy: bool) -> Self;
    pub async fn log(self);
}
pub async fn for_config(cfg: &DiscordConfig) -> Vec<StartupAuditEntry>;
```

- [ ] **Step 1:** Read `src/gateway/interfaces/discord/mod.rs::start` — find the section after `allowed_guilds` is populated (search `for guild_id in allowed_guilds` or similar). If absent, find where `self.client.start().await` is called.
- [ ] **Step 2:** Add loop AFTER client start, BEFORE returning Ok:
  ```rust
  for guild_id in &self.allowed_guilds {
      let entry = startup_audit::StartupAuditEntry::for_guild(*guild_id, true);
      entry.log().await;
  }
  ```
- [ ] **Step 3:** Verify with `cargo check -p alephcore` that types resolve.
- [ ] **Step 4:** Verify `cargo test --lib gateway::interfaces::discord::security::startup_audit` green (these tests exercise `for_config` and `for_guild` directly).
- [ ] **Step 5:** Add 1 new test in `startup_audit.rs`: `for_config_with_empty_allowed_guilds_emits_zero_entries` (covers the empty-list edge).
- [ ] **Step 6:** Commit: `discord(r2-line1): D3 wire startup_audit::for_guild into DiscordChannel::start (5 lines)`

---

## Task 3: D6 — Wire `ReconnectCoordinator::mark_event` into Handler events

**Files:**
- Modify: `src/gateway/interfaces/discord/mod.rs::Handler` — add `self.reconnect.mark_event()` in three serenity event handlers

**Interface contract (already defined by R1 line2 + R1 fixup):**
```rust
impl ReconnectCoordinator {
    pub fn mark_event(&self);  // atomic store of now_nanos; no-op when not yet started
    pub fn is_zombie(&self) -> bool;  // nanos-comparison, sub-second aware (R1 fixup)
}
```

- [ ] **Step 1:** Read `src/gateway/interfaces/discord/mod.rs` — find `impl EventHandler for Handler` block. Identify `message`, `interaction_create`, `ready` functions.
- [ ] **Step 2:** In each of those 3 functions, add at the top (after early-return guards but before any heavy work):
  ```rust
  if let Some(rc) = &self.reconnect { rc.mark_event(); }
  ```
- [ ] **Step 3:** Verify with `cargo check -p alephcore`.
- [ ] **Step 4:** Verify `cargo test --lib gateway::interfaces::discord::reconnect` green (covers `mark_event` + `is_zombie` interaction).
- [ ] **Step 5:** Commit: `discord(r2-line1): D6 wire ReconnectCoordinator::mark_event into Handler (3 call-sites)`

---

## Verification (run before merge)

```bash
CARGO_BUILD_JOBS=1 cargo check -p alephcore 2>&1 | tail -3
CARGO_BUILD_JOBS=1 cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -3
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::interfaces::discord 2>&1 | tail -10
```

Expected: `test result: ok. N passed; 0 failed; 0 ignored` (N ≥ 84 — current R1 baseline).

## Merge

After all 3 tasks green, worktree is `discord-r2-line1`. Branch merged into `main` like R1 (single merge commit: `merge: discord R2 line 1 (D2+D3+D6 audit/wiring)`).

## Review Focus

1. **`mark_event` on `ready` firing before `ReconnectCoordinator` is set** — verify the `if let Some(rc)` guard is in place.
2. **`record_resolved` double-counting** — `resolve_with_reason` may be called from cascade paths; ensure the audit entry uses `correlation_id` (not caller id) so the audit trail stays dedupeable.
3. **`startup_audit` running on every reconnect** — `start()` may be called by reconnect logic; if so, the audit repeats. Add a `started_once: AtomicBool` gate, OR move the audit to `DiscordChannel::new`.

These three failure modes aren't covered by any test in this plan — add a one-line test each if you find them real.
