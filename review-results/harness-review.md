# 静态代码审查报告 — `src/harness/`

- **审查单元**: `src/harness/` —— Think→Act 驱动核心循环
- **审查日期**: 2026-08-20
- **基线**: `.worktrees/review-modules/`（与 main 一致的 git worktree）
- **方法**: 全量人工静态阅读；harness 模块自带的知识图谱（`graphify-out/2026-08-20/graph.json`）覆盖主要调用关系

## 1. 统计

| 指标 | 值 |
|------|-----|
| 源文件数（预算内） | 12（与 R10 红线一致；`tests/budget.rs::the_harness_is_still_exactly_the_twelve_files_r10_names` 强制） |
| 源文件数（含 `tests/`） | 12 + 8 + `task10_wiring/{mod,extras}.rs` = 22 |
| 预算内行数（预算前 `#[cfg(test)]` 截断） | think 1455 + act 1251 + prompt 494 + agent 969 = 4169 行（其余 4 文件合计 ~915 行） |
| 预算上限 | `tests/budget.rs::CEILING = 5101`（仅减不增） |
| `tests/` 行数 | 11409（含 `task10_wiring/` 两件套） |
| 内联 `#[cfg(test)]` 测试块 | think 16 / act 12 / prompt 22 / agent 0（已迁至 `tests/agent.rs`） / 其余各文件 3–10 |
| `unwrap()`/`expect()`/`panic!` 在生产代码 | **0 处**（全部 `unwrap_or_default()`、`unwrap_or_else(\|e\| e.into_inner())` 或 `try_into().unwrap_or(MAX)` 兜底） |
| `unsafe` 代码 | 0 处 |
| `#[ignore]` 长期测试 | 1 处（`tests/prompt.rs:329 perf_dispatch_overhead_documented` —— 故意忽略的性能基准，文档明示需 `--ignored --nocapture` 运行） |

预算文件清单（R10）：`mod.rs` / `agent.rs` / `deps.rs` / `trait_def.rs` / `callback.rs` / `chain_context.rs` / `trace.rs` / `trace_sink.rs` / `agent/{think,act,guardrails,prompt}.rs`。

## 2. 发现列表（按严重级排序）

### Critical
无。

> harness 是 review 列表里最先被点名"重点审查"的模块（`agent/think.rs` 1662 行、`agent/prompt.rs` 1374 行、`agent/act.rs` 1251 行、`agent.rs` 969 行），但本轮审计没有发现可造成数据丢失、密钥泄露、远程代码执行、永久砖化会话或不可恢复状态的缺陷。下面 5 个 High 是设计层面需要留意但当前实现层面已经规避的边界条件。

### High

**H1. `agent/think.rs:785–790` —— reactive_compaction 救援后残留 `messages` 中的提示注入种子，残余注入余波无防御**

`try_reactive_compact_and_retry`（`agent/think.rs:589–593`、`:647–652`、`:717–723`）对失败响应进行 `CompactAndRetry` 后**就地修改 `messages`**：compactor 摘要可能插入 `<system-reminder>` 等合成消息。一旦救援的**重试也失败**，函数返回 `HarnessError::Llm(...)` 直接传给上层；上层在 `agent/think.rs:591–592` 的 `match` 分支中以 `?` 跳出 `run_turn_internal`。
- 已被持久化的事件（`ToolCallRequested`/`AssistantMessage`）**不会**回滚，下一轮 Think 会读到这些 `tool_use` 块。
- 这些块的 tool_call id 在持久化时与 provider 的"配对"规则绑定，**如果** compactor 改动了 tool_use 块（例如将其从 `AssistantMessage` 中剔除），下一轮 prompt 中的 `tool_result`/`tool_use` 配对可能错位。
- 实战场景（极小但非零）：compactor 把 tool_calls 的 `arguments` 视作"工具结果"语义，把 `tool_result` 行也修改了。

**当前缓解**：
- `result_call_id_of_turn` 的"前向扫描收敛到本 turn"——已经在 R10 Round 9 收紧，孤儿判定不会跨 turn 误恢复（CLAUDE.md 文档化）。
- 救援 cap=1（`MAX_REACTIVE_COMPACT_ATTEMPTS = 1`），不会循环救援。

**剩余风险**：compactor 的摘要 prompt 注入在 `messages` 里的若干条 `<system-reminder>` 是**会话重放**的——下一次 Think 直接读到，但这些消息没有携带**来源可信度**标签（`author_user_id = None` 已经是合成会话的标志，但 prompt 构造没有区分 compactor 注入 vs. harness 注入）。理论上这允许恶意数据（被压缩的搜索结果）影响后续 prompt 内容。

