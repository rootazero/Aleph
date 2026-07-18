# Aleph harness 深度审计报告

> 方法：8 个 lens 独立取证 → 复现镜（repro）+ 红线镜（redline）对抗验证 → 去重合成。所有结论均带 `file:line` 锚点。已剔除 14 条被证伪/被红线否决的项（见 §五）。
> 基线：`src/harness/tests/budget.rs::CEILING = 5997`（budget.rs:77），TARGET 4900（budget.rs:56），当前**超 1097 行**（src/harness/CLAUDE.md:17）。逐文件实测：mod 20 / agent.rs 1060 / deps 226 / trait_def 211 / callback 97 / chain_context 100 / trace 465 / trace_sink 33 / think 1889 / act 1296 / guardrails 124 / prompt 476 = **5997**。

---

## 一、确认的 bug（按严重度）

### 🔴 CRITICAL

#### B1. 输入护栏只扫「尾段最新一条 user message」，但 build_prompt 每轮重放全量 raw log → 第 1 轮被脱敏的密钥，第 2..N 轮原文发给 provider
- **锚点**：`src/harness/agent/guardrails.rs:27-35`（`events[tail_start..].rev().find_map(UserMessage)`，只改内存 clone，:46-51）；`src/harness/agent/prompt.rs:79-89`（"Walk the FULL conversation in order"，从 index 0 重放）；`src/harness/agent/think.rs:361`（每轮从 store 重取原始 events）。
- **触发条件**：`[guardrails] enabled` 或 Panel Security 配了 virtual_keys / custom_leak_patterns（`orchestrator_init.rs:375-380`）。第 1 轮 sanitize 生效 → 模型调工具 → 第 2 轮 `tail_start` 越过 AssistantMessage，尾段无 UserMessage → `latest_user_idx = None` → `Allow(events)` 原样放行 → 原文密钥出网。**更糟**：PiiSecretsGuardrail 输入面是 **Block** 而非 Sanitize（`pii_secrets.rs:138-142`，测试 :346-364），被 Block 的那轮不产生 AssistantMessage，`tail_start` 不前进，脏事件留在 log 里，**下一条用户消息把被 Block 的密钥原文推给 provider** —— 门是可绕过的，不只是衰减。
- **修法（外科手术）**：**下沉**，不要在 harness 里加行。
  1. 把 `apply_input_guardrail` 主体（guardrails.rs:20-57）与 `InputGuardrailOutcome`（agent.rs:67-79）搬到 `src/guardrails/registry.rs`，改名 `GuardrailRegistry::screen_session_input(events, tail_start)`；harness 只保留 think.rs:379-394 那 ~10 行 dispatch。
  2. 扫**全部**非 synthetic UserMessage（内存 clone），**Block 语义非对称**：尾段最新一条保持今天的「Block 即结束本轮」；**历史消息的 Block 必须降级为 redact-and-continue**（事件是不可变持久化的，重复 Block 会让会话永久砖化 —— think.rs:387-390 每轮返回 done()，且 `pii_secrets.rs:103-109` fail-closed，瞬时错误也会触发）。
  3. redaction 占位文案属模型可见文案 → 放 `src/thinker/nudges.rs`（src/harness/CLAUDE.md:30）。
- **src/harness/ 净行数：约 −38**（不是 +8）。
- **验证**：`src/harness/tests/guardrails.rs` 新增 `input_guardrail_sanitize_survives_second_turn`（两轮 run_turn，断言第 2 轮 provider 收到的 payload 不含明文密钥）；另加 `input_guardrail_block_on_history_does_not_brick_session`。现有 `input_guardrail_sanitize_rewrites_text_seen_by_provider`（:394-439）恰好把原文钉进 log —— 那正是泄漏源。
- **附带（不同 chokepoint，跟进项）**：ToolResult body 与 recall_context 尾注（think.rs:408-410）同样未扫。建议 registry 侧加 `screen_outbound_messages(&mut Vec<UnifiedMessage>)`，**不要放进 harness**。

#### B2. 离线工件存储 + FTS 索引是**进程级单例**：跨 agent 的 `ctx_search` 可检索到别的 agent 的工具输出；一个会话触发拒绝熔断会 purge 掉所有会话的工件
- **锚点**：`src/bin/aleph-server/commands/start/mod.rs:2454-2457`（`ToolResultStore::new("global")` + `set_global_tool_result_store`）；契约明写「One instance per session … `<session_id>/index.db`」（`src/context/retrieval/content_index.rs:96-99`）；FTS5 schema 无 session/agent 列（content_index.rs:133-150），`search(query, limit)` 无 scope 谓词（:239-268）；`ctx_search` 直接读单例（`src/builtin_tools/ctx_search.rs:96`）；`purge_all()`（`src/tools/result_store.rs:274-300`）唯一生产调用方是拒绝熔断 `src/tools/scoped/dispatch.rs:534-541`，阈值仅 `SESSION_PAUSE_THRESHOLD = 3`（`denial_ledger.rs:73`）。
- **触发条件**：(a) 任意两个 agent/会话并发（Panel 多会话是常态）；(b) 任一会话被用户点 3 次「拒绝」→ 抹掉所有并发会话的 blob 与索引，其上下文里的 `[Full output persisted: …]` 标记指向已删文件。违反 INV-ISO（`src/gateway/execution_engine/tests.rs:692-707`）。
- **修法**：**不要改 `persist_if_large` 签名** —— 它的两个调用方在 `src/harness/agent/act.rs:1136/:1147`，改签名会往超预算的 harness 加行。做法：
  1. 把 session key 烘进 **store handle**（共享 inner DB，外层 session-scoped handle），在已有 session 上下文的接缝构造：`src/orchestrator/harness_bridge/runner_impl.rs:523` 与 `src/gateway/execution_engine/tool_service_builder.rs:135`（后者签名已收 `session_id`，:107）。
  2. FTS5 两张表加 `session_id UNINDEXED` 列，`search()` 与 `purge_all()` 加 session 谓词。
  3. **读路径**（`ctx_search`）用仓库已有的 `crate::tools::turn_context::current_session_key()`（`src/tools/turn_context.rs:93-112`）解析 scope —— 否则改完写路径，读路径仍指向没人写的 "global" 库，检索静默返回 0 命中。
- **src/harness/ 净行数：0**。
- **验证**：新增 `src/tools/tests/result_store_scope.rs`：两个 session 各 offload 一份内容，断言 A 的 `ctx_search` 查不到 B 的；A 触发 `purge_all` 后 B 的 blob 与索引仍在。

---

### 🟠 HIGH

#### B3. 上下文溢出分类器只认 Anthropic 措辞 → 在 OpenAI/vLLM/Ollama/Gemini 上整套 reactive-compaction 救援是死代码，run 直接 Fatal 中止
- **锚点**：`src/providers/llm_retry.rs:388-404`（`classify()` 仅认 `413` / `prompt is too long` / `prompt_too_long` / `request_too_large` / `model_context_window_exceeded`，全是 Anthropic 形状）；消费者不留情：`src/harness/agent/think.rs:1299-1301`（`_ => return Err(HarnessError::Llm(primary_err))`），I1 二次机会同门 think.rs:1430。OpenAI 400 body 原样进 Display：`src/providers/protocols/openai_chat/adapter.rs:311-313`。`classify_http_error`（llm_retry.rs:251-304）**零生产消费者**，没有结构化兜底。
- **触发条件**：任何非 Anthropic provider 上估算欠计（`pressure.rs:101` 是启发式估算，`capability_gate` 是 fail-open 的 chars/4）→ provider 返回 `context_length_exceeded` → 五个子串一个都不匹配 → Fatal。且**是粘性的**：`observe_actual_usage` 只在成功响应后校准，欠计会话永远欠计，整个会话砖化。
- **修法**：**只改 `src/providers/llm_retry.rs`**。把五个内联 `contains` 提成 `CONTEXT_OVERFLOW_PATTERNS`，补 hermes 已验证的措辞（`context_length_exceeded` / `maximum context length` / `context length exceeded` / `max_model_len` / `input is too long` / `reduce the length of the messages` / `input token count`）。**不要**顺手改 `classify_http_error`（零消费者，加了是死代码）；**不要**放宽成 `too large` / `token limit` 之类宽词 —— 该检查位于配额 Fatal 与 429 分支之前（:395 vs :413-425），宽词会把 OpenAI TPM 429 劫持成无意义的压缩。
- **src/harness/ 净行数：0**（+~15 行在 providers）。
- **验证**：`src/providers/llm_retry.rs` 单测：每种 provider 措辞各一条断言 `CompactAndRetry`；**外加一条** OpenAI TPM 429 body 断言仍是 Fallback/Fatal（防止 pattern 过宽）。

