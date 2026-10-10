# PSTN 与 SMS 薄 Relay 接入实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在通过大陆/海外线路资格与安全闸门后，为 Aleph Core 增加一个复用既有 Gateway 路由、身份、记忆、工具和审批链的 SMS 适配器，以及一个不承载第二个 Agent 的电话薄传输适配器。

**Architecture:** 平台 Relay 和区域供应商负责号码、短信/电话媒体、转写合成、连接租约与传输回执；本仓只实现 Core 侧的 versioned wire contract、认证出站连接、去重/代次栅栏、SMS channel、电话 turn/playback 状态，以及现有 Core 的权限和工具接缝。电话首版优先采用供应商托管 ASR/TTS，Core 接收 committed transcript 并返回文本流；原始 SIP/RTP/WebRTC 媒体栈留在 Relay/外部连接器。

**Tech Stack:** Rust、tokio、serde/schemars、现有 `alephcore` Gateway、`shared/protocol`、WSS/等价认证双向连接；供应商和 Relay 不作为本仓运行时依赖。

**Spec:** `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md`

**Addendum（用户已正式批准）：** `docs/superpowers/specs/2026-10-09-telecom-public-context-addendum.md`——SMS/PSTN 弱来源进入同一 Core 的“公共上下文”会话。基线设计中的薄托管 Relay、G1–G5、双地区发布闸门、手机号弱保证、生产工厂关闭与“禁止真线路验收”均不变；本计划中的字段/API 名称均为概念边界，**不是已存在接口的声明**。

## Global Constraints

- 完整首版只有在 G1–G5 全部通过且第 10 节双地区效果验证通过时才能发布；任何线路能力未通过闸门都返回明确 unsupported，不静默降级。
- 首版同一已验证手机号只绑定一个主 Core；手机号按 E.164 规范化；来电显示和 SMS 发件地址只是低保证来源标识。
- SMS/电话默认只获得现有 `Chat`/guest 级别；敏感记忆、财务操作、密码/密钥和高风险工具必须走已认证控制面或其他可验证的可信审批通道。
- **公共上下文上限（addendum §2，已批准）：** 弱来源回合仍是同一 Core、同一模型、同一运行循环，不是第二个 Agent。provider 输入只能包含显式公共的 Core 提示 + 当前来电/短信文本 + 隔离的已完成公共历史；默认不加载所有者 SOUL/USER/profile/identity 额外文件、工作区指令、skills、hooks、全局记忆/wiki、路由召回、goals、agent 私有角色，不暴露主机路径与密钥。私有读取必须在历史/hook/编排提示词组装**之前**被跳过，不是事后字符串过滤，也不是仅设 `memory_mode=off`。该白名单是确定性的安全边界，不替代 LLM 意图判断（R7）。
- **来源 provenance fail-closed（addendum §3）：** 先过基线的已登记本人寻址、同意与绑定准入，再由 Core 依据实际注册的 SMS/PSTN 传输及已校验的绑定/租约盖章；未知/隐号/无法确定租户的输入不因公共模式获得默认路由；载荷中的手机号、配对记录、`role=config`、metadata、`approval_reference` 均不能提升权限。已知电信入口缺失或不支持来源/限制标记 = 拒绝执行，绝不退回 legacy owner 路径（`None` 恰是不受限 owner，不得当作隐私拒绝）。限制须沿 `RunRequest` → flow/envelope → 提示词/工具 → 会话持久化/重建全程携带，并在任务派生、会话复用处不丢失。不新增 `ScopeId` 变体、不伪造 Personal/Project scope、不改 `src/harness`（R10）。既有非电信渠道行为**不变**。
- **隔离历史与共享已完成快照（addendum §4.1–4.4，已批准数值）：** 会话键由权威绑定 + Core + 区域 + 严格 E.164 规范化来电方派生；SMS/语音地址仅为同一绑定的别名，二者共享**已完成**的公共历史；新短信运行/新电话开始时取得已完成历史快照，电话结束前不发布其回合、不刷新其他活跃运行的快照（活跃回合不互注）；绝不使用 `DmScope::Main/PerPeer` 所有者历史。公共历史加密存储：正文 TTL 24 小时（可降低，`0`=不持久化跨轮历史）、每会话 ≤ 32 个已完成回合 / 64 KiB、全 Core ≤ 8 MiB 加密载荷，先到者为准；只淘汰完整旧回合，不拆 tool-call/result；额度不足时拒绝持久化/新准入，不溢出到普通历史、不延长期限。撤销/重绑隔离旧历史（新绑定身份或 incarnation），`connection_generation` 不冒充绑定代次。公共运行禁用所有者记忆写入/抽取、compaction spill、全局缓存。
- **两阶段私有审批（addendum §4.5–4.7、§5）：** 私有读取批准 A（绑定精确来源、查询/原始输入、当前回合）与最终披露批准 B（绑定读取后生成的完整有界最终正文/语音文本、接收绑定、通道、generation、回合）分开、各自一次性、不可互换；A 不授予 B。A/B 期限均**从各自请求创建起计，批准不续期，B 不延长 A**；SMS ≤ 300 秒，通话 ≤ 60 秒；披露必须同时落在 A 的数据使用期限与 B 的效果期限内，语音剩余部分播放同样受两者约束。到期/撤销/挂断/打断/generation 变更/Core 断线：停止尚未提交的短信与未播放的语音，丢弃草稿与私有上下文；已提交/已播放效果不假装撤回，重试需新 operation 与新审批。模型草稿在 B 通过前不得发送、不进入可播放队列、不写公共历史；发送/播放前正文必须与获批正文精确匹配。只使用运营者已认证控制面请求器（`operator_requester`），不复用通用授权缓存/`GrantStore`、guardian 自动审批或不透明引用；`resolved_by` 仅审计；渠道内 `/approve`/回调旁路必须被拒。无可信审批通道时，需审批的动作 fail-closed，普通公共问答与停止/挂断/撤销仍可达。其他渠道“有人值守无限等待”政策不变，电信 TTL 仅是范围限定的扩展。
- **整回合脱敏（addendum §4.6–4.7）：** 私有数据一旦进入回合，该回合后续所有模型输出/工具内容均按私有来源处理；**整个回合（含用户原始问题）**在公共历史中只留固定不可展开的 opaque marker——不含操作名、目标、成功状态或私有细节；追问不自动回放原问题/回答，需重新提供并重新审批。精确效果账本只在受限控制面/执行层用于防重复与对账，不作为下轮公共 prompt 的入口；不做语义内容分类。
- **通用等待/拒绝提示（addendum §6.4）：** 进入私有审批时仅向访客发一次固定公共提示（“需要在可信控制面确认，当前尚未发送私人信息”）；等待期间电话保持连接、可挂断、不播放私有内容、不周期性发送收费短信；拒绝/超时只发固定“本次请求未完成，可继续普通聊天”，不说明来源、审批阶段或私人执行状态；这些状态提示不等于披露批准，公共停止/控制状态输出不受私有草稿阻塞。运营者在已认证控制面看到完整有界的收件人/动作/来源/查询/最终正文，不泄露到访客通知；该渲染能力是否已存在**未验证**。
- **不引入未证明的工具/来源（addendum §7）：** 后台任务/subagent/MCP/扩展/工具的私有来源在被审计并继承公共上限之前，弱回合不得走这些路径；公共安全工具初始集合可以为空，未知工具/私有来源默认拒绝；公共安全工具须有肯定的披露/数据访问契约，不按工具名或内容启发式选择。普通回复走既有输出路由，不靠启用主动发送工具。不添加第二个 Agent/访客 Agent/访客工作区，不修改 `src/harness`。
- Relay/Core 使用一份 versioned wire schema；不得分别手写互相抵消的两份 JSON 形状。
- Core 连接由 Core 主动建立并使用可轮换/可吊销凭据；旧 `connection_generation` 不得继续提交发送或播放。
- SMS 使用持久化后 ACK、至少一次投递和接收端去重；去重键为供应商账号、事件类型和供应商事件 ID，不使用正文哈希。
- `accepted`、`submitted`、`delivered/connected`、`played`、`failed`、`cancelled`、`unknown` 和 `outcome_unknown_expired` 必须保持语义区别；未知结果不得自动重发。
- 电话必须区分已生成、已进入播放队列和用户实际听到；只允许 committed transcript 开启 Agent turn，interim transcript 不得触发工具。
- 电话打断必须清空旧播放、取消旧 epoch 的生成、丢弃迟到输出，并保留已经发生的工具副作用；取消请求本身不能作为实际停止播放的验收证据。
- 不引入第二个云端 Agent，不把通信业务规则放入 Interface 层，不将 SIP/RTP/WebRTC 重型栈引入 Rust Core。
- 遵守 R1/R3/R4/R6/R7/R8/R9/R10；若采用本地 modem/SIM，必须走 Bridge/独立连接器，不进入 Core。
- 不把托管 Relay、供应商资格、线路价格、真实双向可达性或性能目标当成已经验证的事实。
- 单文件格式化使用 `rustfmt --edition 2021 <file>`；不要使用 `cargo fmt -- <file>`，因为该仓库的 workspace rustfmt 基线会产生无关修改。

