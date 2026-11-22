# Findings — IF5 (Slack / QQ / WeChat / Nostr channel adapters)

Files reviewed: 42 (9991 LOC)
Counts: P0= 4 P1= 13 P2= 5 P3= 0

## Review scope / 审查范围

已完整阅读批次 42 个文件及其内嵌测试；先执行指定 graphify query，再以源码验证候选并独立审查。图谱仅用于定位，不作为证明。以下为静态发现，不声称经过平台联调或可执行复现。

All 42 batch files, including embedded tests, were read in full. Graph traversal supplied orientation only; source paths establish the findings. No compilation, tests, platform calls, source changes, or git mutations were performed.

## Findings / 发现

### IF5-01 [P0] Nostr 用公开哈希代替签名验证 / Public hash is not authentication

- **Location**: `src/gateway/interfaces/nostr/message_ops/ops.rs:162–178` (`NostrMessageOps::run_relay_loop`)
- **Failure mode**: 不可信 relay 发送任意内容，将 `pubkey` 填成 allowlist 中的用户，按公开算法重算 `id`，将 `sig` 填成任意非空字符串 → 哈希检查与 allowlist 均通过，内容以被冒充用户的身份进入 agent。攻击不需要该用户私钥；不是“修改 pubkey 却保留旧 id”的情形。
- **Evidence**:
  > `if expected_id != event.id || event.sig.is_empty() {`
  > `if !config.is_pubkey_allowed(&event.pubkey) {`
  > `convert_event_to_inbound(&event, &channel_id, &own_pubkey)`
- **Suggested fix**: 在 allowlist 与转换之前，以事件公钥对 canonical event ID 做 NIP-01/BIP340 签名验证；格式错误或验证失败直接拒绝。保留现有哈希一致性检查及授权过滤，不依赖 relay 的认证承诺。
- **Confidence**: 1.0

### IF5-02 [P0] Nostr 私信以明文发布到 relay / Plaintext kind-4 direct messages

- **Location**: `src/gateway/interfaces/nostr/message_ops/protocol.rs:59–79` (`build_dm`); `src/gateway/interfaces/nostr/mod.rs:248–270` (`NostrChannel::send`)
- **Failure mode**: 向非 `public` conversation 发送包含私密信息的回复 → 生产路径构造 kind-4 event，却把正文原样放入 `content` 后广播给 relay。接收端无法按 NIP-04 解密，relay 及获得事件的第三方可直接读取正文；签名不提供保密性。
- **Evidence**:
  > `pub fn build_dm(content: &str, pubkey: &str, recipient_pubkey: &str) -> NostrEvent {`
  > `content: content.to_string(),`
  > `message_ops::build_dm(&message.text, &own_pubkey, recipient)`
- **Suggested fix**: 支持正确的私信加密/解密后再声明 DM 可用；在此之前对 DM 发送明确报不支持，禁止明文降级。继续保留公开 note 的独立发送语义。
- **Confidence**: 1.0

### IF5-03 [P0] 四个平台的配置 Debug 暴露凭据 / Credential-bearing derived Debug

- **Location**: `src/gateway/interfaces/slack/config.rs:16–22` (`SlackConfig`); `src/gateway/interfaces/qq/config.rs:40–50` (`QQAccountConfig`); `src/gateway/interfaces/wechat/config.rs:36–42` (`WeChatConfig`); `src/gateway/interfaces/nostr/config.rs:14–20` (`NostrConfig`)
- **Failure mode**: 对任一配置做合法的 `format!("{:?}", config)`、调试输出或错误上下文打印 → 原样输出 Slack app/bot token、QQ client secret、WeChat token 或 Nostr 私钥。这直接违反批次的“凭据不得出现在 Debug impl”契约；不需要假定已经存在某个生产日志调用。
- **Evidence**:
  > `#[derive(Debug, Clone, Serialize, Deserialize)]`
  > `pub client_secret: String,`
  > `pub private_key: String,`
- **Suggested fix**: 为这四个凭据配置实现脱敏 Debug，或删除不必要的 Debug 派生；保留 serde wire 格式不变。QQ 外层配置的派生应通过内层脱敏实现，不能再次输出原 secret。
- **Confidence**: 1.0

