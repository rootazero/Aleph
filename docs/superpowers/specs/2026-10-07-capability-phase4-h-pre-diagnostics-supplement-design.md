# H-pre Diagnostics Supplement / 生产投影诊断补充设计

**Status / 状态:** Proposed written supplement; not approved for implementation. / 书面补充待审批，尚未授权实施。
**Parent / 主设计:** `docs/superpowers/specs/2026-10-07-capability-phase4-h-pre-runtime-mount-design.md`
**Scope / 范围:** H-pre QA control and evidence only. Gate H ACP inbound, Gate I, Safe Replay remain deferred. / 仅补齐 H-pre QA 控制及证据，不启动 H／I 或 Safe Replay。

## 1. Intent and gap / 意图与缺口

The approved H-pre host must deliver registry and ownership invalidation to the real run-loop consumer. Existing `mcp_config.create/update/delete` can mutate the production registry; recording mock-provider requests can prove tools actually reach the real AgentLoop. Targeted source lookup found no existing external `OwnershipTree.bump/revoke/dispose` or host slow-delivery/close control. Library-only tests cannot substitute for the required current-binary evidence.

已审批的 host 必须把 registry 和 ownership 变化交付给真实 run-loop。已有 MCP 管理入口能修改生产 registry，recording provider 能证明工具定义真正到达 AgentLoop，但现有定向查找未定位外部 ownership 或慢交付／关闭控制入口。不得把 library-only consumer、catalog 或日志当作等价证明。

User allowed designing a default-off, local-operator diagnostic entry and subsequently asked not to repeatedly reconfirm the same authorization boundary. This does not approve this not-yet-reviewed written artifact or silently waive the existing spec/plan gates.

用户已允许设计默认关闭、本机 operator 诊断入口，并要求不重复确认同一授权边界；这不是对尚未审阅的本文或后续计划的自动批准。

## 2. Authorization and tool surface / 授权与工具面

- Name: `capability_projection_diagnostics`. Register only when the server starts with **exactly** `ALEPH_CAPABILITY_DIAGNOSTICS=1`; unset and every other value disable it. Do not hot-toggle registration or add a registry authority.
- Every operation, including status, requires server-stamped `Some("operator")`, loopback peer, and an actual connection id. Put this strict check **inside the diagnostic tool handler**, reading `caller_identity::current_caller_role()`, `caller_identity::current_caller_is_loopback()` and `caller_identity::current_caller_conn_id()`. Never authorize from `TurnContext::caller_is_operator()` or request arguments. Missing context is denied; do not reuse `role_is_operator(None)`'s trusted-internal exception. Loopback means the actual peer observed by the gateway; do not trust forwarding headers. An authenticated operator through a localhost tunnel/proxy is a local peer under this boundary, not proof of physically local origin.
- Add the exact tool name to existing `src/gateway/method_authz.rs` `OPERATOR_TOOLS` and `src/security/dangerous_tools.rs` `DANGEROUS_TOOLS`. Existing `ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics` may explicitly admit the direct gateway surface, but neither override nor diagnostic enablement bypasses the handler's strict local identity check.
- Use the existing **conditional runtime builtin registration** shape, not `BUILTIN_TOOL_DEFINITIONS`: `src/executor/builtin_registry/definitions.rs:5-18` defines that table as unconditional metadata advertised before dependencies exist. Enabled registration must provide the tool-owned DESCRIPTION/schema, runtime metadata and real execute dispatch; disabled registration must advertise nothing through catalog, provider or MCP-face projections. Extend the existing dangerous-tool existence guard to the existing advertised-and-dispatchable registration shapes, using the established source census machinery and enabled/disabled runtime tests. Never weaken it to a diagnostic-name exception or make unconditional advertisement merely to satisfy the old static-only guard.
- The ordinary agent run loses ambient connection task-locals across spawn and `TurnContext` lacks a loopback fact. Do not expand run identity metadata for this supplement. Such invocations, cron, channel, MCP-face callers without the full trusted local context, and operators with a non-loopback peer fail closed. Never trust `caller_role`, `is_loopback`, or `connection_id` supplied in tool arguments.
- Gateway handler remains a thin tool-dispatch face. Diagnostic behavior lives with the host/its tool adapter, not in interface business logic. Use existing builtin schema/dispatchability machinery; ensure registration and actual dispatch both exist.

