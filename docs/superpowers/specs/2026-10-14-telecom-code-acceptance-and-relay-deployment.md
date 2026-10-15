# 电信代码验收与 Relay 部署契约 / Telecom code acceptance and Relay deployment contract

日期 / Date: **2026-10-14**。范围 / Scope: `/home/zou/data/workspace/Aleph-sms-pstn-sdd`。

**状态：增量范围契约；Task15/16 离线 Core＋契约验收已完成，生产验收延后、未验证。**
**Status: additive scoped contract; Task15/16 offline Core + contract acceptance complete; production acceptance deferred and unverified.**

## 1. 授权与判定 / Authorization and verdict

用户明确授权：尚无 SMS 服务账户、不做真实电话，也可以完成当前 SMS/PSTN **CODE acceptance**。
The user authorizes current SMS/PSTN **CODE acceptance** without an SMS service account or live calls.

后续范围确认：用户选择 **Core＋契约验收**，本次交付 Core、五个电话工具、权限／一次性审批／预算／控制桥及 fake 端到端；明确不交付可部署的托管网络 Relay 服务器或具体供应商 webhook／媒体适配。这是用户确认的实现边界，不是从“无账号”自行推导的豁免。
Subsequent explicit scope confirmation: the user selected **Core + contract acceptance**. This delivery includes Core, five lifecycle tools, authority/one-shot approval/budget/control integration and fake E2E. A deployable hosted network Relay and concrete provider webhook/media adapters are excluded from this delivery, explicitly by user choice rather than inferred from the absence of an account.

**外部生产 G1–G5 不阻塞当前 Task15/16 代码验收；Core 安全与 fake 集成缺口仍阻塞。**
**External production G1–G5 do not block current Task15/16 code acceptance; Core safety and fake integration gaps still do.**

代码通过不等于托管 Relay 已部署、手机已收件、电话可听、地区已支持或可以发布。
Code PASS does not establish hosted deployment, handset delivery, audible calls, regional support or release permission.

本文只追加此验收/依赖边界，不重写原设计、addendum、计划或历史报告，不降低未来完整产品的发布约束。
This document adds only acceptance/dependency boundaries; it does not rewrite protected design, addenda, plans or reports, or weaken future full-product release requirements.

方案：同一 Core 的隔离文本运行接入薄 Relay；离线契约先验收，网络权威与运营商效果另验收。
Approach: connect thin Relay transport to isolated text execution in the same Core; accept offline contracts first and network authority/carrier effects separately.

代价：默认无工具/私有语音、单份缓冲输出降低攻击面，但暂不支持低延迟流式语音；真实远端不可逆边界仍须独立证明。
Trade-off: no default tools/private voice and one buffered output reduce attack surface but defer low-latency streaming; remote irreversible boundaries still need independent proof.

## 2. 来源与当前事实 / Sources and current facts

下列均为本工作树的精确相对路径；以当前源码、最新报告及其有界证据为准，旧 handoff 不是状态权威。
Paths below are exact relative paths in this worktree; current source, latest reports and bounded evidence govern, not an old handoff.

