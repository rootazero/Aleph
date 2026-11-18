# Phase10 NSH shell restart prompt / Phase10 NSH 桌面壳重启提示词

> Copy the prompt below into a new session opened at this repository's **local `main`**. It carries the next task; the linked plan contains the full checklist. / 在本仓库本地 `main` 重启会话后，复制以下提示词即可。

---

请从当前仓库的本地 **main** 继续 Aleph 的 Phase10。先确认真实 git root、branch、HEAD、工作区和 worktree 状态，不要 reset 到历史 SHA，不要 pull，不要转而从 `origin/dev` 起步，也不要删除原 `runtime-proof` 分支、旧 worktree 或证据。上一个会话已按用户要求将 Phase9 的已验收实现和本提示词/计划整合到本地 main；用当前 Git 状态确认这一点，不能仅相信文字。

本次任务：按 [Phase10 实施计划](../plans/2026-11-18-phase10-nsh-shell-integration.md) 推进 **NSH macOS 桌面壳集成**。先完成基线/总纲/API 差距审计，形成可审查的具体 Phase10 spec，再实施并验证窗口生命周期、托盘/单实例、热键、deeplink、通知和窗口状态。持续推进到该范围的完整验收；不要只交计划、只跑单测，或把 Phase9 PASS 当作新功能已经完成。每个实际缺陷先确认原因，再做局部修复并验证。

这是 Phase9 之后新提出的有界下一阶段，**不是原总纲已经定义好的 Phase10**。如果既有公共 NSH API 无法满足某项行为，先做小型受控验证并提出具体替代方案；涉及 R2/宪法、重依赖、新 daemon ownership 或 release 信任策略变化时，完成可审查方案后再取得必要决策。普通实现和审查修复不需要反复问“是否继续”。

## 必须先读 / Required reading

1. 根 `AGENTS.md`，然后 `docs/reference/SUBSYSTEM_ROUTING.md`、`REDLINES.md`、`DEVELOPMENT.md`、`WORKSPACE.md`、`STYLE.md`、`PROCESS_MANAGEMENT.md`、`SECURITY.md`、`TOOL_SYSTEM.md` 和适用的子系统说明。
2. **完整阅读 NSH 总纲，不只读摘要：** [docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md](../specs/2026-11-05-nsh-desktop-refactor-design.md)。重点是目标/反目标、crate/进程边界、Phase0–6、Phase2 OS 集成、Phase5 ADR 前提、Phase6 两周双端并行前提。
3. [当前 Phase10 计划](../plans/2026-11-18-phase10-nsh-shell-integration.md) 和 [Phase10 entry brief](../runs/2026-11-18-phase10-entry-brief.md)。计划列有全部历史 NSH Phase0/API/input/Phase1/Phase2/Phase3/技术债/Phase4 路径和实际源码 owner，请按表阅读。
4. [R2 已选 A 的边界记录](../runs/2026-11-16-phase9-nsh-r2-boundary.md)、[Phase9 规格](../specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md)、[Panel parity 合同](../specs/2026-11-15-nsh-panel-parity-slice.md)、[Phase8 main handoff](../runs/2026-11-16-phase8-main-handoff.md)。旧边界记录底部的 D OPEN 是历史状态，以最终验收为准。
5. Phase9 最终事实源：
   - [closeout](../runs/2026-11-16-phase9-closeout.md)
   - [execution receipt](../runs/2026-11-16-phase9-execution-receipt.md)
   - [acceptance JSON](../runs/2026-11-18-phase9-acceptance.json)
   - [final integrity](../runs/2026-11-16-phase9-final-integrity.json)
   - [source manifest](../runs/2026-11-18-phase9-source-manifest.json)
   - [default asset manifest](../runs/2026-11-18-phase9-default-manifest.json)
   - [original parity receipt](../runs/2026-11-18-phase9-parity-receipt.json)
6. `qa/phase9_runtime.md` 和计划中列出的 QA、NSH、Panel/WASM transport 源码。Graphify 先于源码探索；不要凭历史文件名推断它已挂载。

## 已结项事实 / Accepted starting point

**Phase9 A/B/C/D/E 全部 PASS，Phase10 admission READY。** 不要重开已闭合 Phase9，不要恢复旧 BLOCKED 状态，也不要伪造 active goal；旧 goal ID 已 blocked，不能沿用其 ID 调用完成工具。

