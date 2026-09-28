# Discord R2 — Line 3: Session-Key SSOT + Group Policy Module (D8)

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract Discord session-key concatenation logic (currently scattered across `inbound_router` and 3 handler branches) into a single-source `session_key.rs`. Extract the DM-vs-guild-vs-thread group policy decisions (currently hard `if` chain) into a dedicated `group_policy.rs`.

**Architecture:** Two new sibling modules under `src/gateway/interfaces/discord/`. Pure refactor — no behaviour change for existing tests; the existing 3 discord integration tests must stay green.

**Tech Stack:** Rust (no new deps)

**Spec:** `docs/superpowers/specs/2026-09-27-discord-r2-backlog.md` (D8 section). Reference: openclaw `session-key-normalization.ts` (~120 LOC) + `group_policy.ts` (~224 LOC).

**Hard rule from R1 lesson:** every task ends with `cargo test --lib <new module>` + run the existing discord integration tests to confirm no regression.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/interfaces/discord/session_key.rs` | Create | Pure function `key_for(channel_kind, ids) -> String` covering DM/guild/thread |
| `src/gateway/interfaces/discord/group_policy.rs` | Create | `GroupPolicy` enum + `decide(ctx) -> GroupDecision` covering allow-list / silence / rate-limit / ignore |
| `src/gateway/interfaces/discord/mod.rs::inbound_router` | Modify | Replace 3 inline `format!` calls with `session_key::key_for(...)` |
| `src/gateway/interfaces/discord/mod.rs` | Modify | Replace inline `if channel.is_dm()` chain with `group_policy::decide(ctx)` |
| `tests/discord_session_key_test.rs` | Create | 6 unit tests (3 session_key shapes × 2 group_policy branches) |

---

## Task 1: Create `session_key.rs`

**Why:** R1 left three separate `format!("dm:{}", user_id)` / `format!("{}#{}", guild_id, channel_id)` / `format!("{}+{}", channel_id, thread_id)` call-sites. If the format changes (e.g. to add a shard id), all three need editing. SSOT.

**Interface contract:**
```rust
pub enum ChannelKind { DirectMessage, GuildChannel, Thread }
pub struct ChannelContext {
    pub kind: ChannelKind,
    pub user_id: u64,      // always present
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub thread_id: Option<u64>,
}
pub fn key_for(ctx: &ChannelContext) -> String;
//   DM       → "dm:<user_id>"
//   Guild    → "<guild_id>#<channel_id>"
//   Thread   → "<channel_id>+<thread_id>"
```

- [ ] **Step 1:** Create `src/gateway/interfaces/discord/session_key.rs` with struct + enum + signature stubs.
- [ ] **Step 2:** Add `pub mod session_key;` to `src/gateway/interfaces/discord/mod.rs`.
- [ ] **Step 3:** Write 3 tests in module-internal `#[cfg(test)] mod tests`:
  ```rust
  #[test]
  fn dm_key_uses_user_id_only() {
      let ctx = ChannelContext { kind: DirectMessage, user_id: 42, ..default() };
      assert_eq!(key_for(&ctx), "dm:42");
  }
  #[test]
  fn guild_key_uses_hash_format() {
      let ctx = ChannelContext { kind: GuildChannel, user_id: 1, guild_id: Some(7), channel_id: 99, ..default() };
      assert_eq!(key_for(&ctx), "7#99");
  }
  #[test]
  fn thread_key_uses_plus_format() {
      let ctx = ChannelContext { kind: Thread, user_id: 1, channel_id: 99, thread_id: Some(123), ..default() };
      assert_eq!(key_for(&ctx), "99+123");
  }
  ```
- [ ] **Step 4:** Run `cargo test --lib gateway::interfaces::discord::session_key` — expected PASS (3 tests).
- [ ] **Step 5:** Replace the 3 inline `format!` call-sites in `inbound_router` with `session_key::key_for(&ctx)`.
- [ ] **Step 6:** Run existing discord integration tests — expected PASS (no behaviour change).
- [ ] **Step 7:** Commit: `discord(r2-line3): D8 session_key.rs SSOT + inbound_router migration`

---

