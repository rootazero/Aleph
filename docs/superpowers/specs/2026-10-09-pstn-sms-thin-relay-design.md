# SMS 与 PSTN 个人助理通信设计 / SMS & PSTN Personal Assistant Design

- Date / 日期：2026-10-09
- Status / 状态：**待用户审阅的条件性设计；不是开发启动或线路可用性承诺。** / Conditional design for human review; not implementation authorization or a carrier-availability claim.
- Scope / 范围：Aleph Gateway 接入原生 SMS 和普通手机电话，平台只托管通信连接；不开发第二个云端 Agent。 / Native SMS and PSTN access through a hosted transport layer, without a second agent brain.

## 1. 设计简报 / Agreed design brief

### 已确认的需求 / Confirmed requirements

| 维度 / Dimension | 要求 / Requirement |
|---|---|
| 地区 / Regions | 目标覆盖中国大陆 +86 与海外；首发海外国家清单、线路和能力必须在 G3 中逐国选择并核验，当前不宣称任何海外国家已支持。 / Mainland China and overseas are goals; each first-launch country must be explicitly selected and verified under G3, with no overseas country currently claimed as supported. |
| SMS | 用户主动发送、Aleph 回复及用户授权后的主动发送，支持开放式文字对话。 / User-initiated messages, replies and authorized proactive contact using conversational text. |
| 电话 / Calls | 呼入、授权呼出和实时双向语音。 / Inbound calls, authorized outbound calls and real-time two-way speech. |
| 用户配置 / Setup | 用户绑定并验证手机号、授权通信，不自行申请线路、供应商账户、公网地址或 webhook。 / Phone enrollment and consent only; no user-managed carrier accounts or public endpoints. |
| 托管边界 / Hosting | 平台提供号码、线路与薄通信连接；身份、记忆、推理、工具及审批由用户自己的 Aleph Core 负责。 / Hosted numbers and transport; identity, memory, reasoning, tools and approvals remain with the user's Core. |
| 号码体验 / Addresses | SMS 地址和电话号允许不同，但映射到同一个 Aleph 身份。 / Separate SMS and voice addresses may map to the same identity. |

“零配置”不等于无注册、无授权、无运营成本或 Core 无需在线。仅知道手机号也不等于获得联系许可或可靠身份认证。 / Zero carrier configuration does not eliminate enrollment, consent, operating costs or Core availability; a phone number is not an authorization credential.

### 本文提出、尚待审阅的选择 / Choices proposed for review

1. 托管转写/语音合成 + Core 文本流作为首选电话路径；原始音频桥作为备选，不把音频直连到另一套 Realtime Agent。 / Prefer hosted speech I/O with Core-generated text; raw audio is a fallback transport, not an independent realtime agent.
2. Core 主动建立认证连接，不开放个人设备公网入口。 / Core initiates the authenticated transport connection.
3. 首版一个已验证手机号只绑定一个主 Core；多 Core 选择不通过猜测实现。 / One primary Core per enrolled phone in v1; no ambiguous routing.
4. 首版只允许向已验证且授权的本人号码主动联系；第三方联系人、群发、紧急呼叫和营销外呼不在范围内。 / Proactive contact is limited to the enrolled owner; third-party contact, broadcasting, emergency services and marketing calls are excluded.
5. 默认不录音；通话内容和长期记忆不在托管服务保存。 / No recording by default and no hosted long-term conversational memory.

这些是审阅对象，不是已经获批的实施参数。 / These proposals require review before implementation planning.

## 2. 能力证据与发布闸门 / Evidence and release gates

截至本次研究，**还不能承诺大陆开放式双向 SMS 可交付**。代码或开源网关能收发短信，不代表线路允许个人助理 AI 对话。 / Open conversational SMS in mainland China remains unproven; software capability is not carrier permission.

| 路径 / Path | 已有证据 / Evidence | 不可据此推断 / Not established |
|---|---|---|
| 海外 SMS、电话 / Overseas | Twilio 的消息与电话接口、ConversationRelay/Media Streams 文档和公开样例可供参考。 / Public messaging and voice interfaces and examples exist. | 每个国家的双向号码资源、AI 用途准入、注册要求和可达性。 / Per-country eligibility, number availability and reachability. |
| 大陆电话 / Mainland voice | 阿里云 AICCS 支持自有后端 HTTP/SSE，含 `session_id`、`messages`、`biz_params`、`out_id`，返回 `choices[].delta.content`；大模型应用有呼入配置。 / A documented external-backend streaming interface exists. | 个人助理用途许可、线路开户、真实打断/取消、请求重放身份及尾延迟。 / Use-case approval, provisioning, interruption, retry identity and latency. |
| 大陆普通云短信 / Mainland cloud SMS | 阿里云可接收上行回复，但下行号码不能指定；创蓝公开资料仍以签名/模板及上行回复为主。 / Replies exist; public products remain template-oriented, and Aliyun documents non-selectable sender numbers. | 任意 AI 文本、稳定可主动发起的接收地址及长期连续聊天。 / Arbitrary generated text, stable user-initiated addressing and persistent conversations. |
| SIM/Android 网关 / SIM gateways | 开源代码可提供 HTTP API 和 SMS 接收转发。 / Open-source software can bridge device SMS to APIs. | 商用线路许可、三网稳定性、扩容、免用户配置及托管运营合法性。 / Commercial permission, carrier reliability, scale and compliant hosted operation. |