### IF5-04 [P0] WeChat 把普通私聊与不同群合并到账号会话 / Conversation identity collapse

- **Location**: `src/gateway/interfaces/wechat/inbound/mapper.rs:84–120` (`guess_chat_type`, `map_message_to_inbound`); `src/gateway/interfaces/wechat/inbound/policy.rs:11` (`should_accept_message`); `src/gateway/interfaces/wechat/runtime.rs:147–153` (`process_messages`)
- **Failure mode**: 普通用户消息 `msg_type=1, to_user_id=account_id` 且没有 room → policy 按私聊放行，mapper 却推断为 group，并把 conversation ID 设为机器人账号，而非发送人；Alice/Bob 因而共享 conversation，context token 也写到同一键。另一个确定性错误是 typed Message 序列化后 `room_id: null` 让 `Value::get(...).or(...)` 不再读取有效 `chat_room_id`，不同群也会落到相同账号 ID。结果是会话隔离失效、回复目标错误及 token 相互覆盖。
- **Evidence**:
  > `let room_id = msg.get("room_id").or(msg.get("chat_room_id"));`
  > `&& msg.get("msg_type").and_then(|v| v.as_u64()) == Some(1));`
  > `guess_chat_type(&msg_json, account_id)`
- **Suggested fix**: 直接从 typed Message 解析非空 room 字段，policy 与 mapper 使用同一 chat-identity 推导；无 room 的入站私聊必须使用 `from_user_id`，不能按“消息发给机器人”猜成群。保留 sender identity、群授权和每会话 context-token 隔离。
- **Confidence**: 1.0

### IF5-05 [P1] Nostr 入本地队列即报成功，relay 拒绝无法到达调用者 / Publish failures are detached

- **Location**: `src/gateway/interfaces/nostr/mod.rs:270–278` (`NostrChannel::send`); `src/gateway/interfaces/nostr/message_ops/ops.rs:92–99,206–215` (`run_relay_loop`)
- **Failure mode**: 本地 write channel 接受事件后 `send` 已返回成功；随后 WebSocket 写失败，已取出的命令被丢弃，或者 relay 返回 `OK accepted=false`，循环只记录日志。调用者与 Core delivery path 均认为已发送，用户收不到回复，也没有可处理的失败结果。
- **Evidence**:
  > `.send(event_msg)`
  > `tracing::warn!("Nostr write failed: {e}");`
  > `tracing::warn!("Nostr: event {event_id} rejected by relay: {message}");`
- **Suggested fix**: 让每个 publish command 携带结果回传，并按 event ID 将 relay OK 与调用关联；对写失败、明确拒绝和有界等待超时返回适当错误。不要把不确定交付自动标成安全可重试，也不在 adapter 内另造 durable queue。
- **Confidence**: 1.0

### IF5-06 [P1] QQ 重连永久复用启动时的 access token / Reconnect retains an expired token

- **Location**: `src/gateway/interfaces/qq/mod.rs:143–158` (`QQChannel::start`); `src/gateway/interfaces/qq/gateway.rs:14–18,61–67` (`GatewayContext`, `run_gateway`)
- **Failure mode**: 启动时取 token 并把字符串复制进 GatewayContext；运行超过有效期后发生断线 → 每次 Identify 仍发送旧 token。REST TokenManager 即使刷新也不会更新该字符串，重连不能通过认证，必须重启 channel 才能取得新凭据。
- **Evidence**:
  > `let token = api`
  > `pub token: String,`
  > `"token": format!("QQBot {}", ctx.token),`
- **Suggested fix**: GatewayContext 持有共享 API/token manager，在每次 Identify 前获取当前有效 token，而非持有永久快照；保持既有 shutdown 和重连退避，不另建第二份 token 缓存。
- **Confidence**: 0.99

### IF5-07 [P1] QQ 宣称支持图片附件，发送路径却忽略它们 / Advertised media is silently discarded

