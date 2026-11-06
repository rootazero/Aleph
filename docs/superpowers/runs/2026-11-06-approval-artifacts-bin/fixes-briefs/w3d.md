# W3D brief — native QA gateway + CI fixes (wave 3)

Contract: `../BRIEF_FIX.md` (same directory). Worktree: /home/zou/data/workspace/Aleph-wt-fix-wave3d (branch fix-wave3d).
Report: `/home/zou/data/workspace/Aleph-wt-fix-wave3d/.review/rd-2026-11-06-approval-artifacts-bin/fixes/w3d.md` (uncommitted, one section per ID).

## W3D-DC-01 [P1] `stopped` message now reports `fixtureClosed: false`, breaking the documented contract and the QA harness
- location: src/bin/aleph-panel-native-qa-gateway/main.rs:822-823 (`fixture_closed: false`, was `true` at base commit)
- kind: api-break
- finding: the helper's `stopped` message flipped `fixtureClosed` from `true` to `false`, but that flag is a published protocol fact: the plan that defines the contract requires `stopped(listenerClosed:true, fixtureClosed:true)` (docs/superpowers/plans/2026-11-02-panel-native-runtime-completion-implementation.md:143) and the harness consumes it as a hard assertion (qa/native_ipc/native_project_gateway_smoke...).
- suggested-fix: release the fixture for real before emitting `stopped` (revoke the fixture's device tokens / drop the QA root) and keep `fixture_closed: true`; if the fixture genuinely cannot be released, keep the protocol contract intact and change only the internal flag the fix needed.

## W3D-DC-02 [P2] Shutdown ignores the server join result and always reports `listenerClosed: true`
- location: src/bin/aleph-panel-native-qa-gateway/main.rs:806-812 (and the EOF path at 838-841)
- kind: swallowed-error
- finding: the base propagated the shutdown outcome (`server.await??`); the fix replaced it with `tokio::time::timeout(5s, server).await.is_err()` and discards the result, so a timed-out or failed shutdown is invisible while the helper then asserts the unverified fact `listener_closed: true`.
- suggested-fix: capture the outcome (`Ok(Ok(()))` → closed) and set `listener_closed` from it (`false` on timeout or join error); keep the 5 s deadline so the helper can still exit, and report the failure instead of asserting it away.

## W3D-DC-03 [P2] 16 KiB line bound is checked after the whole line has been buffered
- location: src/bin/aleph-panel-native-qa-gateway/main.rs:432-456
- kind: partial-fix
- finding: the stdin reader is `BufReader::new(tokio::io::stdin()).lines()`; `next_line()` has no size cap, so the entire line is read into memory before `line.len() > 16384` rejects it. The rejection itself is correct and stream-safe, but the unbounded allocation the bound was meant to remove still happens.
- suggested-fix: read the line with an explicit bound (e.g. `read_until(b'\n', &mut buf)` into a capped buffer) and error out as soon as the cap is exceeded, so an oversized line is never fully materialised.

## W3D-DC-04 [P3] `release_response` ack uses a status outside the declared ack vocabulary
- location: src/bin/aleph-panel-native-qa-gateway/main.rs:676-690 (status emitted at 682 and 689)
- kind: api-break
- finding: the release branch now reports `"released_before_park"` for the held-but-never-parked and idle/no-op cases, but the ack validator allows only `isOneOf(["ok"])(message.status)` (qa/native_ipc/native_project_protocol.mjs:83). The drift is latent today only because `isAck` is not called by the harness.
- suggested-fix: keep `status: "ok"` and carry the distinction in a separate field, OR add `"released_before_park"` to the protocol's ack vocabulary (qa/native_ipc/native_project_protocol.mjs:83) and to every consumer that switches on the status. Prefer the option that keeps the published vocabulary intact.

## W3D-DC-05 [P3] `revoke_exact` masks every non-ambiguity error as `already_revoked`
- location: src/bin/aleph-panel-native-qa-gateway/main.rs:726-731 (catch-all at 730)
- kind: swallowed-error
- finding: `resolve_device` can fail with `unknown_device` (no binding), `device_not_found` or `ambiguous_device`; the fix routes every remaining error to `already_revoked`, losing the distinction.
- suggested-fix: map `unknown_device` to `not_found` (already in the vocabulary, protocol.mjs:91), keep `ambiguous_device` → `bad_state`, and route anything else to `internal` instead of `already_revoked`.

## W3D-DC-06 [P3] New CI step re-checks the whole crate on all three OS runners while clippy still skips the gated bin
- location: .github/workflows/aleph-core-ci.yml:422-423 (lint job matrix at 351-356, clippy step at 392-393)
- kind: over-reach
- finding: the new step `cargo check -p alephcore --bin aleph-panel-native-qa-gateway --features native-qa -j1` has real value (the `native-qa` feature gates code no other CI job compiles) but it runs in the lint matrix on Linux, macOS and Windows, tripling the cost, and the clippy step still does not compile the gated code.
- suggested-fix: scope the compile check to Linux (`if: runner.os == 'Linux'`) like its sibling steps, and add `--features native-qa` to the clippy step so the gated code is linted too.

Scope: src/bin/aleph-panel-native-qa-gateway/main.rs, .github/workflows/aleph-core-ci.yml, and (only if W3D-DC-04 needs it) qa/native_ipc/native_project_protocol.mjs.
Read the current HEAD code before editing — wave-1 fixes are already merged.
