# Task #14 slice: encrypted bounded public history

Commit: `feat(telecom): encrypt bounded public history`

## What changed
- `src/gateway/telecom/public_history.rs` (new): `PublicHistoryStore` (SQLite table `telecom_public_history`, separate DB, caller-supplied dedicated `SecretsCrypto` key), `PublicHistorySession` (SHA-256 of region+binding_ref from typed `TelecomProvenance`; channel and connection_generation excluded; redacted Debug), `PublicHistoryTurn`, `PublicHistoryConfig` (frozen ceilings: 24h TTL, 32 turns, 64KiB plaintext/session, 8MiB ciphertext total; `ttl==0` disables), `install_global`/`installed`.
- Payload (version, session digest, created/expires ms, user, assistant) is AES-GCM encrypted and re-verified against the row columns on read.
- `src/gateway/telecom/public_run.rs`: `run_public_sms` gained `history: Option<&PublicHistoryStore>`. Load before run (error => runner not invoked, permit dropped, nothing sent, `Failed`); append only for Ok, non-empty, non-cancelled reply, while the permit is still held; append errors are logged and only mean "not remembered".
- `src/orchestrator/dispatch.rs`: `PublicRunRequest.history: Vec<PublicHistoryTurn>`.
- `src/orchestrator/harness_bridge/public_run.rs`: seeds history turns into the ephemeral in-memory session; reply extraction restricted to the current turn id (no stale history reply). Spend-agent's `metered_public_provider` wrap preserved.
- `src/gateway/inbound_router/executor.rs`: passes `public_history::installed()` (None in production today).
- `src/gateway/telecom/mod.rs` re-exports; `src/orchestrator/tests/dispatch.rs` and `.../harness_bridge/public_spend_tests.rs`: `history: Vec::new()` compatibility.

## Tests run (all `-p alephcore --lib`, `--test-threads=1`)
- `public_history`: 12 passed (round trip via API; raw file has no plaintext/binding/region; TTL boundary + physical delete; zero TTL; config ceilings; 32-turn; 64KiB + oversize refusal; total-cap refuse without evicting others/rollback; binding isolation; wrong key + tamper => Corrupt + purge; row relabel/metadata tamper; missing key).
- Earlier combined run (`public_` filter) showed all `telecom::public_run` (incl. history: second turn sees first, binding isolation, no store = unchanged, failed/cancelled not stored, unreadable history fails closed with runner not invoked), `harness_bridge::public_run` (history order, no stale reply), `public_spend_tests`, `public_admission`, dispatch tests passing; the only failure in that run was the tamper test, since fixed (SQLite `||` turned the blob into TEXT) and re-run green.
- `cargo check -p alephcore --lib --tests`: passed before the final test-only edit (warnings pre-existing/unused-import only).
- Not run: clippy, full workspace test, the full lib suite after the final tamper-test edit.

## Residuals / NOT done
- No production key provisioning or installer: `install_global` is never called, so production behaviour is still history-free single-turn. Needs a dedicated key (not login/shared token) + db path + config knob.
- Provenance has no peer address: history is per binding_ref+region; if a binding serves multiple callers they share history.
- Key rotation, SQLite WAL/backup physical erasure, and ciphertext-length leakage are not addressed.
- Append failure (e.g. capacity) is silent beyond a warn log.
- No concurrent-process locking beyond SQLite + the per-binding permit.
- Task #14 remains open: A/B trusted approval/effect fence, complete tool boundary audit, dedup/pair-loop verification, PSTN, release gates, private read/disclosure approvals.
