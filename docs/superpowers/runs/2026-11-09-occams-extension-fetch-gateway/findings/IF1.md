# Findings — IF1 (Telegram channel adapter)

Files reviewed: 25 (8232 LOC)
Counts: P0= 2 P1= 12 P2= 6 P3= 0

Static review only / 仅静态审查。已全文阅读批次全部文件；跨模块检查仅用于确认生产调用、授权、去重和第三方 I/O 契约，具体位置随 finding 标明。Graphify 仅作定位，不作证据。没有修改源码、运行编译/测试或改动 git。未证实阻塞文件 I/O、可实际并发触发的锁死或无界队列，因此不把这些猜测列为发现。

### IF1-01 [P0] Multi-account replies select the first bot, not the originating account

- **Location**: `src/gateway/interfaces/telegram/mod.rs:809-822` (`TelegramChannel::send`), `src/gateway/interfaces/telegram/config_resolver.rs:58-67` (`ConfigResolver::resolve`), `src/gateway/interfaces/telegram/mod.rs:1020-1054` (`approval_capability`)
- **Failure mode**: 配置 A/B 两个 bot，目标群只在 B 下显式配置 → `resolve(A, chat, thread)` 仍命中 A 的账户默认值，`find` 提前选择 A。A 不在群内时交付失败；A/B 都在群内时由错误 bot 身份发出回复。审批 capability 更直接地只保留首个 bot，修复普通 `send` 后它仍无法给 B 的会话正确交付。跨模块确认 `src/gateway/inbound_router/executor.rs:690-705` 的流式路径也独立选择首账户。
- **Evidence**: `self.config_resolver.resolve(&inst.account_id, chat_id_i64, thread_id).is_some()`; `.or_else(|| self.lookup.get(&(account_id.to_string(), 0, None)))`; `let first = self.bot_instances.first()?;`
- **Suggested fix**: 保留入站的 account identity 并在回复/审批/流式交付统一使用它；已有显式 chat/topic 映射必须优先于默认回退。无唯一账户时返回配置错误，不猜首账户。保持单账户回退、现有 Telegram wire ID 与审批授权语义。
- **Confidence**: 0.99

### IF1-02 [P0] Normal polling never advances the durable tracker, so restart drops offline backlog

- **Location**: `src/gateway/interfaces/telegram/polling.rs:80-98,128-130,230-232` (`run_polling_loop`)
- **Failure mode**: 新账户启动时诊断请求没有更新，tracker 保持 0；此后 dispatcher 正常接收许多消息，但没有任何正常处理路径调用 `advance`。停止后积压的新消息在下次启动仍被当作首次启动，通过 `drop_pending_updates(true)` 永久删除。跨模块确认 teloxide 0.17.0 的 `dispatching/dispatcher.rs:358-367` 使用默认 polling listener，它只维护自己的内存 offset，并不写入此 tracker。诊断请求返回一条更新本身不等于已确认该更新，不能据此声称诊断直接消费了消息。
- **Evidence**: `let persisted_offset = offset_tracker.as_ref().map_or(0, |t| t.load());`; `bot.delete_webhook().drop_pending_updates(true).await`; `tracker.advance(i64::from(last.id.0), "boot").await;`
- **Suggested fix**: 将真正的 polling/处理进度接入每个 bot 自己的持久化 tracker，并用明确的一次性初始化事实区分首次启动与重启；不要让“offset 仍为 0”授权删除积压。保持更新确认顺序，不能在处理前把探测结果当作 durable completion。
- **Confidence**: 0.99

### IF1-03 [P1] Inbound IDs collide across chats, voters and reaction changes

