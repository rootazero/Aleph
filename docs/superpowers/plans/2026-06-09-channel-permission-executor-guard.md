# Channel Permission Executor Wiring Regression Guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a regression guard for the executor's external-channel permission wiring by extracting the `channel_id → (caller_role, locked_workspace)` decision into one testable pure function and pinning its behavior — especially the unconfigured-channel-defaults-to-`"guest"` security line.

**Architecture:** The 2-layer channel permission feature is already implemented and merged. The only untested seam is `executor.rs` mapping a channel config into the `RunRequest` (`caller_role` metadata + `workspace_override`). Extract that 3-line inline block into a private free function `channel_run_identity(&HashMap<String, ChannelConfig>, &str) -> (&'static str, Option<PathBuf>)`, call it from the executor, and unit-test it directly (no router/dispatcher harness needed).

**Tech Stack:** Rust, `cargo` workspace crate `alephcore`. Module: `src/gateway/inbound_router/executor.rs`.

**⚠️ Resource directive (overrides skill defaults):** The task owner explicitly requires **no `cargo check` / `cargo test` runs before commit**. This is a higher-priority instruction than the writing-plans skill's "run to verify" steps. Therefore the verify-by-running steps below are marked **DEFERRED** — write the test as the behavioral contract, write the implementation, and commit. Compilation/test verification is the owner's responsibility post-merge.

---

### Task 1: Extract `channel_run_identity` helper + regression tests

**Files:**
- Modify: `src/gateway/inbound_router/executor.rs` (imports at lines 7-17; inline block at lines 238-246 and `workspace_override` at line 276; add a `#[cfg(test)]` module at end of file)

**Context for the implementer:**
- The impl block `impl InboundMessageRouter { ... }` owns a field `channel_configs: HashMap<String, ChannelConfig>`.
- `ChannelConfig` is defined in `src/gateway/inbound_router/types.rs` and has:
  - `caller_role_str(&self) -> &'static str` (delegates to `permission_level`; `Config → "operator"`, `Chat → "guest"`).
  - `resolved_default_workspace(&self) -> Option<PathBuf>` (honors only an absolute+existing dir, else `None`).
  - `Default` impl → `permission_level = Chat`, `default_workspace = None`.
- `ChannelPermissionLevel { Chat, Config }` is also in `types.rs`.
- The current inline code (lines 238-246, 276):

```rust
        let channel_cfg = self
            .channel_configs
            .get(ctx.message.channel_id.as_str())
            .cloned()
            .unwrap_or_default();
        metadata.insert(
            "caller_role".to_string(),
            channel_cfg.caller_role_str().to_string(),
        );
        // ... (~30 lines of other metadata inserts) ...
            workspace_override: channel_cfg.resolved_default_workspace(),
```

- [ ] **Step 1: Add imports needed by the helper**

In `src/gateway/inbound_router/executor.rs`, add `PathBuf` to the std imports and `ChannelConfig` to the `super::types` import.

Change lines 7-8 from:

```rust
use crate::sync_primitives::Arc;
use std::collections::HashMap;
```

to:

```rust
use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::path::PathBuf;
```

Change line 16 from:

```rust
use super::types::{RoutingError, SLASH_COMMAND_MODE_KEY};
```

to:

```rust
use super::types::{ChannelConfig, RoutingError, SLASH_COMMAND_MODE_KEY};
```

- [ ] **Step 2: Add the pure helper function**

Insert this free function immediately **before** the `impl InboundMessageRouter {` line (currently line 19):

```rust
/// Resolve the run identity a channel's inbound messages execute under:
/// the `caller_role` fed to the tool-dispatch config-tier gate, and the
/// workspace it is locked into (Layer-1 lock for Chat tier).
///
/// An **unconfigured** channel (`None` in the map) defaults to Chat
/// (`"guest"`) with no locked workspace. This default is the over-permission
/// fix — a missing config must never be treated as operator. The
/// `permission_wiring_tests` below pin exactly this.
fn channel_run_identity(
    configs: &HashMap<String, ChannelConfig>,
    channel_id: &str,
) -> (&'static str, Option<PathBuf>) {
    let cfg = configs.get(channel_id).cloned().unwrap_or_default();
    (cfg.caller_role_str(), cfg.resolved_default_workspace())
}
```

- [ ] **Step 3: Call the helper from the executor (replace the inline block)**

Replace the inline `channel_cfg` binding + the `caller_role` insert (current lines 238-246) with a single call that destructures both halves. The new code (keep the surrounding comment that explains the tiering):

```rust
        // Stamp this channel's permission tier as the run's caller_role so the
        // tool-dispatch config gate (tools/scoped/dispatch.rs) applies uniformly
        // to external-channel messages. Unconfigured channels default to Chat
        // ("guest") — closing the prior over-permission where a missing role was
        // treated as operator. An operator opts a channel up to Config tier via
        // `permission_level = "config"` in its config block.
        let (caller_role, channel_workspace) =
            channel_run_identity(&self.channel_configs, ctx.message.channel_id.as_str());
        metadata.insert("caller_role".to_string(), caller_role.to_string());
```

