# Findings — IF6 (sms, irc, signal, email, mattermost, webhook + root)

Files reviewed: 23 (12225 LOC)

Counts: P0= 5 P1= 7 P2= 4 P3= 0

静态审查 / Static review：已逐份完整阅读批次文件；graphify 仅用于定位，不作为缺陷证据。下列跨模块读取仅用于确认工厂契约和去重消费方，不计入批次文件数。未修改源码，未运行 cargo、rustc、编译、测试或网络集成验证。

### IF6-01 [P0] Email allowlist 子串匹配可被后缀域名绕过

- **Location**: `src/gateway/interfaces/email/config.rs:140-146` (`EmailConfig::is_sender_allowed`)
- **Failure mode**: 配置允许 `boss@company.com` 或已有测试支持的 `@company.com` → `boss@company.com.attacker.example` 同样命中子串，虽实际邮箱域名由攻击方控制，仍通过邮件适配器的 sender allowlist。调用方 `src/gateway/interfaces/email/message_ops.rs:374-378` 直接以此决定是否发布入站消息；这里证明的是该 allowlist 被绕过，不把它夸大成完整邮件身份认证被绕过。
- **Evidence**:
  > `.any(|allowed| sender.contains(allowed))`
  > `if !config.is_sender_allowed(&sender) {`
  > `continue;`
- **Suggested fix**: 明确区分现有的完整 mailbox 项和 `@domain` 项；前者匹配完整邮箱，后者解析后匹配完整域名，而非任意子串。保留空列表允许所有发送者及现有 domain-list 用法，不把正常过滤逻辑搬成第二套 Core 授权。
- **Confidence**: 1.0

### IF6-02 [P0] IMAP 在消息成功发布前置 Seen，失败批次无法重取

- **Location**: `src/gateway/interfaces/email/message_ops.rs:346-356` (`EmailMessageOps::poll_imap_once`); `src/gateway/interfaces/email/message_ops.rs:409-421` (publication and acknowledgement)
- **Failure mode**: `UNSEEN` 找到一批邮件 → 使用会隐式置 `\\Seen` 的 `RFC822` FETCH 读取正文；若 fetch 流中途出错，错误又被 `r.ok()` 丢弃，或者 inbound receiver 已关闭，部分邮件从未成功发布。随后代码仍对整个 search set 执行 `+FLAGS (\\Seen)`。下一轮只搜 `UNSEEN`，这些未处理邮件不再重取。即使仅修复末尾 STORE，前面的 RFC822 读取仍会提前确认。
- **Evidence**:
  > `.fetch(&uid_set, "(RFC822 UID)")`
  > `.filter_map(|r| async { r.ok() })`
  > `if let Ok(mut store_stream) = session.store(&uid_set, "+FLAGS (\\Seen)").await {`
- **Suggested fix**: 用 `BODY.PEEK[]` 读取，保留并传播 fetch 错误；只对成功发布的消息及明确决定丢弃的消息确认 Seen，发布失败则停止并保留未处理项为 UNSEEN。确认集合和 fetch 使用同一套 sequence/UID 语义；不引入盲目重发出站邮件的行为。
- **Confidence**: 1.0

### IF6-03 [P0] 三种配置的 Debug 直接暴露凭据

- **Location**: `src/gateway/interfaces/email/config.rs:29-49` (`EmailConfig`); `src/gateway/interfaces/irc/config.rs:16-30` (`IrcConfig`); `src/gateway/interfaces/mattermost/config.rs:12-18` (`MattermostConfig`)
- **Failure mode**: 对任一配置执行正常的 `format!("{config:?}")` 或调试日志 → 派生的 Debug 输出完整邮件密码、NickServ 密码或 Mattermost bot token。三个类型均直接派生 Debug，违反本批次明确要求的 secret-bearing Debug 禁令；不需要假设上游错误响应会恰好回显秘密，也不声称已观察到生产日志泄露。
- **Evidence**:
  > `pub password: String,`
  > `pub password: Option<String>,`
  > `pub bot_token: String,`
- **Suggested fix**: 保留现有 serde 配置格式，移除敏感类型的自动 Debug，改为与 webhook 配置相同的脱敏 Debug；如没有 Debug 使用契约则直接不实现。不要把 Serialize 移除当成修复，配置保存本身有合法用途。
- **Confidence**: 1.0

### IF6-04 [P0] Signal / IRC 时间戳 ID 冲突导致正常入站消息被 Core 去重丢弃

