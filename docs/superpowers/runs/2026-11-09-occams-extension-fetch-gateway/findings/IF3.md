# Findings — IF3 (Feishu + WhatsApp channel adapters / 飞书与 WhatsApp 通道适配器)

Files reviewed: 54 (11537 LOC)
Counts: P0= 4 P1= 14 P2= 3 P3= 0

本批为静态审阅：完整阅读批次全部文件；graphify 仅用于定位，不作为证据。未修改源码，未执行编译、测试或真机网络验证。下文明确区分生产接线与仅公开 API 可达的缺陷。

Static review: every batch file was read in full; graphify supplied orientation only. No source edits, compilation, tests, or live-provider verification. Findings distinguish production wiring from public-API-only reachability.

### IF3-01 [P0] Encryption mode accepts unsigned plaintext events / 加密模式接受未签名明文事件

- **Location**: `src/gateway/interfaces/feishu/feishu_inbound/webhook_server.rs:141-161` (`resolve_payload`)
- **Failure mode**: With an Encrypt Key configured, submit a plaintext event containing the valid verification token but no `encrypt` field or signing headers → the event bypasses signature verification and timestamp freshness and can enter the inbound pipeline. This is a downgrade of the configured signature boundary, not a claim that requests without the verification token are accepted. / 配置加密密钥后，持有 verification token 的请求可通过省略加密字段绕过签名和时效校验。
- **Evidence**: `match (state.config.encrypt_key.as_ref(), encrypt_field) {`; `(Some(key), Some(encrypted)) => {`; `_ => Ok(body_str.to_string()),`. `verify_payload_token` at lines 165–178 still checks the token, but never restores the skipped signature/freshness checks.
- **Suggested fix**: When a key is configured, require the authenticated encrypted event envelope and reject its absence. If platform URL verification needs a plaintext exception, constrain it to that handshake shape rather than all event bodies. Preserve token validation, encrypted challenge support, and fail-closed signature checks. / 仅为确有需要的握手保留窄例外，普通事件不得降级到明文。
- **Confidence**: 0.99

### IF3-02 [P0] Group card actions are classified as DMs / 群卡片操作被当作私聊

- **Location**: `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs:62-80` (`map_event_to_inbound`, `CardAction` arm)
- **Failure mode**: An otherwise DM-approved user clicks an actionable card in a group whose messages are disabled or not allowlisted → the action is mapped with `is_group: false`, bypassing the group gate and mention requirement. The central permission check also branches on this same flag, so it does not repair the lost classification. / 已获私聊授权的用户可以通过群卡片交互绕过群访问策略。
- **Evidence**: `conversation_id: ConversationId::new(chat_id.clone()),`; `is_group: false,`; `raw: None,`. `src/gateway/interfaces/feishu/feishu_policy/group_policy.rs:22-24` immediately passes non-group messages. Cross-module confirmation: `src/gateway/inbound_router/permission.rs:79-118` chooses group versus DM authorization using `ctx.message.is_group`.
- **Suggested fix**: Resolve the originating card's authoritative chat type and session scope before mapping the action; reject unresolved chat context instead of treating it as a DM. Apply the same group policy as ordinary messages while preserving the intended explicitly addressed-card interaction semantics. / 不得把未知聊天类型默认成私聊。
- **Confidence**: 0.99

### IF3-03 [P0] Credential persistence corrupts arbitrary binary bytes / 凭据持久化损坏二进制数据

