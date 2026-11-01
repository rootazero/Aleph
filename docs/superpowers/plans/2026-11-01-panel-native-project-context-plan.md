# Native Panel Project Context Acceptance Plan / 原生 Panel 项目上下文验收计划

> **For agentic workers:** Start with the read-only audit/design task below. After the native design is approved, use `superpowers:writing-plans` to refine the executable tasks, then `superpowers:executing-plans` task-by-task. Steps use checkbox syntax. / 先审计与设计，native 设计批准后细化执行计划，再逐项执行。

**Status / 状态:** Proposed next phase, prepared at the user's request after merging the completed task. This document is not approval of a new native credential or authorization architecture. / 用户要求合并完成任务并规划下一步；本文件是下一阶段建议，不冒充新 native 架构批准。

**Goal / 目标:** Prove the existing authenticated community Panel project context through a real Windows Tauri/WebView2 host, its actual credential IPC and protected credential backend, and the real production-library Gateway. Fix only defects demonstrated by this path. / 用真实 Windows 原生宿主、凭据链和生产 Gateway 验证已实现的项目上下文，仅修复实证缺陷。

**Architecture / 架构:** Reuse the existing Core publication authority and the one Gateway-owned selector shared by RPC and HTTP. Extend the existing owned-root native QA infrastructure with a bounded project scenario; the host remains I/O and retains its existing credential lease/endpoint checks. QA controls may drive real UI and server-side fixture lifecycle, but must not synthesize snapshots, byte delivery, native IPC or project authority. / 复用唯一 Core 权威、共用 selector 和既有 owned-root native QA；不增加第二套 registry、授权或假 native bridge。

**Tech Stack / 技术栈:** Rust/Tokio/Axum, existing `ProjectStore`/`SecurityStore`, Leptos/WASM, Tauri 2/WebView2, existing native credential Bridge, Node QA orchestration.

**Spec / 规范:** Existing invariants are in [trusted-project design](../specs/2026-11-01-panel-trusted-project-context-design.md). Task 1 must produce a native acceptance design at `docs/superpowers/specs/2026-11-01-panel-native-project-context-design.md` before implementation. / 既有规范继续约束；新 native 设计尚待 Task 1 形成并批准。

## Starting point / 起点

Local `main` was fast-forwarded from `52e13cb3db8cf27a2a9bede773cbcc8742cb28e5` to `904e6dc99351590871ba824791c8233cc681f546`. No conflicts or history rewrite; no push. Resolve the current root and HEAD at session start instead of assuming a checkout path or that no later documentation commits exist. / main 已快进合并完成的六个提交；重启后以当前 git root/HEAD 为准。

Read first / 先读：

- [Trusted-project run ledger](../runs/2026-11-01-panel-trusted-project-context.md).
- [Production publication validation](../../reports/panel-community-live-publication-validation.md).
- [Original implementation plan and rulings](2026-11-01-panel-trusted-project-context-implementation-plan.md).
- `docs/reference/FEATURE_LOCATOR.md` §6.11, `TERMINAL_RUNTIME.md`, `PRODUCT_TOPOLOGY.md`, `DESKTOP_SHELL.md`, `SECURITY.md`, `DEVELOPMENT.md` and relevant local constitutions.
- `qa/native_ipc/README.md`, `run-real.mjs`, `real_run_core.mjs`, `real_run_io.mjs`, `panel_driver.mjs`, `monitor_contract.mjs`, and `real_qa_prep.mjs`.

### Established evidence / 已证明内容

| Area / 范围 | Actual result / 实际结果 | Interpretation / 含义 |
| --- | --- | --- |
| Selector | 7 passed | Exact private source; not ingress proof / 私有源码行为，不冒充入口 |
| Real WS + HTTP project path | 1 passed | Actual authenticated Gateway TCP traffic / 真实认证流量 |
| HTTP / census / protocol | 2 / 4 / 9 passed | Separate executed targets / 分别执行 |
| Native Rust Panel/Terminal tests | 1607 passed | Native unit/regression execution, **not a native WebView launch** / 非 WebView 真机 |
| Fresh production-feature WASM/glue loader | 15 passed, 0 skipped | Source/glue provenance verified / provenance 已核对 |
| Real Gateway + Chrome | Gateway fixture 1 passed, driver exit 0 | 12 snapshots, 13 observed assets; project/user isolation and lifecycle verified / 真实浏览器证据 |
| Whole-branch review | Complete written assessment, no confirmed issue; process SIGTERM | Not a clean-process review or independent runtime rerun / 不称 clean review |