- **Location**: `src/gateway/interfaces/signal/message_ops.rs:328-346` (`SignalMessageOps::convert_message`); `src/gateway/interfaces/irc/message_ops.rs:167-172` (`convert_privmsg`)
- **Failure mode**: Signal 同一配置实例中两个不同发送者发送毫秒时间戳相同的消息，或 IRC 同一发送者的两条缓冲消息在同一毫秒被处理 → 两条消息的 channel ID 和 message ID 都相同。跨模块确认 `src/gateway/inbound_router/mod.rs:612-624` 的 key 只有 `channel_id:message_id`，第二条会被当作重复而丢弃，并不包含 sender/conversation 来补救。
- **Evidence**:
  > `id: MessageId::new(timestamp_ms.to_string()),`
  > `Utc::now().timestamp_millis()`
  > `let dedup_key = format!("{}:{}", msg.channel_id.as_str(), msg.id.as_str());`
- **Suggested fix**: Signal 优先使用源协议稳定消息标识，或至少组合 sender、conversation 与源 timestamp，保留重放去重；IRC 使用服务器 message-id（若有），否则为实际接收到的每条事件生成唯一 ID，而不是依赖本机毫秒时钟。不要关闭 Core 去重来掩盖适配器 ID 冲突。
- **Confidence**: 1.0

### IF6-05 [P0] 四个工厂丢失 configured instance ID，同类型实例互相替换

- **Location**: `src/gateway/interfaces/irc/mod.rs:250-262` (`IrcChannelFactory`); `src/gateway/interfaces/email/mod.rs:212-224` (`EmailChannelFactory`); `src/gateway/interfaces/signal/mod.rs:256-268` (`SignalChannelFactory`); `src/gateway/interfaces/mattermost/mod.rs:328-340` (`MattermostChannelFactory`)
- **Failure mode**: 注册 `email-work` 与 `email-personal` 等两个合法同类型实例 → 四个工厂只实现 create，并把 runtime ID 固定成类型名。跨模块确认 `crates/aleph-slot-api/src/channel.rs:1187-1200` 的默认 create_with_id 会忽略传入 id；`src/gateway/channel_registry.rs:475-487` 随后以返回的 channel.id() 为准，`src/gateway/channel_registry.rs:520-545` 停止并替换该 ID 的 incumbent。于是第二个账户覆盖第一个，配置实例 ID 的寻址失效，隔离的账户共享 runtime/channel 去重身份。Webhook 与 SMS 已覆盖 create_with_id，不属于此缺陷。
- **Evidence**:
  > `Ok(Box::new(IrcChannel::new("irc", config)))`
  > `Ok(Box::new(EmailChannel::new("email", config)))`
  > `.create_with_id(&config.id, config.config.clone())`
- **Suggested fix**: 按现有 webhook/SMS 模式给这四个工厂实现 create_with_id，构造时使用传入实例 ID；保留 create 的历史默认 ID 给确实没有实例 ID 的兼容调用。不要通过改注册宏或建立第二张 ID 映射绕过工厂契约。
- **Confidence**: 1.0

### IF6-06 [P1] Email HTML 转换把普通文本与 URL 当成原始标记

- **Location**: `src/gateway/interfaces/email/message_ops.rs:122-157` (`EmailMessageOps::markdown_to_html_email`); `src/gateway/interfaces/email/message_ops.rs:589-600` (`EmailMessageOps::convert_links`)
- **Failure mode**: 发出包含普通说明文字 `Use List<T>` 或 inline code 中 `<tag>` 的回复 → 未转义内容进入 text/html，接收端将其解释为标签而不是所写文字；输入中的 `<img ...>` 也能变成真正的 HTML 元素。Markdown link 的 URL 含引号时还会逸出 href 属性。发送路径仅构造 HTML 正文，不能靠一个正确的纯文本 alternative 补救。这里不假设邮件客户端允许执行 JavaScript。
- **Evidence**:
  > `let mut html = markdown.to_string();`
  > `let replacement = format!("<a href=\"{url}\">{link_text}</a>");`
  > `<p>{html}</p>`
- **Suggested fix**: 将源文本/inline-code 作为文本节点转义，将 link destination 按属性值转义，然后才插入适配器生成的固定标签；可复用已有渲染/转义工具，但不为此引入新重依赖。保留正常 Markdown 格式和 fenced-code 换行，不转义已经生成的整份 HTML 模板。
- **Confidence**: 1.0

### IF6-07 [P1] IRC 换备用昵称后仍使用旧昵称过滤消息

