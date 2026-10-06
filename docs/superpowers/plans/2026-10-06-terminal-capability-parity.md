# Terminal Capability Parity Implementation Plan — 总编排 / Integration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. User requested delegated implementation; native implementation needs explicit permission if no Agent tool is available.

**Goal:** 将 Aleph terminal 收敛为 TUI-first、Core-owned 的 capability 工作台，闭合 herdr 会话、交互、布局、agent 控制、hooks、worktree 与恢复能力，并清理断链 / Deliver a capability-owned terminal workbench, not another UI-only terminal.

**Architecture:** 复用唯一 `ToolHandlerRegistry` 和 `ScopedToolService`；内部 `TerminalRuntime` 只借用已有 PTY/runtime stores。逻辑布局属于 Core，TUI/Panel/CLI 是投影；人类交互与不可自动重放的 agent 控制采用不同授权 / One callable registry, borrowed runtime state, thin clients, separate human grants.

**Tech Stack:** Rust 1.96.0 / MSRV 1.95；Tokio、serde/schemars、现有 portable-pty/vte、ratatui 0.29/crossterm 0.28；Python 标准库用于 QA。不加运行时第三方依赖 / No new runtime dependencies.

**Spec:** `docs/superpowers/specs/2026-10-06-terminal-capability-parity-design.md`，含用户单独批准的 §2.3 人类交互例外。

## Global Constraints

- 所有写入和提交仅在 `D:/Workspace/Aleph/.worktrees/terminal-capability-parity` / `feat/terminal-capability-parity`；base 实际 SHA `a5300221d27c7a916407d8b96cadccb91cde1bbf`。不编辑、checkout、merge 到 `main` / Keep main untouched.
- 用户的“合并回主分支”按本任务集成分支解释：支线进入 `feat/terminal-capability-parity`，不得因此直接修改 main / Integrate into the feature branch, not main.
- `src/harness/` 零改动；`interfaces/tui` 不依赖 alephcore；不另建 callable registry、scheduler、数据库或 VT/parser / No second brain or store.
- agent 控制配置键定为 `policies.terminal.terminal_write`，`off|ask|on`，默认 `off`；`on` 仍不消除 ExecTier、身份、owner、前台、审批与审计 / On is not unrestricted.
- human raw input 独立显式授权，默认未授予；仅连接绑定 grant 可用；不能由 JSON 中 human 标志或 Tool call 获得；原始字节流明确不等同逐命令 command policy / Human grant is not delegable.
- 起进程与自动命令调用既有 command policy；prompt 是 agent 文本，不宣称能够在 Aleph 外约束第三方 agent 的执行 / No false end-to-end sandbox claim.
- 所有新 terminal descriptors `ReplayPolicy::Unsafe`；只有显式另行批准的 Safe replay 设计才能改变它 / Never automatically repeat unknown writes.
- 16ms sole flush clock；前台 probe `500ms/3000ms/6 misses`；cwd OSC7 > foreground cwd > spawn cwd；`quiet_since` 不产生 Idle；seq gap 必须 reattach。
- 禁止扩大 actor-less RPC 权限。read tool 与 RPC 现有 actor-less 差异必须通过显式 caller provenance 保存，而非粗暴统一 `owner_admits` / Preserve narrower tool admission.
- 中文交流、英文代码注释、双语文档、English scoped commits；完成报告明确 UNRUN、未交付及恢复限制。

## Review Focus

1. Hook 改写有效输入、同名并发调用、registry replacement：批准不能串给别的调用，identity 与 handler 必须同代 / A3, A4, B2。
2. 断连重连、事件先于 attach、重复帧、漏帧与旧 attach 回包：客户端不得回滚新屏幕 / A5, A6。
3. UTF-8/CJK/emoji、末列 wide cell、嵌套控制字符与大 paste：渲染/输入有界且不把 prompt 变成控制序列 / A6, B3。
4. 假 human 标志、跨连接盗用 grant、actor 缺失、前台在两次写入间变化：拒绝且不留下首字节之后的自动重试 / B1–B3。
5. 磁盘失败、PID 重用、layout revision 冲突、hook 配置外部修改：效果未知不报成功、不重复 spawn、不覆盖用户内容 / B4–B6, C1, C4。