| 已有来源 / Existing source | 边界 / Boundary |
| --- | --- |
| `docs/superpowers/specs/2026-10-09-pstn-sms-thin-relay-design.md` | 原产品范围和 G1–G5 / Original scope and production gates. |
| `docs/superpowers/specs/2026-10-09-telecom-public-context-addendum.md` | 公共上限、历史、A/B / Public ceiling, history, A/B. |
| `docs/superpowers/plans/2026-10-09-pstn-sms-thin-relay-implementation.md` | 受保护原计划，本文不改 / Protected original plan, unchanged here. |
| `shared/protocol/src/telecom_relay.rs` | 唯一 versioned wire 契约 / Single versioned wire contract. |
| `src/bin/aleph-server/commands/start/mod.rs` | 条件构造 owner-memory wiring；不是 Relay 权威 / Conditional owner-memory wiring, not Relay authority. |
| `src/gateway/interfaces/sms/mod.rs` | SMS factory 注册存在；未挂外部生产 transport / Factory exists, no attached external production transport. |
| `src/gateway/interfaces/sms/transport.rs` | `SmsRelayPeer`、`SmsRelayTransport`、`PrivateSmsLease`、提交栅栏 / Peer, transport, lease and submission fences. |
| `src/gateway/inbound_router/mod.rs` | 入站路由边界 / Inbound routing boundary. |
| `src/gateway/telecom/public_context.rs` | `TelecomPublicContext` 与 typed provenance / Typed public provenance. |
| `src/gateway/telecom/public_run.rs` | 公共运行 driver / Public execution driver. |
| `src/gateway/telecom/public_admission.rs` | 同绑定准入与 `PublicRunPermit` / Binding admission and permit. |
| `src/gateway/telecom/public_history.rs` | 隔离加密已完成历史 / Isolated encrypted completed history. |
| `src/gateway/telecom/public_history_startup.rs` | 独立历史配置与密钥接线 / Dedicated history configuration/key wiring. |
| `src/gateway/telecom/private_turn.rs`、`src/gateway/telecom/owner_memory_read.rs` | 现有私有 SMS driver、只读 owner memory / Existing private SMS driver and read-only owner memory. |
| `src/gateway/telecom/public_approval.rs`、`src/gateway/telecom/operator_control.rs` | A/B 账本与可信操作员控制面 / A/B ledger and trusted operator control. |
| `src/orchestrator/dispatch.rs` | `PublicRunRequest`、`HarnessRunner::run_public_text` / Narrow request and execution seam. |
| `src/orchestrator/harness_bridge/public_run.rs` | 同一 Core/provider 的隔离实现 / Isolated execution in the same Core/provider. |
| `src/gateway/interfaces/voice_call/protocol.rs` | 已有纯 reducer / Existing pure reducer. |
| `src/gateway/interfaces/voice_call/transport.rs` | 已有 `OfflineCallTransport`，命令不是播放 / Offline transport; commands are not playback. |
| `src/gateway/interfaces/voice_call/relay.rs`、`coordinator.rs`、`relay_tests/` | 语音接缝、受控同一 Core coordinator 与 fake 验收已完成 / Voice seam, controlled same-Core coordinator and fake acceptance complete offline. |
| `src/gateway/telecom/lifecycle_tools.rs`、`lifecycle_bridge.rs`、`src/builtin_tools/call_lifecycle.rs` | 五工具实际 registry、operator once、permit/control 与清理/预算栅栏 / Actual registry, operator once, permit/control and cleanup/budget fences. |
| `tests/voice_call_protocol_test.rs` | 既有离线 reducer/transport 测试 / Existing offline lifecycle tests. |
| `tests/sms_contract_test.rs`、`tests/sms_delivery_policy_test.rs`、`tests/sms_fixture_test.rs` | 既有 SMS 离线边界测试 / Existing SMS offline boundary tests. |

最新验收记录：`.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-16-code-acceptance-matrix.md`。
Latest acceptance record: `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-16-code-acceptance-matrix.md`.

当前主 Agent 新鲜验收：Core 定向 **449 passed / 0 failed**，共享协议 **7 passed / 0 failed**；`cargo check` 与 `cargo clippy --lib --tests --bins` 通过、有警告。449 项中包含 58 项 Relay/coordinator/lifecycle、8 项实际 registry E2E 与 3 项补充边界测试，后两者均为 58 项的子集，不重复计数。
Current fresh lead verification: **449 Core targeted passes / zero failures**, **seven shared-protocol passes / zero failures**; check and clippy passed with warnings. The 449 include 58 Relay/coordinator/lifecycle cases, of which eight are actual-registry E2E and three supplemental boundary cases; subsets are not counted twice.

精确命令、临时目录修复前失败日志、最终通过日志、19 个选定源码/manifest 哈希与保护文档完整 SHA-256 已持久归档于 `task-15-16-offline-validation/`。当前未提交工作树才是验收快照；定向通过不证明全库或生产通过。
Exact commands, first temporary-directory failure logs, final successful logs, 19 selected source/manifest hashes and protected-document SHA-256 checks are durably archived in `task-15-16-offline-validation/`. The uncommitted worktree is the accepted snapshot; scoped passes do not prove full-library or production success.

以下四份当前报告均位于 `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/`，已按当前源码校准：
These four current reports were reconciled against source under `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/`:

- `task-5-voice-offline-report.md`：离线 reducer 已有；coordinator/准入/历史不在其完成范围。 / Offline reducer exists; coordinator/admission/history are outside its completed scope.
- `task-13-sms-offline-validation-report.md`：SMS 局部通过；缺生产 adapter 属后续 readiness。 / Scoped SMS PASS; production adapter absence is later readiness.
- `task-14-owner-memory-offline-verification.md`：owner-memory A/read/draft/B 有实现；其“启动始终 None、19 仅规划”是旧检查点。 / Owner-memory A/read/draft/B exists; “startup always None / 19 only planned” is an older checkpoint.
- `task-19-live-lease-report.md`：Core live witness/guarded submission 已实现；不证明远端权威。 / Core live witness/guarded submission exists; remote authority is not proven.

四份前期规划输出若仍写“Task14 未做/无私有 owner-memory/无 Core live lease”，不得原样照搬；历史缺口须按上述来源重分类。
Any of the four earlier planning outputs still claiming missing Task14, owner-memory or Core live lease are stale; reclassify gaps against these sources rather than copying them.

当前 startup 在配置启用且 trusted operator/memory 可用时构造 `PrivateTurnWiring`；否则关闭。配置不授予 live witness。
Current startup constructs `PrivateTurnWiring` only with enabled configuration and trusted operator/memory; otherwise disabled. Configuration grants no live witness.

## 3. 实现与外部依赖矩阵 / Implementation and external dependency matrix

| 项 / Item | Core 状态与责任 / Core status and ownership | 外部责任 / External ownership |
| --- | --- | --- |
| 公共隔离 / Public isolation | 已有 typed context、同一 runner；默认无工具 / Existing typed context, same runner, no tools by default. | 已认证来源、正确绑定事实 / Authenticated origin and binding facts. |
| SMS 公共收发 / Public SMS | 已有离线协议/通道/fake 栅栏 / Existing offline protocol/channel/fake fences. | 生产连接、运营商收发与送达事实 / Production connection and carrier delivery facts. |
| 私有 SMS A/B / Private SMS A/B | commits `fa0562ed0` / `4f5877535`：owner-memory-only + live `PrivateSmsLease`，Core 离线已实现 / Implemented offline Core safeguards and live lease. | attach peer、远端租约与不可逆写入重验 / Attached peer, remote lease and final-write revalidation. |
| Voice reducer / 电话 reducer | 已有 committed/epoch/播放事实/副作用账本 / Existing committed/epoch/playback/side-effect state. | 精确供应商事件与实际 clear / Exact provider events and actual clear. |
| Voice peer/transport/coordinator / 语音接缝 | **OFFLINE COMPLETE**；受控 coordinator 经实际接缝 fake 验收 / Controlled coordinator accepted through actual seams with fakes. | 网络认证与媒体适配 / Network authentication and media adaptation. |
| Voice admission/history / 语音准入/历史 | **OFFLINE COMPLETE**；整通话 permit、精确播放历史、撤销/取消/预算/有界清理已验收 / Whole-call permit, exact played history and bounded cleanup/cancellation/budget accepted. | 共享 binding/incarnation 权威 / Shared binding/incarnation authority. |
| 私有语音 / Private voice | 本切片关闭，不以 SMS A/B 冒充 / Disabled in this slice; SMS A/B is not voice approval. | 未来单独设计、授权、验收 / Separate future design, authorization and acceptance. |
| Task16 fake E2E | **OFFLINE COMPLETE**；五工具实际 registry→operator→fake peer→隔离 Core 效果及回归通过 / Actual registry/operator/fake peer/Core effects and regressions passed. | 无需账户/真线路 / No account/live line required. |
| G1–G5 与发布 / Gates and release | 本次延后，不阻塞代码 / Deferred, not code blockers. | 运营者、供应商、用户另行授权 / Separate operator/provider/user authorization. |

## 4. Task15 新语音切片契约 / New Task15 voice slice contract

本节契约已在离线 Core＋fake 范围验收完成，依据为最新矩阵、实际运行日志及独立源码复核；不代表网络/媒体/运营商效果通过。
This contract is accepted within offline Core + fake scope by the latest matrix, executed logs and independent source review; network/media/carrier effects remain unverified.