#### B4. 无预算工具（所有 MCP/plugin/skill + ~100 个 builtin）把一次慢调用变成 run 级 `StalledTurn` 中止，而不是可恢复的 ToolError
- **锚点**：`src/harness/agent/act.rs:588`（`per_tool_budget.is_some()` → 可恢复 ToolError）vs `act.rs:594-599`（`None` → `HarnessError::StalledTurn` 杀 run）；`max_duration_ms` 只从 19 名 builtin 表填充（`src/tools/budget.rs:14-45`，构造点 `src/tools/scoped/builder.rs:337`、`src/tools/handlers/builtin.rs:61`）；生产 fallback 是 **120s**（`orchestrator_init.rs:448-450`）。MCP client 自身超时默认 **300s**（`src/mcp/client.rs:503`，仅 remote HTTP/SSE；stdio 是 30s），永远轮不到它优雅返回。
- **最狠的两个受害者（审计者没找到，我们找到了）**：
  - **`ask_user`**：始终注册且 LLM 可见（`optional_tools.rs:403-415`），不在预算表 → `None` → 120s。而它阻塞等待 `DEFAULT_CLARIFY_TIMEOUT = 600s`（`src/clarification/session.rs:41`，await 于 `ask_user.rs:387`）。**用户回答慢于 2 分钟 → 整个 run 被 StalledTurn 杀掉**。F7 的阻塞式澄清在生产里根本到不了 600s。
  - **`subagent`**：`subagent_definition` 硬编码 `ToolDefinitionMetadata::default()`（`builder.rs:356-364`）→ `None` → 任何超过 120s 的子代理委派会杀掉父 run。
- **修法（0 harness 行）**：
  1. `LoopTool` 加默认 hook `fn max_duration_ms(&self) -> Option<u64> { None }`（`src/tools/runtime.rs`），`McpRegistryTool`（`src/tools/adapters/mcp_adapter.rs`）override 为「owning server 的 `timeout_seconds` + 余量」。
  2. `builtin_metadata` 解析链：`tool.max_duration_ms()` → `builtin_tool_budget_ms(name)` → `DEFAULT_TOOL_BUDGET_MS`。
  3. **覆盖全部 describe() 站点**：`scoped/builder.rs:337`、`handlers/builtin.rs:61`、`builder.rs::subagent_definition`、`src/tools/mcp_scope_view.rs:64-81`。
  4. **先给 `ask_user` 加显式行** `("ask_user", 630_000)`（> 600s），同步改 `table_size_matches_expected_count` 断言。
- **R10 红利（同一 PR 做）**：一旦所有生产 `describe()` 都返回 `Some`，act.rs 的 StalledTurn 分支（act.rs:581-634 串行、:1017-1077 并行、`STALLED_CALL_CAUSE`）失去生产消费者 → **净删约 −40 行**（run 级 stall 仍由 `think.rs:1571-1578` + `stall_tracker` 负责）。
- **验证**：`src/tools/tests/budget_defaults.rs` 断言每个 registry 出来的 def 都有 `max_duration_ms`；`src/harness/tests/act.rs` 加「无预算工具超时 → 得到 ToolError 而非 Err(StalledTurn)」。

#### B5. 每工具 wall-clock 时钟在 **HITL 审批门之前**起表 → 人的思考时间算进工具预算，能在中途杀掉一条已批准的命令
- **锚点**：`src/harness/agent/act.rs:556-560` 构造 `exec_fut`，`act.rs:579` `tokio::time::timeout(budget, exec_fut)` 包住**整个** future（并行孪生 :957-959）。而人的等待就在这个 future 里面：`src/tools/scoped/dispatch.rs:513-515`（`requester.request_approval(action).await`）在 routing/执行（dispatch.rs:269-332）之前。`DEFAULT_APPROVAL_TIMEOUT_MS = 120_000`（`src/exec/manager.rs:17`），而 `bash`/`code_exec` 预算是 180s（`src/tools/budget.rs:42-43`）。
- **附带破坏一条已记录的不变量**：FEATURE_LOCATOR:282 记载 `CodeExecTool` 把前台 timeout 夹到 `FOREGROUND_MAX_TIMEOUT_SECS = 170`，专为「坐在 180s wrapper 下 10s」而设 —— **任何 > 10s 的人工审批都会静默摧毁这条不变量**。
- **修法（把整个 Act 期时钟下沉，不能只沉一半）**：
  - harness 侧删：`resolve_effective_budget`(92-97)、`budget_overrun_cause`(115-124)、`STALLED_CALL_CAUSE`(99-113)、串行 timeout wrapper + describe 探针(562-603)、StalledTurn 恢复块(604-635)、并行的 describe 探针(897-908)/`budgets` 线程(798-799,946,958-972)/PASS-2 `Err(elapsed)` 臂(1017-1041)/`first_stall`(1009,1071-1076)。`ExecOutcome` 收敛为 `Result<ToolOutput, ToolError>`。
  - `self.deps.turn_timeout` **保留唯一合法归宿**：`think.rs:1571`（`race_llm_call` —— LLM 调用没有人工门）。
  - tools 侧加 ~12 行：`src/tools/scoped/dispatch.rs::execute_inner` 在 confirm 门关闭之后（dispatch.rs:239 之后）包 `tokio::time::timeout(builtin_tool_budget_ms(name), …)` 并返回 `ToolError::Timeout`。
- **src/harness/ 净行数：约 −25/−30**（诚实计数：act.rs:1297 起的 3 个内联测试**本来就不在预算内**，budget.rs:95-99 —— 不能算进红利）。
- **注意**：生产 `turn_timeout` 默认 120s（`orchestrator_init.rs:443-450`），与审批超时同长度，所以**不能**保留 turn_timeout 包在 Act 的 future 上，否则人一慢就是整 run 中止（比现状更差）。
- **验证**：`src/harness/tests/act.rs` 新增「3s 预算的工具 + 5s 审批等待 + 1s 执行 → 成功」。

#### B6. 一次 wall-clock 超时被写成**跨批次永久失败**，于是 act.rs 自己写的「retry」提示保证会被自己拒绝
- **锚点**：`src/harness/agent/act.rs:588-593` 把超时造成 `ToolError::Execution { cause: budget_overrun_cause(..) }`，文案是 "…retry, narrow the query, or switch source/tool"（act.rs:119-124）；`Err(e)` 臂**无条件** `record_failure`（act.rs:654；并行 :1059）；下一轮同调用在 preflight 被杀（act.rs:475 / :868），回以 `CROSS_BATCH_REFUSED_CAUSE`（act.rs:102-103）。
- **两个加重项**：(a) 因为是 `Execution` 而非 `Timeout`，`is_retryable()` 为 false（`src/tools/service.rs:50-52`），`classify_error_str` 的超时探针只认 "timed out after"/"execution timeout"（`error_kind.rs:107`），于是 hint 打成 `kind=execution` + 错误的 switch-list（`fallback_registry.rs:295-318`）。(b) 同一条无条件 `record_failure` 吞掉审批过期 —— `DenialLedger::record_denial` 故意丢弃 `DenialReason::Timeout`（"a timeout is not a decision"，`denial_ledger.rs:224-233`），act.rs 却在上一层把它永久 ban。
- **修法**：act.rs:589-592 / :966-969 改造为 `ToolError::Timeout { name, elapsed_ms: u64::try_from(budget.as_millis()).unwrap_or(u64::MAX) }`（注意 `as_millis()` 是 u128，用 act.rs:1265 已有惯用法）；删 `budget_overrun_cause`（10 行）；两处 ledger 写入加 `if !e.is_retryable() { … }`。
- **src/harness/ 净行数：约 −6**。（若 B5 先落地，超时改由 tools 层产生，本项 harness 侧只剩 ledger guard。）
- **验证**：改 `src/harness/tests/task10_wiring/mod.rs:1055` 的 `contains("wall-clock budget")` → `contains("timed out after")`；**新增缺失的回归**：超时后下一批次同调用**不得**被跨批 dedup 拒绝。血径说明：guard 同时豁免 `Transport`（service.rs:50-52），这是对的，但要写进 commit message；**不要**放宽成 `kind().is_transient()`（那会连 RateLimited/UpstreamServerError 一起豁免）。

#### B7. `max_output_tokens` 续写把已生成的半截回答丢了：既不落库，非流式轮次连推送都没有
- **锚点**：`src/harness/agent/think.rs:717-721`（partial 只 push 进**局部** `messages`，注释 :693 自陈 "local, never persisted"），`response` 随后被续写整体替换（:729-750）；落库只用续写文本（:862 → :926-940）。`ProviderResponse::text_content()` 无累积（`src/providers/adapter.rs:286`）。
- **触发条件**：provider 触顶 `StopReason::MaxTokens`（anthropic sse.rs:169 / openai_chat sse.rs:189 / gemini sse.rs:149 / ollama.rs:387 均可产）。后果：(a) 会话 log 里的回答从半句开始；(b) 下一轮 prompt 由 log 重建（prompt.rs），模型自己也看不到前半截；(c) 任何非流式轮次（mock provider，或**任何配了 output guardrail 的部署** —— `may_stream_deltas` 此时返回 false，think.rs:169-175）用户**完全收不到**前半截。thinking / thinking_signature 全路径丢失。
- **修法（必须在护栏之前拼接，循环内不得 emit）**：
  1. 循环前 `let mut carried = String::new(); let mut streamed_prefix_len = 0usize;`
  2. 循环内（think.rs:717-720）记录字节 + 已流式发出的前缀长度，**不发任何 delta**（否则会绕过 Stage-5a `evaluate_output`，重开 FEATURE_LOCATOR §3.1 fix ① 关掉的那个泄漏面）。
  3. think.rs:862 先拼接：`let text = format!("{carried}{}", response.text_content());` —— 让**已有的** guardrail 块（:874-903）看到完整输出，Block/Sanitize 自然覆盖前半截。
  4. 一次性 emit（:916-918）只发未流式过的后缀：`if let Some(rest) = text.get(streamed_prefix_len..)`（P7 UTF-8 安全，用 `.get()` 不用 `&text[..]`）。
