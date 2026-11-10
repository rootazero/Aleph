# Findings — EE2 (execution engine part B + run_loop)

Files reviewed: 26 (21248 LOC)

Counts: P0= 0 P1= 1 P2= 1 P3= 0

Static review only / 仅静态审查：已逐一复核候选项并审阅批次全部文件；未修改源码，未运行 cargo、rustc 或测试。跨批次读取仅用于下述调用方和缓存生命周期契约核实。

### EE2-01 [P1] Direct-tool fast path never observes run cancellation or timeout

- **Location**: `src/gateway/execution_engine/slash_command.rs:399-403` (`ExecutionEngine::execute_direct_tool`)
- **Failure mode**: 用户执行获准走 fast path 的 direct-tool slash command，工具调用尚未返回时取消 run，或设置比工具耗时更短的 `timeout_secs` → 调用仍继续等待工具返回，取消消息无人消费，run timeout 也根本没有启动。慢工具会在取消后继续执行；不返回的工具则持续占用 session slot 和 concurrency permit，后续同 session 请求无法正常接替。取消通道容量为 1，因此重复调用 cancel 还可能阻塞在向已满通道发送消息上。
- **Evidence**:
  ```rust
      self.tool_registry.execute_tool(tool_id, arguments),
  );
  match execution.await {
  ```
  调用方契约已跨批次核实：`src/gateway/execution_engine/execute.rs:768-778` 直接 await `execute_slash_command_fast_path`；fast path 成功/失败从该分支提前结束，只有 fallthrough 才到 `src/gateway/execution_engine/execute.rs:858-912` 创建 deadline 并进入监听 `cancel_rx.recv()` / `wait_for_deadline` 的 `tokio::select!`。`src/gateway/execution_engine/execute.rs:468` 的取消通道容量为 1，`src/gateway/execution_engine/execute.rs:495` 已持有 run-lifetime admission guard；`src/gateway/execution_engine/engine.rs:520-533` 的 cancel 仅发送该通道，不会另行中止工具 future。这不是“工具内部是否有自己的超时”的推测：无论工具自行多久返回，本 run 的取消和 timeout 都不约束这段 await。
- **Suggested fix**: 将 fast-path dispatch 也纳入同一个 run cancellation/deadline 生命周期，并让 Cancelled/Timeout 进入正确的终态收尾，而非当作普通工具失败或成功。保留现有权限 gate、dispatch 前原子 journal、正常返回的工具结果和 FsScope；取消时按已有 journal recovery 契约处理未完成 dispatch，不伪造成功回执。不要简单另起 detached 工具任务来“返回取消”，否则只会隐藏仍在运行的副作用。
- **Confidence**: 0.99

### EE2-02 [P2] Media cleanup guard is installed after the cache-producing awaits

- **Location**: `src/gateway/execution_engine/run_loop/inner.rs:659-663` (`ExecutionEngine::run_agent_loop_inner`, `MediaCleanupGuard`)
- **Failure mode**: 带附件的 run 已将远程附件写入 session media cache，随后在归档下一附件、写 artifact 或调用图片理解/语音转写期间取消或超时 → 外层 select 丢弃 inner future，但 `MediaCleanupGuard` 尚未构造，已写入的临时文件不被清理。具体可由“两份 URL 附件：第一份完成缓存，第二份下载未完成时取消”触发。若该 session 此后不再成功经过 media 清理路径，文件会留到后续 stale sweep/系统清理；长时间运行的服务器反复处理此类取消会累积附件缓存。
- **Evidence**:
  ```rust
  let mut media_cleanup_guard = MediaCleanupGuard {
      processor: self.media_processor.clone(),
      session_key: request.session_key.to_key_string(),
  ```
  `src/gateway/execution_engine/run_loop/inner.rs:596-611` 已先 await `archive_inbound_attachments` 和 `media_processor.process`；前者在 `src/gateway/execution_engine/run_loop/inner.rs:1723-1724` resolve 缓存后继续异步读文件，并在 `src/gateway/execution_engine/run_loop/inner.rs:1751-1760` await artifact 写入，整个阶段都在 guard 之前。跨批次核实 `src/media/cache.rs:173-201`：URL resolve 创建 session 目录并写入下载文件；`src/media/processor.rs:62-78` 顺序 await 各附件，`src/media/processor.rs:82-90` 的 cleanup 才负责删除 session 缓存，stale cleanup 是启动期补救。另核实 `src/gateway/execution_engine/execute.rs:883-912`：取消/超时会直接 drop inner future，而不是等其走到 guard。这里的问题是 guard 的生效时机，不是候选项所猜测的“非空附件可能返回零个 block”。
- **Suggested fix**: 在任何可能创建 media cache 的归档/处理 await 之前构造并激活现有 guard，以“将处理非空附件且存在 media processor”的条件决定是否需要 cleanup，不依赖事后得到的 `media_blocks`。保留现有 success-arm `cleanup_now()`、Drop 兜底及一次性 disarm；只清理由 MediaCache 拥有的临时 session 目录，不改变原始附件或 durable artifact 的生命周期。
- **Confidence**: 0.99

## Clean files

- `src/gateway/execution_engine/run_loop/tests.rs`
- `src/gateway/execution_engine/tests.rs`
- `src/gateway/execution_engine/btw_wire_tests.rs`
- `src/gateway/execution_engine/run_loop/mod.rs`
- `src/gateway/execution_engine/mod.rs`
- `src/gateway/execution_engine/run_loop/flow_scope_census.rs`
- `src/gateway/execution_engine/slash_skill_scope.rs`
- `src/gateway/execution_engine/fast_path.rs`
- `src/gateway/execution_engine/engine.rs`
- `src/gateway/execution_engine/tool_service_builder.rs`
- `src/gateway/execution_engine/session_run_registry.rs`
- `src/gateway/execution_engine/scratchpad_progress_sink.rs`
- `src/gateway/execution_engine/agent_trace_emit_sink.rs`
- `src/gateway/execution_engine/run_loop/project_context.rs`
- `src/gateway/execution_engine/markdown_skill_tools.rs`
- `src/gateway/execution_engine/settle.rs`
- `src/gateway/execution_engine/turn_memory.rs`
- `src/gateway/execution_engine/run_loop/author_census.rs`
- `src/gateway/execution_engine/trace_sink_adapter.rs`
- `src/gateway/execution_engine/persistence.rs`
- `src/gateway/execution_engine/adapter.rs`
- `src/gateway/execution_engine/concurrency_handle.rs`
- `src/gateway/execution_engine/history.rs`
- `src/gateway/execution_engine/deadline.rs`