- **Location**: `src/gateway/interfaces/telegram/handlers.rs:161` (`convert_message`), `src/gateway/interfaces/telegram/mod.rs:683-684` (`poll_handler`), `src/gateway/interfaces/telegram/reaction_handler.rs:28-32` (`convert_reaction`)
- **Failure mode**: 两个群在五分钟内分别收到 Telegram message ID 相同的消息 → 转换后 ID 相同；同一 poll 的第二位投票者、同一消息的另一位 reaction 用户/反应变更也复用 ID。跨模块确认 `src/gateway/inbound_router/mod.rs:612-615` 仅以 `channel_id:message_id` 去重，`src/gateway/inbound_router/dedup.rs:8,31-36` 在 300 秒内拒绝重复，所以真实的新事件被静默当作重放丢弃。普通群的 message ID 只有 chat-local 唯一性。
- **Evidence**: `id: MessageId::new(msg.id.0.to_string()),`; `id: MessageId::new(format!("poll_{}", q.poll_id)),`; `"react_{}_{}", update.chat.id.0, update.message_id.0`
- **Suggested fix**: 将 adapter 的事件去重 identity 与 provider message/reply identity 分开；用 account/chat/update identity 保证事件唯一，继续保留原始 Telegram message ID 给回复、reaction、编辑使用。不能直接更改 ID 字符串而不同时更新依赖其为纯数字的消费端。
- **Confidence**: 0.99

### IF1-04 [P1] Empty-account configuration panics before startup can return its error

- **Location**: `src/gateway/interfaces/telegram/mod.rs:113-119` (`TelegramChannel::new`), `src/gateway/interfaces/telegram/access.rs:59-63` (`AccessController::new`)
- **Failure mode**: 传入 `accounts: []` → 构造器创建不存在的 `default` 账户 controller，resolver 没有账户条目，`expect` panic；`start()` 预期的 `No Telegram accounts configured` 返回路径无法到达。`parse_telegram_channel_config` 对 validate 结果只警告，仍可返回该配置。
- **Evidence**: `if config_v2.accounts.is_empty() {`; `Arc::new(AccessController::new(resolver.clone(), "default"))`; `resolver.resolve(account_id, 0, None).cloned().expect(`
- **Suggested fix**: 空配置不要创建虚假 controller，让现有 `start` 配置错误正常返回，或在配置拥有边界拒绝空账户；保持有效账户 controller 的内部不变量，不在所有内部访问点重复验证。
- **Confidence**: 0.99

### IF1-05 [P1] Outbound sends discard the resolved chat/topic overrides

- **Location**: `src/gateway/interfaces/telegram/mod.rs:232-247,809-829` (`TelegramChannel::start`, `TelegramChannel::send`)
- **Failure mode**: 单账户下给群/topic 设置独立 `send_typing`、retry/error policy → `send` 虽调用针对 chat/topic 的 resolver，却只检查 `is_some()`，实际传给 delivery 的仍是启动时以 `(0, None)` 缓存的账户默认配置，因此 override 不生效。此缺陷与多账户选错 bot 独立存在。
- **Evidence**: `.resolve(&account.id, 0, None)`; `let resolved_config = instance.resolved_config.clone();`; `delivery::send_message(&bot, &resolved_config, &message, &error_cooldown).await`
- **Suggested fix**: 账户选定后直接使用此次 `(account, chat, thread)` 查询得到的配置交付，不重新引入配置副本；保持 topic > group > account 优先级和已有错误返回类型。
- **Confidence**: 0.99

### IF1-06 [P1] Adapter policy and Core policy disagree on allowed group overrides

