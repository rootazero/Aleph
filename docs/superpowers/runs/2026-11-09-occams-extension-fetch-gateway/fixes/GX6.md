# Fix brief — GX6

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/GX6.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/GX6.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/GX6.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-GX6.md

## FIX THESE (17)

### GX6-01 [P0] MCP sessions are not bound to the authenticated principal
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/http.rs:208` (`session_from_header`, `handle_get`, `handle_delete`); `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/session.rs:52` (`SessionView`, `SessionTable::create`); `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/protocol.rs:98` (`dispatch`, `initialize`)
- **Failure mode**: 用户 A 初始化 session 后，持有另一份有效凭据的用户 B 一旦知道 A 的 session ID，就能对它发 GET 接管通知连接，或发 DELETE 删除它。HTTP admission 只证明 B 有权进入 MCP face，不证明这个 session 属于 B；POST 同样只按 ID 取 session。`initialize` 没有把已认证 caller 传给 session 创建，表内也没有 owner，因此这不是补一个 HTTP 判断就能成立的隔离。前提是 B 获得已有 ID，不声称 UUID 可猜测或无凭据访问可行。
- **Evidence**: `return initialize(face, id, msg.params);`；`state.face.sessions().touch(id).ok_or_else(|| {`；`if state.face.sessions().remove(id) {`
- **Suggested fix**: 创建时保存 admission 已解析的稳定 principal identity；POST/GET/DELETE 在 touch、替换 notifier 或删除之前比较同一 identity。不保存原始 bearer，不另造鉴权来源，保留现有 origin、TLS、role、expiry 和 operator gate。
- **Confidence**: 0.99

### GX6-02 [P0] StreamingTarget Debug exposes the provider API key
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/voice/streaming/mod.rs:83` (`StreamingTarget`)
- **Failure mode**: 对包含真实凭据的公开 `StreamingTarget` 执行正常的 Debug 格式化，例如 `format!("{target:?}")` → 派生实现逐字段输出原始 `api_key`。这直接违反本批明确的“secret 不得出现在 Debug impl”契约；确认的是可调用的 Debug 输出泄密，不声称已观察到生产日志泄漏。
- **Evidence**: `#[derive(Debug, Clone)]`；`pub struct StreamingTarget {`；`pub api_key: String,`
- **Suggested fix**: 去掉不需要的 Debug 派生；若调用方确需 Debug，仅手写隐藏 key 的实现。保留 Clone、目标配置和鉴权传递语义，不增加第二份 secret 存储。
- **Confidence**: 1.00

### GX6-03 [P1] BTW promotion can pair a side question with a newly seeded main answer
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/btw/promote.rs:81` (`latest_complete_exchange`, `exchange_in`)
- **Failure mode**: side log 已有 `TurnStarted → side question → side answer → RunFinished(Completed)`；主会话随后产生新问答，下一次 warm seed 把这些 prompt-bearing events 追加到 side log，但不复制主会话的 turn/run 边界。此时 promote 的旧 side span 一直延伸到“下一次 TurnStarted”或日志末尾，反向找最后一条 assistant 就选中新复制的 main answer → 返回旧 side question + main answer，而非真实的完整 side exchange。下一 side turn 尚未完成时，回退选旧 completed turn 也会触发。已在本批 `/home/zou/data/workspace/Aleph/src/gateway/btw/seed/mod.rs:203` 确认 warm top-up 的追加路径。
- **Evidence**: `let end = opens.get(nth + 1).copied().unwrap_or(records.len());`；`if !completed_here(span) {`；`let answer = span.iter().rev().find_map(|r| match &r.event {`
- **Suggested fix**: 在匹配的 `RunFinished` 处关闭候选 turn，并仅在该已关闭范围内找 question/answer；如采用 turn_id，必须排除复制来的不同 turn。保留首个非 synthetic user、最终 assistant、失败 run 不可 promote 的规则，不修改正确的 warm seed 行为。
- **Confidence**: 0.98

### GX6-04 [P1] Responses tools are parsed using the incompatible Chat Completions shape
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/mod.rs:90` (`handle`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/passthrough.rs:25` (`convert_openai_tools`, `convert_tool_choice`)
- **Failure mode**: 标准 Responses 请求传 `tools:[{"type":"function","name":"lookup","parameters":{"type":"object"}}]` 和 `tool_choice:{"type":"function","name":"lookup"}` → 复用的 Chat converter 只接受嵌套 `function`，前者被 filter_map 静默删掉、后者变成 None，provider 得不到工具或指定工具的约束。不是 provider 不支持工具，而是边界把合法请求翻译丢了。
- **Evidence**: `let tool_defs = req.tools.as_ref().map(|t| convert_openai_tools(t));`；`let tool_choice = req.tool_choice.as_ref().and_then(convert_tool_choice);`；`let func = t.get("function")?;`
- **Suggested fix**: 在 Responses 边界把 flattened definition/named choice 转成现有内部类型；保留 Chat 的 nested schema，不为两种外部协议共用错误的 wire parser。不能表达的合法选项应显式拒绝，不能静默降级为无工具。
- **Confidence**: 1.00

### GX6-05 [P1] Responses cannot round-trip function-call items
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/types.rs:253` (`ResponsesInput`, `ResponsesMessage`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/mod.rs:147` (`handle`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/sse.rs:278` (`completed_frame`)
- **Failure mode**: 客户端把工具结果以标准 `input:[{"type":"function_call_output","call_id":"c1","output":"ok"}]` 发回 → 所有列表元素都被强制反序列化成带必填 role 的 `ResponsesMessage`，在入口返回 400。反方向生成的 function_call output item 又只含 `id`，缺少工具结果关联必需的 `call_id`。因此即便绕开 tools definition 缺陷，标准 Responses tool loop 仍无法完成。
- **Evidence**: `Messages(Vec<ResponsesMessage>),`；`pub role: String,`；`"id": id,`（function_call 输出中没有 `call_id`）
- **Suggested fix**: 用实际支持的 Responses input-item 变体表达 message、function_call、function_call_output，映射到现有 UnifiedMessage tool-call/result；输出同时保留 item identity 与 call_id。流式和非流式共用同一关联规则，不能把 message-only schema 冒充整个 input union。
- **Confidence**: 1.00

### GX6-06 [P1] Responses SSE frames do not satisfy the Responses event schema
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/sse.rs:71` (`provider_deltas_to_responses_sse`, `completed_frame`)
- **Failure mode**: 标准 Responses SDK 消费 stream → `response.created`/`response.completed` 的 data 是裸 response，而不是包含 `type`、`sequence_number`、`response` 的事件；文本 delta 又缺少 item/output/content 定位字段，且没有对应 message/content-part 的 added/done 生命周期。仅 SSE 的 `event:` 名称正确不足以让 SDK 按类型构造并累积 response，常规 `stream.get_final_response()` 一类消费者无法重建完整输出。
- **Evidence**: `let created_event = sse_event(`；`"object": "response",`；`"status": "completed",`（created/completed 都把 response 字段直接放在事件根部）
- **Suggested fix**: 按 Responses wire contract 包装 lifecycle events，给每帧统一递增 sequence_number，并发送带一致 item_id/output_index/content_index 的必要 item/content 生命周期。保留真实 delta 顺序与工具关联，不只补一个根部 type 来让单帧看似合法。
- **Confidence**: 0.99

### GX6-07 [P1] Responses streaming reports token-limit truncation as completed
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/sse.rs:231` (`provider_deltas_to_responses_sse`, `completed_frame`)
- **Failure mode**: 请求设置较小 max_output_tokens，provider 正常返回 `Done(MaxTokens)` → 分支明确丢弃 stop reason，发送 `response.completed` 和 `status:"completed"`，没有 `incomplete_details`。客户端会把被 token 上限截断的答案或工具参数当作完整成功结果，无法按 Responses 语义处理 incomplete。
- **Evidence**: `ProviderDelta::Done(_reason) => {`；`let frame = completed_frame(&state);`；`"status": "completed",`
- **Suggested fix**: 在终态映射中保留 provider stop reason；token-limit 结束应输出 `response.incomplete`、incomplete status 和 max_output_tokens reason，正常完成仍为 completed。不要改变 provider 的生成或重试策略。
- **Confidence**: 0.99

### GX6-08 [P1] Chat translation silently drops developer and subsequent system instructions
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/passthrough.rs:129` (`convert_messages`, `extract_system_prompt`)
- **Failure mode**: 合法 Chat 请求包含两条 system message，或使用 developer role → converter 丢弃所有非 user/assistant/tool 消息，而另一路只取第一条 system。后续 system 和全部 developer instructions 从实际 provider payload 消失，模型在缺少客户端指令的情况下运行，HTTP 却正常报成功。
- **Evidence**: `_ => None,`；`.find(|m| m.role == "system")`；`.and_then(|m| m.content.clone())`
- **Suggested fix**: 通过 provider 的既有 system/context 契约保留这些 instructions 的顺序和内容；不能保持语义的 role 应在入口明确拒绝而非静默删除。保留 tool-call/result 关联，不新增 Gateway 侧推理策略。
- **Confidence**: 0.99

### GX6-09 [P1] Accepted stop sequences never reach the provider
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/passthrough.rs:166` (`handle`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/types.rs:19` (`ChatCompletionRequest`)
- **Failure mode**: 客户端发送已被请求 schema 接受的 `stop:["END"]` → payload 仅转发 temperature/max_tokens/tool_choice 等字段，完全不读取 stop；模型继续生成并向客户端返回停止符及其后的内容。这是静默改变生成边界，不是客户端显式选择不支持的参数。
- **Evidence**: `pub stop: Option<Vec<String>>,`；`temperature: req.temperature.map(|t| t as f32),`；`max_tokens: req.max_tokens,`
- **Suggested fix**: 将 stop 传入拥有该功能的 provider 契约；若当前内部契约不支持，最小诚实修复是在边界拒绝非空 stop，不能接受后忽略。不要用 Gateway 文本截断伪装成上游生成停止。
- **Confidence**: 0.99

### GX6-10 [P1] Outward usage omits cached prompt tokens
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/stream.rs:265` (`provider_deltas_to_sse`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/passthrough.rs:210` (`handle`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/mod.rs:156` (`handle`); `/home/zou/data/workspace/Aleph/src/gateway/openai_api/responses/sse.rs:222` (`provider_deltas_to_responses_sse`)
- **Failure mode**: 上游 OpenAI usage 为 prompt=100、cached=80、completion=10 → 内部协议按计费契约拆成 input=20、cache_read=80、output=10；四个出口直接把 internal input 当作 OpenAI prompt/input，返回 prompt=20、total=30，而不是 100/110。流式与非流式都会向客户端低报实际上下文与 token 使用量。为确认内部单位，额外只读了跨批契约 `/home/zou/data/workspace/Aleph/src/providers/protocols/openai_chat/sse.rs:107`，其注释和减法明确说明 internal input 与 cache_read 是互斥计数。
- **Evidence**: `prompt_tokens: u.input_tokens,`；`input_tokens: u.input_tokens,`；`total_tokens: u.input_tokens + u.output_tokens,`
- **Suggested fix**: 在对外翻译时按内部 usage 契约把互斥的输入/cache buckets 重组为 OpenAI 总输入，并由同一总输入派生 total；四个出口保持一致。保留内部互斥计费单位，不回改 provider 来制造缓存双计费。
- **Confidence**: 0.99

### GX6-11 [P2] Disconnecting an agent SSE response does not cancel execution
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/agent.rs:429` (`handle_streaming`)
- **Failure mode**: 客户端在 agent 执行长工具或等待 provider 时断开 HTTP stream → body 的 rx 被丢弃，但后台 execute 的 JoinHandle 也已丢弃，没有任何 disconnect-to-cancel 连线，run 仍可继续工具副作用、占用 session 和生成预算直到正常结束或 deadline。返回 ChannelClosed 不是执行取消机制，尤其工具未产生下一帧期间没有事件可触发它。额外只读确认了 `/home/zou/data/workspace/Aleph/src/gateway/execution_adapter.rs:64` 的 cancel API、`/home/zou/data/workspace/Aleph/src/gateway/execution_engine/adapter.rs:32` 的实际委派，以及 `/home/zou/data/workspace/Aleph/src/gateway/execution_engine/execute.rs:569` 的 emitter 错误只警告和 `/home/zou/data/workspace/Aleph/src/gateway/execution_engine/execute.rs:883` 的真实取消 select。
- **Evidence**: `tokio::spawn(async move {`；`if let Err(e) = adapter.execute(run_request, agent, emitter.clone()).await {`；`while let Some(frame) = rx.recv().await {`
- **Suggested fix**: 让响应流的关闭通知触发已存在的 execution cancellation，并保留可等待的完成路径；处理 admission 与断开之间的时序，确保取消的是本请求的 run。不要仅 abort 执行 task，绕过引擎持久化和清理语义。
- **Confidence**: 0.98

### GX6-12 [P1] Agent history seeding ignores durable append failures
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/agent.rs:344` (`handle`)
- **Failure mode**: SessionService 存在但客户端历史中的一条 emit_event 返回错误 → handler 丢弃错误，继续追加其他历史并执行当前用户输入。一次失败可能丢掉 system 或 assistant 上下文，模型据残缺历史作出决定，而客户端不会收到历史未完整写入的错误。这里处理了“service 不存在”，却没有处理同一写入边界的 Err。
- **Evidence**: `if let Some(svc) = crate::session::service::global_session_service() {`；`let _ = svc.emit_event(&session_key, event).await;`
- **Suggested fix**: 将 append 错误带上下文传播为 ApiError，在历史未完整接受时不开始 run；若要保证批次原子性，使用已有 service batch 契约而不是在 Gateway 另写状态事务。保留 SessionService 为唯一 durable writer，不用旧存储 fallback 掩盖失败。
- **Confidence**: 0.97

### GX6-13 [P1] Agent streaming violates the advertised include_usage contract
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/agent.rs:128` (`SseEventEmitter::emit`, `handle_streaming`)
- **Failure mode**: agent run 的 summary.total_tokens 非零 → 无论客户端是否设置 `stream_options.include_usage` 都在有 choices 的 finish chunk 中发 usage；客户端显式请求 include_usage 时也收不到标准的独立 empty-choices usage chunk。此外 prompt/completion 被硬编码为 0 而 total 非零，给出自相矛盾的 token breakdown。与本批 passthrough 路径已实现的 opt-in 合约不一致。
- **Evidence**: `usage: if summary.total_tokens > 0 {`；`prompt_tokens: 0,`；`completion_tokens: 0,`
- **Suggested fix**: 将 include_usage 传给 emitter，默认 usage 为 null；请求时在 finish 后、[DONE] 前发独立 usage chunk。计数来自已有权威 usage 数据，不能用零值伪造未知 breakdown；保留 summary text 不作为 assistant 正文发送的现有规则。
- **Confidence**: 0.99

### GX6-14 [P1] Local speech synthesis labels server-default audio as Ogg
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/voice/local_provider.rs:210` (`LocalVoiceProvider::tts`)
- **Failure mode**: BYO OpenAI-compatible speech server 未配置 format，按契约自行选择默认 MP3 → 请求正确省略 response_format，但返回值根据空 format 落到 audio/ogg。显式 FLAC/AAC/PCM 也全部被标 Ogg。`/home/zou/data/workspace/Aleph/src/gateway/voice/outbound.rs:223` 会把这个 metadata 原样写入附件，后续按 MIME 的上传/解码收到错误类型，而不是服务器真实输出格式。
- **Evidence**: `if !format.is_empty() {`；`let content_type = match format.as_str() {`；`_ => "audio/ogg",`
- **Suggested fix**: 读取成功响应的实际 Content-Type，再按明确配置的格式作必要 fallback；不要把未指定格式猜成 Ogg。保留“未配置字段由 BYO server 默认值决定”这一契约，不擅自强制 response_format。
- **Confidence**: 0.99

### GX6-15 [P1] Replies with nothing speakable count as failed synthesis and disable voice
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/voice/outbound.rs:168` (`generate_tts`, `generate_tts_outcome`, `TtsOutcome`)
- **Failure mode**: 连续三条回答都是 code block 或纯链接，sanitize 后为空 → generate_tts 明确“skipping”且没有调用 provider，却返回与真实失败相同的 None；outcome wrapper 将其全部判 Failed，reply emitter 记录三次 strike，关闭健康的 voice mode。为确认效果到达，额外只读了跨批消费端 `/home/zou/data/workspace/Aleph/src/gateway/reply_emitter/emitter/helpers.rs:152`，其 Failed 分支实际更新 registry 的失败计数并触发 auto-disable。
- **Evidence**: `debug!("TTS: reply has no speakable content after sanitization, skipping");`；`return None;`；`None => TtsOutcome::Failed,`
- **Suggested fix**: 将“没有可朗读内容、未尝试生成”与真正失败区分，例如 crate-internal Skipped outcome，并同步唯一 emitter match；跳过时回文本但不加 strike。保留真实 provider 失败三次关闭、成功清零的策略。
- **Confidence**: 0.99

### GX6-16 [P1] Telegram formatting interprets markup inside inline code
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/formatter/markdown_to_platform.rs:84` (`telegram_inline`)
- **Failure mode**: 常见 Markdown 输入 `` `**x**` `` → converter 先处理星号再处理 backtick，得到 `<code><b>x</b></code>`，而不是保留字面量 `**x**`。Telegram 的 code/pre 不能与 bold 等实体嵌套，可能直接拒绝整个 parse_mode=HTML 消息；即使发送端退回纯文本，原本合法代码内容也已被错误解释。后续 repair_html_tags 仅修复标签嵌套，不恢复 code 字面量。
- **Evidence**: `s = replace_paired_marker(&s, "**", "<b>", "</b>");`；`s = replace_single_asterisk_italic(&s, "<i>", "</i>");`；`s = replace_paired_marker(&s, "`", "<code>", "</code>");`
- **Suggested fix**: 在 inline 扫描中把 code span 当作不再解析其它 markup 的字面区域，复用现有 HTML escaping；不能只交换两行顺序，因为后续 marker pass 仍会进入已输出的 code 内容。保留普通文本的 bold/italic/link 和 block parser 行为。
- **Confidence**: 0.98

### GX6-17 [P2] MCP cancellation notifications never reach running tools
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/protocol.rs:87` (`dispatch`); `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/mod.rs:326` (`McpFace::call_tool`)
- **Failure mode**: 客户端启动耗时 tools/call，随后按 MCP 发送 `notifications/cancelled` 携带该 requestId → dispatcher 对除 initialized 以外的所有 notification 立即 Accepted；执行端创建的 CancellationToken 没有保留或和 requestId 关联。请求即使仍在审批/长工具等待，也只能继续运行至自己的结束/超时，客户端取消不释放资源。JSON-RPC notification 不应回应，不代表不应执行其取消语义。
- **Evidence**: `if msg.id.is_none() {`；`if msg.method == "notifications/initialized" {`；`.execute_with_cancel(name, arguments, CancellationToken::new())`
- **Suggested fix**: 在 session/request 生命周期内登记已有 tool cancellation token，以 session + requestId 查找并响应 cancelled notification，完成后移除；保持 notification 无响应以及现有调用鉴权、审批和工具执行入口，不用 abort 跳过工具清理。
- **Confidence**: 0.99

## Clean files

以下文件已全文审阅，未定位到满足本轮具体触发条件要求的独立缺陷；作为其它 finding 的 producer/consumer 证据，不表示该文件本身有缺陷。All files below were reviewed in full; no independent finding is assigned to them.

- `/home/zou/data/workspace/Aleph/src/gateway/agent_env/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/btw/guard_tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/btw/tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/formatter/helpers.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/formatter/tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/agent_env/manager_ops.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/agent_env/ops.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/streaming/whisperlive.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/btw/seed/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/btw/promote/tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/streaming/relay.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/btw/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/auth.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/sanitize.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/streaming/deepgram.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/voice_mode.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/auth.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/mcp_face/config.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/inbound/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/inbound/provider.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/state.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/format.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/inbound/stt.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/formatter/splitting.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/btw/seed/gate.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/formatter/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/embeddings.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/models.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/formatter/platform_to_markdown.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/completions/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/router.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/state.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/openai_api/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/voice/hallucination.rs`

