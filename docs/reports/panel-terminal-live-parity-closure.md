# Terminal Live Parity Closure

Date: 2026-10-28
Commit under test: `11513ec59` (`docs(panel): plan terminal live parity closure`)
Worktree: `panel-live-parity`

## Verdicts

```text
component: PASS
production_host_loopback: PASS
live_gateway: BLOCKED
```

`component` and `production_host_loopback` are carried by the existing deterministic Terminal evidence and the read-only baseline run at the same commit in the primary checkout:

- `node --test tests/panel-terminal-host.test.mjs` passed. The fixture reported the `/terminal` production host, staged versioned JavaScript/WASM assets, and the loopback WebSocket fixture.
- `node --test tests/panel-terminal-protocol.test.mjs` passed. The fixture covered versioned module reuse, exact PTY topics, sequence-gap attach recovery, UTF-8 input, resize geometry, PTY exit fallback, and connection-epoch recovery. It explicitly reported that a live Gateway was not exercised.
- The isolated worktree has no ignored `interfaces/webchat/.panel-tabs` build output, so rerunning those fixtures there was not possible without generating staged assets. No assets were generated because the live gate was already blocked and this run must remain read-only at that gate.

## Live gate disposition

```text
live_gateway: BLOCKED
reason: no authorized disposable Gateway/project/session/credential/cleanup scope
```

No live HTTP health/auth request, WebSocket connection, PTY operation, or Gateway process control was attempted. No user Gateway, project, session, PTY, or credential was accessed or modified.

The following operator-provided prerequisites are still required before live acceptance can begin:

- a disposable Aleph Gateway target and its endpoint or launch scope;
- an explicitly test-owned project;
- an explicitly test-owned session and permission to create a test-owned PTY;
- a credential supplied through the process environment or a controlled secret channel;
- explicit cleanup authority covering only the disposable Gateway and the test-owned project, session, and PTY.

No test-owned live resources were created, so live cleanup was not applicable and there is no cleanup residue to claim as live evidence.

## Checks and scope

- The required memory preflight returned `ALLOW` with the required 4 GiB headroom. No standalone cargo validation was planned for the blocked run; the repository pre-commit hook nevertheless ran `cargo fmt --check` and `cargo clippy --all-targets` during the documentation commit, and both passed.
- No production code, PTY server logic, harness code, Capability/ZahirFacade runtime, community lifecycle, or other Tab business logic was changed.
- Loopback evidence remains loopback evidence and is not promoted to live Gateway evidence.

## Explicitly not done

- A second Tab migration.
- Gateway Bound Stage C.
- Default release flip.
- Community lifecycle work.
- Full mobile, accessibility, or release hardening.
- Live Gateway health/auth, WebSocket, `pty.list`, `pty.spawn`, `pty.attach`, real screen events, UTF-8 input, resize, PTY exit/fallback, reconnect/epoch recovery, or live cleanup acceptance.