- **Location**: `src/gateway/interfaces/qq/mod.rs:64–67` (`QQChannel::capabilities`); `src/gateway/interfaces/qq/delivery.rs:147–160` (`deliver_message`)
- **Failure mode**: Core 按公开 capabilities 发出含图片的 OutboundMessage → delivery 只读取 text，始终构造文本 payload，完全不消费 attachments，仍返回成功。附带正文的图片消失；仅图片消息也没有实际图片交付。
- **Evidence**:
  > `attachments: true,`
  > `msg_type: 0,`
  > `media: None,`
- **Suggested fix**: 在完成对应上传/媒体发送之前将能力声明改为真实值，且收到非空 attachments 时显式返回不支持，不允许“只发送正文却声称整条成功”；已有文本发送与顺序保持不变。
- **Confidence**: 1.0

### IF5-08 [P1] WeChat 多媒体能力声明与纯文本发送不一致 / Media capability has no outbound implementation

- **Location**: `src/gateway/interfaces/wechat/mod.rs:61–66,149–199` (`WeChatChannel::capabilities`, `send`)
- **Failure mode**: 调用者据 capabilities 发送图片、音频、视频或普通附件 → send 仅格式化 `message.text` 并构造文本 payload，未读取 attachments，最后照常返回成功。用户请求的附件没有被交付。
- **Evidence**:
  > `attachments: true,`
  > `let text = outbound::markdown::markdown_to_wechat(&message.text);`
  > `Ok(SendResult {`
- **Suggested fix**: 将尚未实现的出站媒体能力标为不支持，并在实际收到附件时明确拒绝；若保留能力，则必须接通对应平台上传与消息构造，不能把文本成功等同于附件成功。
- **Confidence**: 1.0

### IF5-09 [P1] WeChat 入站使用固定 channel ID / Registered channel identity is lost

- **Location**: `src/gateway/interfaces/wechat/runtime.rs:142–145` (`WeChatRuntime::process_messages`); `src/gateway/interfaces/wechat/mod.rs:35–45` (`WeChatChannel::new`, `with_mode`)
- **Failure mode**: 用受支持的构造参数注册 `wechat-work` 等非默认 ID → runtime 对所有入站消息写入 `channel_id="wechat"`。注册器转发保留消息字段，不改回实例 ID，因此路由/回复查找指向不存在的 channel 或另一个名为 wechat 的实例，而不是实际收信实例。
- **Evidence**:
  > `id: ChannelId::new(id),`
  > `&crate::gateway::channel::ChannelId::new("wechat"),`
- **Suggested fix**: 将实际注册的 ChannelId 随 runtime 初始化传入并用于所有映射；不从 platform type 反推实例 ID。保留多个实例的隔离以及既有 wire conversation ID。
- **Cross-module contract checked**: 只读核对了批次外 `src/gateway/channel_registry.rs:1921–1953`，forwarder 原样 `inbound_tx.send(message)`，没有重写 channel_id。/ The registry preserves the adapter-provided identity.
- **Confidence**: 1.0

### IF5-10 [P1] WeChat 只检查 HTTP 状态，吞掉发送 API 拒绝 / HTTP success masks API errors

- **Location**: `src/gateway/interfaces/wechat/api.rs:181–200,204–219` (`ILinkApi::send_message`, `send_typing`)
- **Failure mode**: iLink 返回 HTTP 200，但 envelope 包含非零 `ret`，例如 session-expired `-14` → 出站方法不读取 body，直接返回 Ok，Channel 又报发送成功。相同 API 客户端的 get_updates 已把这些 envelope 当错误处理，出站却丢失错误语义。
- **Evidence**:
  > `.error_for_status()`
  > `.map_err(WeChatApiError::Http)?;`
  > `Ok(())`
- **Suggested fix**: 解析发送响应的 ret/errcode/errmsg envelope，复用既有 SessionExpired/Api 错误语义；typing 若被定义为 best-effort，应在上层明确降级，而不是在 HTTP 边界伪造成功。避免对不确定提交盲目重发。
- **Confidence**: 0.99

### IF5-11 [P1] WeChat 在分块之前截掉长回复 / Truncation defeats the chunker

