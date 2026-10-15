# Telecom real-line verification follow-up / 真实电话与 SMS 后续验证任务

**Status / 状态：OPEN — DEFERRED, UNVERIFIED / 待办、延后、未验证。**

Task 15/16 offline completion does not close this document. The user requested the maximum code/contract acceptance possible without registered telephone/SMS services, with real-line gaps retained for a later task.

任务 15/16 的离线完成不关闭本文任务。用户要求在未开通电话/SMS 服务的前提下完成可做的代码与契约验收，并保留真实线路缺口作为后续任务提示。

## Scope boundary / 范围边界

- Offline tests inject authenticated fake Relay facts and fake provider outcomes. They establish Core behavior for those inputs; they do not establish carrier truth, handset delivery, speech quality, actual billing or production authentication. / 离线测试注入 fake Relay 的认证事实与供应商结果，证明 Core 对这些输入的行为，不证明运营商事实、手机送达、语音质量、实际账单或生产认证。
- Saving SMS/PSTN configuration in Panel is a configuration operation. An unavailable runtime must remain unavailable until an authenticated implementation is attached. / Panel 保存 SMS/PSTN 配置只表示配置操作；未接入认证实现前，运行时必须继续显示不可用。
- Core public-spend admission and unknown-cost fences are tested. They are not a carrier monetary ledger or proof of telephone/SMS charges. / Core 公共支出准入及未知费用栅栏有测试；它们不是运营商金额账本，不证明实际电话/SMS 费用。
- No account subscription, purchasing, provider contact, numbers, live traffic, merge, push or release was performed or authorized by this offline closure. / 本次离线收尾未开通账户、购买、联系供应商、取得号码、发送真实流量、合并、推送或发布。

## Future work checklist / 后续任务清单

### F1 — Deployable Relay and trusted startup / 可部署 Relay 与可信启动接线

- [ ] Implement a separately hosted thin network Relay and concrete supplier webhook/media adapters. Keep routing, decisions, approval semantics and AI inference in the same Rust Core. / 实现独立托管的薄网络 Relay 与具体供应商 webhook/media 适配；路由决策、审批语义与 AI 推理仍在同一 Rust Core。
- [ ] Attach production transport and lifecycle service at authenticated startup; install explicit user-to-binding ownership from server configuration. Exercise actual server-minted run authority, not test capability issuance. / 在可信启动路径接入生产 transport 与 lifecycle service，使用服务端用户/绑定归属配置；验证实际服务端 run 权威，不以测试 capability 签发替代。
- [ ] Verify WSS/TLS, SSRF pinning, Relay credentials, binding/account/caller/call/generation correlation, heartbeat, reconnect, revocation and credential rotation against a real deployment. / 在真实部署验证 WSS/TLS、SSRF 地址钉住、Relay 凭据、绑定/账户/来电者/通话/generation 对应、心跳、重连、撤销及凭据轮换。
- [ ] Prove every provider adapter rechecks the exact lease/witness after each await and immediately before its irreversible write; no detached effect, implicit retry or arbitrary recipient selector. / 证明各供应商适配在每次 await 后及不可逆写入前复检 exact lease/witness；无脱离任务的副作用、隐式重试或任意收件人选择。
- [ ] Keep SMS/private lookup unavailable without authoritative live transport; preserve A/B separation and exact final-body/expiry/cancellation fences when attaching the production path. / 无权威 live transport 时 SMS 私有查询继续拒绝；接入生产路径须保留 A/B 分离与最终正文、期限、取消栅栏。

### F2 — Accounts, regions and operational limits / 账户、地区与运营限制

- [ ] Confirm eligible providers, number capabilities, consent/opt-out and permitted usage for Mainland China +86 and overseas routes separately. No working overseas route substitutes for +86 evidence. / 分别确认中国大陆 +86 与海外路由的供应商资质、号码能力、同意/退订及允许用途；海外成功不能替代 +86 证据。
- [ ] Define and verify carrier-specific spending caps, call-duration limits, SMS segment fees, outstanding/unknown liability and reconciliation; stop future paid effects when limits or billing certainty fail. / 定义并验证运营商费用上限、通话时长上限、短信分段费用、在途/未知负债和对账；额度或费用确定性失败时停止后续付费副作用。
- [ ] Verify data location, retention/deletion, speech-processing policies, training use, log redaction and incident procedures. / 验证数据位置、留存/删除、语音处理政策、训练用途、日志脱敏及事故处理。

