# Phase9 Candidate Implementation Plan: Runtime Proof & Dependency Closure
# Phase9 候选实施计划：运行时证据与依赖闭合

**Status / 状态:** plan only; no Phase9 production implementation in this session
**Review gate / 审查闸门:** independent SPEC review, then independent QUALITY review, then next-session human approval

## Step 1 — Freeze the candidate boundary / 固定候选边界

**Create:** `docs/superpowers/runs/2026-11-15-phase9-entry-inventory.md` and the candidate specification.
**Evidence:** record that no official Phase9 charter exists; carry forward the Phase8 limits; name the candidate label and approval gate.
**Terminal:** `PASS` when the candidate scope is explicit; `BLOCKED` if the next session rejects the name or scope.

## Step 2 — Close the full-test result / 闭合全量测试结果

**Create/modify candidates:**

- `qa/phase9_test_closure.py`
- `docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md`
- existing `src/extension/claude_cache_tests.rs` and `src/extension/context_adapter.rs` only if source evidence identifies a concrete lifecycle defect

**Frozen inputs and commands:**

- Full-suite command from repository root: `CARGO_TARGET_DIR=.phase9-target cargo test -p alephcore --lib --offline --locked --jobs 1`, preceded by `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`. The harness timeout is 600 seconds.
- Create `qa/phase9_test_closure.py`. It runs each filter with `cwd` set to the repository root, `CARGO_TARGET_DIR=.phase9-target`, `RUST_BACKTRACE=1`, inherited remaining environment, `--offline --locked --jobs 1` and `--test-threads=1`; it writes stdout/stderr under `docs/superpowers/runs/phase9-test-output/` and removes only its own temporary fixture directory after each case. The nine complete invocations are:
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::an_unreadable_index_forgets_nothing'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::the_model_may_disable_but_not_enable_a_claude_cache_row'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::the_rpc_face_enables_and_disables_a_claude_cache_row'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::claude_cache_tests::two_broken_claude_code_plugins_at_one_version_are_two_error_rows'`
  - `python3 qa/phase9_test_closure.py --repo-root . --target-dir .phase9-target --receipt docs/superpowers/runs/2026-11-15-phase9-test-closure-receipt.md --timeout-seconds 600 --test-filter 'extension::context_adapter::tests::lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement'`

The closure harness records each exact child argv, cwd, environment delta, fixture setup/cleanup, timeout, stdout/stderr paths and contents, process exit, and classification. A full-suite timeout remains `UNRESOLVED_TIMEOUT` until every long-running test has a terminal classification. A completed assertion/panic or source-proven lifecycle/teardown violation is `FAIL`; `BLOCKED` requires evidence that a required environment, dependency or external service is unavailable; an unexplained wait remains `UNRESOLVED_TIMEOUT` and continues to block closeout; a completed predicate is `PASS`. `BLOCKED` 必须有环境、依赖或外部服务不可用的证据；原因未证明的等待不能降格为环境阻塞。 The six Phase8 focused tests are rerun with the same harness as positive controls and are listed separately in the receipt. No skip list is allowed.

## Step 3 — Close dependency strategy / 闭合依赖策略

**Create:** `docs/superpowers/runs/2026-11-15-phase9-cargo-lock-decision.md`.
**Modify only if approved by evidence:** `Cargo.lock`, workspace manifests or NSH manifest.

**Frozen baseline and candidate artifacts:**

- Frozen original: `git show e08af3789dbf194f02f08461c215eb3c7701557f:Cargo.lock`, SHA-256 `bb1b647bb8f53785467c2dd47b40ed0bd46f2514c9de4e6e94bc29fccc900345`.
- Retained Phase8 graph: copy the retained lock to `docs/superpowers/runs/2026-11-15-phase9-retained-Cargo.lock` before any Phase9 candidate mutation; expected SHA-256 `d6f2e99dd2df5868b15f2940d74cf8845ea7c26d3ff69819ce42422180bb1cdb`.
- Candidate: the session’s `Cargo.lock` after a single candidate generation decision; record its SHA-256 in `docs/superpowers/runs/2026-11-15-phase9-cargo-lock-decision.md`.
- Machine-readable output: `docs/superpowers/runs/2026-11-15-phase9-cargo-lock-delta.json` with top-level `frozen_to_retained` and `retained_to_candidate`, each containing complete `added`, `removed`, and `changed` package identity arrays. `changed` entries contain old/new version, source and checksum values.

**Method and exact commands:**

- Create the delta helper `qa/cargo_lock_delta.py`, which uses Python `tomllib` and accepts `--baseline-rev`, `--retained-lock`, `--candidate-lock` and `--output`. Run from the repository root: `python3 qa/cargo_lock_delta.py --baseline-rev e08af3789dbf194f02f08461c215eb3c7701557f --retained-lock docs/superpowers/runs/2026-11-15-phase9-retained-Cargo.lock --candidate-lock Cargo.lock --output docs/superpowers/runs/2026-11-15-phase9-cargo-lock-delta.json`.
- Run `cargo metadata --locked --offline --no-deps --format-version 1` with output captured in the decision receipt, then inspect the seven pinned NSH entries.
- Test any minimized candidate in a separate target with target-package compile/test commands. Preserve the retained full graph as rollback baseline. No hand-edited lock splice is accepted after the Phase8 resolver conflicts.

**Acceptance:** one resolver-valid policy is documented with reproducible `--locked` checks and review of unrelated version churn. If minimization is unsafe, retain the generated graph and classify the risk explicitly.

## Step 4 — Execute live NSH QA in the next session / 下一会话执行 NSH live QA

**Create candidates:**

- `qa/nsh_panel_launcher.py`
- `qa/nsh_panel_runtime.schema.json`
- `docs/superpowers/runs/2026-11-15-phase9-nsh-runtime-receipt.md`
- QA-only marker seams under `desktop/shell/src/main.rs` and `desktop/nsh-shell/src/main.rs` immediately before `app.run`/`run_app`, only under the debug QA feature

**Build and launch contract:**

1. From the repository root, run `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`, then use a fresh `.phase9-target` for the debug-only build: `CARGO_TARGET_DIR=.phase9-target cargo build -p aleph-desktop-shell -p aleph-nsh-shell --no-default-features --features aleph-desktop-shell/qa-owned-root --offline --locked --jobs 1`.
2. Run the launcher from the repository root with this complete argv: `python3 qa/nsh_panel_launcher.py --repo-root . --target-dir .phase9-target --fixture-root qa/fixtures/nsh-panel-parity --receipt docs/superpowers/runs/2026-11-15-phase9-nsh-runtime-receipt.json`. The launcher derives both executable paths from `--repo-root` and `--target-dir`; it creates its own receipt and process roots, so no shell variables or repository absolute paths are part of the contract.
3. The launcher creates an OS-temp direct child named `aleph-shell-qa-*`, writes a 32-byte lowercase-hex nonce to the Tauri `<owned_root>/.qa-owned` followed by `\n`, and rejects any child outside that root. The Tauri child is launched with the argv array `[repo_root / ".phase9-target/debug/aleph-desktop-shell", "--qa-root", str(tauri_owned_root), "--qa-nonce", tauri_nonce]` and cwd `repo_root / "desktop/shell"`.
4. The NSH child is launched with the argv array `[repo_root / ".phase9-target/debug/aleph-nsh-shell"]` and cwd `repo_root / "desktop/nsh-shell"`. The first NSH slice has no `--qa-root`/`--qa-nonce` contract; the launcher records its pid, cwd, fixture id, stdout/stderr and exit/cleanup separately. This is consistent with the NSH parity plan and does not create an NSH ownership tree or daemon ownership.
5. The launcher uses Python `subprocess` argv arrays with `shell=False`. Candidate QA-only marker seams under `desktop/shell/src/main.rs` and `desktop/nsh-shell/src/main.rs` are added only after the next approval gate.
6. The runtime schema validates lifecycle events (`run_id`, `phase`, `event`, monotonic timestamp, pid, owned root where applicable, nonce where applicable, session key where applicable). The launcher emits a value-only metrics file from these events; `qa/nsh_panel_parity.py` consumes that metrics file and writes the final parity receipt. The runtime schema is therefore an event/ownership input contract, while the existing reducer remains the threshold contract.

**Required barriers and observations:**

- `ready`: both shell processes have started and emitted their QA marker; record pid/root/nonce.
- `hello`: the client receives `system.hello`, then `events.subscribe` and `sessions.list` complete.
- `accepted`: the Chat `/` request returns/observes `RunAccepted`; record `run_id` and `session_key`.
- `streamed` and `complete`: observe at least one `ResponseChunk`, then `RunComplete` or `RunError`; compare the session key on every event.
- `artifact-refreshed`: inject/observe matching `session.artifact`, wait for the corresponding `artifacts.list` response, and reject wrong-session/malformed topics.
- `reconnected`: close the client transport, wait for a new connection epoch and fresh snapshot/history, then inject an old-epoch event and prove it is ignored.
- `closed`: request shell shutdown, record both child exit codes, confirm no remaining Tauri child under its owned root and no remaining NSH launcher-supervised child. A missing exit/cleanup observation is `FAIL` for the lifecycle contract, not `BLOCKED`.

**Acceptance:** fixed schema and marker protocol produce a terminal receipt; live session/artifact state is canonical; reconnect rejects stale state; Tauri owned-root and NSH launcher-supervised children settle; no detached daemon ownership is inferred.

## Step 5 — Performance and release boundary / 性能与 release 边界

Run the approved debug QA protocol only after Steps 2–4 have terminal evidence. Keep Tauri-first/NSH-second order, ten warmups, thirty measured runs, p95 rank 29, 100ms process sampling and existing thresholds. A valid measurement that misses a threshold must be recorded with `valid: true` and `threshold_status: NOT_ACCEPTED`; it must not be converted to `invalid` or `BLOCKED`. Record `PASS`, valid-but-`NOT_ACCEPTED` or `BLOCKED` for the debug gate. Release-profile performance is a deliberate `RECORDED_LIMIT` in this candidate plan and remains a separate future gate; no Phase9 closeout may claim release acceptance.

## Step 6 — Closeout and approval / 结项与批准

Write a Phase9 execution receipt and closeout only after the evidence is complete. The next session must approve this candidate plan before any production source, dependency, launcher or QA marker change. Phase9 implementation is deliberately excluded from the current session.

## Validation discipline / 验证纪律

Before every Cargo invocation:

```sh
python3 qa/browser_live/check_memory.py --required-bytes 4294967296
```

Use a fresh task-specific `CARGO_TARGET_DIR`, `--offline --locked --jobs 1`, and record natural exit status. Run source exploration through Graphify before raw source reads. Keep one Cargo owner and one source writer.

## Rollback / 回滚

If any live launcher, marker or dependency change fails, remove only the candidate change in its isolated worktree/commit and preserve the Phase8 receipt, current full Cargo.lock and current-HEAD WebChat artifact evidence. Do not reset, pull, delete historical evidence or remove Tauri components as a recovery action.