- **Location**: `src/gateway/interfaces/wechat/outbound/markdown.rs:4–15,91–96` (`markdown_to_wechat`, `truncate`); `src/gateway/interfaces/wechat/mod.rs:174–178` (`send`)
- **Failure mode**: 发送 5000 个普通 ASCII 字符 → formatter 先保留前 4000 字节并附加 truncated marker，之后 chunker 只能分割这个已丢失尾部的字符串；剩余 1000 字符不可能到达用户，send 仍成功。已经有分块路径，故不是平台长度限制迫使整条消息截断。
- **Evidence**:
  > `result = truncate(&result);`
  > `crate::utils::text_format::truncate_bytes(text, MAX_MESSAGE_LENGTH)`
  > `let text = outbound::markdown::markdown_to_wechat(&message.text);`
- **Suggested fix**: formatter 只做格式转换，不破坏正文；在唯一发送分块边界按平台字节上限、UTF-8 边界分块并顺序发送。不能用字符数上限替代此文件已明确的字节上限。
- **Confidence**: 1.0

### IF5-12 [P1] Slack 禁用操作的配置不生效 / Disabled operations remain executable

- **Location**: `src/gateway/interfaces/slack/mod.rs:279–323` (`SlackChannel::react`, `edit`, `delete`); `src/gateway/interfaces/slack/config.rs:44–46` (`SlackConfig::enable_deletion`)
- **Failure mode**: 保持默认 `enable_deletion=false`，随后调用 Channel::delete → adapter 无条件执行 Slack chat.delete，真实消息仍被删除。enable_editing/enable_reactions 也未参与对应操作，capabilities 同样没有反映这些设置。
- **Evidence**:
  > `pub enable_deletion: bool,`
  > `SlackMessageOps::delete_message(`
  > `SlackMessageOps::update_message(`
- **Suggested fix**: 在操作边界执行已经配置好的 enable 判断，关闭时明确返回不支持/禁用，并让 capabilities 与同一配置一致；开启时继续使用现有 API 和错误分类，不重新实现 Core 业务策略。
- **Confidence**: 1.0

### IF5-13 [P1] Slack 常规文件消息 subtype 被提前丢弃 / file_share events never reach media handling

- **Location**: `src/gateway/interfaces/slack/message_ops/converter.rs:78–87` (`SlackMessageOps::convert_event_to_inbound`)
- **Failure mode**: Slack 接收 `type="message", subtype="file_share"` 的普通用户文件分享（尤其 DM 中只有文件） → match 的 Some(_) 直接返回 None，后面的文件检测/下载完全不会运行。Socket envelope 已 ACK，用户看不到 agent 对文件的处理或失败。
- **Evidence**:
  > `let (msg_data, _is_edit) = match subtype {`
  > `Some(_) => return None, // Skip other subtypes (joins, leaves, etc.)`
- **Suggested fix**: 将用户 file_share 等明确可处理的消息 subtype 纳入正常消息路径，继续过滤 join/leave 等系统事件与本机器人消息；保留空正文但有文件的合法消息。
- **Confidence**: 0.99

### IF5-14 [P1] Slack 线程内 app mention 丢失回复线程 / Mention conversion drops thread_ts

- **Location**: `src/gateway/interfaces/slack/message_ops/converter.rs:47–59` (`SlackMessageOps::convert_app_mention_to_inbound`)
- **Failure mode**: 用户在线程中 @机器人，event 带有效 thread_ts → app_mention 转换硬编码 reply_to=None。socket 随后用 reply_to 作为 debounce 线程键，出站也无法沿该字段回复原线程；线程上下文丢失，回复可能出现在频道顶层。普通 message 转换在同文件 145–148 行却保留了 thread_ts。
- **Evidence**:
  > `reply_to: None,`
  > `metadata: vec![MessageMeta::AppMention],`
- **Suggested fix**: app_mention 与普通消息采用一致的 thread_ts→reply_to 映射；保留根频道 conversation ID 以及 mention metadata，不用 mention 消息自己的 ts 替代已存在的线程根。
- **Confidence**: 0.99