### 必须关闭的闸门 / Mandatory gates

- **G1 大陆 SMS / Mainland SMS**：供应商针对“个人 AI 助理开放式对话”给出可存档的用途准入与接口答复；证明自由文本下行、用户无前置通知主动上行、稳定接收地址和可唯一关联的入站事件。模板中塞入整段 AI 回复不视为绕过限制的合法方案。 / Written eligibility plus free-text downlink, unsolicited user initiation, stable addressing and reliable inbound identity.
- **G2 大陆电话 / Mainland voice**：确认入/出站号码、资质、外部 Core 接入；实测打断、取消、请求重放、超时和已播放内容反馈。 / Provisioning and real call behavior must be validated.
- **G3 海外范围 / Overseas scope**：用户选择首发国家清单后，逐国核验 SMS 双向、电话双向、用途与号码注册。未列入清单的地区不开户、不宣称支持。 / A reviewed country allowlist is required before provisioning.
- **G4 运营与隐私 / Operations and privacy**：确认线路费用、平台资质、地区数据路径、供应商留存/训练政策、用户同意和撤销机制。涉及跨境路径时单独审批，不自动跨区回退。 / Review costs, eligibility, data flows and consent; no automatic cross-region fallback.
- **G5 接入安全 / Security**：手机号绑定、租户隔离、Core 密钥吊销、发送审批及回调防伪/防重放验证通过。 / Enrollment, isolation, revocation, approvals and callback authenticity must pass.

完整首版只有在 G1–G5 全部通过且第 10 节双地区效果验证通过时才能发布；若大陆线路未过 G1/G2，海外线路也不自动先发，因为“首版两地区同功能”是本设计的整体验收约束。若用户后续明确选择分阶段发布，必须重新记录范围与验收标准。允许先进行隔离技术验证，但不能降级需求后宣称完整功能已完成。 / A full release requires all gates and both-region acceptance tests; neither region silently becomes a substitute for the other.

## 3. 方案选择 / Alternatives and decision

| 方案 / Option | 收益 / Benefits | 代价 / Costs | 决策 / Decision |
|---|---|---|---|
| 薄托管连接 + 用户 Core / Thin hosted relay | 保留一个个人大脑，用户无需电信配置；地区供应商可独立替换。 / One brain and replaceable regional carriers. | 平台承担号码/资质/费用与可用性；多一段网络；Core 离线影响服务。 / Operations burden, extra hop and Core availability dependency. | **采用，符合已确认边界。 / Selected.** |
| 通信与 Agent 全托管 / Fully hosted agent | 始终在线较容易实现。 / Easier always-on availability. | 改变数据与部署主权，形成第二套大脑或迁移 Core。 / Changes ownership and deployment boundaries. | 不采用。 / Rejected. |
| 用户自备 SIM/供应商 / User-managed telecom | 平台运营较轻，可作个人实验。 / Lighter platform operations. | 用户要准备硬件、线路和回调，违背免配置目标。 / Violates the setup requirement. | 仅作参考/实验，不计入产品交付。 / Reference only. |

## 4. 架构与责任 / Architecture and responsibilities

```text
Native SMS / PSTN phone
           ↕
Regional carrier / speech transport
           ↕ authenticated HTTP, WS or SSE
Hosted Telecom Relay (regional deployment)
  number bindings · carrier verification · bounded queue · status · quota
           ↕ outbound authenticated connection initiated by Core
Aleph transport adapter / Gateway I/O
           ↕ normalized messages, call turns and output events
Existing router → identity/permission → session → Agent/tools/approval
           ↕
User-owned Core memory and execution
```

