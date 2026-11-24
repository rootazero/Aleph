# Restart Prompt / 新会话启动 Prompt

复制下面代码块到同一 Aleph 仓库的新会话。仓库位置以新会话 `git rev-parse --show-toplevel` 为准；本文不授予 native launch。

```text
继续处理同一个 Aleph 仓库的 Panel authenticated project-context native acceptance。请先完整阅读并执行：

docs/superpowers/plans/2026-11-03-panel-native-route-mount-investigation.md

本次任务：定位 authenticated project A route 上 Panel surface 未出现的真实边界；证实 QA defect 后做最小 RED/GREEN 修复，完成当前 HEAD artifacts/provenance 和可审阅 preflight。不要直接重跑 native。

执行方式：主 agent 直接执行，不使用子代理，不启动 CLI 模型。先读 AGENTS.md 和相关子目录指令；保持同仓库 main，不 reset/rewrite 历史，不丢弃既有改动，不自行 commit/push/merge。使用 executing-plans 按任务推进，但用户的直接执行/禁止委派约束优先。

最新事实：
1. 交接编写时 git status --short 干净，main HEAD=f9fa908b1c215ea93401abbb6e42d107c7f350e8。新会话重新检查；旧会话“14 个未提交文件”描述已过期，不要据此回退。
2. 历史 artifact/provenance revision=063345f2f30bf999b28ee5d84aac860be8432fa7，不能当作当前 HEAD preflight。检查实际 revision/digests，再决定重建；禁止只给旧 manifest 改 revision。
3. 最新 native run=dd6f874403829221f12f09f277fe008dd30984091f21dd9b990c777bdd48a0a5，exit 5，BLOCKED/panel_route_unavailable。
4. helper_ready、protected_pairing、protected_reload、global_discovery PASS。accepted native save seq3(ok/null) 与 load seq4(ok/lease_object/credentialPresent:true)；accepted helperMessages/receipts 共六条，覆盖 Global connect/discovery 两组 seq1–4、A select/discovery seq5–6；第三个 monitor document hello 认证通过。
5. A route bounded inspect={panelShell:false,panelTabA:false,panelTabB:false,panelContent:false}；B 未到达；未开始等待 asset receipt。primaryFailure.phase=project_a，operation=monitor_inspect；原报告 lastBoundary=monitor_command，failureReason=panel_route_unavailable，missingLayers=[]。保留原事实，不把 missingLayers 空数组当成验收完成。
6. monitor upgrades/accepted/helloSeen=3/3/3，无 origin/channel/concurrent/stream/install failure；trace={records:4,unconsumed:2,ignoredLines:5,poisoned:null}；helper={sent:11,acknowledgements:1,timeouts:0,closed:0,invalid:0}；receipt={requests:7,retries:1,notFound:1,stale:0,invalid:0,cursorBefore:5,cursorAfter:6}。
7. 六项 cleanup 全 true；历史完整 Node suite=95/95，docs checks/diff 通过。这些不是新会话的测试结果。
8. A/B asset、B surface、late-A UI fencing、endpoint/lease、业务 revocation 都未证明；Panel 为何未挂载尚无根因证据。
9. 既有修复已在当前代码：reload expectReload-before-command；post-save seq cursor 的 waitReloadNativeTrace；project control bounded polling；固定 navigation 与四个只读 DOM boolean。不要重做 selector 或这些已完成工作。

先调查，后选择最小修复：
- 完整读新计划，以及 docs/superpowers/runs/2026-11-01-panel-native-project-context-acceptance.md 最新小节、FEATURE_LOCATOR.md §6.11.2。
- 对照 desktop/shell/src/main.rs 实际 monitor selector 与 interfaces/webchat/src/panel/shell.rs 实际 data-panel-shell/data-panel-content-root/data-panel-plugin 渲染行，排除“量具错误”。
- 核查当前 shell 真实加载的 staged bundle、panel-modular feature、host/terminal manifest/revision、固定 navigation 的实际 path、app.rs 的 MainContent/explicit_community_panel、form factor、discovery readiness、descriptor key/status、reconnect 与 loader activation。
- Gateway project selection 按 authenticated device/user 保存；新 document 的 JS state 生命周期与 Gateway selection 是不同层。不得猜测 selection reset/timing/产品缺陷已获证明。
- 若需新增诊断，只允许固定 enum/boolean/bounded counters；比如字面量 path 比较后输出闭集 route kind，绝不输出 raw pathname/URL、page text、错误文本或身份值。测试读取实际 monitor JS，避免 mock 自己证明自己。
- product Panel/Gateway/credential/auth 改动超出当前范围；若确需此类修改，先给出具体证据、建议最小改动与影响并请求范围/架构批准，同时继续完成可独立进行的 QA 工作。

硬约束：
- 允许 native QA/test/docs 与已批准的 desktop/shell/src/main.rs QA monitor 固定导航/诊断；不改产品认证、Gateway API、credential semantics，不新增产品 wire/RPC。
- 保持真实 Projects/Global → DashboardState::select_panel_project → PANEL_PROJECT_SELECT_METHOD → authenticated Gateway。
- 固定路径仅 /projects、/panel/project-a/overview、/panel/project-b/overview；wire 不接受 route/URL/selector 参数。每次 document transition expectReload 在 command 前，且接受 authenticated 新 hello 后才 inspect。
- credential effect 只认 owned shell authenticated ALEPH_QA_TRACE；保留 OS backend，不切 memory。DOM/helper/receipt/HTTP/provenance/cleanup 各自标注，不能相互升级。
- 不新增 pairing UI、hidden selector、direct Tauri invoke、browser credential、fake actor、actor=None、test endpoint；不连接/停止/替换日常 daemon，不碰 18790，不读旧 token connection file。
- 不记录 raw URL/Origin/port/runtime path/ticket/token/credential/request ID/device ID/lease ID/raw OS error。未知错误 fixed internal_unclassified；安全旧 reason 保兼容。
- 每个 Cargo 命令前从仓库根 python qa/browser_live/check_memory.py，通过后 CARGO_BUILD_JOBS=1；不 bash -lc。不要修无关 baseline。
- exact cleanup 固定 root-last：shell_stopped → helper_stopped → exact_devices_revoked → credential_deleted_exact → credential_absent_exact → owned_root_removed。exact(service,account) reaper，无 wildcard，无按名/端口 kill。cleanup 不覆盖 primaryFailure。

验证与 launch gate：
1. 每个实质修复先写能复现行为的 RED，再最小 GREEN；保留闭集 schema、auth/document/cursor/deadline/秘密拒绝测试。
2. 仓库根 node --test qa/native_ipc/*_test.mjs；python qa/browser_live/check_docs.py --self-test；python qa/browser_live/check_docs.py；git diff --check。Gateway smoke 不从 qa/native_ipc 子目录运行。
3. 按当前 HEAD 与源码变化重建 Panel/QA shell/Gateway/reaper；命令/QA TAURI_CONFIG 方法见计划。Panel 输出 .panel-live/shell，校验 host/terminal assets/descriptor/revision/features；fresh exclusive provenance 独立 inspect，旧 ignored manifest 仅在确认后删除。
4. node qa/native_ipc/native_project_runner.mjs（不带 --execute）必须 NOT_RUN/preflight_only、webviewLaunches=0、launchGate=new_explicit_budget_required、artifacts.ok=true、exact cleanupSequence 不变；正确保留 exit 4 mapping。
5. 输出可审阅 diff、真实验证结果、preflight 和下一窗口要判别的边界，然后请求一次新的明确 120 秒授权。当前所有历史授权均已消费；本 prompt 不是 launch 授权。未获新明确答复不得 --execute，不启动任何 ad-hoc browser/helper/reaper。
6. 得到新授权后仅一次 node qa/native_ipc/native_project_runner.mjs --execute，结束立即停止，无自动 retry/第二窗口。记录精确 phase/failure/diagnostics/accepted evidence/cleanup；不能用多 document navigation 证明同 document late-A UI fencing，不能用 cleanup revoke 证明业务 revocation。

把进展追加到新计划目录对应的 ignored .superpowers/sdd/2026-11-03-panel-native-route-mount-investigation/progress.md；实际结果更新旧 run ledger 和 FEATURE_LOCATOR §6.11.2，保留历史记录。最终中文说明完成内容、命令结果、run（若获授权才存在）、具体 blocker、未证明层、未做/未跑检查及下一步需要的范围或 launch 授权。不要以 generic native_run_failed 收尾。
```
