# Task #14 — Owner-memory offline verification / 任务 #14：所有者记忆离线验证

Date / 日期：2026-10-13

Worktree / 工作树：`/home/zou/data/workspace/Aleph-sms-pstn-sdd`

Status / 状态：**bounded offline evidence recorded; #14 INCOMPLETE; production private turn DISABLED**.
状态：**记录有界离线证据；#14 未完成；生产私有 turn 关闭**。

## 1. Acceptance boundary / 验收边界

This report documents the current uncommitted owner-memory WIP and existing fresh verification logs. It is not a task-completion report, full-Core success claim, release authorization, or evidence of real SMS/call delivery. This documentation pass ran **no cargo** and did no staging or commit.

本文记录当前未提交的 owner-memory WIP 和已有 fresh 验证日志，不是任务完成报告、Core 全量通过声明、发布授权或真实短信/通话送达证据。本次文档操作**未运行 cargo**，未暂存、未提交。

**Critical production status:** `src/bin/aleph-server/commands/start/mod.rs` ALWAYS sets `public_private_turn` to `None`, including enabled config with an installed operator plane. `src/gateway/interfaces/sms/mod.rs` factory constructs `SmsChannel::new(id, config)` without a production relay transport. There is no authoritative live SMS lease connected to private A/B and delivery fencing. No production relay authority, rebind incarnation, or credential-provisioning adapter is implemented.

**生产关键状态：**`src/bin/aleph-server/commands/start/mod.rs` 始终令 `public_private_turn = None`，配置启用且操作员平面已安装时也不例外。`src/gateway/interfaces/sms/mod.rs` 工厂只构造 `SmsChannel::new(id, config)`，没有生产 relay transport；私有 A/B 及发送栅栏未连接权威 live SMS 租约。生产 relay 权威、重绑定 incarnation 和凭据供应适配器均未实现。

The original **Important generation defect is closed ONLY through startup disable**. An inbound generation snapshot cannot certify a live transport lease after reconnect; offline consumed-grant/lock fences do not fix that missing authority. This is containment, not feature completion.

原 **Important generation 缺陷仅由启动禁用关闭**。入站 generation 快照不能证明重连后真实 transport 租约仍有效；离线 consumed-grant/lock 栅栏无法填补该权威缺口。这是隔离风险，不是完成整项功能。

**Follow-up status:** a new **Task #19 live-lease Core capability design is being planned**, not implemented or accepted by this report. A later authorized follow-up may replace this checkpoint's startup-disabled-only containment; until then, this report asserts the current disabled state only.

**后续状态：**新的 **Task #19 live-lease Core capability 设计正在规划**；本文不宣称它已实现或验收。后续获授权实现可能替代“仅靠启动禁用”的隔离方案；在此之前，本文只确认当前关闭状态。

| Item / 项目 | Current status / 当前状态 |
| --- | --- |
| #13 | Pending, blocked / 待办、受阻 |
| #14 | Incomplete; offline WIP verification only / 未完成，仅验证离线 WIP |
| #15 / #16 | Pending / 待办 |
| #19 | Live-lease Core capability design being planned; no completion claim / 正在规划 live-lease Core capability 设计，不宣称完成 |
| Real G1–G5 / 真实 G1–G5 | Blocked, no authorization / 受阻、未获授权 |
| Production private turn / 生产私有 turn | Always disabled at startup, even enabled config / 启动始终禁用，启用配置也不例外 |

No `progress.md`, original plan, or historical status has been edited to mark these tasks complete.

未修改 `progress.md`、原计划或历史状态来把这些任务标为完成。

## 2. Current implemented offline contract / 当前已实现的离线契约

The companion [owner-memory private-turn checkpoint](../../../docs/superpowers/specs/2026-10-13-telecom-owner-memory-private-turn-checkpoint.md) describes behavior in detail. The prior `docs/superpowers/specs/2026-10-13-telecom-public-trusted-scope-and-private-read.md` describes an older no-source skeleton; the new checkpoint supersedes **only owner-memory WIP behavior**, not other specs, plans, release gates, or production-disable status.