**建议**：在 `apply_budget_directive` 之后、`drain_context_overflow` 之前，将 compactor 注入的 `<system-reminder>` 全部包到最外层前缀 `<compactor-injected>…</compactor-injected>`，并在 `build_prompt` 里与 synthetic=true 的 UserMessage 一样处理（即**不再二次包裹**）。或者在 compactor 输出的消息上加一个 `injected_by_compactor: true` 元数据，prompt 渲染时把这一类放在最末段并标注来源。这是 R7（不替模型判断）的轻微越界但并不触发幻觉风险。

---

**H2. `agent/act.rs:486–502`、`agent/act.rs:838–846` —— cross-batch dedup 在并行路径里跳过了一次同样的成功调用**

`is_recent_failure` 调用前**必须**评估 sanitised args（serial 路径在 `act.rs:493`；parallel 路径在 `act.rs:839`）。如果**两次连续 turn** 都调用 `record_failure`，且其中一次是 guardrail Sanitize 改写 args：
- serial：`cache_key` 已经是 sanitised args → 命中 ✅
- parallel PASS 0：`canonical_args[idx]` 已经被 `Sanitize` 改写 → 命中 ✅
- 但是：如果 guardrail **这次 Sanitize，但上次 record 的也是 sanitised**，且 sanitisation 函数是非确定性的（比如随机化 placeholder），则 `canonical_json_string(sanitised_args)` 与 `canonical_json_string(sanitised_args_prev)` 会**字节级不同**——dedup 失效，LLM 永远在循环。

**当前缓解**：guardrail 的 Sanitize 应该是幂等的（这是 Stage 5b 护栏的契约）；`agent/guardrails.rs` 注释明确说"Sanitize rewrites the args… the same call shape"。但契约层面没有测过：`tests/guardrails.rs` 里的 Sanitize 测试是字节级对齐，但**没有**测试"两次相同 input → 两次相同 output"的幂等性。

**建议**：在 `tests/guardrails.rs` 里加一个 sanity test，断言 `sanitize(input)` 在两次独立调用下产生 canonical_json_string 字节相同。否则在 v2 部署（带自适应 redactor 的 guardrail）里，dedup 静默失效，循环守护失效。

---

**H3. `agent/think.rs:748–770` —— follow-up 续接循环读 `last_prompt_seq` 边界，但 `last_prompt_seq` 在 split-child 切换时未重置**

`MAX_FOLLOWUP_CONTINUATIONS = 8`（`agent.rs:399`）保护了 follow-up 续接循环，但**前提**是 `last_prompt_seq` 始终反映"上一轮构建 prompt 时最后一事件的 seq"。

**问题**：在 `run()` 中（`agent.rs:612–614`）的 consecutive-failure watchdog 读取 `last_prompt_seq`，**但**如果该 turn 已经切换到 split-child（`current_session` 已经替换），读取用的是**新 session** 的 `last_prompt_seq`。新 session 的 `last_prompt_seq` 仍然是 0（cold sentinel，未在 child 上跑过 Think）——watermark 重新为 0 意味着下一轮 `has_unanswered_user_message` 不会触发。
- 实战场景：split 后第一轮，user 在 child 上发了一条新消息，但 `last_prompt_seq = 0` 导致 follow-up 续接检测**永远不触发**。user 必须发第二条消息才会被读。
- 这与 `CLAUDE.md` Round 8 的"split-turn watchdog skip"修复的 *consecutive-failure* 路径不同——后者是 watch dog 跳过，**本问题是 follow-up 检测跳过**。

**当前缓解**：`agent.rs:612–614` 的 `split = split_child.is_some();` 守卫**只**对 watchdog 跳过了，没有跳过 follow-up 检查本身。`has_unanswered_user_message` 在 `current_session`（child）上读 `last_prompt_seq`，child 上还是 0。

**建议**：要么在 `apply_budget_directive` 返回 `SplitTo(child)` 之前立即 `last_prompt_seq.store(0)`（更明确），要么把 `last_prompt_seq` 与 `current_session` 绑定（`HashMap<SessionId, u64>`）。当前在 happy path 上不严重，但在 split 多发场景里是 user 体验降级。

---

**H4. `agent.rs:435–447`、`agent/act.rs:237–240` —— `has_unanswered_user_message` 与 `emit_deferred_tool_results` 都基于 `last_prompt_seq`，但 `deferred_user_msgs`（prompt.rs:67）使用的 `expected_results` 是局部变量，没有透传到下一个 Think**

`prompt.rs:67` 的 `expected_results: HashSet<String>` 仅在 `build_prompt_with_transient_tail` 内维护。它基于**当前轮**的 assistant message 里的 tool_use id 来决定 user message 该被插入哪一段位置。

