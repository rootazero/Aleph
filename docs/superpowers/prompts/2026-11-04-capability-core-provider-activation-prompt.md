# Restart prompt — Capability Core provider activation / 重启提示——Capability Core 提供者激活

> Copy the text below into a fresh session. Discover the active checkout from Git; do not assume an absolute path. / 将下方正文交给新会话。先从 Git 定位实际 checkout，不假设绝对路径。

---

请按 `docs/superpowers/plans/2026-11-04-capability-core-provider-activation-plan.md` 执行下一有界任务，并先完整阅读设计 `docs/superpowers/specs/2026-11-04-capability-core-provider-activation-design.md` 与前序证据 `docs/superpowers/runs/2026-11-03-capability-tool-mount-evidence-run.md`、`docs/superpowers/runs/2026-11-03-memory-source-owned-validation-closure-run.md`。用户已授权在下一会话按计划实施本切片；不要重新发明已批准的 kind-neutral 契约方向，也不要把本切片扩成「所有能力都可按需启停」的总体工程。

## 目标

终极目标（用户原话）：**Everything externally composable is a Capability.** 一切需要被 Agent、Runtime 或其他系统动态发现、调用、替换、订阅或管理的能力，都是 Capability；Aleph 要从「一个集成很多功能的进程」演进成「一个轻量 Capability Core，加上一组按需启动、可停止、可替换的能力提供者」。

本切片只做其中**第一块地基**：让 Capability Core 能看见、解析并**显式请求启动**第二个种类的能力（provider-shaped capability），并用一个既有子系统用真实资源端到端证明。

1. **Task 1 — provider 家族 census + 契约冻结**：普查仓库里所有 provider 家族（WASM/plugin owner、Terminal/`PtyManager`、Gateway channels、Feishu webhook server、Memory runtime，以及能发现的 browser / sandbox / computer-use owner），逐项记录 owner、是否已有 `ProviderHandle`、生命周期状态、真实资源、今天如何启动与停止、Core 今天能否看见；核对设计文档的前提假设；冻结契约词汇（registry / adapter / activation hook / provider lease 的证据形态 / 激活错误集 / `MemoryState → ProviderState` 映射）。**契约必须是 kind-neutral**：如果词汇里出现家族名，现在就改，不要等代码存在后再改。
2. **Task 2 — Core provider registry + provider-backed backend**：新增 Core 内 provider registry 与实现既有 kind-agnostic `CapabilityBackend` 的第二个 adapter；`ZahirFacade` 同时服务 callable 与 provider；**把 Memory 从 facade 的硬编码特例迁到同一条通用路径**（判据 #1：同一个事实只能有一份表述，不留两套 Resource 机制）；既有 `projection_host_memory_tests` / `zahir_facade_memory_tests` 是通用路径的回归网，必须保持绿。
3. **Task 3 — 显式激活契约**：`resolve` 永不启动任何东西；激活是独立、显式的请求，由 owner 执行效果（R1：Core 不做平台工作），失败必须 fail-closed（不得返回陈旧 lease），返回绑定新 provider generation 的 lease；provider lease 在 stop / 替换后必须 fail-closed，且证据是真实效果而不是 identity 比较。
4. **Task 4 — Terminal 证明实例**：把 `PtyManager` 作为 `CapabilityKind::Resource` provider 接入 Core。证据必须是真实资源语义：cold 意味着**不存在 PTY 资源**（无 owned driver、无 session）；resolve 不启动；显式激活真的拉起 owned driver（用真实可观测输出，不要只看状态 flag）；stop 真的 join driver、关 session，且此前 lease 失效；再激活产生**新 generation**，旧 lease 保持失效。**不得改 `PtyManager` 自己的状态机**。
5. **Task 5 — 证据、归因、审阅、交付**：全量串行测试；若出现新失败，按完整测试名与「本切片起始 commit」对照归因（detached worktree + 隔离 `CARGO_TARGET_DIR`），不修复无关失败；写双语 run record；跑文档/rustfmt/diff 检查；取一次全新只读 whole-diff 审阅并修复具体 findings；本地 conventional commit 并确认工作区干净；最后**按事实**校准 `docs/reference/FEATURE_LOCATOR.md` 与 `docs/reference/MEMORY_SYSTEM.md`。

## 起始状态和 Git 纪律

