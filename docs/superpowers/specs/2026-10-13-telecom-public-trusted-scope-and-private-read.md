# 2026-10-13 — Telecom Public Trusted Scope & Private-Read Driver
# 2026-10-13 — 电信公开可信作用域与私有读取驱动

Status: implemented in worktree `Aleph-sms-pstn-sdd`, uncommitted, pending
verification. Private sources are deliberately NOT enabled (user decision);
see §7 for what is not done.
状态：已在工作树 `Aleph-sms-pstn-sdd` 中实现，未提交，待验证。私有来源
按用户决定**暂不开放**；未完成项见 §7。

This spec describes the lifecycle, capability-slot semantics, and "no
leakage into the owner plane" guarantees for the trusted A/B scope
that gates private data reads from a public telecom (SMS / voice)
inbound message.
本规范描述了为一个公开电信（短信 / 语音）入站消息所触发的私有数据
读取提供门控的可信 A/B 作用域的完整生命周期、能力槽语义，以及
"不泄漏到所有者平面"的强保证。

---

## 1. Process-global ledger slot / 进程全局账本槽

The `PublicApprovalLedger` is the single source of truth for both
A-stage (private-read request) and B-stage (disclosure) approval
records under a telecom binding. It is exposed process-globally
through a capability slot so any subsystem can ask whether a ledger
is wired in without taking a dependency on a particular constructor.
`PublicApprovalLedger` 是某个电信 binding 下 A 阶段（私有读取请求）
和 B 阶段（披露）审批记录的唯一事实来源。它通过一个能力槽
（capability slot）在进程内全局暴露，使得任何子系统都能在不依赖
特定构造器的前提下查询账本是否已经接入。

```rust
// src/gateway/telecom/public_approval.rs
use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};

pub static INSTALLED: CapabilitySlot<Arc<PublicApprovalLedger>> =
    CapabilitySlot::new(
        SlotId::from_static_str("telecom.public_approval.ledger"),
        MissingSemantics::ConsumerDecides, // reads as None when not installed
    );

pub fn install_global(
    ledger: Arc<PublicApprovalLedger>,
) -> Result<(), Arc<PublicApprovalLedger>> { /* Err if already installed */ }
pub fn decline_global(because: &'static str) {
    INSTALLED.decline(because)
}
pub fn installed() -> Option<&'static Arc<PublicApprovalLedger>> {
    INSTALLED.get()
}
pub fn installed_slot() -> &'static dyn SlotStatus { &INSTALLED }
```

The slot is enumerated in `capability::ALL_SLOTS` (the project-wide
roster) and contributes one entry to `handles_count` and one to
`gated_blocks_count` in `capability::census`. The census literal
bumps from 54 to 55 with this change.
该槽被登记进 `capability::ALL_SLOTS`（全项目花名册），并在
`capability::census` 的 `handles_count` / `gated_blocks_count` 中
各贡献一项。census 字面量因本次改动由 54 增至 55。

MissingSemantics is `ConsumerDecides` (reads as `None` when the slot
has not been installed yet). This matches the sibling history slot
and is the documented "no ledger wired in → A/B path disabled" mode.
缺失语义为 `ConsumerDecides`（未安装时读为 `None`）。与同级的
history 槽保持一致，这也是"未接入账本 → A/B 路径关闭"的官方
未接入模式。

---

## 2. Private-read driver protocol / 私有读取驱动协议

The A-stage read is performed by a `PrivateReadDriver` trait. The
driver is the ONLY place that can read owner-side data (memory,
workspace, secrets, owner history) under a `PublicApprovalScope`.
Production installs NO driver (`None`), and `PublicRunRequest.scope`
is always `None`, so the driver is never called. `NullPrivateReadDriver`
(always `Unavailable`) exists for tests.
A 阶段读取由 `PrivateReadDriver` trait 完成。该驱动是唯一允许
在 `PublicApprovalScope` 下读取所有者侧数据（记忆、工作区、
秘密、所有者历史）的入口。生产环境**不安装任何驱动**（`None`），
且 `PublicRunRequest.scope` 恒为 `None`，因此驱动永远不会被调用。
`NullPrivateReadDriver`（恒返回 `Unavailable`）仅供测试。

```rust
// src/gateway/telecom/private_read.rs
#[async_trait::async_trait]
pub trait PrivateReadDriver: Send + Sync {
    async fn read(
        &self,
        scope: &PublicApprovalScope,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<PrivateReadResult, PrivateReadError>;
}

pub enum PrivateReadResult {
    /// Owner-side body. Fails closed today (see §2.2).
    /// 所有者侧正文。当前一律 fail-closed（见 §2.2）。
    Transient(Arc<str>),
    /// No private data, run proceeds as public.
    /// 无私有数据，公开运行继续。
    Unavailable,
}

pub enum PrivateReadError {
    Denied,
    Cancelled,
    Failed,
}
```