- Phase9 实现提交为 `42764c43d4be5bbe26094a84d1ce9495f08a6dca`（`feat(nsh): close phase9 runtime proof and dependency validation`）。先用 `git merge-base --is-ancestor 42764c43d4be5bbe26094a84d1ce9495f08a6dca HEAD` 确认本地 main 包含它。
- 历史测量 HEAD/base 为 `3892804c112fd6e574166a4c03a7c40a77d0b239`，分支 `runtime-proof`。这不是重启后的当前 main HEAD；提交/合并改变 Git 身份不会改写旧证据的测量身份。
- A：retained 图 54 次终态 PASS；全量 Core `22,402 passed / 0 failed / 20 existing ignored / 656.84s`。
- B：真实 resolver 采纳 1,572-entry lock 图；adopted 图独立全量 Core `22,402 / 0 / 20 / 661.14s`。不要与 A 混作同一次执行。
- `Cargo.lock` SHA-256：`12300967333b45f8a0fa1f401cf13a559b5b8a881ee95550b1bbf1a8d42e21f5`。
- NSH 固定 revision：`f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1`，当前已验证的是 0.5，不是总纲旧的 0.4。
- C：同一生产 Panel 的 Chat/stream/Artifacts/reconnect 和真实 ingress guard、history hydration/session identity；Panel 1,568、Tauri 190、NSH 5 tests。专用真实 Gateway `queued → accepted → complete` 帧已接入生产 decoder/dispatch/ChatState 验证，这不是每轮性能矩阵都发生 queue 的声明。
- D：完整新块 `isolated-matrix-40-20261118T185504`，精确 **40 Tauri → 40 NSH**，各 10 warmups + 30 measured，每轮 fresh Gateway/provider/session；80 轮全部 PASS，6,400 原始关联事件，7,002 个真实资源样本。原 reducer 未改，`VALID / ACCEPTED` exit0；固定独立审计器从 raw 重新归约 PASS，`failures: []`。
- 中位数 Tauri → NSH：startup `5357.951 → 4933.0425ms`，route `33 → 33ms`，input `198.5 → 198.5ms`，reconnect `1041.5 → 1022.5ms`，close `47.679 → 48.921ms`，RSS `678.1015625 → 640.84375MB`，threads `76 → 68`；close p95 `77.444 → 61.365ms`。
- E：最终 SPEC `1b81b8e5` 后接不同模型 DeepSeek Flash QUALITY `8377f29f`，无阻断项；30 份审查原文及计数纠正均保留，最终 handoff checker PASS。
- 验收 JSON SHA-256：`8714d91fb95a07a95f0cf77f921c1917ca4451de5f31d79db7c9f72bd4e4f181`。
- default WASM SHA-256：`9f708c647e481cb1e1a53947ec8a6c0694895d6f10f9409ba46732e8db367cb8`。52 源码项、递归九个 default 文件精确恢复，三个 QA seam 不在 default WASM。
- 固定独立审计器 SHA-256：`1d358b50a1417c9ccad34e3fceaa7a042d44485355c8b7bd5b6c313865dfdc94`。不得修改它来通过新审计。
- 保留 17 个完整历史矩阵目录：15 INVALID + `174308 VALID/NOT_ACCEPTED` + `185504 PASS`，全部已恢复默认资产。174308 只因 close median 超限而未接受，不能改成 PASS，也不能拼接旧轮。

## 不可重开或越过的决定 / Binding decisions

**R2 choice A 已明确批准：NSH 0.5 WKWebView 只托管现有 Leptos/WASM Panel。** Panel 是唯一业务 UI、Chat/session 状态与 Gateway transport 权威。NSH 总纲中的 native Chat、`interfaces/nsh-desktop` 和旧 native UI 模块不能借本阶段复活。无需再次问用户是否选 A。

当前 `desktop/nsh-shell/src/main.rs` 只挂载 `shell_host` 和 debug QA clock。`connection.rs`、`session.rs`、`runtime.rs`、`supervisor.rs` 及旧 `components/` 都未挂载；不要为了“已有代码”把第二个 Gateway client、状态机或 detached daemon supervisor 接回来。`HostSettings::from_inputs(url, nonce)` 默认 `http://127.0.0.1:18790/`，debug 覆盖有成对 loopback/nonce 校验。

Phase9 `quit_app()` 证据只覆盖单窗口 **应用退出**。它证明 `run_app` 返回、kernel 退出与全部已观测成员 settlement；**没有证明用户 WindowClose 回调**。Phase10 必须通过真实 public native callback 补齐 close/hide/reopen/quit；不能把 `owned_drained` 一个标记当作 OS 完整清理。

Phase10 的 G1–G7：

1. 真实 user close → resident hidden → reopen/focus；显式 Quit 终结 shell-owned 资源。
2. Tray Show/Quit + 真实两实例的主实例/意图投递/次实例退出。
3. Summon hotkey、注册冲突、禁用及 teardown。
4. 冷/热/第二实例 deeplink；复用 canonical grammar 和 Panel navigation，拒绝无效输入。
5. 原生通知与点击回到当前 Panel；拒绝权限时如实记录，不能绕过系统设置。
6. 支持的窗口状态恢复与无效/off-screen 几何处理。
7. 所有新增 callback/registration/worker 的启停和终态；外部 managed Gateway 不受 shell quit 影响，原 Panel 功能和新性能证据通过。