| 单元 / Component | 负责 / Owns | 明确不负责 / Does not own |
|---|---|---|
| 区域供应商 / Regional carrier | PSTN/SMS、音频或转写/合成、线路回执。 / Telecom and speech transport. | Aleph 任务推理和工具执行。 / Agent reasoning and tools. |
| 托管 Relay / Hosted relay | 供应商鉴权、号码/租户映射、协议转换、连接租约、限额、短时消息与可观测状态；托管控制面保存租户、手机号绑定、Core 设备密钥状态和计费主体。 / Authenticated routing and bounded transport; the hosted control plane owns tenant, binding, device-key and billing records. | 记忆检索、意图判断、任务规划、语义摘要、决定是否改发短信、代替离线 Core 回答；不向 Core 下发供应商主账号密钥。 / No semantic planning, alternate brain or carrier master credentials. |
| Core 接入适配 / Core adapter | 认证连接、去重、规范化 I/O、传输状态及电话播放/取消映射。 / Normalized I/O and transport lifecycle. | 自建业务路由规则或绕过现有权限链。 / No independent business router. |
| 既有 Core / Existing Core | 身份策略、会话/记忆、模型推理、工具审批与主动联系意图。 / Identity policy, memory, reasoning and approvals. | 原生设备驱动、运营商媒体栈。 / Device drivers and carrier media stacks. |

遵守 R3/R4/R6/R7/R8/R9/R10：适配层不把“短信短就走某工具”“用户着急就打电话”编码成规则，不扩大 harness 以容纳通信业务；SIP/RTP/WebRTC 等重媒体栈留在外部连接器，不引入 Rust Core；绑定、解绑、预算与发送能力必须最终暴露为现有工具，而不是藏在 Interface 层。固定的鉴权、预算闸门和传输状态机不替代 LLM 意图推理。 / Respect R3/R4/R6/R7/R8/R9/R10: heavy media stacks stay outside Core, and transport controls remain tools rather than Interface business logic.

R1 的原生 OS/设备操作必须走 Bridge IPC；公网通信供应商 API 位于 I/O 适配边界，不因此强制绕桌面 Bridge。若未来采用本地 modem/SIM 驱动，放在 Bridge/独立连接器，而非 Core。 / Native device APIs require Bridge IPC; carrier HTTP belongs at the transport boundary.

### 对现有代码的接入 / Existing integration seams

- `src/gateway/channel.rs`：复用 `Channel`、`InboundMessage`、`OutboundMessage`、`ChannelCapabilities`。SMS 声明不支持 typing、附件及按钮，不能假成功。 / Reuse channel contracts with honest capabilities.
- `src/gateway/inbound_router/mod.rs`、`src/gateway/inbound_router/permission.rs`、`src/gateway/pairing_store.rs`：复用现有路由与配对，但补充电话通道的绑定与身份强度，不把号码 allowlist 当作强认证。 / Preserve routing and pairing while distinguishing authentication strength.
- `src/gateway/webhook_receiver.rs`：现有 `WebhookHandler::verify` 参数不含外部 URL，不能未经核查当作 Twilio 等 URL 相关签名的完整实现。托管回调按供应商协议验签，Core 主动连接路径不要求公网 webhook。 / Provider verification must include the required request context.
- `src/gateway/voice/streaming/mod.rs`：可参考 provider-neutral 转写语义，但现有 PCM 合约是 16 kHz mono s16le；电话媒体不得直接假装兼容，需在媒体边界明确解码/重采样。 / Telephone codecs require explicit conversion.
- `src/gateway/voice/inbound/mod.rs`、`src/gateway/voice/outbound.rs`：现有语音附件 STT/TTS 不等于 PSTN 全双工。 / Voice-note processing is not a live call stack.
- `src/builtin_tools/channel_message.rs`：SMS 回复/主动发送复用消息发送语义，通话需独立生命周期操作与审批能力。 / Reuse messaging semantics, not an invented parallel sender.
- `plugins/voice-call/README.md`：目前明确为 WebRTC/SIP stubs，不计作现成可工作的电话实现；不平行激活两个 call manager。 / The stub plugin is not a functioning PSTN backend.

这些是设计接缝，不是本轮已修改的文件清单。具体模块划分和注册清单留到获批后的实施计划。 / These are integration seams, not changes made in this round.

## 5. 寻址、身份与同意 / Addressing, identity and consent