### 2.1 Wire-in / 接线位置

`AgentHarnessRunner` carries `Option<Arc<dyn PrivateReadDriver>>`
(production: `None`) and exposes a `with_private_read_driver` builder;
it forwards the driver to `run_public_with`.
`AgentHarnessRunner` 持有 `Option<Arc<dyn PrivateReadDriver>>`（生产为
`None`）并暴露 `with_private_read_driver` 构造器，转发给 `run_public_with`。

`PublicRunRequest` carries `Option<PublicApprovalScope>`. In production
it is always `None`: the run driver keeps its live turn scope to itself
and does not hand it to the orchestrator. The driver is called only when
both a scope and a driver are present (tests only today).
`PublicRunRequest` 携带 `Option<PublicApprovalScope>`。生产中恒为
`None`：运行驱动不把 live turn 作用域交给编排器。只有作用域与驱动同时
存在时（目前仅测试）才会调用驱动。

Preconditions before ANY real source may be installed (documented on the
trait): (a) the orchestrator consumes an operator-approved A record for
the live scope and uses ONLY the approved body as the query; (b) the B
disclosure path is wired so every reply derived from a private body is
approved before reaching SMS or history.
开放任何真实来源之前必须满足（已写入 trait 文档）：(a) 编排器 consume
已批准的 A 记录，且只把审批过的 body 作为查询；(b) 接通 B 披露路径，
任何基于私有正文的回复在到达短信或历史前都须经审批。

### 2.2 Driver dispatch table / 驱动分发表

| Result / 结果 | What run_public_with does / run_public_with 的行为 |
|---------------|---------------------------------------------------|
| `Transient(_)` | Fail closed: drop the body, do NOT call the LLM, return `Err(FlowError::InvalidConfig("private-read result requires the operator disclosure path, which is not wired"))` (static text, no body). <br> fail-closed：丢弃正文，**不调用** LLM，返回静态错误 `Err(FlowError::InvalidConfig("private-read result requires the operator disclosure path, which is not wired"))`（不含正文）。 |
| `Unavailable` | No transient; run as a plain public turn. <br> 无 transient，按普通公开 turn 运行。 |
| `Denied` | Log + run as public turn. <br> 记日志，按公开 turn 运行。 |
| `Cancelled` | Return `Err(FlowError::Cancelled)` — aborts the run (not degraded to a public turn). <br> 返回 `Err(FlowError::Cancelled)`，中止本次运行（不降级为公开 turn）。 |
| `Failed` | Log (warn) + run as public turn. <br> 记 warn 日志，按公开 turn 运行。 |

---

## 3. No-history write of private result / 禁止把私有结果写入公开历史

`PublicHistoryStore::append` accepts a new `private_result: bool`
parameter. When `true`, it returns `Err(PublicHistoryError::PrivateResultRefused)`
BEFORE touching any disk allocation. The capacity counter is not
incremented (the refusal happens in O(1) without any side effect).
`PublicHistoryStore::append` 新增 `private_result: bool` 参数。当
为 `true` 时，它在任何磁盘分配之前就返回
`Err(PublicHistoryError::PrivateResultRefused)`。容量计数不会
增加（拒绝本身是 O(1) 且无副作用）。

The executor's history-write path always calls
`store.append(key, &turn, now, false)` — there is no code path in
`run_public_sms` that flips it to `true`. The only callers that
could set it `true` are tests that prove the refusal behaviour.
执行器的历史写入路径始终调用
`store.append(key, &turn, now, false)` —— `run_public_sms` 中
没有任何代码路径会把它翻成 `true`。唯一能把它设成 `true` 的
调用方是用于证明拒绝行为的测试。

Combined with the `Transient` fail-closed rule (§2.2), a private body
never reaches the LLM, the SMS reply, or the encrypted public history.
The `private_result` refusal is a second wall for the future B path.
结合 `Transient` 的 fail-closed 规则（§2.2），私有正文不会到达 LLM、
短信回复或加密公开历史。`private_result` 拒绝是为将来 B 路径准备的
第二道墙。

---

## 4. Trusted scope register / retire lifecycle / 可信作用域注册/注销生命周期

`InboundMessageRouter` carries
`operator_control: Option<Arc<OperatorControl>>` and exposes a
`with_operator_control` builder.
`InboundMessageRouter` 持有
`operator_control: Option<Arc<OperatorControl>>` 并暴露
`with_operator_control` 构造器。