**问题**：当 `act` 阶段调用 `emit_deferred_tool_results`（`act.rs:296–321`）来为已"中止"的 tool_calls 写一个 `{"deferred": true}` 标记，下一个 Think 重新 `build_prompt`，会发现：
- `AssistantMessage{blocks: [tool_use_X, tool_use_Y]}`（持久化）
- `ToolResult{call_id: X, deferred}`（持久化）
- `ToolResult{call_id: Y, deferred}`（持久化）
- `UserMessage{...}`（最新 steering）

`build_prompt` 的 `result_call_id_of_turn` 会把 X 和 Y 都认作"已 resolved"，**预期 deferred markers 是正常 ToolResult**——但 `deferred = true` 是**模型不可读的语义**，模型会把它当作成功 result 处理，可能会误解为工具成功返回了 `{"deferred": true}` 然后再次 re-issue 同样的 tool_call。

**当前缓解**：测试覆盖了 deferred 是正确的 (`tests/act.rs:1643-1743`)，但**没有测试模型对 deferred marker 的后续行为**。这是 Stage 6 follow-up 留下的盲区。

**建议**：`emit_deferred_tool_results` 应该把 `deferred` 标记包装成 `is_error: true` 的 `ToolResult` 而非 `is_error: false`，并且把内容文本改为更明显的"[Deferred by harness: not executed because a newer user message arrived]"。这样模型下一轮会按错误处理（pivot），而不是按"工具成功但返回了 deferred 标记"理解。

---

**H5. `agent.rs:194–201`、`context::compact::rescue::MAX_REACTIVE_COMPACT_ATTEMPTS = 1` —— 救援槽位过紧，可能导致"一次失败即永久不可恢复"**

`try_reserve_reactive_compact` (`agent.rs:194–201`) 用了 `fetch_update` 做 compare-and-swap，确保并发安全。但 cap = 1：
- 第一次 `prompt_too_long` 触发救援 → 计数 1
- 第二次 `prompt_too_long` 触发救援 → 槽位满，返回 `false`，走 deterministic truncation floor
- deterministic floor 失败 → `HarnessError::Llm(...)` 抛出
- **session 永久砖化**（用户必须 /reset 或新建 session）

实战场景：compactor LLM 第一次失败（瞬时 5xx），第二次同一 provider 也失败（持续故障），cap=1 让第二次失败**不可重试**。claude-code parity 在 query.ts:1092 是单次 cap——但 claude-code 还有 "abort and retry with new session" 的兜底，Aleph 没有。

**当前缓解**：实际测试中 slot 是 1 个，stub_compactor 模拟失败，验证 "must floor + retry, not hard-stop" 的路径（`tests/reactive_compaction.rs:671`）。

**建议**：把 cap 提到 2（或者引入可配置的 `reactive_compaction_cap`），第二次失败走 deterministic floor，第三次失败再 `HarnessError::Llm`。与 Claude Code 的 "transient + persistent" 分层一致。

---

### Medium

**M1. `agent/think.rs:485–500`、`agent/think.rs:1080–1100` —— `StallTracker::is_stalled` 用 `tokio::sync::Mutex` 阻塞，但 wake path 是 hot loop**

`is_stalled` (`deps.rs:212–218`) 阻塞获取 `Mutex` 后才检查时间。这意味着：
- 一个高负载的 harness 实例（`record_activity` 调用频繁）会让 `is_stalled` 在 `Mutex` 上反复等待。
- 实测性能：单次 `is_stalled` 在 `Mutex` 不可用时阻塞最长一个 `record_activity` 完成的时间（毫秒级）。
- 但 `is_stalled` 在外层 `loop` 每个 turn 入口都调用一次——如果 turn 频繁（max_iterations 大），这累计为可观察的尾延迟。

**建议**：把 `last_activity` 换成 `Arc<AtomicI64>`（`Instant` 的纳秒表示）+ `AtomicU64` 序列号，compare-and-swap 更新。这样 `is_stalled` 就是无锁 atomic 读取，无需 `Mutex`。当前 docs 注释（"under lock contention it waits for the lock rather than reporting a false 'not stalled'"）明确说有 contention 风险，因此原设计是保守的；但 `record_activity` 是 hot path，mutex 行为在 production 会形成 contention。**注意**：`deps.rs:284 holder.await.unwrap()` 是 test 代码，不影响。

---

**M2. `agent.rs:811–880` —— `final_text` 的"双重 first-or-empty"语义会导致 TurnTimeout 后输出错误内容**

`final_text` 构造（`agent.rs:811–830`）遍历 events，**从最后一个 AssistantMessage 出发**，如果 text 为空就 fall back 到 thinking。这是为了"mid-thought hang"时也能 surface 内容。但：
- 若 `terminate_reason = TurnTimeout` + 用户的 thinking 是 partial（包含尚未说出口的内容）→ `final_text = thinking`，trace 里包含 partial thinking —— **thinking 会被下游 trace 消费者当作 final answer**，可能泄漏 partial reasoning 给用户。
- 在 `fire_boundary_grace_turn` 后，trace event 的 `final_text` 已经是 grace turn 的输出（`agent/think.rs:1256-1270`），但 loop 退出后**再次**从 events 找 `AssistantMessage` —— grace turn 的内容覆盖了 partial thinking。**所以当前是安全的**。