- **src/harness/ 净行数：+8** —— **用同一 commit 把 `MAX_OUTPUT_TOKENS_RESUME_NUDGE`（think.rs:88）挪去 `src/thinker/nudges.rs` 偿付**（见 §三 S4），这本来就是 R9 欠账（src/harness/CLAUDE.md:30）。
- **验证**：`src/harness/tests/task10_wiring/extras.rs` 现有 `max_output_tokens_recovery_*`（:1031-1148）只断言 `call_count`/`terminate_reason` —— 补断言持久化的 AssistantMessage 文本 == partial + continuation，且非流式轮 `on_delta` 收到全文。

#### B8. `build_prompt` 没有 `SessionEvent::SystemMessage` 分支 → session-split 子会话的 `[Context Summary]` 被静默丢弃（OpenAI-compat 客户端的 system message 同样丢）
- **锚点**：`src/harness/agent/prompt.rs:239`（`_ => {}` 吞掉一切）；生产者 A：`src/context/compact/session_split.rs:113,166-173`（`build_summary_event` → `SystemMessage`），生产者 B：`src/gateway/openai_api/completions/agent.rs:329-333`。**同一个文件里的 sibling mapper 是映射它的**（session_split.rs:242-246 `SystemMessage => UnifiedMessage::user`）—— 说明 harness 的缺失是疏漏而非约定。
- **触发条件**：`[context_budget]` 开启 + SQLite boot（`session_epoch_registrar: Some(...)`，orchestrator_init.rs:66-72,314）→ 跨临界阈 → `SplitSession` → 子会话 prompt = 孤儿 ToolResult 降级注记（prompt.rs:165-171，其 doc 自己就点名 "split-child head"）。**摘要没了、原始任务也没了**，split 的全部意义归零。
- **修法**：prompt.rs 加一个 arm，**必须走 deferral buffer**（照抄 UserMessage 臂 prompt.rs:155-159），不能用「`expected_results.is_empty()` 才 push」的条件跳过（那正是原 bug）：
  ```
  SessionEvent::SystemMessage { content, .. } => {
      let msg = UnifiedMessage::user(content.clone());
      if expected_results.is_empty() { messages.push(msg); } else { deferred_user_msgs.push(msg); }
  }
  ```
  `assistant_emitted` 不动（对齐 compactor.rs:447 的裸 user message）。
- **src/harness/ 净行数：+6**（若 prompt.rs 已按 §三 迁出，则为 0）。**同 commit 必须抬 `CEILING` 并写 3 问答**（budget.rs:60-76 有 5994→5997 先例）。
- **⛔ 拒绝「零 harness 行」的生产者侧变体**（让 session_split 发 `UserMessage{synthetic:true}`）：projection 按事件 **kind** 映射（`src/session/projection.rs:29-43`、`store.rs:621`），那会让压缩摘要在 Panel 与 `sessions` 工具里冒充成用户发言。
- **验证**：`src/harness/tests/prompt.rs` 加 `system_message_is_replayed`；`src/harness/tests/task10_wiring/extras.rs` split 用例断言子会话首轮 payload 含 `[Context Summary]`。

#### B9. `resolved` 集合扫描「整个未来 log」而非本轮窗口 → 后轮复用同一 call_id 会「复活」前轮孤儿 tool_use，provider 400
- **锚点**：`src/harness/agent/prompt.rs:103-110`（`events[idx + 1..]` 全量未来）；prompt.rs:352-357 据此保留 tool_use；`open_tool_use_ids` 是 HashSet（:117-123），重复 id 静默 no-op，单条 ToolResult 只消费一次 → **两个 tool_use + 一个 tool_result** → 400。
- **三个会撞 id 的生产者**（第三个审计者漏了）：`src/providers/protocols/gemini/sse.rs:100-107`（`gemini_fc_{n}`，counter 每请求重置）；`src/providers/delta.rs:359`（`json_{name}`，按名字确定性）；**`src/providers/ollama.rs:376`（`ollama_call_{i}`，每响应 enumerate 从 0 起，doc 还自称 "stable per-response ids"）**。
- **触发条件**：**用户点一次 Stop/Interrupt 就够**，不需要 daemon 崩溃 —— `gateway/execution_engine/execute.rs:510-527` 的 `tokio::select!` 在取消时 drop 掉 act() 中的 future，AssistantMessage 已落库、ToolResult 没落 → 孤儿。下一轮首个 functionCall 又叫 `gemini_fc_0`，其 ToolResult 复活了孤儿。
- **修法（0 harness 行，沿用仓库自己的先例）**：`promoted_{i}_{nonce}`（think.rs:277-288，FEATURE_LOCATOR §3.1 fix ②）就是同一 bug 的既定解法 —— 在**源头**给 id 加 per-response nonce：
  1. `gemini/sse.rs:102`（同步改 `gemini/tests.rs:115` 的字面量断言为前缀/唯一性断言）；
  2. **`ollama.rs:376`**（提案漏了这个，不改则 Ollama 用户 bug 依旧）；
  3. `delta.rs:359`（其 :357-358 的注释「确定性以便 harness 关联结果」是**错的/过时的**：关联走持久化的 `call_id`，全仓无任何代码从名字/前缀反推 id）。
- **⛔ 不做 prompt.rs 的「窗口收窄」加固（+3~15 行）**：源头修完后本仓零个撞 id 生产者，「未来某个 OpenAI 兼容代理」无代码锚点 = 为未来留口（src/harness/CLAUDE.md:47）。
- **验证**：`src/harness/tests/prompt.rs` 加回归（测试不计预算）：turn N 留孤儿 id X，turn M 重发 X + 其 ToolResult，断言 `build_prompt` 丢弃 turn N 的孤儿。

#### B10. Goal/loop 续跑静默丢掉 project root —— 其他所有子 run 生产者都继承它
- **锚点**：`src/gateway/execution_engine/execute.rs:1060-1061`（`spawn_continuation_run` 硬编码 `sandbox_override: None, workspace_override: None`），而它就在 `post_run` 里、`request` 在作用域内（同块 :759 读 `carry_policy_metadata(&request.metadata)`）。对照：steering `steering.rs:288`、team `teams/dispatcher/runner.rs:238`、session.send `send_tool.rs:350`、resume `resume_coordinator.rs:388` **全都继承**。
- **后果**：`run_loop/inner.rs:66-83` 回落到 `agent.workspace()`；`run_loop/mod.rs:127-157` 据此建 `FsScope` / `ToolContext` / `with_project_root` task-local；`inner.rs:331` 直接跳过项目 CLAUDE.md/AGENTS.md 与项目 skills；`inner.rs:355` 的 workspace_directive **主动告诉模型**它的 cwd 是 agent workspace。且 `src/goal/types.rs` / `src/looping/types.rs` 都不持久化 project root，丢了不可恢复。run 被标 `unattended`（execute.rs:1026-1031），没人看着。
- **修法**：`spawn_continuation_run`（execute.rs:1039）加 `workspace_override` 参数，三个调用点传 `request.workspace_override.clone()`（execute.rs:814、goal_continuation.rs:124/305），**以及 AgentBusy 重试再生成路径（execute.rs:1197-1206，把它 clone 进 `retry_workspace`，与已有的 `retry_policy_meta` 并列 execute.rs:1126）**。⛔ 不要改用 `projects::run_context::current()` —— 该 task-local 在 `run_agent_loop_inner` 结束时就关了（run_loop/mod.rs:156），hook 在其之后跑，读到 None。
- **src/harness/ 净行数：0**（harness 从不见此字段）。
- **验证**：`src/gateway/execution_engine/tests.rs` 加「project 模式 run 的 goal 续跑 RunRequest 带 `workspace_override == Some(project)`」。

#### B11. `ResumeCoordinator` 从零重建 metadata → 丢掉 channel exec-tier 钳制，这正是 `carry_policy_metadata` 为之而生的 bug 类
- **锚点**：`src/gateway/resume_coordinator.rs:355-374`（`HashMap::new()`，只塞 `resume` + 可选 `project_root`）。缺失的两个键都是**限制性**输入：`turn_permissions.rs:41-45`（`caller_role` 为 None 则**不做** `clamp_tier_for_channel`）、:101-103（channel `ToolPermissionsConfig` deny 层不合并）。更狠：`src/tools/turn_context.rs:52-53` `role_is_operator(None) == true`，`src/tools/scoped/dispatch.rs:146-148` 于是让 resumed run 直接过 config-tier 门。
- **触发条件**：resume 默认开（`src/config/types/resume.rs:28-30`），扫描无 channel 过滤（`src/session/store.rs:460-471` 全会话），Telegram 会话即 `SessionKey::DirectMessage`。**进程被杀 → 重启 → 一条 Chat 档的 Telegram run 以 operator 身份、无 deny 层、无人看守地复活。** 对照 `steering.rs:260-278` 是 `request.metadata.clone()`，只有 boot coordinator 从零重建。
- **修法**：走**实时重导**分支（不是持久化快照 —— 快照会以宽松方向变陈旧）：把 channel config map（或一个共享 resolver）给 ResumeCoordinator，用 `channel_run_identity`（`src/gateway/inbound_router/executor.rs:33-49`）从 session origin channel 同时导出 `caller_role` **和** `CHANNEL_TOOL_PERMISSIONS_KEY`（只补 caller_role 不够），保留其 fail-closed `unwrap_or_default()`（未知 channel ⇒ guest）。另：无可路由 origin `(channel, conversation)` 时打 `UNATTENDED_KEY`（审批无处落地 ⇒ fail closed）。注意 `origin_route`（`agent_instance.rs:435-443`）返回的是 `(channel, conversation)` 而**不是** role。
- **src/harness/ 净行数：0**。
- **验证**：`tests/resume_coordinator_integration.rs`（现有 :26-49 只断言 resume 信号）加：guest channel 会话被 resume 后，`request.metadata["caller_role"] == "guest"` 且 channel perms 键存在。

