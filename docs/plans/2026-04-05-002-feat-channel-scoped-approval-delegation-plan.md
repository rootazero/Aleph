---
title: feat: Add Channel-Scoped Approval Delegation
type: feat
status: active
date: 2026-04-05
origin: docs/brainstorms/2026-04-05-channel-scoped-approval-delegation-requirements.md
---

# Channel-Scoped Approval Delegation

## Overview

Implement per-channel approval delegation in Aleph, enabling each messaging channel (Telegram, Discord, etc.) to deliver approval requests to users and check sender authorization before showing approve/deny controls. Builds on existing `ApprovalGate`, `TrustStage`, and `ExecApprovalManager`.

## Problem Frame

OpenClaw implements channel-scoped approval delivery via `ChannelApprovalCapability` — each channel plugin declares how to deliver approval UI, render it, and authorize actors. Aleph has the approval core (`exec/approval/`) but lacks channel-level integration:

1. `Channel` trait has no approval capability method
2. No per-channel delivery exists (Telegram inline keyboard, Discord components, etc.)
3. No channel-scoped authorization check (can sender approve?)

## Requirements Trace

- R1. Each `Channel` implementation can optionally expose an approval capability
- R2. Approval requests route to the correct channel based on session context
- R3. Channel checks sender authorization before showing approve controls
- R4. Approval resolution routes back through the originating channel
- R5. Channels without approval capability fall back to existing behavior

## Scope Boundaries

- **Not in scope:** Native OS approval integration (future work)
- **Not in scope:** Changing the existing approval core (`ApprovalGate`, `TrustStage`)
- **Not in scope:** Multi-channel broadcast approval (single channel per session)

## Context & Research

### Relevant Code and Patterns

- `src/gateway/channel.rs` — `Channel` trait (line 575), already has `async_trait`
- `src/exec/approval/types.rs` — `ApprovalRequest`, `TrustStage` enum
- `src/gateway/handlers/exec_approvals.rs` — existing `exec.approval.request/resolve` handlers
- `src/gateway/interfaces/telegram/mod.rs` — Telegram channel implementation
- `src/gateway/interfaces/telegram/delivery.rs` — Telegram `send_message` with inline keyboard

### OpenClaw Reference Patterns

- `channels/plugins/approvals.ts` — `resolveChannelApprovalCapability()` merges legacy + new capability
- `infra/exec-approval-channel-runtime.ts` — Per-channel `ExecApprovalChannelRuntime` manages pending lifecycle
- `infra/channel-approval-auth.ts` — `resolveApprovalCommandAuthorization()` checks actor permissions

### Institutional Learnings

- Aleph uses Tower-style middleware layers — approval routing should integrate with middleware chain
- `ChannelState` already has health tracking — reuse pattern for approval state
- DashMap used in `RequestStateRegistry` for concurrent access — apply same pattern

## Key Technical Decisions

- **Decision**: Add `approval_capability()` to `Channel` trait returning `Option<Arc<dyn ChannelApprovalCapability>>`
  - Rationale: Non-breaking addition, `None` means channel doesn't support approval delivery
  - Follows existing pattern of `Option<>` returns for optional capabilities

- **Decision**: `ChannelApprovalCapability` is a `dyn` trait object, not a concrete struct
  - Rationale: Each channel implements differently; avoids monomorphization bloat

- **Decision**: Approval delivery uses existing `OutboundMessage` with inline keyboard attachment
  - Rationale: Reuses existing Telegram/Discord send infrastructure, no new primitives

- **Decision**: Authorization check at channel level, not approval manager level
  - Rationale: Channel knows its own auth rules (e.g., paired user in DMs vs. group restrictions)

## Open Questions

### Resolved During Planning

- **Q: How does approval routing work when a session has multiple channels?**
  - A: Each `PendingApproval` stores its originating `channel_id`. Resolution routes back via that channel.

- **Q: What happens if a channel doesn't implement approval capability?**
  - A: Returns `None`; `ApprovalGate` falls back to default behavior (auto-deny or config-based)

## High-Level Technical Design