## Review Focus

- **重复内容与重放：** 相同正文的两条合法 SMS 必须都到达；重复 webhook、SSE 重试和断线重发只能产生一次副作用。测试归 Task 1/Task 2。
- **低保证号码身份：** 伪造 caller ID、号码回收、旧绑定和未知/隐号不得读取私人记忆或获得高风险工具。测试归 Task 4。
- **公共上下文泄漏：** 私有 canary 经任一来源（历史、memory、profile/identity、routing recall、hooks、skills、goals、wiki、缓存、重建路径、后台/subagent/MCP/扩展）均不得到达 provider 输入、存储或效果边界；在**实际 provider 输入/存储/副作用处断言**，而非只检查标记或调用次数。测试归 Task 4。
- **A/B 审批与效果：** A 批准不得授予 B；期限从创建起且不续期；披露前正文与获批正文精确匹配；到期/撤销/挂断/generation 变更停止未发送/未播放内容。测试归 Task 4/Task 6。
- **未知提交结果：** 供应商已可能接受但网络超时的发送/拨号必须进入有限期 `unknown`，不得盲目重试；测试归 Task 3/Task 5。
- **边界与编码：** UCS-2/Unicode 长短信按真实 segment count 计费/预算，超限返回错误，不静默截断；测试归 Task 3。
- **播放效果与断线：** 电话旧 epoch 的迟到文本不得播放；挂断、断线、代次变化和审批过期都必须取消待处理动作，不能自动恢复。测试归 Task 5/Task 6。

---

## 外部前置闸门（实现前必须单独通过）

这组步骤不修改 Aleph 产品代码。它们把设计里的线路可行性和运营责任变成可审阅证据；未通过时只允许继续纯协议/状态机实验，不得实现或宣称可用线路。

### Gate 0：冻结首发范围与供应商证据

**Files:**
- Read: `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md`
- Evidence: G1–G5 questionnaire and provider replies, stored outside runtime code or in a separately reviewed design record

- [ ] 确认首发海外国家清单、国内 SMS/voice 目标能力、数据区域、AI 告知、退订、预算和用户同意范围。
- [ ] 对大陆 SMS 获得“个人 AI 助理开放式对话”的书面用途准入，并证明自由文本下行、无近期下行时的用户主动上行、稳定接收地址和唯一入站事件身份。
- [ ] 对大陆电话验证号码、资质、外部 Core 接入、turn/retry 标识、取消传播和已播放反馈；不能用“返回取消请求”替代实际停止播放。
- [ ] 对每个海外国家逐国验证双向 SMS、双向电话、号码注册、AI 用途和数据路径。
- [ ] 验证租户隔离、密钥吊销、回调防伪/防重放、费用和供应商留存/训练政策。
- [ ] **停止条件：** G1、G2 或 G3 任一未通过时，不实现对应生产 adapter；不得用模板变量塞入整段 AI 回复，也不得用另一地区线路静默代替。

