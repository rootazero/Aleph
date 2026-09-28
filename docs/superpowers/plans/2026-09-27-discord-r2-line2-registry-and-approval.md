# Discord R2 — Line 2: CommandRegistry Wiring (D1) + Approval Capability (D4)

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the legacy raw `cb_<id>` text-channel fallback in `handle_component` with the typed `CommandRegistry::dispatch` from R1 (D1), and then actually wire Discord approval buttons to the approval sink instead of the agent text loop (D4).

**Architecture:** Behaviour-preserving rewrite of one serenity handler function, then a follow-up that replaces a Panel UI button-emitter. D1 first (it's D4's prerequisite); D4 second.

**Tech Stack:** Rust, tokio, serenity, Panel Leptos

**Spec:** `docs/superpowers/specs/2026-09-27-discord-r2-backlog.md` (D1 + D4 sections)

**Hard rule from R1 lesson:** every task ends with `cargo test --lib <new module>` AND one integration-level test that exercises the new wiring end-to-end (not just unit tests on the new struct).

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/gateway/interfaces/discord/mod.rs::handle_component` | Modify (D1) | Parse `custom_id` to `ComponentId`, call `CommandRegistry::dispatch`, route outcome |
| `src/gateway/interfaces/discord/commands.rs` | Modify (D1) | Register `ComponentKind::Callback` → fallback text channel handler in `with_defaults` |
| `src/gateway/interfaces/discord/approval_card.rs` | Modify (D4) | Emit `ComponentId{kind: ApprovalApprove, payload: <message_id>}` buttons instead of raw `cb_<id>` |
| `src/gateway/handlers/approval.rs` | Verify (D4) | Receiver side — already accepts `ForwardToApproval` (no change) |

---

## Task 1: D1 — Replace raw `cb_<id>` path with `CommandRegistry::dispatch`

**Why:** R1 line2 built `ComponentId` codec (4 variants: ApprovalApprove / ApprovalDeny / Callback / Pagination) + `CommandRegistry` with `dispatch` returning `DispatchOutcome`. But `handle_component` still uses the legacy path: `self.inbound_tx.send(InboundMessage { text: custom_id.clone() })` — passing the whole custom_id string to the agent as user text. This means approval buttons are broken in the typed sense (they go through the agent loop, not the approval sink).

**Files:**
- Modify: `src/gateway/interfaces/discord/mod.rs::handle_component` (currently ~30 lines)
- Verify: `src/gateway/interfaces/discord/commands.rs::with_defaults` already includes `Callback` fallback (R1 fixup added it)

**Interface contract:**
```rust
// From R1 line2
pub struct ComponentId { pub kind: ComponentKind, pub payload: String }
pub enum ComponentKind { ApprovalApprove, ApprovalDeny, Callback, Pagination }
pub enum DispatchOutcome { ForwardToApproval, AckNoReply, Execute(String), Reply(String) }
impl CommandRegistry {
    pub fn with_defaults() -> Self;  // registers Approval{Approve,Deny,Callback}
    pub fn dispatch(&self, id: &ComponentId) -> Option<DispatchOutcome>;
}
```

- [ ] **Step 1:** Read `src/gateway/interfaces/discord/mod.rs::handle_component` — capture the exact current logic.
- [ ] **Step 2:** Write a **failing** integration test in `tests/discord_handle_component_test.rs`:
  ```rust
  #[tokio::test]
  async fn approval_button_dispatches_to_approval_sink_not_text_loop() {
      // Construct a fake serenity ComponentInteraction with custom_id
      // "approval-approve:abc123". Construct DiscordChannel with a mock
      // approval sink receiver. Call handle_component. Assert:
      //   - approval sink receives one message with payload "abc123"
      //   - inbound_tx receives ZERO messages
  }
  ```
- [ ] **Step 3:** Run `cargo test --test discord_handle_component_test` — expected FAIL (current code routes to `inbound_tx`).
- [ ] **Step 4:** Rewrite `handle_component`:
  ```rust
  async fn handle_component(&self, interaction: &ComponentInteraction) -> Result<()> {
      let id = match ComponentId::parse(&interaction.data.custom_id) {
          Some(id) => id,
          None => {
              // Malformed custom_id — preserve legacy text fallback
              self.inbound_tx.send(InboundMessage { text: interaction.data.custom_id.clone() }).await?;
              return Ok(());
          }
      };
      match self.registry.dispatch(&id) {
          Some(DispatchOutcome::ForwardToApproval) => {
              self.approval_sink.send(ApprovalRequest { message_id: id.payload.clone(), actor: ... }).await?;
          }
          Some(DispatchOutcome::AckNoReply) => { /* acknowledge to serenity */ }
          Some(DispatchOutcome::Execute(cmd)) => { self.inbound_tx.send(InboundMessage { text: cmd }).await?; }
          Some(DispatchOutcome::Reply(text)) => { self.send(OutboundMessage { text }).await?; }
          None => { /* registered as Callback kind with fallback to inbound */ }
      }
      Ok(())
  }
  ```
- [ ] **Step 5:** Run new integration test — expected PASS.
- [ ] **Step 6:** Run `cargo test --lib gateway::interfaces::discord::commands` — expected PASS (84+ tests).
- [ ] **Step 7:** Run `cargo test --lib gateway::interfaces::discord` — expected ALL GREEN.
- [ ] **Step 8:** Commit: `discord(r2-line2): D1 replace raw cb_<id> with CommandRegistry::dispatch (handle_component rewrite)`

---

## Task 2: D4 — Approval card emits typed ComponentId buttons

**Why:** With D1 in place, the Panel UI's approval card can emit `ComponentId{kind: ApprovalApprove, payload: <message_id>}` instead of `cb_<message_id>` strings. Clicked buttons now route via `dispatch` → `ForwardToApproval` → approval sink (was: text → agent loop).

**Files:**
- Modify: `src/gateway/interfaces/discord/approval_card.rs` (Panel-side button emitter)
- Modify (likely): shared protocol type for the button payload

**Interface contract:**
```rust
// From R1 line2 commands.rs
ComponentId { kind: ApprovalApprove, payload: String }  // payload = approval message_id
ComponentId { kind: ApprovalDeny, payload: String }
```

- [ ] **Step 1:** Read `src/gateway/interfaces/discord/approval_card.rs` — find `format!("cb_{}", message_id)` or similar.
- [ ] **Step 2:** Replace with `ComponentId::encode(ComponentKind::ApprovalApprove, message_id)` / `ApprovalDeny`.
- [ ] **Step 3:** Verify with `cargo check -p alephcore` and `cargo check -p aleph-panel` (if Panel is a separate crate).
- [ ] **Step 4:** Add unit test in `approval_card.rs`:
  ```rust
  #[test]
  fn approval_card_emits_typed_component_ids_not_cb_prefix() {
      let buttons = ApprovalCard::new("msg-1").buttons();
      assert!(buttons[0].custom_id.starts_with("approval-approve:"));
      assert!(!buttons[0].custom_id.starts_with("cb_"));
  }
  ```
- [ ] **Step 5:** Commit: `discord(r2-line2): D4 approval card emits typed ComponentId buttons (no more cb_<id> raw text)`

---

## Verification (run before merge)

```bash
CARGO_BUILD_JOBS=1 cargo check -p alephcore 2>&1 | tail -3
CARGO_BUILD_JOBS=1 cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -3
CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib gateway::interfaces::discord 2>&1 | tail -10
CARGO_BUILD_JOBS=1 cargo test --test discord_handle_component_test 2>&1 | tail -10
```

Expected: `test result: ok` for both lib and integration. **Plus: an integration test that goes end-to-end through `handle_component` (D1 acceptance).**

## Merge

After both tasks green, worktree `discord-r2-line2` → `merge: discord R2 line 2 (D1 registry + D4 approval capability)`.

## Review Focus

1. **Behaviour regression in unknown-kind path** — old code passed `cb_<id>` to text; new code routes to `Callback` handler → `AckNoReply`. A bot integration test that hit unknown kinds before now silently swallows. Add a log line.
2. **Race between `approval_sink.send` and `interaction.create_response`** — serenity expects acknowledgement within 3s. If `approval_sink.send` blocks, the interaction times out. Add a `tokio::time::timeout` on the send.
3. **Custom_id parser** — `ComponentId::parse` must accept the legacy `cb_<id>` format with `None` (so D1's "malformed → legacy fallback" path actually works for old bots still emitting `cb_`).

These three failure modes are not covered by the unit tests in this plan.
