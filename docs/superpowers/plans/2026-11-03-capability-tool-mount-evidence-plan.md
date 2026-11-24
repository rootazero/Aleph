# Production Capability Mount Evidence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Native execution is preserved: main agent sole writer, one fresh read-only whole-diff review before delivery. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close production mount evidence and current authority documentation gaps without implementing a second mount or expanding deferred capability kinds. / 闭合生产 mount 的证据与当前文档缺口，不重复挂载或扩展 deferred kind。

**Architecture:** Startup already composes canonical Tool registry, one shared ownership tree and concrete Memory Resource through `ProjectionHost::mount_with_memory`. Reuse source-owned `StartupResources` cleanup and existing effect tests. Correct current claims only after evidence comparison; production changes are limited to a demonstrated local defect with a real RED. / 复用既有生产组合与清理，先验证后校准，生产修改仅限真实 RED 证明的局部缺陷。

**Tech Stack:** Rust, Tokio, existing alephcore/server tests, Python stdlib Markdown checker; no new dependencies. / 沿用现有栈，不新增依赖。

**Spec:** [Production mount evidence design](../specs/2026-11-03-capability-tool-mount-evidence-design.md)

**Status:** Planned, not executed. Prepared for the user's restarted session; no result below is a new PASS claim. / 已计划、未执行，供用户重启会话后使用，以下不构成已通过声明。

## Global Constraints / 全局约束

- Preserve `ToolHandlerRegistry`, `OwnershipTree`, `ProviderHandle`, `RuntimeOwner`, `MemoryRuntime` authorities. Memory Resource composition already exists; distinguish it from generic Resource support. / 保留各自权威，区分具体 Memory Resource 与通用 Resource。
- No new registry/tree/generation map/kind/proxy/supervisor, broad startup redesign, raw ownership rewrite, deferred non-Tool mount or replay expansion. / 不新增权威、不做宽范围改造或 deferred 扩展。
- Before every Cargo command run `python qa/browser_live/check_memory.py`; require `ALLOW` and available bytes >= `4294967296`. Use `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`, serial execution and positive selected counts. / 每次 Cargo 前门禁，串行且实际选中测试。
- Native sole writer; preserve cancelled admitted work, no lifecycle locks across await/I/O, no sleep races, global mocks or public test-only API. / 唯一写者，保留取消后的已准入任务，禁止跨 await 持锁等。
- Bilingual durable documents; English code comments. Synchronize `AGENTS.md` and `CLAUDE.md` in the same commit. / 双语文档、英文注释，双入口同提交更新。
- No push/merge, remote fetch, force or reset. Work from current local `main` containing `4b6da265a`; do not switch to unrelated `origin/dev`. Linux/Windows native validation remains waived. / 从本地 main 继续，不切换无关分支；原生平台豁免保留。

## Review Focus / 审查重点

1. Source guard passes while actual host delivery never happens → Task 1 must separate wiring from effect evidence. / 源码守卫不能替代效果证明。
2. Wrong registry/tree accepted → Task 1 exercises canonical publication/owner invalidation and existing mismatch rejection. / 错 authority 必须 fail closed。
3. Startup error/caller cancellation drops cleanup ownership → Task 2 exercises source-owned cleanup and retained waiter cancellation. / 错误和取消不得丢收尾。
4. “Tool-only” erases concrete Memory Resource, or Resource implies universal lifecycle → Task 3 checks both claims explicitly. / 双向防止夸大或遗漏。
5. Historical assertions silently presented as newly executed results → Task 3 records command, selected count, exit code and historical/current distinction. / 区分历史和本轮证据。

## File and interface map / 文件与接口

| File / 文件 | Responsibility / 职责 |
|---|---|
| `src/bin/aleph-server/commands/start/mod.rs` | Existing production composition and source-wiring tests / 生产组合与接线守卫 |
| `src/bin/aleph-server/commands/start/builder/startup_resources.rs` | Existing retained cleanup and real startup-error tests / 收尾及早期错误效果测试 |
| `src/capability/projection_host.rs` | Existing readiness/delivery/close effect tests / readiness、delivery、close |
| `src/capability/{projection_host_memory_tests,zahir_facade_memory_tests}.rs` | Concrete Memory Resource evidence / Memory Resource 效果证据 |
| `AGENTS.md`, `CLAUDE.md` | Current constitution pointers, synchronized / 同步宪法入口 |
| `docs/reference/{FEATURE_LOCATOR,ARCHITECTURE,MEMORY_SYSTEM}.md` | Owning current contracts / 当前契约归属；只改相关断言 |
| `docs/superpowers/runs/2026-11-03-capability-tool-mount-evidence-run.md` | Create evidence matrix, exact validation, review and limits / 新建证据与验收记录 |

No new public interface is planned. Existing APIs consumed: `ProjectionHost::mount_with_memory`, `wait_until_ready`, `close_and_await`, `StartupResources::{cleanup,finish}`. If a real defect needs a new cross-module interface, stop before inventing it and amend the design. / 不计划新 public API；需要新增跨模块契约时先修订设计。

### Task 1: Map the production authority and reuse effect evidence / 生产 authority 与效果证据

**Files:** Read start module, projection host, Memory Resource tests; create the run record above.
**Interfaces:** Consumes existing APIs; produces an evidence matrix with columns claim/source/static guard/effect test/result/gap. / 产出六列证据表。