- **Location**: `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:60-79,119-124` (`WaAuthManager::save`, `load`)
- **Failure mode**: Call the public encrypted save/load API with real binary auth data, e.g. `creds_blob: vec![0xff]` → UTF-8 replacement changes serialized bytes before encryption; save returns success but load cannot reconstruct the credentials. `0xff` becomes `ef bf bd`, invalidating both payload and subsequent bincode offsets. Current production `RealWaRuntime` uses SQLite via `db_path`, not these save/load methods; this is a verified persistence-API defect, not a claim that the current SQLite backend is corrupted. / 公开凭据保存接口会不可逆地替换非 UTF-8 字节；当前生产 SQLite 路径不经过该接口。
- **Evidence**: `bincode::serialize(data).map_err(|e| WaAuthError::Serialization(e.to_string()))?;`; `.encrypt(&String::from_utf8_lossy(&bytes))`; `let data: WaAuthData = bincode::deserialize(&decrypted.into_bytes())`.
- **Suggested fix**: Use a lossless byte representation across the text-only encryption boundary, such as base64 with matching decoding on load. Distinguish the new stored encoding from existing entries and request re-authentication for irrecoverably corrupted entries; do not reintroduce plaintext fallback. / 保留加密 fail-closed 行为，使用无损编码并明确旧数据处理。
- **Confidence**: 1.0

### IF3-04 [P0] Configured chat restriction is never enforced / 配置的聊天限制没有接线

- **Location**: `src/gateway/interfaces/whatsapp/mod.rs:409-418,439-468` (`WhatsAppChannel::start` inbound policy construction and dispatch); `src/gateway/interfaces/whatsapp/config.rs:21,46-50` (`allowed_chats`, `is_chat_allowed`)
- **Failure mode**: Set a nonempty `allowed_chats` restricting the channel to chat A, while the sender/chat otherwise passes the configured access and central authorization policies → a message from chat B is still forwarded. The adapter only constructs policy from `config.access`; the separately exposed chat restriction has no consumer. / 限定到 A 聊天的配置不会阻止其他已获一般授权的聊天 B。
- **Evidence**: `pub allowed_chats: Vec<String>,`; `self.allowed_chats.iter().any(|c| c == chat_id)`; `match policy.evaluate(&msg) {`. The complete inbound branches contain no call to `is_chat_allowed`; a source-wide reference check found only its definition and the field in this config.
- **Suggested fix**: Feed the nonempty restriction into the existing owning chat-policy boundary, or apply its intersection before forwarding; preserve empty-list unrestricted behavior and all existing access checks. Do not create another independent authorization registry. / 与现有访问策略取交集，不新增第二份授权权威。
- **Confidence**: 0.98

### IF3-05 [P1] WebSocket inbound uses the app ID as channel ID / WebSocket 入站误用应用 ID

- **Location**: `src/gateway/interfaces/feishu/mod.rs:266-272` (`FeishuChannel::start_runtime`)
- **Failure mode**: Start a normal Feishu channel whose registry ID is `feishu` and app ID is `cli_...` → all WebSocket messages carry `cli_...` as `channel_id`. The router consults the wrong policy/binding key and replies target a channel absent from the registry. Webhook mode correctly uses `self.info.id`. / 默认 WebSocket 模式把入站策略和回信路由到未注册的应用 ID。
- **Evidence**: `.start(`; `ChannelId::new(&self.config.app_id),`; `self.channel_state.sender(),`. Cross-module confirmation: `src/gateway/inbound_router/agent_resolver.rs:197-198` copies `msg.channel_id` into `ReplyRoute`; `src/gateway/inbound_router/permission.rs:72-77` uses it for policy lookup.
- **Suggested fix**: Pass `self.info.id.clone()` as in webhook startup, retaining `app_id` solely as provider credentials. Preserve instance identity end to end. / WebSocket 与 webhook 使用同一注册实例 ID。
- **Confidence**: 1.0

### IF3-06 [P1] Session-scoped IDs overwrite the transport chat ID / 会话作用域 ID 覆盖传输聊天 ID