1. 手机号按 E.164 规范化；国际地区、服务地址和供应商账号作为路由事实记录，不根据语言推断地区。 / Use explicit normalized routing facts.
2. 平台使用 `(region, service_address, verified_phone)` 唯一定位绑定；服务号码可共享，但首版同一手机号只对应一个主 Core。匿名/隐号/未知号码不得进入私人 Agent 会话。 / Resolve bindings uniquely; do not guess the tenant.
3. SMS 地址和电话号是同一绑定的两个别名，供应商号码不是 Aleph 身份。绑定撤销或重分配使旧地址/连接租约失效。 / Addresses are aliases, not identity.
4. 初次绑定在已登录控制面发起 OTP 验证，保存范围明确的通信同意；换号、解绑、设备密钥轮换必须可通过工具与控制面操作。 / Enrollment requires authenticated consent and revocable credentials.
5. **来电显示和 SMS 发件地址仅是低保证来源标识**：可能存在伪造、SIM swap、号码回收或转接。绑定只用于找到候选 Core；未增强验证的会话使用最低权限上下文，在记忆检索前就阻断私人上下文注入，而不是先喂给模型再希望它不透露。敏感记忆披露、财务操作、密码/密钥和高风险工具，必须走既有可信审批面/增强验证，普通电话中一句“同意”不得替代强审批。SMS/电话默认只获得现有 `Chat`/guest 级别；需要提升权限时，审批必须路由到已认证控制面或既有可验证的可信通道，找不到可达审批通道就拒绝，不以语音或短信中的一句“同意”自批准。 / Caller ID selects a candidate Core, not private context access; gate memory injection before reasoning, default to Chat/guest, and require a separately routed trusted step-up approval.
6. 入站同意不自动包含主动外呼/主动短信同意；两种主动联系权限可分别撤销。短信即时回复可以使用绑定时明确授权的“回复本人”范围，但这只是已授权的低风险传输回复，不提升 `Chat`/guest 权限，也不允许披露私人记忆或执行高风险工具；主动发送/外呼使用单次或用户明确配置的有限授权；不能把通道自动回复和工具主动发送混成一项永久许可。通信撤销优先于队列投递及外呼提交。电话或短信会话等待审批时，审批有效期内若通话被打断、挂断、Core 断线或范围变化，待审批动作立即取消，不在重连后自动恢复。 / Distinguish scoped low-risk replies from proactive authorization; a reply never elevates Chat/guest privileges, and interrupted or disconnected approvals are cancelled, never resumed automatically.
7. Core 不在线时，未知号码不触发计费短信自动回复。撤销联系、解绑和挂断是可达的停止能力，不因为缺少再次审批而失效。 / Off-switches must remain reachable.

号码回收无法仅靠一次 OTP 永久检测：长期停用后重新验证；发现号码更换信号就暂停敏感能力并提示本人重新绑定。确切复验周期须由安全/运营审阅确定，未确定前不把一次验证视为永久强身份。 / Periodic verification is an operational policy; enrollment alone never grants permanent strong authentication.

## 6. 传输契约与会话 / Transport contracts and sessions

### 共享逻辑契约 / Shared logical contract

两端使用一份 versioned wire schema，实施时生成/共享类型；不得分别手写互相抵消的两份 JSON 形状。以下字段是设计契约，不是已有 Aleph API。 / Use one versioned schema shared by both ends; these fields are proposed, not existing APIs.

| 字段组 / Group | 字段及语义 / Fields and semantics |
|---|---|
| 信封 / Envelope | `protocol_version`, `binding_id`, `connection_generation`, `event_id`, `kind`, `created_at`, `expires_at`。身份取自已认证连接，消息自报的绑定只能一致性校验。 / Authenticate identity from the connection, not untrusted payload. |
| SMS | `provider_account_id`, `provider_message_id`, `service_address`, `peer_address`, `body`, `encoding`, `segment_count`；回执关联原始发送操作。 / Correlate receipts to the original operation. |
| 电话 / Call | `phone_call_id`, `provider_call_id`, `turn_id`, `output_epoch`, `output_seq`；`connection_generation` 按一次 Core 连接租约递增，而不是按绑定递增；取消与播放反馈必须关联到具体输出 epoch。 / Scope playback and cancellation to output epochs; distinguish phone-call identity from tool-call IDs, and fence each Core connection lease. |
| 命令 / Command | `operation_id`, `approval_reference`, `recipient_binding_id`, `expires_at`；审批真实性、语义和动作绑定由 Core 负责；Relay 不解释审批引用，只核对已绑定本人号码、有效期、撤销状态、generation 与确定性预算，并把引用原样留作审计。 / Core owns approval meaning; Relay enforces only transportable recipient, expiry, revocation, generation and budget checks. |
| 结果 / Outcome | 接受、已提交、已送达/已接通、已播放、失败、过期、取消和结果未知互不混淆；携带可核查的供应商关联 ID。 / Accepted is not delivered; unknown is not failure. |

