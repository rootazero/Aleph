# SMS/PSTN 公共上下文模式补充设计 / Telecom Public-Context Mode Addendum

- Date / 日期：2026-10-09
- Base / 基线：`docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md`（薄托管 Relay、G1–G5、双地区发布闸门、手机号弱保证、生产工厂关闭/禁止真线路验收——**均不变**）；证据：`.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-4-preflight.md`
- Status / 状态：**草案。用户已批准产品模式（下节），尚未批准本补充文档全文。** 后续顺序：builder-check → 用户审阅本文 → 更新实施计划 → 实施。本文不是实施计划，不是任何已有字段/API 已存在的声明。 / DRAFT. The user approved the product MODE, not this full text. Not an implementation plan; names below are conceptual boundaries, not existing APIs.

## 1. 已决定的产品模式 / Decided product mode

弱来源（SMS/PSTN）发来的对话进入**同一个 Core 的公共上下文会话**，立即可用：只有隔离的电话通信历史和公共信息；不自动带入个人记忆、文件、人格；涉及私人信息或敏感动作时，再经**单独的可信审批**。“每段对话预先批准”的方案已被拒绝。不引入其他云端 Agent，不新增访客 Agent/工作区充当第二个大脑。 / Weak SMS/PSTN origins converse with the same Core in a public-context mode, immediately usable; private info and sensitive actions need separate trusted approval; per-conversation pre-approval, a second cloud agent and a guest agent/workspace are rejected.

## 2. 公共上下文 = 数据访问上限 / "Public" is a data-access ceiling

1. “公共”不是公开发布消息，也不是第二个 LLM。仍是同一 Core、同一模型、同一运行循环；输入仅为：显式声明为公共的通用核心提示 + 当前来电/短信文本 + 隔离的通信历史。 / Same Core, model and run loop; input is a generic explicitly-public core prompt, current caller text and isolated telecom history.
2. 默认不加载：所有者 SOUL/USER/profile/identity 额外文件、工作区指令、skills、hooks、全局记忆/wiki、路由召回、goals、agent 私有角色；不暴露主机路径与运行时密钥。 / Owner persona, workspace instructions, skills, hooks, global memory/wiki, routing recall, goals, agent-private role, host paths and secrets are excluded by default.
3. 私有读取必须在**历史/hook/编排提示词组装之前**被跳过，而不是事后字符串过滤，也不是仅把 `memory_mode` 设为 off（预检已证实 off 不覆盖 identity 文件、routing recall 等其他消费者）。 / Skip private READS before assembly; neither post-hoc filtering nor memory-off suffices.
4. 数据来源白名单是确定性的安全边界，**不是意图路由，不替代 LLM 推理**（R7）；模型仍在已授予权限内自行判断意图和选择工具。 / A deterministic source allowlist is security, not intent routing (R7).

## 3. 来源与模式的归属 / Provenance and mode ownership

1. 先遵守基线的已登记本人寻址、同意与绑定准入，再由 Core 根据**实际注册的 SMS/PSTN 传输**及已校验的绑定/租约盖章。未知/隐号或无法确定租户的输入不因“公共”模式而获得默认路由；载荷中的手机号、配对记录、`role=config`、metadata、`approval_reference` 都不能提升权限。 / Enrolled-owner routing and consent precede mode selection; unknown callers cannot acquire a tenant route and payload fields cannot promote.
2. 在已知电信入口，缺失或不支持的来源/限制标记必须拒绝执行，绝不退回普通 owner 路径；已校验准入的弱来源才进入公共模式。既有非电信渠道行为**不变**。 / Missing telecom provenance denies execution, never falls back to the legacy owner path; admitted weak origins use public mode.
3. 该限制需沿 `RunRequest` → flow/envelope → 提示词/工具 → 会话持久化/重建 全程携带，并在任务派生、会话复用处不丢失。不改 `src/harness`（R10），不新增 `ScopeId` wire 变体，不伪造 Personal/Project scope，不把 `None` 当作隐私拒绝（预检：`None` 恰是不受限的 legacy owner）。 / Carry the ceiling end-to-end; no harness change, no new ScopeId variant, no fabricated scope.
4. 以上是概念边界；现有哪些字段可承载留待实施调研。 / Carrier fields are implementation work.

## 4. 隔离的访客历史 / Isolated telecom history