#### B12. 递减收益（diminishing）宽限轮用「本轮之前」的事件日志判断「用户是否已拿到终稿文本」→ 可能把刚刚答完的一轮再答一遍
- **锚点**：`src/harness/agent/think.rs:1164-1173`（唯一传 stale `&events`/`&messages` 的 grace 站点；`events` 取自 :361，本轮 AssistantMessage 落库于 :926-939 之后从未刷新）。跳过守卫 `last_assistant_has_text(events)` 在 think.rs:1680，随后无条件 `on_delta`（:1767）。**其他所有 grace 站点**（think.rs:1023、agent.rs:550/597/713/746/771/821）都走 `fire_boundary_grace_turn`，后者**重取** log（:1806）。
- **触发条件**：`[context_budget]` 开启（默认关，故降级为 high 而非 critical）。现有测试 `diminishing_returns_fires_grace_and_hits_limit`（`src/harness/tests/task10_wiring/mod.rs:594-654`）**机械地复现了双答**并把它当成期望行为（断言 `call_count == 2`）。
- **修法（净负）**：think.rs:1164-1173 改调 `fire_boundary_grace_turn`；然后 `fire_grace_turn` 只剩一个调用方（是 100% 透传壳）→ **合并两者**，保留 `fire_boundary_grace_turn` 之名（agent.rs 五个站点零改动），只留**一个** `last_assistant_has_text(&events)` 守卫（放在 `get_events` 之后、`build_prompt` 之前）。
- **src/harness/ 净行数：约 −25**（优于提案自称的 −12）。
- **诚实声明（写进 commit）**：这**不是**「与其他 grace 站点语义相同」：现场的 `events` 是被输入护栏 sanitize 过的内存副本（guardrails.rs:46-51），而 `fire_boundary_grace_turn` 重读的是 raw store。改完之后 diminishing grace 也会把**未脱敏**的用户文本发给 provider —— 这不是新洞（五个既有 boundary grace 站点早就如此），但要显式接受，并作为独立 finding 跟进（覆盖全部 6 个站点）。
- **验证**：改 `mod.rs:594-654` 使其断言「本轮已产出文本时 grace **不**触发」（`call_count == 1`）；另加一条「本轮以未决 tool_use 结束 → grace 的 prompt 里**包含**本轮 assistant message 与 tool results」。

---

### 🟡 MEDIUM

#### B13. 压缩与确定性 floor 把用户自己的消息当普通可压缩历史 —— 原始指令是**第一个**被摘要的、也是**第一个**被删的（codex 逐字保留每条 user message）
- **锚点**：`src/context/compact/fit.rs:36-38`（`while … { messages.remove(0); }`，只保护 `protected_tail`，无角色感知）；`src/context/compact/compactor.rs:286`（窗口 `snap_boundary_forward(messages, 0)` **头锚定**，原始任务在第一个被替换的窗口里）。唯一的用户意图保护是 `latest_user_task(&messages[cut_end..])`（compactor.rs:365）—— 读的是**幸存尾段**，且只做 ≤600 字符的 focus hint（summary_utils.rs:160-176）。对照 codex：`codex-rs/core/src/compact.rs:53`（`COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000`）、:499 `collect_user_messages`、:600-624 逐字重排。
- **更锋利的一击（审计者漏了）**：在 `compact_to_fit` 里 compactor 先跑，`messages[0]` 此时**就是刚花了一次侧信道 LLM 调用生成的 `[Context Summary]`**（compactor.rs:407）—— floor 的第一个 `remove(0)` 把它删了。
- **修法（0 harness 行，全在 `src/context/compact/`）**：
  1. 抽 `preserved_user_messages(window, budget)`，在**所有四个 drain 站点**调用 —— 不只是 LLM 摘要路径：`compact_inner` LLM 路径(:406-407)、截断兜底(:424-425)、**`reapply_cached` 两个分支(:496, :569)**、`SessionSummarySource::try_reuse`(:340)。只补第一个会让保留的用户消息在每个 cache-hit 轮闪现消失（`CacheReuse` 才是稳态路径）。跳过以 `CONTEXT_SUMMARY_PREFIX` 开头的窗口（codex 在 compact.rs:504 同样处理）。
  2. **按时序输出**（codex 只是为花预算才 newest-first，之后 `reverse()`，compact.rs:623），user messages 放在 summary **之前**。
  3. `fit.rs::truncate_to_fit` 角色感知：优先驱逐 assistant/tool，**且必须把 assistant ToolCall 与其 ToolResult 作为一个单元一起驱逐** —— 否则 `src/providers/message.rs:461-467` 会合成 `is_error=true` 的 "No result provided — tool call was interrupted"，**凭空捏造一次工具失败**并且还增加 token。保留现有的 leading-orphan snap。
- **验证**：`src/context/compact/tests`：长会话跨压缩后断言首条用户约束逐字仍在；cache-reuse 轮同样在；floor 驱逐后不产生孤儿 ToolResult。

#### B14. Layer-2（8k/结果）与 Layer-3（50k/轮）工具输出预算是固定常量，对模型窗口全盲 —— hermes 已把两者按窗口缩放
- **锚点**：`src/tools/result_processing.rs:28`（`DEFAULT_RESULT_BUDGET_TOKENS = 8_000`，`resolve_result_budget` :47-66 从不看模型）；`src/tools/turn_budget.rs:24`（`DEFAULT_MAX_TURN_TOKENS = 50_000`，其 doc 自陈 "Mirrors hermes' MAX_TURN_BUDGET_CHARS" —— 镜的正是 hermes **后来修掉**的常量）；进程级单例安装于 `mod.rs:2458-2461`。而真实窗口就在同一个 match 臂里（`runner_impl.rs:371` 的 `cfg.token_budget`，由 `deps_builder/context_budget.rs:176-195` 从 provider/capabilities 导出）。
- **后果（32k 本地模型）**：单条 bash 结果 8k = 全窗口 25%；三条并行 = 24k 落在**受合同保护、压缩不可动的** fresh tail（fit.rs:35-38 `protected_tail.max(1)`，三个 cheap pass 全部 `protects_fresh_tail`）；50k 的 Layer-3 上限是窗口的 156%，**永远不会触发**。叠加 B3，这个溢出在这些 OpenAI-compat 端点上是 Fatal。
- **修法（0 harness 行）**：
  1. `src/tools/turn_budget.rs` 加 `budget_for_window(token_budget) -> (per_result, per_turn)`：`per_result = clamp(0.15*w, 2_000, 8_000)`、`per_turn = clamp(0.30*w, 4_000, 50_000)` —— **向上夹到现常量**，大窗口模型逐字节不变。诚实表述：这是**小窗口下钳**，不是「大模型自动变大」。
  2. per-turn：在 `runner_impl.rs` 的 `Some(cfg)` 臂用它构造 `TurnResultBudget`（喂 `HarnessDeps.turn_budget`，:519-522）。
  3. per-result：**⛔ 绝不能穿 `ToolService::execute` 签名**（那会改 `src/harness/agent/act.rs`，往超预算的树加行）。改在 boot 接缝（`mod.rs:2458-2461`，`OnceLock<usize>` 给 `resolve_result_budget` 读），或给 `build_request_tool_service` 加一个参数（`run_loop/inner.rs:686, :910`）。
  4. 把它当作**上限**而非仅 `None` 分支默认值 —— 否则 `web_fetch` 的 10_000（result_processing.rs:63）和显式声明值绕过缩放。
- **验证**：`src/tools/tests/turn_budget.rs`：16k 可用窗口 → per_turn < 窗口；200k 窗口 → 与今天完全相同。

#### B15. 子代理 Think→Act 循环可以**无迭代上限** —— 主路径的「永远有 cap」不变量在 spawn 路径没有对应物
- **锚点**：`src/agents/subagent_spawner/mod.rs:403-408, :433`（`max_iterations: max_iter`，直接透传 `Option<u32>`）；`HarnessDeps` 明写 "None → unbounded"（`src/harness/deps.rs:98-101`），执行点 `agent.rs:520/760/815` 全是 `if let Some(limit)`。主路径永不为 None：`runner_impl.rs:230-233, :509`（`src/orchestrator/harness_bridge/mod.rs:141-144` 直接写着 "The harness loop is never left uncapped"）。
- **零配置即可命中**：内建 `"default"` 子代理（模型省略 `agent_type` 时的落点，`subagent_tool/loop_tool.rs:465-467`）在 `src/agents/registry.rs:268-272` **没有** `.with_max_iterations(...)`（explore/coder/researcher/plan/verify 分别是 20/30/15/20/25）。
- **实际伤害修正**：不是无限跑 —— 每次 spawn 都被 `tokio::time::timeout(req.timeout_secs)` 包着（mod.rs:473-481，默认 120s）。真正的损失是**质量**：超时被杀返回 `Err("Sub-agent timed out")`，成果全丢；而命中 cap 会触发 boundary grace turn（agent.rs:771-780）返回可用摘要。
- **修法**：`FlowRunner` trait 加默认方法 `fn default_max_iterations(&self) -> Option<usize> { None }`（`src/orchestrator/dispatch.rs`，与已有 `stall_config()`:585 / `consecutive_failure_cap()`:592 并列），像 `with_stall_config`（`run_loop/inner.rs:842-850`）一样穿到 `SpawnerBase`；spawn 点改用**已导出的** `resolve_max_iterations(None, req.agent_def.max_iterations, base.default_max_iterations)`（`harness_bridge/prompt_build.rs:613-623`，含 `.filter(|&n| n > 0)`）。**⛔ 不能写成裸 `.or(...)`** —— `[execution] max_iterations = 0` 或 frontmatter `0` 会让每个子代理跑一轮就死（`resolve_max_iterations_never_returns_zero`，harness_bridge/tests.rs:549-555 就是为此而设）。
- **src/harness/ 净行数：0**。
- **验证**：`src/agents/subagent_spawner/tests.rs` 加「`AgentDef` 无 max_iterations 时 deps 拿到 `Some(default)`」「配置为 0 时不会退化成 `Some(0)`」。