---

## 1. 三份垂直子计划 / Three reviewable subplans

| 子计划 | 任务 | 产物 | 启动前依赖 |
|---|---|---|---|
| `2026-10-06-terminal-capability-parity-a.md` | A0–A7 | 同代执行、canonical read、typed client、TUI screen/attach/reconnect | 总计划批准；A0 baseline |
| `2026-10-06-terminal-capability-parity-b.md` | B1–B6 | human grant、guarded agent writes、start/close、external agent hooks | A3/A4 canonical dispatch；B3 TUI 集成依赖 A6 |
| `2026-10-06-terminal-capability-parity-c.md` | C1–C6 | workspace/tab/BSP、worktree、durable restore、parity/cleanup/docs | C1 可并行 A5；C3 依赖 A6；C4 依赖 B4 |

三份计划共享本 header 的所有约束。禁止提前注册尚未实现的 no-op descriptors；每个切片只有其真实功能可见 / Register only working slices.

## 2. 命名与 spec 问题的明确答案 / Planning decisions

以下是供计划审阅的具体实现决策；不是已执行的代码 / Proposed concrete contracts, not existing APIs:

- **Canonical callable names:** `terminal_sessions_list/read/status/wait/explain/attach/resize/prompt/send_keys/start/spawn/close`；布局动词 `terminal_layout_get/apply`；worktree `terminal_worktree_open/close/list`；hooks `terminal_agent_hooks_status/plan/apply`；恢复 `terminal_recovery_inspect/recreate/abandon`。RPC dotted face 与旧 `pty.*` 只是参数适配到同一 identity，无别名 descriptor。
- **Streaming is not a Tool-per-frame:** screen/exit/runtime topics 继续原 event source，仅 attach/subscription 的 admission 经过既有可见性边界；不为每个 16ms patch 写 durable Tool intent。
- **Human input:** 独立 privileged callable `terminal_interaction_authorize/revoke/input` 使用同一 canonical registry，但只给服务器验证过的 direct-interaction request context；从 LLM catalog 隐藏，隐藏不是授权。`pty.input` 映射到同一 input，必须有 grant。
- **start vs spawn:** spawn 仅启动已配置 shell；start 用受治理的 argv 启动 manifest 已识别的 agent（不在 shell 中插入字符串命令）。自动执行不接受自由 shell 脚本。两者分别审批。
- **Workspace:** 复用已存在 AgentEnvStore workspace identity；terminal tab/layout 是其扩展，不重定义 `workspace.create`。第一期即多 workspace + 多 tab + BSP，不降低全量目标。
- **Worktree:** 复用 `src/sandbox/worktree.rs::create` 与生命周期 handle；先 policy/admission，后 git effect；引用现有 workspace root，禁止任意 path。显式 open/close 暴露 callable；不让布局命令偷偷创建或删除 worktree。
- **Hooks:** 现有 Aleph extension hooks 不等于第三方 agent lifecycle hooks。第一 adapter 为 Claude settings JSON，支持无关键保留、授权 patch、source evidence；其他 manifests 无验证的 hooks 返回 Unsupported，不造通用成功。
- **Recovery:** 必交 durable layout/session intent + restart reconcile + operator-driven recreate/abandon。真实 live-process PTY handoff 是**单独 parity gate**，先做平台 probe；成功才能标为 Recoverable，不能把 recreate 写成原进程恢复。若 portable-pty/ConPTY 不支持，需要另一个明确批准的 broker/Bridge 生命周期设计；保持全量目标 OPEN。
- **Approval:** 非构造自 wire 的 per-call proof，绑定 actor/call identity/canonical descriptor revision/effective input；不能提升 TurnContext caller role。task-local 不自然传播到新 spawned task，必须测试继承拒绝。