1. **唯一大脑 / Sole brain.** authenticated ingress → typed `TelecomPublicContext` → 同一隔离 `HarnessRunner::run_public_text`。
   Authenticated ingress reaches typed public context and the same isolated runner; no second agent/model control plane.
2. **输入上限 / Input ceiling.** 只含公共提示、当前 committed transcript、隔离已完成历史；无 owner session/workspace/profile/SOUL/hooks/private recall。
   Only public prompt, committed transcript and isolated completed history; no owner-session or private-source fallback.
3. **默认无工具 / No tools by default.** voice 固定不开放 private lookup/私有草稿；payload 中 role、metadata、approval reference 不提升权限。
   Voice enables neither private lookup nor private drafts; payload roles, metadata and approval references confer no authority.
4. **新接缝 / New seam.** `VoiceRelayPeer` / `VoiceRelayTransport` 只承载 authenticated facts、事件和命令；可信 embedder 注入身份/策略。
   The peer/transport carries authenticated facts, events and commands; a trusted embedder injects identity/policy.
5. **信任边界 / Trust boundary.** typed facts 是应用内契约，不是密码学证明；fake 证明本地拒绝与效果栅栏，不证明 TLS、供应商签名或远端身份。
   Typed facts are an in-process contract, not cryptographic proof; fakes prove local fences, not TLS/provider signatures/remote identity.
6. **局部权威 / Local authority.** 单一本地 connection generation 与可撤销 call lease；同一 transport 不并存第二 active call，不把本地 generation 当 binding incarnation。
   One local connection generation and revocable call lease; no second active call in the same transport; local generation is not binding incarnation.
7. **租约失效 / Lease invalidation.** revoke/disconnect/hangup fencing、旧 generation/旧 call/晚到事件拒绝；旧运行不得换取新 lease 后继续提交。
   Fence revocation/disconnect/hangup and stale generation/call/late events; an old run cannot reacquire a new lease to submit.
8. **有界 coordinator / Bounded coordinator.** 临时已完成回合最多 **32 / 64 KiB**，整通话最多 **10 分钟**，runner **120 秒**、submit **30 秒** 上限。
   Transient completed turns: **32 / 64 KiB**; call lifetime **10 minutes**; runner **120 seconds**, submission **30 seconds** maximum.
9. **整通话 permit / Whole-call permit.** 同一 binding 的 `PublicRunPermit` 从通话准入持有至关闭/所有退出路径释放，不能每 turn 重取制造并发洞。
   Hold the same binding permit from call admission through closure and release on every exit; no per-turn reacquisition hole.
10. **缓冲输出 / Buffered output.** 每 turn 一份有界完整文本输出，`output_sequence = 1`；生成/提交/queued/played 分开，不假装流式媒体已落地。
    One bounded complete text output per turn at sequence 1; generated/submitted/queued/played are distinct, not a streaming-media claim.
11. **历史时点 / History timing.** 开始时取得已完成 caller 快照；只有 closure 后才发布本通话的精确 played prefixes，未播放生成内容不写历史。
    Snapshot completed caller history at call start; publish exact played prefixes only after closure, never unplayed generated text.
12. **跨 SMS/voice / Cross-channel history.** 同一权威 binding/caller 共用已完成历史；活跃通话回合不被 SMS 或其他活跃运行看到，也不刷新其既有快照。
    Share completed history for the same authoritative binding/caller; active call turns stay hidden from SMS/other active runs and do not refresh their snapshots.
13. **打断 / Barge-in.** 取消旧输出 epoch、阻止晚到文本/播放；取消请求不等于队列 clear，必须等待实际 clear 事实。
    Cancel the old output epoch and fence late text/playback; a cancel request is not queue-clear evidence.
14. **副作用 / Side effects.** 打断不回滚已发生工具效果；现有账本保留事实。此切片无工具，不因此声称未来工具路径已验收。
    Barge-in cannot roll back committed tool effects; the ledger preserves facts. Tool-free scope does not accept future tool paths.