- Relay↔Core 使用认证的出站 WSS/等价双向连接，带租户隔离、心跳、重连及单活 generation。旧 generation 不得继续提交发送/播放。 / Fence stale sessions after reconnect.
- SMS 采用持久化后 ACK、至少一次投递和接收端去重；供应商回调先完成验证与安全落盘，再快速 ACK，不能等待 LLM。阿里云 FAQ 的 700 ms 回调要求只约束该供应商。 / Decouple callback acknowledgment from reasoning.
- 去重键包含供应商账号、事件类型与供应商事件 ID；同一短信的提交/送达/失败是不同状态事件，不能仅凭 `provider_message_id` 丢掉后续回执。不能用正文哈希去重，否则用户连续发两条相同短信会丢失。Relay 持有去重键直到供应商声明的最大重试窗口结束；窗口外迟到回调只记录为 stale/unmatched，不改变已关闭结果。缺少可靠事件身份的供应商必须先解决契约，不伪造 exactly-once 承诺。 / Distinguish receipt transitions, preserve repeated content, and define Relay ownership of the dedup horizon.
- 发送请求超时但已可能被供应商接受时进入 `unknown`，查询状态或等待回执，不能盲目换供应商重复发送/拨号。`unknown` 具有有限保留期限；期限结束后转为 `outcome_unknown_expired`，不会自动重发，重发必须产生新的 `operation_id` 并重新通过相应审批。 / Unknown outcomes expire into an explicit terminal state; retry is a new approved operation.
- 区域能力来自已验证的配置/线路事实，禁用能力明确返回 unsupported，不猜测替代方式。 / No implicit degradation of promised capabilities.

### SMS 与电话会话 / SMS and voice sessions

- SMS 以绑定和对端会话为基本连续线程；入站事件的时间顺序、长短信拼接与重复投递由适配层明确处理。 / Preserve message identity and ordering.
- 每次电话有独立 `call_id` 和 turn 序列，绑定到同一主体，可访问经授权的 Core 记忆；不把原始 transcript 自动灌进另一条活跃 SMS 会话造成并发污染。 / Share identity, not mutable transport history.
- 入站完整转写作为 user turn；interim 只用于界面/传输反馈，不触发重复工具执行。 / Only committed utterances start agent turns.
- 供应商携带的历史 `messages` 是传输上下文，不是权威 Core 记忆或指令。重放同一 turn 只能重播已有安全结果/返回状态，不再次执行外部工具。 / Provider history is untrusted context, not authoritative state.
- AICCS 的公开请求协议未证明存在足够的 turn/retry 唯一标识；必须向供应商验证，不能假设 `session_id` 就能区分一通电话里的每一轮。 / Call identity is not turn identity.

## 7. 电话流式交互与打断 / Streaming calls and interruption

首选路径：供应商负责 ASR/TTS、VAD 与 PSTN；Relay 只把完整转写和 Core 文本流互转。Core 仍执行完整 Agent，而不是只能调用一个无工具的 chat completion。 / The carrier owns speech transport while Core retains the complete agent.

呼入流程：验签/鉴权 → 解析服务号码与绑定 → 检查 Core 在线和权限 → 建立 call session → 完整转写进 Core → 输出文本分块 → 供应商播放 → 回传已播放范围。呼出流程在以上步骤前增加本人号码授权、预算校验及外呼提交。 / Admission precedes reasoning, and proactive calls require additional authorization.

必须区分三种状态：**已生成、已发送到播放队列、用户实际听到**。Core 不得将整段尚未播放内容记为已经告诉用户。 / Generated, queued and heard content are different states.

打断契约：
1. 用户开始新一轮讲话时关闭旧 `output_epoch`，请求清空供应商播放队列，取消旧 Core 生成任务。
2. 对被取消 epoch 的迟到文本/音频直接丢弃；只有新 epoch 可以继续播放。
3. 根据已播放 ACK 或可靠供应商反馈截断助理已说内容。供应商不能提供精确范围时标记不确定，不谎报精确播放。
4. 已发生的工具副作用不能靠取消生成回滚；它们保留在 Core 的执行账本，新 turn 能看见实际结果。
5. 若供应商不能可靠停止旧音频或取消仍会持续计费的会话，G2 不通过；不以“发出了取消请求”作为通过证据。

Interruption must stop actual playback, fence late output and preserve already-executed tool effects. A cancellation request alone is not evidence of interruption.

原始媒体备选（Twilio Media Streams/SIP）需显式验证双向音轨、μ-law/PCM/采样率、抖动、背压及 `clear`/播放反馈。只把 `StreamingTranscriber` 接上不能完成 TTS 与电话控制。重型 SIP/RTP/WebRTC 栈留在外部连接器，不引入 Rust Core。 / Raw media needs a complete transport pipeline outside the Core, not just transcription.

OpenAI Realtime 参考仓只借鉴媒体生命周期和打断：不照搬其模型作为独立大脑。若将来采用 speech-to-speech 模型，必须先独立设计其与 Core 工具/记忆的唯一控制关系。 / Realtime examples are transport references only.

## 8. 离线、失败与运营边界 / Offline behavior and operations

