# Fix brief — GX5

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/GX5.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/GX5.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/GX5.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-GX5.md

## FIX THESE (1)

### GX5-03 [P3] TOCTOU in `DeviceTokenManager::exchange_bootstrap_ticket`
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/security/device_token_manager.rs:142-146` (`exchange_bootstrap_ticket`)
- **Failure mode**: Two concurrent calls with same `device_id` both check `get_device` (line 142) → both see no conflict → first call consumes ticket and writes device row → second call's `consume_bootstrap_ticket` (line 149) fails with `Invalid` (ticket already consumed) but the conflict check passed, so the returned `DeviceIdConflict` error never fires and the second caller gets a confusing "invalid ticket" error instead of "device id conflict"
- **Evidence**:
  ```rust
  if let Some(existing) = self.store.get_device(&device_id)? {  // line 142
      if existing.device_type.as_deref() != Some(PANEL_DEVICE_TYPE) {
          return Err(DeviceTokenError::DeviceIdConflict(device_id));
      }
  }
  let consumed: ConsumedBootstrapTicket = self
      .store
      .consume_bootstrap_ticket(ticket, Some(&device_id))?;  // line 149
  ```
- **Suggested fix**: Move the conflict check inside the transaction that consumes the ticket, or make `consume_bootstrap_ticket` return `DeviceIdConflict` when the device already exists with wrong type. Smallest fix: document that the check is advisory (race is rare, ticket is single-use so failure is safe). Preserves: the conflict prevention for sequential calls; sacrifices: clear error message on race.
- **Confidence**: 0.75

## Clean files

- `src/gateway/link/types.rs` — no finding
- `src/gateway/message_assembly/assembler.rs` — no finding
- `src/gateway/message_assembly/mod.rs` — no finding
- `src/gateway/message_assembly/tests.rs` — no finding
- `src/gateway/middleware/auth.rs` — no finding
- `src/gateway/middleware/chain.rs` — no finding
- `src/gateway/middleware/handler_service.rs` — no finding
- `src/gateway/middleware/latency.rs` — no finding
- `src/gateway/middleware/metrics.rs` — no finding
- `src/gateway/middleware/mod.rs` — no finding
- `src/gateway/middleware/rate_limit.rs` — no finding
- `src/gateway/middleware/redact_query.rs` — no finding
- `src/gateway/middleware/request_state.rs` — no finding
- `src/gateway/middleware/trace.rs` — no finding
- `src/gateway/middleware/traits.rs` — no finding
- `src/gateway/middleware/validate.rs` — no finding
- `src/gateway/pipeline/mod.rs` — no finding
- `src/gateway/security/artifact_caps.rs` — no finding
- `src/gateway/security/canvas_caps.rs` — no finding
- `src/gateway/security/crypto.rs` — no finding
- `src/gateway/security/mod.rs` — no finding
- `src/gateway/security/shared_token.rs` — no finding
- `src/gateway/security/store/bootstrap_tickets.rs` — no finding
- `src/gateway/security/store/devices.rs` — no finding
- `src/gateway/security/store/identity.rs` — no finding
- `src/gateway/security/store/mod.rs` — no finding
- `src/gateway/security/store/slot.rs` — no finding
- `src/gateway/security/store/tests.rs` — no finding
- `src/gateway/security/store/tokens.rs` — no finding
- `src/gateway/security/store/types.rs` — no finding
- `src/gateway/security/store/users.rs` — no finding
- `src/gateway/security/token_readonly.rs` — no finding
- `src/gateway/surface/delivery.rs` — no finding
- `src/gateway/surface/desktop.rs` — no finding
- `src/gateway/surface/mod.rs` — no finding
- `src/gateway/surface/r5_router.rs` — no finding
- `src/gateway/link/mod.rs` — no finding


## DO NOT FIX (2) — refuted / deferred by adjudication or routing

- GX5-01: ### GX5-01 [P1] Tokio blocking I/O in async context
- GX5-02: ### GX5-02 [P2] Unbounded memory growth via environment variable expansion
