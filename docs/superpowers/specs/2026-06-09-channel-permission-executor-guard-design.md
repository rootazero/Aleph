# Channel Permission — Executor Wiring Regression Guard

**Date:** 2026-06-09
**Branch:** `feat/channel-perm-executor-guard`
**Scope:** A single regression guard. The requested 2-layer channel permission
feature is **already implemented, merged, and well-tested** (commit `298c57135`,
merge `f97b2b94f`, prior spec `2026-06-09-channel-2layer-permission-design.md`).
This round adds the one missing test seam and records why two other suspected
gaps are non-gaps.

## Why this is a tiny spec (gap analysis)

The four requested capabilities were verified present on `main` by reading the
code path end-to-end:

1. **Panel-remote reuses the same permission logic as channels** — both the WS
   path and the external-channel path converge on the single chokepoint
   `caller_role` → `TurnContext::caller_is_operator()` → `tools/scoped/dispatch.rs`
   config-tier gate.
   - `connect.rs`: loopback bootstrap → `default_tier(kind, true)` → Config →
     `role="operator"` (`:221`, `:303`); remote device token →
     `role_for_permissions(device.permissions)` (`:473`), default Chat="guest",
     elevated via operator-only `devices.set_level`.
   - `handler.rs:1052-1058`: at dispatch, `caller_role` is read from
     `ConnectionState.role` (only when auth is required; a no-auth local daemon
     stays `None` = trusted) and scoped into `CALLER_ROLE` (`:1117`).
2. **2-layer model** — `ChannelPermissionLevel{Chat, Config}` with `Chat` =
   converse + read + locked `default_workspace`, `Config` = config tools + free
   workspace (`inbound_router/types.rs:31-117`).
3. **Workspace lock / free-create by tier** — `agent.rs:218-249` gates a
   `project_root` override on operator/config tier; external channels are pinned
   to `resolved_default_workspace()` in `executor.rs:276`.
4. **Reserved for future multi-level** — `ChannelPermissionLevel` is an enum;
   adding tiers is additive.

### Existing test coverage (already green)

- `types.rs` `permission_tier_tests`: default Chat; `caller_role_str` mapping
  (Chat→guest, Config→operator); `ChannelPolicyConfig` flat-key deserialization;
  `is_default`; `resolved_default_workspace` rejects relative/missing, honors
  absolute+existing.
- `agent.rs`: `chat_tier_caller_cannot_choose_project_root` (`:710`),
  `config_tier_caller_may_choose_project_root` (`:740`).
- `tools/scoped` guest-gating tests (config-tool path).

## The one real gap (A)

The executor's **wiring** of those tested helpers into the `RunRequest` has no
regression guard. `executor.rs:238-246` + `:276`:

```rust
let channel_cfg = self.channel_configs
    .get(ctx.message.channel_id.as_str())
    .cloned()
    .unwrap_or_default();                      // <- unconfigured channel = Chat ("guest")
metadata.insert("caller_role", channel_cfg.caller_role_str().to_string());
// ...
workspace_override: channel_cfg.resolved_default_workspace(),
```

The `unwrap_or_default()` is the **load-bearing security line**: it is what makes
an unconfigured external channel default to the gated `"guest"` tier (the
over-permission fix). If a refactor drops or alters this, every component-level
helper test still passes while the over-permission hole silently reopens. There
is no test asserting the executor actually maps `channel_id` →
`(caller_role, workspace_override)` this way.

### Fix: extract a testable pure function

Replace the inline `get(...).unwrap_or_default()` + two field reads with one
small private helper in `executor.rs` (behavior byte-identical — pure entropy
reduction + testability):

```rust
/// Resolve the run identity a channel's inbound messages execute under.
/// An **unconfigured** channel defaults to Chat ("guest") with no locked
/// workspace — this default is the over-permission fix and is what the
/// regression test below pins.
fn channel_run_identity(
    configs: &HashMap<String, ChannelConfig>,
    channel_id: &str,
) -> (&'static str, Option<PathBuf>) {
    let cfg = configs.get(channel_id).cloned().unwrap_or_default();
    (cfg.caller_role_str(), cfg.resolved_default_workspace())
}
```

The executor calls it once and uses both halves (caller_role into metadata,
workspace into `workspace_override`).

### Tests (new)

A `#[cfg(test)]` module in `executor.rs` covering the承重 default and both tiers:

- **Unconfigured channel** (empty map / unknown id) → `("guest", None)`. This is
  the hardening — the most important assertion.
- **Chat-tier registered with an absolute+existing `default_workspace`** →
  `("guest", Some(ws))`.
- **Config-tier registered** → `("operator", _)`.

These use `ChannelConfig` values directly (no `AgentRunManager` / dispatcher
harness needed), matching the lightweight style of the existing `types.rs` tier
tests.

## Non-gaps (verified, intentionally not changed)

- **Comment "byte-identical"** (`types.rs:58-60`): accurate. Both
  `caller_role_str` and `tier::role_for_permissions` yield the literal
  `"operator"`/`"guest"` (`auth_probe_tests.rs:982-984` asserts the latter). No
  change.
- **Fanout caller_role re-check** (`event_emitter/origin_fanout.rs`): the
  `OriginFanoutEmitter` only mirrors a run's **final response text** to the
  origin channel on `RunComplete` (`:99-105`); it dispatches no tools and starts
  no run. The permission gate already applied during the run itself, so there is
  no permission to "re-check" on the fan-out — adding a check would be a no-op
  guard with zero consumers (R10 YAGNI). No change. Recorded here so it is not
  re-flagged.

## Entropy reduction

- Inline `get(...).unwrap_or_default()` + duplicated field reads collapse into
  one named, documented, tested helper. No new abstraction beyond a private fn
  that already-existing code paths fold into.

## Out of scope

- Any new permission tier beyond Chat/Config (the enum already reserves space).
- Multi-end session sync, cross-channel delivery receipts (separate specs,
  already addressed in `channel-session-origin-binding`).
- Per the task's resource directive: **no `cargo check` / test run** before
  commit.
