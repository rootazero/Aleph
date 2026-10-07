# Terminal Capability Census / 终端能力清点

**Branch / 分支:** `feat/terminal-capability-parity`

**Implementation baseline / 实施基线:** `dba2cfce496152c31934f4af34c4f41666151b64` (approved documentation commit; product source unchanged from `a5300221d27c7a916407d8b96cadccb91cde1bbf`)

**Plan / 计划:** `docs/superpowers/plans/2026-10-06-terminal-capability-parity.md`

**Status / 状态:** A0 Core baseline passed with a user-approved, process-local Windows paging strategy; A1 is committed. A2 borrowed observation runtime and targeted validation passed. The normal commit hook is blocked by pre-existing formatting differences; the user explicitly authorized a new exception for only this batch's eight files. No A3 or later stage is implemented by this session; this is not full parity. / A0 已解除资源阻塞，A1 已提交；A2 定向验证已通过，正常 hook 被既有格式差异阻塞，用户新授权仅本批 8 文件的提交例外，不等于完整 parity。

## 1. Current wiring / 当前连线

Counted from source, not inferred from names / 按源码计数，不按名字推断：**7 legacy RPCs, 5 read-only tool actions, 3 event topics**。

| Face / 入口 | Source / 当前写者 | Authority and state / 权限与状态来源 | Planned convergence / 收敛方式 |
|---|---|---|---|
| `pty.spawn` | `src/bin/aleph-server/commands/start/builder/handlers/system.rs:47` → `src/gateway/handlers/pty.rs::handle_spawn` | Live terminal config, cwd jail, actor stamp → existing `PtyManager::spawn` / 实时配置、cwd jail、身份戳 | B4 governed lifecycle capability; retain thin compatibility adapter / B4 生命周期能力及薄兼容适配 |
| `pty.input` | `src/gateway/handlers/mod.rs:415` → `src/gateway/handlers/pty.rs::handle_input` | Operator RPC gate + ownership; raw `PtyManager::write`, no command-grained enforcement / 原始字节输入 | B1/B2 explicitly granted human input; B3 separate agent control, never a tool bypass / B1/B2 人类授权输入，B3 agent 控制 |
| `pty.resize` | `src/gateway/handlers/mod.rs:416` → `src/gateway/handlers/pty.rs::handle_resize` | Operator + ownership; connection-scoped smallest viewport / 按连接管理 viewport | B4 lifecycle adapter over existing manager; no duplicate viewport store / 保留已有 viewport 真源 |
| `pty.close` | `src/gateway/handlers/mod.rs:417` → `src/gateway/handlers/pty.rs::handle_close` | Operator + ownership; close currently removes registry entry before observed exit / 关闭请求不等于进程回收 | B4 settlement-aware lifecycle; do not report reaped merely because kill was sent / B4 验证实际结算 |
| `pty.list` | `src/gateway/handlers/mod.rs:418` → `src/gateway/handlers/pty.rs::handle_list` | Owner-filtered `PtyManager::list`; typed `PtyListResponse` / 带所有权过滤 | A1/A2/A4 typed contract, shared read runtime and canonical dispatch / 契约及只读调度收敛 |
| `pty.attach` | `src/gateway/handlers/mod.rs:419` → `src/gateway/handlers/pty.rs::handle_attach` | Owner-filtered atomic screen/seq snapshot / 原子 screen/seq 快照 | A2/A4 reuse attach; A5 reattach after frame gap / 复用快照及断帧重附加 |
| `runtime.agents.list` | `src/gateway/handlers/mod.rs:424` → `src/gateway/handlers/runtime.rs::handle_list` | `RuntimeAgents::snapshot` filtered through manager ownership / 状态与所有权结合 | A2/A4 capability read projection; no second sampler / 不另建采样器 |
| `terminal` actions `list/read/status/wait/explain` | `src/executor/builtin_registry/definitions.rs:253`, `src/executor/builtin_registry/builder/constructor/mod.rs:869`, `src/executor/builtin_registry/registry/tool_registry_impl.rs:1655`; implementation `src/builtin_tools/terminal.rs` | Inline operator gate; stricter `terminal_admits`; wait uses runtime watch / 内联权限闸、严格所有权、watch 等待 | A1–A4 typed contract, shared read implementation, call-bound proof and canonical descriptors; preserve outer compatibility surface / 保留兼容工具，消除重复业务 |
| `pty.screen`, `pty.exit` | `src/gateway/pty/manager.rs`; wire DTOs `shared/protocol/src/pty.rs` | `src/gateway/event_scope.rs` operator gate + `src/gateway/event_visibility.rs` PTY ownership / 双重事件过滤 | Keep existing event stream and typed wire; test actual delivery effects / 保留事件流，验证送达效果 |
| `runtime.agents.changed` | `src/gateway/runtime/mod.rs`; wire `shared/protocol/src/runtime.rs` | Operator-only empty invalidation; clients refetch filtered list / 空失效通知，再拉取带过滤列表 | Keep existing runtime generation watch; no payload/state fork / 不分叉状态 |