历史存储已有 24h TTL（0 禁持久化）、32 回合/64 KiB 会话上限、8 MiB 全 Core 加密载荷；只淘汰完整回合，损坏/容量不足 fail-closed。
Existing store: 24h TTL (0 disables persistence), 32 turns/64 KiB per session, 8 MiB encrypted Core-wide payload; evict complete turns only and fail closed on corruption/capacity.

这些存储上限不等于通话期限；binding 重绑须隔离旧历史，不能仅用 connection generation 冒充重绑身份。
Store limits are not call deadlines; rebinding must isolate old history and cannot use connection generation as rebinding identity.

## 5. 现有私有 SMS：保留而不夸大 / Existing private SMS: preserve without overclaiming

`fa0562ed0` 的 owner-memory-only 读取与 `4f5877535` 的 live-witness/send fence 是已实现的 Core 离线能力，不再列为缺失 Task14 功能。
Owner-memory-only reads in `fa0562ed0` and live-witness/send fences in `4f5877535` are implemented offline Core capabilities, not missing Task14 work.

A 批准精确来源/查询/本回合原始输入后只读 owner memory；隔离无工具草稿经 B 批准完整精确最终正文后才可提交。
A binds source/query/current input before read-only owner memory; an isolated tool-free draft needs B approval of the exact full final body before submission.

同一 live `PrivateSmsLease` 贯穿 selection、A/read、draft、B/send、固定通知；最后锁等待后重验正文、scope、epoch、取消和 A/B 原始期限。
The same live lease spans selection, A/read, draft, B/send and fixed notices; recheck body/scope/epoch/cancellation/original A/B deadlines after final lock waits.

短信 A/B 创建起最多 300s，不续期；无 witness 禁私有读取，公开聊天仍可达；无公开 send fallback、队列或自动重试。
SMS A/B lasts at most 300s from creation without renewal; no witness means no private read, while public chat remains reachable; no public-send fallback, queue or auto-retry.

整个私有回合（含问题）在公共历史只留 opaque marker；ACK 不等于手机收件，Core handoff 不等于远端不可逆写入。
A private turn including its question leaves only an opaque public-history marker; ACK is not handset receipt and Core handoff is not remote irreversible write.

factory 仍只构造未附生产 transport 的 `SmsChannel`；条件 startup wiring 不等于已连接生产 peer，也不开放私有语音。
The factory still constructs an unattached channel; conditional startup wiring is not a connected production peer and does not enable private voice.

## 6. 外部 Relay/供应商部署依赖 / External Relay/provider deployment dependencies

**本节全部 DEFERRED，另行授权；不选定供应商，不采购、不开户、不发短信、不拨号。**
**All of this section is DEFERRED and separately authorized; no provider selection, purchase, account opening, SMS or calls.**

