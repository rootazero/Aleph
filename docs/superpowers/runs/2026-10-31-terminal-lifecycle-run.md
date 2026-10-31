# Terminal source-owned lifecycle run / Terminal 源所有生命周期实施记录

Date / 日期：2026-10-31
Status / 状态：Complete; focused validation passed and the independent review correction is approved. / 已完成；专项验证通过，独立审查修正已获通过。

## Authorization and baseline / 授权与基线

The implementation request approves the existing [design](../specs/2026-10-31-capability-core-terminal-lifecycle-design.md) and [plan](../plans/2026-10-31-capability-core-terminal-lifecycle-plan.md). Approval was recorded before production edits in `1505d06e5`. The implementation worktree is `main`, starting from `fae49f506`; it contains `023a552da` and `fc1eda686`.

用户实施指令批准上述既有 design 与 plan。生产修改前以 `1505d06e5` 记录授权。实施检出为 `main`，起点 `fae49f506`，已包含 `023a552da` 与 `fc1eda686`。

Implementation commit / 实现提交：`2372b2c22 feat(terminal): own PTY provider lifecycle`. This record and the reference alignment are a separate documentation commit; neither commit is pushed or remotely merged. / 本记录与 reference 校对单独形成文档提交；两个提交均不 push 或远端 merge。

## Scope and authority / 范围与权威

- `Config::policies.terminal.enabled`: enabled intent. / enabled 意图。
- Existing `PtyManager`: sessions, owners, retained owners, journal integration and event-bus delivery. / 现有 session、owner、owner retention、journal 接线与 event-bus delivery。
- One manager-owned, side-effect-free `ProviderHandle`: provider state, generation and admission. / 一个 manager 所有、无副作用的 `ProviderHandle`：provider 状态、generation 与 admission。
- Manager-owned flush driver: activation, stop signal and join evidence. / manager 所有的 flush driver：激活、停止信号与 join evidence。

Session ownership is not migrated to `OwnershipTree`. No second registry, owner map, enabled-state map, runtime table or durable store is authorized.

Session ownership 不迁入 `OwnershipTree`。不授权第二 registry、owner map、enabled-state map、runtime table 或 durable store。

## Reference review / 参考核对

The local `deepseek-harness` terminal implementation was reviewed for owner checks, publication after backend setup and retaining failed cleanup evidence. Its separate Terminal service/registry is not copied. The research documents `docs/research/aleph_modularization_plan.md` and `docs/research/Everything-externally-composable-is-a-Capability.md` inform lifecycle and authority boundaries; their broader capability goals do not authorize additional mounts in this task.

已核对本机 `deepseek-harness` Terminal 的 owner 检查、backend setup 后发布与失败清理证据保留；不复制其独立 Terminal service/registry。上述两份 research 文档用于核对 lifecycle 与 authority 边界，其更广泛 capability 目标不构成本任务额外 mount 授权。

Consumer review identified router boot, live config apply, WebSocket disconnect, normal/forced shutdown and PTY event visibility as integration boundaries. Existing owner retention and journal fallback must remain intact. File-watcher reload is distinct from the existing explicit config patch/reload/self-config live-apply path.

消费者审计确认接线边界包括 router boot、live config apply、WebSocket disconnect、normal/forced shutdown 与 PTY event visibility。既有 owner retention 与 journal fallback 必须保留。文件 watcher reload 与显式 config patch/reload/self-config live-apply path 不等价。

## Implementation and proof / 实现与证据

Implemented in the existing `PtyManager`: `OnceLock` plus `try_manager` preserve cold access; one `ProviderHandle` gates generation and admission; one retained flush `JoinHandle` is owned before `Ready`. Operations and frame commits use the captured generation. `deactivate` revokes admission, calls the existing session cleanup path and awaits the retained handle; cancelling the caller leaves the handle in the source. Failed kills retain their sessions in the canonical map, and failed join/cleanup evidence remains observable.

实现复用现有 `PtyManager`：`OnceLock` 与 `try_manager` 保持 cold access；一个 `ProviderHandle` 控制 generation/admission；发布 `Ready` 前已持有唯一 flush `JoinHandle`。操作和 frame commit 检查捕获的 generation。`deactivate` 撤销 admission、执行现有 session cleanup 并等待保留的 handle；取消调用者不会丢失该 handle。kill 失败的 session 保留在原 map 中，join/cleanup 失败证据继续可见。

Router wiring reads the existing shared application `Config`. Disconnect and shutdown avoid initializing an absent manager. Synchronous live apply requests stop and reports pending join as non-live/restart-required; a later apply/activation or async shutdown polls the same handle. Cold enable without an existing event bus also requires restart. No detached cleanup task or second enabled authority was added. This is the explicit fallback allowed by plan Task 3, not a claim of immediate completion for every live apply.

Router 接线读取既有共享 application `Config`。断连与 shutdown 不初始化尚不存在的 manager。同步 live apply 请求停止，并将 pending join 如实报告为 non-live/需要重启；后续 apply/activation 或异步 shutdown 继续 poll 同一 handle。缺少既有 event bus 的 cold enable 同样需要重启。未新增 detached cleanup task 或第二 enabled authority。这是 plan Task 3 允许的明确降级，不表示所有 live apply 都即时完成。

Test-first evidence: three initial lifecycle tests failed with missing APIs (`E0599`, 16 compiler errors) before production implementation. The rest of the lifecycle/failure matrix was added during implementation rather than all being demonstrated RED first; this is a process deviation. Main acceptance separately reproduced a failing adjacent event-visibility fixture (cold provider rejects spawn), then adapted it to explicit activation and real child cleanup.