- Core 离线：SMS 可以在告知的期限内排队；电话不排队等待重连，不由备用 Agent 接管。固定不可用提示属于传输状态告知，不访问记忆或生成语义回答。 / Offline SMS may queue; calls do not acquire a substitute brain.
- SMS/电话触发的高风险工具审批必须路由至已认证控制面或既有可验证的可信通道；若当前会话没有可达审批通道则 fail-closed，不提供“在短信/通话里自己批准”的后门。等待中的审批绑定 `phone_call_id`/turn；挂断、打断、连接代次变化、Core 断线或审批过期都会取消它，重连不自动恢复。 / Step-up approval needs a reachable trusted channel; otherwise deny and cancel approvals on call/session loss.
- 通话中断连：停止输出、释放线路，Core 记录 interrupted 状态；重连后不自动重拨。是否改发 SMS 或再次联系由 Core 在用户许可下决定。 / Recovery contact decisions remain with Core.
- 不支持送达回执时显示 unknown/accepted，不显示“用户已收到”；电话接通不等于接收人身份被确认或回复被完整听到。 / Observe effects honestly.
- 限制号码租金、短信分段、并发电话、每通时长和日预算；预算是确定性 admission，不替 Agent 决定联系理由。供应商实际账单用于核对本地预算偏差。 / Enforce mechanical cost limits without intent inference.
- UCS-2/Unicode 与长短信按供应商实际分段计费；超出批准预算就返回明确错误给 Core，由 Core 决定缩短或询问，不静默截断语义。 / No hidden semantic truncation.
- 一次性外部动作在提交前重新验证绑定、同意、审批有效期与 generation，关闭“审批后撤销仍发出”的窗口。 / Revalidate at the irreversible boundary.

### 建议初始运行参数 / Proposed initial defaults

这些参数用于审阅和试验，不是已测性能或运营承诺。 / Defaults for review, not measured guarantees.

| 参数 / Parameter | 建议值 / Proposal |
|---|---|
| Core 离线入站 SMS 队列 TTL / Offline inbound TTL | 24 小时；只有 Core 重连时仍未过期的事件才进入 Agent；过期事件转为 `expired_before_core`，不产生语义回复。默认不额外发送一条收费 SMS；控制面显示可审计的过期状态，若供应商支持则返回传输层失败/过期回执。 / Only unexpired events reach the Agent; expiry is explicit and does not create a hidden paid reply. |
| 待提交主动短信授权有效期 / Outbound authorization expiry | 必须由 Core 明确填写且至多 5 分钟；过期重新审批。 / Explicit, at most 5 minutes. |
| 待提交外呼授权有效期 / Call authorization expiry | 至多 60 秒；离线不排队；过期、挂断、打断或 Core 断线都转为 cancelled/expired，重新外呼必须重新审批。 / At most 60 seconds, never queued offline; disconnects cancel it and require fresh approval. |
| 传输元数据去重窗口 / Metadata deduplication | Relay 初始保存 7 天且不保存正文；必须覆盖供应商最大重试窗口。超过 7 天的供应商在批准更长窗口前不启用；窗口过后迟到回调只记 stale/unmatched。 / Relay initially retains metadata for 7 days without content; late callbacks after the approved horizon are non-mutating. |
| `unknown` 结果保留 / Unknown outcome retention | 按供应商查询/回执 SLA 设定且必须有上限；到期转为 `outcome_unknown_expired`，不自动重发，任何重发均需新的审批。 / Bound unknown outcomes and never retry them implicitly. |
| 电话服务体验试验目标 / Voice probe targets | 当前作为发布门槛提案：用户话音结束至首个可听输出 p95 ≤ 2 秒；打断至旧音频停止 p95 ≤ 500 ms。若目标不现实，须在发布前由用户明确降为监测项，不能由实现者静默降低。必须真机测量，不只统计 SSE 首 token。 / Proposed release gates; any relaxation requires an explicit product decision. |

各供应商的硬超时、队列容量、号码价格和总预算必须取得实际合同/测试数据再配置；没有数据就不启用该线路，不能使用一个统一猜测值上线。 / Missing operational evidence disables a route rather than inviting guessed production settings.

## 9. 安全、隐私与区域数据路径 / Security, privacy and regional data paths

