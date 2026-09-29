# Phase 3 Deferred Items Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the three Phase-3 items the user flagged: iMessage R8 coverage in `channel_manage.rs`, wizard onboarding updates, and agent/tool wiring for iMessage-native methods (polls.create / group.setIcon). R8 (tools = everything) is the binding redline — any channel operation that has an implementation must be reachable through an `AlephTool` without bypass.

**Architecture:**
- Task 5 (iMessage R8): minimal — widen the default channel-id resolution in `channel_manage.rs` so an unspecified `channel_id` falls back to an iMessage channel as well as Telegram.
- Task 6 (wizard): deferred — `src/wizard/flows/onboarding.rs` is the current main-tree copy; `archive/wizard/flows/onboarding.rs` is a stale duplicate. Without an explicit "what to update" from the user (model list? new steps? new channel-pairing stage?) the work is unbounded; surface as a follow-up with a concrete question.
- Task 7 (agent wiring): design-only this round — write the trait extension, the iMessage implementation stub, and the new `ChannelMessageAction` variants as `Unsupported` paths. A working iMessage poll + group-icon implementation is its own ticket (BlueBubbles API surface to verify, attachment upload for icons, scope confirmers).

**Tech Stack:** Existing Rust + existing `Channel` trait (`src/gateway/channel.rs:818`) + existing `IMessageChannel` (`src/gateway/interfaces/imessage/mod.rs`).

**Spec:** `docs/superpowers/specs/` (none — bounded cleanup). Plan carries the design.

---

## Global Constraints