Fresh WASM SHA-256 from the completed phase: `4ff8be5092bb21ad90b502cd37dba909c217dcc571e5448e213c036ea63e8488`; 14 source hashes matched. Reuse only with an explicit current-source provenance check. / 只可在当前源码 provenance 一致时复用。

### Remaining work classification / 未完成内容分类

1. **Recommended now:** real native WebView credential/project/asset/lifecycle proof. / 本阶段推荐补齐真机证明。
2. **Verification limits, not passing results:** monolithic Core lib-test LLVM OOM, full workspace suite not run, Windows symlink privilege limitation, historical deterministic Terminal/spike browser suites not rerun in the latest phase. Address only checks relevant to native changes; report the rest accurately. / 不以 OOM 或未执行充当通过，不把全部历史验证债打包进此阶段。
3. **Separate product projects:** install/update/uninstall, marketplace/signatures, multi-file assets, external WASM, default-release switching, live notifications/background refresh and Zahir runtime mount. These require their own design and remain excluded. / 这些是独立产品阶段，不是当前项目上下文的未完成实现。
4. Process-local device selection, Global + selected project visibility, next-boundary revocation/cross-tab observation and the lack of immediate idle teardown are intentional approved semantics. / 这些是设计边界，不默认作为缺陷修复。

## Global Constraints / 全局约束

- Core remains the only declaration/immutable-byte/trust/scope/source-revision authority; RPC and HTTP use the same selector. No second project authority or publication registry. / 保持唯一权威。
- Missing claim means Global. Explicit malformed, inaccessible, cross-user, archived, revoked or no-workspace claims refuse without Global fallback; use `Some(actor)`, never `authz::project_for(..., actor=None)`. / 显式无效 claim 不回退。
- No project inference from CWD, filesystem paths, request URL/query/header or client booleans. Keep project roots out of the Panel context wire/browser state. / root 保持内部。
- Snapshot arrival stays passive; only explicit selection loads a module. Preserve Terminal `ControlPlaneAssets`, generic refusal and exact community `private, no-store`. / 不借验收扩大加载行为。
- Real native evidence requires the unmodified real Tauri invoke boundary and native trace. Do not replace `__TAURI_INTERNALS__`, fake native markers, supply browser-storage credentials as native proof, or turn in-memory backend success into OS-keyring evidence. / 不伪造 native 身份或凭据链。
- Use `qa-owned-root` debug **lite** shell (`--no-default-features`), an owned WebView profile and QA-only exact credential namespace; no production default/release switch, installed daemon replacement or embedded-core supervision. / 不触及日常安装与服务。
- Use only a disposable Gateway this run owns. Existing `run-real.mjs` requires `ws://127.0.0.1:18790/ws`; that restriction is not permission to attach to, stop or replace the user's daemon. Resolve the QA origin design explicitly. / 固定端口限制不等于用户服务授权。
- Keep the existing exact `(service, account)` credential cleanup and owned PID/root checks. Do not enumerate/delete unrelated credentials or use process-name-wide kills. Tokens stay out of argv, env, tracked files, reports and screenshots. / 只清理本次拥有的对象。
- Before **every Cargo invocation**, run `python qa/browser_live/check_memory.py` from the worktree root; `CARGO_BUILD_JOBS=1`, serial Cargo. Use one consistent profile/cache; no `bash -lc` for Cargo. / 内存 guard、单 job、不并发，不用 login shell 改变工具链环境。
- The known Core harness OOM does not justify another identical retry or adding public testing APIs. Prefer independent production-library targets. The existing Terminal HTTP test writes/removes watched `dist/` files; compile required targets once, then run freshly built executables or run that test last. / 避免重复重编译，不能用 test shim 降低证据标准。
- English code comments, bilingual docs, isolated short branch/worktree from current local `main`; no merge/push without a new explicit request. / 重启后新建隔离分支。

## Review Focus / 审查重点

1. A real paired native device must resolve the same actor in RPC and asset HTTP; a page-provided identity or fake Gateway cannot establish that proof. / Task 3–4。
2. Endpoint/lease changes during credential handoff must refuse, with no browser-storage fallback. / Task 2、4。
3. Equal Core revision can carry a changed authorized filtered view; old A completion/refusal must not mount A or clear B. / Task 3–4。
4. Native reload must load the protected credential for the same device, not silently pair again; server revocation must refuse the next boundary. / Task 4。
5. A missing monitor, incomplete trace, wrong binary/source, fake backend or ambiguous cleanup must be NOT_RUN/BLOCKED/FAIL rather than PASS. / Task 2、4–5。