```
┌─────────────────────────────────────────────────────────────┐
│                    Channel trait                            │
│  + approval_capability() -> Option<ChannelApprovalCapability>│
└─────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌───────────────┐   ┌───────────────┐   ┌───────────────┐
│ Telegram      │   │ Discord      │   │ Slack        │
│ Channel       │   │ Channel      │   │ Channel      │
│ + delivers    │   │ + delivers   │   │ + delivers   │
│   approval    │   │   approval   │   │   approval   │
│ + authorizes  │   │ + authorizes │   │ + authorizes │
└───────────────┘   └───────────────┘   └───────────────┘

┌─────────────────────────────────────────────────────────────┐
│              ChannelApprovalCapability trait                 │
│  + deliver_approval(req) -> PendingApproval               │
│  + authorize_actor(actor, action) -> AuthorizationResult  │
│  + render_approval(req) -> RenderedApproval              │
└─────────────────────────────────────────────────────────────┘
```

## Implementation Units

- [ ] **Unit 1: Define `ChannelApprovalCapability` trait**

**Goal:** Create the capability interface that each channel implements for approval delivery and authorization

**Requirements:** R1

**Dependencies:** None

**Files:**
- Create: `src/gateway/channel_approval.rs`

**Approach:**
Define `ChannelApprovalCapability` as a trait with three core methods:
1. `deliver_approval(req: &ApprovalRequest) -> Result<PendingApproval, ChannelError>` — sends approval UI to user
2. `authorize_actor(actor: &Actor, action: ApprovalAction) -> AuthorizationResult` — checks if actor can approve
3. `render_approval(req: &ApprovalRequest) -> RenderedApproval` — generates the approval UI payload

**Patterns to follow:**
- `src/gateway/channel.rs` — trait definition style with `async_trait`
- `src/exec/approval/types.rs` — `ApprovalRequest` already defined, extend or reuse

**Test scenarios:**
- Happy path: `TelegramChannelApprovalCapability::authorize_actor()` returns authorized for paired user
- Edge case: `None` capability returns `None` from `Channel::approval_capability()`
- Error path: `deliver_approval()` returns error when Telegram API fails

**Verification:**
- `cargo check -p alephcore` compiles
- Unit tests pass for all three trait methods

---

- [ ] **Unit 2: Extend `Channel` trait with `approval_capability()`**

**Goal:** Add approval capability to the Channel interface with default `None`

**Requirements:** R1, R5

**Dependencies:** Unit 1

**Files:**
- Modify: `src/gateway/channel.rs`

**Approach:**
Add to `Channel` trait (around line 604):
```rust
/// Get approval capability (None if channel doesn't support approval delivery)
fn approval_capability(&self) -> Option<Arc<dyn ChannelApprovalCapability>> {
    None
}
```

**Patterns to follow:**
- Existing `capabilities()` method pattern (line 602)
- `Option<Arc<...>>` return type used elsewhere in Aleph

**Test scenarios:**
- Happy path: Channels with capability return `Some(...)`
- Edge case: Default implementation returns `None`
- Integration: `ChannelRegistry` can query channel approval capability

