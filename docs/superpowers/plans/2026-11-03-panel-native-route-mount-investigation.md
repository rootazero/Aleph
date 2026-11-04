# Panel Native Route Mount Investigation Plan
# Panel native 路由挂载调查与验收续行计划

> **For agentic workers:** Use `superpowers:executing-plans` to execute this plan task by task in the main session. 用户要求直接执行，禁止子代理和 CLI 模型；该约束覆盖技能默认委派建议。
>
> **Status / 状态：** Restart handoff, not execution authorization / 重启交接，非 native launch 授权。先完成源码调查、RED/GREEN、artifact/provenance 与可审阅 preflight，再请求一次新的 120 秒授权。

**Goal / 目标：** Explain the authenticated project-A route's missing Panel surface, fix a demonstrated QA defect within scope, and continue A/B native acceptance only after a new launch grant. 定位 A route 上 Panel surface 未出现的具体边界；只修复证据支持且授权范围内的缺陷。

**Architecture / 架构：** Keep real Projects UI → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` → authenticated Gateway. QA drives only closed fixed navigation/DOM actions; native credential effects, helper receipts, DOM observations and artifact checks remain separate evidence. 不用 Node HTTP、VM、DOM 或 cleanup 替代 native credential/project/asset 证据。

**Tech Stack / 技术栈：** Rust/Tauri QA-owned shell + Leptos/WASM Panel + Node ESM harness + WebView2 + exact OS credential reaper.

**Spec / 规格与事实源：** This is a bounded investigation follow-up to [the prior completion plan](2026-11-03-panel-native-acceptance-completion.md) and [the run ledger](../runs/2026-11-01-panel-native-project-context-acceptance.md). 本文更新“当前状态”和下一步任务，不重新执行旧计划已经完成的工作。历史方案中的旧 HEAD、未修 reload 等现状描述已过期；以当前源码、本文和最新 run 为准。

## 1. Restart identity / 重启身份与证据基线

本交接编写时已实际检查：

- `git status --short` 无输出；当前分支 `main`，HEAD `f9fa908b1c215ea93401abbb6e42d107c7f350e8`。This is an observed handoff baseline, not an instruction to reset to it.
- 既有 QA/docs 工作已经存在于当前干净检出；不要根据旧对话的“14 个未提交文件”重置或重新实施。
- 历史 native build/provenance revision 是 `063345f2f30bf999b28ee5d84aac860be8432fa7`；**它不能作为当前 HEAD 的有效 preflight**。新会话先检查实际 HEAD 和 artifacts，缺失/过期则重建；不得把旧 manifest 仅改 revision 后复用。
- 最新完整 Node native suite 历史结果为 95/95；这是上次运行前的结果，不是新会话已执行的验证。
- No active launch authorization / 所有历史单次 launch 授权均已消费。本计划及其 prompt 不授予启动窗口。

最新 native run：`dd6f874403829221f12f09f277fe008dd30984091f21dd9b990c777bdd48a0a5`，exit 5，`BLOCKED/panel_route_unavailable`。

| Boundary / 边界 | Accepted evidence / 已接受证据 |
| --- | --- |
| helper_ready / protected_pairing | PASS |
| protected_reload | PASS；save seq 3，`ok:true,resultShape:"null"`；load seq 4，`ok:true,resultShape:"lease_object",credentialPresent:true` |
| global_discovery | PASS |
| project_a | select/discovery receipts seq 5/6；连同 Global 共六条 helperMessages/receipts；第三个 document 的 monitor hello 通过认证 |
| A route inspect | `{panelShell:false,panelTabA:false,panelTabB:false,panelContent:false}`；B 未到达；未进入 asset receipt polling |
| primaryFailure | `phase:"project_a",operation:"monitor_inspect"` |
| summary | `lastBoundary:"monitor_command",failureReason:"panel_route_unavailable",missingLayers:[]`；这是原报告值，不能修改成期望值 |
| cleanup | 六项全部 true，次序见下文 |

Fixed diagnostics / 固定诊断：

```json
{
  "monitor": {"upgrades":3,"accepted":3,"helloSeen":3,"refusedOrigin":0,"refusedOther":0,"refusedConcurrent":0,"streamInvalid":0,"installFailed":0,"refusedOriginKinds":{"missing":0,"appShell":0,"other":0},"installFailedReason":null},
  "trace": {"records":4,"unconsumed":2,"ignoredLines":5,"poisoned":null},
  "helper": {"sent":11,"acknowledgements":1,"timeouts":0,"closed":0,"invalid":0},
  "receipt": {"requests":7,"retries":1,"notFound":1,"stale":0,"invalid":0,"cursorBefore":5,"cursorAfter":6}
}
```

**Not established / 尚未证明：** 为何 Panel 未出现；A/B asset、B surface、late-A UI fencing、endpoint/lease、业务 revocation。exact cleanup revoke 成功不能升级成业务 revocation 验收。`missingLayers:[]` 不意味着整体验收完成。

## 2. Global constraints / 全局约束

- 在同一仓库当前 `main` 直接推进；先 `git rev-parse --show-toplevel` 定位，不在持久文档写死检出绝对路径。不 reset/rewrite 历史，不删既有工作，不自行 commit/push/merge。
- No subagents or model CLIs / 不使用子代理，不启动任何 CLI 模型。主 agent 做源码审计、实现、测试与自审。
- 已批准 QA-owned monitor 的固定导航与脱敏诊断扩展；默认允许 `qa/native_ipc/*`、`desktop/shell/src/main.rs` 内 QA monitor、计划/run ledger/`FEATURE_LOCATOR.md`。产品 Panel/Gateway/认证/credential semantics 的修改须另获明确 scope/architecture approval。源码可以只读调查。
- 不新增 pairing UI、hidden selector、direct Tauri invoke、browser credential storage、fake actor、`actor=None`、test endpoint、产品 RPC/wire 字段；不通过删掉 mount/receipt 判据制造 PASS。
- Monitor 仅固定 actions 和字面量 `/projects`、`/panel/project-a/overview`、`/panel/project-b/overview`。不接受 caller URL/route/selector；任何新 QA 字段必须闭集、脱敏、协议/测试同步。
- Credential effect 仅来自 owned shell 的 authenticated `ALEPH_QA_TRACE`。保留 `--qa-credential-backend os`；不得切换 memory 让测试通过。
- 不连接、停止或替换日常 daemon，不碰 `18790`，不读取旧 token-bearing connection file；不按进程名/端口 kill，不 wildcard 删除 credential。
- 报告只含固定 enum、boolean、有界 counter、允许的 revision/digest/cursor。禁止 raw URL/Origin/port/runtime path/ticket/token/credential/request ID/device ID/lease ID/raw OS error；源码相对路径可以用于交接定位。
- 每条 Cargo 命令前从仓库根执行 `python qa/browser_live/check_memory.py`，通过后设 `CARGO_BUILD_JOBS=1`。不使用 `bash -lc` 隐藏环境设置。
- 六步 cleanup 必须保持 root-last：`shell_stopped` → `helper_stopped` → `exact_devices_revoked` → `credential_deleted_exact` → `credential_absent_exact` → `owned_root_removed`。reaper 只认 exact `(service, account)`，cleanup 不覆盖 primaryFailure。
- 新 preflight 未就绪前不启动 WebView2；preflight 后仍须新的明确授权。每份授权最多一次 120 秒 business window，结束后停止，无自动第二次 launch。

## 3. File map / 文件责任与阅读顺序

先读 `AGENTS.md`、`docs/reference/WINDOWS_RUNTIME.md`、`docs/reference/DESKTOP_SHELL.md`、`docs/reference/DESKTOP_BRIDGE.md`、`docs/reference/FEATURE_LOCATOR.md` §6.11.2 和最新 run ledger。相关子目录指令也须遵守。

| Files / 文件 | Responsibility / 审计问题 |
| --- | --- |
| `qa/native_ipc/native_project_native_io.mjs` + `_test.mjs` | `waitForPanelMount`、`navigateNativeDocument`、reload cursor、A/B/late-A sequence、failure operation 与真实 DOM 输入 |
| `desktop/shell/src/main.rs` | QA monitor 固定路径、inspect selector、ack/assign/hello 时序、实际加载 bundle 的机制 |
| `qa/native_ipc/monitor_contract.mjs`、`native_project_driver_test.mjs`、`panel_driver.mjs` | 严格闭集 wire；测试是否读取真实 monitor JS，mock 是否只是复述 selector 假设 |
| `qa/native_ipc/real_run_io.mjs`、`real_run_core.mjs` | single-document arm、authenticated monitor、shell argv、owned trace/process 边界 |
| `interfaces/webchat/src/app.rs`、`panel/projection.rs`、`panel/shell.rs` | `/panel/{plugin}/{tab}` 路由、feature/form factor/discovery/descriptor gate、真实 DOM markers |
| `interfaces/webchat/src/context.rs`、`components/sidebar/projects.rs`、`api/native_credentials.rs` | project select、reconnect/discovery、native lease 与 credential 调用链；只读 |
| `interfaces/webchat/src/panel/loader.rs`、`interfaces/webchat/src/panel/loader/browser.rs`、`interfaces/webchat/panel_loader.js` | activation → credential → authenticated asset fetch 的边界；只读 |
| `src/bin/aleph-panel-native-qa-gateway/main.rs`、`src/gateway/panel_project_context.rs` | fixture A/B `overview` identity、authenticated device selection、snapshot 与 asset receipt；只读 |
| `qa/native_ipc/native_project_evidence.mjs`、`native_project_runner.mjs`、`native_project_run.mjs`、`native_project_run_core.mjs` | reason 闭集、diagnostics、first-failure-wins、phase/operation 与 missingLayers |
| `qa/native_ipc/native_project_provenance.mjs`、`desktop/shell/tauri.qa.conf.json`、`interfaces/webchat/scripts/build-panel-tabs.mjs`、`interfaces/webchat/scripts/build-panel-live.mjs` | staged `.panel-live/shell` 实际构建/加载来源、feature 与 revision/digest |

## 4. Review focus / 重点防止的误判

1. 量具错误：全 false 可能来自错误 selector、错误 document 或错误 bundle；先对照真实渲染代码，再称为路由失败。
2. 历史 provenance 错配：当前 HEAD 已变化；旧 artifact hashes 匹配旧 manifest 也不能证明当前源码。
3. authenticated hello 只证明 monitor document，不能证明目标路径、应用启动或 Panel readiness。
4. route navigation 会创建新 document；不能默认旧 JS discovery/selected state 被保留，也不能默认 Gateway device selection 丢失。
5. mount、tab presence、asset receipt 和 UI fencing 是不同事实；完整 document navigation 不能替代同 document 的 stale-A UI fencing。

## 5. Tasks / 分步执行

### Task 1 — Establish current state and falsifiable hypotheses / 建立可证伪调查表

**Files:** 当前 plan、旧 run ledger、上表源码；记录到 `.superpowers/sdd/2026-11-03-panel-native-route-mount-investigation/progress.md`（ignored）。

- [ ] 运行 `git status --short`、`git branch --show-current`、`git rev-parse HEAD`；记录实际状态与初始改动，禁止回退到交接 HEAD。
- [ ] 按上表读代码，记录每个 marker 的生产渲染位置、monitor selector、fixture identity 与触发时机。当前检查已确认 `data-panel-shell`、`data-panel-content-root` 在 `panel/shell.rs`，但新会话仍须验证代码未变化。
- [ ] 建表：hypothesis / exact source evidence / smallest deterministic test / what native evidence is still missing。覆盖实际 document path、bundle/feature、form factor gate、discovery readiness、descriptor visibility/status/key、Panel loader activation。
- [ ] 先审计新 document 的启动/reconnect/snapshot/route 重评估顺序；区分 source fact、历史 runtime fact 和 hypothesis。不得以“肯定 selection reset”或“肯定是 timing”作结。
- [ ] 无需 native 就能复现 QA defect 时进入 Task 2；若定位为产品改动，整理具体证据、建议最小 diff 范围和影响，请求范围批准，先完成独立 QA 检查。

### Task 2 — Pin the actual boundary with RED/GREEN / 用真实实现约束判据

**Files:** `native_project_native_io_test.mjs`、`native_project_driver_test.mjs`、`monitor_contract.mjs`、`native_project_evidence.mjs`，以及证据支持的 QA monitor/runner 文件。

**Existing interfaces / 保持接口：**

- `waitForPanelMount(inspect, project, { deadline, timeoutMs = 10_000, intervalMs = 100, diagnostics } = {})`
- `navigateNativeDocument({ monitor, command, action, deadline, phase, observe = () => {} })`
- `waitReloadNativeTrace(trace, afterSeq, options = {})`；reload 使用 accepted save cursor，不能回到默认 0。

- [ ] 针对 Task 1 证实的具体错误写 RED 并运行确认；不要以 missing export/syntax error 作为唯一行为回归证据。
- [ ] 验证真实 monitor JS 与生产 marker 一致；fixture 不得自己定义 selector 然后证明同一 selector 存在。需要产品组件测试时先读适用本地测试指南并取得产品修改范围批准。
- [ ] 若源码不足以定位，允许最小固定只读诊断：例如 exact literal path 分类为 `projects|project_a|project_b|other`，只能从已允许路径比较派生；不返回 pathname 原文。是否添加由 Task 1 证据决定，不预造产品状态或 marker。
- [ ] 新诊断同步严格 wire validator、实际 JS driver 测试、evidence/report schema。测试合法/非法字段、非布尔值、未 ready→ready、永久未 ready 的 deadline、旧 document 结果不能满足新 document。保持 10 秒 mount 上限；不靠加长预算掩盖失败。
- [ ] 最小 GREEN；验证 `expectReload()` 在 command 前、新 hello 验证后才 inspect、固定导航拒绝 caller `route`/URL/selector、无 IPC/秘密泄露。未知错误仍落 `internal_unclassified`，保留安全旧 reason。
- [ ] 若没有可证明 QA defect，交付诊断改动与明确未决假设即可；不得为了完成计划强行改产品。对 failure phase/operation/lastBoundary 不一致，只在确定性测试说明问题时修报告逻辑，保留原 run 历史。

### Task 3 — Verify, rebuild and independently audit / 验证与构建

- [ ] 从仓库根运行 `node --test qa/native_ipc/*_test.mjs`；Gateway smoke 必须从根运行，避免相对 `target/debug` 假 ENOENT。
- [ ] 运行 `python qa/browser_live/check_docs.py --self-test`、`python qa/browser_live/check_docs.py`、`git diff --check`；记录实际 totals，不沿用 95/95。
- [ ] 按改动范围选择 Cargo 检查，全部前置 memory gate、单 job。QA shell 源改变时做针对格式检查和 QA build；Panel 产品测试只有批准后才改，验证用 `cargo test --lib --no-default-features --features panel-modular` / `cargo check --lib --no-default-features --features panel-modular`。
- [ ] 当前历史 artifacts 对应旧 HEAD，按实际 revision 重建 Panel：在 `interfaces/webchat` 运行 `bun run panel:tabs:build -- --clean --output=.panel-live/shell`；审计 host/terminal JS/WASM、HTML/CSS、loader、manifest 与 descriptor entry。
- [ ] memory gate + `CARGO_BUILD_JOBS=1` 构建 Gateway：根 `cargo build --locked --features native-qa --bin aleph-panel-native-qa-gateway`；reaper：根 `cargo build --locked -p aleph-client --example qa_credential_reaper --features qa-service`。
- [ ] shell：从根执行 memory gate，再在 `desktop/shell` 设置 QA `TAURI_CONFIG`（读取 `tauri.qa.conf.json` 内容，不仅路径）与 `CARGO_BUILD_JOBS=1`，运行 `cargo build --locked --no-default-features --features qa-owned-root`。构建环境设置必须显式，不启动 shell。
- [ ] 检查 `build.json.revision === git rev-parse HEAD`、实际 shell 加载的是 staged `.panel-live/shell`、bundle feature 和所有所需 assets。源码改动未提交时记录变更范围/摘要与 build 关联，不能只凭 HEAD 声称源码完全一致。
- [ ] 仅在 `git check-ignore` 确认 exact stale manifest 为 ignored generated file 后删除它；运行 `node qa/native_ipc/native_project_provenance.mjs --repo-root . --output interfaces/webchat/.panel-live/native-provenance.json` exclusive 生成；独立通过导出 `inspectNativeProvenance` 检查（先读签名；CLI 没有 `--inspect`）。不能仅替换 revision 或覆盖旧 manifest。

历史限制仅供定位：根 `bun run check` 曾报 `error: Script not found "check"`；workspace fmt 有基线错误；`--all-features` 会触发 `feature qa-owned-root must not be combined with embedded-core`。新会话先看现状，按范围验证，不重复无关全量检查，不修任务外 baseline，不全局绕过 hooks；未执行的检查必须注明。

### Task 4 — Reviewable preflight and separate authorization / 预检与独立授权

- [ ] 在 ignored plan-scoped ledger 保存构建、独立 provenance audit 和源码检查结果；只记录允许字段。
- [ ] 执行 `node qa/native_ipc/native_project_runner.mjs`（无 `--execute`），保存 JSON 和原 exit code；4 是 `NOT_RUN` mapping，不能被管道吞掉后报 exit 0。
- [ ] 必须 `NOT_RUN/preflight_only`、`webviewLaunches:0`、`launchGate:new_explicit_budget_required`、`artifacts.ok:true`，exact six cleanupSequence 不变，无 native evidence 冒领。artifact 失败先修，禁止 launch。
- [ ] 展示具体 diff、测试摘要、仍未决假设以及下一次窗口要区分的边界；明确请求一次新的 120 秒授权。**得到明确答复前不得 `--execute`。** 历史“继续授权”及本计划不是新的授权。

### Task 5 — One authorized run and accurate handoff / 单窗执行与交付

- [ ] 新授权后仅运行一次 `node qa/native_ipc/native_project_runner.mjs --execute`。不同时运行 ad-hoc browser/helper/reaper；到 120 秒 business deadline 按 runner cleanup 退出，禁止自动重试。
- [ ] 依次保留 pairing save、post-save cursor 的 reload load、Global、A select/discovery、authenticated route/mount、A asset、B select/discovery/asset 的独立证据。某层失败不复用前一 phase 的证据。
- [ ] 到达 late-A/endpoint/lease/revocation 时先核对当前 runner 能力；无真实驱动/观测能力必须固定 BLOCKED，不能用多 document navigation 或 cleanup revoke 代替业务效果。
- [ ] 无论结果均记录 run ID、phases、accepted helperMessages/receipts/nativeTrace、primaryFailure、lastBoundary、failureReason、missingLayers、固定 diagnostics/cursors 与六项 cleanup。
- [ ] 更新 ignored progress、`docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md`、`docs/reference/FEATURE_LOCATOR.md` §6.11.2，保留历史 run 不改写；新增说明中英双语。
- [ ] docs self-test/normal check、`git diff --check`；中文最终交付，列未做、未证明、未跑检查、范围/授权需求。一次窗口阻塞后先分析报告，不能直接再次索要窗口重跑。

## 6. Completion / 完成判据

本计划的阶段性交付可以是：证据支持的 QA 修复 + 通过的非 native 验证 + 当前 HEAD preflight，等待授权；或一次授权 run 的精确结果/明确产品范围阻塞。**整体 A/B native 验收只有各独立 required layers 都具备证据才完成。** 不以 generic `native_run_failed` 或单纯 `missingLayers:[]` 宣称完成。

No implementation or launch is performed merely by writing this plan. 本交接本身未实施上述后续任务、未构建新 artifacts、未启动新窗口。