- 供应商回调按其完整协议验证：原始请求字节、外部 URL/表单参数（如要求）、时间戳/nonce（如提供）、账号与线路；不能把通用 HMAC-SHA256 当成所有供应商的验签协议。可信代理的 URL 重构需固定配置，不能相信任意转发头。 / Implement provider-specific request authentication.
- 不支持签名的供应商需要等价的可验证来源机制与重放控制；不能因文档要求“200 ACK”就免掉安全校验。 / Fast acknowledgment never removes admission checks.
- Core 连接通过控制面注册与短期凭据/设备密钥鉴权，凭据可轮换/吊销，不把供应商主账号密钥下发到用户 Core。托管控制面是租户、手机号绑定、设备密钥状态和计费主体的权威存储；Relay 代码属于独立托管部署单元，通过 versioned wire schema 与 Core 适配器连接。 / The control plane is authoritative for tenant/binding/device-key/billing records; Relay is a separate hosted deployment unit.
- 队列正文静态加密、传输 TLS、消费/过期删除；运维日志默认只记关联 ID、脱敏号码、错误码、时长与费用，不记正文/完整 transcript。 / Bound content retention and minimize logs.
- Relay 会在转发时接触明文，电信及云 ASR/TTS 供应商也可能接触内容；**普通 SMS/PSTN 不承诺端到端保密或平台零可见**。 / Do not claim E2E confidentiality.
- 提示用户电话使用 AI；录音另需明确同意与地区审查。本设计不启用录音，供应商侧留存仍需单独查明。 / AI disclosure and recording are separate consent requirements.
- 境内与海外 Relay/供应商按区域隔离部署和账号；国内电话到境外 Core/模型的路径也可能构成跨境，不因 Relay 在境内就声称数据不出境。 / Trace the complete path, including user-owned Core and model endpoints.
- 来自 SMS/语音的文本均按不可信输入处理，不能凭声称“管理员”突破既有权限和工具准入。 / Treat transport input as untrusted.

## 10. 验证矩阵与效果验收 / Verification and effect-based acceptance

测试证明效果到达，不以 HTTP 200、Connected 标记或函数调用代替用户实际收发。真实通信试验需要线路与参与者授权，不能在本次文档工作中擅自拨号。 / Verify actual delivery with separately authorized real-line tests.

| 场景 / Scenario | 通过证据 / Required evidence |
|---|---|
| 大陆/每个首发海外地区 SMS 主动上行 | 用户未先收到通知也能向约定地址发普通 SMS，正确 Core 收到且仅处理一次。 / Unprompted native SMS reaches the correct Core once. |
| SMS 回复与主动触达 | 开放式 AI 文本在目标手机出现，正文/编码/分段与回执对应；拒收撤销后不再提交。 / Received text matches the authorized operation. |
| 电话呼入/呼出 | 两个方向真机接通、双向可听、关联同一用户 Core；拒绝未授权号码。 / Audible two-way calls in both directions. |
| 打断与迟到输出 | 讲话打断停止旧音频；旧 epoch 的迟到输出不播放；状态不包含未听到内容。 / Playback truly stops and state remains truthful. |
| 重试/重放 | 重复 webhook、SSE重试、断连重发不重复发送/拨号/执行工具；同正文新 SMS 不丢失。 / Retry safety without content-based loss. |
| 接受状态未知 | 提交后网络中断，系统查询/等待结果而不盲目重发；UI/工具显示 unknown。 / Honest ambiguous outcomes. |
| 离线与 TTL | SMS 重连恢复仅在 TTL 内发生；电话没有隐形云 Agent，恢复不自动重拨。 / Offline behavior respects the agreed boundary. |
| 租户/绑定隔离 | 伪造 binding、旧 generation、服务号码重分配、同号码多 Core、隐号及未知号码均不串租户。 / No cross-tenant or stale-binding access. |
| 认证与审批 | 伪造来电显示不能披露敏感记忆；短信/电话不能绕过工具审批；解绑立即失效。 / Sensitive access requires appropriate assurance. |
| 成本与边界 | 长中文短信、并发电话、费用上限、退订、超时、背压和 cancel 的停止方向均有效。 / Limits apply at the actual effect boundary. |
| 地区证据 | 大陆至少覆盖移动/联通/电信的目标场景；海外按获批国家/线路逐项留证。 / Region-specific evidence, not one demo for all. |

实施后按实际模块运行 targeted tests、`cargo check`、适用 clippy 与项目最小可信验证集；本次只写设计，未运行 cargo，不宣称实现测试通过。 / Implementation validation is future work, not completed evidence.

## 11. 供应商能力询证清单 / Vendor qualification questionnaire

询证用途：个人用户自己的 AI 助理；平台持有号码/线路，用户 Core 经托管接口实时生成回复；不是营销群发，不以模板通知冒充对话。询证可以先整理，发送/开户/签约需另行授权。 / Qualify the actual personal-agent use case; external commercial action requires separate authorization.

### 大陆 SMS 必答 / Required mainland SMS answers

