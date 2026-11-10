# Fix brief — IF4

- worktree: /home/zou/data/workspace/Aleph-wt-fix-extfx
- branch: review/occams-ext-fx-fixes
- findings file: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/findings/IF4.md
- final verdicts: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/final/IF4.md
- classification: /home/zou/data/workspace/Aleph/docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/classification/IF4.class.md
- fix note path (in worktree): docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/fixes/FIX-IF4.md

## FIX THESE (29)

### IF4-01 [P0] XMPP 在未加密连接上发送 SASL PLAIN 密码
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/xmpp/message_ops/ops.rs:181` (`XmppMessageOps::run_xmpp_loop`); `src/gateway/interfaces/xmpp/config.rs:41` (`XmppConfig::use_tls`)
- **Failure mode**: 使用默认 `use_tls=true` 连接服务器 → 实际直接拆分裸 TCP，在收到 features 后发送包含用户名和密码的 SASL PLAIN；没有 STARTTLS、TLS 握手或证书验证。网络观察者取得可直接解码的凭据；要求 TLS 的正常服务器则无法登录。
- **Evidence**: `let stream = match TcpStream::connect(&addr).await {`; `let (mut reader, mut writer) = stream.into_split();`; `let auth = build_auth_stanza(&config.jid, &config.password);`
- **Suggested fix**: 在认证前完成受验证的 TLS/STARTTLS，并按服务器宣告机制认证；实现之前对要求 TLS 的配置明确返回不支持，不能继续明文认证。保留 JID、SASL 负载与上线顺序，改变的仅是不安全的连接路径。
- **Confidence**: 1.0

### IF4-02 [P0] XMPP 属性提取可把正文内容当作发送者身份
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/xmpp/message_ops/xml_helpers.rs:40` (`extract_attribute`); `src/gateway/interfaces/xmpp/message_ops/stanza.rs:127` (`parse_message_stanza`)
- **Failure mode**: 合法 stanza `<message from='attacker@example.org' type='chat'><body>from="victim@example.org" type="groupchat"</body></message>` → helper 在整个 XML 中优先找双引号属性，得到正文中的 victim/groupchat，而不是根元素上的真实属性。转换后 sender/conversation/group 身份均受正文控制，不能作为路由和权限判断的可信身份。
- **Evidence**: `if let Some(start) = xml.find(&dq_pattern) {`; `let from = extract_attribute(stanza, "from").unwrap_or("").to_string();`; `let msg_type = extract_attribute(stanza, "type")`
- **Suggested fix**: 用 XML 事件解析取得根 message 的精确属性；至少将属性扫描限制在正确的根开始标签且匹配属性边界，不能遍历子元素/正文。保留两种引号与合法转义的协议兼容性，不再信任正文提供的身份。
- **Confidence**: 0.99

### IF4-03 [P0] 四种配置的派生 Debug 直接暴露认证秘密
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/line/config.rs:38` (`LineConfig`); `src/gateway/interfaces/matrix/config.rs:36` (`MatrixConfig`); `src/gateway/interfaces/xmpp/config.rs:21` (`XmppConfig`); `src/gateway/interfaces/imessage/bluebubbles/config.rs:23` (`BlueBubblesConfig`)
- **Failure mode**: 对任一已填入真实凭据的配置执行 Debug 格式化（例如排障日志）→ token、LINE signing secret、XMPP/BlueBubbles password 原文进入输出。这里不声称生产当前必然记录了配置；泄密发生在公开 Debug 契约本身，明确违反本批 secret hygiene 约束。
- **Evidence**: `#[derive(Debug, Clone, Serialize, Deserialize)]`; `pub channel_access_token: String,`; `pub channel_secret: String,`
- **Suggested fix**: 删除这些类型的派生 Debug，改为遮蔽秘密字段的手写实现；包含非秘密配置的信息仍可供排障，serde/wire 格式与认证值不变。BlueBubbles 的 JsonSchema derive 保留。
- **Confidence**: 1.0

### IF4-04 [P0] BlueBubbles 将单人目标解析为包含该联系人的群聊
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/api.rs:188` (`BlueBubblesApi::resolve_chat_guid`)
- **Failure mode**: 发送到某个电话号码/邮箱，查询结果中先出现包含此联系人的群、后出现其私聊 → `matches_part` 在群上成立，立即返回并缓存群 GUID。原本的私人内容随后发送给整个群；缓存使后续发送持续错误。
- **Evidence**: `.any(|p| p.address.as_deref() == Some(target));`; `if matches_id || matches_part {`; `cache.lock().await.put(target, &g);`
- **Suggested fix**: 对参与者匹配只接受明确的一对一 chat，并使精确 chat identifier 匹配优先；显式群 GUID 继续按原意使用。不要将群参与关系当作私聊身份，保留现有缓存边界。
- **Confidence**: 1.0

### IF4-05 [P0] iMessage 批量解码失败时已前移水位导致前面消息永久丢失
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/imessage/db.rs:281` (`MessagesDb::poll_new_messages`); `src/gateway/interfaces/imessage/mod.rs:173` (`IMessageChannel::start_polling` spawned loop)
- **Failure mode**: 一批中先有合法行 A，随后某行 B 的列类型/非空约束不满足 `row.get` → A 已使 `last_message_rowid` 前移，但 B 的 `?` 丢弃整个已收集 Vec。调用方把错误变成空 Vec，却继续持久化前移水位；后续 `ROWID > watermark` 永远不再取到 A。
- **Evidence**: `let raw = row_result?;`; `self.last_message_rowid = raw.rowid;`; `Err(e) => {`（调用方随后返回 `(vec![], db.last_rowid())`）。
- **Suggested fix**: 批次成功前不提交水位，或明确逐条提交已交付前缀并单独处理坏行；不能让返回 Err 同时悄悄提交前缀。保持 ROWID 排序、单调持久化以及合法批次的交付顺序。
- **Confidence**: 0.99

### IF4-06 [P0] BlueBubbles 时间戳水位不能恢复同毫秒超过一页的消息
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/api.rs:458` (`query_messages_after`); `src/gateway/interfaces/imessage/bluebubbles/inbound/poll.rs:35` (`run_catchup_poll`, `newest_cursor`)
- **Failure mode**: 离线期间同一 `dateCreated` 毫秒积累超过 100 条记录 → 查询只返回按时间升序的前 100 条，水位直接提交该毫秒；下次查询 created-after 水位时排除其余同时间记录。恢复路径没有分页 offset 或稳定的同时间 tie-breaker，丢失剩余消息。
- **Evidence**: `"limit": 100,`; `let cursor = newest_cursor(&msgs, after);`; `tracker.advance(cursor, "imessage-bb").await;`
- **Suggested fix**: 在推进时间水位前分页排空该时间边界；可对边界做重叠查询并按 GUID 去重，但必须能够越过同时间超过一页的数据。保留排序、单调恢复和 webhook/poll 去重契约。
- **Confidence**: 0.93

### IF4-07 [P0] LINE 的禁用策略与 allowlist 未接到唯一权限决策
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/line/config.rs:58` (`LineConfig`); `src/gateway/interfaces/line/webhook.rs:258` (`dispatch_event`)
- **Failure mode**: 配置 `group_policy=disabled`，群成员发送包含 `@aleph` 的消息 → adapter 直接转发；Core 收到的却是默认 Open/require-mention，因此仍允许该消息。`dm_policy=disabled` 对已经配对的用户也同样无效，非空 allowed_users/allowed_groups 不被消费。不是“空 allowlist 开放”的争议，而是明确禁用/限制配置没有生效。
- **Evidence**: `pub dm_policy: LineDmPolicy,`; `pub group_policy: LineGroupPolicy,`; `ctx.sender`（其后直接 `.send(inbound)`，无策略传递/检查）。
- **Suggested fix**: 在启动配置转换处把 LINE 策略/名单映射到 Core 的 ChannelConfig，并按运行时 channel id 注册；不要在 adapter 再造一套权限判断。保留签名验证、身份转换与 Core 单一授权所有权。范围外确认了 `src/bin/aleph-server/commands/start/builder/subsystems.rs:1091-1142`（通用注册只解析 slash-access/access-tier，余项 Default）及 `src/gateway/inbound_router/permission.rs:79-149` 的实际消费分支。
- **Confidence**: 0.99

### IF4-08 [P0] Matrix 编辑长消息会静默覆盖为第一片
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P0
- **Location**: `src/gateway/interfaces/matrix/outbound.rs:270` (`edit_message`)
- **Failure mode**: new_text 超过 MATRIX_MSG_LIMIT 被分成多片 → 函数只取 chunks[0]，以 replacement event 覆盖原消息并返回成功，后面的文本被静默舍弃。调用者没有任何截断提示。
- **Evidence**: `let chunks = MessageFormatter::split(&formatted, MATRIX_MSG_LIMIT);`; `let chunk = &chunks[0];`; `let content = content.make_replacement(ReplacementMetadata::new(event_id, None));`
- **Suggested fix**: 在发出 replacement 前拒绝不能完整编辑的内容并返回明确错误，或实现有明示契约的多事件编辑；最小修复是拒绝多片，而非隐藏截断。保留原消息身份、单次编辑的原子可见性和成功结果意义。
- **Confidence**: 1.0

### IF4-09 [P1] 本地 iMessage 群消息的回复目标落入联系人发送分支
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/db.rs:348` (`poll_new_messages`); `src/gateway/interfaces/imessage/mod.rs:295` (`IMessageChannel::send`)
- **Failure mode**: 收到群消息后沿 conversation_id 回复 → 数据库填的是裸 chat_identifier，send 只识别 chat_id:/chat_guid: 前缀，因而调用 participant send_text，而不是已有群 chat 发送入口。正常群回复失败或找不到接收人；代码注释也明确承认该断线。
- **Evidence**: `chat.chat_identifier.clone()`; `// so they still route to send_text — fully fixing AppleScript group`; `MessageSender::send_text(target, &message.text)`
- **Suggested fix**: 为群会话传递现有 ChatInfo.guid 可解析的 chat_guid 目标并使用已有 send_to_chat；同时检查附件分支不能把群目标作为联系人发送。保留私人号码/邮箱发送行为，不另造群解析缓存。
- **Confidence**: 1.0

### IF4-10 [P1] 两种 iMessage transport 对非 path 附件返回成功但未发送
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/mod.rs:336` (`IMessageChannel::send`); `src/gateway/interfaces/imessage/bluebubbles/mod.rs:226` (`BlueBubblesChannel::send`)
- **Failure mode**: OutboundMessage 包含只有 data 或 url、没有 path 的附件 → 两个循环均静默跳过；带 caption 时只发文字，附件独立消息甚至没有发送 I/O，却返回新 UUID 或 `"ok"`。上层据成功结果不会报告附件失败。
- **Evidence**: `if let Some(path) = &attachment.path {`; `let mut last = String::from("ok");`; `message_id: MessageId::new(last),`
- **Suggested fix**: 在任何发送副作用前检测实际不支持的附件表示并明确报错；若支持 data/url，则先用现有 staging 路径物化再发送。保留已有 path 附件顺序和部分发送错误语义，不以虚构 ID 掩盖 no-op。
- **Confidence**: 1.0

### IF4-12 [P1] BlueBubbles webhook 启动失败仍报告 Connected
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/mod.rs:143` (`BlueBubblesChannel::start`); `src/gateway/interfaces/imessage/bluebubbles/inbound/webhook_server.rs:35` (`run_webhook_server`)
- **Failure mode**: webhook 端口已被占用，或上游注册返回失败 → 子任务仅记录 bind 错误并结束，注册失败也只 warn；start 不接收启动结果，最终仍返回 Ok/Connected。`BlueBubblesChannel::new` 默认没有 offset_tracker，此时既无实时 inbound 又无 catch-up，用户只看到健康通道而收不到消息。
- **Evidence**: `self.webhook_handle = Some(tokio::spawn(inbound::webhook_server::run_webhook_server(`; `tracing::error!("BlueBubbles webhook bind {addr} failed: {e}");`; `.set_status(ChannelStatus::Connected)`
- **Suggested fix**: 启动时等待 bind 就绪并向 start 传播失败；注册失败在没有可用恢复路径时返回错误，有恢复路径则明确表示降级而非无条件健康。保留已有 poll 可选性与资源清理。
- **Confidence**: 1.0

### IF4-13 [P1] 远程 BlueBubbles 默认把 webhook 注册到服务器自己的 localhost
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/api.rs:390` (`webhook_callback_url`); `src/gateway/interfaces/imessage/bluebubbles/config.rs:29` (`webhook_host`)
- **Failure mode**: Core 在 Linux、BlueBubbles 在另一台 Mac（配置文档明确允许远程 server_url），使用默认监听地址 0.0.0.0 → callback 被改成 http://localhost:8645/...。请求由远程 Mac 发起，localhost 指向 Mac 而非 Core，因此注册即使成功也无法送达实际 listener。
- **Evidence**: `"0.0.0.0" | "127.0.0.1" | "::" | "localhost" => "localhost",`; `format!("http://{host}:{port}{path}?password={enc}")`; `/// Host the inbound webhook listener binds to.`
- **Suggested fix**: 将监听地址与上游可访问的 callback 地址区别对待；最小安全过渡是在远程服务器搭配 unspecified/loopback callback 时明确拒绝并要求可达地址，不可猜成本机 localhost。保留本机部署兼容性和密码编码。
- **Confidence**: 1.0

### IF4-14 [P1] BlueBubbles 目标解析只检查前 100 个聊天
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/api.rs:177` (`resolve_chat_guid`)
- **Failure mode**: 用户有超过 100 个聊天，所需电话号码/邮箱对应 chat 不在第一页 → 查询固定 offset=0，只遍历一次，返回 None；send 将实际存在的会话报为 chat not found。缓存不能修复从未解析成功的目标。
- **Evidence**: `let body = serde_json::json!({ "limit": 100, "offset": 0, "with": ["participants"] });`; `for chat in wrap.data.unwrap_or_default() {`; `None`
- **Suggested fix**: 查询服务端精确目标，或按 offset 分页直到命中/耗尽；和 IF4-04 的一对一匹配修复保持一致，保留显式 GUID 快路径及缓存命中行为。
- **Confidence**: 1.0

### IF4-15 [P1] BlueBubbles 暂存清扫直接阻塞异步启动和 poll worker
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/staging.rs:34` (`sweep_stale`); `src/gateway/interfaces/imessage/bluebubbles/mod.rs:162` (`start`); `src/gateway/interfaces/imessage/bluebubbles/inbound/poll.rs:63` (`run_catchup_poll`)
- **Failure mode**: 暂存目录积累大量下载或目录所在文件系统慢 → start/poll 直接枚举目录并逐项 stat/remove_file，runtime worker 在全部清扫完成前不可让出。api 下载路径同步 create_dir_all 也是同一 blocking 边界问题。
- **Evidence**: `let swept = staging::sweep_stale(staging::RETENTION);`; `let Ok(entries) = std::fs::read_dir(dir) else {`; `match std::fs::remove_file(&path) {`
- **Suggested fix**: 将整次同步清扫/目录准备放到 spawn_blocking，仍 await 结果，不另起无主清扫循环。保留仅清扫专用目录、6h 保留期及逐文件 best-effort 错误语义。
- **Confidence**: 1.0

### IF4-16 [P2] BlueBubbles 附件下载没有实际字节上限
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/api.rs:482` (`download_attachment`)
- **Failure mode**: 外部发送者提交大附件，上游返回巨大的 body → `res.bytes()` 无上限完整缓冲，再全量写入磁盘。通道宣告的 100 MiB attachment cap 未在此下载边界执行，单次合法响应即可产生远超 cap 的内存/磁盘占用。
- **Evidence**: `let bytes = res.bytes().await.ok()?;`; `if let Err(e) = tokio::fs::write(&path, &bytes).await {`
- **Suggested fix**: 下载时按实际累计字节 enforce cap，并避免把整份未知大小响应先放入内存；可检查 Content-Length 提前拒绝，但不能仅信 header。保持认证请求与失败时 metadata-only 降级语义。
- **Confidence**: 1.0

### IF4-17 [P2] BlueBubbles 暂存失败的 fallback 绕过所有清理
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/imessage/bluebubbles/api.rs:503` (`download_attachment`); `src/gateway/interfaces/imessage/bluebubbles/staging.rs:19` (`staging_dir`, `sweep_stale`)
- **Failure mode**: temp 根可写，但 aleph-imessage-attachments 路径被同名文件占据或不可创建 → 下载降落到裸 temp_dir。所有定期 sweep 只检查专用子目录，持续收到不同 GUID 的附件便永久积累 fallback 文件，不受 6h retention 约束。
- **Evidence**: `let dir = super::staging::ensure_staging_dir().unwrap_or_else(|_| std::env::temp_dir());`; `std::env::temp_dir().join("aleph-imessage-attachments")`; `sweep_stale_in(&staging_dir(), max_age)`
- **Suggested fix**: 专用目录不可用时明确跳过落盘并记录失败，或让 fallback 具有同样明确的文件所有权与清理机制；最小修复是移除失去所有权的 fallback。保留不扫描/删除其他应用 temp 文件的安全边界。
- **Confidence**: 1.0

### IF4-18 [P1] LINE 默认 webhook 路径永远匹配不上正常 HTTP 请求
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/line/config.rs:83` (`default_webhook_path`); `src/gateway/interfaces/line/webhook.rs:197` (`handle_connection`)
- **Failure mode**: 使用默认配置，并将 LINE webhook 设置为正常 URL /line/webhook → request target 带前导 `/`，配置却是 `line/webhook`；精确比较失败，返回 404，签名正确的消息也不会分发。
- **Evidence**: `"line/webhook".to_string()`; `if req.path != ctx.config.webhook_path {`; `let response = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";`
- **Suggested fix**: 默认值改为 /line/webhook，并在拥有配置的边界统一处理旧的无斜线配置；保持显式路径匹配与健康检查语义，不放宽为任意路径。
- **Confidence**: 1.0

### IF4-19 [P1] LINE 入站类型使用 Rust 字段名而非实际 camelCase 字段
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/line/types.rs:143` (`FileMessage`), `src/gateway/interfaces/line/types.rs:159` (`StickerMessage`), `src/gateway/interfaces/line/types.rs:124` (`ImageMessage`, `ContentProvider`)
- **Failure mode**: 收到标准 fileName/fileSize 的 file event 或 packageId/stickerId 的 sticker event → 必填 snake_case 字段找不到，整条 event 反序列化失败，webhook 将其当成 unparseable 跳过并成功应答。image 的 contentProvider/originalContentUrl 等同样丢失，只因默认值而未报错。
- **Evidence**: `pub file_name: String,`; `pub package_id: String,`; `pub content_provider: ContentProvider,`
- **Suggested fix**: 在这些 wire 类型使用准确的 camelCase rename（包含 ContentProvider 嵌套字段）；保留内部 Rust 字段、未知事件容忍和已支持消息类型，不把实际支持的事件误判成未知。
- **Confidence**: 1.0

### IF4-22 [P1] LINE 通用 send 完全忽略标准 OutboundMessage 附件
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/line/mod.rs:169` (`LineChannel::send`)
- **Failure mode**: 标准交付路径提供 text 加 attachments，但未设置 LINE 专属 metadata → send 默认仅 push_text，并返回成功；图片、音频和视频附件均未读取。专属 metadata 的 push_image 等分支不能让通用 Channel 附件契约自动成立，附件独立消息则尝试发送空文本。
- **Evidence**: `attachments: true,`; `api.push_text(chat_id, &message.text)`; `Ok(SendResult {`
- **Suggested fix**: 将标准 attachments 转换为平台可发送的媒体消息，并保持原顺序；实现前对未支持的表示在发送前明确报错并校准 capability，不可静默丢弃。保留现有 metadata 扩展路径和 text 错误语义。
- **Confidence**: 1.0

### IF4-23 [P2] LINE listener 对未认证连接无限 spawn 且没有读超时
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/line/webhook.rs:56` (`run_webhook_server`) and `src/gateway/interfaces/line/webhook.rs:156` (`handle_connection`)
- **Failure mode**: 外部客户端持续建立 TCP 连接但不发送/不完成请求 → 每个连接都创建独立 task/socket/buffer，read 永远等待，任何签名检查都还没执行。单请求 64KiB cap 不限制连接总数；stop 也只 abort listener，未持有的子任务继续存活。
- **Evidence**: `tokio::spawn(async move {`; `let mut buf: Vec<u8> = Vec::with_capacity(4096);`; `let n = stream.read(&mut chunk).await?;`
- **Suggested fix**: 为处理中的连接施加总并发限制与整请求读取截止时间，并在 stop 时结束拥有的子任务。保留 TCP 分片累积及签名对原始 body 的验证，不通过单次 read 捷径破坏协议。
- **Confidence**: 1.0

### IF4-27 [P2] Matrix 所谓 LRU 实际无限积累且数据库仅启动时清理
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/matrix/dedupe.rs:9` (`EventDeduper::cache`, `try_insert`)
- **Failure mode**: 通道长期不重启，已加入房间中的用户持续产生新 event_id → 每条记录同时加入 HashSet 和 SQLite，运行期间均无淘汰/清理。初始化 SELECT 的 LIMIT 10000 只限制预热，不限制随后的 cache.insert；连不支持或随后被策略拒绝的 timeline event 也先加入，RAM 与磁盘随历史无限增长。
- **Evidence**: `cache: Mutex<HashSet<(String, String)>>,`; `// Prune entries older than 7 days on startup`; `cache.insert(key);`
- **Suggested fix**: 让内存 cache 真正有上限，并在运行期间按既有 7 天窗口清理持久层；缓存 miss 仍查询 DB，不能把淘汰内存误变成允许重复。保留去重键 (room_id,event_id) 与串行插入契约。
- **Confidence**: 1.0

### IF4-28 [P2] Matrix 媒体 cap 在完整分配之后才执行
- VERDICT: DOWNGRADE | ROUTE: FIX-CAREFULLY | claimed severity: P2
- **Location**: `src/gateway/interfaces/matrix/media.rs:72` (`hydrate_inbound_attachments`, `download_media`); `src/gateway/interfaces/matrix/outbound.rs:196` (`prepare_attachment`)
- **Failure mode**: 外部 event 将附件 size 省略或写小，实际 MXC body 很大 → within_cap 放行，SDK get_media_content 先产生完整 Vec，之后才检查 bytes.len。30s timeout 不限制这段时间可传输的字节。出站 URL 附件同样先完整 resp.bytes，无实际上限，远端响应可导致不受 cap 约束的内存占用。
- **Evidence**: `size.is_none_or(|s| s <= max_bytes)`; `.get_media_content(&media_request, false)`; `Ok(Ok((bytes, _mime))) if bytes.len() as u64 <= max_bytes => {`
- **Suggested fix**: 在读取实际流时限制累计字节，超限立即停止；SDK 路径若不能限流，应改用其可限制的下载接口/受控 transport，而非将 post-check 称为下载内存限制。保持认证 MXC 下载和 metadata-only 降级，向出站下载传递实际 cap。
- **Confidence**: 0.99

### IF4-29 [P1] XMPP MUC 自回声过滤使用账号名而非实际昵称
- VERDICT: UPHELD | ROUTE: FIX-CAREFULLY | claimed severity: P1
- **Location**: `src/gateway/interfaces/xmpp/message_ops/ops.rs:60` (`convert_message`) and `src/gateway/interfaces/xmpp/message_ops/ops.rs:321` (`run_xmpp_loop`)
- **Failure mode**: JID bot@example.org、默认 MUC nick=aleph → join 使用 aleph，但过滤把回声资源名与 own.local=bot 比较。room/aleph 的自身消息被当成用户消息重新路由，可能反复触发回复；真实昵称为 bot 的其他成员反而被过滤。
- **Evidence**: `if nick == own.local {`; `let muc_presence = build_muc_join_stanza(room, &config.nick);`
- **Suggested fix**: 把实际 MUC nickname 传给转换并用它过滤 MUC 回声；一对一分支仍用自身 bare JID。保留 room 身份及真正其他参与者消息，不以账号 localpart 代替昵称事实。
- **Confidence**: 1.0

### IF4-30 [P1] XMPP 出站队列在 SASL/bind 尚未完成时就写入
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/xmpp/message_ops/ops.rs:358` (`run_xmpp_loop` outbound select branch)
- **Failure mode**: start 后立即 send，或断线重连期间已有排队消息 → TCP 一连上，write_cmd_rx 分支即可被 select 选中；没有 authenticated/bound 条件，message stanza 在 SASL 或 resource bind 前发送，服务器按协议拒绝/断开，而 send 早已报告成功。
- **Evidence**: `let mut authenticated = false;`; `let mut bound = false;`; `Some(raw_stanza) = write_cmd_rx.recv() => {`
- **Suggested fix**: 仅在完成认证、bind 和必要上线步骤后消费普通出站队列；握手失败不得把消息当作已交付。保留 FIFO 与重连顺序，不在重连时悄悄丢弃队列。
- **Confidence**: 1.0

### IF4-31 [P1] XMPP 把入队当作发送成功且写失败只记录日志
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/xmpp/message_ops/ops.rs:126` (`send_message`) and `src/gateway/interfaces/xmpp/message_ops/ops.rs:359` (`run_xmpp_loop`)
- **Failure mode**: 队列接受 stanza 后连接恰好断开，writer.write_all 失败 → 用户 send 已拿到构造的 xmpp-sent ID；consumer 仅 warn 然后重连，已取出的 stanza 不再排队，也没有失败回执。交付层无法据此通知失败或判断重试，ID 甚至不是 stanza 的真实消息 ID。
- **Evidence**: `.send(stanza)`; `message_id: MessageId::new(format!("xmpp-sent-{}", Utc::now().timestamp_millis())),`; `tracing::warn!("XMPP write failed: {e}");`
- **Suggested fix**: 队列项携带发送结果回执和实际 stanza ID，consumer 完成对应写入后才确认本地发送；失败传回调用者，部分写入保持不确定结果，不盲目宣称 exactly-once 或自动重放。保留协议不保证对端已读这一边界。
- **Confidence**: 1.0

### IF4-32 [P1] XMPP 饱和发送持有读锁，使 stop 等待不可取消的网络操作
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/xmpp/mod.rs:210` (`send`), `src/gateway/interfaces/xmpp/mod.rs:243` (`send_typing`), `src/gateway/interfaces/xmpp/mod.rs:197` (`stop`)
- **Failure mode**: 对端停止读导致 writer.write_all 挂住，64 槽队列被后续 send 填满 → send 持 write_tx 的 RwLockReadGuard 等待 queue.send；stop 发出 watch 后等待同一 RwLock 写锁。连接任务正等待的 write_all 不再轮询 shutdown 分支，不能释放 receiver，停止流程可无限挂起。
- **Evidence**: `let write_tx = write_tx.read().await;`; `XmppMessageOps::send_message(write_tx, conversation, &message.text, msg_type).await`; `*self.write_tx.write().await = None;`
- **Suggested fix**: 在锁内 clone Sender，离开 guard 后再 await 发送；连接/写入等待也必须可被 shutdown 取消，并持有/收尾连接任务。保留 bounded FIFO 和关闭后 send 的 NotConnected/失败语义，不能用无限队列掩盖锁问题。
- **Confidence**: 0.98

### IF4-33 [P2] XMPP 接收缓冲对未结束 stanza 没有大小限制
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P2
- **Location**: `src/gateway/interfaces/xmpp/message_ops/ops.rs:195` (`run_xmpp_loop`)
- **Failure mode**: 对端持续发送合法 UTF-8 的 `<message><body>...` 而不结束 stanza → 每次 read 都继续追加 buffer；extract_stanza 不能提取时没有截断、拒绝或上限。单个远端流即可使 String 随输入不断增长，4096 字节 read_buf 并不限制累计内存。
- **Evidence**: `let mut buffer = String::new();`; `buffer.push_str(text);`; `while let Some((stanza, remaining)) = extract_stanza(&buffer) {`
- **Suggested fix**: 对累计未解析字节施加明确最大 stanza/buffer 大小，超限关闭并报告协议错误；保持跨 read UTF-8 拼接和正常完整 stanza 的 FIFO，不任意丢前缀后继续解析身份。
- **Confidence**: 1.0

### IF4-34 [P1] XMPP XML 实体被递归解码而非仅解一层
- VERDICT: UPHELD | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/xmpp/message_ops/xml_helpers.rs:63` (`xml_unescape`)
- **Failure mode**: 用户发送字面量文本 `&lt;`，wire body 正确编码成 `&amp;lt;` → 先替换 amp 再替换 lt，最终收到 `<`；`&amp;quot;` 等同样错误。机器人看到/处理的内容不再是用户原文。
- **Evidence**: `s.replace("&amp;", "&")`; `.replace("&lt;", "<")`; `.replace("&quot;", "\"")`
- **Suggested fix**: 以 XML 解析器单次实体解码，或至少把 amp 放在最后避免二次展开，并明确数字实体处理；保留已有五种命名实体与 Unicode 原文，不递归解释用户文本。
- **Confidence**: 1.0

### IF4-35 [P1] 本地 iMessage 的 chat 查询失败被当作私人会话继续发送
- VERDICT: DOWNGRADE | ROUTE: FIX-NOW | claimed severity: P1
- **Location**: `src/gateway/interfaces/imessage/db.rs:321` (`poll_new_messages`)
- **Failure mode**: message 有 chat_id，但对应 chat 记录缺失或查询解码失败 → `.ok()` 抹去错误，conversation_id 回退为 handle.id、is_group=false。原本来自群的消息因此可以进入私人会话策略并将回复发送给单个联系人，路由错误却没有错误日志/标识；这与本来就没有 chat_id 的消息不是同一状态。
- **Evidence**: `self.get_chat_info(chat_id).ok()`; `handle.id.clone()`; `let is_group = chat_info.as_ref().is_some_and(|c| c.is_group);`
- **Suggested fix**: 区别“没有 chat_id”与“有 chat_id 但查失败”；后者报告并拒绝不可信路由，不能降级为私人身份。配合水位修复保证拒绝不会悄悄消费消息，保留确实无 chat_id 时既有直聊处理。
- **Confidence**: 0.97

## Clean files

以下文件均已完整阅读；“clean”仅表示本轮没有独立、可证实的 finding，不表示经过动态验证。

- `src/gateway/interfaces/xmpp/message_ops/tests.rs`
- `src/gateway/interfaces/imessage/bluebubbles/inbound/mapper.rs`
- `src/gateway/interfaces/imessage/config.rs`
- `src/gateway/interfaces/imessage/target.rs`
- `src/gateway/interfaces/imessage/sender.rs`
- `src/gateway/interfaces/imessage/reaction.rs`
- `src/gateway/interfaces/matrix/client.rs`
- `src/gateway/interfaces/xmpp/message_ops/types.rs`
- `src/gateway/interfaces/imessage/bluebubbles/inbound/dedup.rs`
- `src/gateway/interfaces/imessage/bluebubbles/outbound/text.rs`
- `src/gateway/interfaces/xmpp/message_ops/mod.rs`
- `src/gateway/interfaces/imessage/bluebubbles/inbound/mod.rs`
- `src/gateway/interfaces/imessage/bluebubbles/outbound/attachment.rs`
- `src/gateway/interfaces/imessage/bluebubbles/outbound/reaction.rs`
- `src/gateway/interfaces/imessage/bluebubbles/outbound/mod.rs`


## DO NOT FIX (6) — refuted / deferred by adjudication or routing

- IF4-11: ### IF4-11 [P1] 本地 iMessage 在 Tokio worker 上执行同步 SQLite 与文件查询
- IF4-20: ### IF4-20 [P1] LINE Flex/Template/Quick Reply 的序列化形状不符合 push 协议
- IF4-21: ### IF4-21 [P1] LINE 入站非文字消息被转换成没有内容的消息
- IF4-24: ### IF4-24 [P1] Matrix 将私人房间也硬编码为群聊
- IF4-25: ### IF4-25 [P1] Matrix 与 Core 重复 mention 决策且未传递已验证信号
- IF4-26: ### IF4-26 [P1] Matrix 每个同步事件直接在 runtime worker 上读写 SQLite