- **Location**: `src/gateway/interfaces/irc/message_ops.rs:306` (`IrcMessageOps::run_irc_loop`); `src/gateway/interfaces/irc/message_ops.rs:382-393` (433 handling and PRIVMSG dispatch)
- **Failure mode**: 配置昵称 `alephbot` 已占用，433 分支请求 `alephbot_` 并成功注册 → 本地 nick 仍是 `alephbot`，convert_privmsg 继续据此做自身过滤和 require_mention。实际发给 `alephbot_` 的群内 mention 被丢弃；旧昵称的消息反而可能被当作本机器人消息跳过。另外原昵称与单下划线昵称都已占用时，每次 433 都重复请求同一个备用名，无法推进注册。
- **Evidence**:
  > `let nick = config.nick.clone();`
  > `let alt_nick = format!("{nick}_");`
  > `convert_privmsg(&parsed, &channel_id, &nick, &config)`
- **Suggested fix**: 持有并更新当前/已确认昵称，从服务器 welcome/NICK 事件校准；下一次 433 基于已尝试昵称生成不同候选，并传播写入错误。保持现有 allowlist 和 mention 规则，只修正其使用的实际 bot 身份。
- **Confidence**: 1.0

### IF6-08 [P1] IRC 把排队当成发送成功，实际写失败只打日志

- **Location**: `src/gateway/interfaces/irc/message_ops.rs:229-238` (`IrcMessageOps::send_message`); `src/gateway/interfaces/irc/message_ops.rs:428-433` (`IrcMessageOps::run_irc_loop`)
- **Failure mode**: send 将 PRIVMSG 放进有界 write channel 后立即返回成功；随后 worker 的 write_all 因断线失败 → 已出队 raw_cmd 被丢弃，只记录 warning 并重连。上层只拿到成功 SendResult，无法把这次发送标为失败或向用户报告。此问题不要求 IRC 提供端到端送达确认：本地已经观察到的写错误也未传回。
- **Evidence**:
  > `write_tx.send(raw).await.map_err(|e| {`
  > `if let Err(e) = writer.write_all(raw_cmd.as_bytes()).await {`
  > `tracing::warn!("IRC write failed: {e}");`
- **Suggested fix**: 为排队的发送附带 completion/oneshot，只有对应本地写入完成后才返回成功；写失败或 worker 退出时完成为错误。保持 FIFO 和有界背压，对可能部分写出的命令不要偷偷盲重试。
- **Confidence**: 0.99

### IF6-09 [P1] Signal 重试耗尽仍被映射为正常停止

- **Location**: `src/gateway/interfaces/signal/message_ops.rs:125-131` (`SignalMonitor::run_sse_loop`); `src/gateway/interfaces/signal/mod.rs:162-181` (`SignalChannel::start` monitor completion)
- **Failure mode**: signal-cli 不可达直至耗尽 SSE retry budget → run_sse_loop 仅 break，JoinHandle<()> 正常完成；外层 result.is_err() 为 false，将状态设为 Disconnected，而不是 Error。代码注释声称已区分 retry exhaustion 与 shutdown，但实现只区分 panic/正常返回，用户和依赖错误状态的恢复逻辑无法获知永久接收故障。
- **Evidence**:
  > `"Signal SSE max retries exceeded, stopping monitor"`
  > `if result.is_err() {`
  > `*guard = ChannelStatus::Disconnected;`
- **Suggested fix**: 让 monitor 返回明确的 shutdown / retry-exhausted 结果（例如 ChannelResult<()>），外层同时处理 JoinError 与 monitor 的业务错误；仅主动 shutdown 映射 Disconnected。保留现有 backoff 和成功连接后重置 retry budget 的规则。
- **Confidence**: 1.0

### IF6-10 [P1] Email / Signal / Mattermost 声称支持附件但成功发送时丢弃附件

- **Location**: `src/gateway/interfaces/email/mod.rs:88-104` (`EmailChannel::capabilities`), `src/gateway/interfaces/email/mod.rs:178-200` (`EmailChannel::send`); `src/gateway/interfaces/signal/mod.rs:101-111` (`SignalChannel::capabilities`), `src/gateway/interfaces/signal/mod.rs:200-215` (`SignalChannel::send`); `src/gateway/interfaces/mattermost/mod.rs:94-104` (`MattermostChannel::capabilities`), `src/gateway/interfaces/mattermost/mod.rs:229-238` (`MattermostChannel::send`)
- **Failure mode**: 根据 capability 声明向任一适配器发送含正常非空文本与附件的 OutboundMessage → 三条实际发送路径仅传文本，未读取 message.attachments，远端文本发送成功后仍返回成功，附件没有到达。Signal 还宣称 reactions/replies/typing 支持，但 react 无条件返回 UnsupportedFeature，send 不传 reply_to，send_typing 是成功 no-op。平台本身支持这些功能不等于本适配器已实现。
- **Evidence**:
  > `attachments: true,`
  > `&message.text,`
  > `"Signal reactions require signal-cli v0.12+ REST API".to_string(),`
