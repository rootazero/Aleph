# Phase10 closeout / Phase10 收尾

**Status: completed for the approved local macOS slice, under the explicitly amended Quit contract, with physical multi-display topology user-deferred and NOT TESTED. / 状态：按明确修订的退出合同完成本机 macOS 范围；用户暂缓的多屏物理拓扑仍未测试。** This is not a pass under the original performance limits and not release or Tauri-removal approval.

## 1. Decision and candidate / 决定与候选

- Official registry NSH `0.6.0 (5541637)`; the old 0.5 fork is historical, not a fallback. One NSH root hosts the existing Leptos/WASM Panel. No native business UI or second Gateway client was introduced. / 仅一个原生宿主，业务仍在共享 Panel。
- The operator explicitly approved Quit median ≤ **1.30×**, then separately approved Quit p95 ≤ **1.30×** on 2026-11-21. All other gates retain the original thresholds. See the [decision](2026-11-21-phase10-acceptance-amendment.md) and [machine assessment](2026-11-21-phase10-acceptance-amendment.json). This accepts a performance trade-off; no Quit optimization is claimed. / 此为性能取舍，不是提速。
- Working HEAD: `0e4e1d4ebaa6e6dee8d6af9928f969d2e90f42b7`, with the existing Phase10 working-tree implementation. This is a source/build manifest acceptance, **not a claim that Phase10 is committed at HEAD**. / 验收对象包含现有未提交改动，不能把 HEAD 当作已提交成果。
- Final NSH executable: `fed640635c1bf6697e76af723c075935169bb58c3b96e752a8495483df2077cc`; Gateway: `1902bf9b94318b9a67fa5151c5043ddec7dab0cdf996aa0e8dc607bf5f891417`; lock: `555ccd6f1a0a0da1904ebf9c0b05974fbccce81dd1643912cdaf2fd5f3d0a448`. Complete identities and historical per-run differences are in the [candidate index](2026-11-20-phase10-official-candidate-integrity.json). / 各原生回执仍保留自己的构建身份，不把较早回执说成本轮重跑。

## 2. Gate disposition / 逐项处置

The [native ledger](2026-11-20-phase10-official-nsh-checkpoint.md) retains earlier rejected attempts and exact scope. The closeout rechecks evidence and the unchanged retained candidate; it does not pretend all earlier OS cases were freshly rerun today. / 本次核验现有回执与保留候选，不声称今日重新执行所有系统交互。

| Gate | Disposition / 处置 | Evidence / 证据 |
|---|---|---|
| G1 | PASS: same resident window close/reopen, hidden Quit, observed settlement; resized case at 720×620. / 同窗常驻与隐藏退出。 | [current](../../../.phase10-evidence/g1-official-current-e3w84dqy/receipt.json), [resized](../../../.phase10-evidence/g1-official-resized-6_7rpfra/receipt.json) |
| G2 | PASS within normal packaged LaunchServices activation and native tray Show/Quit; no forced-process singleton claim. / 不外推到强制新实例。 | [tray](../../../.phase10-evidence/g2-tray-d6pwnub_/receipt.json), G1 and G4 |
| G3 | PASS: summon/focus toggle, disabled registration, conflict and release/re-registration. / 禁用的是召回热键。 | [disabled](../../../.phase10-evidence/g3-hotkey-fl_jsz7n/receipt.json), [conflict](../../../.phase10-evidence/g3-official-conflict-6dkacdqz/receipt.json) |
| G4 | PASS: validated cold/warm routes, malformed rejection, reload binding, trusted top-frame origin, temporary default dispatch and restoration. / 默认处理器测试后恢复。 | [current routes](../../../.phase10-evidence/g4-official-panel-zll9j6qx/receipt.json), [reload](../../../.phase10-evidence/g4-official-reload.T89qTsUR/receipt.json), [default](../../../.phase10-evidence/g4-default-dispatch-lmd3unzg/receipt.json) |
| G5 | PASS in dedicated signed QA identity: real Notification Center card action reopens/focuses original window 9140 and reaches Chat. Permission/admission alone is not PASS. / 实际点击与同窗路由合并核验。 | [joined click](../../../.phase10-evidence/g5-official-2wphghqq/click-verification.json) |
| G6 | PASS for normal-frame restore, off-screen clamp and corrupt-state fallback. **Physical multi-display transition remains NOT TESTED/user-deferred**, not a full topology PASS. / 不以延期冒充通过。 | [placement](../../../.phase10-evidence/g6-actual-ikue8zka/receipt.json) |
| G7 | ACCEPTED **under the amended contract**: complete retained 40+40, natural exit and observed settlement, original input/resource/regression gates pass; pre-existing managed Gateway survives. / 只在修订合同下通过。 | [amended assessment](2026-11-21-phase10-acceptance-amendment.json), [managed Gateway](../../../.phase10-evidence/g7-official-managed-9mt2xujl/receipt.json), [busy hidden Quit](../../../.phase10-evidence/g7-official-busy-oswcl4oj/receipt.json) |