### IF5-15 [P1] Slack 多附件重复发送整段正文 / Multi-file sends duplicate the caption

- **Location**: `src/gateway/interfaces/slack/mod.rs:377–379,456–468` (`send_with_attachments`); `src/gateway/interfaces/slack/message_ops/api.rs:335–336` (`upload_file_with_base`)
- **Failure mode**: 非空正文附带两个成功上传的文件 → 第一个文件已经通过 initial_comment 发布完整正文，循环后又调用 chat.postMessage 发布相同正文。用户收到两份相同文本，后一个还经过额外格式转换。
- **Evidence**:
  > `Some(message.text.as_str())`
  > `complete_body["initial_comment"] = serde_json::Value::String(cap.to_string());`
  > `if !message.text.trim().is_empty() && message.attachments.len() > 1 {`
- **Suggested fix**: 明确正文只交付一次：保留首文件 caption，移除成功上传后的重复文字发送；或统一单独发正文并不给文件 caption。保留文件顺序和 thread_ts，不能简单删掉唯一的正文交付点。
- **Confidence**: 1.0

### IF5-16 [P1] Slack 部分附件已提交后仍返回可整体重试的限流 / Partial success violates retry contract

- **Location**: `src/gateway/interfaces/slack/mod.rs:442–451` (`send_with_attachments`); `src/gateway/interfaces/slack/message_ops/api.rs:356–363` (`upload_file_with_base`)
- **Failure mode**: 文件 A 已完成上传并在频道发布；文件 B 的 getUploadURL/completeUpload 返回明确 ratelimited → helper 返回 RateLimited，外层原样传播。Core 的 RateLimited 契约是“尚未交付，可安全重试”，会重新调用整条 message 的 send，文件 A 和 caption 因而被重复发布。文本多 chunk 分支已避免此问题，附件聚合边界没有同样判断。
- **Evidence**:
  > `last_result = Some(SendResult {`
  > `return Err(e);`
  > `return Err(super::super::errors::classify_slack_error(`
- **Suggested fix**: 在 send_with_attachments 的整条消息边界跟踪已提交事实；已有成功附件后，将后续 RateLimited 转为含部分成功说明的终止错误，禁止整条自动重放。只有尚未发布任何部分时保留 RateLimited；不新增 adapter 内部重试队列。
- **Cross-module contract checked**: 只读核对了批次外 `src/gateway/channel_registry.rs:1677–1765`（每次循环 `channel.send(message.clone())`，RateLimited 触发重试）及 `src/gateway/delivery_queue.rs:535–539`（仅 NotConnected/RateLimited 可入自动队列）。/ Both retry boundaries treat RateLimited as safe to resubmit.
- **Confidence**: 1.0

### IF5-17 [P1] Slack 优雅停机丢弃已 ACK 的 debounce 消息 / Shutdown discards acknowledged input

- **Location**: `src/gateway/interfaces/slack/message_ops/socket.rs:110–120,190–194,210–213` (`run_socket_mode_loop`); `src/gateway/interfaces/slack/message_ops/debouncer.rs:43–55` (`SlackDebouncer::enqueue`)
- **Failure mode**: debounce_ms>0，用户消息已 ACK 并加入尚未到期的 entry；在窗口内停机/重启 channel → shutdown 分支直接 return，局部 debouncer 被销毁，消息从未进入 Core，Slack 也不会重投已确认的 envelope。
- **Evidence**:
  > `let ack = serde_json::json!({ "envelope_id": envelope_id });`
  > `if debouncer.enqueue(resolved_inbound, thread_ts).await {`
  > `let _ = ws_tx.close().await;`
- **Suggested fix**: 优雅退出所有路径统一 drain 已接收的 debounce entries，并处理转发失败后再结束任务；保留按会话/线程合并与顺序。不要通过取消已接收消息来掩盖停机延迟；非优雅进程终止的 durable recovery 不在此最小修复中。
- **Confidence**: 0.99

### IF5-18 [P2] Slack debounce 无上限且持续流量可永久推迟交付 / Unbounded trailing-edge buffer