- **Location**: `src/gateway/interfaces/telegram/access.rs:72-116` (`AccessController::check_message`), `src/gateway/interfaces/telegram/config_resolver.rs:92-106` (`merge_group`), `src/gateway/interfaces/telegram/mod.rs:533-555` (message handler)
- **Failure mode**: 账户 group policy 为 Disabled，但明确给某群覆写 Open → 本地 controller 按群级 resolver 放行并入队；跨模块确认 `src/gateway/inbound_router/types.rs:419-425,444-453` 仅从首账户构造 Core policy，`src/gateway/inbound_router/permission.rs:79-83` 再以 Disabled 拒绝，因此配置允许的群仍不能工作。这里不是声称所有 poll/reaction 绕过 Core 授权：Core 确实会重新检查。R4 的具体问题是两份 policy reduction 已产生不同判决。
- **Evidence**: `.unwrap_or_else(|| base.group_policy.clone()),`; `match access.check_message(user_id, chat_id, thread_id_i32, is_group) {`
- **Suggested fix**: 由 Core 的授权边界按完整 account/chat/topic 解析同一有效配置，adapter 仅携带必要 I/O 上下文；保留 Core 为授权/配对最终权威，不能删除第二道检查来“修复”可用性。
- **Confidence**: 0.97

### IF1-07 [P1] Attachment-only delivery reports success even when every upload failed

- **Location**: `src/gateway/interfaces/telegram/delivery.rs:359-375` (`send_message`)
- **Failure mode**: 空文本、非空附件，Telegram 拒绝所有上传 → 错误只写日志，函数仍返回成功和伪造 message ID `0`，调用者无法知道用户完全没有收到输出。
- **Evidence**: `tracing::warn!("Failed to send voice attachment: {}", e);`; `first_msg_id = Some("0".to_string());`; `return Ok(SendResult {`
- **Suggested fix**: 全失败必须传播交付错误；成功结果使用实际 provider message ID，而不是占位值。保留部分交付的可观测事实，避免在传播错误时无条件重发已经成功的附件。
- **Confidence**: 0.99

### IF1-08 [P1] Streaming answers exceeding Telegram's limit are silently truncated

- **Location**: `src/gateway/interfaces/telegram/delivery.rs:292-299,1094-1135` (`is_benign_edit_error`, `TelegramDelivery::send_text_message`, `edit_text_message`)
- **Failure mode**: 已存在流式 preview，累计/最终答案超过 Telegram 4096 字符限制 → edit 返回 `EditedMessageIsTooLong` 却被转成 `Ok(())`，lane 认为最终答案已落地；用户永远只看到最后一次成功的短 preview。没有 preview 时一次性长 send 也不拆分并失败。普通 `send_message` 已有分片，但本路径绕开它。
- **Evidence**: `| teloxide::ApiError::EditedMessageIsTooLong`; `.edit_message_text(chat_id, msg_id, &html_text)`; `Err(ref e) if is_benign_edit_error(e) => { ... Ok(()) }`
- **Suggested fix**: 不把长度错误当作成功；在流式 delivery 层复用有界 HTML-aware 分片，将超出 preview 的内容按顺序发送并在 finalization 确认全部落地。保留真正的 MessageNotModified 幂等语义。
- **Confidence**: 0.99

### IF1-09 [P1] Orchestrator final delivery failure never reaches the event producer

- **Location**: `src/gateway/interfaces/telegram/streaming/orchestrator.rs:163-184` (`StreamOrchestrator::run`), `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:90,102-117` (`new`, `emit`)
- **Failure mode**: 最终 Telegram send/edit 因 Forbidden、断网或格式错误失败 → orchestrator 只警告然后退出，spawn 的结果也无人接收；`emit(RunComplete)` 只确认事件入队，正常返回成功。与长度特例无关，任意最终交付失败都不能被调用者观测，用户无回复也没有可采取行动的交付失败结果。
- **Evidence**: `tracing::warn!("Failed to finalize answer lane: {}", e);`; `let _ = answer_lane.finalize(&error_msg).await;`; `tokio::spawn(orchestrator.run(inbound));`
- **Suggested fix**: 保留可等待的终结结果或 completion acknowledgement，最终交付失败向 producer 返回 delivery error；维持已发生外部副作用的事实，不将失败简单改成自动全量重试。
- **Confidence**: 0.99

### IF1-10 [P1] RunError skips the only pending-media drain

- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:102-115` (`TelegramEventEmitter::emit`)
- **Failure mode**: 工具已产生图片/文件并进入 pending media，随后 run 失败并发出 RunError → 仅 RunComplete 才调用 media leg；RunError 在 orchestrator 中终止 run，已生成媒体不交付。跨模块确认 `src/gateway/reply_emitter/emitter/streaming.rs:608-637` 的通用路径在 RunError 仍 drain/send media，本路径未继承该行为。
- **Evidence**: `let run_ended = matches!(event, StreamEvent::RunComplete { .. });`; `if run_ended { self.media.deliver_run_media().await; }`
- **Suggested fix**: RunComplete 与 RunError 都执行一次终结媒体交付，且不能重复转发 text；保持 pending-media 单次消费与通用 ReplyEmitter 的错误终结契约。
- **Confidence**: 0.99

### IF1-11 [P1] Enabling orchestrated streaming bypasses a valid configured proxy

- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:36-44` (`TelegramEventEmitter::new`); production caller `src/gateway/inbound_router/executor.rs:690-705` (`try_create_telegram_emitter`)
- **Failure mode**: 账户使用有效 proxy，普通账户 bot 能通过代理工作；开启 reasoning/status features 后，跨模块生产 caller 用 `Bot::new` 另造无代理 bot 并传给 emitter → 所有流式 text/reaction 走直连，在只能经代理访问 Telegram 的部署中整个回复失败。不是把明确写出的“无效 proxy fallback”推测成安全漏洞。
- **Evidence**: `bot: teloxide::Bot,` (emitter constructor); `let bot = teloxide::Bot::new(&account.bot_token);` (cross-module caller)
- **Suggested fix**: 复用账户已配置的 Bot/transport，而不是另造客户端；保留账户 proxy、token、连接配置的唯一来源，不新增另一套代理解析。
- **Confidence**: 0.99

### IF1-12 [P1] Zero streaming buffer survives validation and panics on a real run

- **Location**: `src/gateway/interfaces/telegram/config.rs:252-260` (`parse_telegram_channel_config`), `src/gateway/interfaces/telegram/config_v2.rs:317-321` (`validate`), `src/gateway/interfaces/telegram/streaming/orchestrator.rs:41-46` (`StreamOrchestrator::new`)
- **Failure mode**: `streaming.buffer_size = 0` 且 reasoning lane 开启 → validate 发现问题但 parser 只 warning 后返回 Ok；生产构造 orchestrator 时 `mpsc::channel(0)` panic，而非返回配置错误。
- **Evidence**: `tracing::warn!("Telegram config validation: {}", err);`; `Ok(config)`; `let (event_tx, event_rx) = mpsc::channel(config.buffer_size);`
- **Suggested fix**: 在配置拥有边界将该硬约束校验失败转换为现有配置错误；保持默认 buffer 和正常 bounded-channel backpressure，不在每次内部使用时重复修正无效值。
- **Confidence**: 0.99

### IF1-13 [P1] Explicit streaming disable settings do not disable the orchestrator

- **Location**: `src/gateway/interfaces/telegram/config_resolver.rs:92-150` (`merge_group`, `merge_topic`), `src/gateway/interfaces/telegram/config_v2.rs:96-102,200-218` (streaming configuration)
- **Failure mode**: 配置 `mode: Disabled` 或 `enabled: false`，但保留 reasoning/status 开关 → 跨模块检查 `src/gateway/inbound_router/executor.rs:691-702` 只检查 feature 开关并选择 orchestrator；lane 持续发送 preview/edit。群/topic 的 `block_streaming: true` 也在两个 merge 中被丢弃，无法阻止同一行为。
- **Evidence**: `pub mode: StreamingMode,`; `pub block_streaming: Option<bool>,`; `streaming: base.streaming.clone(),`
- **Suggested fix**: 在统一的有效配置解析中合并 group/topic block decision，emitter selection 与 preview 写入共同遵守有效 disable 状态；Disabled 时保留最终非流式回复，不把“关闭 preview”误实现成关闭回复。
- **Confidence**: 0.99

