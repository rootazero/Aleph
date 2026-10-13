# Telecom owner-memory private-turn checkpoint / 电信所有者记忆私有回合检查点

Date / 日期：2026-10-13

Scope / 范围：current uncommitted WIP in `/home/zou/data/workspace/Aleph-sms-pstn-sdd`; documentation checkpoint, not a release specification or task-completion declaration.
范围限定：上述工作树当前未提交的 WIP；本文是实现检查点，不是发布规范，也不是任务完成声明。

## 1. Critical status and supersession / 关键状态与替代范围

**Production startup ALWAYS disables `private_turn`, including when `[telecom_private_read] enabled = true`. Task #14 remains incomplete.** Offline implementation and bounded tests do not make the feature production-ready.

**生产启动始终禁用 `private_turn`，即便 `[telecom_private_read] enabled = true`。任务 #14 仍未完成。** 离线实现和有界测试不能证明功能已经可用于生产。

- `src/bin/aleph-server/commands/start/mod.rs` computes `public_private_turn: Option<PrivateTurnWiring>` as `None` in every branch. With enabled config and an operator plane, it warns: `telecom_private_read enabled but no authoritative SMS relay lease is wired; private turn disabled`.
  `src/bin/aleph-server/commands/start/mod.rs` 的所有分支都把私有接线设为 `None`；配置启用且操作员平面存在时仍记录上述警告并关闭。
- `src/gateway/interfaces/sms/mod.rs` factory constructs `SmsChannel::new(id, config)` without a production relay transport. A captured inbound `connection_generation` is not authoritative evidence that the live lease remains valid after reconnect.
  `src/gateway/interfaces/sms/mod.rs` 工厂仅调用 `SmsChannel::new(id, config)`，没有生产 relay transport；入站时捕获的 generation 不能证明重连后的租约仍有效。
- No production relay authority, rebind incarnation, or credential-provisioning adapter is implemented. The original **Important generation defect is closed ONLY by disabling the path**, not by completing live authority/revocation fencing.
  生产 relay 权威来源、重绑定 incarnation、凭据供应适配器均未实现。原 **Important generation 缺陷仅通过禁用路径关闭**，并非已完成 live authority 与撤销栅栏。
- #13 is pending and blocked; #14 is incomplete; #15 and #16 are pending. Real G1–G5 remain blocked and unauthorized. Do not infer any task closure or live enablement from this checkpoint.
  #13 待办且受阻；#14 未完成；#15、#16 待办。真实 G1–G5 仍受阻且未获授权；本文不解除任何任务或真实线路闸门。
- Task #19 live-lease Core capability design is being planned, not implemented or accepted here. A later authorized follow-up may replace the startup-disabled-only containment; this checkpoint states the current disabled implementation only.
  #19 live-lease Core capability 设计正在规划，本文不宣称已实现或验收。后续获授权实现可能替代“仅靠启动禁用”的隔离方案；本文只记录当前关闭的实现。

The earlier [2026-10-13-telecom-public-trusted-scope-and-private-read.md](2026-10-13-telecom-public-trusted-scope-and-private-read.md) describes the older no-source skeleton. This checkpoint supersedes **only its owner-memory WIP behavior description**: the current offline A/read/draft/B path exists, but production is still disabled. It does not replace the original telecom plan, other specs, release gates, or historical reports.

此前的 [2026-10-13-telecom-public-trusted-scope-and-private-read.md](2026-10-13-telecom-public-trusted-scope-and-private-read.md) 描述旧版无来源骨架。本文**仅替代其 owner-memory WIP 行为描述**：当前离线 A/读取/草拟/B 路径已经存在，但生产仍关闭；不替代原电信计划、其他规范、发布闸门或历史报告。

## 2. Same Core, restricted public turn / 同一 Core，受限公开回合

`[telecom_private_read]` is startup-only and defaults to `enabled = false`; absent or empty section is disabled. The config requests capability, not live transport authority. The following path describes explicit offline/test wiring, not what production startup enables.

`[telecom_private_read]` 仅在启动时读取，默认 `enabled = false`；配置段缺失或为空均关闭。配置表达能力请求，不提供真实 transport authority。以下路径描述显式离线/测试接线，不表示生产启动已开放。