- **Location**: `src/gateway/interfaces/slack/message_ops/debouncer.rs:43–55,61–72` (`SlackDebouncer::enqueue`, `flush_expired`)
- **Failure mode**: 同一 sender/channel/thread 持续以短于 debounce_ms 的间隔发送消息 → 每次追加完整 InboundMessage 并重置 deadline，flush 条件始终不满足。消息及已下载附件持续驻留、用户输入永远不向 Core 交付；外层 bounded channel 不能约束这个 Vec。
- **Evidence**:
  > `entry.messages.push(msg);`
  > `entry.deadline = deadline;`
  > `.filter(|(_, entry)| entry.deadline <= now)`
- **Suggested fix**: 增加明确的最大驻留时间及消息/字节预算，达到任一上限就按原顺序 flush；限制应覆盖附件数据，而非仅正文字符数。不改变正常短 burst 的合并语义。
- **Confidence**: 1.0

### IF5-19 [P2] Slack 下载先完整分配再检查上限 / File limits do not bound allocation

- **Location**: `src/gateway/interfaces/slack/message_ops/files.rs:94–98,127–163` (`fetch_attachments`, `download_one`); `src/gateway/interfaces/slack/mod.rs:475–494` (`download_url`)
- **Failure mode**: 入站文件未提供 size（按 0 处理）或广告大小小于实际 body → resp.bytes() 完整缓冲后才检查 max_bytes，超大响应可远超配置的内存预算，15 秒 timeout 不能替代字节限制。出站附件 URL 的 download_url 也完整缓冲，连同样的大小检查都没有；用户可控制的 URL/body 可造成显著内存消耗。
- **Evidence**:
  > `Some(url) if file.size <= max_bytes => {`
  > `resp.bytes().await.ok()`
  > `if bytes.len() as u64 > max_bytes {`
- **Suggested fix**: 对已知 Content-Length 先拒绝超限，并增量读取、按累计字节在越界前停止；出站 URL 使用对应媒体预算和请求时限。保留入站 metadata-only 降级策略及认证 header，不把单个超限文件升级为全部消息失败。
- **Confidence**: 1.0

### IF5-20 [P2] Slack 可选用户名查询无限期占住收信循环 / Cosmetic lookup stalls the socket loop

- **Location**: `src/gateway/interfaces/slack/directory.rs:27–32,62–73` (`UserDirectory::new`, `fetch_user_name`); `src/gateway/interfaces/slack/message_ops/socket.rs:165–168` (`run_socket_mode_loop`)
- **Failure mode**: resolve_user_names=true、缓存未命中，users.info 连接建立后响应不结束 → 默认 Client 无整体 timeout，socket 事件分支同步 await 该查询；后续 WebSocket 读、ACK、debounce flush 与 shutdown select 均不能继续。仅为了显示名便可挂住整个 channel。这里是单个异步循环的 head-of-line stall，不是声称发生同步线程阻塞。
- **Evidence**:
  > `client: reqwest::Client::new(),`
  > `.get(format!("{SLACK_API}/users.info"))`
  > `dir.resolve(inbound.sender_id.as_str()).await`
- **Suggested fix**: 给用户名解析增加覆盖请求与 body 的短时限，失败/超时回退 sender ID，不能阻止原消息交付；无需增加并发框架或复制消息状态。
- **Confidence**: 0.99

### IF5-21 [P2] QQ 单航班使用一次性 receiver，三个并发请求就会穿透 / Token refresh is not single-flight

- **Location**: `src/gateway/interfaces/qq/api.rs:47–69` (`TokenManager::get_token`)
- **Failure mode**: 缓存为空/过期：A 发布 in_flight receiver 并开始认证；B take 走 receiver 等待 A；C 看到空槽又发起认证。A 完成时无条件 take 还可能清掉 C 的槽。因而正常并发发送可以产生多次认证请求与失效的协调关系，而不是共享一个刷新结果。
- **Evidence**:
  > `if let Some(rx) = in_flight.take() {`
  > `*in_flight = Some(rx);`
  > `let _ = self.in_flight.lock().await.take();`