## 3. Exact file ownership / 文件职责

| 路径 | 责任与唯一写者 |
|---|---|
| `src/gateway/pty/runtime.rs` (new) | 借用现有 managers，typed observation；线1 |
| `src/builtin_tools/terminal/capabilities.rs` (new) | descriptor-backed terminal handlers，兼容 terminal action 仅适配；线1 |
| `src/tools/registry.rs`, `src/tools/scoped/dispatch.rs`, `src/tools/turn_context.rs` | 同代 capture + proof；线1，独占审阅 |
| `shared/protocol/src/terminal.rs` (new) | session/control outcome DTO；线1 contract owner |
| `shared/protocol/src/terminal_layout.rs` (new) | 逻辑 BSP/wire mutation；线2；不得改 `workspace.rs` 另造身份 |
| `src/gateway/pty/interaction.rs`, `src/gateway/pty/control.rs` (new) | human grants / guarded agent bytes；线1 |
| `src/gateway/pty/layout.rs`, `src/gateway/pty/recovery.rs` (new) | layout reducer / persistence adapter；线2 |
| `src/gateway/pty/agent_hooks.rs` (new) | 第三方 agent config patch，不复用 Aleph hooks 当同类；线1 |
| `interfaces/tui/src/tui/terminal.rs`, `interfaces/tui/src/tui/widgets/terminal.rs` (new) | client screen cache、绘制、focus；线2 |
| `shared/client/src/terminal.rs` (new) | typed RPC facade，无业务判断；线2 |
| boot/catalog/handler registration, `shared/protocol/src/lib.rs`, TUI app/event/render module roots | 集成者唯一写入；子任务提交模块后由集成者接线 |
| `qa/terminal/`、FL/reference docs | 按任务认领；最终由集成者汇总 |

新 module 名称是决定，不留“两条路径任选”。只在同一子任务证明无法复用且给出删旧方案后加入；同功能不能 parallel truths / New adapters must replace duplicated behavior.

## 4. 双线与资源纪律 / Two tracks

```text
A0 → A1 → A2 → A3 → A4
                      ├─ line 1: B1 → B2 → B3 → B4 → B5 → B6
                      └─ line 2: A5 → A6 → A7 → C1 → C2 → C3 → C4
                                      integration barriers → C5 → C6
```

- 分配：关键安全/恢复评审仅最难处用 Opus；契约/计划与集成用 gpt-sol/kimi；单文件/测试用 DeepSeek/MiniMax/sonnet/gpt-luna；TUI/docs 用 MiniMax；简单分类用 Jev。模型可用性以实际工具为准，不伪称调用。
- 每条线独立 worktree，从当前 feature HEAD 建支线；路径锁表由集成者维护；共享 module roots 由集成者改。不能两个 agent 写同一检出。
- 每个 2–3 task wave 集成到 feature branch 再验证；任务的红→绿测试仍是必需，可在一个 test binary 的一次编译中跑多个测试，不以“省编译”为由跳过 red evidence。
- 线2每次启动 Cargo 前检查 available memory，低于 `4 GiB = 4_294_967_296 bytes` 等待；不可读取时停止并报告，不按无限内存处理。Windows 用 `GlobalMemoryStatusEx().ullAvailPhys`；Linux 用 `/proc/meminfo: MemAvailable * 1024`。其他 host 未验证则 fail-closed。
- 整个 wave 只发一张 Cargo 编译租约；各支线独立 `CARGO_TARGET_DIR`，不得互争 lock/内存。QA daemon 只允许一个 disposable home，绝不重启用户常驻实例。
- `just test-shared` 和全量 recipe 的每个内部 Cargo 都经同一检查：A0 在临时 PATH 放 Cargo shim（检查后 exec 原 absolute cargo）；不能只在启动 just 前检查一次。
- 不安装依赖、不构建产品，直到计划批准。当前阶段只提交 planning docs；baseline 的 PASS 尚无证据。