- **Location**: `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs:33-34,95-100` (`build_conversation_id`); `src/gateway/interfaces/feishu/feishu_policy/group_policy.rs:28-36` (`GroupPolicyEngine::evaluate`)
- **Failure mode**: Configure user- or thread-scoped group sessions → inbound `conversation_id` becomes `oc_chat:ou_user` or `oc_chat:om_root`. A raw-chat allowlist entry `oc_chat` no longer matches. With an open group policy, the same synthetic ID is copied into the reply route and used as a Feishu `chat_id`, making normal replies invalid. / 非默认群会话作用域同时破坏群白名单和实际回信目标。
- **Evidence**: `GroupSessionScope::User => format!("{chat_id}:{sender_id}"),`; `let chat = msg.conversation_id.as_str();`; `let chat_id = message.conversation_id.as_str();` (the last line is `src/gateway/interfaces/feishu/feishu_outbound/sender.rs:43`). Cross-module confirmation: `src/gateway/inbound_router/agent_resolver.rs:197-198` directly copies the inbound conversation into the reply route.
- **Suggested fix**: Preserve the platform chat ID for policy checks and delivery; carry user/thread scoping into the existing session-key derivation instead of rewriting the transport address. Preserve configured session isolation and allowlist meaning. / 会话隔离与平台收件地址必须分别派生。
- **Confidence**: 1.0

### IF3-07 [P1] Distinct card clicks share one deduplication key / 不同卡片点击共用去重键

- **Location**: `src/gateway/interfaces/feishu/feishu_inbound/mapper.rs:62-68` (`CardAction` mapping); `src/gateway/interfaces/feishu/feishu_inbound/events.rs:128-132,151-155` (`parse_card_action`)
- **Failure mode**: Two users, or one user choosing two different buttons, act on the same card within the 300-second dedup TTL → both actions use the card's `open_message_id`, so the second independent action is discarded as a redelivery. Both WebSocket and webhook paths deduplicate `inbound.id` before policy dispatch. / 同一卡片上的第二次独立操作会被误删。
- **Evidence**: `.pointer("/context/open_message_id")`; `message_id,`; `id: MessageId::new(message_id),`. The source message ID is also separately retained in `reply_to`, so it need not serve as the unique action identity.
- **Suggested fix**: Retain the event's unique identity from the event header for action deduplication, while keeping the card message ID for replies/reactions. Preserve suppression of actual redeliveries, not independent clicks. / 以事件身份去重，以卡片消息身份引用原卡片。
- **Confidence**: 0.99

### IF3-08 [P1] CardKit application errors are reported as success / CardKit 业务错误被当作成功

- **Location**: `src/gateway/interfaces/feishu/api.rs:508-511,553-556` (`update_streaming_card`, `close_streaming_card`)
- **Failure mode**: Feishu returns HTTP 200 with a nonzero Lark `code`, e.g. a rejected sequence, permission error, or application-level throttle → both methods return `Ok(())`. The emitter advances its throttle/closed bookkeeping although the user-visible card was not updated or closed. / HTTP 成功但业务失败时，卡片状态仍被记为成功。
- **Evidence**: `if !resp.status().is_success() {`; `return Err(format!("Update streaming card HTTP {}", resp.status()));`; `Ok(())`. The close method repeats the same status-only verdict; the existing `src/gateway/interfaces/feishu/envelope.rs:170-184` demonstrates the required body-code check.
- **Suggested fix**: Parse and check the common `code`/`msg` envelope for these calls, using the existing envelope machinery and retaining HTTP context. Preserve provider-admission fencing and CardKit sequence/UUID fields. / 复用现有 envelope 判据，而不是仅检查状态行。
- **Confidence**: 0.99

### IF3-09 [P1] Failed final card delivery is permanently settled / 最终卡片发送失败仍永久结算

