# Restart prompt: production Capability mount evidence / 重启执行 prompt

**Status / 状态:** Unimplemented next task / 下一任务未实施。
**Prepared after / 前序完成:** `4b6da265a feat(capability): activate cold tools on first call` on local `main`. The subsequent handoff commit contains this prompt/spec/plan and bookkeeping only. / 后续交接提交仅含本 prompt、设计、计划和记录修正。

Copy the block below into a new session. Repository paths are relative; discover the current checkout rather than assuming this machine's absolute path. / 将以下整段交给新会话，先定位实际仓库，不依赖本机绝对路径。

---

请按 `docs/superpowers/plans/2026-11-03-capability-tool-mount-evidence-plan.md` 执行下一有界任务，并完整阅读对应 spec：`docs/superpowers/specs/2026-11-03-capability-tool-mount-evidence-design.md`。本次明确调用授权该计划中的证据核验、文档校准和经真实 RED 证明的最小局部修复；不要重新询问已确定的 Native 执行方式或重复前序设计讨论。新的跨域架构仍需单独决策。

Execute the linked bounded plan with Native execution: main agent sole writer, supported Pi read-only children for final review. This invocation authorizes evidence verification, current documentation alignment and only a real-RED-backed local defect fix. It does not authorize a new lifecycle architecture.

目标仍是 Aleph 的生命周期隔离与运行时可组合性，每项事实只有一个权威。前序完成的内容：

- Tool-only WASM Cold publication、统一 receipt、权限通过后的首调单飞激活、Ready 原子替换与 sealed dispatch proof。
- Cold/Ready receipt 不受普通 register/replace/unregister 绕过；owner/provider/OwnershipTree fencing 保留原 authority。
- canonical/agent metadata 可见 Cold，MCP compatibility 仍 mounted-only，Safe Replay 在 claim 前拒绝 Cold。
- FEATURE_LOCATOR Current / Deferred / Historical 分层及可验证相对链接修复；scanner 的识别范围有限，不代表完整 Markdown 解析。

基线验证（历史证据，不算你本轮新跑）：Cold registry 2/2，stage_f 10/10，stage_h 6/6（其中 2 项是 harness trace），Cold replay refusal 1/1；core tests/server check 和 scoped clippy 通过；最终 Pi whole-diff review Clean/APPROVE。上一轮全仓 fmt 仍有无关既有差异，不要批量格式化。

首先核验当前 Git root、分支、工作区及 `4b6da265a` 是 HEAD 祖先。工作原先已经直接提交到 `main`，无需伪造分支 merge；允许更新的 handoff-only HEAD，禁止 reset 回历史 SHA。保持本地，不 fetch/push/force/merge，不切到无关 origin/dev。遇到他人脏改动先识别，不能覆盖。

关键纠正：**不要实现新的生产 Zahir mount。它已经存在。**

- `src/bin/aleph-server/commands/start/mod.rs` 在 canonical registry 安装后调用 `ProjectionHost::mount_with_memory`，安装 host 并等待 `wait_until_ready`。
- `src/capability/projection_host.rs::mount_sources` 从传入 registry/tree 构造 `ZahirFacade::new_with_memory`，订阅后读 snapshot。
- `StartupResources::cleanup` 先 stop Memory，再 `close_and_await` host；共同 shutdown funnel 再处理 builtin registration scope。
- Tool callable backend 是 `ToolBackendAdapter`；具体 `MemoryResource(Weak<MemoryRuntime>)` 也已 source-backed 组合，不代表通用 Resource 后端或所有 Memory writer 都迁入同一生命周期。
- `AGENTS.md` 与 `CLAUDE.md` 仍有“全仓无生产构造/mount 未完成”等过期表述，二者必须一起校准；先用源码和现有测试核实，不能盲信入口文字。

按计划先创建 `docs/superpowers/runs/2026-11-03-capability-tool-mount-evidence-run.md`，区分生产源码、静态接线守卫、真实效果测试、历史结果和本轮结果。优先运行并复用现有测试；不要复制测试或为了改代码编造缺口。若全部效果已覆盖，文档和证据闭环就是合格交付。

每个 Cargo 命令前必须执行 `python qa/browser_live/check_memory.py`，要求 `ALLOW` 且 available bytes 至少 `4294967296`。使用 `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`，串行执行，测试必须实际选中正数项目。不要把零测试、编译报错或源码扫描当作真实效果通过。新增测试前读取适用测试规范；用真实 store/host、Notify/channel 握手，不用 sleep race、global mocks 或 public test-only API。Linux/Windows 原生执行豁免继续有效，但最终披露未跑。

约束：不新增 registry/tree/generation authority/Capability kind/provider proxy/universal supervisor；不改 raw archival ownership；不推进 Feishu/Channel/Terminal/MCP lazy/EffectClaim producer/Safe Replay 扩展。如果必须如此才能解决，完成独立证据整理，报告具体阻塞并提出单独架构设计，不扩大本计划。

最后运行文档 scanner/self-test/diff-check，按是否改 Rust 执行计划要求的 check；获取 fresh read-only whole-diff review，修复 concrete findings 后更新双语记录和 checklist，再本地 conventional commit，核验 clean。模型只能用环境实际支持的配置；不要再硬编码前序失败的 `openai-codex/gpt-5-codex`，禁止模型 CLI fallback。

最终说明修改文件、复用/新增测试的准确计数、source-wiring 与 effect evidence 区别、review verdict、未跑检查、deferred 边界和明确未做项，并基于证据建议一个下一独立切片。不声称 universal all-writer stop、native unload、RSS reduction、process isolation 或 external exactly-once。

---

This file is an execution handoff, not evidence that the next task has already run. / 本文件是执行交接，不是下一任务已实施的证据。