## 5. 可复制的验证命令 / Verification commands

以下均在 worktree 执行，`CG` 表示 A0 的 memory-guard Cargo shim / All commands require the guard。每个带 filter 的 test command 先 `-- --list`，确认匹配测试数 >0；零测试不得记 PASS。除 no-run 编译验证外，完整 suite 必须实际运行 / No green from empty filters:

```bash
CG test -p alephcore --lib --no-run
CG test -p alephcore --bins
CG test -p alephcore --features test-helpers --test '*' --no-run
CG test -p aleph-protocol
CG test -p aleph-client
CG test -p aleph-tui
# With the guarded cargo at the front of PATH:
just test-shared
CG test -p aleph-panel --lib
just wasm
just _stage-shell-placeholders
CG check -p aleph-desktop-windows
CG clippy --workspace --all-targets
```

A0 若发现 `.pi/rolecast.yaml`，先 `scaffolder_validate`；INVALID 则修配置前不跑 gates，valid 后按 `gate_run` 执行。没有 profile 时不擅自 scaffold，按上述真实命令验证。

QA 单独调用 `qa/terminal/run.sh identify`、`wait`、`quiet`、`cwd`、`real`、`tui`（不是一个含 `|` 的 shell 命令）。Windows 未支持的 fixture 标 UNRUN；新增 Windows ConPTY fixture 验证被用户交互授权后的输入、识别与撤销。所有 QA 从 capability face 走，旧脚本不能继续无授权用 `pty.input`。

## 6. 审阅与提交 / Review and handoff

文档提交例外：首次 docs-only commit 被已有 `.git/hooks/pre-commit` 的全仓 `cargo fmt --check` 拦截，124 个基线文件出现格式差异；clippy 未执行。暂存集只有本 spec + 四份计划，diff-check 通过。用户已明确允许**仅本次** `--no-verify`；不修改 hook/Git 配置、不格式化无关文件、不把例外延伸到实现提交 / Single user-approved documentation commit exception, not a waived code gate.

当前工具发现结果：`tool_search` 查询 subagent/delegate 无匹配，未发生实际委派。用户要求的执行方式仍是 subagents；计划获批不自动授予 native implementation 例外。正式启动前需获得可调用 Agent 工具，或用户显式批准由主 agent 按同一任务边界执行 / Delegation is unavailable, not silently substituted.


- [ ] 每任务先 red test/compile evidence，再最小实现、green、fresh reviewer；doc-only task 以自动化 key/path/placeholder 检查替代 TDD。
- [ ] 集成时 `git -c core.autocrlf=true diff --check`；代码注释 English；提交显式文件集，禁 `git add .` 把 QA artifacts 带入。
- [ ] 每任务或 wave `<scope>: <description>` commit；no main merge、no push、no worktree removal 未获授权。
- [ ] 最终状态表是 `PASS | FAIL | UNRUN | BLOCKED`，附命令与证据路径；禁止把 planned tests 当 PASS。
- [ ] full parity 的 live-process handoff、非 Claude hooks、跨平台 QA 未过时保持 OPEN；已经批准的方案不意味着可以声称全量完成。

### Plan self-review / 自审

覆盖：spec §3–§6 → A2–A7/B1–B4/C1–C4；§7 parity → C5；§8 phases → 三份子计划；§9 failure → A3/B2/B4/C4；§10 QA → A0/C5；§11 cleanup/docs → C6；§12 non-goals → 本 Global Constraints。所有拟新增接口在相应 Interfaces 中定义；未实现接口的测试失败不得误归为 baseline regression。尚未调用任何实现 skill、启动编译或修改产品代码 / Planning only。路径自审发现并修正了 CLI parser 实际路径、server connection dispatch/cleanup、TOML agent manifests；确认 20 tasks 均含 red/run-red/implement/green/commit，无未填写占位标记。配置 profile 当前不存在，实施时不擅自 scaffold。
