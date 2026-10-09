# Task 7 H-pre runtime-mount QA report

Date: 2026-10-07

## Scope and boundary

This report covers only the approved H-pre Task 7 QA/docs work. The fixture uses the real `aleph-server` binary, real gateway RPC, real `RealAgentLoop`, the public MCP configuration/catalogue surface, and the deterministic mock provider request log as the model-visible effect oracle.

The fixture does not add a diagnostic/admin route and does not claim unsupported controls. Ownership invalidation, registry-cursor inspection, ProjectionHost close/hold inspection, post-close mutation, and slow-consumer overflow controls remain `UNVERIFIED`, not `PASS` or `SKIP`.

## Exact build and tree

- Repository: `/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up`
- QA HEAD before guarded build: `ea8d567ca69b724c2506921185d78aed7ca0ebd0`
- QA HEAD after guarded build: `ea8d567ca69b724c2506921185d78aed7ca0ebd0`
- Guard command: `.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py build --bin aleph-server`
- Guard result: `CARGO_EXIT: 0`
- Memory gate: `PASS`, `page_bytes=16384`, `available_KiB=7280992`, `threshold_KiB=4194304`
- Binary: `/Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up/target/debug/aleph-server`
- Binary SHA-256: `27db0fe1703e4f76fc03e8d38845043d6e71302110575f70b6cb8e4068ae59fa`
- Binary guard: `Fresh artifact (mtime=1791401683 build_started=1791402359)`
- Runtime mode: `Mode: Real AgentLoop (config provider)`

## Focused QA command and result

Command:

```text
cd /Volumes/TBU4/Workspace/Aleph-capability-phase4-follow-up
KEEP=1 GATEWAY_PORT=18847 MOCK_PORT=18848 ./qa/capability_hpre/run.sh
```

Overall result: `exit=3` (truthful non-green result because required unsupported controls are unverified).

Scenario results from the run:

| Scenario | Exit | Assertions | Effect receipts | Verdict |
|---|---:|---:|---:|---|
| `initial` | 0 | 11 | 6 | PASS |
| `replacement` | 3 | 19 | 11 | UNVERIFIED: changed consumer effect is proven; registry-cursor receipt is unavailable |
| `ownership` | 3 | 1 | 0 | UNVERIFIED |
| `close` | 3 | 1 | 0 | UNVERIFIED |
| `overflow` | 3 | 1 | 0 | UNVERIFIED |

Positive evidence:

- `initial`: catalogue delivery, nonempty `chat.send` receipt, marker-attributed provider request, nonempty provider body, and the mounted MCP tool in the actual provider `tools` payload all passed; assertion floor `>=8` and receipt floor `>=3` passed.
- `replacement`: old tool removal, new tool delivery, changed consumer identity, nonempty `chat.send` receipt, marker-attributed provider request, nonempty provider body, and the new MCP tool in the actual provider `tools` payload all passed; assertion floor `>=8` and receipt floor `>=3` passed.

## Limitations recorded as UNVERIFIED

- `ownership`: no legitimate public `OwnershipTree` bump/revoke/dispose control; no public observation of `Invalidated` plus replacement snapshot; no public registry cursor needed to prove the cursor is unchanged.
- `close`: no legitimate public `ProjectionHost` hold/inspect/close control; no public observation of delivery completion before teardown; no public post-close mutation test.
- `overflow`: no legitimate public slow-consumer hold/control; no public way to fill one per-consumer queue and observe the actual 64-event replacement; no public comparison against the independent 256-slot registry notification ring.
- `replacement`: the public catalogue/provider surfaces prove changed effect identity but do not expose the registry cursor/generation receipt.
- External provider and loopback ports were available in this run; no prerequisite was skipped.

## Authorized files

- `qa/capability_hpre/run.sh`
- `qa/capability_hpre/drive.py`
- `docs/reference/FEATURE_LOCATOR.md`
- `.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/task-7-report.md`

Syntax and executable checks passed for both QA scripts. No Rust, diagnostics, H/I, harness, dependency, or unapproved diagnostic-route files were changed. The approved pre-existing untracked plan/prompt files were not staged.