- **Suggested fix**: 最小修复是把未实现的媒体/其他 capability 置 false（附件大小相应归零），并对直接传入的不支持附件显式返回 UnsupportedFeature，不能继续报成功。Signal 同步修正 reactions/replies/typing 声明；不在本轮凭空补齐三套媒体上传协议。
- **Confidence**: 1.0

### IF6-11 [P1] CLI stdout 同步写入会阻塞 Tokio worker

- **Location**: `src/gateway/interfaces/cli.rs:294-316` (`CliChannel::send`)
- **Failure mode**: CLI 输出被重定向到已满且暂不读取的 pipe，或终端输出阻塞 → async send future 在 runtime worker 上执行 std stdout.lock/write/flush，整条 worker 被同步 I/O 占住；单 worker runtime 的其他任务也无法推进。输入已移到专门线程，输出却仍在异步执行器中阻塞。
- **Evidence**:
  > `let mut stdout = io::stdout().lock();`
  > `writeln!(stdout, "\n{}", message.text).map_err(|e| {`
  > `stdout.flush().map_err(|e| {`
- **Suggested fix**: 把一次回复及 prompt 的完整同步写入放到 spawn_blocking，等待其结果并保持现有写入顺序和 SendFailed 错误传播；不要在 async worker 上持有 StdoutLock 或调用同步 flush。
- **Confidence**: 1.0

### IF6-12 [P2] CLI stop 未终止 stdin reader，restart 的第一行被旧线程吃掉

- **Location**: `src/gateway/interfaces/cli.rs:212-228` (`CliChannel::start`); `src/gateway/interfaces/cli.rs:278-283` (`CliChannel::stop`)
- **Failure mode**: 无输入时 start 后 stop，并让异步 line consumer 退出，再 start → 旧 std 线程仍阻塞在持有 StdinLock 的 lines() 中，新线程等待同一锁。下一行先由旧线程读取；它向已关闭 receiver 做 blocking_send 才发现关闭，随后退出，读掉的那一行没有交给新实例。每次无输入 stop 都可暂留一个无法 join/取消的线程。
- **Evidence**:
  > `std::thread::spawn(move || {`
  > `for line in stdin.lock().lines() {`
  > `if line_tx.blocking_send(text).is_err() {`
- **Suggested fix**: 让一个有明确所有权的 stdin reader/line stream 跨 start/stop 复用，重启只切换异步消费方，不再重复创建争抢 stdin 的阻塞线程；或采用真正可取消的读取。仅保存 JoinHandle 再在 stop 中 join 不够，会因无输入永久等待。
- **Confidence**: 0.99

### IF6-13 [P2] SMS submit future 被取消后留下永不回收的 pending 操作

- **Location**: `src/gateway/interfaces/sms/transport.rs:599-617` (`SmsRelayTransport::submit_impl`); `src/gateway/interfaces/sms/transport.rs:696-716` (peer submission); `src/gateway/interfaces/sms/transport.rs:1005-1007` (`prune_operations`)
- **Failure mode**: 公共 submit/submit_at future 在 authorizer await 期间被 timeout/drop，或在 peer await 期间被取消 → 操作已经插入 operations，outcome 仍为 None，正常错误清理代码没有机会运行。prune 明确永久保留 outcome=None；同 operation ID 后续始终 OperationPending，过 command expiry/retention 也不回收其正文。尤其 authorizing 阶段尚未调用 peer，不可能靠远端 receipt 自动结算；连续取消不同 ID 可无界累积。此处审的是公开的 Core-side seam，不声称仓库已有真实网络 Relay 实现。
- **Evidence**:
  > `outcome: None,`
  > `None => self.authorizer.authorize(operation).await,`
  > `if stored.outcome.is_none() {`
- **Suggested fix**: 给 pending reservation 明确的 cancellation-safe 生命周期：未越过 peer 提交边界时释放 reservation，越过边界后按 Unknown 语义结算且可按现有策略到期，不把未完成操作永久留在 None。实现时用受拥有者管理的完成/清理路径，不能简单按超时删除 Submitting 项后允许自动重发，以免产生重复收费短信。
- **Confidence**: 0.99