1. 会话键由：权威绑定 + Core + 区域 + 规范化来电方身份 派生。同一绑定的 SMS/语音地址只是**别名**，不产生不同主体；同一规范化来电方的两种地址可访问**已完成的公共历史**。新短信运行/新电话开始时取得已完成历史的快照；电话结束前不把其回合发布到共享历史，不刷新活跃运行的快照，结束后的公共历史才供后续运行读取。绝不使用通用 `DmScope::Main/PerPeer` 所有者历史（预检：`agent:{id}:main`/`dm:{peer}` 不区分绑定与 Core）。 / New SMS runs/calls snapshot completed public history; active calls do not publish turns or refresh another active run's snapshot. SMS/voice aliases share a principal, never owner history.
2. “绑定”以基线设计中的托管控制面为权威，该独立服务尚未实现/验收；本地配置字符串不构成证明。来电方使用严格 E.164，不猜测默认国家；别名仅由权威绑定声明。撤销/重新绑定必须隔离旧历史（新的绑定身份或 incarnation）；`connection_generation` 仍按 Core 连接租约递增，不冒充绑定代次。手机号身份**始终是弱保证**。 / Planned hosted authority, not local strings; strict E.164 and authoritative aliases; rebinding invalidates old history while lease generation remains distinct.
3. 公共会话历史加密存储。Lead 提案：初始正文 TTL **24 小时**（沿用短时消息期限，非此前已批准的 Core 历史数值）；可降低，`0` 表示不持久化跨轮历史。每会话至多 **32 个已完成回合 / 64 KiB 正文**，全 Core 至多 **8 MiB 加密载荷**；以先到的界限为准。只淘汰完整旧回合，不拆 tool-call/result；不能满足存储额度时拒绝持久化/新准入，不转存普通历史，不偷偷延长期限。SQLite/WAL/备份的额外物理占用需另行设限；逻辑删除不等于物理擦除。上述数字随本文一起供用户审阅。 / Proposed defaults: 24h, optionally lower or zero; 32 complete turns/64KiB per session and 8MiB encrypted payload per Core; no spill to normal history. Physical storage bounds and erasure semantics remain separate.
4. 公共运行禁用：所有者记忆写入/抽取、压缩溢出（compaction spill）、全局缓存。 / Disable owner memory writes, extraction, compaction spill and global caches for public runs.
5. 已批准的私有读取只允许指定资料进入**当轮内部推理**，不允许直接发送。模型生成的回答先作为待披露草稿，不播放、不发送、不进入公共历史；运营者必须在可信控制面看到完整有界的最终正文/语音对应文本，再以第二次、精确绑定的审批授权该正文向当前绑定和当前通话/短信回合披露。若只批准读取而没有披露批准，动作安全失败。 / Read approval only permits the specified source into the current-turn internal reasoning. The generated answer is a disclosure draft: no send, playback or public-history write. A trusted operator must review the complete bounded final text and separately approve disclosure to the current binding and turn.
6. 披露审批包含原始输入/完整有界正文、接收绑定、通道、connection generation/turn 及期限；生成期间、等待批准期间和播放/发送前再次校验撤销、挂断、代次变化、Core 断线与预算。审批到期、撤销或打断时，停止尚未提交的短信和未播放的语音；已提交/已播放效果不可假装撤回。私有结果、草稿和正文**不得在下一轮变成未经批准的回放**：公共历史只留固定脱敏事件标记；权限来源只存受限账本；**不做语义内容分类**。 / Disclosure approval binds exact input, bounded body, recipient, channel, generation/turn and deadline; revalidate before effect. Expiry, revocation or interruption stops unsent SMS and unplayed audio; committed/played effects cannot be undone. Private results and drafts never replay in a later turn; history keeps only provenance and a redacted event marker.
7. 私有数据一旦进入回合，该回合随后产生的模型输出/工具内容均按私有来源处理，不让模型自行判定“已经不敏感”。该整个回合（包括用户原始问题）在公共历史中仅保留固定不可展开标记，不包含操作名、目标、成功状态或私有细节；追问不得自动回放原问题或回答，需重新提供问题并审批；精确效果账本留在受限控制面/执行层用于防重复和对账，不作为下轮公共 prompt 的例外入口。下轮需再审批才能读取该账本的私有细节。 / A private-source turn's downstream content remains restricted by provenance. Public history has only an opaque fixed marker; a separate restricted effect ledger prevents duplicate execution without leaking into future public prompts.
8. 若某私有来源/日志/持久化的隔离无法证明，则拒绝该动作，而不是解锁全局上下文。 / If isolation is unproven, deny the action; never unlock global context.