---

## 文件结构与责任

| 文件 | 动作 | 职责 |
|---|---|---|
| `shared/protocol/src/telecom_relay.rs` | Create | Core/Relay 共用的 versioned envelope、SMS、call turn、command、outcome 和能力声明 |
| `shared/protocol/src/lib.rs` | Modify | 导出 telecom relay 类型 |
| `src/gateway/telecom/mod.rs` | Create | 导出 Core 侧通用 telecom 状态机 |
| `src/gateway/telecom/protocol.rs` | Create | Core 侧纯传输状态机：去重、TTL、代次栅栏、outcome 单调转换 |
| `src/gateway/interfaces/sms/config.rs` | Create | SMS adapter 的配置反序列化和能力/区域声明 |
| `src/gateway/interfaces/sms/mod.rs` | Create | `Channel`/`ChannelFactory` 实现、入站规范化和 channel 生命周期 |
| `src/gateway/interfaces/sms/message_ops.rs` | Create | SMS `OutboundMessage` 到 relay command 的映射与结果关联 |
| `src/gateway/interfaces/sms/transport.rs` | Create | Core 主动 WSS/等价连接、心跳、重连、单活 generation |
| `src/gateway/interfaces/mod.rs` | Modify | 注册 SMS factory |
| `shared/protocol/src/channels.rs` | Modify | 加入排序后的 `CONFIGURABLE_CHANNEL_TYPES` |
| `interfaces/webchat/src/platform/wide/views/settings/channels/definitions.rs` | Modify | 增加与协议集合相等的 SMS 配置卡片 |
| `src/config/tests/channels.rs` | Test/Modify if required | 保持配置实例、factory table 和 Panel 集合一致 |
| `src/gateway/inbound_router/permission.rs`、`src/gateway/pairing_store.rs`、`src/acp/manager/session_key.rs` | Modify after interface review | 绑定、低保证来源和 session key 的权限边界；不得把号码当强认证 |
| 公共上下文限制载体（概念）：`RunRequest` → flow/envelope → 提示词/工具 → 会话持久化/重建；落点待 Task 4 source census 确认 | Investigate, then Modify only verified seams | 携带“公共上下文上限 + 已校验来源”的不可提升标记；现有可承载字段**未验证**；不新增 `ScopeId` 变体、不改 `src/harness`、不用 `None` 表达拒绝 |
| prompt 组装/runner/history 重建/记忆与缓存/compaction/二级 prompt 写入点（概念，参考 addendum §9 引用的 `prompt_build.rs`、`runner_impl.rs`、`inner.rs`）；真实路径由 Task 4 census 记录 | Investigate, then Modify only verified seams | 在组装前跳过私有读取；公共提示模板；禁用公共运行的 owner 记忆写入/抽取、compaction spill、全局缓存；census 须覆盖 hooks、skills、routing recall、goals、wiki、后台/subagent/MCP/扩展 |
| 公共历史存储与加密/留存（概念）：会话持久化 seam；落点待调研 | Investigate, then Create/Modify | 绑定+Core+区域+E.164 派生会话键、已完成回合快照、24h/32 回合/64 KiB/8 MiB 界限、绑定重建 incarnation；是否可复用现有加密存储**未验证** |
| 审批 seam（概念）：`operator_requester`、`ExecApprovalManager`、`inbound_router` 的 `/approve` 路径（`resolve_for_session`）、完整正文渲染 | Investigate, then Modify only verified seams | 两阶段 A/B 审批、期限、一次性、拒绝渠道内旁路；现有渲染/schema 是否支持完整有界正文**未验证** |
| `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-4-census.md` | Create | Task 4 的 source/rebuild/cache/background/subagent/MCP/extension 普查记录与每个来源的处置（denied/inherits-ceiling/proven-public） |
| `src/gateway/interfaces/voice_call/{mod.rs,protocol.rs,transport.rs}` | Create | 电话通道/turn 生命周期和薄 transport，不实现另一 Agent |
| `src/builtin_tools/channel_message.rs` | Modify | 复用现有消息发送语义，接入 SMS recipient binding 和真实 outcome |
| `src/builtin_tools/telecom.rs` | Create | 绑定状态、主动联系授权、电话生命周期和取消等工具契约（若现有工具盘点证明可合并，则保留现有工具并在计划执行前记录理由） |
| `src/executor/builtin_registry/{groups.rs,optional_tools.rs}` | Modify | 注册 telecom tools，并接入现有 approval census |
| `src/gateway/channel_approval.rs`、`src/exec/approval/channel_bridge.rs` | Modify only where needed | 将 approval reference 与 operation/call/turn 绑定，保证不可达时 fail-closed |
| `tests/telecom_relay_protocol_test.rs` | Create | Core 侧 wire/state contract 集成测试 |
| `tests/sms_contract_test.rs`、`tests/sms_fixture_test.rs` | Create | SMS channel/factory/fixture 测试 |
| `tests/voice_call_protocol_test.rs` | Create | 电话 epoch、打断、播放反馈和重放测试 |
| `tests/telecom_approval_test.rs` | Create | 来源保证、审批取消、解绑和租户隔离测试 |
| `tests/telecom_public_context_test.rs` | Create | 公共上下文上限：provider 输入 canary、隔离历史/快照、整回合脱敏、A/B 审批与效果边界、未知来源默认拒绝；在真实边界断言 |

Relay 服务、供应商 webhook、SIP/RTP/WebRTC 和真实线路脚本属于独立托管/QA 项目，不在本仓创建占位 server；本仓只消费共享 wire contract。

---

### Task 1: 建立共享 telecom wire contract 与纯状态机

**Depends on:** Gate 0 的协议范围可以冻结；不要求线路已可用。