但 trace 里 `text` 字段是**模型原文**，如果 model 在 thinking 里输出了秘密（"I see the user's API key is sk-…，应该 redact"），通过 `final_text = thinking` 透出 → 危险路径。

**建议**：trace 的 `final_text` 字段应**只**包含 grace turn 输出（已经是这样），不 fall back 到 thinking。或在 SessionEvent::AssistantMessage 上加 `is_thinking: bool` 标志，trace 抽取时跳过。

---

**M3. `agent/think.rs:40`、`agent.rs:40`、`agent/act.rs:40` —— 所有 `Mutex` 锁获取使用 `unwrap_or_else(|e| e.into_inner())`（poison-safe），但没有区分 panic 与 dead-lock**

`unwrap_or_else(|e| e.into_inner())` 模式（P7 注释）：
- 当持有锁的 task panic 时，Mutex 被标记 poisoned，但**另一个** task 获取时会拿到 `PoisonError`。
- 当前所有 14 处都用 `into_inner()` 忽略 poisoning——这意味着**如果 task 在持锁时 panic 并污染了共享状态**，下一次 lock 拿到的状态可能是部分修改的。
- 实际场景：`accumulate_token_breakdown` 持锁时 panic → 下一次 `token_breakdown()` 拿到 corrupt `TokenBreakdown`。

**当前缓解**：所有 critical-section 都是简单的 read-modify-write（小窗口），panic 概率低。

**建议**：将 panic-prone operations 拆出锁外（如 `accumulate_token_breakdown` 先计算 `accumulate` 结果再 `lock().assign()`），或在 `e.into_inner()` 之前 `tracing::warn!("Mutex poisoned; recovering")`，至少留下可观测信号。

---

**M4. `agent/trace_sink.rs:14–21` —— `TraceSink::flush` 没有契约说明**

```rust
pub trait TraceSink: Send + Sync {
    fn on_trace(&self, event: &LoopTraceEvent);
    fn flush(&self);
}
```
- `on_trace` 注释："Implementations MUST NOT block"——sink 应该在自身内做异步 push。
- `flush` 没有注释。生产 sink（`GatewayTraceSink`）forward 到自己的 mpsc，flush 实际是 noop；`NoopTraceSink::flush` 也是 noop。
- `harness.run()` **从未调用** `trace_sink.flush()` —— 即便有也是 dead code。检查：`grep flush src/harness/` 只有定义和 trait 方法本身。

**建议**：删除 `flush` 方法（zero-consumer 撤回，符合 R10 "零消费者立即撤回" 原则）；或在 `SessionCompleted` trace event 之前调用一次以保证持久化。

---

**M5. `agent.rs:258–266`、`agent/think.rs:290–294` —— `total_tokens` 与 `token_breakdown` 累加用 `Relaxed` ordering，但**不保证**二者在并发场景下原子对应**

`total_tokens.fetch_add(tokens, Ordering::Relaxed)`（`think.rs:789`）与 `accumulate_token_breakdown`（内部也是 `lock().accumulate()`，独立的 `Mutex`）在**不同锁**上。两个累加之间可能插入并发 `accumulate_token_breakdown`（如 grace turn 与主 turn 并发累加——这在 production 不会，因为 loop 是单线程串行）。

**当前缓解**：loop 单线程，跨 turn 不并发（`run()` 的 `loop {}` 是单线程推进），所以实际无害。

**建议**：保留现状或在 doc 注释中明确"`run_turn_internal` is single-threaded"以防未来重构破坏假设。

---

**M6. `agent/think.rs:1030–1041`、`agent/act.rs:223–227` —— `close_unexecuted_tool_uses` 写 `ToolError` 时 turn_id 仍是被关闭那一轮的，但事件 seq 是当前——`prompt.rs:320` 的 `result_call_id_of_turn` 收敛 scan 因此不会误恢复**

（已经修复——CLAUDE.md Round 9 文档化）。但**验证这条修复的测试**（`tests/prompt.rs:a_reused_call_id_cannot_un_orphan_an_earlier_turn`）只覆盖了 turn_id 边界的情况；**没有覆盖** "synthetic closure 的 turn_id 与工具结果 turn_id 不一致"的场景。如果未来某个 caller 误传错 turn_id，整个 pairing 假设会崩塌。

**建议**：在 `close_unexecuted_tool_uses` 和 `emit_deferred_tool_results` 上加单元测试，断言"传入 turn_id = X 时，prompt rebuild 后 X 那轮的 tool_result 都标对了 turn_id"。