| 外部交付 / External deliverable | 所有权与后续证据 / Ownership and later evidence |
| --- | --- |
| 网络 peer / Network peer | 部署方实现 Core 主动连接、网络认证/TLS、凭据 provision/rotate/revoke；旧凭据/租约在最后写入边界失效。 / Deployer owns outbound Core connection, authentication/TLS, credential lifecycle and final-write revocation. |
| 权威单一真源 / Single authority | 运营控制面持久保存跨 SMS/voice 的 shared binding + incarnation + lease authority；不能两个 adapter 各维护一份权威。 / Operator persists one shared authority, not separate SMS/voice sources. |
| 重绑/重启 / Rebind/restart | 原子传播吊销，跨进程/重启拒绝旧 incarnation、隔离历史与审批；本地 fake generation 不证明此效果。 / Propagate revocation atomically, reject stale incarnations across restart and isolate history/approvals. |
| provider 验签 / Provider verification | 外部 ingress 按供应商契约校验精确 raw URL/body/form 与 timestamp，处理代理 URL 还原；解析后的 JSON 不能替代签名原始字节。 / Verify exact raw URL/body/form and timestamp, including proxy URL reconstruction; parsed JSON is not signed bytes. |
| durable-before-ACK / 先持久化再 ACK | 验签/防重放与最小去重 metadata 必须 durable-before-ACK；崩溃重放不得产生重复效果，敏感正文不冒充必要 metadata。 / Persist verified minimal dedup metadata before ACK; crash replay cannot duplicate effects. |
| dedup 保留 / Dedup retention | **7 天仅提案**，须按 provider 最大重放/重试 SLA 确认，再批准 retention；不宣称已满足任意供应商。 / Seven days is a proposal pending maximum replay/retry SLA confirmation. |
| unknown / 不确定结果 | 未知提交/拨号效果不自动重试或自动重拨；有界保留、人工/权威对账，重新操作需新授权。 / No automatic retry/redial; bounded retention and authoritative reconciliation, fresh authorization for a new operation. |
| ASR/TTS/media / 语音媒体 | 媒体、SIP/RTP/WebRTC、ASR/TTS 在 Core 外；只返回转写与传输事实，不加第二大脑/工具/记忆策略。 / Media and speech I/O stay outside Core and add no second reasoning/tool/memory authority. |
| cancel 与 clear / Cancel versus clear | 供应商接收 cancel 不证明实际停止；需事件与真线路测量证实 buffer clear、晚到音频停止及计费边界。 / Cancel acceptance is not actual stopping; verify clear, late audio and billing boundaries. |
| Played / 已播放事实 | 绑定精确 call/turn/epoch/sequence 与精确文本前缀的 provider evidence；不能由 generated/queued 推导，也不证明人听见或理解。 / Exact provider-backed prefixes, not inferred from generated/queued and not proof of human hearing/comprehension. |
| 运营约束 / Operations | 用途/资质、同意/退订、费用预算、地区数据路径、留存/训练政策、日志脱敏、事故响应由运营者核验。 / Operator verifies eligibility, consent, budgets, regional flows, retention/training, redaction and incident response. |

`binding_ref`/caller 历史键依赖外部权威正确隔离 incarnation；Core 的 typed 构造、锁和取消 token 不替代远端持久权威。
History keys depend on correct authoritative incarnation isolation; typed constructors, locks and cancellation tokens do not replace persistent remote authority.

G1 大陆 SMS 用途/自由文本/主动上行；G2 大陆电话及真实打断；G3 明确海外国家清单及逐国核验。
G1: mainland SMS eligibility/free text/initiation; G2: mainland voice and real interruption; G3: explicit overseas country allowlist and country-by-country verification.

G4 运营/隐私/费用/数据路径；G5 绑定/租户隔离/吊销/审批/回调防伪防重放。均须后续单独授权、证据归档与验收。
G4: operations/privacy/cost/data flows; G5: binding/isolation/revocation/approvals/callback authenticity and replay protection. All need separate later authorization and archived evidence.

完整发布仍受原双地区效果闸门约束；不因当前 CODE acceptance 默默先发海外或改用大陆模板短信绕过用途限制。
Full release retains the original both-region effect gates; code acceptance authorizes neither silent overseas-first release nor template-SMS workarounds.

## 7. Task15/16 代码闭环证据 / Task15/16 code closure evidence

Task15 由 worker 完成接线，主 Agent 验收；Task16 必须用 fake 走实际 transport/coordinator/admission/runner/history/output 接缝。
Worker owns Task15 integration and the lead accepts it; Task16 must traverse actual transport/coordinator/admission/runner/history/output seams with fakes.

- [x] authenticated committed transcript 到达真实隔离 provider 输入；私有 canary、owner history、工具、私有语音入口均不可达。 / Prove isolated provider input and absence of private canaries/owner history/tools/private voice.
- [x] 错 binding/account/caller/call/generation、未知事实、缺 lease、撤销均 fail-closed，无 owner fallback；payload 不能制造权威。 / Reject mismatched/unknown/revoked facts without owner fallback or payload authority.
- [x] 同 binding permit 覆盖整通话；并发 SMS/另一电话、错误退出、超时、挂断均验证 permit 寿命和释放。 / Prove whole-call permit lifetime/release across concurrency and every exit.
- [x] 32 回合/64 KiB/10min/120s/30s 边界及取消/锁等待/peer await 后重验有可控时钟证据。 / Verify limits and post-await fences using controlled time.
- [x] 每 turn 只有 buffered sequence 1；generated/submitted/queued/played 分离，晚到旧 epoch 不送达 fake 输出边界。 / Verify single buffered output and distinct states, with late-epoch suppression at fake output.
- [x] barge-in cancel 不冒充 clear，不重放旧输出；unknown 不 retry；已提交副作用账本不被取消抹掉。 / Verify cancel/clear distinction, no replay/retry and preserved committed effects.
- [x] closure 前 SMS 看不到活跃回合；closure 后只看到精确 played prefixes；隔离/撤销/重绑/历史损坏不回退普通历史。 / Verify completed-only publication, exact prefixes and fail-closed isolated history.
- [x] 现有私有 SMS A/B、live witness、approved-body/deadline/cancel/opaque marker 与非电信路径不回归。 / Preserve private SMS fences and non-telecom behavior.
- [x] 定向测试、实际 check/review 结果按 commit、命令、日志、范围记录；局部 PASS 不冒充全库或生产 PASS。 / Record commit/command/log/scope evidence without widening scoped success.