配套的 [owner-memory 私有回合检查点](../../../docs/superpowers/specs/2026-10-13-telecom-owner-memory-private-turn-checkpoint.md) 详述行为。旧 `docs/superpowers/specs/2026-10-13-telecom-public-trusted-scope-and-private-read.md` 描述无来源骨架；新检查点**仅替代 owner-memory WIP 行为**，不替代其他规范、计划、发布闸门，也不解除生产禁用。

- **Default and public boundary:** `[telecom_private_read]` defaults false and is startup-only. Public runs use the same Core with an isolated public context, not another brain or owner-session fallback. No tools by default; explicitly wired offline runs have at most effect-free `request_private_lookup { query }`, non-empty trimmed query ≤512B, first recorded query wins. Executing the tool reads no private data.
  **默认与公开边界：**配置默认 false，仅启动时生效。公开回合使用同一 Core 的隔离公开上下文，不另造大脑、不回退 owner 会话。默认无工具；显式离线接线后最多有无外部效果的 `request_private_lookup { query }`，去首尾空白的非空查询 ≤512B，首次记录胜出；执行工具不读取私有数据。
- **A:** exact `source: owner_memory (read-only search)`, exact query and complete caller message, quoted/escaped against field spoofing. Caller text ≤1024B before escaping; excess is refused, not truncated. The complete rendered A body also obeys the ledger's 8KiB cap. No private read until operator approval and exact A consumption. Current scope, cancellation and original A deadline are rechecked before read and draft.
  **A：**精确来源行、查询和完整来电者消息经引用/转义，避免伪造字段。来电者文本转义前 ≤1024B；超限拒绝、不截断；渲染后 A 全文还须满足账本 8KiB 上限。操作员批准并消费精确 A 前不得读取；读取/草拟前重检当前 scope、取消及 A 原始截止时间。
- **Owner memory:** only `main__u-owner`, no global/legacy `main` fallback. Parameterized `notes_fts` SELECT with content-only MATCH, quoted AND of ≤16 terms; excess terms refused, no FTS operator expansion. SQL `substr` bounds materialized output, LIMIT 8; character bounds in SQL are supplemented by UTF-8-safe byte rendering, at most 8 hits and ≤8KiB total (path 256B, note 2KiB). This is not a CPU/index-size bound.
  **所有者记忆：**仅 `main__u-owner`，不回退 global/legacy `main`。参数化 SELECT、仅 content MATCH、逐词引用的 AND，最多 16 项；拒绝多余项、不展开 FTS 运算符。SQL `substr` 限制物化输出，LIMIT 8；SQL 字符上限辅以 UTF-8 安全的字节渲染，总计最多 8 条、≤8KiB（路径 256B、单笔记 2KiB）；不保证 CPU/索引规模有界。
- **Read-only state:** shared connection mutex held; `query_only` prestate preserved and RAII-restored on unwind. Restoration failure aborts the process rather than reuse unknown shared-connection state, an explicit availability risk. No hooks, embedding, recall writes, memory/tool sinks, owner history, workspace/profile/SOUL/secrets, or memory write path. No query/result logging.
  **只读状态：**持有共享 connection mutex；保留原 `query_only` 并以 RAII 在 unwind 时恢复。恢复失败直接 abort，不重用状态未知的共享连接，这是明确的可用性风险。不走 hooks、embedding、recall 写入、记忆/工具 sink、owner history、工作区/profile/SOUL/secrets 或记忆写入；查询/结果不进日志。
- **Draft/B:** private draft is ephemeral, isolated, tool-free. Non-empty trimmed exact draft ≤8KiB becomes B's complete review body. Read/draft remain within A's original remaining lifetime; B and linked consumed A must both be unexpired. SMS TTL is 300s from request creation; consumption does not renew grants.
  **草拟/B：**私有草稿临时、隔离、无工具；去首尾空白的非空精确草稿 ≤8KiB，作为 B 完整审核正文。读取/草拟不超过 A 原始剩余寿命；B 及关联已消费 A 都须未过期。短信 TTL 为创建起 300 秒，消费不续期。