## File boundaries / 文件边界

Audit these existing owners first / 首先审计：

- `interfaces/webchat/src/context.rs::panel_asset_credential`, `interfaces/webchat/src/api/native_credentials.rs`, `interfaces/webchat/src/panel/sdk.rs`, `interfaces/webchat/src/panel/loader/browser.rs`, `interfaces/webchat/src/panel/discovery.rs`: existing host credential and context fences.
- `desktop/shell/src/{main.rs,keyring_bridge.rs,qa_root.rs,qa_trace.rs}`, `desktop/shell/tauri.qa.conf.json`: real IPC, target/lease checks and isolated QA hooks.
- `qa/native_ipc/{run-real.mjs,real_run_core.mjs,real_run_io.mjs,panel_driver.mjs,monitor_contract.mjs}`: reuse owned-root/process/reaper/native-trace helpers; preserve historical runner semantics.
- `tests/panel_http/{live.rs,common.rs}`, `tests/panel_project_rpc.rs`: production-library Gateway and actual project lifecycle fixtures.
- Proposed new QA-only scenario module/entrypoint: `qa/native_ipc/panel_project_scenario.mjs` and `qa/native_ipc/run-panel-project.mjs`, **only if the audit confirms a separate thin scenario is needed**. Final names/interfaces are locked in the approved native design; do not scaffold them during audit. / 不先造第二套框架。
- New report/run ledger: `docs/reports/panel-native-project-context-validation.md`, `docs/superpowers/runs/2026-11-01-panel-native-project-context.md`; update the existing reference entry rather than inventing another status authority.

## Task 1: Audit native proof and approve the bounded design / 审计并批准窄设计

**Deliverable / 交付:** Source-backed native boundary matrix, prerequisite verdict and approved design. No runtime or product implementation in this task. / 此任务只读调查和设计。

- [ ] Reconfirm current main/HEAD, clean status and the merged evidence. Locate the current machine's toolchain, WebView2, QA shell/reaper sources, hashes and unused owned test endpoint; stale executables are not proof.
- [ ] Trace actual native connect → lease → protected store → RPC actor → host bearer → HTTP context. Identify how fixture pairing produces a **verified user bound to the native device**, not merely an unbound legacy credential.
- [ ] Record that `run.mjs` proves fake-page/memory-backend IPC; `real_qa_prep.mjs` normal mode always returns NOT_RUN; `real_run_core.mjs::validateRealArgs` currently fixes Gateway port 18790. Audit current sources over historical README claims. / 明确已有装置证明范围。
- [ ] Present 2–3 viable QA binding approaches grounded in the real code. Recommend reuse of owned-root/native trace plus a narrow real project scenario; never relax production origins/leases or reuse a user's daemon to satisfy a fixture.
- [ ] Resolve owned Gateway origin, protected pairing path, real DOM driving, exact credential cleanup, source/served-asset provenance, and a bounded window budget in the design. Historical run-window permissions/constants are not automatic authorization for new launches. / 明确本次窗口预算，不暗改历史限制。
- [ ] Write the bilingual native design, perform a fresh builder check, obtain explicit design approval, then refine Tasks 2–5 through writing-plans. If trustworthy actor-bound/native proof is unavailable, deliver an audit finding and concrete blocker instead of a weaker endpoint.

## Task 2: Make the native QA scenario fail closed / 原生场景装置先 fail closed

**Depends on / 依赖:** Approved Task 1 design; reuse the existing owned-root, process and exact-reaper contracts.

- [ ] Write meaningful RED scenario/contract tests for foreign/unowned Gateway, wrong origin, stale lease, missing native trace, memory backend, stale binary/source and ambiguous cleanup. A pure self-test must never produce a protected-success verdict.
- [ ] Run existing pure controls first: `node qa/native_ipc/run-real.mjs --self-test`, `node qa/native_ipc/panel_driver.mjs --self-test`, `node qa/native_ipc/real_qa_prep.mjs --self-test`. These are harness checks only, not native runtime acceptance.
- [ ] Implement only the approved QA scenario/monitor actions and strict report contract. Reuse process/root/credential cleanup; do not intercept native IPC or inject project state. Pin shell, reaper, source and served-WASM hashes before launch.
- [ ] Re-run RED controls to GREEN and retain a negative control proving that loss of genuine native evidence cannot report PASS. Commit `test(panel): add native project acceptance scenario` with ledger evidence.

## Task 3: Wire the real project Gateway fixture / 接线真实项目 Gateway fixture