当前 Task16 判定为 **OFFLINE COMPLETE**，依据为实际运行而不是 worker 文件存在。当前完整 workspace 未跑至全绿；历史完整库为 21,063 passed / 25 failed / 19 ignored，失败归因记录仍保留，不宣称干净基线。任务37和真实电话/SMS、生产 Relay/可信启动接线、G1–G5及发布维持待办。
Task16 is **OFFLINE COMPLETE** based on execution, not worker files appearing. The current full workspace was not run to green; the historical full-lib result remains 21,063 passed / 25 failed / 19 ignored with attribution retained, not a clean baseline. Task37, real lines, production Relay/trusted startup, G1–G5 and release remain pending.

## 8. 阶段、责任与停止条件 / Stages, ownership and stop conditions

| 阶段 / Stage | 所有者 / Owner | 出口 / Exit evidence |
| --- | --- | --- |
| A：当前 additive 文档 / Current document | 文档 worker + lead / Documentation worker and lead | 单文件、双语、保护原文档 / One bilingual file, protected originals preserved. |
| B：Task15 code / 代码 | worker + lead review | **OFFLINE COMPLETE**；受控同一 Core 接线、离线效果测试、独立复核通过 / Controlled same-Core integration, offline effect tests and independent review passed. |
| C：Task16 CODE acceptance / 代码验收 | 验证与主 Agent / Verification and lead | **OFFLINE COMPLETE**；当前 Core 449、共享协议7、check/clippy通过；证据归档 / Current Core 449, protocol seven, check/clippy passed and archived. |
| D：外部 adapter 部署 / External deployment | 运营者、Relay/供应商工程方 / Operator and Relay/provider engineers | 用户另授权后，网络/持久权威/验签/dedup/媒体证据 / Separately authorized network/authority/verification/media evidence. |
| E：G1–G5 与发布 / Gates and release | 用户审批、运营者执行、独立验收 / User approval, operations and independent acceptance | 真实双地区效果及全套生产约束；另行决定 release / Real both-region effects and production constraints; separate release decision. |

B/C 若存在 Core 安全或集成失败，保留 PENDING 并给出定位证据；D/E 未授权就停在依赖记录，不以开启网络服务“补齐”。
Keep B/C pending on Core safety/integration failures with localized evidence; stop D/E at dependency records without authorization, rather than enabling services to fill gaps.

未做：生产 adapter/可信启动接线、真实 SMS/电话/播放/费用/地区效果、G1–G5、stage/commit、merge/push/release、采购/开户/供应商选择或接触；未重写受保护设计/计划/报告。生产缺口单列于 `docs/superpowers/plans/telecom-real-line-verification-follow-up.md`（任务37）。
Not done: production adapters/trusted startup, real SMS/calls/playback/billing/regions, G1–G5, staging/commits, merge/push/release, purchases/accounts/provider selection/contact, or protected-document rewrites. Production gaps are retained in Task37's follow-up checklist.

残余风险：远端与本地锁不能原子合并；trusted embedder/同机信任不是恶意进程隔离；WAL/备份删记录不等于物理擦除；未提交工作树及选定源码哈希不是完整发布版本档案。
Residual risks: local locks cannot make remote effects atomic; trusted embedder/same-host trust is not hostile-process isolation; WAL/backup deletion is not physical erasure; the uncommitted worktree and selected source hashes are not complete release provenance.