### IF1-14 [P1] Streaming reconstruction turns disabled link previews back on

- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:43-62` (`TelegramEventEmitter::new`), `src/gateway/interfaces/telegram/delivery.rs:1103-1104,1126-1127` (streaming send/edit)
- **Failure mode**: 账户显式 `link_preview: Disabled`，启用 orchestrator → emitter 手工拼出的 ResolvedConfig 固定为 Enabled；streaming send/edit 据此向 Telegram 请求预览，配置的关闭决策被翻转。这里不以未实际消费的 retry/cooldown 字段推导额外故障。
- **Evidence**: `link_preview:` / `crate::gateway::interfaces::telegram::config_v2::LinkPreviewMode::Enabled,`; `request = request.link_preview_options(opts);`
- **Suggested fix**: 传入账户/chat/topic 的真实 ResolvedConfig，删除此处虚构的配置镜像；保留关闭预览和其他已解析交付选项的单一来源。
- **Confidence**: 0.99

### IF1-15 [P2] Status reactions always target message 1

- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:75-90` (`TelegramEventEmitter::new`), `src/gateway/interfaces/telegram/streaming/orchestrator.rs:95-104` (`run`)
- **Failure mode**: 实际触发消息 ID 不是 1 → 构造的伪入站消息仍用 ID 1，run 从中取 reaction target，导致反应落在旧消息上或被 Telegram 拒绝，当前消息没有处理状态。
- **Evidence**: `id: MessageId::new("1"),`; `let inbound_message_id: Option<i64> = initial_inbound.id.as_str().parse().ok();`; `self.status_controller.handle_event(&event, msg_id).await`
- **Suggested fix**: 从真实 inbound/reply route 显式携带 provider message ID，不能用占位值代替；保持 Telegram reaction 目标为原消息、text/media 交付不重复。
- **Confidence**: 0.99

### IF1-16 [P2] Enqueueing RunComplete does not order final text before media

- **Location**: `src/gateway/interfaces/telegram/streaming/telegram_event_emitter.rs:103-115` (`emit`)
- **Failure mode**: 发送 RunComplete 到有容量的 channel 后，orchestrator 尚未处理事件或仍在 final edit 网络请求中 → producer 立即发送媒体，附件可以先于最终答案到达。跨模块确认 `src/gateway/reply_emitter/emitter/helpers.rs:194-219,256-258` 的 drain 只是取走 pending vector，没有代码注释假设的 download 延迟可提供顺序保障。
- **Evidence**: `self.event_tx.send(event).await`; `// leaves the lane's finalize edit ahead of it in practice.`; `self.media.deliver_run_media().await;`
- **Suggested fix**: 等待 finalization acknowledgement 后再执行 media leg，或由同一终结 owner 顺序执行两者；保持 no-double-text 和单次媒体 drain。与交付错误传播共用 completion 信号即可，不需要新调度框架。
- **Confidence**: 0.99

### IF1-17 [P2] Mixed attachment batches reorder documents behind later images

- **Location**: `src/gateway/interfaces/telegram/delivery.rs:794-818` (`send_attachments`)
- **Failure mode**: 输入 `[image A, document B, image C]` → 遇到 B 时先 flush A，但只把 B 放到 singles；循环结束发送 C，最后才发送 B，输出变为 A/C/B。最简单 `[document, image]` 也会倒序，改变多模态回复的对应关系。
- **Evidence**: `flush_image_album(bot, chat_id, thread_id, &image_buf).await?;`; `singles.push(att);`; `for att in singles { send_attachment(bot, chat_id, thread_id, att).await?; }`
- **Suggested fix**: 遇非图片时 flush 前序 album 后立即发送该项，仅批量连续图片；保持 10 张 album 上限、原输入顺序及现有错误传播。
- **Confidence**: 0.99

