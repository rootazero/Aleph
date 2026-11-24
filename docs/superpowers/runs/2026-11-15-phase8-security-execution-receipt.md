# Phase8 / NSH Implementation Execution Receipt
# Phase8 / NSH 实施执行回执

**Date / 日期:** 2026-11-15
**Approval / 批准:** user approval `m01055`
**Scope / 范围:** bounded Phase8 F1–F5 evidence slices and the NSH v0.5 macOS first slice.

## Passed / 已通过

- `cargo check -p alephcore --lib --offline --locked --jobs 1` from the repository root passed in 5m45s using isolated target `/tmp/aleph-phase8-main-lib-20261115-2210`.
- `cargo check --bin aleph-server --offline --locked --jobs 1` passed in 4.93s with the same isolated target.
- Focused Phase8 cases passed, each 1/1:
  - `foreign_scope_denial_records_policy_violation_without_handler_effect`
  - `stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`
  - `ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`
  - `projection_lag_requires_snapshot_before_reconnect_resolve`
  - `disable_closes_owned_surface_and_rejects_old_lease`
  - `captured_terminal_rejects_provider_replacement_before_invoke`
- `cargo check -p aleph-nsh-shell --offline --locked --jobs 1` passed.
- `cargo test -p aleph-nsh-shell --offline --locked --jobs 1` passed: 27 shell tests and 1 pinned v0.5 Window/WindowRef compatibility test, 28 total, 0 failed.
- `python3 -m py_compile qa/nsh_panel_parity.py` and JSON parsing of `qa/nsh_panel_parity.schema.json` passed.
- The parity reducer smoke checks passed for both valid/accepted (exit 0) and valid-but-threshold-rejected (exit 3) receipts. The rejected case retained `status: "VALID"`, `threshold_status: "NOT_ACCEPTED"`, and `invalid_reason: null`.
- `rustfmt --edition 2021 --check` on changed NSH files and `git diff --check` passed.
- `panel:tabs:test` passed 6/6 against the rebuilt current-HEAD artifact at `/tmp/aleph-panel-tabs-current-20261115`.
- `panel:tabs:artifacts:test` passed 12/12 against the rebuilt current-HEAD artifact.
- `panel:tabs:protocol:test` passed. It is a protocol fixture and explicitly does not exercise a live Gateway.

每一次 Cargo 命令之前都运行了 `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`；通过的命令使用了独立 `CARGO_TARGET_DIR`、`--offline --locked --jobs 1`。

## Recorded limits / 已记录限制

- The full `cargo test -p alephcore --lib --offline --locked --jobs 1` run did not produce a verdict within the 600-second tool limit. Its log contained nine existing extension/cache tests reporting “has been running for over 60 seconds”, with no `FAILED`, compiler error, panic or final `test result` summary. The process was terminated without treating this as pass or failure. The six approved Phase8 cases were then run individually and all passed.
- The NSH check/test proves compilation, projection behavior and the pinned v0.5 compatibility case. It does not prove native window opening, a live canonical Gateway Chat/Artifacts exchange, reconnect against a running server, or the Tauri/NSH dual-run performance gate.
- The first WebChat artifact check rejected the stale `interfaces/webchat/.panel-tabs` output because it was built from old HEAD `8c07dca9f5ccaa33a64ec8105c82c60e84faca49`, while current HEAD is `e08af3789dbf194f02f08461c215eb3c7701557f`; one provenance source entry was also missing. The artifact was then rebuilt from current HEAD with task-local `wasm-bindgen-cli 0.2.130`, and both build/provenance suites passed. The rebuilt artifact remains a temporary `/tmp` QA output.
- `bun check` at the repository root failed immediately with `error: Script not found "check"`; the root package has no such script. The approved package-specific commands are the available WebChat panel scripts and package checks.
- Cargo’s generated lock graph is retained because NSH v0.5 cannot be minimally spliced into the old lock: the minimal merge produced real `webpki-roots` 1.0.9/1.0.12 and `toml` 0.8.2/0.8.23 resolver conflicts. Current `Cargo.lock` has 1,596 package entries versus 1,537 at HEAD, with 328 added package IDs and 269 removed package IDs; the generated graph changed 280 unrelated package version sets and contains 7 pinned NSH Git entries. This is a material dependency review risk and should receive a separate lock-minimization pass before release.
- The current NSH shell still has one non-test-build dead-code warning for `SessionManager::create_session` and `SessionManager::session_id`; they remain as explicit disconnected/no-local-ID test seams.

## Not executed / 未执行

Tauri/WebChat removal, detached daemon lifecycle ownership, silent R2 or constitutional changes, all-platform NSH completion, release-profile performance, Safe Replay, exactly-once effects, universal scheduling, publication, push, and release were not executed. The external Gateway/daemon was not claimed as NSH-owned.