工具仅在启动时显式开关开启后注册。授权校验放在工具 handler 内，直接读取可信 CALLER task-locals，不能读取 TurnContext 的宽松 operator 判据。缺失身份即拒绝；loopback 只代表实际本机 peer，不据此证明客户端物理位置，亦不信转发头。新增名称明确进入既有 operator／dangerous 表，但 override 不免除自身授权。采用条件 runtime builtin 注册而非无条件 schema 表，同时验证 metadata 与真实 dispatch；危险工具守卫覆盖现有两种注册形状，不能给新工具加名字特例。不会扩充 TurnContext；参数不能声明身份，网关保持纯 I/O。

## 3. One target and bounded operations / 唯一目标与有界操作

All operations address the installed production host, its sole runtime ownership tree, and its **default production subscription** used by the run-loop. Never create a substitute consumer and report its state as production evidence.

所有动作针对已挂载 host、同一 runtime tree 和 run-loop 使用的默认生产订阅。禁止创建替代 consumer 再将其状态当作生产证据。

Operations / 动作：

1. `status`: return host lifecycle, registry cursor, pending depth/capacity, replacement/lag counts, and **applied** snapshot tool ids and owner-generation observations. Clearly distinguish queued from applied state. No tool inputs, session contents, handlers, credentials or durable replay facts.
2. `bump_runtime`: invoke the existing tree `bump(Runtime)`; return its generation, not a claimed delivered generation. QA captures existing fixture owner generations before the call, then waits for applied values **strictly greater** at the unchanged registry cursor. In the isolated no-concurrent-bump fixture, they must equal the returned generation. Never require `old + 1`: the ownership nonce also serves other ownership operations and may already have advanced. An unchanged observation fails this receipt even if a snapshot exists.
3. `revoke_tool { tool_name }`: derive the existing canonical Tool capability id and revoke it on the same tree. No arbitrary kind/namespace or user-supplied generation. Irreversible; QA uses disposable fixtures and a fresh server instance between incompatible cases.
4. `dispose_runtime`: dispose the existing Runtime lifetime on the same tree. This is irreversible and does not promise cancellation of captured handlers or external effects.
5. `hold { plane, duration_ms }`: temporarily gate either `source_intake` (genuine broadcast lag) or `delivery` (genuine bounded pending overflow) for the **production** subscription. A single outstanding hold, integer duration `1..=5000` ms; overlapping holds are rejected. Worker cancellation/close bypasses the hold and releases it. A timer releases it even if the caller disconnects; no permanent pause or resumable lease.
6. `release`: idempotently release an active diagnostic hold. It grants no additional authority and does not imply drain completion.
7. `close`: release any hold, request normal host close, and await the existing shared completion boundary for at most **5000 ms**. On timeout return a tool **error**, leave the host close-requested/fail-closed, and report actual non-quiescent state through subsequent status; never report successful closure, reopen, detach joins or abandon background cancellation. Cancellation of this waiting call must preserve the completion boundary. Status remains queryable locally after closure, but no new subscription or canonical read may use a closed host. No reopen/reinstall/fallback operation.

`status` 区分已排队与已应用状态；mutation 回包只证明 mutation 返回。bump 观测值必须严格增加，隔离 fixture 无并发 bump 时等于回包 generation，不能假设旧值加一。revoke／dispose 不可逆，fixture 分进程隔离。hold 最长 5 秒、仅一个、断线后自动释放，close/cancel 不受 hold 阻挡。close 最多等待 5000 ms；超时返回工具错误并保持 close-requested／fail-closed，不能声称完成或遗弃 join；无 reopen 或回退绕过。

The host's source receiver and pending queue must continue to follow the same normal code paths during diagnostics. The hold is a narrow delivery/intake gate, not a replacement event generator. Pending capacity remains ordinary policy; diagnostic fixtures may configure a smaller startup capacity through the host's existing mounting policy, never change an active queue's capacity to manufacture overflow.

诊断 hold 只阻挡实际 intake 或实际 delivery，不合成事件。overflow 必须来自正常有界队列；fixture 可以在挂载时选择较小容量，不能运行时改容量制造“溢出”。

## 4. Production receipts and negative controls / 生产交付与反向控制

Use current `aleph-server`, an isolated profile, real `chat.send`/AgentLoop, existing stdio MCP mutation, and the recording mock-provider pattern in `qa/plugins`. The production projection consumer remains the real run-loop; the mock provider is only an evidence sink at its ordinary provider boundary, never a substitute projection consumer. Capture provider `tools[]` with unique request markers; a log/catalog/diagnostic counter alone is never a positive tool-delivery receipt.