### F3 — SMS real effects / SMS 真实效果

- [ ] For each claimed region, send from a handset to the configured number and receive the Core answer on a handset; correlate provider receipt IDs and timestamps. / 对每个宣称支持的地区，手机向配置号码发短信并在手机收到 Core 回复，关联供应商回执 ID 与时间。
- [ ] Verify multipart GSM7/UCS2/Unicode behavior, billing segments, receipts, duplicate/late/out-of-order callbacks and expired operations. Provider acceptance must not be reported as handset delivery. / 验证 GSM7/UCS2/Unicode 分段、计费分段、回执、重复/迟到/乱序回调及过期操作；供应商接受不能写成手机送达。
- [ ] Disconnect/reconnect, stop, off-switch and rebinding tests prove no queued paid replay, stale send, cross-caller disclosure or history leak. / 断线/重连、stop、关闭开关及重绑定验证无排队付费重放、过期发送、跨来电者披露或历史泄漏。

### F4 — PSTN audio and lifecycle real effects / PSTN 音频与生命周期真实效果

- [ ] Real outbound initiation and authenticated incoming answer reach the intended handset only after operator AllowOnce. Denial/expiry/cancellation produce no paid dial/answer. / 真实外呼与认证来电接听在 operator AllowOnce 后到达目标手机；拒绝/过期/取消不触发付费拨号/接听。
- [ ] Two-way audible conversation validates committed STT transcript, TTS audio, latency, interim/duplicate handling and public context isolation. / 可听的双向对话验证 committed STT 文本、TTS 音频、延迟、interim/重复处理与公共上下文隔离。
- [ ] Interrupt a generated/queued/playing response: verify sound actually stops, exact epoch is cleared and late audio is never replayed. / 打断生成中/排队中/播放中的回复，验证声音实际停止、exact epoch 清除且迟到音频不重放。
- [ ] Verify exact played-text acknowledgement against handset observations; only established heard content enters completed public history. / 将 exact played-text 确认与手机观察对照；仅已证实听到的内容进入完整公共历史。
- [ ] Hangup, disconnect, provider timeout and shutdown tests record local closure separately from remote acceptance/unknown, check the remote call actually ends, and reconcile uncertain charges without automatic redial. / 挂断、断线、供应商超时及关闭验证分别记录本地关闭和远端接受/未知，检查远端通话实际结束，并对账未知费用，不自动重拨。

### F5 — Acceptance evidence and release / 验收证据与发布

- [ ] Re-read the original design G1–G5 and satisfy each gate with dated, region-specific evidence. / 重新读取原设计 G1–G5，逐项提供带日期、分地区的证据。
- [ ] Run operator-approved live end-to-end QA through Panel configuration and the real server, not only fixture constructors. / 经运营者授权，从 Panel 配置和真实服务端运行端到端 QA，不只用夹具构造器。
- [ ] Re-run relevant regressions on the intended release revision, investigate full-library failures, and archive build/revision/provider/region/result evidence durably with secrets redacted. / 在计划发布版本重跑相关回归、调查全库失败，持久归档构建/版本/供应商/地区/结果证据并脱敏。
- [ ] Obtain a separate production/release verdict. Task 15/16 offline completion is not that verdict. / 单独取得生产/发布判定；任务 15/16 离线完成不等于该判定。

## Required result record / 结果记录模板

For each item record: revision, UTC timestamp, region, provider and number capability (redacted), exact scenario, observed handset/audio/provider/billing effect, expected result, actual result, evidence location, residual uncertainty and reviewer. Never store live secrets in this document.

每项记录：代码版本、UTC 时间、地区、供应商与号码能力（脱敏）、精确场景、手机/音频/供应商/计费观察、预期与实际结果、证据位置、残余不确定性和审核者。本文不存储真实凭据。

Until these checks pass, real telephone/SMS behavior, +86/overseas availability, carrier billing, G1–G5 and production release remain **UNVERIFIED / 未验证**.