#### B16. `reactive_fit_and_retry` 的 still-overflow 分支丢弃一次已计费响应且不折算 usage —— 最后一个未记账的往返
- **锚点**：`src/harness/agent/think.rs:1531-1542`（`Ok(_still_overflow) =>` 绑到 `_` 后 drop）。**所有兄弟丢弃点都记账**：空响应循环(:631)、max_output_tokens 循环(:711)、overflow drain(:1233)、grace turn(:1730)；不变量写在 `account_intermediate_tokens` 的 doc（:308-321）。该响应确实计费（anthropic `message_delta` 同帧带 usage，`sse.rs:155-183`）。
- **修法（净负，不是 +1）**：:1531-1542 与 :1543-1554 两个失败臂逐字节同构（只差 surface 哪个 error），**合并**为一个 `other =>` 臂，`if let Ok(r) = &other { self.account_intermediate_tokens(r); }`，行为完全等价，**净删约 8 行** → CEILING 下调而非上调。顺手补 `account_intermediate_tokens` 的 doc（:310-321 只列了两个循环）。
- **src/harness/ 净行数：约 −8**。
- **消费者澄清**（写进 commit）：真实消费者是 `total_tokens()`(agent.rs:271) 与 `token_breakdown()`(agent.rs:287) → `runner_impl.rs:762` → FlowOutcome；`metadata["session_id"]` 的计费 hook 与此无关，别写进理由。
- **验证**：`src/harness/tests/reactive_compaction.rs` 现有 mock 全是 `usage: None`；加一条带 usage 的 still-overflow mock，断言 `harness.total_tokens()` 包含它。

#### B17. 并行准入用「模型原始 args」算 ConcurrencyClaim，PASS 1 却执行「护栏改写后的 args」→ 数据竞争守卫按构造已失效
- **锚点**：`src/harness/agent/act.rs:199-204` / `:719-728`（claim 用 `&call.arguments`）；PASS 0 的护栏可改写（`act.rs:840-846` `Sanitize` → `sanitized[idx]`）；PASS 1 执行改写后的（`act.rs:943-945`）。改写是真实的：PII 掩码把不同路径塌成**同一个常量占位符**（`[PHONE]`，`phone.rs:81-83`；默认开启 `runtime_guard.rs:38-48`），而 `file_write`/`file_edit` 的 claim 正是从 path 导出（`registry_adapter.rs:136-145`）且默认 Auto 档不需确认 → 会并行。
- **具体复现**：`file_write("/data/customers/13800138000.md")` + `file_write("/data/customers/13900139000.md")` → claim 不相交 → 准入并行 → PASS 0 双双掩码成 `/data/customers/[PHONE].md` → **两个并发截断写打同一个文件**，分区的不相交性证明作废（`concurrency.rs:252-262`），底层无锁（`file_ops/write.rs:131-152`）。
- **修法（+2 行）**：act.rs:918 之前先绑定，让三元式不超 50 列（否则 rustfmt 展开成 5 行）：
  ```
  let cap = self.deps.parallel_tool_concurrency.unwrap_or(0).max(2);
  let rewritten = sanitized.iter().any(Option::is_some);
  let parallelism = if rewritten { 1 } else { cap };
  ```
  `.buffered(1)` 保持所有事件/trace/预算发射逐字节不变，只串行化实际派发。
- **理由必须只留一句**：「护栏改写会改变 claim 所依据的路径集合，故准入前的资源不相交证明作废；一旦有改写就串行化该批次。」**删掉 `newest_tool_call` / 审批卡盖错 id 那条腿** —— 已证伪：该不变量是**发射邻接性**（PASS 0 已为整批预发 `ToolCallRequested`，act.rs:800-820），`buffered(1)` 恢复不了；且 sanitize 不可能把 `operation` 从只读翻成 destructive（`exec_tier.rs:160-168`），confirm 门永远不会被偷渡。
- **src/harness/ 净行数：+2**（同 commit 抬 CEILING → 5999，并诚实写明这是 low-severity 窄竞态）。
- **验证**：`src/harness/tests/guardrails.rs` 现有并行护栏测试用的是 `ConcurrencyClaim::Shared` fixture（:888-897，永远碰不到 Paths claim）；新增一条 Paths claim + Sanitize 改写 → 断言 `parallelism == 1`。

#### B18. 后台子代理的 50 条进度轨迹在完成时被丢弃 —— **失败**的并行探索到父代理手里只剩一个 error 字符串
- **锚点**：`src/agents/background_tracker.rs:251-300`（`mark_completed` 搬走 task/meta/started_at/tool_count/last_tool，**不搬 `progress`**，:99-100 的 50 上限 VecDeque 随 `RunningAgent` 一起 drop）；`progress_snapshot` 只读 `self.running`(:532-545)；失败分支 `src/agents/subagent_tool/loop_tool.rs:305-310` 只返回 `ToolResult::Error{error}`，`completed_to_json` 的 Err 臂(:910-916)无 progress。Weng：「负面结果必须被刻意保留；并行探索必须可经日志/状态记录检视」。
- **修法（0 harness 行）**：
  1. `CompletedAgent.progress_tail: Vec<SubagentProgress>`（在 `mark_completed` 里带上尾部 10 条），`CompletedSnapshot` 暴露之，`progress_snapshot` 对非 running id 回落到 completed map。
  2. 失败路径**保持 `ToolResult::Error` 的形状不变** —— ⛔ 不要给 `ToolResult::Error` 加字段（`src/tools/runtime.rs:17`，它在 `src/harness/trace.rs:281` 被穷举解构、在 `src/harness/agent/act.rs:1281` 被构造，加字段=往 harness 加行）；把有界压缩后的轨迹**追加进现有的 `error: String`**（这正是 A2 的错误压缩）。也不要改成 `Success{status:"failed"}`（会抹掉 harness 连败计数所依赖的失败信号）。
  3. Ok 臂与 `list` action 加 `"progress"` + `"summary"`。
- **⛔ 撤回 step (4)（把子会话真实 SessionKey 透出）**：今天没有任何工具能按 key 读会话事件日志（`src/builtin_tools/sessions/` 只有 list/new/send/set_topic/spawn）→ 零消费者抽象。
- **验证**：`src/agents/subagent_tool/tests.rs`：后台子代理失败后 `check_status` 的 error 串包含最后一个工具名与步数。

---

## 二、死连线与 YAGNI 撤回（连或删）