使用当前二进制、隔离 profile、真实 chat.send／AgentLoop、已有 stdio MCP mutation 与 recording provider。生产 consumer 是真实 run-loop，mock provider 仅在正常 provider 边界记录证据，不是替代投影。每次请求使用唯一 marker；日志、catalog、诊断计数不能单独证明工具交付。

Required evidence / 必需证据：
- Positive control: a known fixture tool reaches recorded provider `tools[]` and a mock-provider-requested invocation reaches its actual handler.
- Normal registry replacement/removal reaches later real requests without per-request raw-registry fallback.
- Owner-only revoke/dispose leaves registry revision unchanged yet removes fixture tools from later provider requests. Bump changes applied owner observations at the same cursor, without making that generation a Cursor.
- Delivery hold + more registry changes than pending capacity produces **at least one** invalidation/replacement boundary; exact count may depend on scheduling. After release the next real request contains the final surviving fixture set and not removed tools. Inspect applied replacement payload/cursor evidence, not an empty marker.
- Source-intake hold + more changes than the actual broadcast capacity causes genuine Lagged recovery with snapshot payload/cursor; subsequent provider request uses the replacement. Bounded failure if recovery never arrives.
- Close during an outstanding hold completes; later canonical requests refuse the closed projection, never fallback around ownership. After completion, additional authority mutations cannot deliver to the closed production subscription.
- Disabled tool cannot be listed/invoked. Enabled tool with member/non-loopback-peer/missing-context/spoofed-argument identity is denied **without mutation**. Existing diagnostic/allow overrides never bypass these checks. Cover routes and error effects with deterministic tests; if a real non-loopback peer is unavailable, record that physical QA limitation rather than claim it ran.

证明工具到 provider 以及实际 handler；owner-only 变化在 cursor 不变时影响后续真实请求；两种 hold 分别制造真实 queue overflow 与真实 broadcast lag，恢复后真实请求使用最终 replacement。关闭后拒绝而不回退。所有拒绝测试断言没有 mutation，而不仅断言错误消息。

## 5. Lifecycle, limits and trade-offs / 生命周期、限制与取舍

- Runtime instrumentation is default-off and startup-scoped, but it is real code and requires production security tests, tool registry/schema/dispatch conformance, and existing root validation. No QA-only magic RPC. Disabled startup allocates no diagnostic hold/timer machinery and runs the ordinary source/delivery path. Test disabled registration on catalog/runtime/provider/MCP surfaces and event/snapshot/lifecycle equivalence without active diagnostics. Do not substitute a platform-dependent bytecode/assembly "no branch" assertion for behavioral proof.
- Extra telemetry is observational only. Cursor, owner generation and SessionEvent.seq remain distinct; no committed store, registry of registries, durable cursor, replay permit, external exactly-once or universal scheduler is introduced.
- Diagnostics intentionally can damage the disposable QA runtime by revoke/dispose/close. Document that operator use against a real profile is disruptive; do not add automatic recovery or a hidden reopen.
- H-pre Task3/4 keep normal transport and completion semantics; diagnostic gates must not require harness changes. Required root tests/clippy failures remain failures, not waived baselines.

诊断是真实生产代码，默认关闭也不能免除安全测试与根验证；关闭时不得创建 hold／timer，验证所有广告面不存在诊断工具且普通事件／快照／生命周期行为等价，不用字节码“无分支”量具代替效果验证。telemetry 不构成新权威。cursor／owner generation／SessionEvent.seq 分域，不引入 durable cursor、Replay permit、external exactly-once 或通用调度器。诊断可破坏 disposable runtime，不能偷偷自动恢复。所有既有红灯仍如实报告。

## 6. Approval and planning / 审批与计划

After self-review and independent design review, present this written supplement once for human review. Only written-spec approval permits its implementation-plan addendum; only approved addendum plus the already chosen Subagent-driven method permits diagnostic implementation. Approved H-pre Tasks1–6 may continue independently; Task7 cannot be claimed complete until its legitimate diagnostic controls and real receipts are validated.

自审与独立设计审查后集中提交本文审批。书面批准后才写补充实施计划；计划批准后按已选 Subagent-driven 实施。已批准 Tasks1–6 可继续，但 Task7 必须补齐合法诊断控制与真实交付证据才能完成。