- [ ] **Step 1:** Verify `git rev-parse --show-toplevel`, `git status --short --branch`, `git log -5 --oneline`, `git merge-base --is-ancestor 4b6da265a HEAD`. Preserve unrelated changes; never reset to the baseline.
- [ ] **Step 2:** Read repository constitution, relevant reference docs, spec, this plan and prior Tool activation run completely. Create the new bilingual run record before edits; record actual HEAD and no-duplicate-mount conclusion.
- [ ] **Step 3:** Trace actual registry/tree sharing through startup, ExtensionManager, host, request consumers and Memory runtime. Identify the first consumer after readiness and the shutdown funnel; distinguish process slot ownership from authority.
- [ ] **Step 4:** Read existing named tests before adding any. Under the per-command memory gate run separate positive-count selections:
  - `cargo test -p alephcore --bin aleph-server boot_mounts_projection_host_from_canonical_registry_and_awaits_readiness -- --nocapture`
  - `cargo test -p alephcore --bin aleph-server diagnostics_registration_follows_canonical_mount_readiness -- --nocapture`
  - `cargo test -p alephcore --lib initial_delivery_via_real_applier_and_fails_closed -- --nocapture`
  - `cargo test -p alephcore --lib ownership_bump_invalidates_without_registry_revision -- --nocapture`
  - `cargo test -p alephcore --lib resource_is_discoverable_without_fake_tool_entry -- --nocapture`
  - `cargo test -p alephcore --lib resource_resolve_does_not_mint_custom_scope_or_accept_another_tree -- --nocapture`
- [ ] **Step 5:** Record each actual selected count and result; source guard PASS is not runtime PASS. If existing effects already prove a guarantee, mark reuse and do not add a duplicate test. A command selecting zero tests is a validation failure.

### Task 2: Verify cleanup and only repair confirmed local gaps / 收尾验证与局部缺口

**Files:** Existing startup resources and host tests; production only if RED proves a local defect.
**Interfaces:** Consumes Task 1 gaps; produces verified evidence or an explicit design blocker, no new authority.

- [ ] **Step 1:** Read cleanup order in production: Memory stop, host close/join, builtin registry scope disposal; read `finish` error-preservation semantics.
- [ ] **Step 2:** With a fresh memory gate before each Cargo command, run:
  - `cargo test -p alephcore --bin aleph-server boot_closes_projection_host_before_registry_scope_dispose -- --nocapture`
  - `cargo test -p alephcore --bin aleph-server early_registration_error_drains_memory_and_its_projection_host -- --nocapture`
  - `cargo test -p alephcore --bin aleph-server early_startup_failure_preserves_original_error_after_cleanup -- --nocapture`
  - `cargo test -p alephcore --lib cancelled_first_waiter_does_not_detach_teardown -- --nocapture`
  - `cargo test -p alephcore --lib idle_close_wakeup_joins_both_workers -- --nocapture`
  - `cargo test -p alephcore --lib concurrent_close_is_idempotent -- --nocapture`
- [ ] **Step 3:** Classify all remaining gaps. For a confirmed missing guarantee, use the relevant existing fixture and Notify/channel handshake to assert the actual delivery/file/worker outcome before changing production. A compile failure alone is not RED; no added test is required if evidence already covers the claim.
- [ ] **Step 4:** If a local defect exists, implement the smallest fix in its existing owner and rerun its RED plus affected siblings. If a new authority, broad startup redesign, raw ownership change or deferred mount is required, record a blocker and stop dependent work for design review.

### Task 3: Align current documentation, validate and hand off / 文档校准、验收与交付

**Files:** Constitution pair, owning references as needed, new run record; update this plan checkboxes only after actual completion.
**Interfaces:** Consumes verified matrix/results; produces current facts and bounded next-slice recommendation.

- [ ] **Step 1:** Correct the matched stale no-production-mount paragraph in both `AGENTS.md` and `CLAUDE.md` identically. State existing Tool callable backend + concrete Memory Resource, startup readiness/common cleanup, independent authorities and remaining deferred kinds. Keep constitution concise and link to owning reference rather than copy the run.
- [ ] **Step 2:** Reconcile corresponding current reference claims; preserve dated research/run text as historical rather than rewriting past results. Search for both absence claims and overbroad all-kinds-mounted claims and classify every hit.
- [ ] **Step 3:** Run `python qa/docs/check_markdown_links.py --self-test`, `python qa/docs/check_markdown_links.py`, and `git diff --check`. Require no unresolved recognized repository links; report the scanner's syntax limits. Verify the two constitution edits agree.
- [ ] **Step 4:** If any Rust production/test source changed, memory-gate and run serial `cargo check -p alephcore --tests`, `cargo check -p alephcore --bin aleph-server`; run scoped clippy only when the changed logic justifies it. If no Rust changed, focused runtime evidence from Tasks 1–2 plus docs checks suffice; disclose unrun broad checks. Do not run whole-repository format to fix unrelated differences.
- [ ] **Step 5:** Obtain a fresh read-only whole-diff review through an actually supported Pi model. Fix concrete findings and rerun affected checks. A timeout or missing report is not approval. No model CLI fallback or unsupported hard-coded model name.
- [ ] **Step 6:** Finish bilingual run record with exact counts, gaps/non-gaps, reviewer verdict, residual risks and non-actions; mark plan steps accurately. Commit locally with `docs(capability): align production mount evidence` (or `fix(capability): ...` only for an actual production fix). Verify clean status and no push/merge.
- [ ] **Step 7:** Recommend one next capability slice based on remaining source gaps and total goal. List its required architecture decision; do not implement it under this plan.

## Self-review / 自审

Spec coverage: Task 1 covers identity/readiness/Memory exception; Task 2 covers error/cancellation/drain; Task 3 covers synchronized facts, validation and bounded follow-on. No invented API, new authority or assumed defect. Existing tests precede any new test; test filters are substrings and must select positively. This is an evidence task, so no production rewrite is required for completion. / 覆盖完整，无假造 API 或缺陷；已有测试优先，正数选中；完成不以生产改写为条件。
