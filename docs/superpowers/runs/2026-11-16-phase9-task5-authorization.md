# Phase9 Next-Session Authorization / Phase9 下一会话实施授权

**Date / 日期:** 2026-11-16
**User direction / 用户指示:** User message `m02541`: asks whether Phase9 can proceed; requests a comprehensive restart prompt with references, local integration to `main`, and restart from `main` to implement.
**Authorization / 授权:** The next session is authorized to begin the Phase9 candidate implementation described by `docs/superpowers/prompts/2026-11-16-phase9-main-restart.md` and the reviewed plan. Do not ask the user to repeat approval for this same scope.

## Authorized scope / 授权范围

- Classify and close the unresolved full `alephcore` lib test run with deterministic per-test evidence; fix concrete lifecycle defects only when source/test evidence supports them.
- Audit/minimize or explicitly retain the NSH v0.5 Cargo dependency graph, using the frozen pre-NSH lock baseline and machine-readable deltas.
- Implement owned debug QA launcher/schema/fixtures/marker instrumentation and exercise the macOS NSH first slice against a live canonical Chat `/` + Artifacts exchange, reconnect, and child shutdown/cleanup.
- Run the reviewed debug-only Tauri/NSH parity protocol and write evidence-based receipts/closeout.
- Use independent read-only SPEC then QUALITY review as scoped in the handoff.

This records the next-session authorization so that implementation can begin after restart; it does not mean any Phase9 implementation or runtime acceptance has already occurred.

## Exclusions / 排除范围

No Tauri/WebChat/workspace-member removal; no detached daemon lifecycle ownership; no silent R2 or constitutional change; no all-platform NSH completion; no release-profile performance acceptance; no Safe Replay, exactly-once external effects or universal scheduler; no publication, remote push or release. Phase4 parity plus two-week dual run remains a required gate before Tauri removal.

## Handoff / 交接

Start from local `main` and read `docs/superpowers/prompts/2026-11-16-phase9-main-restart.md`. Phase8 remains `PASS WITH RECORDED LIMITS`; the full test suite has an unresolved timeout and remains an explicit Phase9 objective.
