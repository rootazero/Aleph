# Panel Native Runtime Completion Design

日期 / Date: 2026-11-02

## 意图 / Intent

本阶段继续 Aleph Panel authenticated project context 的 native 验收。目标是让 runner 的证据和 cleanup 结论可信，并准确记录普通 fresh Panel 在当前产品路由下为什么没有 pairing surface。只有在 preflight 结果可审阅且获得新的明确 WebView2 launch budget 后，才允许进行一次受控 native launch。

The goal is to make the native QA runner's evidence and cleanup outcomes trustworthy, while recording precisely why a normal fresh Panel does not expose a pairing surface under the current product routing. A WebView2 launch remains gated by a new explicit budget approval after the preflight report is reviewable.

## 目标 / Goals

- 修复 helper stop 生命周期，使异步 `stopped` facts 在等待完成后被读取并进入 cleanup 判定。
- 使 `waitReceipt` 严格要求 `seq > cursor`，避免 stale receipt 满足 predicate 或污染 ledger cursor。
- 让 native run 使用 prelaunch `interfaces/webchat/.panel-live/build.json` 中的独立 artifact manifest 作为期望值；执行阶段只比对当前产物，不用执行时即时 hash 生成自己的 provenance 期望值。
- 使用同一个 native run deadline 限制 monitor、trace、receipt、cleanup 相关等待；超时必须形成可复现的 blocked/ambiguous 结论。
- 保持六步 exact cleanup 的顺序和 fail-closed 行为；cleanup 失败后不得重复调用 helper stop 改变已记录事实。
- 补充 Node 回归测试，覆盖 stopped facts、stale cursor、manifest provenance 和 cleanup phase。
- 将 pairing surface 诊断写入双语 run ledger 与适用的 `FEATURE_LOCATOR.md` 条目，不把源码支持路径升级为 native WebView2 proof。

## 非目标 / Non-goals

- 不为 QA 增加隐藏 pairing UI、direct invoke、browser storage credential、synthetic DOM、fake actor、`actor=None` 或测试 endpoint。
- 不改变 Panel 的 auth semantics、TokenWall 条件、Browser viewer 路由或产品 UI。
- 不替代现有 `projects.rs` → `DashboardState::select_panel_project` → `PANEL_PROJECT_SELECT_METHOD` 产品路径。
- 不在本阶段解决 ConnectionSection 无法由 bounded DOM driver 驱动 endpoint/lease transition 的产品能力缺口；没有现有 bounded path 时保持 `BLOCKED`。
- 不启动 WebView2、写 OS credential、连接/停止/替换日常 daemon 或读取旧 token-bearing connection file，直到用户给出新的明确 launch budget。
- 不 merge 或 push。

## 诊断结论 / Diagnosis

`PairingNotice` 只在 `interfaces/webchat/src/platform/wide/views/browser/viewer.rs` 的 Browser viewer `LaunchSection` 中渲染。它依赖用户已经进入 workspace browser slot/resource，且当前 `ViewerCaller` 未配对；普通 fresh Panel 根路由没有该 slot，因此缺少 `PairingNotice` 是既有路由设计。

`TokenWall` 由 `interfaces/webchat/src/app.rs` 挂载，但只在 `needs_token` 或 native welcome 条件满足时可见。`needs_token` 是 Gateway connect 的真实 remote auth verdict；loopback credentialless authorized connection 会进入 authorized 状态而不显示 login wall。native welcome 还要求 native marker、已连接、未 protected、未 acknowledged 且是 local endpoint。Shell 注入 native marker 是静态支持路径，不能替代真实 WebView2 observation。

因此此前把普通 fresh Panel 缺少两个 DOM surface 直接归因于 pairing 缺失是 RED；源码和路由测试确认现有产品支持路径后，结论应为 GREEN：普通 Panel 没有 Browser pairing surface，TokenWall 是否出现必须由真实 connection state 决定，QA runner 不能自行制造入口。

## 方案 / Approaches

1. **手术式修复现有契约（选定 / Selected）**：保留 helper 协议、receipt schema 和六步 cleanup；修复 stop 快照读取、receipt cursor、共享 deadline、独立 manifest provenance 和 cleanup 的重复 stop；增加 focused Node tests。它保持现有证据边界和产品边界，改动最小。
2. **扩展 helper 协议**：新增显式 shutdown stopped-facts 返回和 receipt batch/cursor 语义。它可以扩大协议表达力，但会增加 helper、协议和测试面，不能改变普通 Panel 没有 Browser pairing route 的事实。
3. **重做 pairing 流程**：为 QA 增加专用页面或直接 IPC/浏览器注入。它会绕过真实产品路径，违反本阶段禁止新增入口和伪造 native proof 的约束，因此排除。

