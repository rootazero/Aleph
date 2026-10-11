# Task 4 — Coalescer isolation fix (public SMS)

Base: 006f0e9d8. Scope: `src/gateway/inbound_router/mod.rs` only (+ this report).

## Finding
Inbound `run_loop` runs dedup -> pair_loop -> optional `MessageCoalescer` -> `handle_message`.
The coalescer is globally enabled and `merge()` keeps only the first fragment's
metadata/channel, so a typed `MessageMeta::Sms` could be merged with other text:
ordinary text leaking into a public run, SMS metadata dropped, `/stop` buried.

## Fix
New `InboundMessageRouter::admit_to_coalescer(msg) -> Vec<InboundMessage>`: if
`executor::telecom_public_from_message_meta(&msg.metadata).is_some()` the message
is returned individually and never buffered; otherwise `coalescer.push(msg)` as before.
The coalescing run-loop push path calls it. Since typed SMS never enters a buffer,
the tick path and the shutdown `flush_all` path can never emit one (no change needed
there). No channel-string / text inference. Dedup and pair-loop untouched (memory-only).
Non-coalescing path unchanged.

## Tests (inbound_router::tests)
- `typed_public_sms_bypasses_coalescer_individually`: two SMS + one ordinary
  same conversation/sender -> SMS come back one each with own id/text/channel/typed meta,
  `/stop` text intact; only the ordinary message is buffered; tick yields it alone with
  no SMS meta; `flush_all` empty.
- `ordinary_messages_still_coalesce`: non-SMS fragments still merge ("hello\nworld").
- `run_loop_routes_through_coalescer_bypass`: source guard — run_loop has no raw `coalescer.push(`.

## Verification (OPENSSL_NO_VENDOR=1 CARGO_BUILD_JOBS=2)
- `cargo test -p alephcore --lib inbound_router`: 83 passed, 0 failed.
- `cargo check -p alephcore --lib --tests`: OK.
- `rustfmt --edition 2021` on mod.rs only.

## Not done
- Full Task #14 is NOT complete; no full run-loop (broadcast-driven) integration test
  (tests exercise `admit_to_coalescer` + source guard).
- `/stop` recognizability is asserted at the coalescer boundary (text unmerged), not
  through the downstream public-run stop handling.
- Full-lib test suite not run.
