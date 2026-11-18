> Actual independent QUALITY reviewer: `00ad58cb-3ac1-404d-8509-4a00f30c9b62`, after completed SPEC `8315699d-cf9c-4a8c-8710-9b2273ce091b`. Verdict retained: **BLOCK / NOT COMPLETE**, solely D remains; original code P1 independently CLOSED. Original raw SHA-256: `0d0533df9b3d33a15f3f77af5bd4b316d874fa78de5a29314984d63f65c6ba06`. Only repository-absolute prefixes, if present, are normalized below; findings are unchanged. / 保留独立原文与真实 verdict，不将父代理修复改写为独立通过。

## Review

审查范围为基线 `3892804c112fd6e574166a4c03a7c40a77d0b239`、分支 `runtime-proof` 上的最终 hydration 修复；当前 43-source manifest 为记录中的 `c489ab6324cf3aeb339bdb4083827c100ff669981d7595bb73ea25f283046b8e`，完整补丁 SHA 为记录中的 `add9d9ea27910f5f371d54065e73b4b2fcf425e2c8e4a64eeb5fcd18f12d703f`。本次没有修改、暂存或提交文件。

由于受限 reviewer 工具没有执行能力，我没有重新运行 `graphify`。我先读取了父会话提供的成功 scoped graph 输出；该输出为 955 节点中的截断视图，仅展示约 53 个节点，并带有 graphify skill/package 版本差异警告。因此本次不能宣称完成完整调用图审计，只围绕已定位的 hydration seam 读取源码和报告。

- **Correct：原 QUALITY 代码 P1 已闭合。**

  当前 [chat_sidebar.rs:31](<interfaces/webchat/src/components/chat_sidebar.rs:31>) 的 `await_current_history` 在异步加载前保存：

  - connection epoch；
  - session key；
  - `sends`；
  - 完整可见 `messages`；
  - `active_run_id`；
  - phase；
  - reasoning text；
  - history limit。

  异步返回后，它通过 `try_*` 逐项比较，并在 signal 已 dispose 或任一值变化时 fail closed。完整 `messages` 比较覆盖了没有本地 send 计数变化但 assistant stream 已追加内容的情况。

- **Correct：破坏性 transcript 替换受完整加载结果保护。**

  在 [chat_sidebar.rs:283](<interfaces/webchat/src/components/chat_sidebar.rs:283>) 开始的 `hydrate_session_history` 中，`ChatApi::history` 与 `TraceApi::by_runs` 位于同一个 `await_current_history` 保护的 future 内。只有该 future 被接受后，才在 [chat_sidebar.rs:346](<interfaces/webchat/src/components/chat_sidebar.rs:346>) 清空并重放消息。

  因此原 QUALITY 报告描述的顺序——旧 history 返回、继续等待 traces、期间同会话产生新消息或 stream、旧结果最后覆盖当前 transcript——已被当前 checkpoint 拒绝。

- **Correct：可选 context estimate 没有延迟首次 transcript 呈现。**

  历史 metadata、消息、trace replay、plan 和 last-run notice 先同步提交。没有持久化 occupancy 时，代码才在 [chat_sidebar.rs:482](<interfaces/webchat/src/components/chat_sidebar.rs:482>) 之后请求 estimate，并在 [chat_sidebar.rs:486](<interfaces/webchat/src/components/chat_sidebar.rs:486>) 使用 replay 后的新 checkpoint。

  新 send、stream、phase、active run、reasoning 或 history window 发生变化时，旧 estimate 被丢弃；首次 transcript 已经完成呈现，不会被 estimate 请求阻塞。

- **Correct：回归测试覆盖了实际竞态效果。**

  [chat_sidebar.rs:54](<interfaces/webchat/src/components/chat_sidebar.rs:54>) 起的五个测试使用真实 `ChatState` effects 和 owned oneshot barriers，覆盖：

  - 新 send 加 assistant stream；
  - 没有本地 send 的 stream chunk；
  - send counter 已前进但还没有消息 bubble；
  - history window 扩大后旧请求失效；
  - 状态不变时正常接受。

  这些测试直接断言新消息和最新 stream 内容仍然存在，证明的是效果保留，而不只是 future 曾经完成。