## 5. 审批：普通回复与主动发送分离 / Approvals

1. 普通回复（对已绑定本人同通道回复）与主动发送/外呼是两件事；前者不提升权限（沿用基线 §5.6）。没有可达的可信审批通道时，需审批的动作一律 fail-closed；普通公共问答可继续，停止/挂断/撤销始终可达。 / Replies and proactive sends are separate; unavailable trusted approval denies the action, not public conversation or off-switches.
2. 现有 `ExecApprovalManager` 是单次飞行 oneshot，**不是令牌授权存储**；`GrantStore` 为 session/persistent，无 once/TTL。不得复用通用授权缓存、guardian 自动审批或不透明引用。直接复用**仅限运营者的已认证控制面请求器**（`operator_requester`），绝不走同一弱通道。 / Reuse the authenticated operator-only requester directly; not the grant cache, guardian auto-approval or an opaque reference.
3. 审批分为两个不可互换的阶段：A) 私有读取批准，绑定精确来源、查询/原始输入、当前回合和期限；B) 披露批准，绑定读取后生成的**完整有界最终正文/语音文本**、接收绑定、通道、generation、当前回合和期限。普通公共聊天不需要这两类批准。每次批准**一次性**；电信场景有限时（SMS ≤ 300 秒，通话 ≤ 60 秒）；取消即丢弃/`retire` 等待者；A 的批准不自动授予 B。await 之后、模型生成完成后、发送/播放前及外部副作用边界处**重新校验**。其他渠道现有“有人值守无限等待”政策不变（`approval/mod.rs:139-161`），电信 TTL 仅是范围限定的扩展。 / Separate read and disclosure approvals: A permits exact private input for this turn; B binds the final bounded text/audio transcript and effect. Public chat needs neither; A never grants B. Revalidate after generation and before effect; attended-channel waits stay unchanged.
4. 每阶段期限从该请求创建起计，审批通过不重置期限；A 的有效性覆盖指定资料的读取和当轮生成，A 到期则停止私有生成、丢弃草稿/私有上下文并取消尚未执行的 B，不自动续期。B 另有有限期限，但不能延长 A；披露必须同时满足 A 的数据使用期限和 B 的效果期限，语音播放剩余部分也持续受这两个期限约束。到期后想继续须重新请求，不复用旧数据/草稿。这使私有电话回答可能需要更快生成/审批，不能用延长许可掩盖延迟。 / Deadlines start at request creation, never reset on approval. Read/data-use validity survives only within A's deadline; B has its own deadline but cannot extend A. Remaining playback must stay within both. Expiry discards drafts/private context; a new request is required.
5. `resolved_by` 自由字符串只是审计信息，非权限。拒绝渠道内 `/approve`/回调旁路（预检：`inbound_router/mod.rs:1053-1132` 当前可达 `resolve_for_session`）。 / `resolved_by` is audit only; reject channel `/approve` bypass.
6. 审批针对单个动作与披露意图，不等于手机认证，也不提升为所有者。私有读取许可只放行所请求的数据/动作/结果，且仅当前回合；不放行人格/私有历史引导。 / Per-action; not authentication or owner promotion.

## 6. 运营者可见性与副作用语义 / Operator display and effect semantics

1. 对主动动作及私有披露审批，运营者必须看到**完整（有界）的收件人/动作/消息/最终正文**，而非仅 200 字符摘要或哈希；读批准阶段显示精确来源与查询。此为需求；现有渲染/schema 是否支持**未验证，属实施工作**。 / The operator sees the complete bounded action, source/query, recipient and final disclosure body; renderer/schema integration is implementation work.
2. 正文只在已认证控制面展示，不泄露到访客通知；通话在披露批准前不得进入可播放队列。 / Bodies appear only on the authenticated control plane; call audio cannot enter playable custody before disclosure approval.
3. 对已提交给运营商的效果不做虚假“取消”；保留 accepted/submitted/unknown 结果；重试需新的操作与新的审批。 / No fake cancellation after submission; retries need new approval.

4. Lead 提案：进入私有审批时仅向访客发一次固定公共状态提示“需要在可信控制面确认，当前尚未发送私人信息”；等待期间电话保留连接并允许挂断，不播放私有内容，不周期性发送收费短信。拒绝/超时只发固定“本次请求未完成，可继续普通聊天”，不说明来源、审批阶段或私人执行状态。这些状态提示不等于披露批准；公共停止/控制状态输出不受该回合私有草稿阻塞。 / Proposed UX: one generic pending notice, keep the call open and cancellable; denial/timeout gets a generic unavailable notice. No periodic paid SMS or private detail; operational status is not disclosure approval.

