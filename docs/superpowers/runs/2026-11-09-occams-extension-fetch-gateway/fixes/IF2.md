# Fix brief — IF2

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/IF2.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/IF2.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/IF2.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-IF2.md

## FIX THESE (12)

### IF2-01 [P0] Derived Debug exposes the Discord bot token / Debug 泄露机器人令牌
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/config.rs:12` (`DiscordConfig`)
- **Failure mode**: 对含真实令牌的配置执行 `format!("{config:?}")` 或 tracing 的 debug-field 格式化 → 派生实现直接输出完整 `bot_token`。这直接违反本批次明确规定的“secret 不得进入 Debug”契约；不假定当前存在某条生产日志调用。
- **Evidence**:
  > `#[derive(Debug, Clone, Serialize, Deserialize)]`
  > `pub struct DiscordConfig {`
  > `    pub bot_token: String,`
- **Suggested fix**: 保留 Clone 与现有 serde wire/config 格式，改为手写 Debug，只将 token 字段输出为固定 redacted 值。增加不包含 token 原文的格式化断言；不要通过删除配置序列化改变持久化契约。
- **Confidence**: 1.0

### IF2-02 [P0] Configured Discord instance identity is discarded / 多实例身份被折叠
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:1297` (`DiscordChannelFactory::create`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:571` (`Handler::message`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:738` (`Handler::interaction_create`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/commands.rs:408` (`legacy_inbound`)
- **Failure mode**: 以不同配置实例 ID 注册两个 Discord bot，例如 work/personal → factory 都创建 ID 为 `discord` 的 channel，入站普通消息、slash 与按钮事件也都标记为 `discord`。registry 按实际 channel.id 发布，故两个账号不能保留独立身份，后者进入前者的替换/路由身份，而不是配置指定的实例。直接调用 `DiscordChannel::new(custom_id, ...)` 也不能修复入站硬编码。
- **Evidence**:
  > `Ok(Box::new(DiscordChannel::new("discord", config)))`
  > `            channel_id: ChannelId::new("discord"),`
  > `        channel_id: ChannelId::new("discord"),`
- **Suggested fix**: 覆盖 `create_with_id`，将配置 ID 传给 channel；让 Handler 持有该实例 ID，并将同一 ID 传到 component 转换。保留旧 `create` 的默认 `discord` 兼容语义与会话/用户 ID 格式。跨模块确认：额外阅读 `/home/zou/data/workspace/Aleph/crates/aleph-slot-api/src/channel.rs:1187-1200`，默认 create_with_id 确实忽略 ID；`/home/zou/data/workspace/Aleph/src/gateway/channel_registry.rs:475-487` 确实使用配置 ID 调 factory、随后改用返回的 channel.id。
- **Confidence**: 0.99

### IF2-03 [P0] Draft chunking consumes text before truncating it / 分块静默丢字
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/draft.rs:243` (`DraftChunker::chunk`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/draft.rs:335` (`find_split_index`)
- **Failure mode**: `DraftChunker::default().chunk(&"x".repeat(4000))` → 先消费两个各 2000 字符的 body，再按 marker 缩减并截断；continuation marker 为 12 字符、end marker 为 6 字符，helper 又各预留一次，最终只保留 1976 + 1988 = 3964 个原文字符，丢失 36 个，且不会进入下一块。另一个独立触发是空 marker、cap=10、输入 `12345678. abcdef`：边界 helper 已返回 `. ` 结束位置 10，调用方再加 2，消费到 12；最终截断掉 `ab`。这是导出 chunker 的确定性缺陷，不宣称当前 main send 路径已经接入该 helper。
- **Evidence**:
  > `let budget = self.max_chars.saturating_sub(marker.chars().count());`
  > `truncate_reserving(&body, budget, marker)`
  > `return Some(idx + terminator.len());`
- **Suggested fix**: 在消费输入之前按 marker 后的真实 body budget 分割，将溢出原文留给后续块，不做事后有损截断；统一 `last_index_of` 的结束偏移契约，调用方不再重复加 needle 长度。保留 Unicode 边界、块顺序与最终 marker 语义，增加拼回原文的断言。跨模块确认：额外阅读 `/home/zou/data/workspace/Aleph/src/utils/text_format.rs:82-87`，truncate_reserving 的参数已经是包含 marker 的总预算；本文件的 `last_index_of` 在 390 行同样返回匹配末端。上述数字是静态算术推演，未运行 Rust 测试。
- **Confidence**: 1.0

### IF2-04 [P1] Slash commands are executed without acknowledging the interaction / Slash 未应答
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:757` (`Handler::interaction_create`)
- **Failure mode**: 一个通过所有检查的 slash command → 仅被发送到 inbound queue，Command 分支没有 response/defer；超过 Discord 的初次应答期限，客户端显示 application did not respond，即使后端操作已执行。发普通 channel 消息不能补上 interaction ACK，且入站 raw 也没有保留可供下游补 ACK 的 interaction token。用户可能因此重复提交有副作用的命令。按钮分支会 ACK，不能覆盖 slash 分支。
- **Evidence**:
  > `if let Err(e) = self.inbound_tx.send(inbound) {`
  > `    tracing::error!(error = ?e, "Failed to send inbound Discord interaction");`
  > `}`
- **Suggested fix**: 在排队执行前完成 interaction receipt ACK，或 defer 并保证后续完成 deferred response；不要只调用 defer 却永远保留 loading 状态。保留现有命令文本转换和 Core 执行路径，明确处理 ACK 失败，避免用户看见失败而实际重复执行。
- **Confidence**: 0.99

### IF2-05 [P1] Numeric zero IDs panic instead of returning ChannelError / 零 ID 触发 panic
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:1066` (`DiscordChannel::send`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:274` (`resolve_channel_id`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:284` (`parse_message_id`)
- **Failure mode**: 给适配器传入字符串 conversation ID `0`、`dm:0`，或 reply/message ID `0` → u64 parse 成功，但随后 Serenity ID::new(0) panic。send/edit/delete/react 的可恢复错误 API 因此变成任务 panic；reply_to 的 `if let Ok` 也不能阻止合法数值零。这里是通用字符串 ID 到 Discord ID 的适配边界，不是要求内部类型重复校验。
- **Evidence**:
  > `SerenityChannelId::new(message.conversation_id.as_str().parse().map_err(`
  > `    .map(SerenityChannelId::new)`
  > `    .map(SerenityMessageId::new)`
- **Suggested fix**: 在这些适配边界拒绝 zero，再构造 Serenity ID，并返回现有 ChannelError 类别；覆盖 DM user、reply_to 以及 API 数值 guild ID 的同类构造。保留正常 snowflake 的 wire 格式。跨模块确认：额外阅读 `/home/zou/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serenity-0.12.5/src/model/id.rs:42-49`，new(0) 明确使用 NonZeroU64 检查并 panic。
- **Confidence**: 1.0

### IF2-06 [P1] start reports success after gateway failure or readiness timeout / 启动失败仍报成功
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:949` (`DiscordChannel::start`)
- **Failure mode**: client 构造成功但 gateway 在 READY 前失败（例如 gateway 拒绝配置的 privileged intents），或 READY 超过 15 秒 → 后台设置 Error/连接仍未就绪，start 只等待超时并记录 warn，最终无条件返回 Ok。调用方收到成功的启动结果，随后 send 却不可用，启动审计也并非注释声称的已完成握手之后的测量。
- **Evidence**:
  > `tracing::warn!(`
  > `    "Discord gateway did not signal ready within {:?}; channel may not be connected yet",`
  > `Ok(())`
- **Suggested fix**: 将 READY 和后台启动终止作为同一个启动结果返回；超时返回 ChannelError，并完成该次启动任务清理。保留已连接后自动重连的既有 Serenity 行为，不把终态 Error 当成成功启动，也不让超时任务成为未持有的第二连接。
- **Confidence**: 0.99

### IF2-07 [P1] The thread-events option never subscribes to its required intent / Thread 配置没有订阅
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:853` (`DiscordChannel::start`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:762` (`Handler::thread_create`)
- **Failure mode**: `config.intents.guild_threads=true` → handlers 虽然放行 thread_create/update/delete，但构建 intents 从 empty 开始，仅添加四种 message/member intents，从未添加 GUILDS；Discord 不会下发这些 thread lifecycle 事件，因此 thread_bindings 缺失/不更新。不是以“无调用者”推断死代码，而是 producer 所需协议订阅位明确缺失。
- **Evidence**:
  > `let mut intents = GatewayIntents::empty();`
  > `async fn thread_create(&self, _ctx: Context, new_channel: GuildChannel) {`
  > `    if !self.config.intents.guild_threads {`
- **Suggested fix**: guild_threads 启用时加入 `GatewayIntents::GUILDS`，保持配置关闭时的行为与现有 handler 逻辑。跨模块确认：额外阅读 `/home/zou/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serenity-0.12.5/src/model/gateway.rs:466-490`，THREAD_CREATE/UPDATE/DELETE/LIST_SYNC 明列在 GUILDS 下。
- **Confidence**: 1.0

### IF2-08 [P1] Guild-qualified lookup silently picks the first duplicate guild name / 同名 guild 消解不安全
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/resolver/mod.rs:60` (`DiscordResolver::resolve`)
- **Failure mode**: bot 所在两个 guild 的名字相同或 slug 相同，且两者都有 general，用户解析 `guild/general` → `.find` 只保留第一个 guild，得到唯一 candidate 并返回成功，而不是既有 Ambiguous 错误；若据此发送，目标是枚举顺序决定的服务器。这是公开 resolver API 的错误目标消解，不假定它已接入生产 send。
- **Evidence**:
  > `let target_guild = guilds_data`
  > `    .iter()`
  > `    .find(|(g, _)| to_guild_slug(&g.name) == to_guild_slug(guild));`
- **Suggested fix**: 搜集所有 slug 匹配的 guild，再统一搜索并使用现有 candidates.len 的 NotFound/Resolved/Ambiguous 分支。保留按 ID 查找和不带 guild 的搜索格式，不添加第二套歧义策略。
- **Confidence**: 1.0

### IF2-09 [P1] A transient guild fetch failure poisons the resolver cache / 瞬态故障被缓存成不存在
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/resolver/mod.rs:128` (`load_guilds_and_channels`)
- **Failure mode**: 一次 list_channels 遭遇网络错误/Discord 5xx → 所有错误都被当成 no permission 跳过，然后仍把缺失该 guild 的结果写入永久 Option cache。后续同一 resolver 的 resolve/list_channels 不再触网，故障恢复后仍持续 NotFound 或缺少渠道；瞬态错误被变成稳定且误导的成功快照。
- **Evidence**:
  > `Err(_) => {`
  > `    tracing::debug!("Skipping channels for guild {} (no permission)", guild.name);`
  > `*cache = Some(Cache {`
- **Suggested fix**: 最小修复是加载失败时返回现有错误且不提交 cache；若必须容忍确证的 permission denial，应只区分该错误而不要缓存未知故障造成的不完整快照。保留成功快照唯一所有者和既有 resolution 错误协议，无需引入通用缓存框架。
- **Confidence**: 0.99

### IF2-10 [P1] The guild-channel allowlist incorrectly rejects DM slash commands / DM 的 slash 判据不一致
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:683` (`Handler::interaction_create`)
- **Failure mode**: `dm_allowed=true` 且 allowed_channels 列出获准的 guild 渠道 → 普通 DM 消息和 DM 按钮不受 guild 渠道白名单影响，DM slash 却无条件用私信 channel ID 检查同一列表并返回。配置明确定义 allowed_channels 为“within guilds”，因此被允许的同一私信 surface 只有 slash 被错误拒绝。
- **Evidence**:
  > `if !self.config.is_channel_allowed(command.channel_id.get()) {`
  > `    return;`
  > `}`
- **Suggested fix**: 和普通消息 483 行、component 366 行使用相同的 guild-present 前提；继续保留 dm_allowed 和 guild allowlist 的现有拒绝规则，不能通过删除频道检查来允许未授权 guild 命令。
- **Confidence**: 1.0

### IF2-12 [P2] stop returns before its gateway owner has stopped / 停止后旧任务仍能覆盖新连接
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:1006` (`DiscordChannel::stop`), `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/mod.rs:909` (`DiscordChannel::start` background task)
- **Failure mode**: stop 发送 oneshot 后立即返回，旧任务还在等待 shard_manager.shutdown_all；调用方随后 start，新 READY 把共享状态设为 Connected；旧任务最后执行 933-935 行又把同一个状态设为 Disconnected。新连接仍活着但 send 被状态门拒绝。触发顺序来自实际异步 shutdown 与丢弃的 JoinHandle，不是假设锁内跨 await。旧连接还可能在 stop 已返回后继续交付事件。
- **Evidence**:
  > `tokio::spawn(async move {`
  > `client.shard_manager.shutdown_all().await;`
  > `let _ = shutdown_tx.send(());`
- **Suggested fix**: channel 保存连接任务 JoinHandle，stop 发信号后等待该任务实际退出，再完成 Disconnected/http 清理；start 不得与未退出的上一代重叠。保留现有 Error 不被正常 epilogue 覆盖的规则，不跨 await 持有状态锁。
- **Confidence**: 0.98

### IF2-13 [P2] Guild listing fetches only the first page / Guild 列表分页缺失
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/api.rs:120` (`list_guilds`)
- **Failure mode**: bot 加入超过 200 个 guild → 函数只请求首个 200 条页并返回 Ok，后续 guild 永远不在面板列表或 resolver 的初始快照中，尽管 API 自称列出所有 guild。达到阈值即可稳定复现，不依赖竞态。
- **Evidence**:
  > `/// List all guilds the bot is a member of.`
  > `.get_guilds(None, Some(200))`
  > `Ok(summaries)`
- **Suggested fix**: 用上一页末尾 snowflake 作为 after 游标，逐页拉到不足页大小后，再按现有逻辑生成 summaries；保留 API 错误传播，不把中途失败的部分结果宣告完整。跨模块确认：额外阅读 `/home/zou/data/workspace/Aleph/src/gateway/handlers/discord_panel.rs:167`，真实面板 consumer 调用该函数。
- **Confidence**: 1.0

## Clean files

以下文件完整阅读后未提出满足本轮证据阈值的 finding，不表示已经完成运行验证。

- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/relay_tests.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/protocol.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/relay_tests/lifecycle_dispatch.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/relay.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/transport.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/channel.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/relay_tests/limits.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/config.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/voice_call/mod.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/reconnect.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/permissions.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/security/startup_audit.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/security/audit_hooks.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/group_policy.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/session_key.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/resolver/strategy.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/resolver/input.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/resolver/error.rs`
- `/home/zou/data/workspace/Aleph/src/gateway/interfaces/discord/security/mod.rs`


## DO NOT FIX (1) — refuted / deferred by adjudication or routing

- IF2-11: ### IF2-11 [P1] Synchronous history persistence blocks the async voice driver / 同步历史 I/O 阻塞语音驱动