测试先行证据：最初三个 lifecycle 测试在生产实现前因缺少 API 而失败（`E0599`，16 个编译错误）。其余 lifecycle/failure 矩阵在实施过程中补齐，未逐一先演示 RED；这是流程偏差。主验收另行复现了相邻 event-visibility 夹具失败（cold provider 拒绝 spawn），再将其改为显式 activation 与实际 child cleanup。

## Validation / 验证

The final combined focused run passed **194 tests, 0 failed, 0 ignored**, including the natural-exit correction and mixed activation/disable regression:

最终组合专项验证通过 **194 项，0 failed，0 ignored**，包含 natural-exit 修正与 activation/disable 混合竞争回归：

| Scope / 范围 | Passed / 通过 |
|---|---:|
| `gateway::pty` | 140 |
| `gateway::handlers::pty` | 23 |
| `config::live_apply` | 14 |
| `config::types::policies::terminal` | 4 |
| `capability::provider` | 10 |
| Terminal router boot / Terminal router 启动 | 2 |
| PTY event visibility / PTY 事件可见性 | 1 |

The worker separately ran the required focused commands before the review correction (PTY 138, handlers 23, live apply 14, policy 4, provider 10 and boot 2). Main acceptance reran all those filters together plus event visibility after the final Rust correction, with `CARGO_BUILD_JOBS=1 cargo test -p alephcore --lib -- gateway::pty gateway::handlers::pty config::live_apply gateway::server::tests::terminal_ gateway::event_visibility::tests::pty_frames_reach_only_the_owner_and_outlive_the_session capability::provider config::types::policies::terminal`. The memory gate passed before each Cargo invocation. Final `cargo check -p alephcore --tests`, `rustfmt --check --edition 2021 --config skip_children=true` over all 12 affected Rust files, and `git diff --check` passed. Existing Panel asset dead-code warnings and the macOS linker large-`__eh_frame` warning remain. No full workspace test suite, clippy, live browser/Panel QA, Windows/Linux run or native process/thread/RSS measurement was performed.

实施者在审查修正前分别执行必需专项命令（PTY 138、handlers 23、live apply 14、policy 4、provider 10、boot 2）。主验收在最终 Rust 修正后，以上述单 job 命令组合重跑这些 filter 并加入 event visibility。每次 Cargo 调用前 memory gate 均通过。最终 `cargo check -p alephcore --tests`、全部 12 个受影响 Rust 文件的 `rustfmt --check --edition 2021 --config skip_children=true` 与 `git diff --check` 均通过。仍有既有 Panel asset dead-code 和 macOS linker large-`__eh_frame` 警告。未执行完整 workspace test suite、clippy、live browser/Panel QA、Windows/Linux 运行或 native process/thread/RSS 测量。

## Review / 审查

Initial independent review was **Needs fixes**. It identified a natural-exit/stop race: a reaped child could return `No such process (os error 3)` while its exit callback waited for admission, permanently retaining a failed session. The real-child regression reproduced that failure before the correction. `PtySession::child_exited` now records only successful `child.wait()` evidence; requested closure and failed kill do not set it. Cleanup accepts an already observed exit, while kill errors without that evidence still retain residual state. A mixed activation/disable race test was also added.

首轮独立审查为 **Needs fixes**：child 已被回收、exit callback 尚在等待 admission 时，重复 kill 返回 `No such process (os error 3)`，使 session 永久保留为失败。实际 child 回归在修正前复现此错误。现在 `PtySession::child_exited` 只记录成功 `child.wait()` 的证据，请求关闭或 kill 失败均不设置它。Cleanup 接受已观察到的退出；缺少该证据的 kill 错误仍保留 residual。另补 activation/disable 混合竞争测试。

Targeted independent re-review: **Approved**; the original P2 is resolved and no new concrete defect was found in the correction. The reviewer read the RED evidence but did not run Cargo. Main acceptance independently confirmed the final 194-test GREEN and required package/format/diff checks. A real WebSocket stop/re-enable interleaving during asynchronous visibility filtering was not exercised; predicate coverage and inspection of both forwarding guards do not exhaust all schedules.

独立定向复审：**Approved**；原 P2 已解决，修正未发现新的具体缺陷。审查者读取 RED 证据但不运行 Cargo；主验收已独立确认最终 194 项 GREEN，以及必需 package/format/diff 检查。未执行实际 WebSocket 在异步 visibility filter 等待期间 stop/re-enable 的交错测试；predicate 覆盖及两处 forwarding guard 的源码核对不穷尽所有调度。

## Deferred and unproven / 延期与未证明边界

No Terminal Zahir/BackendLease/MCP/ACP projection, Generic Gateway Zahir mount, Feishu/Memory/Sandbox work, new `CapabilityKind`, subscription, durable Capability recovery, EffectClaim, Safe Replay, universal scheduler or harness change is included.

不包含 Terminal Zahir/BackendLease/MCP/ACP projection、Generic Gateway Zahir mount、Feishu/Memory/Sandbox、新 `CapabilityKind`、subscription、durable Capability recovery、EffectClaim、Safe Replay、universal scheduler 或 harness 修改。

The session cleanup evidence is a successful kill request or a successful waiter-observed child exit; flush-driver join is separate evidence. This does not establish complete descendant-process/reader-thread reclamation, native unload, RSS reduction, process isolation, external delivery or exactly-once effects. No remote push or merge is authorized.

Session cleanup 的证据是 kill 请求成功或 waiter 成功观察到 child 退出；flush-driver join 是独立证据。这不证明完整 descendant-process/reader-thread reclamation、native unload、RSS reduction、process isolation、external delivery 或 exactly-once effects。不授权 remote push 或 merge。