- 本切片计划写于本地 `main` 的 `8e8502385`（`test(memory): close reconciler evidence gaps`）之后；Memory source-owned boundary 与 validation-closure 两个切片已完成并已落在本地 `main`（`a4fa8088f` / `2fcd93970` / `8e8502385`），**无需再做分支合并**。先核验 Git root、当前分支、HEAD、`git status --short`，并把实际起始 commit 写进 run record（它就是全量归因的基线）。
- 只在本地工作；除非本会话用户明确要求，不要 fetch、push、merge、rebase、reset 或切换 primary checkout；不要覆盖用户改动；工作区若有与本切片无关的改动，先停下报告。
- 对照基线只允许创建本任务拥有的 detached 临时 worktree 与隔离 `CARGO_TARGET_DIR`；不要并行运行两套 Cargo；结束后只清理自己创建的 worktree/临时文件。
- 上一轮全量对照结论可作背景：当前 `main` 与固定 pre-slice 基线 `063345f2f30bf999b28ee5d84aac860be8432fa7` 的失败具名集合完全相同（51 共有、0 独有）。本切片若出现新失败，同样按具名集合归因，不得用聚合计数下结论，也不得顺手修复无关失败。

## 必须遵守的测试纪律

- 每条 Cargo 命令前运行 `python qa/browser_live/check_memory.py`；必须得到 `ALLOW` 且 available bytes 至少 `4294967296`。设置 `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0`；聚焦与全量都使用 `--test-threads=1`。
- 聚焦测试必须实际选中正数；零测试、编译错误与源码扫描都不算行为证据。
- 测真实行为：真实 Core、真实 owner、真实资源。禁止 sleep race、global mocks、公开 test-only API、第二个 registry、第二个 lifecycle authority。合成 provider owner 只能放在 Core 内部的 `#[cfg(test)]` 模块里。
- 只有当真实 endpoint/契约行为先 RED 证明缺陷时，才允许最小生产修复。
- `bun run check` 在本 checkout 不存在，除非新 checkout 真的出现 `check` script，否则不要运行或声称通过。Linux/Windows 原生检查在不可用主机上只记录未执行，不伪造替代结果。

## 范围红线

- **Core 必须保持 kind-neutral**：`src/capability/` 内不得出现家族名（terminal / pty / memory / channel / feishu）。任何家族都是实例，不是词汇表。
- **R1**：Core 不做平台工作（不起线程、不开 transport、不加载动态库、不做 IPC）。Core 只**请求**，owner 才**执行**。
- 不做 universal all-writer stop、native unload、RSS、process isolation、external exactly-once 声明；不接 Channel/Feishu/Terminal 之外的其他通道；不接 MCP/browser/sandbox/computer-use；不建 EffectClaim producer、Safe Replay、universal scheduler。
- **不宣称「代码不被主线程拉入内存」**：进程内懒加载 ≠ 二进制瘦身，那需要 Bridge/进程拆分（R1），是后续切片。
- 不改 Memory producer 架构，不引入 supervisor；raw/session 归档保持与 derived Memory 独立。若某个行为必须新增公开或生产生命周期控制接口才能确定性运行，停下并记录理由，把该行为保留为未闭合限制。
- 已知仍 deferred、不得在本切片顺手做掉的：Gateway channels / Feishu / MCP / browser / sandbox / computer-use 接入、EffectClaim producer 与外部 exactly-once、Safe Replay、universal durable scheduler、Memory 侧剩余 derived consumer / RPC operation escape census。

## 交付

先创建 `docs/superpowers/runs/2026-11-04-capability-core-provider-activation-run.md`，记录起始状态与 census。随后完成计划五个任务：记录精确测试名/计数、内存门读数、契约冻结结果、Terminal 的真实资源证据、全量失败具名归因、审阅结论与未执行项。做文档 scanner/self-test、rustfmt、diff-check，取得全新受支持的只读 whole-diff 审阅，修复具体 findings 后本地 conventional commit 并验证 clean。更新 docs 只限于事实校准：明确 Core 现在能看见/启动/围栏什么，以及 channels/Feishu/MCP/browser/sandbox/computer-use/EffectClaim/exactly-once/二进制瘦身仍然 deferred。最终清楚区分证据与未完成项。

---

本 prompt 是下一会话的执行授权与范围交接，不代表本计划已实施。 / This prompt authorizes the next session's bounded execution; it is not evidence that the plan has run.