`handle_public_telecom` only forwards `self.operator_control.as_ref()`
to `run_public_sms`. The scope is opened inside `run_public_sms`, AFTER
admission:
`handle_public_telecom` 只把 `self.operator_control.as_ref()` 转交给
`run_public_sms`；作用域在 `run_public_sms` 内部、**准入之后**才打开：

```
1. PublicRunRegistry admission (Busy / OverSpend / OverGlobalCap / Refused)
   → refused messages return here and NEVER touch the ledger
   被拒消息在此返回，绝不触碰账本
2. harness missing → return (no scope)
3. Admitted + harness present + Some(operator_control):
     scope = PublicApprovalScope::new(ctx, uuid_v4, uuid_v4, uuid_v4,
                                      ctx.provenance().connection_generation())
     _scope_guard = ScopeGuard::register(control, scope)   // register_scope
4. history load → harness run → delivery → history append
5. return: ScopeGuard drops with the permit (also on future drop / panic)
     → retire_scope_if_current(&scope)  // compare-and-retire
     → cancel_binding(binding)          // pending A/B records cancelled
```

core/agent/turn ids are random UUIDs that only give the turn a unique
identity, so an operator decision bound to this turn cannot be replayed
against a later one; they are not correlated with real core/agent/turn
ids. Compare-and-retire means an older turn's guard never retires a
newer turn's scope on the same binding.
core/agent/turn id 均为随机 UUID，只为赋予本 turn 唯一身份，使针对本
turn 的操作员决策无法重放到后续 turn；它们与真实 core/agent/turn 无关。
compare-and-retire 保证旧 turn 的 guard 不会撤销同 binding 上新 turn
的作用域。

`register_scope` REPLACES any prior scope under the same binding
and calls `cancel_binding` on the previous ledger. This is the
documented behaviour: a new public turn from the same binding
auto-cancels outstanding approvals against the prior scope.
`register_scope` 会替换同一 binding 下先前的作用域，并对上一个
账本调用 `cancel_binding`。这就是约定的行为：来自同一 binding
的新公开 turn 会自动取消针对上一个作用域的待决审批。

Because registration happens only after admission, a busy refusal from
any number on the same line cannot replace the live scope or cancel its
pending requests.
由于注册发生在准入之后，同线路任意号码的 busy 消息都无法替换 live
作用域或取消其待决请求。

If `operator_control` is `None` (ledger not installed), the public run
still happens, no scope is opened, and no `telecom.approval.*` method is
registered.
如果 `operator_control` 为 `None`（账本未安装），公开运行依然进行，不打开
作用域，`telecom.approval.*` 方法也不注册。

---

## 5. Startup wiring / 启动接线

`src/bin/aleph-server/commands/start/mod.rs` (after the
`clarification::register_handlers` call):
`src/bin/aleph-server/commands/start/mod.rs`（在
`clarification::register_handlers` 之后）：

```rust
let public_approval_ledger = Arc::new(PublicApprovalLedger::new());
let trusted_operator_control: Option<Arc<OperatorControl>> =
    if install_global(public_approval_ledger.clone()).is_ok() {
        Some(Arc::new(OperatorControl::new(public_approval_ledger.clone())))
    } else {
        tracing::warn!("... operator approval surface disabled"); // static
        None
    };
// inside the handler-registration block:
if let Some(control) = trusted_operator_control.as_ref() {
    operator_control::register_handlers(server.handlers_mut(), control.clone());
}
// later:
initialize_inbound_router(..., trusted_operator_control.clone(), ...)
```

If `install_global` fails, startup fails closed: no handlers, and the
router gets `None`, so the capability slot and the ledger actually in use
can never diverge. A positive source guard in `operator_control.rs` tests
asserts this wiring.
若 `install_global` 失败则 fail-closed：不注册 handler，router 拿到
`None`，保证能力槽与实际使用的账本不会分叉。`operator_control.rs` 中的
正向源码守卫测试断言了这套接线。

The orchestration is intentionally linear: ONE ledger, ONE control,
ONE inbound router. There is no second copy, no ScopedToolService
replacement, and no leak into the owner approval chain.
整个编排刻意做成线性：一份账本、一份 control、一台入站路由器。
不存在第二份拷贝、不替换 ScopedToolService，也绝不泄漏到
所有者审批链中。

---

## 6. Owner-plane leakage guarantees / 所有者平面泄漏保证

These are the structural invariants enforced by the design. They
are properties of the code, not properties of a particular test run.
以下是该设计所强制的结构性不变量。它们是代码的属性，而非
某次特定测试运行的属性。