- **Submission boundary:** after B consumption, revalidate the exact body, current scope, linked consumed A/B, both deadlines and cancellation after the last registry/channel lock acquisition and before first send poll. Fixed notices use the same scope fence. No public-send fallback, durable queue, retry, or owner observer. First poll is Core's handoff boundary, not proof of a remote irreversible write or cross-thread atomicity; ACK is transport acceptance, **not recipient delivery**. Ambiguous send error/timeout is not retried or compensated.
  **提交边界：**B 消费后，在最后一次 registry/channel 锁取得之后、首次 send poll 前重验精确正文、当前 scope、关联已消费 A/B、双截止时间和取消。固定通知同样受 scope 栅栏保护。不回退公开发送、不走持久队列/重试/owner observer。首次 poll 是 Core 交接边界，不证明远端不可逆写入或跨线程原子性；ACK 是 transport 接受，**不是收件人送达**。模糊错误/超时不重试、不补偿。
- **History and cancellation:** public history retains fixed `[private turn omitted]` / `[omitted]` markers only, even the question is omitted; no query/result/draft retention. `/stop` adds no notice; `caller_stop` cannot cancel another caller on the same binding. ACK-based marker eligibility must not be described as handset receipt.
  **历史与取消：**公开历史仅保留上述固定标记，连问题也省略，不留查询/结果/草稿。`/stop` 不追加通知；`caller_stop` 不能取消同 binding 的另一个 caller。基于 ACK 的标记条件不能说成手机收件。
- **Spend:** provider errors/timeouts/cancelled paid work with unknown usage exhaust the process fallback budget when no formal ledger/policy applies; they are not zero-cost. Formal metering remains authoritative. This deliberate false-closed behavior can deny future public turns until policy configuration or restart.
  **费用：**没有正式 ledger/policy 时，provider 错误/超时/被取消付费工作若用量未知，会耗尽进程 fallback 预算，不按零费用处理。正式计量仍为权威。这种刻意 false-closed 行为可能拒绝后续公开回合，直到配置 policy 或重启。
- **Operator surface:** full typed review has `binding_ref`, `caller_digest`, `connection_generation`, `turn_id`, `session_epoch`; preview ≤256B is not decision input. Lists enumerate all 256 possible live records. Desktop/phone mounted views poll every 4s, with cleanup/disconnect stop and reconnect renewal; role refusal stops polling. The user explicitly accepts loopback implicit operator, hence **same-host processes are trusted**, not isolated hostile peers. Public SMS content cannot confer operator status.
  **操作员界面：**完整类型化审核含上述五项身份字段；≤256B 预览不是决策输入。列表覆盖全部 256 条可能 live 记录。桌面/手机挂载视图每 4 秒轮询，卸载/断连停止、重连重建，角色拒绝停止轮询。用户明确接受 loopback implicit operator，因此**信任同机进程**，不声称隔离同机恶意进程；SMS 文本不授予 operator 身份。

## 3. Existing fresh verification logs / 已有 fresh 验证日志

All entries below are existing artifacts under **`/tmp/aleph-telecom-task14`**, read during this documentation pass. Commands were run in the prior verification, not rerun here. Results must be read at their stated scope; green targeted suites do not erase the incomplete full-Core run.

下表均为 **`/tmp/aleph-telecom-task14`** 中已有文件，本轮只读；命令属于此前验证，本轮未重跑。必须按各自范围理解，定向绿不能覆盖未完成的 Core 全量运行。

| Evidence / 证据 | Recorded result / 已记录结果 | Scope / 范围 |
| --- | --- | --- |
| `/tmp/aleph-telecom-task14/telecom-tests5.log` | **153 passed / 0 failed** | Telecom targeted suite / Telecom 定向测试 |
| `/tmp/aleph-telecom-task14/final-gateway-inbound_router.log` | **93 / 0** | Inbound router targeted suite / 入站路由定向测试 |
| `/tmp/aleph-telecom-task14/final-gateway-channel_registry.log` | **24 / 0** | Channel registry targeted suite / channel registry 定向测试 |
| `/tmp/aleph-telecom-task14/final-orchestrator-harness_bridge.log` | **142 / 0** | Harness bridge targeted suite / harness bridge 定向测试 |
| `/tmp/aleph-telecom-task14/final-config-types-telecom_private_read.log` | **2 / 0** | Private-read config suite / 私有读取配置测试 |
| `/tmp/aleph-telecom-task14/resume-final.log` | **45 / 0** | Resume regression suite / resume 回归测试 |
| `/tmp/aleph-telecom-task14/check5.log` | **PASS** | Core lib/tests/bins check; warnings present / Core lib/tests/bins 检查，有 warning |
| `/tmp/aleph-telecom-task14/panel-all2.log` | **1408 / 0** | Panel all tests / Panel 全量测试 |
| `/tmp/aleph-telecom-task14/panel-clippy.log` | **PASS** | Panel wasm clippy / Panel wasm clippy |
| `/tmp/aleph-telecom-task14/clippy-core.log` | **PASS, 18 warning lines** | Core clippy, not warning-free / Core clippy，非零警告 |
| `/tmp/aleph-telecom-task14/panel-all.log` | **1407 / 1**, superseded by `panel-all2.log` / 被后者替代 | Earlier introduced English ratchet failure / 早期新引入英文 ratchet 失败 |
| `/tmp/aleph-telecom-task14/core-all.log` | **Timeout exit 124; NOT PASS; no final totals** | Incomplete full Core run / 未完成的 Core 全量运行 |