1. 是否允许未经逐条预定义的 AI 生成回复？哪些签名、模板或内容限制仍适用？请对该用途书面确认。 / Is generated conversational text permitted for this use case?
2. 用户能否在没有近期下行消息的情况下主动发短信？接收地址是否稳定、能保存到联系人并长期使用？ / Can a user initiate and reuse a stable address?
3. 上行是否有稳定唯一 ID、收件服务地址、发件号码、时间、拼接段和重试语义？是否可提供安全鉴权？ / What is the authenticated inbound event contract?
4. 是否覆盖移动/联通/电信？跨网、漫游、长中文短信、号码更换如何处理？ / Which networks and edge cases are supported?
5. 平台需要哪些资质/报备，谁承担内容审核和退订责任？允许的本人主动联系频率与费用模型是什么？ / Eligibility, content responsibilities, opt-out and pricing?
6. 可否提供测试线路验证无前置通知上行 → Core 开放式回复 → 同手机实际接收的闭环？ / Can the exact conversation be tested on a real line?

### 电话必答 / Required voice answers

- 个人助理用途、呼入/呼出号码、AI 告知、接入资质和线路费用。 / Eligibility, bidirectional provisioning, disclosure and cost.
- 外部 Agent 的 HTTP/SSE/WS 协议、首包/整轮硬超时、并发、重试及 turn 标识；工具运行较慢时的允许行为。 / Streaming deadlines, retry identity and slow tool behavior.
- 用户打断事件、取消传播、实际停止播放、已听文本/音频 ACK；挂断后是否还继续请求或计费。 / Interruption, playback feedback and termination effects.
- 语音、转写、模型输入留存位置、期限、训练用途与删除能力。 / Data location, retention, training and deletion.

候选：阿里云 SMS/AICCS、创蓝及有明确运营商许可的通信服务商。它们是询证对象，不是已通过资格审查的选定供应商。大陆 SIM/Android 网关只作为实验或协议参考，不作为大陆商用产品路径，除非另有明确资质与法律审查。 / Candidates are qualification targets, not approved providers; SIM/Android gateways are not the mainland commercial path without separate legal qualification.

## 12. 参考来源 / Sources

检索日期 / Accessed：2026-10-09。官方接口文档证明接口，不证明商用准入；GitHub 样例证明代码结构，不证明大陆线路。 / Documentation and sample code do not establish carrier eligibility.

1. [阿里云 AICCS 自有后端协议 / External backend protocol](https://help.aliyun.com/zh/aiccs/user-guide/large-model-gateway-docking-parameter-protocol)
2. [阿里云 AICCS 大模型网关配置 / Model gateway configuration](https://help.aliyun.com/zh/aiccs/user-guide/large-model-gateway-configuration/)
3. [阿里云 AICCS 大模型应用与呼入配置 / Application management](https://help.aliyun.com/zh/aiccs/user-guide/big-model-application-management)
4. [阿里云 SMS 回执 FAQ / SMS reply and receipt FAQ](https://help.aliyun.com/zh/sms/developer-reference/receipt-message-faq)
5. [阿里云 SMS 下发 FAQ / Sender limitations](https://help.aliyun.com/zh/sms/user-guide/message-delivery-faq)
6. [创蓝国内短信服务介绍 / Mainland SMS product](https://doc.chuanglan.com/docs/P6HPKQVBLZ4QKT09)
7. [Twilio ConversationRelay](https://www.twilio.com/docs/voice/conversationrelay)
8. [Twilio Media Streams](https://www.twilio.com/docs/voice/media-streams)
9. [Twilio 国际 SMS 指南 / International SMS guide](https://www.twilio.com/docs/messaging/guides/sending-international-sms-guide)

本地项目约束：`AGENTS.md`、`docs/reference/ARCHITECTURE.md`、`docs/reference/REDLINES.md`、`docs/reference/SECURITY.md`、`docs/reference/SUBSYSTEM_ROUTING.md`、`docs/reference/FEATURE_LOCATOR.md`、`docs/reference/DEVELOPMENT.md`。 / Local architecture and validation constraints govern subsequent implementation.

GitHub 参考快照的双语台账位于 `/home/zou/mnt/macmini/TBU4/Github/telephony-sms-refs/snapshots-2026-10-09/README.md`；它们仅作参考，不引入运行时依赖。 / The bilingual ledger for the GitHub snapshots is stored at the mounted TBU4 path above; the snapshots are references only, not runtime dependencies.

## 13. 审阅与下一阶段 / Review and next stage

请审阅产品边界、身份保证、电话路径和建议运行参数。书面设计获批后，下一阶段是制定实施/可行性验证计划，而不是自动写代码。 / Human review of this written spec precedes implementation planning.

海外首发国家、供应商准入、生产预算及数据路径通过显式闸门选择；未通过时停止该线路交付，给出证据与所需决定，不以静默假设填补。 / Missing decisions block provisioning rather than becoming silent assumptions.

**本轮未做 / Not done：**产品代码、依赖安装、线路购买、供应商联系/签约、手机号收集、真实 SMS/电话、生产部署、性能测试与实施计划。开源参考不改变这些未验证状态。 / No code, provisioning, commercial outreach, real communication, deployment, benchmarks or implementation plan in this round.