| Invariant / 不变量 | Where it is enforced / 强制点 |
|--------------------|------------------------------|
| A/B path never reads owner memory. <br> A/B 路径永不读取所有者记忆。 | Production installs no driver; `NullPrivateReadDriver`'s file does not contain `owner_memory` / `owner_workspace` / `owner_secrets` / `owner_history` (structurally asserted by a test). <br> 生产不安装驱动；`NullPrivateReadDriver` 源文件中不包含 `owner_memory` / `owner_workspace` / `owner_secrets` / `owner_history`（由一项测试结构性断言）。 |
| A/B path never writes to owner workspace. <br> A/B 路径永不写入所有者工作区。 | No driver is installed by the binary (`private_read_driver: None`). <br> 二进制不安装任何驱动（`private_read_driver: None`）。 |
| A/B path never fires owner hooks. <br> A/B 路径永不触发所有者钩子。 | `run_public_sms` and `handle_public_telecom` do not call into `crate::hooks::*`. A transient body is dropped (fail-closed). <br> `run_public_sms` 与 `handle_public_telecom` 不会调用 `crate::hooks::*`。transient 正文被丢弃（fail-closed）。 |
| SMS errors do not become owner events. <br> SMS 错误不会变成所有者事件。 | `run_public_sms` already maps all dispositions to its own `PublicRunDisposition` enum (Replied / Refused / Cancelled / Failed). None of these are translated into owner-plane `FlowEvent`s or owner session updates. <br> `run_public_sms` 早已把所有 disposition 映射到自身的 `PublicRunDisposition` 枚举（Replied / Refused / Cancelled / Failed）。这些都不会被翻译为所有者平面的 `FlowEvent` 或所有者会话更新。 |
| The owner approval chain is untouched. <br> 所有者审批链未受影响。 | `OperatorControl` is a NEW JSON-RPC namespace (`telecom.approval.*`); the existing `exec.approval.*` and `clarification.*` handlers and their wiring are unchanged. <br> `OperatorControl` 是一个全新的 JSON-RPC 命名空间（`telecom.approval.*`），现有的 `exec.approval.*` 与 `clarification.*` 处理器及其接线未变。 |
| No `ScopedToolService` replacement of public paths. <br> 不存在以 `ScopedToolService` 替代公开路径的情况。 | `AgentHarnessRunner::run_public_text_impl` still calls `run_public_with` with `NullToolService`; the only added argument is `private_read`. <br> `AgentHarnessRunner::run_public_text_impl` 仍然以 `NullToolService` 调用 `run_public_with`；新增的参数只有 `private_read`。 |

---

## 7. Operator UI / 操作员界面

The webchat operator card is implemented: the list shows a bounded
preview; approve/deny stay disabled until `telecom.approval.review`
returns the complete body; the resolve call is built from that review
(`resolve_params(&TelecomApprovalReview, approve)`) and echoes the full
body. Panel badges refresh on `approval.**` events and when opened;
there is no telecom-specific push event.
webchat 操作员卡片已实现：列表只显示有界预览；在 `telecom.approval.review`
返回完整 body 前，批准/拒绝按钮禁用；resolve 由该 review 构造
（`resolve_params(&TelecomApprovalReview, approve)`）并回传完整 body。
面板角标依赖 `approval.**` 事件与打开时刷新；没有 telecom 专用推送事件。

## 8. Not done / 未完成

- Real private sources (user decision: not enabled for now).
  真实私有来源（用户决定暂不开放）。
- The A consume gate: the orchestrator does not consume approved A
  records, and does not restrict the query to the approved body.
  A consume 闸：编排器不 consume 已批准的 A 记录，也不把查询限定为审批过的 body。
- Production wiring of the B disclosure sink (`deliver_private_disclosure`
  has no production caller).
  B 披露 sink 的生产接线（`deliver_private_disclosure` 无生产调用方）。
- A telecom-specific push event (badges rely on `approval.**` events /
  refresh on open).
  telecom 专用推送事件（角标依赖 `approval.**` 事件 / 打开时刷新）。
- Loopback connections are treated as operator under the existing trust
  model; not revisited here.
  loopback 连接在既有信任模型下视为 operator；本次未重新审视。
- `session_epoch` uses `connection_generation`; whether it changes on
  every rebind, revocation, or session reset is unproven.
  `session_epoch` 使用 `connection_generation`；其是否在每次重绑、撤销、
  会话重置时都变化尚未证明。
- Out of scope: encryption-at-rest of the public history
  (`PublicHistoryConfig::key` / `SecretService`), and the
  `PublicHistoryStartup` install step.
  范围外：公开历史的静态加密（`PublicHistoryConfig::key` / `SecretService`）
  与 `PublicHistoryStartup` 安装步骤。
