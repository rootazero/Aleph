# Phase9 Candidate Entry Inventory / Phase9 候选入口盘点

**Date / 日期:** 2026-11-15
**Status / 状态:** candidate planning only; not an official Phase9 charter
**依据:** Phase8 closeout `PASS WITH RECORDED LIMITS`

## Naming boundary / 命名边界

The repository has no approved Phase9 charter, official name, schedule or public interface. This document therefore proposes a candidate label only: **Runtime Proof & Dependency Closure / 运行时证据与依赖闭合**. The label and scope require human approval in the next session before production implementation.

仓库当前没有已批准的 Phase9 charter、正式名称、工期或公共接口。因此本文只提出候选名称：**Runtime Proof & Dependency Closure / 运行时证据与依赖闭合**。名称与范围必须在下一次会话中获得人工批准后，才能进入生产实施。

## Carry-forward evidence / 承接证据

- Phase8 bounded F1–F5 tests: passed.
- `alephcore` and `aleph-server` compile checks: passed.
- NSH v0.5 shell tests and Window/WindowRef compatibility: passed.
- Current-HEAD WebChat artifact build and provenance/artifact/protocol checks: passed after task-local `wasm-bindgen-cli 0.2.130`.
- Full `alephcore --lib` suite: no final verdict within 600 seconds; nine existing extension/cache tests emitted long-running markers and no error/failure summary.
- Cargo.lock: resolver-valid generated graph retained, with 328 added and 269 removed package identities, 285 exact package-name version/source set changes, and 7 pinned NSH Git entries. Frozen pre-NSH baseline is commit `e08af3789dbf194f02f08461c215eb3c7701557f`, `Cargo.lock` SHA-256 `bb1b647bb8f53785467c2dd47b40ed0bd46f2514c9de4e6e94bc29fccc900345`; retained generated lock SHA-256 is `d6f2e99dd2df5868b15f2940d74cf8845ea7c26d3ff69819ce42422180bb1cdb`.
- Live native NSH window, live Gateway Chat/Artifacts, dual-run and release-profile performance evidence: not executed. Release-profile performance is a deliberate `RECORDED_LIMIT` for the candidate Phase9 plan.

## Candidate Phase9 boundary / 候选 Phase9 边界

In scope for a future implementation approval:

1. Produce a deterministic full-suite result or a source-backed slow-test closure plan with per-test ownership, timeout cause and terminal evidence.
2. Decide the Cargo.lock strategy with resolver-backed evidence and dependency review; avoid silent lock churn.
3. Execute the debug-only owned-root NSH QA launcher and receipt protocol against current binaries.
4. Prove one live canonical Chat `/` + Artifacts path, including reconnect and session artifact invalidation.
5. Run native macOS window proof and compare the measured debug QA result against the approved receipt schema.

Future Phase9 work must preserve existing Capability authorities, R2, detached daemon exclusion and the no-second-registry/no-second-runtime rules.

## Explicitly out of scope / 明确排除

Tauri/WebChat removal, detached daemon lifecycle ownership, silent R2 or constitutional changes, all-platform NSH completion, release-profile performance acceptance, Safe Replay, exactly-once external effects, universal scheduling, publication, push and release.

## Approval gate / 批准闸门

This inventory authorizes no production change. The next session must approve the candidate name, scope, exact live QA environment and any proposed source/test files before Phase9 implementation begins.

本文不授权任何生产变更。下一次会话必须先批准候选名称、范围、live QA 环境及拟修改的源码/测试文件，之后才能开始 Phase9 实施。