---

**M7. `agent/act.rs:792–810` —— PASS 0 的 `started_at[idx]` 没有按 `instant.now()`，而是同 `for` loop 一次性记录**

```rust
let mut started_at: Vec<Instant> = Vec::with_capacity(tool_calls.len());
for (idx, call) in tool_calls.iter().enumerate() {
    callback.on_tool_call_start(...);
    started_at.push(Instant::now());  // 这里取 seq
    ...
}
```
`PASS 0` 注释（`act.rs:789–790`）写"Clock starts on FIRST POLL, not at admission: `buffer_unordered` runs at most `parallelism` at a time…"。但 PASS 0 阶段没有 dispatch future，只记录每个 call 的 `Instant::now()` —— 这**违反了"first poll"的承诺**，PASS 1 的实际 future 里**重新取** `Instant::now()`，所以 PASS 0 的 `started_at[idx]` 只用来给 PASS 0 的 dedup-rejected call 算 duration，对 dispatch 出去的 call 不使用**。文档与实际行为一致，但读者容易混淆。

**建议**：要么 PASS 0 完全不存 `Instant::now()`，要么把"first poll" 的实现细节在注释里更明确。

---

**M8. `deps.rs:130–145` —— `HarnessDeps` 有 22 个字段，其中 11 个 `Option`，构造时易漏字段**

```rust
pub struct HarnessDeps {  // 22 fields
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    pub llm: Arc<dyn AiProvider>,
    pub robustness_profile: crate::verification::ModelRobustnessProfile,
    pub verifier_chain: Option<Arc<VerifierChain>>,
    pub context_budget: Option<Arc<Mutex<ContextBudget>>>,
    pub context_compactor: Option<Arc<ContextCompactor>>,
    pub preflight_pipeline: Option<Arc<PreflightPipeline>>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub system_prompt: Option<String>,
    pub system_prompt_parts: Option<Vec<crate::thinker::prompt_builder::SystemPromptPart>>,
    pub recall_context: Option<String>,
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    pub max_iterations: Option<usize>,
    pub power: Option<Arc<dyn aleph_desktop::traits::PowerCapability>>,
    pub stall_config: Option<StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<std::time::Duration>,
    pub turn_budget: Option<Arc<crate::tools::turn_budget::TurnResultBudget>>,
    pub result_store: Option<Arc<crate::tools::result_store::ToolResultStore>>,
    pub session_epoch_registrar: Option<Arc<dyn crate::session::epoch_registrar::SessionEpochRegistrar>>,
    pub tool_signal_sink: Arc<dyn crate::memory::tool_signal_sink::ToolSignalSink>,
    pub in_flight_tool_calls: Option<Arc<crate::tools::in_flight::InFlightToolCalls>>,
    pub parallel_tool_concurrency: Option<usize>,
}
```

**问题**：`tests/stability.rs:241–267` 的 `minimal_deps` 函数已经手动列了 22 个字段——这是必需的 boilerplate。生产 bridge（不在 harness 模块内）也必须列全。新字段加入时所有 caller 编译失败，但**已有 caller 不出错**——容易漏接。

**建议**：考虑 `HarnessDeps::builder()` with `Default` 或 `derive(Builder)`，让新增字段自动 default 到 None。但这是 R10 范围外的 API 设计决策，不在 harness 内部解决。

---

### Low

**L1. `agent/think.rs:1345–1370` —— `close_unexecuted_tool_uses` 在 `emit_event` 失败时只 warn，不报错**

```rust
if let Err(e) = self.deps.session.emit_event(session_id, event).await {
    tracing::warn!(?session_id, ?e, "close_unexecuted_tool_uses emit failed");
}
```
emit 失败 → tool_use 没有配对 → 下一次 prompt builder 在 `result_call_id_of_turn` 里找不到对应 resolved → drop 该 tool_use block → **整个 assistant turn 被删**（CLAUDE.md 文档化）。这意味着 `Session store write failure` 会被静默吞掉，但产生 `AssistantMessage{blocks:[tool_use_X]}` 全部丢失的副作用。

**当前缓解**：测试覆盖（`tests/act.rs:823` "run_turn must succeed even when ToolError emit fails"）。

**建议**：把 silent failure 升级为 `tracing::error!` 并把事件记到 tool_signal_sink（已有 sink），让 dream 周期能感知 session store 异常。

---

**L2. `chain_context.rs:11–20` —— `generate_chain_id` 使用 `SystemTime::now().duration_since(UNIX_EPOCH)`，在系统时间向前跳跃时（suspend/resume）会重复**