**Files:**
- Create: `src/gateway/telecom/mod.rs`
- Create: `src/gateway/telecom/protocol.rs`
- Modify: `src/gateway/mod.rs`
- Modify: `shared/protocol/src/lib.rs`
- Test: `shared/protocol/src/telecom_relay.rs` unit tests and `tests/telecom_relay_protocol_test.rs`

**Interfaces:**

- `TelecomEnvelope { protocol_version, binding_id, connection_generation, event_id, kind, created_at, expires_at }`；认证连接是 binding 身份的权威，payload 内重复 binding 只能用于一致性校验。
- `SmsInbound`, `SmsCommand`, `CallTurn`, `PlaybackEvent`, `TelecomOutcome`；字段按 spec §6 固定，包括 `provider_account_id`、`provider_message_id`、`phone_call_id`、`turn_id`、`output_epoch`、`output_seq`、`operation_id` 和 `approval_reference`。
- `TelecomOutcome` 至少包含 `Accepted`、`Submitted`、`Delivered`、`Connected`、`Played`、`Failed`、`Cancelled`、`Unknown`、`OutcomeUnknownExpired`；不得用字符串自由拼出相互冲突的状态。
- 纯函数/状态 API：`dedup_key(...)`、`accept_event(...)`、`advance_outcome(...)`、`fence_generation(...)`、`expire_unknown(...)`。具体错误类型和内部存储可按现有 Rust 错误约定实现，但状态转换必须可单测。`src/gateway/telecom/mod.rs` 负责导出该模块；SMS 和 voice adapter 只能通过此模块引用状态机。

- [ ] **Step 1: 写失败测试。** 在 `shared/protocol/src/telecom_relay.rs` 覆盖 serde round-trip、未知 protocol version 拒绝；在 `tests/telecom_relay_protocol_test.rs` 覆盖重复 webhook、不因正文相同而去重、同一 `provider_message_id` 的不同 receipt event 都保留、旧 generation 拒绝、`unknown → outcome_unknown_expired` 不自动重发、过期事件不触发语义回复。
- [ ] **Step 2: 运行 `cargo test -p aleph-protocol telecom_relay` 和 `cargo test -p alephcore --test telecom_relay_protocol_test`，确认新类型/函数尚不存在而失败。**
- [ ] **Step 3: 实现共享 schema 和状态机。** 类型放在 `shared/protocol`，Core 状态逻辑只引用它；不要在 Relay 或 Core 复制 JSON shape。
- [ ] **Step 4: 运行 `cargo test -p aleph-protocol telecom_relay` 和 `cargo test -p alephcore --test telecom_relay_protocol_test`，确认 PASS。**
- [ ] **Step 5: 运行 `cargo check -p alephcore` 和 `cargo test -p alephcore --lib gateway::telecom`，确认现有 workspace 能引用该协议。**
- [ ] **Step 6: 提交 `feat: add telecom relay wire contract and state guards`。**

验收重点：同正文新消息不丢；receipt 不被 provider message ID 粗暴覆盖；所有 terminal outcome 和 unknown expiry 都是显式状态。

---

### Task 2: 实现 SMS Core channel 骨架、配置和注册

**Depends on:** Task 1。

**Files:**
- Create: `src/gateway/interfaces/sms/config.rs`
- Create: `src/gateway/interfaces/sms/mod.rs`
- Create: `src/gateway/interfaces/sms/message_ops.rs`
- Modify: `src/gateway/interfaces/mod.rs`
- Modify: `shared/protocol/src/channels.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/settings/channels/definitions.rs`
- Test: `tests/sms_contract_test.rs`, `tests/sms_fixture_test.rs`

**Interfaces:**

- 实现现有 `Channel` trait 的 `info`、`state`、`start`、`stop`、`send(OutboundMessage)`；通过 `capabilities()` 声明 SMS 只支持实际实现的文字能力，typing、附件、按钮等保持 false。
- 实现现有 `ChannelFactory` 的 `channel_type() == "sms"` 与 `create(serde_json::Value)`；配置包含显式 `region`、`service_address`、Core binding reference 和能力开关，不从语言或号码前缀猜地区。
- 入站消息转换必须复用 `InboundMessage`，保留 provider event identity、E.164 peer address、encoding、segment count 和 transport outcome metadata。

- [ ] **Step 1: 为 factory、capabilities、E.164 规范化、长短信 metadata、禁用能力和 fixture 写失败测试。** 测试不连接真实供应商。
- [ ] **Step 2: 运行 `cargo test -p alephcore --test sms_contract_test`，确认失败。**
- [ ] **Step 3: 创建 SMS channel/config/message_ops。** 发送函数只能产生待提交 operation，不假报 delivered；不把短信正文写入托管 adapter 的长期日志。
- [ ] **Step 4: 在四个配置/注册集合同步注册 `sms`：** `src/gateway/interfaces/mod.rs` 的 `register_plain_channel!`、`shared/protocol/src/channels.rs` 的排序集合、Panel `ALL_CHANNELS`、必要时 `src/config/tests/channels.rs` 的一致性 fixture。
- [ ] **Step 5: 运行 `cargo test -p alephcore --test sms_contract_test`、`cargo test -p alephcore --test sms_fixture_test` 和 `cargo test -p alephcore register_tests`，确认 factory table 与 Panel/protocol 集合一致。**
- [ ] **Step 6: 提交 `feat: add SMS channel contract and registration`。**

验收重点：配置存在但 relay 未连接时报告 unavailable/unknown；任何函数调用成功都不能替代实际短信效果。

---

### Task 3: 接入 SMS transport、状态回执和预算边界

**Depends on:** Task 1、Task 2；Gate 0 中对应地区线路必须已通过资格审查后才能启用真实 provider。

**Files:**
- Create: `src/gateway/interfaces/sms/transport.rs`
- Modify: `src/gateway/interfaces/sms/mod.rs`, `src/gateway/interfaces/sms/message_ops.rs`
- Modify if needed: existing gateway retry/registry seam identified during implementation
- Test: `tests/sms_fixture_test.rs`, transport unit tests near `transport.rs`

**Interfaces:**

