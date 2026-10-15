# Task #14 — public_run hardening report

Scope: `src/gateway/telecom/public_run.rs` only (plus this report).

## Fixes
1. **Cancel race.** The binding permit is now held through reply delivery and the
   history append (dropped last). The `/stop` fence `cancel.is_cancelled()` is
   evaluated with no `await` between it and the `send_reply` call, so a
   cancellation before the send yields `Cancelled`, no reply, no history append.
   Because the permit is still registered during delivery, `/stop` keeps finding
   the run until the reply is released.
2. **Hung provider.** `harness.run_public_text` is wrapped in
   `tokio::time::timeout(PUBLIC_RUN_TIMEOUT = 120s)` (mirrors
   `DEFAULT_RUN_TIMEOUT_SECS` of subagent runs). On expiry: token cancelled,
   permit dropped, `Failed`, nothing sent, no spend recorded, no history.
   Testable via private `run_public_sms_bounded(.., run_timeout, delivery_timeout)`;
   the public `run_public_sms` signature is unchanged.
3. **Hung sink (consequence of holding the permit through delivery).**
   Delivery bounded by `PUBLIC_REPLY_DELIVERY_TIMEOUT = 30s` so a stuck channel
   cannot pin admission; the turn is still `Replied` (reply was released to the sink).
4. **History/spend preserved.** Append only after an uncancelled, non-empty reply
   was released to the sink; `record_spend` unchanged (still recorded for any
   `Ok` outcome, including one later fenced by cancel — pre-existing behavior).

## Tests added (public_run::tests)
- `cancel_landing_before_send_suppresses_reply_and_history` (runner cancels then returns a reply)
- `binding_stays_held_through_delivery` (blocking sink; admission 1 during send; `/stop` still finds run)
- `hung_delivery_is_bounded_and_releases_admission`
- `hung_provider_times_out_without_reply_history_or_leaked_admission`

## Verification
- `rustfmt --edition 2021 src/gateway/telecom/public_run.rs`
- `cargo test -p alephcore --lib public_run`: 23 passed, 0 failed
- `cargo check -p alephcore --lib`: ok (3 pre-existing warnings)
(all with `OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2 timeout -k 20 900 setsid`; first test build exceeded 900s on a cold target / lock contention and was rerun with a longer timeout)

## Not done / residual
- A `/stop` arriving during delivery (after the fence) cannot recall an already-issued send; the fence is best-effort at the send boundary.
- Spend is still recorded for a run cancelled after completing (provider cost was real).
- A timed-out runner that ignores the cancel token is dropped (cancelled by drop) on timeout; non-cooperative blocking work inside it is not addressed.
- Timeout values are constants, not config-driven.
- Real sink/channel integration (`ChannelPublicReplySink`) not exercised.