## 7. 分阶段的工具限制 / Staged tool restriction

在后台任务/subagent/MCP/扩展/工具的私有来源被审计并继承此上限之前，弱回合**不得**走这些路径。初始隔离切片默认不放行尚未证明安全的工具；这只说明切片边界，不能冒充完整 Task 4。普通回复走既有输出路由，不靠启用主动发送工具。完整功能至少须完成独立审批的私有读取/主动联系链路与效果验收；经审计的公共数据工具可按既有注册机制加入。新增未知工具/来源默认拒绝。公共安全工具须有肯定的披露/数据访问契约，不按工具名或内容启发式选择。 / Unproven tools are disabled in the initial slice, not silently removed from full acceptance; replies use the existing output route. Complete Task 4 requires approved private-read and proactive-contact effects. Unknown sources default-deny.

## 8. 验收测试（在真实边界）/ Acceptance tests at real boundaries

在**实际 provider 输入、副作用与存储**处断言，而非仅检查标记/调用次数：

1. 私有 canary 覆盖每个来源（历史、memory、profile/identity、routing recall、hooks、skills、goals、wiki、缓存）均不进入 provider 输入。
2. 同一手机号跨 Core/绑定：互不共享，且无所有者历史。
3. 普通渠道行为不变。
4. 伪造 role/ref/自审批被拒。
5. 读取与披露各自一次性动作：超时/取消/撤销/竞态；披露前最终正文必须与获批正文精确匹配。
6. 私有结果不在下一轮回放，也不溢出到后台/压缩；跨通道只共享已完成公共历史，实时回合不互注。
7. 未知公共来源默认拒绝。
8. 离线时不出现替代 Agent。

**完整的来源/重建/缓存普查尚未完成，必须作为生产前最终闸门。** 原 G1–G5 保持；当前禁止真线路验收。 / Full source/rebuild/cache census is INCOMPLETE and is the final production gate; G1–G5 preserved; real-line acceptance still forbidden.

## 9. 被拒绝的方案 / Rejected alternatives

| 方案 | 拒绝理由 |
|---|---|
| 另建访客 Agent / 仅换工作区 | 不符合本次“不另起 Agent”的产品选择；单换目录不能隔离全局读取。同 Core 内多 Agent 本身不必然违反 R6，但另建独立云脑会违反唯一 Core 约束 |
| 伪造 Personal/Project scope | 预检：Personal/Project 的 `session_read_ids` 仍读取 base 分区；`None` 是不受限 legacy owner；scope 变体不是隐私拒绝 |
| 仅 `memory_mode=off` | 不覆盖 identity 文件、extra 文件、routing recall、历史、hooks（`prompt_build.rs`、`runner_impl.rs`、`inner.rs` 的实证顺序） |
| 每段对话预批准（方案 B） | 用户明确拒绝；首次使用即阻塞，违背“立即可用” |

## 10. 未实现/未验证与待定 / Not done, unverified, open decisions

- **未实现**：全部；无代码、无测试、无生产启用。本文是设计草案。
- **未验证**：提示词/来源全量普查、重建路径、缓存、secondary prompt writers、gate-chain 默认值/guardian、operator 请求器是否能展示完整正文、会话持久化加密/留存可行性、后台/subagent/MCP 来源。预检的 agent 报告为 PARTIAL，非端到端证明。
- **Lead 提案，供用户审阅**：§4 的 24 小时 / 32 回合 / 64 KiB / 8 MiB 边界、整个私有回合（包括原始问题）不留公共正文，以及 §6 固定等待/未完成提示；公共核心提示由 Core 维护为显式公共模板，不读取用户私有人格（措辞属实施细节）。严格 E.164、权威别名、不可达审批 fail-closed 沿用既定边界，不重复作为未决选择。最终审阅前不得把这些新增数字记为用户已批准。
- **已由用户选择、仍待本文整体批准**：私有读取与最终披露分开批准；已完成的公共历史可在同一绑定/规范化来电方的 SMS 与电话之间互通，活跃通信回合不互注；私有结果始终不自动复用。
- **本文未做**：不改现有 spec/plan/报告、不 stage/commit、不声称产品成功。