- `SmsTransport::connect(config, credentials) -> ...`：Core 主动认证连接；不得开放个人 Core 公网 webhook。
- `send(operation_id, recipient_binding_id, body, expires_at) -> TelecomOutcome`：提交前重验 binding、consent、approval、budget 和 generation。
- 入站回调/relay event handler：先验 envelope、落安全 metadata、持久化 ACK，再把规范化事件交给现有 inbound router；不得等待 LLM。

- [ ] **Step 1: 写 mock transport 测试。** 覆盖心跳、重连、单活 generation、旧 generation 拒发、重复事件、相同正文连续两次、分段预算、`accepted/submitted/unknown/delivered` 分离和 TTL。
- [ ] **Step 2: 运行 targeted SMS tests，确认 transport 尚不存在而失败。**
- [ ] **Step 3: 实现认证连接和 receipt mapping。** 重连后新 generation 生效，旧连接仍收到事件时只可记录 stale，不得发送。
- [ ] **Step 4: 实现成本 admission。** 由 provider 实际 encoding/segment metadata 计数；预算不足返回明确错误交给 Core，不能静默截断或替换为另一线路。
- [ ] **Step 5: 运行 `cargo test -p alephcore --test sms_fixture_test`、相关 `cargo test -p alephcore --lib gateway::interfaces::sms` 和 `cargo check -p alephcore`。**
- [ ] **Step 6: 在一条获批测试线路上执行效果验证：** 用户主动上行、Core 回复、真实手机出现正文、回执与 operation 对应；保存 evidence，不把本地 mock 结果当线路通过。
- [ ] **Step 7: 提交 `feat: connect SMS relay transport and truthful outcomes`。**

验收重点：供应商超时但可能已接受时进入 `unknown`，不会自动换供应商重发；unknown 到期只变为 `outcome_unknown_expired`，重新发送必须是新 operation 且重新审批。

---

### Task 4: 同 Core 公共上下文、身份与 provenance、隔离历史、来源普查和两阶段审批/效果栅栏

**Depends on:** Task 2；Task 3 的 operation/binding 字段必须已冻结；addendum 已获用户批准；基线设计中的托管控制面绑定权威尚未实现，本任务只能消费其**契约**，不得以本地配置字符串冒充绑定证明。

**范围说明：** 本任务不只是“手机号 guest 权限”。它交付：(1) 同一 Core 的公共上下文上限与 provenance 盖章（fail-closed）；(2) 隔离的公共历史及 SMS/语音已完成快照共享（活跃回合不互注）；(3) 完整来源/重建/缓存/后台/subagent/MCP/扩展普查；(4) 私有读取 A 与最终披露 B 的两阶段审批及真实效果栅栏；(5) 整回合脱敏与通用等待/拒绝 UX。既有非电信渠道行为不变。**允许的初始切片**（仅隔离 + 空/未知默认拒绝的工具集）不得冒充完整 Task 4；完整 Task 4 必须包含独立审批的私有读取/主动联系链路及其效果验收。

**Files:**
- Create: `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-4-census.md`（普查记录）
- Investigate first, then Modify only verified seams: `RunRequest`/flow/envelope、prompt 组装、runner、history 重建、会话持久化、记忆/缓存/compaction、二级 prompt 写入点（路径由 census 记录；不得假设上述名称对应现有 API）
- Modify: `src/gateway/inbound_router/permission.rs`
- Modify: `src/gateway/pairing_store.rs`
- Modify after verified seam: `src/acp/manager/session_key.rs`（仅在 census 证明需要；绝不复用 `DmScope::Main/PerPeer` 所有者历史）
- Modify only as required: `src/gateway/channel_approval.rs`, `src/exec/approval/channel_bridge.rs`, `inbound_router` 中可达 `resolve_for_session` 的 `/approve` 旁路
- Modify: `src/builtin_tools/channel_message.rs`
- Create: `src/builtin_tools/telecom.rs` if tool census confirms a separate tool is required
- Modify: `src/executor/builtin_registry/groups.rs`, `src/executor/builtin_registry/optional_tools.rs`
- Test: `tests/telecom_approval_test.rs`, `tests/telecom_public_context_test.rs`
- **Do not touch:** `src/harness/**`、`ScopeId` 定义、非电信渠道的路由/权限/历史/审批行为、基线 spec/addendum/报告。

**Interfaces（概念，非现有 API）：**

- 绑定 key 必须至少区分 `region`、`service_address`、`verified_phone` 和主 Core；SMS 与 voice address 是同一 binding 的别名，不是两个身份。手机号严格 E.164，不猜默认国家；别名仅由权威绑定声明；撤销/重绑产生新的绑定身份或 incarnation，旧历史隔离；`connection_generation` 仍按 Core 连接租约递增，不冒充绑定代次。
- 来源 provenance：已知电信入口 + 已校验准入 → Core 盖章“公共上下文上限”；缺失/不支持的标记 → 拒绝，无 legacy owner 回退；载荷字段不可提升；标记随 `RunRequest` → flow/envelope → 提示词/工具 → 持久化/重建、任务派生、会话复用全程携带。
- 公共 prompt 输入只允许：Core 维护的显式公共模板 + 当前文本 + 同一绑定/规范化来电方的已完成公共历史快照。
- 隔离公共历史：加密；24h TTL（可降、`0`=不持久化）、每会话 ≤ 32 完成回合 / 64 KiB、全 Core ≤ 8 MiB；淘汰整回合；额度不足拒绝持久化/新准入；SMS 与语音共享已完成历史；活跃回合开始时取快照，结束后才发布，不互相刷新。
- 审批阶段 A（私有读取）与 B（最终披露）：各自一次性、各自从请求创建起计时、不续期、B 不延长 A；SMS ≤ 300 秒 / 通话 ≤ 60 秒；A 绑定精确来源 + 查询/原始输入 + 当前回合；B 绑定完整有界最终正文/语音文本 + 接收绑定 + 通道 + generation + 回合。只经 `operator_requester` 的已认证控制面。
- 私有回合：用户原始问题与全部后续输出/工具内容在公共历史中仅留固定 opaque marker；受限效果账本独立存放，不进入下轮公共 prompt。
- 通用提示：一次性固定等待提示；固定拒绝/超时提示；不含来源、阶段或私人执行状态。
- 主动 SMS/外呼授权分别建模；command 带 `operation_id`、`approval_reference`、`recipient_binding_id`、`expires_at`；`approval_reference` 仅为关联，不是权限。
- 待审批动作绑定 `phone_call_id`/`turn_id`（电话）或 operation（SMS）；挂断、打断、Core disconnect、generation 变化、解绑、撤销和过期都立即取消并停止未发送/未播放内容。