| # | 项 | 动作 | src/harness/ 省行 | 锚点 |
|---|---|------|------|------|
| D1 | `HarnessDeps.chain_context` 整条访问链 | **删 + 移文件** | **−125**（chain_context.rs 100 + 26 wiring；12→11 文件） | 唯一非测试读者 `src/harness/agent.rs:408`，其唯一调用者是 trait override `agent.rs:499-503`，而 trait 方法的唯一调用者是**测试** `src/harness/tests/chain.rs:113-206`。真正喂给模型的 chain 走 `subagent_spawner/mod.rs:357` → `PromptBuilder::with_chain_context` → `src/thinker/layers/chain_context.rs`，与 harness 副本无关。`git mv src/harness/chain_context.rs src/agents/chain_context.rs`（src/thinker 已 import crate::agents，无新依赖边）。 |
| D2 | `ChainContext::with_max_depth` + `Display` impl | **删** | −20（含 `use std::fmt;`，否则 `just clippy -D warnings` 挂） | `with_max_depth`（chain_context.rs:52-60）调用方全在 `#[cfg(test)]`；`Display`（:91-99）唯一使用者是自己的 `display_format` 测试。顺手修 `src/tools/runtime.rs:77` 引用的**不存在**的 `ChainContext::cancellation_token`。（被 D1 的整文件外迁包含。） |
| D3 | `TraceSink::on_init_seam` + `emit_init_seams` | **删** | **−10**（repo ~−100 + 1 个测试文件） | trait 默认空实现 `src/harness/trace_sink.rs:16-24`；每轮 run 发 7 个事件（`harness_bridge/mod.rs:63-90`，调用于 `runner_impl.rs:561-570`）；5 个生产 impl 全是纯转发；**两个叶子 sink 都不 override**（`trace_sink_adapter.rs:64-71`、`trace_sink.rs:30-33`）→ 全部落到默认 `{}`。同样的信息 5 行之后已经在活的 tracing 通道里（`runner_impl.rs:572-580`）。唯一有 body 的 impl 是测试 `src/orchestrator/tests/init_audit.rs:21`。**顺带**：CHANGELOG.md:1249 宣传的是「9 events」、init_audit.rs:3 写「eight」、代码发 7 —— 三份文档三个数字，一条死通道。 |
| D4 | `HarnessCallback::on_tool_call`（2 处发射）+ `on_complete`（**8** 处发射） | **删** | **−20** | `BroadcastCallback` **故意不实现** `on_tool_call`（`orchestrator/harness_bridge/callback.rs:56-63`，注释记载它曾双发 `ToolCallStart{id:"legacy"}` 且 `ToolCallDone` 永远配不上）；`on_complete` 被覆写成显式空 body（callback.rs:111），真正的终态事件来自 runner 的 `on_complete_with_outcome`（`runner_impl.rs:810`）。生产 impl 只有两个（`BroadcastCallback` + `NoopHarnessCallback`）。**第 8 个发射点在 `src/harness/trait_def.rs:62`**（提案漏了，不删则编译不过）。 |
| D5 | `Harness` trait 本体（默认 `run()` body 是死代码） | **删（完整撤回）** | **−55** | 生产 impl 唯一：`src/harness/agent.rs:499`（且它 override 了 `run`，所以默认 body 在生产**和测试**里都零执行）；`dyn Harness` 只出现在 doctest 与测试。真正的多态接缝是 `SessionDriver`（agent.rs:476）与 `Arc<dyn HarnessRunner>`（`src/orchestrator/dispatch.rs:496`）。trait doc 自陈 "future alternative drivers" —— 正是 CLAUDE.md:47 禁的「为未来留口」。`TurnState`/`TurnStep`/`HarnessError`/`TurnPhase` **全部保留**。附带：`tests/harness_run_e2e.rs:3` 自称「Exercises the default `run` loop」—— 已经骗到过一个文档作者。 |
| D6 | 子代理 `tool_signal_sink: NoopToolSignalSink` | **连上** | 0（+6 行在 src/agents） | `src/agents/subagent_spawner/mod.rs:448` 无条件塞 Noop，而 `SpawnerBase.raw_memory_writer`（mod.rs:60）在生产里是 Some（`run_loop/inner.rs:885-889`）且已被 Delegation emit 用着（mod.rs:511-526）。镜像 `runner_impl.rs:532-544` 即可。**唯一真实消费者是 `insights.tools` RPC**（`gateway/handlers/insights.rs`）—— ⛔ 别在理由里写「Dream 信号采集」，那是死的（`src/memory/dreaming/mod.rs` 零 `ToolInvocation` 读取，`compression/service.rs:314` 的注释已过时）。归属决定要写进 PR：按 `req.agent_def.id`（子角色 id）打标，与 `routing_store` 先例一致。 |
| D7 | 后台子代理 progress 轨迹（见 B18） | **连上** | 0 | `background_tracker.rs:251` |

**死连线总收益：−230 行 harness（D1+D3+D4+D5，D2 含在 D1 内）。**

---

## 三、R10 减重方案（5997 → ≤4900，需减 1097）

> 铁律（`src/harness/tests/budget.rs:27-34`、`src/harness/CLAUDE.md:15,19`）：**每笔减重必须在同一 commit 里下调 `CEILING`（budget.rs:77）并更新 src/harness/CLAUDE.md:17 的状态行**；测试断言是 `total <= CEILING`，不下调等于把省下的行数变成**隐形额度**，正是 budget.rs 存在的目的所反对的。**不靠删注释凑数。**

### 减重项与算术

| 项 | 手段 | Δ(harness) | 是否改 12 文件集 |
|---|---|---|---|
| **S1** trace DTO 下沉 | `trace.rs:245-464`（6 个 `From<LoopTrace*> for aleph_protocol::AgentTraceEvent`）整块搬到 `src/gateway/trace_protocol.rs`。孤儿规则允许（本地类型作 trait 类型参数，coherence 是 crate 级）。三个调用点全在 gateway（`execution_engine/callback.rs:26`、`event_emitter/mod.rs:123`、`event_emitter/types.rs:315`）。**副产品：src/harness/ 从此不再依赖 `aleph_protocol`**（`rg aleph_protocol src/harness/` 只命中这 6 个 impl）。非逐字：新文件需 `use crate::harness::trace::{...}` 6 个名字。 | **−221** | 否（trace.rs 留下 244 行） |
| **S2** reactive-compaction 救援簇下沉 | 新建 `src/context/compact/rescue.rs`（~330 行，预算外）。搬：`MAX_REACTIVE_COMPACT_ATTEMPTS`(think.rs:233)、`drain_context_overflow`(1216-1260)、`try_reactive_compact_and_retry`(1284-1463)、`reactive_fit_and_retry`(1477-1556)、`compact_to_fit_in_place`(1640-1658，唯一调用者 :1493 随之而去)。五个结构接触点用 `RescueHost` trait（关联类型 `Fatal`，保证 `src/context/` 零 harness import —— 今天 `rg "crate::harness" src/context/` 为 0）+ `RescueCx`（纯数据，**在 `let started`(think.rs:557) 之后构造**，不是 534）+ `RescueSlot`（CAS 一次性槽，harness 持实例、context 持策略）。harness 只剩 `impl RescueHost for AgentHarness`（~52 行）。 | **−367** | 否 |
| **S3** `chain_context.rs` 外迁（D1+D2） | `git mv → src/agents/chain_context.rs` | **−125** | **是（12→11）** |
| **S4** 模型可见文案下沉 `src/thinker/nudges.rs` | `MAX_STEPS_HINT`(think.rs:64-74)、`MAX_OUTPUT_TOKENS_RESUME_NUDGE`(think.rs:88-91)、`INTERRUPTION_NOTE`(prompt.rs:22-27)、G2 interjection `format!`(prompt.rs:138-145)、orphan-result `format!`(prompt.rs:327-333)、`CROSS_BATCH_REFUSED_CAUSE`(act.rs:102)、`STALLED_CALL_CAUSE`(act.rs:112)、`budget_overrun_cause`(act.rs:119)、deferred-result reason（**内联在 act.rs:305-306 的 `json!` 里，不是既有 const**）。长论证注释**随文案一起走**。R9 明文归宿：src/harness/CLAUDE.md:30。 | **−88**（think −33 / act −27 / prompt −28） | 否 |
| **S5** 死连线清扫（D3+D4+D5） | 见 §二 | **−85** | 否 |
| **S6** Act 期 wall-clock 时钟整体下沉（B5） | 见 B5。诚实计数：内联测试本来就不计预算（budget.rs:95-99），别算红利。 | **−28** | 否 |
| **S7** Act 期 StalledTurn 分支退役（B4 之后） | 一旦所有生产 `describe()` 返回 `Some`，`per_tool_budget.is_none()` 分支零生产消费者 | **−40** | 否（与 S6 部分重叠，保守只记 −40 中不与 S6 重叠的部分 ≈ **−12**） |
| **S8** `fit_and_retry` 两个失败臂合并（B16） | | **−8** | 否 |
| **S9** diminishing grace 与 boundary grace 合并（B12） | | **−25** | 否 |
| **S10** timeout→Timeout 变体 + 删 `budget_overrun_cause`（B6） | （文案行已在 S4 计过，此处只记 ledger guard 与结构） | **−6** | 否 |
| **加行项** | B7 `+8`（被 S4 的 nudge 外迁抵消）、B8 `+6`、B17 `+2` | **+16** | |

### 两个场景，诚实算术

**场景 A —— 不动 12 文件集（不修宪）**
```
5997
 −221 (S1)  −367 (S2)  −88 (S4)  −85 (S5)  −28 (S6)  −12 (S7)
 −8  (S8)   −25 (S9)   −6  (S10)  +16 (加行项)
= 5173
```
**距 4900 尚差 273 行 —— 这是诚实的残口。** 单靠不改文件集的手段**到不了红线**。

**场景 B —— 修宪，把两个非 harness 关切迁出（12 → 10 文件）**
在 A 的基础上再加：
```
5173 − 125 (S3: chain_context.rs 外迁, 12→11)
     − 476 (S11: prompt.rs → src/thinker/turn_prompt.rs 或 src/session/, 11→10)
= 4572   ← 低于 4900，留 328 行余量
```
- **S11 `prompt.rs` 外迁的依据与代价**：`build_prompt`(prompt.rs:43) 是**零 harness 状态**的纯函数（`rg 'self\.|&self|deps\.' src/harness/agent/prompt.rs` 零命中），这正是它与 Task-8 那个**被 BLOCK 的**下沉的区别（后者依赖 `&self` 的 AtomicU64/Mutex/CAS，src/harness/CLAUDE.md:27）。HARNESS_PHILOSOPHY.md:167 §4.2 第 5 行明写 **Prompt Assembly → `src/thinker/`**。
  - **但必须诚实**：`prompt.rs` 同时被 R10 的 12 文件清单点名（root CLAUDE.md:70、src/harness/CLAUDE.md:9、HARNESS_PHILOSOPHY §4.1:139），且 `budget.rs:126-144` 断言文件集**精确**等于这 12 个（`removed.is_empty()`）。**这是修宪，不是重构** —— 必须由人拍板，四处文档同 commit 改。
  - **也必须诚实**：外迁的真实消费者是 **2 个生产 + 2 个 harness 内调用点 + 3 个测试**（`harness_bridge/prompt_build.rs:39`、`runner_impl.rs:903`；think.rs:399、think.rs:1817；`session/in_process.rs:367/375/383` 在 `#[cfg(test)]` 里，:272 起）。**不要**沿用「4 个生产消费者在 harness 外」的说法 —— 那是错的。
  - 目的地二选一：`src/thinker/turn_prompt.rs`（R9 友好，让 INTERRUPTION_NOTE 等与 nudges.rs 为邻）或 `src/session/`（模块 #7，`in_process.rs:322` 已称之为「the harness's real replay path」；且 `parse_tool_use_block` 的写方 `tool_use_blocks` 留在 agent.rs:987，读写对的往返测试在 `tests/act.rs:835`）。**显式选一个，别默认**。