## 决策记录 / Decision record

Builder check 后确认以下三项：

- **Manifest 来源**：由独立的 QA preflight preparation 生成，不扩展 Panel 构建脚本作为本阶段的权威源。preflight 读取已构建的 shell/helper/reaper/WASM、Tauri QA config、Panel build/source manifest 和当前 repository revision，生成只包含必要摘要的 native provenance manifest；native runner 只读取该 manifest 并比较即将启动的 bytes。该文件记录 artifact digest、WASM/source manifest digest、Tauri config digest 和 revision，不包含 credential 或不必要路径文本。它不是语义 freshness 证明，只证明记录的 bytes 与启动 bytes 相同。
- **Manifest 失败结果**：缺失、损坏或 mismatch 返回结构化 `NOT_RUN`，记录 preflight phase 和具体原因；不创建 owned root/helper，不生成 cleanup PASS，也不让 CLI 依赖异常转换来推断结果。
- **Deadline 与 cleanup**：deadline 到期后停止新的 native 业务步骤，但 exact cleanup 继续到安全边界。cleanup 使用其自身每步的有界等待并记录“业务预算已耗尽”；只要任一步安全证明失败，结果保持 `AMBIGUOUS`，绝不移除未证明的 root。

这些决定已由用户确认；builder 可据此规划实现，不再需要额外的产品或架构选择。

- 变更集中在 `qa/native_ipc`、run ledger 和 `docs/reference/FEATURE_LOCATOR.md`；若源码诊断只需证据，不改 Panel/desktop 产品代码。
- 每次 Cargo 命令前从仓库根目录运行 `python qa/browser_live/check_memory.py`，固定 `CARGO_BUILD_JOBS=1`，不使用 `bash -lc` 改环境。
- 证据分层保持独立：native Rust trace、Gateway receipts、DOM monitor、artifact/process provenance、exact cleanup。
- `BLOCKED`、`NOT_RUN` 或 cleanup `AMBIGUOUS` 不得成为 PASS；当前即时 hash 不能作为独立 freshness proof。

## 实现留给构建者 / Remaining work for the builder

- 在 `qa/native_ipc/native_project_native_io.mjs` 修复 helper stop 的等待后读取，封装或调整 deadline 检查，并让 receipt 查询在 stale `seq <= cursor` 时继续等待/拒绝。
- 在 native run 开始时读取并验证 prelaunch build manifest；使用 manifest 中的 shell/helper/reaper/WASM 摘要作为独立期望值，禁止执行阶段通过当前文件即时 hash 自我固定期望值。
- 确保 cleanup 只由 exact cleanup state machine 驱动；成功/失败后不再执行会改变 cleanup 事实的隐式 helper stop。
- 在现有 Node test files 中增加必要的真实行为测试，优先测试实际函数，不复制实现。
- 先运行只读 Node/docs/diff 验证，生成 reviewable preflight report；只有用户批准新的 launch budget 后，才运行受控 native path。
- 若获得批准，按既有产品路径验证 protected save/load/delete、same-device reload、Global/A/B 和 revocation next-boundary；无法由现有 bounded path 证明的 endpoint/lease transition 保持 BLOCKED。
- 最后更新双语 run ledger、`FEATURE_LOCATOR.md`，运行完整 Node/docs/Cargo/rustfmt/diff 验证，提交并发起 fresh read-only whole-branch review；任何 timeout/SIGTERM 必须披露。

## 验收标准 / Acceptance criteria

- Node regression tests demonstrate stopped facts are read after the shutdown wait and stale receipts cannot satisfy a cursor predicate.
- Provenance validation fails when the prelaunch manifest is missing, malformed, or mismatched, and no execution-time self-pinning path remains.
- Cleanup phase records the exact step boundary and cannot be upgraded after a failed proof or a second implicit stop.
- Pairing diagnosis is source-backed and says ordinary fresh Panel pairing is unavailable by route design; no product surface or auth behavior is changed.
- Default runner remains read-only; no WebView2 launch occurs before a new explicit budget.