- [ ] **Step 1: 先做完整 source census，不写实现。** 在 `task-4-census.md` 中逐项记录（文件:行、读取时机、是否在历史/hook/编排组装之前、处置）：所有 prompt 来源（核心提示、SOUL/USER/profile/identity 额外文件、工作区指令、skills、hooks、全局记忆/wiki、routing recall、goals、agent 私有角色）、history 重建路径、会话复用/任务派生、缓存与二级 prompt 写入者（含 compaction spill、记忆抽取）、gate-chain 默认值/guardian、后台任务、subagent、MCP、扩展与全部已注册工具。每个来源分类为 `denied` / `inherits-ceiling` / `proven-public`；无法证明隔离 = `denied`。同时确认：现有哪些字段可承载限制标记、`operator_requester` 是否能展示完整有界正文、会话持久化是否可加密与留存界限、`ExecApprovalManager`（单次飞行 oneshot）与 `GrantStore`（无 once/TTL）为何不可复用。census 若发现需改 `src/harness` 或新增 `ScopeId` 变体，**停止并上报**，不要绕过。
- [ ] **Step 2: 在真实边界写失败测试**（`tests/telecom_public_context_test.rs`、`tests/telecom_approval_test.rs`），断言实际 provider 输入、实际存储和实际副作用，而非标记/调用次数：
  - 私有 canary 注入 census 列出的每个来源（历史、memory、profile/identity、routing recall、hooks、skills、goals、wiki、缓存、重建路径），均不出现在 provider 输入；
  - 同一手机号跨 Core/绑定互不共享，且无所有者历史；撤销/重绑后旧历史不可见；
  - 伪造 caller ID、未知/隐号、旧 binding、payload 中伪造 role/`role=config`/`approval_reference`/metadata/自审批被拒；已知电信入口缺失 provenance 拒绝而非回退 owner；
  - 公共历史：24h TTL、32 回合/64 KiB/8 MiB 界限、整回合淘汰、额度不足拒绝（不溢出普通历史）；SMS/语音共享已完成快照，活跃回合不互注，电话结束前不发布；
  - 整个私有回合（含原始问题）在公共历史只有 opaque marker，下轮不回放，不溢出到后台/compaction；
  - 未知工具/私有来源默认拒绝；公共安全工具集初始为空时弱回合无工具；后台/subagent/MCP/扩展未继承上限前被拒；
  - 无可信审批通道 fail-closed，但普通公共问答与停止/挂断/撤销仍可达；渠道内 `/approve`/回调旁路被拒；非电信渠道无变化（含“有人值守无限等待”）；
  - 回归：普通渠道权限/历史/路由/审批行为与改动前一致。
- [ ] **Step 3: 运行 `cargo test -p alephcore --test telecom_public_context_test` 与 `cargo test -p alephcore --test telecom_approval_test`，确认因功能缺失而失败**（不得因编译/夹具错误而“失败”）。
- [ ] **Step 4: 实现 binding/source assurance/provenance 盖章与 session key 组合。** 权限与上限拦截发生在 memory/history/hook/编排注入**之前**；不新增 `ScopeId` 变体、不用 `None` 表达拒绝、不改 `src/harness`、无 legacy owner 回退路径。
- [ ] **Step 5: 实现公共 prompt 组装与隔离历史。** 在组装前跳过私有读取（非事后过滤，非仅 `memory_mode=off`）；落地已完成快照共享、按界限淘汰整回合、加密存储、重绑 incarnation；禁用公共运行的 owner 记忆写入/抽取、compaction spill、全局缓存。若 Step 1 证明无法加密/限界/隔离，则拒绝该能力并上报，不退而求其次。
- [ ] **Step 6: 把 SMS 回复与主动发送权限分开。** 已绑定本人同通道回复走既有输出路由、不提升权限；主动联系始终重新检查 scope、expiry、revocation 和预算。
- [ ] **Step 7: 实现 A→读取→草稿→B→效果的端到端 seam。** 顺序：访客请求私有信息 → 一次性固定等待提示 → 运营者在已认证控制面看到精确来源+查询并批准 A → 仅限当轮内部读取与生成草稿（草稿不发送/不入可播放队列/不入公共历史）→ 运营者看到完整有界最终正文并批准 B → await 之后、生成完成后、发送/播放前重新校验（撤销、挂断、generation、Core 断线、预算、A/B 两个期限、正文与获批正文逐字匹配）→ 实际 SMS 提交或 TTS 播放。A、B 期限均从请求创建起计，批准不续期，B 不延长 A；A 到期丢弃草稿与私有上下文并取消尚未执行的 B；语音剩余播放同时受 A/B 约束；拒绝/超时发固定“本次请求未完成，可继续普通聊天”。已提交/已播放不伪造撤回，保留 accepted/submitted/unknown，重试需新 operation 与新审批。仅使用 `operator_requester`；不复用 `GrantStore`、guardian 自动审批或不透明引用。无可信审批通道 fail-closed。
- [ ] **Step 8: 把 telecom 操作注册到现有 builtin/approval census。** 工具只暴露绑定、撤销、预算、主动 SMS/电话及取消所需的能力，不在 Interface 内判断“用户是否应该被打扰”；公共安全工具初始为空，新增须有肯定的披露/数据访问契约。
- [ ] **Step 9: 运行 `cargo test -p alephcore --test telecom_public_context_test`、`cargo test -p alephcore --test telecom_approval_test`、相关 inbound_router/approval tests（含非电信回归）和 `cargo check -p alephcore`。**
- [ ] **Step 10: 提交 `feat: gate telecom public context, identity and two-stage approvals`。**（若仅完成“隔离 + 默认拒绝”初始切片，提交信息与交付报告必须明确标注为 slice，Task 4 不得标记完成。）

