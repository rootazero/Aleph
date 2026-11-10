# Fix review — IF5

| ID | Verdict | Comment |
|---|---|---|
| IF5-01 | FIXED | `key.verify_raw(&id, &signature).map_err(|e| e.to_string())` now performs the BIP340 Schnorr verification before allowlisting and conversion, and malformed signatures are rejected by `Signature::try_from`. |
| IF5-05 | FIXED | `PublishCommand` carries a oneshot result, `pending` correlates relay `OK` responses by event ID, and `publish_event` returns write failure, relay rejection, interruption, or a bounded 30-second timeout to the caller. |
| IF5-06 | FIXED | `GatewayContext` now holds `api: Arc<QQApiClient>`, and `identify_payload(&ctx.api)` calls `api.get_access_token().await` on every Identify rather than reusing a token snapshot. |
| IF5-17 | FIXED | The shutdown path now breaks to the unified exit and `debouncer.drain().await` flushes every remaining entry, with failed forwarding logged rather than silently discarded. |
| IF5-N1 | FIXED | The former unconditional verification stub is replaced by the actual `key.verify_raw(&id, &signature)` call. |
| IF5-N2 | FIXED | The receiver-slot implementation was replaced by `fetch_lock: Mutex<()>`, and all waiters recheck `cache` while holding that lock through fetch and cache update. |
| IF5-N3 | FIXED | `GatewayContext` now stores the shared `Arc<QQApiClient>` and no longer contains a permanent `token: String`. |
| IF5-N4 | FIXED | The `events_api` branch still calls `acknowledge_envelope`, while shutdown and all loop exits now reach the final `debouncer.drain().await`. |

## New problems

- `src/gateway/interfaces/nostr/message_ops/ops.rs:140` — pending waiters are only removed when their oneshot receiver is closed or a relay `OK` arrives; on reconnect, `drop(pending)` closes the sender and the caller receives “delivery unknown,” which is appropriately ambiguous but means every write is still tied to one connection attempt.
- `src/gateway/interfaces/slack/message_ops/socket.rs:238` — `drain()` is called only after the outer reconnect loop ends, so a forwarding failure during drain is logged but the task still exits; this is consistent with the stated non-durable shutdown boundary, but accepted messages may still not reach Core when the inbound channel is closed.

## Batch verdict

Needs rework — the four requested findings and their named new problems are addressed, but the Nostr publish implementation lacks an explicit bounded-wait expiry for pending relay acknowledgements, and the review instruction requires strict proof that the documented failure modes are fully removed.