`HandlerRegistry` routes wire methods; it is not a new callable capability source. `ToolHandlerRegistry` is the callable source; `ToolCatalog` and clients are projections. `CapabilitySlot<T>` is installation/outcome instrumentation, not another callable registry. / Gateway 路由表不是新的能力真源；可调用真源、目录投影和安装仪表不得混淆。

## 2. Reuse and invariants / 复用与不变量

- `src/gateway/pty/manager.rs` and `src/gateway/pty/session.rs` remain the PTY session truth. `src/gateway/runtime/mod.rs` remains the agent-observation truth. / 保留会话与观测真源。
- The sole flush clock is 16 ms. Foreground probing retains 500 ms minimum, 3000 ms recheck and 6 misses to forget. Cwd precedence stays OSC7 > foreground-process cwd > spawn cwd. `quiet_since` must never imply Idle. / 不增设时钟，不由静默推导 Idle。
- `shared/protocol/src/pty.rs` owns screen frames, atomic attach and sequence contracts. A sequence gap means reattach, not silently accepting the next patch. / 断帧必须重附加。
- `interfaces/tui/` depends on `aleph-client` and `aleph-protocol`, not `alephcore`. Its current runtime refresh/subscription lives at `interfaces/tui/src/tui/mod.rs:136` and `interfaces/tui/src/tui/mod.rs:163`; full PTY workspace interaction is not yet wired. / TUI 仍为薄 I/O 客户端。
- `shared/protocol/src/workspace.rs` models AgentEnv workspace CRUD, not terminal BSP layout. C1 will use distinct terminal-workspace DTOs rather than overload it. / 不混用不同 workspace 领域。
- `src/sandbox/worktree.rs` and existing session/process-journal infrastructure are reuse candidates. Journal recorders can no-op when disabled; they cannot be treated as the sole mandatory write-intent barrier. / journal 不能冒充必达意图屏障。

## 3. Safety seams / 安全接缝

1. A2 `src/gateway/pty/runtime.rs::ObservationCaller` preserves different admission: actorless Gateway retains `SessionOwner::admits(None)` including Unknown owner rows; actorless Tool admits only known-unowned sessions and denies Unknown. Identified callers require exact ownership on both faces. / 不静默统一或放宽所有权规则。
2. `src/tools/scoped/dispatch.rs` can rewrite effective input after an earlier approval. Terminal proof must bind final input, actor, call identity and descriptor/handler generation; never restamp caller role as operator. / 证明绑定最终参数，不提升环境角色。
3. Capture handler and descriptor from the same `ToolHandlerRegistry::snapshot_state().entry(name)`; separate lookups can cross generations. / 同代快照取配对数据。
4. Raw human input is an explicitly approved exception, **not** per-command sandbox enforcement. The grant must be actor/connection/session-generation bound, revocable and nondelegable. Agent/LLM/background calls cannot inherit it from a wire flag or UI label. Starts still require command policy; agent control remains default-off. / 人类交互授权不能流入自动化。
5. `PtySession::kill` currently ignores killer errors and sets closed; `PtyManager::close` removes before observed settlement. B4 must distinguish requested close, failure and reaped exit. / 请求、失败和实际回收必须分离。

## 4. Reference mapping / 参考映射

| Reference / 参考 | Useful pattern / 有效模式 | Aleph gap / 差距 | Mapping and trade-off / 映射与代价 |
|---|---|---|---|
| herdr | Session/workspace/tab/BSP organization, agent APIs, lifecycle hooks, runtime handoff / 多 agent 终端工作台 | TUI lacks terminal workspace; writes/hooks/recovery deferred / TUI 与生命周期缺口 | Reuse Aleph PTY/VT; Core layout tree, typed capability projections and strict agent control. No libghostty import or copied second registry. Platform handoff needs separate evidence. / 不照搬 VT 与注册表 |
| Orca | Terminal stream/runtime audit, admission controls, input/takeover separation / 流与接管边界 | Raw RPC input is not call-bound capability input / 输入权限未收敛 | Bind authorization to canonical dispatch and actual connection; preserve backpressure/ownership and test effects. Approval may add interaction latency. / 审批增加延迟 |
| Paseo | Typed snapshot/restore/stream/input protocols and workspace recovery / 类型化快照与恢复 | TUI has no screen resync consumer; recoverable identity not equivalent to live PTY / 恢复语义不完整 | Shared wire types, resync-on-gap, explicit metadata/history vs live-runtime recovery states. Windows cannot claim Unix FD adoption. / 跨平台恢复有明确边界 |