**若拒绝修宪**：残口 273 行的唯一其他出路是 `act-dispatch-belongs-in-tools`（把批执行引擎搬去 `src/tools/dispatch/`，−800…−950）。但它**不是纯搬运**：`stall_tracker`（deps.rs:192，非 Clone/非 Arc，run-loop watchdog agent.rs:536 与 think.rs:944 也读）、`tool_timeline`（agent.rs:114，run 结束后被 `runner_impl.rs:793` 读）、`last_prompt_seq`（agent.rs:174）都是跨切面 harness 状态；且 `dispatch_group` 内部有**第二个** steer checkpoint（act.rs:381），直接照提案实现会**静默删掉串行批次的中途 steering**（act.rs:44-57 记载它是活的 Pi parity）。→ 需要独立 spec（`DispatchHost` host trait，定义在 `src/tools/dispatch/`），是 L 级任务，**不列入本轮减重路径**。

---

## 四、增强（来自 codex / hermes / Weng，且合规）

| # | 来源 | 机制 | 落点（harness 净行数） | 为何不违 R10/R7 |
|---|---|------|------|------|
| E1 | **hermes** `agent/error_classifier.py:242-275`（~25 条 pattern + 结构化 error-code 分支） | 扩宽上下文溢出识别集（= **B3**） | `src/providers/llm_retry.rs`，**harness 0 行** | `RetryVerdict` 不增变体、harness 不增分支；只是让**已有的**唯一 `CompactAndRetry` 分支在更多 provider 上可达。这是 A2 的「让模型看见并自愈」侧，**不是** hermes 的多级重试矩阵（那条仍 DEFER）。provider 错误体是机器生成文本，子串匹配是 P8 允许的用法。 |
| E2 | **codex** `codex-rs/core/src/compact.rs:53,499,600-624`（`COMPACT_USER_MESSAGE_MAX_TOKENS = 20_000`，用户消息逐字保留） | 压缩后重新逐字附回窗口内的 user message；floor 角色感知（= **B13**） | `src/context/compact/{compactor.rs,fit.rs}`，**harness 0 行** | `UnifiedMessage::User` 是结构判别式（compactor.rs:763/790 已在按角色 match），不是意图分类；不涉错误恢复策略选择。 |
| E3 | **hermes** `tools/budget_config.py:85-115`（`budget_for_context_window`：per-result 15%、per-turn 30%，向历史默认值上夹） | 工具输出预算随模型窗口缩放（= **B14**） | `src/tools/turn_budget.rs` + boot 接缝，**harness 0 行**（⛔ 严禁穿 `ToolService::execute` 签名） | 纯算术，输入是 config 早已算好的 `token_budget`（`deps_builder/context_budget.rs:176-195`）；不看消息内容 → 非「按意图过滤」。 |
| E4 | **codex** `tools/{router,parallel,orchestrator}.rs`（turn loop 只向 tools 层要结果） | 把 Act 批执行引擎搬出 harness | `src/tools/dispatch/`（−800…−950） | 方向合规（HARNESS_PHILOSOPHY.md:164 模块 #2 归属 `src/tools/`），但**需要 spec**（见 §三 末）→ 列入 §六 的 L 级尾项，不是本轮。 |
| E5 | **Weng**「负面结果必须被刻意保留；并行探索必须可经日志/状态记录检视」 | 后台子代理失败时把进度轨迹压缩进 error（= **B18**） | `src/agents/{background_tracker.rs,subagent_tool/loop_tool.rs}`，**harness 0 行** | 把压缩后的错误交给模型自愈 = A2 正例；`summarize_progress`（loop_tool.rs:836-844）是 `max(step)` + 末事件，纯算术。 |
| E6 | **Weng**「文件系统即持久记忆」 | 工件存储/索引 session-scoped 化（= **B2**） | `src/tools/result_store.rs` + `content_index.rs` + `builtin_tools/ctx_search.rs`，**harness 0 行** | session_id 相等谓词是静态所有权 scoping，与 `src/tools/scoped/` 已有的 allowlist/权限/健康三道 `retain` 同层同性质，非意图过滤。 |

---

## 五、有意不做（DEFER）

### 沿用既有 DEFER（不重提）
hermes thinking-budget 耗尽中止 · hermes 多级重试矩阵 · codex 服务端建议重试延迟（活在 `FailoverProvider`）· codex rollout suffix 重建（session 层）· pi 会话树/分支摘要。
**新增强化**：codex `guardian/`（第二个 LLM 自动批准/拒绝审批）与 codex `tools/orchestrator.rs` 的沙箱提权重试阶梯 —— **后者已在 `docs/reference/SECURITY.md:902,913-921` 与 `FEATURE_LOCATOR.md:492` 记录两次**，不要写第三遍（多源漂移是 SECURITY.md:887-889 明文禁止的）；前者（guardian，R10 第 4 不：不做内容审查/安全打分）**确实是新的**，应记进 **SECURITY.md 的 Gap 表**（权限模型的 DEFER 清单），**不是** FEATURE_LOCATOR §3.1（那是 harness 架构区）。

### 本轮查过、确认无恙（一行说明，勿再报）
1. **3a 空响应重试排在 3b overflow drain 之前** —— 不构成 bug：`ContextWindowExceeded` 只由 `anthropic/sse.rs:179` 的 `message_delta` 产生（终帧，必已出过内容 → `is_empty_response` 为 false）；「prompt 一开始就超窗」走的是 `Err` 分支 → `classify` 直接 `CompactAndRetry`，首次调用即压缩。
2. **split 后 `last_prompt_seq` 是父会话水位** —— 现象真实（agent.rs:648 用父水位查子会话），但 `store(0)` 修法是 **fail-CLOSED 回归**：子会话种子里带着上一轮的 AssistantMessage+ToolError，会把已计过的错误**二次计入**连败计数、提前触发 cap。当前的空窗读至少是 fail-open。且唯一读者的结果不变。**不修**。
3. **批内 memo 跨中间写重放旧结果** —— 缺陷真实（act.rs:347 的 key 只有 name+args，资源盲），但审计者给的因果链是错的：删掉 `has_duplicate` 塌缩**并不能**阻止它（act.rs:216 会把无并行组的批次整体收进一个 `dispatch_group`，模块 doc act.rs:11-13 明写）。真正的修法在 memo 本身（需资源感知失效），**列为独立 backlog，不进本轮**。
4. **`dispatchable_list()` 只在 ScopedToolService override，子代理丢 deferred 层** —— 结构观察对，但**不可达**：`tool_search` 通过 `undefer()` 把名字从 deferred 集合里**移除**（`deferred.rs:101-112`，「The set only ever SHRINKS」），模型能合法发出该名字的那一刻它已不在 deferred 集合里。虽如此，`AllowlistToolService` 补上 override 仍是廉价的正确性加固（红线镜判 CONFIRMED，0 harness 行）→ 可选 S 级。
5. **grace turn 在 Anthropic 上 400（tools=None 但 prompt 带 tool_use）** —— 复现镜 CONFIRMED，但红线镜未过（该项被列入 survived 名单外）；**保留为待验证 backlog**，需要一次真实 Anthropic 请求确认，不要盲改。
6. **output guardrail 不扫 `thinking`** —— 复现镜 CONFIRMED（thinking 在 text 为空时被提升为最终答案：`runner_impl.rs:715-722`），红线镜未通过收口 → 与 B1 同属护栏 scope 问题，**并入 B1 的 registry 下沉后续**（`screen_outbound_messages`）统一处理，不单开 harness 改动。
7. **模型没有上下文余量表盘 / `new_context_window` 杠杆** —— 提案的读数源 `ContextBudget::last_pressure()` 在**默认安装下根本不存在**（`[context_budget]` 默认 false，`config/structs.rs:478`；默认走 `SessionCompactor`），且 `AlephToolDyn::call` 拿不到它。且模型已有 `recall_events` / `ctx_search` / `recall_context` 三个核心恢复工具，压缩摘要以可见 `<session_context>` 块注入。**不做**。
8. **压缩有效性用估算器判定（hermes「假节省」）** —— 不成立：`note_compaction_effect`（`context/budget/mod.rs:521-542`）的 before/after **同函数同校准系数**，delta 自洽；且 fingerprint cache（compactor.rs:306-325）使无进展场景不会每轮都付 LLM 调用。
9. **MutationEvidenceVerifier 对纯 Markdown 编辑误触发** —— 现象真实（只按工具名匹配，`mutation_evidence_verifier.rs:58-67`），但代价上限是**每会话一次**额外轮次（`nudged` 去重 :74-84），且 nudge 文案本身就写着「…or finish now if you are confident verification is unnecessary」（`nudges.rs:81-86`）。加扩展名表 = 硬编码领域判断（违 R9），且只会产生**假阴性**。**不做**（模块 doc :27-29 明令禁止「靠检查参数或输出内容来修」）。
10. **verifier 看不到工具是否成功（红树也算「验证证据」）** —— 提案违 R10 第 3 不（完成度判断）与 R7；且模型下一轮**本来就收到** `{"success":false,"exit_code":1,...}`（compressor 对 bash 是恒等，`tool_output/compressor.rs:309`）。**永久不做**（`src/verification/mod.rs:18-27` 已把 JudgeVerifier 列为永久禁止）。
11. **`tools.cancel_call` 无法取消子代理内部的工具调用** —— 前提是伪的：Panel 里**没有**这个按钮（`rg cancel_call interfaces/webchat/` 零命中），唯一消费者是 CLI，而 CLI 的 `calls list` 与 `cancel` 读同一个注册表。且子代理内的工具**已经**可取消（取消父的 subagent 调用即可，`subagent_tool/spawn.rs:49-64` 的 `cancel_for_child_with`，回归测试 `tests/cancellation_chain.rs`）。把子调用注册进扁平全局表反而会制造归属歧义与 id 碰撞。**不做**。
12. **把跨批失败 memo 下沉到 `src/tools/scoped/`** —— 会把它挂到**父 run 的 ScopedToolService** 上（子代理复用父的 `parent_tools`，`subagent_spawner/mod.rs:391-412`），导致父子 run 共享 memo；且拒绝点会挪到审批门**之后**（用户先看到审批卡再被拒）。**不做**。

