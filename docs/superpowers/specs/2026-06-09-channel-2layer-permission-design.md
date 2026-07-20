# Channel 2-Layer Permission Model — Design

**Date:** 2026-06-09
**Branch:** `feat/channel-2layer-permission`
**Scope (this round):** Permission layering only. Multi-end sync gaps (session-scoped
broadcast, cross-channel delivery receipts) are deferred to a separate spec.

## Goal

Give every channel a two-layer permission model, reusing Aleph's **existing**
Chat/Config device tier instead of inventing a parallel concept:

- **Layer 1 — 对话权限 (Chat tier)**: converse + read-only. Locked to a **default
  working directory**. Cannot run config-mutating tools, cannot choose/create an
  arbitrary workspace.
- **Layer 2 — 配置权限 (Config tier)**: everything in Layer 1 **plus** the
  "Everything is a Tool" config tools (`self_config`, `agent_create`, `skill_*`,
  …) **and** the freedom to override/create the working directory.

Requirement ③ (Panel/shell connects to Core remotely): local Panel (loopback) = Config;
remote Panel = Chat by default, elevated to Config via owner approval
(`devices.set_level`, already operator-only). Workspace-free-creation is a pure
function of effective tier — elevation grants it automatically, no separate flag.

## Why reuse the existing tier (not a new ChannelPermissionLevel system)

Aleph already has the complete machinery:

- `gateway/handlers/auth/tier.rs` — `Tier::{Chat, Config}`; `default_tier(kind, is_loopback)`
  → loopback=Config, remote=Chat. Role string `"operator"` / `"guest"`.
- `gateway/method_authz.rs` — `OPERATOR_TOOLS` (self_config, agent_create, skill_*, …),
  `tool_requires_operator(name)`.
- `gateway/caller_identity.rs` — `CALLER_ROLE` task-local; `current_caller_role()`.
- `tools/turn_context.rs` — `TurnContext.caller_role` → `caller_is_operator()`.
- `tools/scoped/dispatch.rs` — the config-tier gate: a non-operator (`"guest"`) caller
  asking for an `OPERATOR_TOOL` is either routed to live operator approval
  (Phase 2b `config_approval_requester`) or hard-rejected (fail-closed).

So "configuration permission" (Layer 2) is **already enforced** for the WS path. Two
gaps remain:

1. **External channels bypass the gate.** The inbound executor
   (`inbound_router/executor.rs`) builds `RunRequest` directly with **no** `caller_role`
   in metadata. `TurnContext.caller_role == None` ⇒ `caller_is_operator() == true`.
   Result: every Telegram/Slack/iMessage message is silently treated as **operator** —
   it can run config tools today. This is exactly the over-permission Layer 1 must close.

2. **Workspace dimension is ungated.** `agent.rs::start_run` validates a
   `project_root` override for *absoluteness + existence* but **never checks caller
   tier**. A remote Chat-tier Panel can pass `project_root` and escape its default
   workspace. Layer 1 must lock the workdir; Layer 2 must allow override.

## Architecture — 4 connection points, minimal new code

### 1. Channel schema (`inbound_router/types.rs`)

Add to the existing `ChannelConfig` (precedent: `SlashAccessConfig` was added here the
same way, mirroring hermes-agent):

```rust
pub enum ChannelPermissionLevel { Chat, Config }   // Default = Chat (safe-by-default)
struct ChannelConfig {
    …existing…
    permission_level: ChannelPermissionLevel,       // default Chat
    default_workspace: Option<PathBuf>,             // Layer-1 locked workdir
}
fn caller_role_str(&self) -> &'static str           // Config→"operator", Chat→"guest"
```

`Default = Chat` mirrors `default_tier`'s remote=Chat philosophy: untrusted by default,
explicit elevation. A small `#[derive(Deserialize, Default)]` `ChannelPolicyConfig`
parses the flat keys `permission_level` / `default_workspace` from each channel
instance's config block (same flat-key pattern as `SlashAccessConfig`).

### 2. Inbound executor stamps tier + workspace (`inbound_router/executor.rs`)

```rust
let channel_cfg = self.channel_configs.get(channel_id).cloned().unwrap_or_default();
metadata.insert("caller_role", channel_cfg.caller_role_str());     // → TurnContext gate
workspace_override = channel_cfg.resolved_default_workspace();      // Layer-1 lock
```

`unwrap_or_default()` ⇒ unconfigured channels default to **Chat** (the hardening).
`resolved_default_workspace()` only returns `Some` for an absolute, existing dir
(else warn + `None` → agent default). Replaces the hardcoded `workspace_override: None`
and the stale "channel-routed messages have no project context" comment.

### 3. Workspace authorization gate (`agent.rs::start_run`)

Before resolving `project_root` → `workspace_override`:

```rust
if params.project_root.is_some() && !caller_is_operator(current_caller_role()) {
    return Err("choosing a working directory requires config-tier authorization …");
}
```

`None` role (trusted local/internal) and `"operator"` pass; `"guest"` (Chat tier) is
rejected. Closes the remote-Panel escape. Mirrors `TurnContext::caller_is_operator`
semantics exactly.

### 4. Boot wiring (`commands/start/builder/subsystems.rs`)

In the non-iMessage channel loop, also parse `permission_level` / `default_workspace`
and register a `ChannelConfig` when slash_access is non-empty **OR** permission/workspace
is non-default. Channels that configure nothing new stay unregistered (byte-identical
DM/group allow-all), and the executor still defaults them to Chat. The iMessage
`From<&IMessageConfig>` impl gains the two new fields (defaults).

## Behavior change (intentional, documented)

External channels lose their implicit **operator** trust and default to **Chat tier**.
This is a security hardening aligned with the explicit goal (Layer-1 default) and
Aleph's safe-by-default tier philosophy. It is **not** a hard lockout: a Chat-tier
channel asking for a config tool routes to live operator approval when a
`config_approval_requester` is wired (Phase 2b). An operator opts a channel up with
`permission_level = "config"` in that channel's config block.

## Tests

- `types.rs`: `ChannelPermissionLevel` default Chat; `caller_role_str` mapping;
  `ChannelPolicyConfig` deserializes flat `permission_level` / `default_workspace`,
  ignores unrelated channel keys; `ChannelConfig::default()` is Chat.
- `agent.rs`: `start_run` with `caller_role="guest"` + `project_root` → `Err`;
  `"operator"` / `None` + `project_root` → `Ok`.
- Existing `tools/scoped` guest-gating tests already cover the config-tool path; the
  executor change routes external channels into that same path.

## Entropy reduction

- Remove the hardcoded `workspace_override: None` + stale comment in the executor.
- No new parallel permission concept; everything routes through the one existing
  `caller_role` → `caller_is_operator()` chokepoint.