### IF6-14 [P1] SMS 公共提交在授权等待后仍用旧时刻校验 TTL

- **Location**: `src/gateway/interfaces/sms/transport.rs:659-664` (`SmsRelayTransport::submit_impl` final fence); `src/gateway/interfaces/sms/transport.rs:980-982` (`SmsTransport::submit`)
- **Failure mode**: outbound 已启用、lease 有效，命令在 t0 尚未到 expires_at；公共 submit 捕获 t0，随后 authorizer 等待到命令过期才成功 → 最后一次 admission 仍使用 t0，因为 pinned=None，peer.submit 被实际调用。私有路径用 Utc::now() 再检验，公共生产路径却没有同等 expiry fence。peer 契约要求的最后写入 generation 验证不能修补过期命令已获提交许可的问题。
- **Evidence**:
  > `self.submit_at(operation, Utc::now())`
  > `if pinned.is_some() { Utc::now() } else { now },`
  > `if now >= operation.command.expires_at {`
- **Suggested fix**: 在公共生产 submit 的最后提交 fence 重取当前时刻；同时保留显式 submit_at 的确定性调用者时钟语义，不直接把所有测试/模拟调用替换成真实 UTC。两条生产路径都应在 authorizer 等待之后、peer call 之前拒绝已过期命令，并保留当前 generation 和 no-blind-retry 判据。
- **Confidence**: 0.99

### IF6-15 [P2] IMAP 一次收集整个 UNSEEN 文件夹的完整正文

- **Location**: `src/gateway/interfaces/email/message_ops.rs:329-356` (`EmailMessageOps::poll_imap_once`)
- **Failure mode**: 一个积累大量未读邮件的账户启动轮询，或外部发送者在轮询间隔内投递大量/大体积邮件 → SEARCH 全量组装 fetch set，随后把所有 RFC822 正文 collect 到 Vec，直到全部取完才开始发布/释放。allowed_senders 在收完之后才判断，因此未授权发件人的大正文也占用这份无上限批次内存；可将 Core 进程推到 OOM。
- **Evidence**:
  > `.search("UNSEEN")`
  > `let fetches: Vec<_> = messages_stream`
  > `.collect()`
- **Suggested fix**: 用固定有界批次拉取/处理，并约束单封或累计正文大小，避免为了释放 session borrow 收集整个邮箱；结合 PEEK/逐项确认，让本批之外的邮件保持 UNSEEN。只限制发布消息数却仍全量 FETCH/collect 不能解决内存问题。
- **Confidence**: 1.0

### IF6-16 [P2] IRC line reader 未限制远端单行长度

- **Location**: `src/gateway/interfaces/irc/message_ops.rs:287-288` (`IrcMessageOps::run_irc_loop`)
- **Failure mode**: 远端 IRC server/网络对端持续发送字节而不发送换行 → Tokio BufReader::lines 为待完成的一行持续扩展 String，直到换行或 EOF，没有遵守 IRC 帧大小上限，单个连接就能持续占用内存。异步和 socket 读缓冲大小并不会限制 lines 内部已累积的正文。
- **Evidence**:
  > `let (reader, mut writer) = stream.into_split();`
  > `let mut lines = BufReader::new(reader).lines();`
- **Suggested fix**: 在读取阶段限制协议允许的单帧字节数（考虑适配器实际支持的扩展），超限返回协议错误并关闭/重连；不要等 next_line 完整分配后才检查长度。保留正常 PING/PRIVMSG 顺序及现有重连策略。
- **Confidence**: 1.0

## Clean files

以下文件均完整阅读，未发现达到本轮证据门槛的独立问题 / Fully reviewed; no independently substantiated finding:

- `src/gateway/interfaces/mattermost/message_ops.rs`
- `src/gateway/interfaces/webhook/mod.rs`
- `src/gateway/interfaces/webhook/message_ops.rs`
- `src/gateway/interfaces/sms/mod.rs`
- `src/gateway/interfaces/sms/message_ops.rs`
- `src/gateway/interfaces/signal/config.rs`
- `src/gateway/interfaces/webhook/config.rs`
- `src/gateway/interfaces/mod.rs`
- `src/gateway/interfaces/signal/error.rs`
- `src/gateway/interfaces/sms/config.rs`
- `src/gateway/interfaces/plugin.rs`
