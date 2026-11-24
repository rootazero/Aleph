# Next Session Prompt / 下一会话启动 Prompt

Copy the block below into the next session. This starts a new native acceptance phase; it does not reopen the completed project-context implementation. / 将下方正文用于新会话；上一阶段已完成，不要从头重做。

---

继续 Aleph Panel 社区 UI 的下一阶段：**真实 Windows Tauri/WebView2 宿主的 authenticated project context 验收**。先按 brainstorming 进行代码审计与 native 设计，设计批准后再实施；不要把本交接计划当成已经批准的新 credential/QA 架构。

先运行 `git rev-parse --show-toplevel`、`git status --short` 和 `git log -3 --oneline` 确认当前检出。仓库路径不要写死；根据本机实际位置操作。请用当前本地 `main` 创建新的短分支和隔离 worktree，不要按无关仓库默认值改用 `origin/dev`。

## 已完成并合并的起点

- authenticated project context 阶段已完成，原任务分支 `panel-trusted-context` 的六个提交已 fast-forward 合入本地 `main`。
- 产品实现与验收提交：`904e6dc99351590871ba824791c8233cc681f546`；前一基线：`52e13cb3db8cf27a2a9bede773cbcc8742cb28e5`。main 后面可能还有本交接文档提交，以实际 HEAD 为准。
- 合并没有冲突、没有改写历史，也没有 push。旧 worktree/分支仍保留；不要擅自删除其中的 ignored evidence 或构建产物。
- 已实现：一个 Gateway-owned、按 authenticated device 保存的 selector；`panel.project.select` 只接受 nullable project ID，每次通过当前 user/device 与 ProjectStore 重验；RPC discovery 与 HTTP asset 共用；Core 仍唯一拥有声明、bytes、trust、scope、source revision。
- 无 claim 保持 Global；有效项目得到 Global + 选中项目；显式无效、跨用户、撤销、归档或无 workspace 的 claim 拒绝，绝不回退 Global。archive 不自动 clear，恢复 Global 需显式操作。
- 浏览器显式项目切换、同 Core revision 的过滤视图、late-result/asset fencing、跨 tab 下一边界观察已实现。snapshot 到达不 fetch/import/mount。

## 先读

1. `docs/superpowers/plans/2026-11-01-panel-native-project-context-plan.md`：本阶段建议计划与审计入口。
2. `docs/superpowers/runs/2026-11-01-panel-trusted-project-context.md`。
3. `docs/reports/panel-community-live-publication-validation.md`。
4. `docs/superpowers/specs/2026-11-01-panel-trusted-project-context-design.md` 与 `docs/superpowers/plans/2026-11-01-panel-trusted-project-context-implementation-plan.md`：已完成方案与 rulings，不重新申请上一阶段批准。
5. `docs/reference/FEATURE_LOCATOR.md` §6.11、`TERMINAL_RUNTIME.md`、`DESKTOP_SHELL.md`、`PRODUCT_TOPOLOGY.md`、`SECURITY.md`、`DEVELOPMENT.md`，以及所动目录的本地宪法。
6. `qa/native_ipc/README.md`、`run-real.mjs`、`real_run_core.mjs`、`real_run_io.mjs`、`panel_driver.mjs`、`monitor_contract.mjs`、`real_qa_prep.mjs`；实际源码优先于历史说明。

## 已有证据与真实缺口

上一阶段实际通过：selector 7；真实 WS+HTTP project integration 1；production HTTP 2；method census 4；protocol 9；native Rust Panel/Terminal regression 1607；fresh production-feature WASM/glue JS loader 15（0 skip）；真实 Gateway fixture 1，Chrome driver exit 0（12 snapshots / 13 observed assets）。14 个 browser source hashes 与 WASM hash 匹配。

这些证明 **不等于真实 native WebView 验收**。旧 `qa/native_ipc/run.mjs` 的 fake Gateway / memory backend 只能证明相应 IPC。上一阶段全分支 reviewer 有完整书面意见且无 confirmed finding，但进程 SIGTERM；不要改称 clean review。完整 workspace suite、monolithic Core lib-test、native WebView、部分历史 Terminal/spike 真机回归均未补跑。

已保留的脱敏本地证据见 `.superpowers/sdd/2026-11-01-panel-trusted-project-context-implementation-plan/browser-evidence/`；这是 ignored evidence，不含连接凭据。不要为了恢复报告去打印 `.panel-live/control/connection.json`。

## 本阶段目标

