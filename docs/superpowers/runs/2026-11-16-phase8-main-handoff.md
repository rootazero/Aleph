# Phase8 Local Main Handoff / Phase8 本地 main 交接

**Date / 日期:** 2026-11-16
**User direction / 用户指示:** prepare a complete next-session Phase9 prompt, integrate this task locally into `main`, and restart from `main` to implement. The user selected local integration; no push/release was requested. 用户已选择本地集成，并授权下一会话在所列范围实施 Phase9。

## Integration shape / 集成形状

At entry, there was one worktree already on local `main` at `e08af3789dbf194f02f08461c215eb3c7701557f`; all task changes were uncommitted. There was no separate existing task branch to merge. The task is captured in a short retained branch `phase9-handoff`, then fast-forwarded into local `main`. Locate the final integration commit without a circular embedded hash:

```sh
git log -1 --format='%H %s' -- docs/superpowers/prompts/2026-11-16-phase9-main-restart.md
git merge-base --is-ancestor phase9-handoff main
```

This preserves both branch names, the original main ancestor and all previous evidence. No pull/reset/cleanup or protected configuration access is part of integration. No remote push occurs.

## Start here / 重启入口

Use `../prompts/2026-11-16-phase9-main-restart.md`. Next-session implementation authorization is recorded in `2026-11-16-phase9-task5-authorization.md`; it records the user's new instruction and exclusions and supersedes earlier “next-session approval required” wording for the same bounded scope. The prompt records the reading order, NSH master-plan obligation, current code/data-path inventory, frozen dependency baselines, execution order, evidence gates and exclusions.

Phase9 may begin investigation and implementation. Phase9 runtime, whole-suite, dependency and performance acceptance remain subject to actual evidence. Phase8 is **PASS WITH RECORDED LIMITS**, not unrestricted completion.

## Prior accepted evidence / 此前已验收证据

See `2026-11-15-phase8-closeout.md`, `2026-11-15-phase8-security-execution-receipt.md` and `2026-11-15-phase8-cargo-lock-audit.md`:

- Six focused Phase8 cases, each 1/1.
- NSH 27 shell tests + 1 pinned v0.5 API test, 28/28; this does not prove live native opening or live canonical Chat/Artifacts.
- `alephcore` lib and server checks.
- Rebuilt pre-handoff-source WebChat artifact: build suite 6/6, artifact suite 12/12, protocol fixture PASS. After this commit advances HEAD, that historical artifact cannot establish new-HEAD provenance without a rebuild.
- QA parity reducer accepted/rejected smoke; source formatting/diff checks.

## Fresh handoff verification / 本次交接验证

The fresh full lib suite reached the 600-second supervisor timeout, was terminated with exit `-15`, and emitted no terminal test summary. This is `UNRESOLVED_TIMEOUT`, not PASS or proven source failure. Its 7,226-line log showed 33 over-60-second notices across 22 distinct extension/cache/hook tests, with no `FAILED`, compiler-error or panic markers before termination. Log: `/tmp/aleph-phase9-handoff-full-lib.log`; structured status: `docs/superpowers/runs/2026-11-16-phase8-handoff-verification.json`. The NSH package suite rerun passed (27 shell tests plus 1 API compatibility test). The first fresh Phase8 focused rerun (`foreign_scope_denial_records_policy_violation_without_handler_effect`) also reached the 300-second supervisor timeout with no test summary; the remaining five were not launched. Earlier 1/1 focused results remain in the Phase8 execution receipt, but this fresh timeout must be investigated and not masked. Full per-case status is in `2026-11-16-phase8-handoff-verification.json`.

Fresh static checks already completed: targeted NSH rustfmt, Python compilation of the reducer, JSON parsing of the schema and `git diff --check` all exit 0. Current `Cargo.lock` SHA-256 remains `d6f2e99dd2df5868b15f2940d74cf8845ea7c26d3ff69819ce42422180bb1cdb`.

## Review provenance / 审查来源

Fresh SPEC `826d7a9d-c679-40a3-beac-764c31de9696` returned PASS WITH RECORDED LIMITS after the timeout-classification repair. It confirmed unresolved waits remain `UNRESOLVED_TIMEOUT`, evidence-backed environmental unavailability alone is `BLOCKED`, teardown defects are `FAIL`, plan readiness is adequate to start next-session work, and no runtime proof is claimed. Historical SPEC `09e4fbf7-0595-41eb-b9e2-65de4173e3d5` remains BLOCK; the prior unsupported PASS attribution was withdrawn in `2026-11-15-phase9-spec-review.md`. The real historical QUALITY PASS from `75929d83-6e48-431a-8d12-d50191574975` remains a scoped verdict with provenance addendum. Independent QUALITY reviewer `933842ca-b352-4e23-990c-50137adb3541` returned OK for the restart prompt and provenance integration, limited to documentation readiness; it did not verify runtime, hashes or commit.

## Remaining work and exclusions / 未闭合与不做事项

Phase9 must investigate unresolved full-suite results, audit/minimize or explicitly retain material lock churn, implement the live launcher/schema/fixtures/markers and prove native Chat/Artifacts/reconnect/owned child settlement, then assess debug-only parity. Newly observed long-running tests in fresh verification supplement the old nine-filter inventory; do not silently restrict investigation to the old nine.

The generated lock has broad unrelated dependency changes; earlier minimal manual splicing failed resolver checks. That does not establish that every resolver-driven minimal-update strategy is impossible. Keep the original lock baseline frozen at `e08af3789dbf194f02f08461c215eb3c7701557f`, not future HEAD.

Not done or authorized by this integration: current-session Phase9 production implementation, Tauri/WebChat removal, detached daemon lifecycle ownership, silent R2/constitutional change, native Windows/Linux completion, release-profile performance acceptance, Safe Replay, exactly-once effects, universal scheduler, remote push/publication/release. Full-suite uncertainty remains explicit even when local integration is requested. 本次不把本地集成当成全量测试或 release 的绿灯。