- **Location**: `src/gateway/interfaces/feishu/feishu_outbound/streaming.rs:86-109,274-306` (`FeishuStreamingCard::close`, `FeishuEventEmitter::emit`)
- **Failure mode**: The final update fails at the HTTP/network layer after some earlier chunks were shown → `closed` was already set, the failure is only logged, and closing the card is attempted anyway. `RunComplete` skips retrying a closed card and skips the inner text-delivery path, leaving the final answer truncated while emit returns success. This remains broken even after application-code checking is fixed. / 最后一帧失败后卡片仍被标为完成，用户只看到截断答案。
- **Evidence**: `if self.closed.swap(true, Ordering::SeqCst) {`; `warn!("Failed to send final streaming card update: {e}");`; `if !card.is_closed() {` (RunComplete at line 291).
- **Suggested fix**: Return the final-delivery result, mark successful settlement only after the required operations succeed, and propagate an actionable error or use one explicit final-text fallback. Preserve serialization and avoid blindly replaying already delivered text. / 区分正在关闭与已成功交付，失败必须可见且不能无条件重复发送。
- **Confidence**: 1.0

### IF3-10 [P1] Switching back to cards loses the fallback prefix / 回退后重新建卡丢失答案前缀

- **Location**: `src/gateway/interfaces/feishu/feishu_outbound/streaming.rs:258-270,288-306` (`FeishuEventEmitter::emit`)
- **Failure mode**: Card creation fails for a first non-final chunk `A`, then succeeds for chunk `B` → `A` was sent only to the inner emitter, but the newly created card accumulates only `B` onward. RunComplete takes the card branch, never flushes the inner answer buffer, and never reconciles the complete response. In non-streaming inner mode `A` is lost entirely; otherwise the answer is split across abandoned fallback and card state. / 瞬时建卡失败后恢复成功，会遗失此前进入回退缓冲区的前缀。
- **Evidence**: `if let Some(new_card) = self.create_card().await {`; `return self.inner.emit(event).await;`; `card.update(&self.api, delta).await;`. Cross-module confirmation: `src/gateway/reply_emitter/emitter/streaming.rs:373-381` buffers non-final deltas and only streams immediately when its separate stream setting is enabled.
- **Suggested fix**: Once a run falls back, keep the entire run on that existing emitter; alternatively transfer all accumulated answer state when switching modes. A per-run fallback decision is smaller and preserves ordering without dual ownership. / 一次回退后固定该轮交付路径，避免两个缓冲区分裂事实源。
- **Confidence**: 0.99

### IF3-11 [P1] Image caption failure disappears from the send result / 图片附文失败被吞掉

- **Location**: `src/gateway/interfaces/feishu/feishu_outbound/sender.rs:85-92` (`FeishuSender::send_message`)
- **Failure mode**: Image delivery succeeds but its accompanying text fails, including a rate limit or permission failure → the error is discarded and the whole outbound message returns success. The caller cannot discover that the answer/caption was never delivered. / 图片已到达但附文失败时，整体仍报成功。
- **Evidence**: `if has_image && !message.text.is_empty() {`; `let _ = api.send_text(chat_id, &message.text, reply_to).await;`; `Ok(SendResult {`.
- **Suggested fix**: Surface partial delivery with the already-sent image ID and failed text leg, or send image and caption in one supported provider message. Do not map the caption failure to a blindly retryable whole-message error, which would duplicate the image. / 使部分失败可见，同时保留已发生副作用的边界。
- **Confidence**: 1.0

### IF3-12 [P1] Production WhatsApp sends discard every attachment / WhatsApp 生产出站丢弃所有附件