不在本期：release 安装/签名/公证/更新安装、切换默认桌面、full embedded-core ownership、detached daemon 接管、Core 执行重构、installed-service SimulatedEngine transcript 修复、新 native 业务 UI、Plugin Runtime、Swift camera/voice/screenshot、i18n 迁移、跨平台验收、移除 Tauri/WebChat/workspace member。总纲 Phase6 的 Phase4+两周双端并行前提仍在。不能凭 Phase9 macOS debug 结果声称这些已完成。

## 执行纪律 / Execution rules

- 回复中文，代码注释英文，新文档中英双语。先说方案与后续代价，再写代码；对不确定 API 做局部实验，不猜。
- **不读取、不修改、不 stage** `.claude/settings.json`、根 `config.lock`、根 `config.toml`；不改宪法/ADR 来回避 R2。`Cargo.lock` 可按真实必要性修改，但不得顺手升级 NSH pin。
- **Graphify 先于源码探索**，再用 targeted search/read；任何获授权 child 的 brief 必须原文包含这一规则。
- Rust workspace/toolchain 以 `docs/reference/DEVELOPMENT.md` 与实际文件为准。每次 Cargo 前必须 `python3 qa/browser_live/check_memory.py --required-bytes 4294967296` 得到 ALLOW；单 Cargo owner、禁止并发，`--offline --locked --jobs 1`，用新的 `.phase10-target`，保护 `.phase9-target` 的旧证据身份。
- 正常使用所需测试，不因历史通过就省掉改动后的验证。Panel 有任何改动跑 Panel lib tests；共享代码改动跑共享测试。构建 QA/default WASM 必须串行，并验证实际 HTTP 资产、默认 seam 缺席及精确恢复。仅文档或 Python 改动时按影响验证，不盲目重编全仓。
- `just stage-shell-placeholders` 可能覆盖 sidecar；debug RustEmbed 会动态读当前 dist。不要把 0-byte placeholder 当全包证据，也不要把复制 Gateway binary 当冻结 WASM。
- 遵守 Capability、工具与已有 source authority；无第二 registry/lease map/business reducer。禁用功能不初始化额外线程、连接或缓存；资源必须可停止、可验证终结。Core 不调用平台 API。
- review 顺序 **SPEC → QUALITY**，后者用不同 Pi 模型；仅在授权下委派。不能用外部 Codex/Claude/Gemini CLI；DeepSeek 仅 Flash。修正真实 finding，保留误报撤回和原始报告；不为假设风险无声扩范围。
- 原 `qa/nsh_panel_parity.py` reducer 和阈值不变：100ms owned-resource 采样、50–175ms 容差、p95 rank29、latency median ≤1.10×/p95 ≤1.20×、RSS median ≤1.10×、threads median delta ≤2。新 shell 变化需要新完整可比 block；不得选轮拼接、补假样本、放宽归属或改用全机 RSS。新增 OS 场景另有原生证据，不能塞入旧 schema 冒充同一测量。
- 无 root、private API、OS policy 绕过、真实用户密钥/daemon 修改。无新 remote push、release、默认替换授权；完成本地实现与可审查结果后再报告。

## 私有证据与续接 / Local evidence continuity

当前检出保留忽略的私有根：

```text
.superpowers/sdd/2026-11-15-phase9-runtime-proof-and-dependency-closure/full-closure/
```

重要内容：`isolated-matrix-40-20261118T185504/`（matrix/source/default restoration/independent integrity/audit execution），`final-audit-program/verify_current_matrix.py`，`verify_final_handoff.py`，`review-archive/`，`final-review-note-reconciliation.md`，`queue-projection-evidence.json`，`queued-wire-20261118T145900/`，`meter-fixes-20261118T155601/receipt.json`，`quit-app-build-20261118T181620/receipt.json`。计划 §7 列出关键机制和历史失败原因。

这些 raw 证据/二进制没有整批写进 Git。切换 main、同检出重启会话仍可用；新 clone/换机器时不保证存在。缺失时报告证据不可用，不能重造旧 PASS receipt。公共 acceptance/manifests 是已提交的导航和身份记录。新的 Phase10 QA 产物使用独立目录，保留旧 blocks、原审计器和被撤回报告。

开始执行：先更新 todo，确认 main 和 accepted baseline，完整读总纲/计划/约束并输出简短基线与范围判断；然后做 Task 1 的公共 API/真实生命周期 spike、具体 spec 与审查，再按任务依赖实现。完成时按 G1–G7 逐项列证据、当前 source/default/build 身份、实际检查和未覆盖项。不要擅自宣称总纲所有阶段或 release readiness。

---

**This prompt prepares the next session; no Phase10 implementation was performed when it was authored. / 本提示词编写时尚未实施 Phase10。**