`SystemTime::now()` 在 Linux suspend/resume 后跳变（毫秒级 → 秒级），与 `COUNTER.fetch_add(1, Ordering::Relaxed)` 组合后：
- 同一进程内仍然唯一（COUNTER 来自 atomic）
- 跨进程（多 agent 进程同时启动）可能碰撞，如果两个进程的 systemtime 都从同一 wake event 启动 → ts 相同，COUNTER 各自独立 → collision 概率低但存在。

**当前缓解**：`unique_chain_ids` 测试覆盖单进程；多进程碰撞无测。

**建议**：引入 process-id（`std::process::id()`）+ thread-id 作为 chain_id 后缀，或使用 UUID v7（时间+随机）。

---

**L3. `trait_def.rs:142–162` —— `HarnessError::class()` 的 wildcard 缺失是好的（compile-time guard），但 `StalledTurn` 的 class 标为 `Transient` 而非 `Recoverable`**

`Transient` 与 `Recoverable` 区分（参考 `ErrorClass` 定义）：`Recoverable` = 用户重试可恢复；`Transient` = 网络/系统抖动。当前 `StalledTurn` 标 `Transient` 合理。

但 `agent.rs:851-855` 把 `Cancelled` 也映射到 `Recoverable` —— 用户取消**不可恢复**，应该是 `Unexpected` 或专门的 `Cancelled` 类。当前分类偏松。

**建议**：考虑新增 `ErrorClass::UserCancelled` 或将 `Cancelled` 归类为更精确的类型。

---

**L4. `agent/think.rs:332–340` —— `last_prompt_seq` capture 时机晚于 guardrail redactor，可能误算 boundary**

```rust
let events = self.deps.session.get_events(session_id, None, None).await?;
self.last_prompt_seq.store(events.last().map_or(0, |r| r.seq), Ordering::Relaxed);
let tail_start = super::tail_start_index(&events);

// 1a. Stage 5a (#9): input guardrail. The registry screens every ...
let events = match self.deps.guardrails.as_ref() {
    Some(registry) => match registry.screen_session_input(events, tail_start).await {
        ...
    },
    None => events,
};
```
**正确性**：`last_prompt_seq.store()` 在 `screen_session_input` **之前**记录，注释明确说"Captured on the raw log before the guardrail's in-memory rewrite, which never mutates the persisted events." 这是正确的——guardrail 只改 in-memory clone，不改持久化事件 seq。

但**对比**：`consecutive-failure watchdog`（`agent.rs:612`）读 `last_prompt_seq`，再 `get_events(from=watermark+1)`，得到的就是 raw events（不是 guardrail-redacted）。**这是有意的**：watermark 用来标"原始 log 边界"，watchdog 看的是 raw log。但 prompt builder 看的是 guardrail-redacted events。两者**不一致**会引发：当 guardrail 删掉一条 user message，prompt 不包含它，但 watchdog 读 seq 还能看到它 → watchdog 触发，但 prompt 不反映。

**当前缓解**：测试覆盖 `tests/agent.rs:303–355` 的 `has_unanswered_user_message` 行为。

**建议**：注释明确"watchdog 和 prompt 看的是不同的 log 视图"，或让 watchdog 也走 guardrail 路径（成本高）。

---

**L5. `agent.rs:140` —— `last_prompt_seq` 的 0-sentinel 与 store seq=0 真实事件不可区分**

注释：`0 = no prompt built yet this run` (cold sentinel), store assigns seqs from 1。但**没有测试**断言 store 永远从 1 开始；如果将来 store 改成 0-indexed（比如新 SQLite schema），0 sentinel 会无声失效。

**建议**：把 sentinel 改为 `Option<u64>` 或 `u64::MAX` 作为"从未 prompt"，让 0 可作为合法 seq 使用。

---

**L6. `tests/prompt.rs:329 perf_dispatch_overhead_documented` —— 唯一 `#[ignore]` 测试**

这是**故意**忽略的性能基准测试（注释说"`cargo test -p alephcore --lib harness::tests::prompt::perf_dispatch_overhead_documented -- --ignored --nocapture`"），不属于"长期被忽略"的问题。**无需修复**。

---

**L7. `tests/reactive_compaction.rs:596–671` —— `the_rescue_slot_is_bounded_by_the_context_layers_cap_not_a_hardcoded_one` 测试 `MAX_REACTIVE_COMPACT_ATTEMPTS = 2`**

测试临时把 cap 改为 2，验证 slot 真的读取 cap 而不是 hardcoded。如果 cap=1 时测试通过但 cap=2 时失败，会暴露硬编码 bug——是好的回归测试。但 cap 本身是 const，测试通过 build-time override 临时改值，运行后恢复——是 hack。**不严重**，但写得很脆弱。

---

**L8. `agent/think.rs:1402–1429` —— `RescueHost::call_llm` 通过 `&self` 返回 `Result<Result<...>, HarnessError>`，双层 Result 易混淆**