### IF1-18 [P2] stop and restart leak health tasks and retain obsolete bot instances

- **Location**: `src/gateway/interfaces/telegram/mod.rs:745-798` (`TelegramChannel::start`, `stop`)
- **Failure mode**: 每次 start 都 spawn 不受取消控制的无限 health loop，并 push instance；stop 仅向 polling 发 shutdown，既不停止 health task 也不清空 instances。反复 start/stop 后 getMe 任务线性累积，send 仍优先搜索最早保留的旧实例。
- **Evidence**: `tokio::spawn(async move {` / `loop { interval.tick().await;`; `self.bot_instances.push(instance);`; `if let Some(shutdown_tx) = instance.shutdown_tx.take() { let _ = shutdown_tx.send(()); }`
- **Suggested fix**: 将 health task 的取消/结束纳入 BotInstance 生命周期，stop 后释放旧 instances，启动部分失败时执行相同清理；保持运行期间健康探测频率和 polling 取消语义。
- **Confidence**: 0.99

### IF1-19 [P2] Reasoning lane has no final flush

- **Location**: `src/gateway/interfaces/telegram/streaming/orchestrator.rs:127-130,139-177` (`StreamOrchestrator::run`)
- **Failure mode**: reasoning lane 已显式开启，最后 reasoning delta 少于首次 preview 阈值，或最后一段被 debounce 合并 → RunComplete 只 finalize Answer，随后 break，Reasoning lane 的待发内容永久留在 tracker。长 reasoning preview 的最后一段同样可能不显示。
- **Evidence**: `reasoning_lane.write_chunk(content).await`; `let outcome = match crate::gateway::reply_emitter::sanitize_final_response(raw)`; `break;` (RunComplete branch)
- **Suggested fix**: 在终结时对已启用且有内容的 reasoning lane flush/finalize 一次；保留用户 opt-in、reasoning marker 和最终答案 sanitizer，不把推理内容回退到 Answer。
- **Confidence**: 0.98

### IF1-20 [P2] Same-state optimization suppresses the configured thinking reaction

- **Location**: `src/gateway/interfaces/telegram/streaming/status_controller.rs:66-75,101-115` (`handle_event`, `derive_target`)
- **Failure mode**: 配置不同的 processing/thinking emoji，事件顺序 RunAccepted → Reasoning → 两者 target state 都是 Thinking，因此第二次提前返回，不比较新 emoji，thinking 配置不会显示，直到其他状态转换才可能改变。
- **Evidence**: `// Same state — nothing to render.` / `return Ok(());`; `(ReactionState::Thinking, self.config.processing.clone())`; `(ReactionState::Thinking, emoji)`
- **Suggested fix**: 以目标 state 与目标 visible emoji 是否均未变化决定 no-op，而不是只比较 state；保持重复事件不重复调用 Telegram 和现有 Done-clear 行为。
- **Confidence**: 0.99

## Clean files

以下文件全文检查后没有独立、经证实的 finding；“clean”不代表经过动态验证。

- `src/gateway/interfaces/telegram/error_cooldown.rs`
- `src/gateway/interfaces/telegram/streaming/reasoning_extractor.rs`
- `src/gateway/interfaces/telegram/chunking.rs`
- `src/gateway/interfaces/telegram/approval.rs`
- `src/gateway/interfaces/telegram/audit.rs`
- `src/gateway/interfaces/telegram/streaming/lane_handle.rs`
- `src/gateway/interfaces/telegram/mention.rs`
- `src/gateway/interfaces/telegram/token_fingerprint.rs`
- `src/gateway/interfaces/telegram/offset.rs`
- `src/gateway/interfaces/telegram/streaming/lane_tracker.rs`
- `src/gateway/interfaces/telegram/bot_instance.rs`
- `src/gateway/interfaces/telegram/sticker.rs`
- `src/gateway/interfaces/telegram/streaming/mod.rs`
