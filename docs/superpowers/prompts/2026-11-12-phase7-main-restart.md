# Main restart prompt / 从 main 重启实施的 Prompt

Use this file after restarting in the actual local `main` worktree. This file is a proposed task request, not a record of approval already given. The only unresolved product-contract choice is Option A's early-row semantics; confirm it once before code. Execution method is already sequential SDD.

请在重启后的 main 会话粘贴下方代码块。无需现在选择实现方法；不要把本会话文档落盘或合并当成 readiness 已修复。若你不同意提前建行，下一会话应改做 B 的就绪事件设计，不能直接执行 A。

## Copy into the new session / 新会话复制正文

```text
从本仓库本地 main 继续，实施会话元数据发布与真实 UI attach 的就绪修复，并按已有路线推进 Phase7 剩余验收和后续阶段。不要只复述总规划，也不要重做已合并的整个 Phase7/NSH 模块。

先读这三个文件：
1. docs/superpowers/specs/2026-11-12-session-ui-readiness-design.md
2. docs/superpowers/plans/2026-11-12-session-ui-readiness-and-phase7-continuation.md
3. docs/superpowers/runs/2026-11-12-phase7-main-checkpoint.md
并遵守 CLAUDE.md、AGENTS.md、DEVELOPMENT.md 和实际涉及子系统的局部约束。

【先核实工作树，不猜路径】
用实际 Git metadata 确认 root、branch、HEAD、worktree 列表和 status。main 必须包含 checkpoint fa996e6407c1ba7adbdf871b9b4185a0257dafd8 及上述最新文档。会话名称、工具缓存中的旧 cwd 或 worktree-nsh-phase0-spike 名称不能代替实际源码树身份。若启动在其他 worktree，按 git worktree list 找到已有 main worktree，不强制 checkout 已被占用的 main，不 reset/clean。然后从 main 创建 fix/session-ui-readiness 工作分支；同名分支已存在时检查并汇报，不覆盖它。

.claude/settings.json、根 config.lock/config.toml 是上次保留的无关本地状态，不读取并外发其内容，不 stash/删除/覆盖/加入提交。原 task branches/worktrees/stash/证据/targets 都保留。旧 NSH 分支和 stash 内容已核对并包含在本地 main；不要重放已包含的 stash。没有新的明确授权，不 push、发布、删分支、清理旧资源或修改全局工具/权限。

【唯一产品合同确认】
先向我明确确认一次：是否采用推荐 A——两个真实 chat.send/agent.run handler 的每个携带 addressed-session key 的成功响应前，canonical store 已提交该 key 的元数据，包括不 enqueue/no RunAccepted 的 inline/slash 成功路径。合法 no-run inline 请求也可能建行，其他合法请求随后仍可能因 queue/spend/run admission 拒绝、timeout/purge/cancel，已提交行不会因 caller 断线自动回滚。错误、不携带 addressed key 的成功及其他 RPC 不在这项承诺内。A 增加一次存储延迟，不承诺 run slot/model 已开始、surface 可用或 renderer ACK。
这项语义还没有在上次会话获批。未明确确认前不要修代码。若不接受，停止 A 的实现，提出 B 的 server-owned readiness/terminal-event 合同；不要给原 QA 偷加 sleep/retry/session.create 来变绿。不要再问通用需求、SDD/Native 或把文档审批当成产品验收。

【批准 A 后具体做什么】
按 plan 的 R0→R1→R2→R3，用既定 superpowers:subagent-driven-development，先准备真实可发现且能编译的 integration target，再做行为 RED/GREEN：
- 新 tests/session_ui_readiness.rs 必须由 --list 发现。aleph-server 在 Cargo.toml 是 test=false，不能把 bin 的 cfg(test) 加了就说 --lib 跑过真实 handler。内部 pub(super) EmptyToolRegistry/test helper 不是集成测试公共 API；fixture 若真正受私有 seam 阻塞，报告具体编译/API 问题，不假造 helper 或放宽生产边界。
- 用同一临时真实 SessionStore backend 的 delegating barrier，真实调用 server_init 的 real-engine handler。卡住 get_or_create commit 前，RPC 不能成功、不能 enqueue/provider effect；释放后成功响应所指 key 的真实 metadata owner/scope 必须正确。旧实现必须因成功响应过早而 RED，不是因 auth/fixture/编译/零匹配失败。
- 将 AgentInstance::ensure_session 改为 Result<SessionMetadata, SessionStoreError>，用一个 prepare_session_for_run(agent, request) 复用现有 request_scope attribution；不能只 await 当前 warn-and-unit helper，不能用 Gateway current_scope 重拼 owner，也不能增加 ready bool/map/latch 或第二 store。
- 盘点两个真实 handler 的全部成功返回点：经过相应 routing、已有 visibility、agent authorization 和 response-key/slash selection 后复用 canonical request-attribution；queued 分支在 spawn_queued_run 前、no-run inline 分支在各自携带 key 的成功返回前 await response-addressed-key preparation。检查真实 returned incumbent metadata 对可信 caller 可见；foreign first-speaker 不能 restamp。inline 同样要求 commit 后同 caller 可 attach、零 queue/provider effect、无伪造 RunAccepted；错误无 success、无 spawn、无 raw backend 泄漏。
- 保留 engine 对实际 execution key 的 preparation；覆盖 execute/simple/btw_promote/OpenAI 等所有 ensure callers 的 Result 和真实资源/guard/emitter 结算。既有 ExecutionError::Failed/safe receipt 足够，不造错误/状态框架。/btw 的 response key 与 side execution key 不混用；非 RPC producers 仍受真实 preparation。
- 跑 plan 指定的 foreign/concurrent/error/cancel/commit后caller丢失、queue拒绝/timeout/purge/spend、inline、/btw、stale mount/provider/connection、late ACK、detach 和 operation lifetime cases。caller future drop、token cancel、oneshot response、trace append、metadata commit、renderer ACK、physical PTY/effect settle 各自分开证明。

【代理与验证纪律】
一棵实际树同一时刻只有一个 agent 读写；主控协调，implementer/reviewer 串行接管并明确 handback。禁止嵌套、晚到消息自行恢复、重复调查或新一轮私自扩面。每个涉及源码的 Agent/SendMessage brief 都原样附带：
MANDATORY: graphify-out/graph.json exists. You MUST run graphify before reading source files. Use: `graphify query "<question>"` (scoped subgraph), `graphify explain "<concept>"`, or `graphify path "<A>" "<B>"`. Only read raw files after graphify has oriented you, or to modify/debug specific lines. This rule applies to subagents too — include it in every subagent prompt involving code exploration.

每任务按 TDD、真实非零测试发现/计数、独立 SPEC 和 QUALITY review、必须的 /verify 与明确文件提交范围推进。常规 scoped 修复提交→复审→下一验收直接继续，不反复问“继续吗”；只有新产品合同/权限/外部副作用/真正缺信息才向我确认。不要在 default branch 直接提交产品修复。
所有 Cargo 命令显式独立 CARGO_TARGET_DIR；先读并满足 repo memory gate。长任务受控后台、owned PID/PG、保留时限与 reserve、零隐藏 retry、实际终态消费。不轮询日志或任务，不以无通知断言运行/成功；到界限的真实恢复是一次有界结果取证，不是重复执行。未知脚本先看内容；绝不改 harness/CI/checker/trust/阈值制造绿灯。命令 consent 缺失就停在真实授权边界，不默认授权全局工具更新、依赖安装或 fake Git。

【实际集成与剩余 Phase7】
R3 用新独立 ALEPH_HOME、ports、target、Panel output、临时身份、local deterministic provider/renderer、真实 Gateway/PTY 运行原 qa/capability_i3/probe.py。确认 Real mode，不用 paid/external LLM 或 Simulated fallback。原 chat.send→立即attach 场景不改；真实 Gateway receipt 与 mock Panel DOM receipt 分开。served gzip/br 解压 hash 对当次 build、真实 render ACK、Memory、PTY、audience/isolation、45s 正常 shutdown 必须实际到达；failure cleanup -9 不是 graceful PASS。

之后按 I3-A fresh final changed-tree 全集成 review→I3-B fresh production acceptance→P7-G 独立全局 gates→四类原 Phase7 deliverables 逐证据结束。旧 E2 warm rebuild 的 Core build0/runtime1、-32099/session unavailable 和 NONE→execute→ensure→OK 只是当前观察，不是旧 binary 根因或规范 ready 承诺。旧 full-core50FAIL、strictlint、>=80% 实测coverage、授权 wire mutations、served bytes、ignored/incomplete、global verification/tooling incident 分别仍 OPEN。未经明确新授权不做破坏性 mutation；编译红不是行为 RED；focused green 不等于 I3/whole Phase7 PASS。D-T=A 是实际 PTY input/attach/resize/close，不冒称严格三流 API。

【后续阶段必须保留，但按真实 gate 推进】
Capability master 为 docs/superpowers/plans/2026-11-05-phase7-advanced-capabilities.md，I3 为 docs/superpowers/plans/2026-11-07-phase7-i3-production-integration.md。Phase7 NOT_ACCEPTED；四类是 Activation/Constraints、Special Capabilities、Event/Audit、Performance/Optimization。全部判定后再为已确认名称 Phase8 Integration & Security 写真实 spec/plan；当前没找到 Phase8 细案，不编造 Phase9/owner/工期或预先批准。

NSH master 为 docs/superpowers/specs/2026-11-05-nsh-desktop-refactor-design.md，编号与 Capability 不同：Phase0 spike/API/成本；Phase1 shared protocol/client/hello/events/sessionlist/transport；Phase2 native shell；Phase3 real chat；Phase4 全 UI parity/双桌面 QA；Phase5 独立 PluginRuntime；Phase6 最后移除 Tauri。先对照 main 实际代码盘点各 gate/delta，不重做已合并 Phase0–4。现在 desktop/nsh-shell 的 session UI 仍有 local IDs/fake replies、hello/list/subscribe 未完整投影真实 chat events；必须接 canonical key/RPC/events/history，不能因 shell transport 存在就宣布集成。

我的偏好是尽量使用最新 NSH 0.5.0/新特性。官方 v0.5.0 release 观测 commit f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1；上次 E2 explicitly 没编译 NSH，resolver 里旧0.4 manifests 不证明兼容。先单独冻结/核实 CLI、crates、native host/runtime、generated types、WindowRef/window kinds/lifecycle、输入/selection/history/streaming/stop 行为及平台/工具链，再给小范围迁移 spec/plan；不更新全局 nsh/Cargo 或一次性跨到未知 nightly。旧输入限制/Tool-only alignment 文档是历史，不能覆盖当前 I3 与0.5事实。
NSH Phase5 先核实 ADR006 WASM、ADR008 signing 的当前 Accepted 前提；Phase6 必须 Phase4 完成并双跑两周，不能提前删除 Tauri/workspace members。GPUI 继续延期。

【最终报告】
分别报告：修复实际源码/测试、真实 I3、whole Phase7、NSH0.5兼容、下一阶段 gate；提供实际 source/build/SHA/receipts、failed/ignored/not reached/incomplete、未做项和代价。不要把保存文档、编译通过、取消标志或旧 reviewer PASS 当成本次 production acceptance。除非我另有明确要求，别 push、发布或清理旧证据。
```

## Reading the gates / 如何使用

The design and task plan are the authoritative detailed handoff. Read the checkpoint note for branch/stash integration and unresolved validation. The copied request deliberately asks for the **one actual contract decision** before code; it does not manufacture a past approval. Later subsystem specs also require their own approval. If tooling or a real fixture is blocked, report it instead of changing the acceptance criteria.