Graph navigation is a hint, not fresh source evidence / 图谱仅作导航：
- Aleph main-checkout `graphify-out/GRAPH_REPORT.md`: commit `0228b7b1`, 2544 nodes / 6690 edges; absent in the new worktree. Current task baseline is newer.
- `T:/Github/herdr/graphify-out/GRAPH_REPORT.md`: commit `d6b40d4e`, 37025 nodes / 76647 edges.
- `T:/Github/orca/graphify-out/GRAPH_REPORT.md`: commit `0d2300ca`, 169568 nodes / 550841 edges.
- `T:/Github/paseo/graphify-out/GRAPH_REPORT.md`: commit `bd3986d9`, 48724 nodes / 117123 edges.

Do not present the graph commit as the current reference HEAD or performance evidence. / 不以图谱版本冒充当前代码或性能证明。

## 5. A0 evidence / A0 验证

| Check / 检查 | Result / 结果 |
|---|---|
| Python memory-guard tests | Observed RED: `ModuleNotFoundError: No module named 'memory_guard'`; then GREEN; expanded suite now passes 15 tests / 已观察失败转通过，扩充后的 15 项测试通过 |
| Memory threshold | `4 * 1024**3` bytes; strictly below refuses; exact threshold admits / 低于拒绝，边界放行 |
| Unknown/malformed probe and child failure | Fail closed before spawn; child exit code preserved / 未知拒绝，保留退出码 |
| Windows core library baseline | Cancelled for host memory safety; exit 1, no compiler error diagnostic observed. `CARGO_BUILD_JOBS=2` with root-crate `-C debuginfo=2` reached only `443_793_408` available physical bytes / 因主机内存安全取消，未通过基线 |
| Lower-memory core retry | User approved jobs=1 and package-only `debug=0`, `incremental=false`; observed actual rustc had neither debuginfo=2 nor incremental. Still reached `1_463_083_008` available bytes; owned guard process tree cancelled, exit 1, no compiler error diagnostic / 参数确已生效，仍因资源保护取消 |
| Protocol and TUI baselines | Passed under the memory guard: protocol 371 tests, TUI library 454 tests and TUI CLI definition 1 test; protocol doc tests retain 2 existing ignored cases / 小包基线通过，2 项既有协议文档测试仍忽略 |
| Unix branch / real TUI / Panel / parity | Not validated by A0; later task/platform evidence required / A0 未证明 |

Local build logs live under ignored `.superpowers/sdd/2026-10-06-terminal-capability-parity/`; they are evidence files, not new product persistence. / 本地日志不提交，也不是产品持久化。