The public turn uses the **same Rust Core**, with isolated public context/provider/history and no owner session fallback. It has no tools by default. When explicitly wired, its maximum tool surface is the single model-chosen signal `request_private_lookup { query }`: non-empty trimmed query ≤512 **bytes**, first request wins. Executing it only fills the turn-local slot and returns fixed text; it performs no private read or external effect. Every other tool name is refused. Repeated calls cannot replace the first query; this is at most one effective lookup request per turn, not a claim that the model can invoke the tool only once.

公开回合运行于**同一个 Rust Core**，但使用隔离的公开上下文、provider 和历史，不回退到 owner 会话。默认无工具；显式接线后最多暴露模型自主选择的一个信号工具 `request_private_lookup { query }`：去首尾空白后的非空查询 ≤512 **字节**，首次请求胜出。执行只写 turn-local 槽并返回固定文本，无私有读取或外部效果。其他工具名一律拒绝；重复调用不能替换首次查询，因此每回合最多一个有效 lookup 请求，而非限制模型只能调用一次工具。

Source / 实现：`src/config/types/telecom_private_read.rs`; `src/orchestrator/harness_bridge/public_lookup_tool.rs`; `src/orchestrator/harness_bridge/public_run.rs`.

## 3. Approval A before any private read / 任何私有读取之前的审批 A

Approval A binds the exact source, exact recorded query, and **complete caller message** to the trusted live scope. Its deterministic body is:

审批 A 把精确来源、精确已记录查询和**完整来电者消息**绑定到可信 live scope，确定性正文为：

```text
source: owner_memory (read-only search)
query: <Rust Debug-escaped exact query>
caller message: <Rust Debug-escaped complete caller message>
```

Both query and caller text are escaped/quoted so control characters or injected newlines cannot impersonate fields. The complete caller text must be ≤1024 **bytes before escaping**; longer text is rejected, never silently truncated. The ledger also bounds the complete approval body to ≤8KiB. Thus A is not just a vague permission to search, and the 1024B cap is not a cap on the entire rendered A body.

查询和来电者文本均经转义/引用，控制字符或换行不能伪造字段。完整来电者文本在**转义前**必须 ≤1024 **字节**；超限拒绝，不静默截断。账本还限制完整审批正文 ≤8KiB。因此 A 不是笼统的“可以搜索”，1024B 也不是渲染后 A 全文的上限。

