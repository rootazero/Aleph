# Findings — GX5 (src/gateway — security + middleware + surface + link + message_assembly + pipeline)

Files reviewed: 39 (9593 LOC)

Counts: P0= 0 P1= 1 P2= 2 P3= 0

### GX5-01 [P1] Tokio blocking I/O in async context

- **Location**: `src/gateway/link/manager.rs:39-41` (`scan_link_configs`)
- **Failure mode**: `tokio::fs::read_dir().await` followed by synchronous `std::env::var()` (line 103) inside `expand_env_vars` → blocks the runtime thread while waiting on environment variable lookup; under high concurrency this can stall other tasks
- **Evidence**: 
  ```rust
  while let Some(entry) = entries.next_entry().await // line 43
  match tokio::fs::read_to_string(&path).await {    // line 57
      Ok(content) => match crate::yaml::from_str::<LinkConfig>(&content) {
          Ok(config) => {
              configs.push(config);  // line 61 — config contains settings
  ```
  then in `expand_env_vars` (line 97):
  ```rust
  match std::env::var(var_name) {  // line 103 — synchronous syscall
  ```
- **Suggested fix**: Replace `std::env::var()` at line 103 with `tokio::task::spawn_blocking(move || std::env::var(var_name))` or cache environment reads at startup. Preserves: env-var expansion semantics, error handling (missing var stays as `${env.VAR}`), and the scan's async contract.
- **Confidence**: 0.85

### GX5-02 [P2] Unbounded memory growth via environment variable expansion

- **Location**: `src/gateway/link/manager.rs:97-143` (`expand_env_vars`)
- **Failure mode**: Attacker sets `ALEPH_MALICIOUS` to 10MB string, then submits link config `{"token": "${env.ALEPH_MALICIOUS}"}` → `expand_env_vars` reads full 10MB (line 104-117), checks length AFTER allocation (line 105), keeps unexpanded reference but memory spike already happened; repeated across multiple link configs exhausts memory
- **Evidence**:
  ```rust
  match std::env::var(var_name) {
      Ok(val) => {
          if val.len() > MAX_EXPANDED_ENV_LEN {  // line 105 — check is AFTER alloc
              warn!(...);
          } else {
              return serde_json::Value::String(val); // line 117
  ```
- **Suggested fix**: Check environment variable length BEFORE reading its full value (not possible with `std::env::var`); instead document `MAX_EXPANDED_ENV_LEN` as a policy and rely on operator control of environment (link configs are operator-written YAML, not user input). Or: read env vars through a guarded accessor that enforces the bound at read time. Preserves: the expansion contract and the documented limit.
- **Confidence**: 0.70

### GX5-03 [P2] TOCTOU in `DeviceTokenManager::exchange_bootstrap_ticket`

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