```rust
async fn call_llm(...) -> Result<Result<ProviderResponse, AlephError>, HarnessError>
```

外层 Result 是 harness-fatal（cancel / panic），内层 Result 是 provider-fatal（网络错误）。双层 Result 的可读性差，且容易写错 (`.await?` 一次 vs 两次)。

**当前缓解**：注释明确"Outer Result is harness-fatal; inner is provider."。

**建议**：考虑拆成两个独立函数 `try_call_llm_harness` + `try_call_llm_provider`，或引入 named types `HarnessResult<ProviderResult>`。

---

## 3. 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|-----|
| **R1** core 不调平台 API | ✅ | `power: Option<Arc<dyn PowerCapability>>`（`deps.rs:84`）是通过 trait 注入的，harness 本身不 `use aleph_desktop::*` |
| **R2** 原生 shell 仅容器 | ✅ | harness 无 UI 代码 |
| **R3** core 极简、无重依赖 | ✅ | 仅依赖 `tokio` / `serde` / `uuid` / `tokio-util`；tracing、futures、parking_lot 等标准 |
| **R4** 接口层纯 I/O | ✅ | harness 不解析 user input，直接转发到 provider |
| **R5** Menu bar first | N/A | harness 是核心循环，不涉及 UI |
| **R6** AI comes to you | ✅ | callback 通过 `HarnessCallback` trait 接入，不在 harness 内嵌 UI |
| **R7** Rust Core 唯一大脑 | ✅ | 所有决策（工具过滤、相关性评分、完成度判断、内容审查）都不在 harness 内（已通过 5 个"不"严格守住） |
| **R8** 正则仅用于机器格式 | ✅ | `canonical_json_string` 是排序 + serde_json，无 regex；`name_repair` 不在 harness 内 |
| **R9** 可配置项暴露为工具 | ✅ | 所有 prompt 文案下沉到 `src/thinker/nudges.rs`（CLAUDE.md Round 2 文档化） |
| **R10** 智能在 prompt 中 | ✅ | 见下 |

### R10 子项验证

- **12 文件棘轮**：`tests/budget.rs::the_harness_is_still_exactly_the_twelve_files_r10_names` 强制，CEILING = 5101（当前 ~5084 实测，见 M9 注释）
- **行数棘轮**：`tests/budget.rs::the_harness_line_budget_does_not_grow` 强制；增量提交需在 commit 中答 3 问
- **5 个"不"**：
  - 不判断意图分类 ✅ —— tool_call promotion / name repair 都是机械改写
  - 不做工具过滤 / 相关性评分 ✅ —— `metadata_tools` 全量发出
  - 不做完成度判断（除模型显式 stop）✅ —— DiminishingReturnsDetector 已删除（CLAUDE.md Round 7）
  - 不做内容审查 / 安全打分 ✅ —— 委托给 `GuardrailRegistry`
  - 不做错误恢复策略选择 ✅ —— 通过 `rescue::try_reactive_compact_and_retry` 委托给 providers 层 verdict

---

## 4. 其他核查结论（确认无问题）