**Depends on / 依赖:** Scenario contract and actor/endpoint decisions from Tasks 1–2. Consume the existing selector and Core producer unchanged.

- [ ] Reuse or extend the opt-in production-library fixture with Global/A/B modules, two users, server-authorized native pairing and local owned lifecycle controls. Keep actual `/ws`, `panel.project.select`, `/tabs/...`, SecurityStore and ProjectStore.
- [ ] Add RED assertions for native verified-device binding, foreign project refusal, current project denial without Global fallback, and equal-Core-revision A/B visibility. Do not seed a browser credential or fake a native bridge to make the test pass.
- [ ] Add only the smallest fixture/QA wiring needed. If a product defect is demonstrated, pin it with a behavior test before a narrow production fix, preserving existing authority and IPC contracts.
- [ ] Build fresh production-feature Panel/WASM and QA shell/reaper with the memory guard and jobs=1; apply `tauri.qa.conf.json` as the existing recipe specifies. Stage external-bin placeholders via the existing recipe if required; do not duplicate placeholder paths or use `embedded-core`.
- [ ] Run focused fixture/bridge tests to GREEN, record exact commands/provenance, commit `test(panel): wire native project gateway proof`.

## Task 4: Execute actual Windows WebView2 acceptance / 执行真实 Windows 验收

**Proof / 证明:** Actual shell/WebView + native credential trace + real Gateway receipts + host UI/module counters. DOM success alone is insufficient. / 四方证据相互印证。

- [ ] Preflight only; no launch until all approved owned-root, endpoint, binary/hash and exact-keyring cleanup checks pass. Missing prerequisites produce NOT_RUN/BLOCKED with the exact reason.
- [ ] Run the approved bounded owned shell window(s); prove native IPC load/save/reload with the protected backend and the same server device identity, without re-pair or web-storage fallback.
- [ ] Observe passive discovery, explicit A load, Global+A/B visibility, same-Core-revision A→B, no foreign user access, and explicit Global clearing. Assert HTTP bearer/current identity/context generation without logging credentials.
- [ ] Hold a real A request across B selection and prove no late A mount/import or clearing of B. Exercise endpoint/lease invalidation during handoff. A request already authorized before revocation may finish; do not claim retroactive cancellation.
- [ ] Revoke/archive/remove current project access and revoke the device through existing server authority; the next snapshot/asset boundary refuses without Global fallback. Idle immediate teardown/live push remain excluded.
- [ ] Prove exact owned process, credential and root cleanup; ambiguous cleanup is not PASS. Record redacted reports, hashes, counters and limits; commit `test(panel): record native project acceptance`.

## Task 5: Regression, independent review and handoff / 回归、审查与交付

- [ ] Run checks appropriate to actual changed files. If webchat changes, run native Panel/Terminal regression and fresh WASM/glue loader; keep production-library RPC/HTTP and Terminal/public behavior green. Guard every Cargo command and run serially.
- [ ] Keep evidence categories separate: harness self-test, memory IPC, protected native persistence, actual community asset load, and desktop runtime. Never promote one into another. Do not label the old Chrome result native proof.
- [ ] Perform a fresh read-only whole-branch review after final product changes. Disclose SIGTERM/timeouts and unresolved findings. The previous review's SIGTERM is not retrospectively repaired by this review.
- [ ] Update native report, run ledger, `FEATURE_LOCATOR` §6.11 and `TERMINAL_RUNTIME`; distinguish PASS, BLOCKED and unrun scopes. Commit conventionally and finish tracked-clean on the isolated branch.
- [ ] Final output lists actual executed checks, remaining limits, evidence paths and branch HEAD. No merge/push, installation, release switch or unrelated daemon operations.

## Acceptance and stop conditions / 验收与停止条件

Complete only when real WebView2 follows the existing native credential path into the same authenticated project context used by discovery and HTTP bytes, the positive/negative lifecycle cases pass, and cleanup/provenance are proven. A fake backend or page cannot satisfy this. If the machine or current APIs cannot provide it, stop with an auditable native blocker report and a concrete next design decision; do not expand production permissions. / 真实链路与清理均证明才完成；否则交付明确 blocker，不降级验收标准。

**Plan self-review / 计划自查:** Prior progress is preserved; intentional limits are separated from missing evidence. All proposed implementation tasks depend on Task 1's design approval. Five review-focus classes map to Tasks 2–4. No new runtime APIs, helper signatures or historical launch permissions are asserted as already available. / 未把建议当批准，也未虚构已有能力。