**Verification:**
- `cargo check -p alephcore` compiles
- All existing channel implementations compile (shouldn't break)

---

- [ ] **Unit 3: Implement `TelegramChannelApprovalCapability`**

**Goal:** Implement approval delivery for Telegram using inline keyboard

**Requirements:** R2, R3, R4

**Dependencies:** Unit 1, Unit 2

**Files:**
- Create: `src/gateway/interfaces/telegram/approval.rs`
- Modify: `src/gateway/interfaces/telegram/mod.rs` — implement `approval_capability()`

**Approach:**
1. Create `TelegramChannelApprovalCapability` struct holding `Arc<TelegramChannel>`
2. `deliver_approval()` — sends message with inline keyboard (Approve/Deny buttons)
3. `authorize_actor()` — checks if sender matches paired user ID
4. Store `pending_approval_id` in message metadata for resolution routing

**Patterns to follow:**
- `src/gateway/interfaces/telegram/delivery.rs` — `send_message` with `InlineKeyboard`
- `src/gateway/handlers/telegram/pairing.rs` — pairing auth pattern

**Test scenarios:**
- Happy path: Approval message sent with correct inline keyboard
- Edge case: Non-paired user in DM → `authorize_actor()` returns denied
- Edge case: Group chat → `authorize_actor()` returns denied (DMs only for now)
- Error path: Telegram API rate limit handled gracefully

**Verification:**
- Tests pass for `TelegramChannelApprovalCapability`
- Integration test with mock Telegram API

---

- [ ] **Unit 4: Wire approval routing through `ApprovalBridge`**

**Goal:** Route approval requests through channel's delivery mechanism

**Requirements:** R2, R4

**Dependencies:** Unit 2, Unit 3

**Files:**
- Modify: `src/exec/approval/bridge.rs` — add channel routing
- Modify: `src/gateway/handlers/exec_approvals.rs` — add `channel_id` to approval request

**Approach:**
1. `ApprovalBridge::request_approval()` accepts optional `channel_id`
2. If `channel_id` provided, lookup channel and call `approval_capability()`
3. If capability exists, use it; otherwise fall back to existing behavior
4. Store `channel_id` in `PendingApproval` for resolution routing

**Patterns to follow:**
- `src/exec/approval/types.rs` — `PendingApproval` struct
- `src/gateway/channel_registry.rs` — channel lookup pattern

**Test scenarios:**
- Happy path: Approval request routes to Telegram channel and delivers
- Edge case: Unknown channel_id → fallback to default behavior
- Edge case: Channel with `approval_capability() == None` → fallback

**Verification:**
- `cargo test -p alephcore approval` passes
- Integration test: request approval via Telegram handler

---

- [ ] **Unit 5: Add `authorize_actor` check in approval UI rendering**

**Goal:** Only show approve/deny controls if sender is authorized

**Requirements:** R3

**Dependencies:** Unit 3

**Files:**
- Modify: `src/gateway/interfaces/telegram/approval.rs` — integrate authorization

**Approach:**
Before rendering and delivering the approval:
1. Call `authorize_actor()` with the sender's identity
2. If `authorized == false`, either skip delivery or send "Not authorized" message
3. Log authorization decision for audit trail

**Patterns to follow:**
- OpenClaw `channel-approval-auth.ts` — `resolveApprovalCommandAuthorization()`
- Aleph `src/security/audit.rs` — audit logging pattern

**Test scenarios:**
- Happy path: Paired user sees approval controls
- Edge case: Non-paired user receives "Not authorized" message
- Error path: Authorization check fails → don't expose approve button

**Verification:**
- `cargo test -p alephcore approval` authorization tests pass
- Manual test: non-paired user cannot see approve button

## System-Wide Impact

- **Interaction graph:** New `approval_capability()` call in `Channel::health()` chain; `ApprovalBridge` now queries `ChannelRegistry`
- **Error propagation:** Channel approval errors fall back gracefully to default behavior
- **State lifecycle risks:** `PendingApproval` now has optional `channel_id` — cleanup must handle both channel-backed and legacy approvals
- **API surface parity:** `exec.approval.request` gains optional `channel_id` param — backward compatible
- **Integration coverage:** End-to-end test: Telegram user requests approval → sees inline keyboard → clicks approve → command executes

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Channel implementations need updates | Work incrementally: Telegram first, others follow same pattern |
| Authorization bypass if channel doesn't implement capability | Default `None` = no approval delivery = fall back to deny |
| Approval timeout routing | `PendingApproval` stores `channel_id` for resolution; if channel gone, timeout handled by `ApprovalBridge` |

## Documentation / Operational Notes

- Update `docs/reference/GATEWAY.md` to document channel approval capability
- Add inline docs to `Channel::approval_capability()` explaining fallback behavior

## Sources & References

- **OpenClaw reference:** `channels/plugins/approvals.ts`, `infra/exec-approval-channel-runtime.ts`, `infra/channel-approval-auth.ts`
- **Aleph approval core:** `src/exec/approval/types.rs`, `src/exec/approval/bridge.rs`
- **Telegram delivery:** `src/gateway/interfaces/telegram/delivery.rs`