- **Suggested fix**: 移除一次性 receiver 协调，使用一个刷新 mutex，在取得锁后重查 cache，再 fetch 并填 cache；等待者复用结果，失败明确返回。该锁只序列化有意共享的 token 刷新，不把所有发消息网络请求串行化。
- **Confidence**: 1.0

### IF5-22 [P2] WeChat 游标和 context-token 持久化失败完全静默 / Persistence failures are invisible

- **Location**: `src/gateway/interfaces/wechat/sync_buf.rs:36–46` (`save_sync_buf`); `src/gateway/interfaces/wechat/auth.rs:65–85` (`ContextTokenStore::persist`); `src/gateway/interfaces/wechat/runtime.rs:84–90` (`start`)
- **Failure mode**: 数据目录只读、磁盘满或 rename 失败 → 两处持久化均返回 unit 并丢弃所有文件错误；runtime 仍推进内存游标。重启读取旧游标/旧 context tokens，可能重收历史消息或无法回复，却没有任何“状态未保存”的错误可供用户处理。这里不声称本地游标能提供端到端 exactly-once。
- **Evidence**:
  > `let _ = tokio::fs::write(&tmp_path, json).await;`
  > `let _ = tokio::fs::rename(&tmp_path, &path).await;`
  > `*buf = new_buf.clone();`
- **Suggested fix**: 持久化返回 Result，向运行状态/日志明确传播保存失败及上下文，不再把失败与成功等价；只在写成功后 rename，保留已有原子替换方式。错误处理不得靠立即重跑整批已交付消息来“恢复”，避免引入新的重复交付。
- **Confidence**: 0.99

## Exclusions and limits / 排除项与边界

- 未把 adapter 结构相似、函数较长、一般 clone、所有 await 或缺少测试当成 finding。未证实新的 Core 决策被 adapter 重实现，因此不以 R4 为由提出推测性业务层重构。/ No architectural finding was inferred from adapter size or superficial duplication.
- QQ 空 allowlist 放行有显式测试固定该语义；现有材料不足以把这一兼容约定直接判为授权回归，未列入正式发现。额外账号被忽略已有启动告警，也未当成静默故障。/ Intentional empty-list semantics and the warned single-account limitation were not classified as regressions.
- QQ 被动回复 msg_seq 的平台去重规则、WeChat client_id 的服务端幂等规则及未接线媒体解密链路需要额外协议/集成证据，未据此提出确定性结论。未将 get_updates 的错误退避误报为每轮长轮询休眠，也未忽略发送失败时已有的 dedup.remove。/ Unverified platform assumptions were excluded.
- 未运行 cargo、rustc、测试、实网/真机验证；未修改源码、构建产物或 git 状态。上述 counts 仅计静态验证后保留的发现。/ Executable and platform validation remains outstanding; no fixes were applied.

## Clean files

以下批次文件已完整阅读，未发现满足本轮具体触发条件与错误后果要求的独立问题；“clean”不代表平台集成已验证。/ Fully read, with no independently established finding in this review.

- `src/gateway/interfaces/nostr/message_ops/tests.rs`
- `src/gateway/interfaces/slack/message_ops/tests.rs`
- `src/gateway/interfaces/wechat/types.rs`
- `src/gateway/interfaces/wechat/media.rs`
- `src/gateway/interfaces/slack/errors.rs`
- `src/gateway/interfaces/qq/handlers.rs`
- `src/gateway/interfaces/wechat/inbound/dedup.rs`
- `src/gateway/interfaces/qq/access.rs`
- `src/gateway/interfaces/qq/types.rs`
- `src/gateway/interfaces/wechat/outbound/mapper.rs`
- `src/gateway/interfaces/nostr/message_ops/types.rs`
- `src/gateway/interfaces/nostr/message_ops/mod.rs`
- `src/gateway/interfaces/slack/message_ops/mod.rs`
- `src/gateway/interfaces/qq/error.rs`
- `src/gateway/interfaces/wechat/inbound/mod.rs`
- `src/gateway/interfaces/wechat/outbound/mod.rs`