Then change the `RunRequest` construction (current line 276) from:

```rust
            workspace_override: channel_cfg.resolved_default_workspace(),
```

to:

```rust
            workspace_override: channel_workspace,
```

Note: `channel_workspace` is `Option<PathBuf>` (owned), held across the ~30 lines of intervening metadata inserts — no borrow issues. After this edit there must be **no remaining references** to the old `channel_cfg` binding (it is fully replaced).

- [ ] **Step 4: Add the regression test module**

Append to the end of `src/gateway/inbound_router/executor.rs`:

```rust
#[cfg(test)]
mod permission_wiring_tests {
    use super::channel_run_identity;
    use crate::gateway::inbound_router::types::{ChannelConfig, ChannelPermissionLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// THE security line: a channel with no registered config runs as the gated
    /// "guest" tier with no locked workspace. A regression here silently reopens
    /// the external-channel over-permission hole.
    #[test]
    fn unconfigured_channel_defaults_to_guest_with_no_workspace() {
        let empty: HashMap<String, ChannelConfig> = HashMap::new();
        assert_eq!(channel_run_identity(&empty, "telegram"), ("guest", None));

        // Unknown id in a populated map → still the safe default.
        let mut configs = HashMap::new();
        configs.insert("slack".to_string(), ChannelConfig::default());
        assert_eq!(channel_run_identity(&configs, "telegram"), ("guest", None));
    }

    /// A Chat-tier channel with an absolute, existing default_workspace is
    /// stamped "guest" and pinned to that workspace (Layer-1 lock).
    #[test]
    fn chat_tier_channel_locks_to_default_workspace() {
        let ws = std::env::temp_dir(); // absolute + existing
        let mut configs = HashMap::new();
        configs.insert(
            "telegram".to_string(),
            ChannelConfig {
                permission_level: ChannelPermissionLevel::Chat,
                default_workspace: Some(ws.clone()),
                ..Default::default()
            },
        );
        assert_eq!(
            channel_run_identity(&configs, "telegram"),
            ("guest", Some(ws))
        );
    }

    /// A Config-tier channel is stamped "operator" (Layer-2). With no
    /// default_workspace set it carries no lock (the agent default applies).
    #[test]
    fn config_tier_channel_maps_to_operator() {
        let mut configs = HashMap::new();
        configs.insert(
            "ops-bot".to_string(),
            ChannelConfig {
                permission_level: ChannelPermissionLevel::Config,
                ..Default::default()
            },
        );
        let (role, workspace) = channel_run_identity(&configs, "ops-bot");
        assert_eq!(role, "operator");
        assert_eq!(workspace, None::<PathBuf>);
    }
}
```

- [ ] **Step 5: Verify by running tests — DEFERRED per resource directive**

Per the task owner's explicit instruction, **do not run `cargo test` / `cargo check`** before commit. The intended command (for the owner to run post-merge) would be:

```bash
cargo test -p alephcore permission_wiring_tests
```

Expected (post-merge): 3 tests pass. Skip executing this now.

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-channel-perm-guard
git add src/gateway/inbound_router/executor.rs
git commit -m "gateway: guard channel→caller_role/workspace executor wiring

Extract the inline channel_id→(caller_role, workspace_override) decision
into a testable channel_run_identity() helper and pin the unconfigured-
channel-defaults-to-guest security line with unit tests. Behavior is
byte-identical; this only adds the missing regression guard for the
external-channel permission wiring."
```

---

## Self-Review

**1. Spec coverage:**
- Spec "Fix: extract a testable pure function `channel_run_identity`" → Task 1 Steps 1-3. ✓
- Spec "Tests (new): unconfigured→guest/None; Chat→guest/workspace; Config→operator" → Task 1 Step 4 (three tests, exactly these cases). ✓
- Spec "Entropy reduction: inline block collapses into one named helper" → Task 1 Step 3 removes the `channel_cfg` binding entirely. ✓
- Spec "Non-gaps B/C: no change" → no task touches `types.rs` comment or `origin_fanout.rs`. ✓ (recorded in spec, intentionally absent from plan)
- Spec "no cargo check before commit" → Step 5 DEFERRED. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to". Every code step shows complete real code. ✓

**3. Type consistency:**
- Helper signature `channel_run_identity(&HashMap<String, ChannelConfig>, &str) -> (&'static str, Option<PathBuf>)` is identical in the definition (Step 2), the call site (Step 3), and all test calls (Step 4). ✓
- `ChannelConfig`, `ChannelPermissionLevel`, `caller_role_str`, `resolved_default_workspace`, `default_workspace`, `permission_level` all match the names verified in `types.rs`. ✓
- Tuple return ordering `(role, workspace)` consistent between definition, call-site destructure, and test assertions. ✓