**Targeted Core total: 459 passed / 0 failed = 153 + 93 + 24 + 142 + 2 + 45.** This arithmetic counts the six recorded scoped suite results; it is neither full-Core coverage nor a claim that every test is uniquely counted across all possible filters. Panel 1408/0 is separately reported, not added to Core.

**定向 Core 总数：459 通过 / 0 失败 = 153 + 93 + 24 + 142 + 2 + 45。**这是六组记录结果的算术汇总，不代表 Core 全覆盖，也不声称所有可能筛选下测试都唯一计数；Panel 1408/0 单列，不并入 Core。

### Earlier Panel failure resolved / 早期 Panel 失败已修复

`panel-all.log` reports the introduced `i18n_census::tests::hardcoded_english_line_ratchet` failure. Five operator identity labels were localized in the implementation; the fresh `panel-all2.log` reports **1408/0**, including the English and Chinese ratchets. This is not a claim that every pre-existing UI string or browser interaction was reviewed.

`panel-all.log` 记录新引入的 `i18n_census::tests::hardcoded_english_line_ratchet` 失败。实现中已把五个操作员身份标签本地化，fresh `panel-all2.log` 为 **1408/0**，包含中英文 ratchet；不表示审核了所有既有 UI 字符串或浏览器交互。

### Full Core run: incomplete, not attributable / Core 全量运行：未完成、未归因

The full run was reported timed out with **exit 124** around the run-origin checkpoint `run_origin::tests::capture_carry_and_reestablish_carry_are_used_only_by_the_carrier`. The log actually marks that entry **`ok`** and later has an unfinished line; no final test-result totals appear. Therefore this report does **not** identify that test as the hanging cause or infer final totals. The parent agent stopped the orphan process **after validating its CWD**; this documentation-only subtask did not stop processes itself.

全量运行报告在上述 run-origin 检查点附近超时，**退出 124**。日志该项实际为 **`ok`**，后续还有未结束的行，但没有最终测试总数。因此本文**不**将该项认定为卡住原因，也不推算最终总数。主 Agent 在**核对 CWD 之后**停止了残留孤儿进程；本次仅文档子任务没有自行停止进程。

The log reports failures in other areas (for example `a2a::adapter::server::routes::tests::extract_credentials_bearer_mixed_case`, `cluster::node_file_cmd::tests::file_read_rejects_missing`, and `extension::validation::tests::every_bundled_plugin_passes_the_installers_own_validation`). These are reported as unrelated failures in the supplied verification status, but no completed full-run attribution or root-cause investigation is supplied here. Do not erase them, claim a clean baseline, or attribute them to this WIP without evidence.

日志出现其他领域的失败（如上述 a2a、cluster、extension 测试）；所提供验证状态将其描述为不相关失败，但本文没有完整全量归因或根因调查。不得抹掉这些失败、声称基线全绿，或无证据归因到本 WIP。

## 4. Independent review / 独立复核

Independent review **`e8e2cc4e`** reports **bounded offline PASS, 0 Critical / 0 Important** after the fixes. This is recorded from the supplied review result, not a newly commissioned review or rerun in this documentation pass. No review artifact path is invented here.

独立复核 **`e8e2cc4e`** 报告修复后 **有界离线 PASS，0 Critical / 0 Important**。此处记录提供的复核结果，不是本轮新发起的复核或重跑；不虚构复核文件路径。

