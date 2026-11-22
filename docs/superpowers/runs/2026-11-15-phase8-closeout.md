# Phase8 Closeout / Phase8 结项

**Date / 日期:** 2026-11-15
**Verdict / 结论:** **PASS WITH RECORDED LIMITS**
**Approval basis / 批准依据:** user approval `m01055` and continued authorization `m01702`
**Scope / 范围:** Phase8 Integration & Security bounded F1–F5 slices, pinned NSH v0.5 macOS first-slice build/test evidence, current-HEAD WebChat artifact provenance, and the approved QA receipt contract.

## Decision / 决策

The approved bounded Phase8 slices pass their required focused evidence. The Phase8 closeout is **PASS WITH RECORDED LIMITS**, not an unrestricted full-workspace or release-readiness pass.

批准范围内的 Phase8 受控切片已通过要求的重点证据。Phase8 结论为 **PASS WITH RECORDED LIMITS**，不等同于全工作区、live runtime 或 release readiness 全部通过。

## Evidence that passed / 已通过证据

- Six focused Phase8 cases passed 1/1:
  - `foreign_scope_denial_records_policy_violation_without_handler_effect`
  - `stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`
  - `ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`
  - `projection_lag_requires_snapshot_before_reconnect_resolve`
  - `disable_closes_owned_surface_and_rejects_old_lease`
  - `captured_terminal_rejects_provider_replacement_before_invoke`
- `cargo check -p alephcore --lib --offline --locked --jobs 1` passed.
- `cargo check --bin aleph-server --offline --locked --jobs 1` passed.
- `cargo test -p aleph-nsh-shell --offline --locked --jobs 1` passed: 27 shell tests plus 1 v0.5 Window/WindowRef compatibility test.
- Current HEAD WebChat panel artifact was rebuilt at `/tmp/aleph-panel-tabs-current-20261115` using the task-local `wasm-bindgen-cli 0.2.130`. The build used the current HEAD and generated terminal entry `./tabs/terminal/03997cef66ced12f/terminal.js`.
- `panel:tabs:test` passed 6/6 against the rebuilt artifact.
- `panel:tabs:artifacts:test` passed 12/12 against the rebuilt artifact.
- `panel:tabs:protocol:test` passed.
- `qa/nsh_panel_parity.py` accepted/rejected receipt smoke tests passed; JSON schema parsing, Python compilation, rustfmt target checks and `git diff --check` passed.
- Cargo.lock audit completed in [phase8-cargo-lock-audit.md](./2026-11-15-phase8-cargo-lock-audit.md).

All Cargo commands were preceded by the 4GiB memory gate and used an isolated target with `--offline --locked --jobs 1`.

所有 Cargo 命令之前都通过了 4GiB memory gate，并使用独立 target、`--offline --locked --jobs 1`。

## Recorded limits / 记录限制

- The full `cargo test -p alephcore --lib --offline --locked --jobs 1` run reached nine existing extension/cache tests that emitted “has been running for over 60 seconds” and produced no `FAILED`, compiler error, panic or final `test result` before the 600-second execution limit. This is classified as an existing slow-test blocking condition, not as a proven test failure. The six approved Phase8 cases were run independently and passed.
- NSH v0.5 evidence proves pinned API compatibility, compilation, projection tests and the current shell test suite. It does not prove native window opening, a live Gateway Chat/Artifacts exchange, reconnect against a running server, Tauri/NSH dual-run, or release-profile performance.
- The rebuilt WebChat artifact is a temporary QA output under `/tmp`; it was not copied into a tracked project artifact directory.
- Cargo.lock retains the complete generated NSH dependency graph. Compared with HEAD it has 1,537→1,596 package entries, 328 added and 269 removed package identities, 285 changed package-name version/source sets under the exact comparison, and 7 pinned NSH Git entries. The minimal splice failed resolver validation on `webpki-roots` and `toml`; the broad graph is therefore retained with a material dependency-review risk.
- Tauri/WebChat removal, detached daemon ownership, silent R2/constitutional changes, all-platform NSH completion, Safe Replay, exactly-once effects, universal scheduling, publication, push and release remain out of scope.

## Follow-up / 后续

Phase8 bounded implementation may be treated as closed with the limits above. The full-suite slow-test investigation, broader lock minimization, live NSH runtime proof and release performance remain follow-up work. Phase9 is authorized in this session only through planning and independent SPEC/QUALITY review; Phase9 production implementation is deferred to the next session.