用真实 Windows Tauri/WebView2、真实 native credential IPC/lease 与 OS protected backend，接入真实 production-library Gateway，验证同一经过认证的项目上下文抵达 discovery 与 HTTP bytes。覆盖真实 native reload 的同 device 身份、显式 A/B/Global、跨用户拒绝、同 Core revision 切换、延迟 A 不覆盖/清掉 B、endpoint/lease 变化，以及项目/设备撤销后下一边界 fail closed。

仅修复该真实路径暴露的问题。process-local selection、Global + selected project、下一边界才观察跨 tab/撤销、无即时 idle teardown 是既定边界，不擅自扩成 durable selection、notification 或后台刷新项目。

## 已知 native QA 约束，先审计后决定

- `desktop/shell` 的 `qa-owned-root` 是 debug lite QA 形态，需 `--no-default-features`，不能与默认 `embedded-core` 混用；配置 `desktop/shell/tauri.qa.conf.json`。
- `--qa-credential-backend os` 与真实 protected store 才能形成相关证据；memory backend/self-test 不能升级为 protected PASS。
- `real_qa_prep.mjs` 普通模式当前总是 NOT_RUN；不是删掉常量就算真机已就绪。
- `real_run_core.mjs::validateRealArgs` 当前只接收 `ws://127.0.0.1:18790/ws`，而产品 native endpoint guard 另有实际规则。必须设计本次拥有的 disposable Gateway origin，不能为满足旧 runner 去连接、停止或替换日常 daemon。
- 复用已有 root/process/reaper/monitor helper 与 exact `(service, account)` cleanup，不另造宽松 runner，不枚举删除其它凭据，不按进程名 kill。
- 历史 run-window 预算或许可不是本次自动 launch 许可。把本次 endpoint、窗口预算、owned PID/root、真实 pairing、trace/DOM evidence 与 exact cleanup 写进待批准 native 设计。
- 不覆盖 `__TAURI_INTERNALS__`、不伪造 native marker、不给 browser storage 注入 token 冒充 native proof，不增加弱化的测试产品端点。

## 不可越过的产品边界

- 不增加第二套 Panel registry/bytes/trust/revision/project authority。
- claimed project 必须绑定当前 authenticated user/device；禁止 `authz::project_for(..., actor=None)`。
- 不从 CWD、root、URL/query/header 或 client boolean 推断项目；Panel context wire/browser 不暴露 root。
- 保持既有 bearer、origin/TLS/proxy、wall、私有限流、Terminal `ControlPlaneAssets`、generic refusal、exact `private, no-store` 和 passive discovery。
- 不做 install/update/uninstall、marketplace/signatures、default-release switching、notifications/background refresh、multi-file assets、external WASM 或 Zahir runtime mount。
- 不触及已安装的日常服务、默认 release 或非本次 QA 拥有的凭据、进程和目录。

## 工作流与验收

1. 先 brainstorming 审计上述 native/credential/project 真实链路，提出 2–3 个可行的窄 QA 方案并推荐一个；如果现有 API 不能提供可信 actor-bound native proof，交付审计 blocker，不做弱替代。
2. 写双语 native design，fresh builder check，取得该具体设计的明确批准；然后 writing-plans 细化本交接计划，再 executing-plans，一次一任务。
3. 代码前 meaningful RED；production/QA wiring；GREEN；ledger；conventional commit。复用现有 production-library fixture，不造假 RPC/snapshot/HTTP bytes。
4. 真实受控 WebView launch 前校验 owned endpoint/root、shell/reaper/source/WASM hashes、exact cleanup 和批准的窗口预算。缺条件写 NOT_RUN/BLOCKED，不写 PASS。
5. 最后 fresh read-only whole-branch review；披露 timeout/SIGTERM、未执行项和验证范围。保留隔离分支并 tracked-clean 交付，**不 merge、不 push**，除非我在新会话另行明确要求。

每次 Cargo 前从 worktree root 运行 `python qa/browser_live/check_memory.py`；固定 `CARGO_BUILD_JOBS=1`，Cargo 串行，统一 profile/target。不要用 `bash -lc` 改变 Cargo 工具链环境。monolithic Core lib-test 已有 LLVM OOM，优先 independent production-library target，不重复同样失败配置或增加 public test shim。Terminal HTTP public-assets test 会写入/清理 build.rs 监视的 `dist/`，应编译一次后运行对应新 executable，或把它放最后，避免无意义完整重编。

最终验收必须能区分 pure harness self-test、memory IPC、protected native persistence、真实 community asset load 与实际 desktop runtime；只有真实链路、隔离/撤销反例、provenance 和 cleanup 均有证据才算 native 阶段完成。英文代码注释，双语文档，最后明确未做与未验证项。