- **Correct：修复保持了原有边界。**

  修复只改变 `interfaces/webchat/src/components/chat_sidebar.rs` 的 hydration 行为，没有增加 generation map、第二套 transport、Gateway authority 或新的状态源。`hydrate_and_follow` 仍在 [chat_sidebar.rs:694](<interfaces/webchat/src/components/chat_sidebar.rs:694>) 使用 history 返回的 active run 和 pending lane，且保留已有 route/session 检查。

- **性能取舍可接受，但应记录为实现成本。**

  每个受保护 checkpoint 会临时 clone 并比较可见 transcript，成本为 `O(visible transcript)`，且 history/traces 与可选 estimate 各有一次 checkpoint。当前代码把该成本直接放在 UI hydration 路径上；它避免了新增全局 generation 机制，也避免用旧持久化快照覆盖较新的 live projection。现有材料没有证明该 clone/equality 已造成用户可见性能回归，因此我不把它列为 P2 缺陷或要求扩大修复范围。

### 原 QUALITY P1 状态

**已闭合。**

独立 SPEC 报告 `8315699d` 对该代码 P1 的判断与本次源码复核一致。原始 QUALITY 报告中的同 epoch、同 session hydration 覆盖新 send/stream 问题，已由当前 checkpoint、受保护的 history+trace 加载、replay 后 estimate checkpoint 和五个真实状态回归测试共同覆盖。

### Finding: P1 — D 的完整验收仍未完成

**位置：**

- [docs/superpowers/runs/2026-11-16-phase9-runtime-attempt16.json:1](<docs/superpowers/runs/2026-11-16-phase9-runtime-attempt16.json:1>)
- [docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:33](<docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:33>)
- [docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:35](<docs/superpowers/runs/2026-11-16-phase9-execution-receipt.md:35>)
- [docs/superpowers/runs/2026-11-16-phase9-closeout.md:17](<docs/superpowers/runs/2026-11-16-phase9-closeout.md:17>)

runtime16 的 Tauri 与 NSH 记录都包含：

- `resource_complete: false`；
- `performance.status: OPEN`；
- 资源范围仅覆盖 schema/marker correlation 与 direct child exit；
- 尚未证明 launchd-owned WebKit XPC 进程的每实例归属和清理关系。

因此当前证据仍不足以完成：

- WebKit XPC 的完整 RSS/thread 归属；
- 有效的 10 次 warmup + 30 次 measured 对照；
- 完整 parity threshold acceptance。

这仍是外部验收证据阻塞，不应解释为性能阈值已经失败，也不应改写为 `NOT_ACCEPTED`。最小后续动作仍是先取得可验证的每实例 WebKit ownership/cleanup 证据，再按既定 Tauri-first / NSH-second 顺序执行完整 10+30 采样并交给现有 reducer。

### 验证边界与跳过项

本次复核：

- 未运行 Cargo、Python、Node 测试或 clippy；
- 未启动、终止或附加任何进程；
- 未重新计算 source manifest、补丁、WASM、binary、HTTP payload 或日志 hash；
- 未读取受保护的 `.claude/settings.json`、根目录 `config.lock` 或根目录 `config.toml`；
- 未执行 WebKit XPC ownership 验证；
- 未执行新的 10 warmup + 30 measured 性能验收；
- 未宣称 release、跨平台 NSH、Tauri/WebChat 移除、detached daemon ownership、Safe Replay、external-effect exactly-once 或 universal scheduler 已完成；
- 请求的 `docs/superpowers/runs/2026-11-16-phase9-final-integrity.md` 不存在，因此没有声称读取该文件；
- 父会话记录的 1,567/0/0 Panel 结果、canonical default/QA build、runtime16、default restoration，以及 24 个选定 Rust 文件的 targeted rustfmt 结果均作为已记录证据引用，未声称由我亲自执行；
- 父会话还记录了递归 rustfmt 在未改动基线文件 `src/extension/runtime/wasm/host_functions.rs:344` 和 `src/extension/runtime/wasm/host_functions.rs:433` 的差异；这不应被表述为当前 hydration 修复引入的格式问题或全仓格式化通过。

## Merge verdict

**BLOCK / NOT COMPLETE。**

代码质量方面，原 QUALITY P1 已独立确认闭合，当前没有发现新的、由该 hydration 修复引入且有具体证据支持的代码缺陷。整体 Phase9 仍因 D 的 WebKit 资源归属和有效 10+30 性能验收未完成而保持 BLOCK；不能据此宣布 Phase9 overall PASS。