The retained [Ji0duOcq matrix](../../../.phase10-evidence/g7-official-matrix.Ji0duOcq/matrix.json) remains **VALID / NOT_ACCEPTED under its original contract**, SHA-256 `fbf5f1272be769ae554ba53df065ef171badb6bbe942bb90e254168664e5f7c2`. The separately accepted assessment uses all of that block, not the reverted tray candidate, an optimized-profile diagnostic or selected samples. / 原失败回执不改，不拼接或挑选成绩。

| Metric | Tauri → NSH | Decision / 判定 |
|---|---|---|
| Quit median | 41.298 → 51.794 ms, 1.254153× | ≤ amended 1.30× |
| Quit p95 | 57.849 → 72.014 ms, 1.244862× | ≤ amended 1.30× |
| Input median / p95 | 29 → 30 ms / 30 → 32 ms | original gates pass |
| RSS median | 663.742 → 624.984 MiB, 0.941607× | original ≤1.10× |
| Threads median | 70 → 66, delta −4 | original delta ≤2 |

First-frame, route and reconnect also retain their original passing gates. Protocol remains 10 warmups + 30 measured per surface, Tauri then NSH, p95 rank 29 and real 100 ms resource sampling. Native close/engine drain/event-loop return/kernel exit/observed settlement remain distinct. / 未移动退出边界或改变统计方法。

## 3. Ownership and integration review / 归属与集成校对

`desktop/nsh-shell/src/main.rs::App` renders the actual `web_view(props.url)` and creates `Bridge::start` under one `Root`. Close hides the resident window; tray Quit uses the official native Quit path. `Bridge::start` owns the navigation worker, one coalesced URL/notification listener and a bounded notification queue. Official `spawn` is owner-bound; `Answer` future drop is not native operation cancellation. Native process-exit/settlement evidence, not a pretend cancellation API, substantiates this bounded app-lifetime teardown. / 根组件拥有工作；不把丢弃 Future 说成原生监听取消。

`ALEPH_SHELL_HOTKEY=disabled` bypasses registration. Notifications have OS permission controls and an explicit permission-request menu, **not a product feature-disable switch**; no claim of zero-resource notification disable or runtime stop/restart is made. Adding such a configurable lifecycle would require its own contract rather than treating denial as disable. / 不新增未定义的通知开关，也不把权限拒绝当作零资源停用。

The Panel owns navigation, session projections, streaming and business notification content. The shell does not change protected root configuration or assume ownership of the independent Gateway. Historical native receipts and source reviews retain their own build scopes. / 业务与平台 I/O 边界不变。

## 4. Verification and review / 验证与审查