---

## 六、建议的执行顺序

> 分批原则：**每个 batch 结束时 `cargo test -p alephcore --lib`（含 `budget.rs`）必须绿**。每个改 harness 行数的 item 都要在**同一 commit** 里改 `budget.rs::CEILING` + `src/harness/CLAUDE.md:17`。

### Batch 0 —— 零 harness 行的安全修复（**全部可并行**）
| # | 项 | 尺寸 | 依赖 |
|---|---|---|---|
| 1 | **B3** 溢出分类器扩宽（`llm_retry.rs`） | **S** | 无 |
| 2 | **B10** 续跑继承 project root | **S** | 无 |
| 3 | **B11** resume 重导 caller_role + channel perms + UNATTENDED | **M** | 无 |
| 4 | **D6** 子代理 tool_signal_sink 连线 | **S** | 无 |
| 5 | **B15** 子代理 max_iterations 兜底（用 `resolve_max_iterations`） | **S** | 无 |
| 6 | **B2** result_store / ContentIndex session-scoped 化 + `ctx_search` 走 `turn_context` | **M** | 无 |
| 7 | **B13** 压缩保留用户消息 + floor 角色感知（四个 drain 站点） | **M** | 无 |
| 8 | **B14** 窗口感知工具预算 | **M** | 无（但与 B4 同族，建议同一人做） |
| 9 | **B18 + D7** 后台子代理进度轨迹保留 | **S** | 无 |
| 10 | **B9** call_id nonce（gemini + **ollama** + delta.rs） | **S** | 无 |

> ⚠️ B3 与 B14 有耦合叙事（本地小模型溢出 → Fatal），但代码独立，可并行。

### Batch 1 —— harness 净负的清扫（**互不冲突，可并行；但 CEILING 需串行落**）
| # | 项 | 尺寸 | Δharness | 依赖 |
|---|---|---|---|---|
| 11 | **D3** 删 `on_init_seam`（含 `src/orchestrator/tests/mod.rs:7` 的 `mod init_audit;`） | **S** | −10 | 无 |
| 12 | **D4** 删 `on_tool_call` / `on_complete`（含 trait_def.rs:62 第 8 个发射点 + 4 处过时 doc） | **S** | −20 | 无 |
| 13 | **D5** 删 `Harness` trait（inherent `impl AgentHarness`；`chain.rs:155-186` 两个 trait 分派测试随之删） | **M** | −55 | 与 D1 无强依赖（inherent `chain_context()` 已 shadow trait 方法） |
| 14 | **B16** 合并两个 still-overflow 失败臂 | **S** | −8 | 无 |
| 15 | **B12** 合并 `fire_grace_turn` → `fire_boundary_grace_turn` | **S** | −25 | 无 |

**Batch 1 后：5997 − 118 = 5879。**

### Batch 2 —— 带 +行的 bug 修复（**必须在 Batch 1 之后，用已挣的余量支付**）
| # | 项 | 尺寸 | Δharness | 依赖 |
|---|---|---|---|---|
| 16 | **S4** 九条模型可见文案下沉 `nudges.rs` | **M** | **−88** | 无（⛔ **不要**依赖 prompt.rs 外迁 —— budget.rs:126-144 断言文件集精确，prompt.rs 必须先原地留下） |
| 17 | **B7** max_output_tokens partial 保留（护栏前拼接、循环内不 emit） | **M** | +8 | 16（用 nudge 外迁的余量支付） |
| 18 | **B8** `SystemMessage` prompt arm（走 deferral buffer） | **S** | +6 | 16 |
| 19 | **B17** 护栏改写后串行化并行批次 | **S** | +2 | 16 |
| 20 | **B1** 输入护栏下沉 `src/guardrails/registry.rs` + 历史消息非对称 Block 语义 | **L** | **−38** | 16（redaction 占位文案要落在 nudges.rs） |

**Batch 2 后：5879 − 88 + 16 − 38 = 5769。**

### Batch 3 —— Act 期时钟与工具预算的联合手术（**串行，同一 PR 族**）
| # | 项 | 尺寸 | Δharness | 依赖 |
|---|---|---|---|---|
| 21 | **B4** 所有 `describe()` 站点返回 `Some(max_duration_ms)`（含 `ask_user` 630s、`subagent`、mcp_scope_view） | **M** | 0 | 8（B14，同族） |
| 22 | **B5** Act 期 wall-clock 整体下沉到 `scoped/dispatch.rs`（审批门之后） | **L** | **−28** | 21 |
| 23 | **B6** timeout→`ToolError::Timeout` + ledger `if !e.is_retryable()` guard | **S** | −6 | 22 |
| 24 | **S7** Act 期 StalledTurn 分支退役 | **S** | −12 | 21, 22 |

**Batch 3 后：5769 − 46 = 5723。**

### Batch 4 —— 两个大下沉（**S1 与 S2 可并行**）
| # | 项 | 尺寸 | Δharness | 依赖 |
|---|---|---|---|---|
| 25 | **S1** trace DTO → `src/gateway/trace_protocol.rs` | **M** | **−221** | 无 |
| 26 | **S2** reactive-compaction 救援簇 → `src/context/compact/rescue.rs`（`RescueHost` / `RescueCx` / `RescueSlot`；**必须保留 `response_was_streamed: &mut bool` 在 port 里**，否则救援文本永远到不了 live delta 或双发） | **L** | **−367** | 14（B16 先合并失败臂，减少搬运面） |

**Batch 4 后：5723 − 588 = 5135。**

### Batch 5 —— 修宪（**需人拍板；两项可并行**）
| # | 项 | 尺寸 | Δharness | 依赖 |
|---|---|---|---|---|
| 27 | **D1/S3** `chain_context.rs` → `src/agents/`（12→11 文件） | **M** | **−125** | 13（D5 先删 trait 的 `chain_context()`） |
| 28 | **S11** `prompt.rs` → `src/thinker/turn_prompt.rs`（或 `src/session/`）（11→10 文件） | **L** | **−476** | 16, 18（先把文案与 SystemMessage arm 落定，再整体搬） |

**同 commit 必改四处**：root `CLAUDE.md:70`（R10 文件清单）、`src/harness/CLAUDE.md:8-9,17`、`HARNESS_PHILOSOPHY.md:139,141`（§4.1 树 + TOTAL）、`budget.rs::BUDGETED`（数组元数 12→10）+ `the_harness_is_still_exactly_the_twelve_files_r10_names` 的名字与断言 + `CEILING`。

**Batch 5 后：5135 − 601 = 4534 —— 低于 4900，余量 366 行。** ✅

### 若 Batch 5 不批（不修宪）
**终点 5135，超红线 235 行 —— 这是诚实的残口。** 唯一其他出路是 **E4（act 批执行引擎 → `src/tools/dispatch/`，−800…−950）**，但它需要独立 spec（`DispatchHost` host trait；必须保住 `dispatch_group` 内的第二个 steer checkpoint，act.rs:381；不得搬走 `stall_tracker` / `tool_timeline` —— 它们有 act 之外的读者：agent.rs:536、think.rs:944、runner_impl.rs:793），是 **L+ 级**，另立任务。

### 并行安全速查
- **Batch 0 全部 10 项互相并行安全**（零 harness 触碰，文件不重叠；仅 B3↔B14 在叙事上相关）。
- **Batch 1 的 11–15 并行安全**（不同函数/文件），但 `CEILING` 只能由最后合入者定稿 —— 建议一人收口，或每次合入后重跑 `budget.rs` 取实测值（**不要手算 CEILING**，budget.rs 存在的全部意义就是替代手算）。
- **Batch 2 的 17/18/19 并行安全**，但都必须排在 16 之后。20（B1）独立文件，可与 17–19 并行。
- **Batch 4 的 25/26 并行安全**（trace.rs vs think.rs）。
- **Batch 5 的 27/28 并行安全**，但两者都改 `budget.rs::BUDGETED` 与四份宪法文档 → **必须串行合入**。