Only after exact A is operator-approved and consumed can the driver read. Scope/currentness, cancellation, consumed A body and its original deadline are revalidated at read and draft checkpoints. Read/draft timeout is capped by the remaining A lifetime and run timeout; consuming A does not renew it. SMS approvals have a 300s TTL from request creation (the ledger's voice TTL is 60s; this is not live voice-feature evidence). B must satisfy both its own deadline and A's deadline.

只有操作员批准并 consume 精确 A 后驱动才可读取。在读取/草拟检查点重新验证 scope/currentness、取消状态、已消费 A 正文及原始截止时间；读取/草拟超时取 A 剩余寿命与 run timeout 的较小值，consume 不续期。短信审批 TTL 为创建起 300 秒（账本语音 TTL 为 60 秒，不代表真实语音能力已验证）；B 必须同时满足自身及 A 的截止时间。

Source / 实现：`src/gateway/telecom/private_turn.rs`; `src/gateway/telecom/public_approval.rs`.

## 4. Owner memory: read-only, bounded, one partition / 所有者记忆：只读、有界、单分区

`OwnerMemoryReadDriver` searches only the proven owner partition **`main__u-owner`**, never the legacy/global `main` partition. Older notes stored under `main` are intentionally unavailable pending provenance proof. No source selection expands this boundary.

`OwnerMemoryReadDriver` 只搜索已证明的 owner 分区 **`main__u-owner`**，不回退 legacy/global `main`。旧笔记若存于 `main`，在来源证明完成前刻意不可用；不会通过来源选择扩大边界。

- A single parameterized SELECT over `notes_fts`: `WHERE content MATCH ?1 AND agent_id = ?2 ORDER BY rank LIMIT ?5`. MATCH is **content-only**, so a filename match alone cannot select a private body. Whitespace-separated terms become quoted/escaped phrases joined with **AND**, with ≤16 terms; excess terms are rejected rather than dropped, and FTS operators are inert.
  仅对 `notes_fts` 做参数化 SELECT；MATCH **只针对 content**，文件名独自命中不能选中私有正文。空白分词后逐词引用/转义，以 **AND** 相连，最多 16 项；超限拒绝而非丢弃，FTS 运算符被当作字面文本。
- SQL bounds output before Rust materialization: `substr(path, 1, 256)`, `substr(content, 1, 2048)`, LIMIT 8. SQLite `substr` counts characters; rendering separately imposes UTF-8-safe **byte** caps (path 256B, note 2KiB, complete result ≤8KiB), with at most 8 hits. This bounds returned output, not FTS execution cost or underlying index size.
  SQL 在进入 Rust 之前用 `substr(path, 1, 256)`、`substr(content, 1, 2048)` 和 LIMIT 8 限制输出。SQLite `substr` 按字符计数；渲染阶段另施加 UTF-8 安全的**字节**上限（路径 256B、单笔记 2KiB、总结果 ≤8KiB），最多 8 条命中。这限制返回输出，不限制 FTS 执行成本或索引大小。
- The backend mutex is held throughout the connection closure. `QueryOnlyGuard` preserves the pre-existing `PRAGMA query_only` state, enables read-only only when needed, and restores writable prestate with RAII even on panic. If restoring writable prestate fails, the process **aborts** instead of exposing the shared connection in an unknown state: fail-closed protection with an explicit availability risk.
  整个 connection closure 持有 backend mutex。`QueryOnlyGuard` 保留原 `PRAGMA query_only` 状态，必要时启用只读，并用 RAII 在 panic 时也恢复原可写状态。恢复失败会**中止进程**，而不是让共享连接处于未知状态：这是 fail-closed 保护，也明确牺牲可用性。
- No hooks, embedding calls, recall signals, tool-result sinks, owner-history reads/writes, workspace/profile/SOUL/secret reads, or memory writes. Query and results are not logged. Returned paths are bounded note metadata, not instructions to open files.
  不调用 hooks、embedding、recall 信号或工具结果 sink；不读写 owner history，不读取工作区/profile/SOUL/secrets，也不写记忆；查询与结果不进日志。返回路径只是有界笔记元数据，不会据此打开文件。

Source / 实现：`src/gateway/telecom/owner_memory_read.rs`.

## 5. Ephemeral draft, approval B, final send fence / 临时草稿、审批 B 与最终发送栅栏

The private result feeds an isolated **tool-free ephemeral draft**, not owner history, durable owner session state, or autonomous follow-up work. The final draft is trimmed, must be non-empty and ≤8KiB, and becomes B's **exact** operator-reviewed body. It is not approved by a preview, paraphrase, or generic permission.

私有结果进入隔离的**无工具临时草拟回合**，不进入 owner history、owner 持久会话状态或自主后续任务。最终草稿去首尾空白后须非空且 ≤8KiB，作为 B 的**精确**操作员审核正文；不能用预览、概述或笼统许可代替批准。

After B is consumed, `send_approved_public_telecom` waits for registry/channel locks, then synchronously validates the exact outbound body, current trusted scope, linked consumed A/B grants and **both deadlines**, plus cancellation, after the **last registry/channel lock wait** and before the **first `Channel::send` poll**. No await separates that final validation from first poll. The private send has no ordinary public-send fallback, durable queue, registry retry, or owner observer path. In-memory fences are not an implemented live relay lease fence.

B 被消费后，`send_approved_public_telecom` 等待 registry/channel 锁；在**最后一次 registry/channel 锁等待之后**、**首次轮询 `Channel::send` 之前**，同步验证精确出站正文、当前可信 scope、关联的已消费 A/B grant、**两个截止时间**及取消状态。最终验证与首次 poll 之间无 await。私有发送不回退普通公开发送，不经过持久队列、registry retry 或 owner observer；进程内栅栏不等于已经实现的 live relay 租约栅栏。

Core treats the first send poll as the handoff boundary; later cancellation cannot be assumed to retract the attempt. This is not proof of the remote Relay's irreversible write boundary or a cross-thread atomic transaction. A transport **ACK is not proof of recipient delivery**. `Disclosed` and `caller_saw_turn` are internal ACK-based outcome names, not carrier/handset receipt evidence. If a transport error/timeout leaves delivery unknown, nothing further is sent to retry or compensate for that ambiguous attempt.

Core 将首次 send poll 视为交接边界；不能假设随后取消会撤回该尝试。这不证明远端 Relay 的不可逆写入边界，也不是跨线程原子事务。transport **ACK 不证明收件人已送达**；`Disclosed`、`caller_saw_turn` 只是基于 ACK 的内部结果名，不是运营商/手机收件证据。transport 错误/超时造成结果未知时，不再重试或补偿该模糊尝试。

Fixed checking/incomplete notices are also **scope-fenced after the final lock wait**. They do not echo the query, source, result, draft, or denial stage. `/stop` emits no further notice. Public history uses fixed markers **`[private turn omitted]` / `[omitted]`**, omitting even the original caller question; the query, private result and approved draft are not retained there. Marker eligibility follows the ACK gate, not inferred recipient receipt.

固定 checking/incomplete 通知同样在最终锁等待后做 **scope 栅栏检查**，不回显查询、来源、结果、草稿或拒绝阶段。`/stop` 不再发通知。公开历史只用固定标记 **`[private turn omitted]` / `[omitted]`**，连原始问题也省略，不保留查询、私有结果或获批草稿。是否记录标记遵循 ACK 闸门，不能据此推断收件人已收到。

`caller_stop` is caller-bound: a caller cannot cancel another caller's active turn merely by sharing a binding. The admission permit and trusted scope remain held through read/draft/approval/delivery; cleanup retires this turn's approvals. This does not solve production relay reconnect/rebind authority.

`caller_stop` 按 caller 绑定：同一 binding 上的某个来电者不能取消另一个来电者的活动回合。准入 permit 和可信 scope 持有至读取/草拟/审批/发送结束，清理时撤销本回合审批；这不解决生产 relay 重连/重绑定权威问题。

Source / 实现：`src/gateway/telecom/private_turn.rs`; `src/gateway/telecom/public_run.rs`; `src/gateway/channel_registry.rs`; `src/gateway/telecom/public_admission.rs`; `src/orchestrator/harness_bridge/runner_impl.rs`.

## 6. Spend and operator trust trade-offs / 费用与操作员信任取舍

Without a formal spend ledger/policy, provider errors, timeouts, or cancelled paid work without trustworthy usage **exhaust the process fallback budget**, rather than pricing unknown usage at zero. Successful usage is counted; formal metering remains authoritative when enabled. This is false-closed availability behavior: an unaccounted failed request can prevent later public turns until policy is configured or the process restarts. It is not proof that the provider charged nothing.

没有正式 spend ledger/policy 时，provider 错误、超时或被取消的付费调用若无可信用量，会**耗尽进程 fallback 预算**，而非把未知费用按零计价。成功调用按用量计数，正式计量启用时仍以它为权威。这是 false-closed 的可用性取舍：一次无法记账的失败请求可能阻止后续公开回合，直到配置 policy 或重启；不表示 provider 没有收费。

The operator UI uses a typed full-review view with `binding_ref`, `caller_digest`, `connection_generation`, `turn_id`, and `session_epoch`. List previews are display-only (≤256B); decisions require the complete reviewed body. Server/default and Panel list limits cover **all 256 live records**, matching the ledger cap, not an unpageable smaller default.

操作员 UI 的类型化完整审核视图含 `binding_ref`、`caller_digest`、`connection_generation`、`turn_id`、`session_epoch`。列表预览仅供展示（≤256B），决策必须针对完整审核正文。服务端默认和 Panel 列表上限覆盖**全部 256 条 live 记录**，与账本上限一致，不再使用无法分页补齐的较小默认值。

Desktop notification and phone-alert views poll while mounted on a **4s** interval; cleanup/disconnect stops disposed runs, reconnect starts a fresh run. Initial refusal or role refusal stops polling; after initial authorization, transient failures can be retried. This is mounted-view polling, not a persistent push service, background approval daemon, or real browser/device QA result.

桌面通知和手机 alerts 视图挂载期间按 **4 秒**间隔轮询；卸载/断连停止旧循环，重连开启新循环。首次失败或角色拒绝停止轮询；初次确认授权后可重试瞬时故障。这是挂载视图轮询，不是常驻推送、后台审批服务或真实浏览器/设备 QA 结论。

**Accepted trust boundary:** the user explicitly accepts loopback implicit operator access, so same-host processes are trusted for this operator surface. Do not describe loopback as per-process identity isolation or claim resistance to a hostile local process. Public SMS text does not grant operator authority.

**已接受的信任边界：**用户明确接受 loopback implicit operator，因此该操作员界面信任同机进程。不能把 loopback 描述为逐进程身份隔离，也不能声称能抵抗同机恶意进程；公开 SMS 文本不授予 operator 权限。

Source / 实现：`src/gateway/telecom/operator_control.rs`; `interfaces/webchat/src/state/telecom_approvals.rs`; `interfaces/webchat/src/api/telecom_approval.rs`; `interfaces/webchat/src/components/telecom_approval_card.rs`; `interfaces/webchat/src/context.rs`.

## 7. Offline evidence, not production acceptance / 离线证据，不是生产验收

Detailed log accounting is in [task-14-owner-memory-offline-verification.md](../../../.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-14-owner-memory-offline-verification.md). Existing fresh logs under `/tmp/aleph-telecom-task14` report:

详细日志账目见上述 task-14 报告。`/tmp/aleph-telecom-task14` 中已有 fresh 验证日志记录：

- Targeted Core **459 passed / 0 failed** = 153 + 93 + 24 + 142 + 2 + 45; Panel **1408 / 0**. These are scoped suite totals, not full-Core totals.
  定向 Core **459 通过 / 0 失败**，Panel **1408 / 0**；仅限对应测试集，不是 Core 全量总数。
- `check5.log`: lib/tests/bins PASS; `panel-clippy.log`: wasm PASS; `clippy-core.log`: PASS with **18 warning lines**, not warning-free.
  `check5.log` 的 lib/tests/bins 检查 PASS；Panel wasm clippy PASS；Core clippy PASS 但有 **18 行 warning**，不是零警告。
- Earlier `panel-all.log` had one introduced English-ratchet failure; localizing five labels resolved it in `panel-all2.log`.
  早期 `panel-all.log` 有一条新引入的英文 ratchet 失败；五个标签本地化后 `panel-all2.log` 通过。
- `core-all.log`: reported timeout exit **124**, unrelated failures reported, no final totals or failure attribution; **NOT PASS**. The reported run-origin checkpoint was `run_origin::tests::capture_carry_and_reestablish_carry_are_used_only_by_the_carrier`; that entry is `ok` in the log, so it is not established as the hang cause.
  全量 `core-all.log` 报告超时退出 **124**，已出现不相关失败，但无最终汇总或归因，**不是 PASS**。所报告检查点为上述 run-origin 测试；日志该项为 `ok`，不能认定其为卡住原因。
  The parent agent stopped the orphan process after validating its CWD; this documentation-only subtask did not stop processes.
  主 Agent 在核对 CWD 后停止了残留孤儿进程；本轮仅文档子任务未自行停止进程。
- Independent review **`e8e2cc4e`**: bounded offline PASS, **0 Critical / 0 Important** after fixes. This is not a production safety or real telecom acceptance verdict.
  独立复核 **`e8e2cc4e`**：修复后有界离线 PASS，**0 Critical / 0 Important**；不是生产安全或真实电信验收结论。

This documentation pass reads existing implementation/logs only; it does not rerun cargo or create new test results.

本轮仅读取现有实现/日志，不重跑 cargo，不产生新的测试成绩。

## 8. Exact next missing implementation and negatives / 下一项缺失实现与未做事项

**Next missing implementation:** a production SMS relay adapter with authenticated credential provisioning, authoritative live lease/generation ownership shared by inbound and outbound transport, explicit reconnect/rebind incarnation, and revoke/stop fencing at the relay's irreversible send boundary. Wire that authority to A/B scope invalidation and final disclosure validation, then test stale generation/rebind/queued-lock/expiry/cancellation races before considering lifting the startup disable. A fixture generation or enabled config is not a substitute. Any real G1–G5 validation additionally requires explicit authorization and real evidence.

**下一项缺失实现：**生产 SMS relay adapter，具备经认证的凭据供应、入站/出站共享的 live lease/generation 权威、显式重连/重绑定 incarnation，并在 relay 不可逆发送边界执行 revoke/stop 栅栏。将该权威接入 A/B scope 失效及最终披露验证，验证旧 generation、重绑定、锁等待、过期、取消竞态后，才可考虑解除启动禁用。fixture generation 或启用配置不能替代它；真实 G1–G5 另须明确授权与实际证据。

Not done: production adapter/enablement, real SMS or calls, provisioning, supplier contacts, purchase, push/merge, task #14 completion, #13/#15/#16 closure, real G1–G5 acceptance, full-Core PASS, or new runtime/browser/device validation. This checkpoint changes no code, progress, protected dirty report, existing plan, or other spec.

未做：生产适配器/启用、真实短信或通话、provisioning、供应商联系、购买、push/merge、#14 完成、#13/#15/#16 关闭、真实 G1–G5 验收、Core 全量 PASS 或新的运行时/浏览器/设备验证。本文不修改代码、progress、受保护脏报告、既有计划或其他规范。