验收清单（全部满足才算 Task 4 完成）：

- [ ] `task-4-census.md` 完整覆盖所有 prompt/history/rebuild/cache/secondary-writer/background/subagent/MCP/extension 来源，且每项有处置；未证明隔离者为 denied。
- [ ] 私有 canary 在每个来源上均未到达实际 provider 输入、公共存储或访客可见效果。
- [ ] 同 Core、同模型、无第二 Agent、无访客工作区；`src/harness` 与 `ScopeId` 无改动；无 legacy owner 回退；非电信渠道行为不变（含永久等待）。
- [ ] 公共历史满足 24h/32 回合/64 KiB/8 MiB 界限；SMS 与语音共享已完成快照、活跃回合不互注；绑定间互不共享，重绑隔离旧历史。
- [ ] 整个私有回合（含原问题）仅 opaque marker，下轮不回放，不溢出到后台/compaction；受限账本不进入公共 prompt。
- [ ] A/B 各自一次性、各自从创建计时不续期；A 不授予 B；B 不延长 A；披露正文与获批正文逐字一致；到期/撤销/挂断/generation 变更停止未发送短信与未播放语音。
- [ ] 无可信审批通道 fail-closed；停止/挂断/撤销始终可达；渠道内 `/approve` 旁路与伪造 role/ref/自审批均被拒。
- [ ] 等待/拒绝提示为固定通用文案，一次性，电话保持连接，不含私有细节；运营者完整有界正文仅在已认证控制面展示。
- [ ] 未知工具/来源默认拒绝；除经审计并有肯定契约者外无公共安全工具；离线时不出现替代 Agent。
- [ ] 以上均在真实边界（provider 输入/存储/副作用）有测试证据；未运行的测试不得声称通过。

验收重点：没有可信审批通道就 fail-closed；停止/解绑能力始终可达；挂断或断线后不会在重连时自动执行旧审批；公共上下文的“公共”是数据访问上限而非第二个大脑。

---

### Task 5: 实现电话薄 transport 与纯 turn/playback 状态机

**Depends on:** Task 1、Task 4；供应商必须先证明 turn/retry/playback 事件足够区分，不能把 `session_id` 当作每轮唯一 ID。

**Files:**
- Create: `src/gateway/interfaces/voice_call/protocol.rs`
- Create: `src/gateway/interfaces/voice_call/transport.rs`
- Create: `src/gateway/interfaces/voice_call/mod.rs`
- Reference only: `src/gateway/voice/streaming/mod.rs`, `src/gateway/voice/inbound/mod.rs`, `src/gateway/voice/outbound.rs`
- Do not extend as production transport: `plugins/voice-call/README.md`
- Test: `tests/voice_call_protocol_test.rs`

**Interfaces:**

- `CallSession { phone_call_id, provider_call_id, binding_id, connection_generation }`。
- `TurnState { turn_id, output_epoch, output_seq, committed_transcript, generated_text, queued_text, played_range }`。
- `CallTransport` 只负责建立/结束/取消/发送文本输出/接收 committed transcript 和 playback event；它不做意图判断、记忆检索或工具审批。
- `start_call`、`hangup`、`cancel_output(phone_call_id, output_epoch)` 和 `publish_playback_ack` 必须绑定具体 call/turn/epoch。

- [ ] **Step 1: 写纯状态机失败测试。** 覆盖 interim 不开 turn、迟到旧 epoch 丢弃、新 turn 清空旧队列、已播放范围截断 transcript、无精确 playback ACK 时标记 unknown、已发生工具副作用不回滚、挂断释放线路。
- [ ] **Step 2: 运行 `cargo test -p alephcore --test voice_call_protocol_test`，确认失败。**
- [ ] **Step 3: 实现 `protocol.rs`。** 明确生成/排队/播放三层状态；状态转换只由事件驱动，不能由“函数调用成功”伪造效果。
- [ ] **Step 4: 在 `transport.rs` 实现 Core 主动认证连接和单活 generation。** 供应商音频编解码、VAD、ASR/TTS 在外部 Relay；若未来使用原始媒体，先单独验证 μ-law/PCM、采样率、背压、双向音轨和 clear/playback feedback。
- [ ] **Step 5: 在 `mod.rs` 将 committed transcript 接入既有 inbound/session 路由。** 不把进行中的电话 transcript 注入或刷新并发 SMS 运行；电话结束前不向共享公共历史发布其回合，结束后仅以 Task 4 的**已完成公共历史**形式（同一绑定/规范化来电方、受同一界限与私有回合脱敏约束）供后续 SMS/电话运行取快照读取。
- [ ] **Step 6: 运行 `cargo test -p alephcore --test voice_call_protocol_test`、`cargo test -p alephcore --lib gateway::interfaces::voice_call` 和 `cargo check -p alephcore`。**
- [ ] **Step 7: 提交 `feat: add thin voice call transport lifecycle`。**

验收重点：只有真实停止旧音频、迟到输出被 fence、工具副作用账本保持一致，电话打断才算通过；不得把 `src/gateway/voice` 的语音附件 STT/TTS 直接称为全双工 PSTN。

---

### Task 6: 接入电话生命周期工具、预算与真实线路效果验收

**Depends on:** Task 4、Task 5；Gate 0 的 G1–G5 全部通过后才可进入产品发布验收。**Task 6 不得在以下任一条件未满足时声称完成：** Task 4 的公共上下文全量 source/rebuild/cache/background/subagent/MCP/extension 普查及真实边界测试已通过（addendum §8 声明该普查是生产前最终闸门）、A/B 两阶段审批在真实控制面通过、且 SMS 提交与 TTS 播放的真实线路效果（含“A 批准但无 B → 安全失败”“到期/撤销/挂断停止未发送/未播放”）已验证。仅有 Task 4 的初始隔离切片不构成满足。