The memory guard is **admission only**, not a RAM reservation. On this cold Windows build, the single `alephcore` test compilation exhausted the margin even after dependency jobs narrowed to one rustc. Only the verified owned tree (Cargo shim PID 8756 → Cargo PID 2792 → rustc PID 9988, whose command carried this worktree's target path) was terminated. Available physical memory recovered to `14_261_317_632` bytes. This is a resource cancellation, not evidence of a source compilation defect or successful baseline. The subsequently approved package-only low-memory retry reused dependencies and compiled only `alephcore`, but still crossed the 2 GiB safety margin. Its owned guard PID 7812 and descendants were terminated after PID, command and creation-time validation. Exit was `{"code":1,"signal":null}`; available memory recovered to `14_332_420_096` bytes. Neither attempt proves a source defect or a passing baseline. At that point the user approved only A1 after protocol/TUI baselines passed; A2 remained blocked until the subsequent successful baseline below. Full parity and later-stage verification remain required. / 两次编译都未完成；只取消已核验的本任务进程，不把取消冒充源码错误或测试通过。

Isolation check also found unrelated-looking current main-checkout changes in `package.json` and `package-lock.json`: `@earendil-works/chord`, `@earendil-works/pi-ai`, `@earendil-works/pi-durable` changed from `^1.0.2` to `^1.0.4`. Attribution is unconfirmed; this task did not execute writes or restoration on those files. Preserve them and do not claim main is currently clean. / 主检出出现依赖升级，归因未确认，保留原样；不再宣称 main clean。

### Subsequent A0 resource resolution / 后续 A0 资源解阻

- A third process-local experiment used jobs=1, entire `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_TEST_INCREMENTAL=false`, the same target cache and unchanged features/source. It was cancelled at `2_667_728_896` available physical bytes after 1295.1s by the experiment's own `2_684_354_560`-byte floor; not a source error or PASS. / 第三次物理内存保护取消也不算基线。
- Crosschecked `GlobalMemoryStatusEx` with Windows counters: available RAM already includes reclaimable standby cache; no cache purge or false admission value was used. / available 已包含可回收缓存，未清缓存或虚报数值。
- User explicitly approved allowing Windows paging for the same command/profile. QA admission stayed at `4_294_967_296` physical bytes; only the disposable experiment's runtime stop policy changed to unknown probe, commit headroom below `4_294_967_296`, or 1800s deadline. No Cargo config, product dependencies, source or features were changed. Cost: disk I/O, longer builds and possible reduced host responsiveness; the guard is still not a reservation/watchdog. / 仅局部实验改变运行期保护，不改变 QA 准入闸。
- Actual guarded `cargo test -p alephcore --lib --no-run -v` completed: exit 0, 724.01s, minimum physical `430_252_032`, minimum commit headroom `15_678_787_584` bytes. The test executable was produced; this is compilation, not the full Core suite. / 真实测试 crate 编译完成，不冒充全量测试。
- Actual guarded `cargo test -p alephcore --bins` baseline: 100 PASS, no ignored; targeted baseline terminal 24 PASS, PTY 138 PASS + 1 existing ignored, runtime filter 42 PASS. Only after these results did A2 begin. / 先过 Core 基线再开始 A2，无新验证豁免。

## 6. A1 wire evidence / A1 契约验证

- `shared/protocol/src/terminal.rs` defines read/wait/explain payloads and session/wait params; `shared/protocol/src/lib.rs` exports the module. `RuntimeAgentEntry` and `RuntimeAgentState` are reused, not copied. / 复用既有 agent wire 类型。
- Observed RED: `error[E0422]: cannot find struct, variant or union type \u0060TerminalReadResponse\u0060 in this scope`, then GREEN: 5 targeted tests passed. / 已观察缺失类型失败，再通过 5 项针对性测试。
- Guarded regression: protocol 376 tests, client 25 tests, TUI library 454 tests and TUI CLI definition 1 test passed; the 15 memory-guard tests also passed again. / 受资源闸保护的回归及 15 项资源闸测试通过。
- `cargo clippy -p aleph-protocol --all-targets -- -D warnings` and the new module's `rustfmt --check` passed. Package-wide formatting remains blocked at unchanged `shared/protocol/src/projects.rs:348`; normalized file contents match HEAD. No unrelated formatting was applied. / 当前模块格式与协议包 clippy 通过；包级格式被与 HEAD 一致的既有文件阻塞，未扩大修改范围。
- Fixtures pin legacy JSON key sets and reason omission; they are **not** actual live server producer tests. Core producer migration/compatibility is deferred to A2. / fixture 不冒充实际服务端构造与兼容验证。
- Shared params preserve omitted/empty `until` and requested timeouts. Defaults, empty-state rejection and clamping stay in Core; no duplicate wire-side execution policy was added. / 参数保留原始形状，默认值、空集合拒绝及超时钳位由 Core 统一执行。
- Rolecast validation could not start: `ModuleNotFoundError: No module named 'yaml'`. No workflow profile or unrelated dependency was installed; verification used existing Cargo commands. / 验证工具缺少外部依赖，未擅自安装或新建工作流配置。
- User explicitly authorized `--no-verify` only for this batch's 5 files in two commits: the 2 QA guard files, this audit and the 2 protocol files. No hook was modified. This does not authorize later bypasses, waive final Core validation or unlock A2. / 用户仅授权本批 5 文件两次提交的有限绕过，不修改钩子，不延伸至后续提交、最终 Core 验证或 A2。

## 7. A2 convergence evidence / A2 收敛证据

- `src/gateway/pty/runtime.rs::TerminalRuntime<'a>` borrows `PtyManager` and `RuntimeAgents`; `ObservationCaller::{Gateway,Tool}` preserves caller provenance and existing ownership admission. `src/gateway/handlers/runtime.rs` and the five legacy terminal tool actions use the shared observation implementation. Legacy `pty.*` adapters remain until A4. / 只复用已有 store，未新增 singleton、clock 或 callable registry。
- Core now constructs `TerminalReadResponse`, `TerminalWaitResponse`, `TerminalExplainResponse` and the existing list/status/attach DTOs. Tool schema, operator gate, envelope, exact refusal text and `lost_with_restart` remain in `src/builtin_tools/terminal.rs`. / Core producer 已迁移，不只展示类型。
- Removed duplicate read/status/list/wait/explain production logic and private output structs from `src/builtin_tools/terminal.rs`. Only necessary tool/journal adapters remain in production; test compatibility helpers are `#[cfg(test)]`. Sampling, flush clock, foreground probing and cwd precedence were untouched. / 清理旧实现，不保留第二个生产业务真源。
- Observed lifecycle RED: `closed_pty_wins_over_a_stale_matching_agent_row` returned `Reached(Working)` instead of `Gone` after real manager close while the sampled row remained. Core now checks PTY registration before consuming a row. Legacy reached/timeout fixtures now own real PTYs with Drop cleanup rather than invent IDs outside the registry. / 已观察真实 RED，再修生命周期判定；gone 不等于进程已回收。
- Fresh guarded test filters, each with nonzero `--list`: `gateway::pty::runtime::tests` **9 PASS**; `builtin_tools::terminal` **27 PASS**; `gateway::pty` **147 PASS + 1 existing ignored** (includes the 9 new tests); `gateway::runtime` **42 PASS**; `gateway::handlers::runtime` **3 PASS**. These overlapping counts must not be summed as unique coverage. / 过滤有实际命中，ignored 不算通过。
- Producer tests pin read/wait/explain serialized key sets/outcomes, exact multiline tail, defaults `[Blocked, Idle]`, quiet staying Working, Alice/Bob ownership, actorless Tool vs Gateway and Unknown owner semantics. Cancellation returns `ToolError::Cancelled`; the same future wakes on the existing generation watch without timer polling or screen resampling. Real `TerminalTool.call` tests pin owner/bob/actorless tombstone envelopes. / 断言效果，不仅断言调用。
- Process-local validation profile: `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_PROFILE_TEST_INCREMENTAL=false`, target `<worktree>/target/terminal-parity`. Every Cargo invocation uses the fail-closed physical-memory admission guard; no overlapping builds. Fresh Core lib `--no-run` PASS; guarded `cargo test -p alephcore --bins` **100 PASS** (compile 7m12s), memory guard unittest **15 PASS**, targeted `rustfmt --edition 2021 --check --config skip_children=true` on the five changed Rust files and `git -c core.autocrlf=true diff --check` **PASS**. Sequential runner exit **0**. Production build reports the new runtime `attach` method as unused until A4 wiring; it is exercised by tests, not yet connected to the legacy PTY RPC. / 局部环境，不改持久配置或 feature。
- Not validated here: full Core library suite, `test-helpers` integration compilation, Panel/TUI live QA, Unix PTY branch, desktop platform checks or workspace all-target clippy. The normal commit hook was actually attempted with guarded Cargo: admission `15_689_998_336` bytes, exit **1** at `cargo fmt -p alephcore -p aleph-cdp -p aleph-desktop -p aleph-desktop-macos -p aleph-desktop-linux -p aleph-desktop-windows -- --check`; clippy was not reached. Its 310 diff hunks cover 121 files, every one verified equal to HEAD after newline normalization (none belongs to the A2 changes). No hook modification or unrelated formatting was performed. After reviewing this evidence and the unrun clippy, the user explicitly authorized `--no-verify` only for the five A2 Rust files and these three documentation files. This new exception grants neither future bypasses nor final Core/whole-parity validation exemptions. A3 approval proof, A4 canonical RPC dispatch and all later grants/control/hooks/layout/worktree/recovery stages remain unimplemented. / 未做项明确保留，不宣称 full parity。

## 8. Execution and cleanup / 执行与清理

Initially the user approved primary-agent implementation because no Agent/subagent tool was available. After installing pi-agents, implementation and read-only review were delegated. One follow-up implementation agent stopped without a result; the user approved primary-agent takeover. Independent reviews identified wire newline/error-text/schema/admission regressions and a stale-row lifecycle defect, corrected before final validation. No stopped agent is counted as a successful review or verification. / 工具安装后使用了真实子代理；无结果的中止不算完成，后续获准由主 agent 接管。

Follow A → B → C plan dependencies. A1–A4 consolidate typed contracts, read logic, proof and registration before A5–A7 wire clients/TUI and verify multi-client compatibility; B1–B6 add grants/input/agent control/lifecycle/hooks; C1–C6 add layout/worktree/recovery/parity QA and cleanup. / 先收敛与连线，再扩展。

Cleanup belongs with each replacement: remove duplicate terminal schema/dispatch authors only after typed compatibility adapters and consumers pass; retire old direct automation write paths when B grants/control land; never remove wire safety, ownership filters or live-config checks merely to make convergence easier. / 清理跟随已验证替换，不能以删除权限闸换取收敛。