The bounded verdict is compatible with production being disabled and #14 being incomplete. It must not be widened to a live-lease/rebind/credential adapter verdict, production safety guarantee, real recipient delivery verdict, or release approval.

有界 verdict 与生产禁用、#14 未完成并不矛盾。不能扩大解释为 live-lease/重绑定/凭据适配器验收、生产安全保证、真实收件人送达判定或发布批准。

## 5. Documentation write scope and protected state / 文档写入范围与受保护状态

Only these two new bilingual documents are written by this pass:

本轮仅写入两份新的双语文档：

1. `docs/superpowers/specs/2026-10-13-telecom-owner-memory-private-turn-checkpoint.md`
2. `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-14-owner-memory-offline-verification.md`

The three protected dirty documents are preserved, not reconciled or updated:

三份受保护脏文档保持原样，不协调、不更新：

- `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-3c-report.md`
- `.superpowers/sdd/2026-10-09-pstn-sms-thin-relay-implementation/task-4-loop-origin-fix-report.md`
- `docs/superpowers/plans/2026-10-09-pstn-sms-thin-relay-implementation.md`

`progress.md`, other specs, source code, translations and existing logs are not modified by this pass. Existing dirty implementation files are neither reverted nor staged. No cargo, formatting, commit, or network verification is performed.

本轮不改 `progress.md`、其他 specs、源码、翻译或既有日志；既有 dirty 实现既不回退也不暂存。不做 cargo、格式化、提交或网络验证。

## 6. Exact next missing implementation / 下一项确切缺失实现

The next missing piece is an **authoritative live-lease Core capability plus its production SMS relay adapter**, not another config flag or mock generation. The planned Task #19 design has not delivered it. Required implementation boundaries are:

下一缺口是**权威 live-lease Core capability 及其生产 SMS relay adapter**，不是另加配置开关或 mock generation。规划中的 #19 尚未交付。所需实现边界是：

1. Authenticated credential provisioning and the actual production relay transport; factory/startup wiring must reference that concrete authority.
   经认证的凭据供应与真实生产 relay transport；工厂/启动接线必须引用该具体权威。
2. Authoritative live lease/generation shared by inbound and outbound, with explicit reconnect/rebind incarnation and no reuse of stale approvals across incarnations.
   入站/出站共享权威 live lease/generation，显式重连/重绑定 incarnation，禁止跨 incarnation 复用旧审批。
3. Revoke/stop propagation to A/B scope invalidation and revalidation at the relay's irreversible write boundary, not merely an earlier captured context or Core lock boundary.
   revoke/stop 传播至 A/B scope 失效，并在 relay 不可逆写入边界重验，而非仅在更早捕获的 context 或 Core 锁边界重验。
4. Bounded tests for stale generation, rebind, lock waits, deadline expiry and cancellation at each effect boundary; independently review before replacing startup-disable containment. Real G1–G5 require separate explicit authorization and evidence.
   对旧 generation、重绑定、锁等待、截止过期、取消在各效果边界做有界测试，并独立复核后才可替代启动禁用隔离；真实 G1–G5 另须明确授权和证据。

## 7. Negatives / 未做事项

No production private-turn enablement or full feature completion; no implementation of Task #19; no real SMS/calls, provisioning, supplier contacts, purchase, push/merge; no #14 completion or #13/#15/#16 closure; no real G1–G5 authorization/acceptance; no full-Core PASS or failure attribution; no new cargo run, browser/device QA, staging, or commit. Existing logs may be temporary artifacts, not durable release evidence. ACK remains different from recipient delivery, same-host processes remain trusted, and query-only restoration abort remains an availability risk.

未启用生产私有 turn，未完成整体功能，未实现 #19；未做真实短信/通话、provisioning、供应商联系、购买、push/merge；未完成 #14 或关闭 #13/#15/#16；未获得/完成真实 G1–G5 授权与验收；未获得 Core 全量 PASS 或完成失败归因；未重跑 cargo、未做浏览器/设备 QA、暂存或提交。既有日志是临时文件，不是持久发布证据。ACK 仍不等于收件人送达，同机进程仍在信任边界内，query-only 恢复失败 abort 仍有可用性风险。