**Files:**
- Modify: `src/builtin_tools/telecom.rs` or the approved existing tool surface
- Modify: `src/executor/builtin_registry/{groups.rs,optional_tools.rs}`
- Modify: `src/gateway/interfaces/voice_call/{mod.rs,transport.rs}`
- Test/evidence: `tests/telecom_approval_test.rs` and separately authorized real-line QA record

- [ ] **Step 1: 为 `initiate_call`、`answer_call`、`hangup`、`cancel_output`、`call_status` 固定输入/输出 schema。** `initiate_call` 需要本人 binding、单次 approval（经 Task 4 的 `operator_requester` 控制面）、60 秒以内 expiry、预算和 operation ID；离线不排队。电话内披露私有信息须走 Task 4 的 A/B 两阶段审批，不新增旁路。
- [ ] **Step 2: 写失败测试。** 覆盖未授权外呼、审批过期、线路预算不足、hangup 的停止方向、Core 断线不自动重拨、unknown outcome 不重复拨号、非本人号码拒绝。
- [ ] **Step 3: 实现工具与 approval wiring。** 不让“电话中一句同意”替代可信审批；停止/挂断必须作为无条件可达 off-switch。
- [ ] **Step 4: 运行 targeted tests、`cargo check -p alephcore`、适用 clippy 和项目最小可信验证集。** 如需格式化，只使用 `rustfmt --edition 2021` 作用于实际修改文件。
- [ ] **Step 5: 进行获批真机矩阵。** 逐地区验证呼入、呼出、双向可听、SMS 主动上行、回复、重复 webhook、断线重连、unknown、打断、已播放反馈、退订、费用上限、租户隔离和解绑失效，以及公共上下文矩阵（私有 canary 不外泄、跨 SMS/语音仅共享已完成历史、A→B 审批后的真实 SMS/TTS 效果、等待/拒绝固定提示、无审批通道 fail-closed）；效果证据必须来自真实手机/线路，不以 HTTP 200、Connected 或函数调用替代。
- [ ] **Step 6: 核对建议运行参数。** 24 小时离线 SMS TTL、主动 SMS 授权最多 5 分钟、外呼授权最多 60 秒、metadata 去重初始 7 天、unknown 有界保留期、p95 首个可听输出 ≤ 2 秒和打断停止 p95 ≤ 500 ms 只能在实际测量与运营审阅后成为配置。
- [ ] **Step 7: 只有 G1–G5 和双地区效果矩阵全绿时，提交 `feat: release telecom channels`；否则保留实验 flag 和证据，明确线路阻塞原因。**

---

## 验证命令索引

按任务逐步运行，避免用一次全量构建掩盖具体回归：

```bash
cargo test -p aleph-protocol telecom_relay
cargo test -p alephcore --test telecom_relay_protocol_test
cargo test -p alephcore --test sms_contract_test
cargo test -p alephcore --test sms_fixture_test
cargo test -p alephcore --test telecom_approval_test
cargo test -p alephcore --test voice_call_protocol_test
cargo test -p alephcore register_tests
cargo check -p alephcore
cargo clippy -p alephcore --all-targets --all-features
```

Rust 测试和 clippy 的既存失败必须先按当前 merge-base 建立基线，再判断是否是 telecom 变更引入；不把环境性/既存红误报为本功能通过。真实线路测试另行记录供应商、国家、运营商、号码、时间窗口、证据和费用，测试记录不得把手机号或正文写进普通日志。

## 计划自审

- 设计 §1 的 SMS、电话、用户配置、托管边界和号码映射分别覆盖 Gate 0、Task 2–6；没有把用户自备 SIM 当产品路径。
- 设计 §2 的 G1–G5 和“无静默降级”写入 Gate 0、Global Constraints 和 Task 6；未通过时有明确停止点。
- 设计 §5 的低保证身份、Chat/guest、分离主动联系同意、审批取消和停止能力写入 Task 4。
- Addendum（已批准）的公共上下文上限、provenance fail-closed、隔离历史与已完成快照共享、整回合脱敏、A/B 两阶段审批、通用等待/拒绝 UX、未知工具默认拒绝写入 Global Constraints 与 Task 4；完整 source census 与真实效果验收是 Task 6 的前置条件。所有承载字段/审批渲染/加密存储均标注为“待调研”，未假设现有 API。
- 设计 §6–§7 的 envelope、dedup、generation、unknown、committed turn、output epoch、playback feedback 写入 Task 1、3、5。
- 设计 §8–§9 的离线、数据保留、地区隔离、凭据轮换和供应商验签由 Task 3–6 的接口/验收承担；Relay 端具体实现明确留在独立部署项目。
- 设计 §10 的效果矩阵写入 Task 3 和 Task 6；未把 HTTP/函数调用当成效果证据。
- 现有代码中 approval census、vault/凭据接线和 `SessionKey` 的最终落点仍需在执行 Task 4 前按实际接口核对；计划明确要求“after verified seam”，不会用臆造签名替代调查。

## Execution Handoff

计划已保存到 `docs/superpowers/plans/2026-10-09-pstn-sms-thin-relay-implementation.md`。请审阅计划，确认它准确表达了产品边界、线路闸门、Core/Relay 分工和验证标准。实现尚未开始；下一步需要你选择执行方式：

- **Subagent-driven**：每个任务由新 subagent 实现，并在进入下一任务前由独立 reviewer 检查；适合这个跨协议、权限、SMS、电话状态机的高风险变更。
- **Native**：我在当前会话按任务逐项实现，最后由独立 reviewer 做整分支审查；成本更低，但中间独立审查较少。

我推荐 **Subagent-driven**，因为任务之间有共享 wire 类型、权限契约和电话副作用状态依赖，且线路/审批错误的代价高。请先确认计划内容和执行方式。