- **Location**: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs:234-261` (`RealWaRuntime::send_message`)
- **Failure mode**: Send text plus an image/document/audio attachment through `Channel::send` → only `msg.text` enters the WhatsApp protobuf; the send returns a message ID and the attachment is silently lost. Attachment-only messages likewise never serialize the requested media. / 通道宣称支持媒体，但成功结果实际只覆盖文字。
- **Evidence**: `text: Some(msg.text),`; `conversation: Some(msg.text),`; `Ok(MessageId::new(msg_id.message_id))`. `src/gateway/interfaces/whatsapp/mod.rs:519-522` directly invokes this runtime; its capabilities at lines 279–300 advertise attachments/images/audio/video. Neither media processor is called by this path.
- **Suggested fix**: Wire supported attachment encoding/upload into this send boundary. Until an attachment type is implemented, reject it explicitly before sending and advertise only real capabilities. Preserve text-only sends and do not report success for omitted payloads. / 未实现媒体必须明确拒绝，不能成功丢弃。
- **Confidence**: 1.0

### IF3-13 [P1] Inbound WhatsApp media becomes an empty prompt / WhatsApp 入站媒体变成空提示

- **Location**: `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs:22,34-45,66-74` (`map_event_to_inbound`, `extract_text`)
- **Failure mode**: Receive a photo with a caption, document, video, or voice note → `extract_text` recognizes neither its body nor caption, and mapping provides no attachment or raw payload. The message is nevertheless forwarded to policy/history as an empty user turn, so the agent cannot inspect or transcribe the media. / 媒体内容、说明文字和恢复所需的原始载荷同时丢失。
- **Evidence**: `attachments: vec![],`; `raw: None,`; `String::new()` (the unsupported-body fallback at line 73).
- **Suggested fix**: Map supported media and captions into the existing attachment contract with the download/reference data needed downstream. Explicitly surface unsupported kinds rather than enqueueing an empty accepted turn. Preserve sender/chat identity and bot-message filtering. / 接到既有附件契约；不支持的类型需可见失败。
- **Confidence**: 1.0

### IF3-14 [P1] Adapter authorization compares phone numbers with JIDs / 适配器重复授权时混用号码和 JID

- **Location**: `src/gateway/interfaces/whatsapp/wa_policy/dm_policy.rs:27-37` (`DmPolicyEngine::evaluate`); `src/gateway/interfaces/whatsapp/wa_policy/group_policy.rs:32-45` (`GroupPolicyEngine::evaluate`)
- **Failure mode**: Configure the documented E.164 allowlist value `+15551234567`; the same sender arrives as `15551234567@s.whatsapp.net` → DM Allowlist or group sender Allowlist blocks the authorized person before the core sees the message. The core's normalizing allowlist comparison cannot repair a pre-filter rejection. This is concrete R4 drift, not a complaint about adapter length or duplication alone. / 适配器的第二套比较规则先行拒绝了同一人的合法消息。
- **Evidence**: `let sender = msg.sender_id.as_str();`; `|| allowed.contains(&sender)`; `if group_allow.contains(&"*") || group_allow.contains(&sender) {`. Cross-module confirmation: `src/gateway/channel_policy.rs:285-295` documents E.164 phone allowlists; `src/gateway/inbound_router/permission.rs:157-175` already compares normalized identities. The mapper at line 23 supplies the upstream JID unchanged.
- **Suggested fix**: Reuse the owning authorization/identity comparison at this boundary instead of a second raw-string policy. Preserve wildcard semantics and keep group JIDs separate from user phone identities; never strip digits from arbitrary JID namespaces as a blanket fix. / 复用既有身份判据，避免以简陋数字清洗造成身份混淆。
- **Confidence**: 0.99

### IF3-15 [P1] Disconnected sends lose their retryable error kind / 断线发送丢失可重试错误类型

- **Location**: `src/gateway/interfaces/whatsapp/mod.rs:519-522` (`Channel for WhatsAppChannel::send`)
- **Failure mode**: Keep an initialized runtime while its network connection is down → the runtime returns `WaRuntimeError::NotConnected`, but the channel converts it to `ChannelError::Internal`. The canonical delivery queue classifies this as an ambiguous terminal failure instead of a duplicate-safe disconnected retry, so the reply is not queued for delivery after reconnect. / 断线期间的正常回信被误判成终止性失败。
- **Evidence**: `.map_err(|e| ChannelError::Internal(format!("send_message: {e}")))?;`. The runtime explicitly preserves `ChannelError::NotConnected(m) => WaRuntimeError::NotConnected(m)` at `src/gateway/interfaces/whatsapp/wa_runtime/client.rs:410-415`. Cross-module confirmation: `src/gateway/delivery_queue.rs:535-554` retries only NotConnected/RateLimited and treats Internal as ambiguous.
- **Suggested fix**: Preserve the runtime's typed NotConnected and SendFailed distinctions when mapping to ChannelError. Keep genuinely ambiguous failures non-retryable; change no queue policy. / 在类型转换处保留错误语义，不扩大队列重试权限。
- **Confidence**: 1.0

### IF3-16 [P1] Reconnection never restores the canonical channel status / 重连不恢复通道权威状态

- **Location**: `src/gateway/interfaces/whatsapp/mod.rs:430-436,663-693` (`PairingStateDriver::apply`, production event loop)
- **Failure mode**: A previously paired account connects using restored credentials, or reconnects after Disconnected → `Event::Connected` only changes the separate `connected` atomic. `PairingStateDriver` ignores that event, while `status()` reads only PairingState, leaving the live channel Initializing/Disconnected even though the runtime can send. A new PairSuccess is not required for a normal reconnect. / 已配对账号恢复连接后，外部状态仍停在初始化或断线。
- **Evidence**: `Event::Connected(_) => { connected.store(true, Ordering::SeqCst); }`; `Event::PairSuccess(p) => { *s = PairingState::Connected {`; `_ => {}` (driver fallback). `status()` at lines 327–329 derives solely from `pairing_state`.
- **Suggested fix**: Drive the canonical PairingState from normal Connected events as well as pairing events, retaining known device details. Remove or derive the redundant connectivity mirror rather than making consumers guess which state to trust. / 普通连接事件必须到达同一状态所有者。
- **Confidence**: 0.99

### IF3-17 [P1] Channel startup performs synchronous filesystem work on Tokio / 通道启动在 Tokio 中同步访问文件系统

- **Location**: `src/gateway/interfaces/whatsapp/wa_auth/vault_store.rs:33-35,136-141` (`WaAuthManager::new`, `db_path`)
- **Failure mode**: Start the production channel while its data/vault directory is on a slow filesystem → opening/preparing the vault and creating the auth directory block a runtime worker until filesystem I/O returns. Both are synchronous calls reached directly from async startup. Directory-creation errors are additionally discarded before a later opaque backend failure. / 慢磁盘上的普通启动可阻塞运行时线程，目录错误还被延迟隐藏。
- **Evidence**: `let vault = SecretVault::open_or_backup(path);`; `let _ = std::fs::create_dir_all(&base);`. Callers are `src/gateway/interfaces/whatsapp/mod.rs:376` inside async start and `src/gateway/interfaces/whatsapp/wa_runtime/client.rs:106` inside `start_inner`. Cross-module confirmation: `src/secrets/vault.rs:54-59` prepares the parent and synchronously reads the vault.
- **Suggested fix**: Execute existing synchronous vault initialization in a blocking worker, and use async directory creation or the same blocking preparation boundary with propagated errors. Preserve vault permissions/encryption and path derivation. / 保留现有安全属性，仅把阻塞准备工作移出异步线程。
- **Confidence**: 1.0

### IF3-18 [P1] Public async image preparation blocks its executor / 公开异步图片处理阻塞执行器

- **Location**: `src/gateway/interfaces/whatsapp/media.rs:59-80` (`MediaProcessor::process_image`, reached from `prepare_outbound`)
- **Failure mode**: Await the public `prepare_outbound` API with a path-backed PNG/JPEG/GIF on Tokio → synchronous file reading, image decoding, Lanczos resizing, and JPEG encoding execute without yielding on the caller's worker. A slow file or large image stalls unrelated tasks on that worker. No current production sender calls this processor; this is a public-API-only async contract defect, not evidence of current media delivery wiring. / 该公开 API 的 async 外观不会隔离同步读盘和计算；当前生产发送器未接入它。
- **Evidence**: `let img = image::open(path)?;`; `image::imageops::FilterType::Lanczos3,`; `img.write_to(&mut cursor, image::ImageFormat::Jpeg)?;`.
- **Suggested fix**: Run the read/decode/resize/encode operation in `spawn_blocking`, returning the same result and preserving existing format/size semantics. Do not add a new media pipeline merely to fix executor blocking. / 用阻塞任务承载现有处理，不额外重构媒体架构。
- **Confidence**: 1.0

### IF3-19 [P2] Per-group history is bounded but the group map is not / 单群历史有界但群集合无界

- **Location**: `src/gateway/interfaces/whatsapp/history_buffer.rs:71-84` (`GroupHistoryBuffer::add`)
- **Failure mode**: Over a long-lived session receive messages from continually new group JIDs → each creates a permanent HashMap entry; only each group's deque is limited. Mapping buffers messages before access-policy rejection, and `inject_context: false` does not prevent storage, so even blocked group traffic consumes retained state. No production caller clears or consumes this history map. / 新群标识持续累积，包括最终被策略拒绝的消息。
- **Evidence**: `.entry(message.conversation_id.clone())`; `.or_insert_with(VecDeque::new);`; `while queue.len() > self.config.limit { queue.pop_front(); }`. `src/gateway/interfaces/whatsapp/wa_inbound/mapper.rs:53` calls add before `src/gateway/interfaces/whatsapp/mod.rs:440` evaluates policy.
- **Suggested fix**: Add a global conversation-count/idle bound and avoid retaining history when disabled or rejected by the owning policy boundary. If history injection is not intended to be connected, remove the write path rather than keeping an inert unbounded cache. Preserve any explicitly intended context-window semantics. / 有界化真正增长的集合；不要为未接线的功能永久留存消息。
- **Confidence**: 0.99

### IF3-20 [P2] Detached lifecycle tasks can overwrite a restarted runtime / 脱离所有权的生命周期任务可覆盖重启实例

- **Location**: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs:134-192,199-223` (`RealWaRuntime::start_inner`, `shutdown_inner`); `src/gateway/interfaces/whatsapp/mod.rs:422-481,488-499` (channel event task and stop)
- **Failure mode**: Call runtime start, shutdown before asynchronous Bot build/publication finishes, then start again → shutdown returns without joining the old builder; that old task can later publish a client/handle or clear the replacement's shared handle/client/shutdown sender. Separately, channel stop can return while its detached event task is still awaiting an acknowledgment; its eventual exit writes PairingState::Idle over a new start. These schedules follow explicit awaits and shared Arcs, not an assumed lock race. / 旧任务在 stop 返回后仍可发布或清理共享状态，破坏新一轮启动。
- **Evidence**: `tokio::spawn(async move {` (line 134); `*guard = Some(bot.client());` (line 166); `*guard = None;` (line 190, clearing shared shutdown sender). The channel event task also executes `*state = PairingState::Idle;` at line 480 after its detached loop exits.
- **Suggested fix**: Retain and join/cancel the owned runtime supervisor and channel event-loop tasks before stop completes and before allowing replacement. Treat starting as owned lifecycle state, not merely `bot_handle.is_some()`. Preserve event ordering and clean shutdown without a second client registry. / 持有并等待生命周期任务，禁止旧代任务在新代启动后回写。
- **Confidence**: 0.99

### IF3-21 [P2] Receipt-map overflow erases messages about to be consumed / 回执映射溢出擦除即将处理的消息

- **Location**: `src/gateway/interfaces/whatsapp/wa_runtime/client.rs:496-511` (`handle_bot_event`)
- **Failure mode**: The receipt map has 9,999 entries and a Messages event contains two new messages → inserting the first fills the map, inserting the second clears the entire map, and only afterward is the event forwarded. Calling mark_read for the first message immediately upon consuming that same batch fails as unknown; this is not merely eviction of sufficiently old history. / 同一新事件中的首条消息在消费者见到之前就失去回执目标。
- **Evidence**: `if guard.len() >= 10_000 { guard.clear(); }`; `guard.insert(info.id.to_string(), info.source.chat.clone());`; `if event_tx.send(event).await.is_err() {`. `mark_read` at lines 299–303 requires the map entry and otherwise returns `Unknown message ID for read receipt`.
- **Suggested fix**: Use bounded incremental eviction that retains the newest receipt targets instead of clearing every entry. Preserve the memory cap and do not claim unlimited historical receipt support. / 保留最近消息的有界淘汰，不作全表清空。
- **Confidence**: 1.0

## Clean files

以下文件已完整阅读，未确认独立、具体的缺陷；不代表经编译或真机验证。Intentional bounded backpressure, short synchronous mutex critical sections, test-only panics, derived provider publications, and correctly ordered dedup TTL entries were not reported as defects.

- `src/gateway/interfaces/feishu/config.rs`
- `src/gateway/interfaces/whatsapp/pairing.rs`
- `src/gateway/interfaces/whatsapp/wa_runtime/fake.rs`
- `src/gateway/interfaces/feishu/types.rs`
- `src/gateway/interfaces/feishu/envelope.rs`
- `src/gateway/interfaces/feishu/api_handle.rs`
- `src/gateway/interfaces/whatsapp/wa_outbound/media.rs`
- `src/gateway/interfaces/feishu/feishu_runtime/mod.rs`
- `src/gateway/interfaces/whatsapp/reactions.rs`
- `src/gateway/interfaces/feishu/auth.rs`
- `src/gateway/interfaces/feishu/feishu_inbound/crypto.rs`
- `src/gateway/interfaces/whatsapp/wa_runtime/traits.rs`
- `src/gateway/interfaces/feishu/feishu_runtime/ws_client.rs`
- `src/gateway/interfaces/feishu/feishu_inbound/policy.rs`
- `src/gateway/interfaces/feishu/feishu_inbound/user_cache.rs`
- `src/gateway/interfaces/feishu/feishu_inbound/dedup.rs`
- `src/gateway/interfaces/whatsapp/wa_inbound/policy.rs`
- `src/gateway/interfaces/feishu/feishu_policy/dm_policy.rs`
- `src/gateway/interfaces/whatsapp/wa_outbound/sender.rs`
- `src/gateway/interfaces/whatsapp/wa_policy/pairing.rs`
- `src/gateway/interfaces/whatsapp/message.rs`
- `src/gateway/interfaces/whatsapp/wa_runtime/http_client.rs`
- `src/gateway/interfaces/whatsapp/wa_runtime/state.rs`
- `src/gateway/interfaces/feishu/message_ops.rs`
- `src/gateway/interfaces/feishu/feishu_runtime/state.rs`
- `src/gateway/interfaces/whatsapp/types.rs`
- `src/gateway/interfaces/feishu/feishu_outbound/reactions.rs`
- `src/gateway/interfaces/feishu/feishu_outbound/media.rs`
- `src/gateway/interfaces/feishu/feishu_inbound/mod.rs`
- `src/gateway/interfaces/whatsapp/wa_runtime/mod.rs`
- `src/gateway/interfaces/feishu/feishu_outbound/mod.rs`
- `src/gateway/interfaces/whatsapp/wa_policy/mod.rs`
- `src/gateway/interfaces/feishu/feishu_policy/mod.rs`
- `src/gateway/interfaces/whatsapp/wa_inbound/mod.rs`
- `src/gateway/interfaces/whatsapp/wa_outbound/mod.rs`
- `src/gateway/interfaces/whatsapp/wa_runtime/event_loop.rs`
- `src/gateway/interfaces/whatsapp/wa_auth/mod.rs`