- `python3 .phase10-evidence/phase10-amended-acceptance.py`: exact original-reducer replay, current source/binary audit, four amendment boundary checks, amended acceptance. / 重放而非重新计时。
- Existing read-only audit: 23 sources and three binaries match; 80 distinct functional PASS rounds, natural exits and observed settlements, 80 unforced Gateway fixture cleanups, 320 actual HTTP asset files match. New audit equals the retained restored-candidate audit. / 直接核验效果和产物。
- Direct array cross-check: exported metric arrays match all 80 source rounds; first 10 excluded per surface; independent median and `sorted(values)[28]` agree exactly with retained summaries. / 单独校对分位数。
- Candidate index hash-addressed evidence links are rechecked, not silently trusted. Original collector, reducer, driver, schema and tracked default dist remain unchanged. / 保留旧判据与资产。
- Existing implementation SPEC → distinct-model QUALITY and signed-bundle publication regression remain recorded in the [review ledger](2026-11-20-phase10-official-nsh-checkpoint.md). Closeout evidence review independently confirms the revised numerical contract; a source audit reconciles the disabled-hotkey/notification distinction. / 源码审查不替代计量与原生回执。
- Final closeout SPEC (`magpie/88apiclaude/claude-opus-5-5`) returned **SPEC PASS with scoped constraints**, followed by distinct-model QUALITY (`magpie/88apigpt/gpt-6-astra`) **QUALITY PASS, scoped**. The [SPEC summary](../../../.phase10-evidence/closeout-20261121/review-SPEC-summary.md) distinguishes the parent summary from the full session return; the [QUALITY return](../../../.phase10-evidence/closeout-20261121/review-QUALITY-retry.md) is retained in full. A stale-model launch and the first QUALITY timeout have no verdict and are not counted as passes. / 按序完成收尾审查，启动失败与超时不算通过。
- Scoped Markdown-link verification uses the existing classifier; counts, hashes and final checks are recorded in the [closeout integrity receipt](2026-11-21-phase10-closeout-integrity.json). External/protocol links were not fetched. The whole-doc-tree scanner hit an unrelated empty-target `IndexError`; verification was restricted to these ten touched documents without changing the scanner. / 本轮本地链接、哈希和校验结果另存回执；未声称全仓文档扫描通过。
- No new Rust/product changes were needed for this acceptance amendment, so no new Cargo build or native timing run was started. Earlier scoped test results are historical and are not recounted as new runs. / 本次不虚增测试数。

## 5. Master roadmap handback / 原总方案逐期回看

The [original master](../specs/2026-11-05-nsh-desktop-refactor-design.md) was a historical v0.4 draft. Its phases are not Phase10's task numbers. / 原总方案编号不等于本轮任务编号。

| Master phase | Current claim / 当前可声称范围 |
|---|---|
| 0 — Compatibility spike | Historical feasibility; Phase10 separately proves bounded official 0.6.0 APIs. Not a new universal toolkit parity certification. / 非全面兼容认证。 |
| 1 — Protocol transport | Existing integrated shared protocol/Gateway foundation is reused, not rebuilt or newly certified in full here. / 沿用既有基础。 |
| 2 — Native shell | This Phase10 local macOS slice is complete under the amended Quit gates and explicit display deferral. Updater, installer/default rollout and full daemon-supervisor parity remain outside scope. / 不是整期所有功能迁完。 |
| 3 — Core Chat UI | Existing shared Leptos/WASM Panel retained by explicit R2 choice A; native Chat replacement not implemented. / 非原生 Chat 重写。 |
| 4 — Full UI migration | Not completed by this slice; shared Panel remains the UI source. / 不宣称完整 UI 迁移。 |
| 5 — Plugin runtime | Deferred independent prerequisites; not implemented here. / 未做插件运行时。 |
| 6 — Remove Tauri | Not authorized or complete; keep Tauri comparator/default path. Full UI prerequisites and two-week dual run not fulfilled here. / 保留 Tauri。 |

## 6. Not done / 未做

No physical multi-display QA, Windows/Linux NSH acceptance, release build/notarization/installer/updater rollout, default-desktop switch, upstream patch/submission, ownership migration, Tauri removal, remote push or new commit. No claim that JS VM cleanup caused the latency: earlier researcher assertions about JS GC and 200–400 ms close animations were unsupported and are explicitly withdrawn in the amendment. / 未验证的内容不以推断填补，也不把本轮收尾当作发布许可。
