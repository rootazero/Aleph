# Wave 3D fix report

## W3D-DC-01 [P1] `stopped` fixtureClosed contract
- verdict: fixed
- files: src/bin/aleph-panel-native-qa-gateway/main.rs:860, src/bin/aleph-panel-native-qa-gateway/main.rs:872
- change: Restored `fixtureClosed: true` on both shutdown response paths, preserving the published stopped-message contract. The gateway still reports the listener result independently.
- risk: The flag remains a protocol-level completion fact; fixture internals are not exposed through a second status vocabulary.

## W3D-DC-02 [P2] Shutdown join result
- verdict: fixed
- files: src/bin/aleph-panel-native-qa-gateway/main.rs:849-860, src/bin/aleph-panel-native-qa-gateway/main.rs:889-894
- change: Shutdown now derives `listenerClosed` only from `Ok(Ok(()))`; timeout and join errors report `false`. EOF cleanup also propagates timeout and join failures instead of discarding them.
- risk: A failed or timed-out EOF cleanup now returns an internal process error, which makes cleanup failure visible to the caller.

## W3D-DC-03 [P2] 16 KiB stdin bound
- verdict: fixed
- files: src/bin/aleph-panel-native-qa-gateway/main.rs:24, src/bin/aleph-panel-native-qa-gateway/main.rs:432-484
- change: Replaced unbounded `lines()` buffering with a `take(16385)` bounded read, rejects oversized input before parsing, and drains the remainder of an oversized line to preserve command framing.
- risk: The 16 KiB limit includes the newline when present, matching the existing byte-based guard; invalid UTF-8 continues to produce `bad_command`.

## W3D-DC-04 [P3] `release_response` ack vocabulary
- verdict: fixed
- files: src/bin/aleph-panel-native-qa-gateway/main.rs:700-715
- change: Kept the declared ack vocabulary intact by returning `status: "ok"` for held, parked, idle, and already-released states. The state transition behavior remains unchanged.
- risk: Consumers no longer receive the latent non-vocabulary distinction `released_before_park`; no protocol files required changes.

## W3D-DC-05 [P3] `revoke_exact` error masking
- verdict: fixed
- files: src/bin/aleph-panel-native-qa-gateway/main.rs:749-771
- change: Mapped `unknown_device` and `device_not_found` to `not_found`, retained `ambiguous_device` as `bad_state`, and mapped all other resolution errors to `internal`. Successful revocation and `already_revoked` behavior remain unchanged.
- risk: Previously hidden resolution failures now surface as `internal`, which may change diagnostics for callers that depended on the incorrect `already_revoked` result.

## W3D-DC-06 [P3] CI matrix scope
- verdict: fixed
- files: .github/workflows/aleph-core-ci.yml:393, .github/workflows/aleph-core-ci.yml:423-424
- change: Enabled the `native-qa` feature for the clippy command and restricted the native gateway compile check to the Linux lint runner, where the existing gated compile checks are scoped.
- risk: Native QA compile coverage remains Linux-only in this job; macOS and Windows do not run this gated check.

Validation: `git diff --check` passed. Per BRIEF_FIX.md, cargo, clippy, rustfmt checks, tests, and builds were not run.