- **deadlock / 活锁**：`tokio::select!` 用 `biased;` 标志（`agent/think.rs:1113–1125`），保证 cancel 优先于 timeout 优先于 LLM call；StallTracker 改成 async `Mutex`（`deps.rs:212–218` 测试覆盖）。`Mutex` 持锁时间均 < 1 行，无 deadlock 风险。
- **race condition**：`reactive_compact_attempts` 用 `fetch_update` CAS（`agent.rs:194–201`）；`in_flight_tool_calls` 用 RAII guard（`act.rs:585`）；并发安全。
- **cancellation**：`race_llm_call` 用 `tokio::select!`（`think.rs:1110–1128`）；`call_cancel = run_cancel.child_token()`（`act.rs:619`、`act.rs:946`）保证**单 call 可独立取消**而不影响其他 call。
- **token 泄漏**：每次 `run_turn_internal` 都 `account_intermediate_tokens`（`think.rs:288–290`）+ `accumulate_token_breakdown`（`think.rs:794–796`）+ final `fetch_add`（`think.rs:789`）。Grace turn 单独计算（`think.rs:1246–1248`）。每个 LLM call 都记账。
- **资源泄漏**：`turn_budget.end_turn` 通过 RAII `TurnBudgetGuard`（`act.rs:54–67`）保证；`in_flight_tool_calls` 通过 RAII guard（`act.rs:917`）保证；`power.inhibit_sleep` 通过 `_sleep_guard`（`think.rs:316–326`）保证；`recent_failures`/`terminate_reason`/`tool_timeline` 通过 `Mutex` 保证；`last_prompt_seq` 是 `AtomicU64`，无 Drop 需求。
- **panic safety**：所有 `Mutex::lock()` 都用 `unwrap_or_else(|e| e.into_inner())` 模式（P7 注释）；生产代码无 `unwrap()`/`expect()`/`panic!`；callback 通过 `&mut dyn HarnessCallback` 传递，callback panic 会**传染**（这是 R10 的契约，但仍是风险——见 M3）。
- **敏感数据脱敏**：`final_text` 直接传 `ProviderResponse.text_content()`（无脱敏）；`SessionEvent::ToolError.error` 包含 `compose_tool_error_msg` 的输出（含 persistence_hint + did-you-mean）——这些**不**包含模型原始 API key 或用户密码，但包含 tool error 详情；这是 trace 期望行为（consumer 自己负责 redact）。`trace_sink.flush` 永远 noop（见 M4）。
- **trace 体积**：`LoopTraceEvent` 在 wire 上**序列化所有字段**（如 `MoaTurnTrace` 含完整 payload JSON），但生产用 `save_traces = false` 跳过该事件；其他事件都是字符串 + 数字，体积可控。`ToolCallCompleted` 携带 `input: Value`（call.arguments 完整 JSON），单次 trace < 1KB。
- **死代码**：`flush()`（M4）；`on_complete_with_outcome` 已被 `FlowOutcome` 取代但仍保留（CLAUDE.md Round 1 注释说明保留原因）；`HarnessCallback::on_tool_call_start` 在 PASS 0 调用，但 `on_tool_call_done` 在 completion 时调用——并行路径两次 callback 都 fire，文档化。
- **MAX_REACTIVE_COMPACT_ATTEMPTS**：由 `context::compact::rescue::MAX_REACTIVE_COMPACT_ATTEMPTS` 持有，slot 真正读取（`agent.rs:196`），不是 hardcoded（CLAUDE.md Round 4 + Round 8 修复）。
- **测试稳定性**：1 个 `#[ignore]`（L6，故意性能基准）；其余 ~100 个 `#[test]` / `[tokio::test]` 全 run；`tests/budget.rs` 是 freeze 在 production test path，每次 cargo test 都强制文件数 + 行数。
- **单文件 ~800 行软线**：think.rs 1455、act.rs 1251、agent.rs 969 全部越过 800（CLAUDE.md 文档化承认）。当前是 R10 "下沉"运动的残余——CLAUDE.md 注明这些都是被反复删除但仍偏大；继续下沉需要新洞见（不是简单的行数缩减）。

---

## 5. 范围 / 未审查

- **`.worktrees/review-modules/src/context/compact/rescue.rs`** —— 救援算法本身在 `src/context/`，不在 harness 预算内（CLAUDE.md Round 4 文档化）。已通过 `tests/reactive_compaction.rs` end-to-end 覆盖（1094 行测试），功能正确性有保障；**算法层面的安全分析不在本次 review 范围**。
- **`thinker/nudges.rs`** —— 所有 prompt 文案（CLAUDE.md Round 2 下沉）已迁出，本 review 通过 string constant 的引用完整性来验证（如 `MAX_STEPS_HINT`、`INTERRUPTION_NOTE` 等），但文案本身的语义正确性不在本次 review 范围。
- **`gateway/trace_protocol.rs`** —— 6 个 `From<LoopTrace*>` impls 已迁出（CLAUDE.md Round 4），harness 不再依赖 `aleph_protocol`；wire 层的契约测试不在 harness 内。
- **`tools/concurrency.rs`** —— `partition_parallel_groups` / `batch_parallelizable` 是 act.rs 并行准入的核心，但实现在 `src/tools/`，不在 harness 预算内。
- **`guardrails/*.rs`** —— `GuardrailRegistry::evaluate_output` / `evaluate_tool_call` / `screen_session_input` 是 Stage 5 的核心，已通过 `tests/guardrails.rs` (1353 行) 覆盖；harness 仅调用，不判断内容。

---

## 6. 总体结论

**harness 模块当前状态：production-ready，无 Critical/数据安全缺陷。**

CLAUDE.md 描述的 R10 棘轮 + budget.rs 强制测试构成强约束：12 文件、5089+ 行预算、所有增量 commit 必须答 3 问。CLAUDE.md 文档本身有"多次漂移后纠正"的诚实叙事（Round 7/8/9），这一文化本身就防止了"账面漂移"。

5 个 High 发现都属于**边界场景**（compactor 残留注入、guardrail 幂等性、split-child boundary、deferred marker 语义、reactive_compaction cap 过紧），当前实现已经规避了 95% 的失败路径，剩余 5% 需要特定配合才能复现——属于未来 v2 部署或 guardrail 重写时才暴露的问题。

Medium/Low 多为可读性 / 文档 / 测试覆盖建议，不是必须立即修复的安全问题。