## Task 2: Create `group_policy.rs`

**Why:** DM-vs-guild-vs-thread decisions (allowed-list check, silent-when-muted, rate-limit by cooldown) live as inline `if/else` blocks in 4 places. Hard to test, hard to extend (e.g. add a new "lurk-only" policy).

**Interface contract:**
```rust
pub enum GroupDecision { Allow, Silenced, RateLimited { retry_after: Duration }, Ignored }
pub struct PolicyContext<'a> {
    pub kind: ChannelKind,
    pub user_id: u64,
    pub is_muted: bool,
    pub is_allowlisted: bool,
    pub last_message_at: Option<Instant>,
    pub now: Instant,
}
pub fn decide(ctx: &PolicyContext) -> GroupDecision;
//   !is_allowlisted (DM/guild)    → Ignored
//   is_muted                       → Silenced
//   last_message_at within 1s      → RateLimited { retry_after }
//   else                            → Allow
```

- [ ] **Step 1:** Create `src/gateway/interfaces/discord/group_policy.rs` with enum + struct + signature stubs.
- [ ] **Step 2:** Add `pub mod group_policy;` to `mod.rs`.
- [ ] **Step 3:** Write 4 tests in module-internal tests:
  ```rust
  #[test]
  fn non_allowlisted_user_is_ignored()
  #[test]
  fn muted_user_is_silenced()
  #[test]
  fn rapid_repeat_is_rate_limited_with_retry_after()
  #[test]
  fn normal_allowlisted_user_is_allowed()
  ```
- [ ] **Step 4:** Run `cargo test --lib gateway::interfaces::discord::group_policy` — expected PASS (4 tests).
- [ ] **Step 5:** Replace the inline if-chain in `mod.rs` (search `if channel.is_dm()` or similar) with `group_policy::decide(&ctx)`. Pattern-match the result and act on each branch.
- [ ] **Step 6:** Run existing discord integration tests — expected PASS.
- [ ] **Step 7:** Commit: `discord(r2-line3): D8 group_policy.rs module + mod.rs policy call-site extraction`

---

## Task 3: Integration test pinning both modules together

**Files:**
- Create: `tests/discord_session_key_test.rs`

- [ ] **Step 1:** Create test file with **integration-level** (not module-internal) tests exercising both modules through a mock `InboundMessage`.
- [ ] **Step 2:** Add 3 tests:
  - `dm_user_with_allowlist_decision_routes_to_inbound`
  - `guild_muted_user_is_silenced_not_routed`
  - `thread_rapid_repeat_is_rate_limited`
- [ ] **Step 3:** Run `cargo test --test discord_session_key_test` — expected PASS (3 tests).
- [ ] **Step 4:** Run full discord test suite: `cargo test --lib gateway::interfaces::discord && cargo test --test 'discord_*'` — expected ALL GREEN.
- [ ] **Step 5:** Commit: `discord(r2-line3): D8 integration tests pinning session_key + group_policy`

---

## Verification (run before merge)

```bash
CARGO_BUILD_JOBS=1 cargo check -p alephcore 2>&1 | tail -3
CARGO_BUILD_JOBS=1 cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -3
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::interfaces::discord 2>&1 | tail -10
CARGO_BUILD_JOBS=1 cargo test -p alephcore --test discord_session_key_test 2>&1 | tail -5
```

Expected: lib ≥ 84 passed + integration 3 passed; no regressions in `mod.rs` call-sites.

## Merge

Worktree `discord-r2-line3` → `merge: discord R2 line 3 (D8 session_key + group_policy SSOT)`.

## Review Focus

1. **`ChannelContext::default()` for tests** — must be `#[derive(Default)]` with sensible zero values; if not, the tests above won't compile.
2. **`last_message_at = None` (first message ever) treated as `Allow`** — verify in the policy; the spec says "first message never rate-limited" but the code should make this explicit, not implicit.
3. **Thread session-key collision with guild channel** — if `channel_id=99` (guild) and `thread_id=99` (thread on different channel), they collide because both use `<channel_id>+...`. Verify whether this is acceptable (likely yes — threads are children of channels, but flag it).

These three failure modes are not covered by the unit tests above.