- **MSRV 1.95** unchanged.
- **No new `pub` API surface beyond R8 minimum**: each new tool action must have a concrete reachable implementation, not a no-op stub (CLAUDE.md P6 + 判据 #11: a "report-success no-op" is the failure mode this rule exists to prevent).
- **Channel trait additions** use `async fn` with `ChannelResult<T>` and a `UnsupportedFeature` variant for channels that can't fulfill them — never panic.
- **Per-channel capability discovery** stays in the `Channel` trait's existing capability flags; the new methods just dispatch on those.
- **No iMessage BlueBubbles API calls in tests** — the BlueBubbles client requires a real server. Tests use a stub channel or rely on the `Unsupported` path.

## Review Focus

| # | Input class / failure mode | Expected behavior | Owning task |
|---|----------------------------|-------------------|-------------|
| P1 | `channel_manage.rs` called with no `channel_id` and only an iMessage channel configured | Falls back to that iMessage channel (was: error "no Telegram channel") | Task 5 |
| P2 | `channel_message.rs` CreatePoll/SetGroupIcon called on a non-iMessage channel | Returns `ChannelError::UnsupportedFeature`; never panics | Task 7 |
| P3 | `channel_manage.rs::resolve_channel_id` given two iMessage channels configured | Returns the first one alphabetically (deterministic) | Task 5 |
| P4 | Wizard "update" request lands before Task 6 has a concrete diff | Documented as blocked on user-scope input | Task 6 |

---

## File Structure

| File | Responsibility (this plan) |
|------|-----------------------------|
| `src/builtin_tools/channel_manage.rs` | Widen default channel-id resolution: iMessage as fallback after Telegram |
| `src/gateway/channel.rs` | Add `create_poll` and `set_group_icon` to `Channel` trait with default `UnsupportedFeature` impls |
| `src/gateway/interfaces/imessage/mod.rs` | Override `create_poll` / `set_group_icon` calling BlueBubbles (`src/gateway/interfaces/imessage/bluebubbles/api.rs`); error out clearly where BlueBubbles lacks the method |
| `src/gateway/interfaces/telegram/mod.rs` (and others) | No-op: accept default Unsupported impls |
| `src/builtin_tools/channel_message.rs` | Add `CreatePoll` / `SetGroupIcon` variants to `ChannelMessageAction` and route them to the new trait methods |
| `docs/superpowers/specs/2026-09-29-wizard-onboarding-scope.md` | Open question document for Task 6 |

---

## Task 5: Widen `channel_manage.rs::resolve_channel_id` to include iMessage

**Files:**
- Modify: `src/builtin_tools/channel_manage.rs:80-100` (`resolve_channel_id`)

**Interfaces:**
- Consumes: existing `ChannelRegistry::list_by_type(&str)` (no change)

- [ ] **Step 1: Read the current resolve_channel_id logic** — confirm it only tries Telegram as the fallback.

- [ ] **Step 2: Add an iMessage fallback** — after the Telegram search, also try `list_by_type("imessage")`. If both are present, prefer Telegram (it has more pairing-specific semantics; iMessage is a new fallback for installs that don't run Telegram).

- [ ] **Step 3: Add a test** `channel_pairing_defaults_to_imessage_when_no_telegram_and_no_channel_id` that constructs a registry with only an iMessage channel and asserts `resolve_channel_id(None)` returns it.

- [ ] **Step 4: Run `cargo check -p alephcore --tests`** — Expected: PASS.

- [ ] **Step 5: Commit** `channel_manage: include iMessage in default channel-id resolution (R8 coverage)`

---

## Task 6: Wizard onboarding — DEFERRED (needs scope)

**Files:**
- None this round.

**Status:** Deferred. The user flagged "wizard onboarding 更新" without specifying what to update. The current state of the codebase:

- `src/wizard/flows/onboarding.rs` is the live copy, last touched 2026-09 (model list already at `claude-opus-4-8`, `gpt-5.5`, `gemini-2.5-pro`).
- `archive/wizard/flows/onboarding.rs` is a stale duplicate (model list still on `claude-3-5-haiku`, `gpt-4o`, `gemini-2.0-flash`); its presence is a `git mv` cleanup, not a feature request.
- The wizard RPC surface (`wizard.start` / `next` / `answer` / `cancel` / `status`) is wired at `bin/.../start/mod.rs:896-908` and `gateway/handlers/mod.rs:1165-1171`.

**Open question to surface to the user** (recorded in `docs/superpowers/specs/2026-09-29-wizard-onboarding-scope.md`):

> The wizard module is already in the main tree (`src/wizard/`); the `archive/wizard/` copy is stale. To act on "wizard onboarding update" we need scope. Which of the following is meant?

> (a) Refresh the model picker list (currently `claude-opus-4-8` / `gpt-5.5` / `gemini-2.5-pro` / `claude-haiku-4-5` / `gpt-5.4-mini` / `gemini-2.5-flash`) against the live `MODEL_CATALOG`.
> (b) Add a new wizard stage (e.g., a channel-pairing step that drives `channel_manage`).
> (c) Replace the onboarding flow with a new design.
> (d) Clean up the `archive/wizard/` directory (git rm) so it stops appearing in `git grep`.

This task is BLOCKED on user response. The plan file ends here; nothing to commit.

---

## Task 7: Agent/tool wiring for iMessage-native methods — design-only this round

**Files:**
- Modify: `src/gateway/channel.rs:818+` (trait additions)
- Modify: `src/gateway/interfaces/imessage/mod.rs:275+` (impls calling BlueBubbles)
- Modify: `src/builtin_tools/channel_message.rs:38+` (new `ChannelMessageAction` variants)

**Interfaces:**

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    // ... existing methods ...

    /// Create a poll in the given conversation. Default: `UnsupportedFeature`.
    async fn create_poll(
        &self,
        _conversation_id: &ConversationId,
        _question: &str,
        _options: &[String],
        _allow_multiple: bool,
    ) -> ChannelResult<MessageId> {
        Err(ChannelError::UnsupportedFeature(
            "polls not supported on this channel".into(),
        ))
    }

    /// Set the icon of a group conversation. Default: `UnsupportedFeature`.
    async fn set_group_icon(
        &self,
        _conversation_id: &ConversationId,
        _icon_data_url: &str,
    ) -> ChannelResult<()> {
        Err(ChannelError::UnsupportedFeature(
            "group icon not supported on this channel".into(),
        ))
    }
}
```

- Consumes: existing `Channel::send` and `OutboundMessage` patterns
- Produces: two new actions `CreatePoll` and `SetGroupIcon` in `ChannelMessageAction`, dispatching to the trait methods

- [ ] **Step 1: Add `create_poll` / `set_group_icon` to the `Channel` trait** with default `Unsupported` impls. Per CLAUDE.md P4 + R8: default impls make this backwards-compatible — every existing channel keeps compiling.

- [ ] **Step 2: Implement in `IMessageChannel`** — forward to BlueBubbles API if available (`src/gateway/interfaces/imessage/bluebubbles/api.rs`). If BlueBubbles doesn't have `create_poll` (verify — see Review Focus P2), return `UnsupportedFeature` with a clear message; do NOT call HTTP.

- [ ] **Step 3: Add `CreatePoll` and `SetGroupIcon` variants to `ChannelMessageAction`** in `channel_message.rs`. Match arm routes to `channel.create_poll` / `channel.set_group_icon`. Args:
  - `CreatePollArgs { channel_id, conversation_id, question, options, allow_multiple }`
  - `SetGroupIconArgs { channel_id, conversation_id, icon_data_url }`

- [ ] **Step 4: Update `AlephTool::Output`** — return `MessageId` (poll) or unit (icon).

- [ ] **Step 5: Update `executor/builtin_registry/registry/tool_registry_impl.rs`** if the new variants need explicit schema descriptions beyond what `schemars` derives — likely just verify the existing DESCRIPTION enumeration covers the new actions.

- [ ] **Step 6: Write tests** — two unit tests on a stub channel: `create_poll_on_non_imessage_returns_unsupported` and `set_group_icon_on_non_imessage_returns_unsupported`. These prove the gating actually fires (CLAUDE.md 判据 #2: "in what situation does this turn red?"). A real iMessage implementation test would need a BlueBubbles mock; out of scope for this round.

- [ ] **Step 7: Run `cargo check -p alephcore --tests`** — Expected: PASS.

- [ ] **Step 8: Commit** `channel_message: wire iMessage-native poll + group-icon actions (R8)`

---

## Self-Review

1. **Spec coverage:**
   - Item 7 (channel_manage R8) — Task 5 ✓
   - Item 8 (wizard onboarding) — Task 6 deferred; explicit user-question artifact produced.
   - Item 9 (agent wiring polls/groupIcon) — Task 7 design + default-Unsupported impls; full iMessage implementation deferred to a follow-up ticket because BlueBubbles API surface needs verification.

2. **Step scan:** every step either names a specific file:line + change, names a test, or names a verification command. No "handle errors" or "TBD."

3. **Type consistency:** the new trait methods return `ChannelResult<T>` matching the trait's existing error type. The new `ChannelMessageAction` variants reuse the existing `channel_id` / `conversation_id` argument shape.

4. **Review Focus:** P1 covered by Task 5 test; P2 covered by Task 7 tests; P3 covered by Task 5 deterministic ordering; P4 is the open question.

5. **Proportion:** 7 tasks total — 1 ship (Task 5), 1 deferred (Task 6), 1 design (Task 7). Plan length ok.

---

## Follow-ups (not in this plan)

- F1: Resolve Task 6 wizard scope — single concrete question to user.
- F2: Implement full iMessage `create_poll` + `set_group_icon` against BlueBubbles API. Verify the upstream API actually supports them; if not, surface as a "feature unavailable" via doctor.
- F3: Per CLAUDE.md 判据 #16, when Task 7 ships, audit the other channels (Telegram, Discord, WhatsApp) for poll/group-icon support — some may already have it under different method names.
- F4: Clean up `archive/wizard/` after Task 6 ships, so the directory stops confusing `git grep`.