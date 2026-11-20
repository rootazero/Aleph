# Severed-Wire Audit — `src/capability` policy & events slice

- 仓库 / repo root: `/home/zou/data/workspace/Aleph-wt-sev3` (HEAD `d57791367`)
- 切片 / slice: `src/capability/policy.rs`, `policy_runtime.rs`, `audit.rs`, `effect_claim.rs`, `event_audience.rs`, `events.rs`, `diagnostic_control.rs`
- 方法: 全文件通读 + 对每个导出符号做仓库级 `rg` 调用者普查（`src/ desktop/ plugins/ crates/ tests/`），生产 vs 测试严格区分
- 只读审计：未修改任何源码，未运行 cargo，未 git add/commit/checkout/stash

---

## 0. 先说结论：哪些线是活的

生产链路 **CONFIRMED ALIVE**（这是本切片唯一真正接上的大线）：

```
src/bin/aleph-server/commands/start/capability_init.rs:7  build_capability_policy
  └─ src/capability/production_init.rs:13  PolicyEngine::with_shared_tree(tree, config.policies.capability_constraints)
src/bin/aleph-server/commands/start/capability_init.rs:15  build_capability_events  → InMemoryEventStream::default()（无持久化）
src/bin/aleph-server/commands/start/capability_init.rs:26  events.subscribe(EventFilter::default())
  └─ src/gateway/capability_events.rs:108 CapabilityEventBridge::start → GatewayEventBus topic "capability.event"（:115）
                                             lag 时发 "capability.event.status"（:118）
src/bin/aleph-server/commands/start/builder/startup_resources.rs:56  install_capability_events(policy, events, bridge)
```

`PolicyEngine` 的**约束链**（config → rule → attachment → admission → event）是通的：
`is_configured`（`policy_runtime.rs:358`）被 `capability_init.rs:50,53` / `src/tools/scoped/dispatch.rs:492` / `src/gateway/execution_engine/slash_command.rs:594` / `src/gateway/mcp_face/mod.rs:245` / `src/gateway/handlers/tools_invoke.rs:267` 调用；`prepare_attachment`（`:380`）由 `zahir_facade.rs:1084` 调用；`admit_runtime`（`:410`）由 `zahir_facade.rs:940` 调用。

但 **`PolicyEngine::resolve` 这条线是断的**，而它恰好是本切片文档上自称的"primary access control gate"（`policy.rs:133-134`）。这就是本次审计的头号发现。

---

## CP-01 — `PolicyEngine::resolve`（自称"primary access control gate"）在生产里没有任何调用者

- Form: no-consumer
- Producer: `src/capability/policy.rs:133` — `pub fn resolve(&self, capability: &CapabilityId, principal: &OwnerRef, visibility: &VisibilityScope, descriptor: &CapabilityDescriptor) -> Result<CapabilityLease, PolicyDenial>`（doc: `policy.rs:133-143` "This is the primary access control gate. It checks: 1. A binding exists for (capability, visibility). 2. not revoked. 3. lifetime not disposed. 4. requesting principal matches binding owner. 5. the visibility scope aligns with the descriptor's constraints."）
- Consumer: **NONE FOUND** — 生产里的 resolve 走 `ZahirFacade::resolve`（`src/capability/zahir_facade.rs:1998`）→ `resolve_with_capture`（`:1453`），它只做 ownership-tree 绑定查找 + owner/generation 校验，**从不调用 `PolicyEngine::resolve`**。
- Live caller search:
  - `rg -n --glob '*.rs' "policy\.resolve\(|policy_engine\.resolve\(|engine\.resolve\(" src desktop plugins crates tests` → 只命中 `src/capability/events_tests.rs:497` 与 `:633`（测试文件）。
  - `rg -n --glob '*.rs' "PolicyEngine" src desktop plugins crates tests` → 生产侧 `PolicyEngine` 只作为 `Arc<PolicyEngine>` 被持有（`production_init.rs:9`、`capability_init.rs:7`、4 个 clone 消费者），没有任何一处调用 `.resolve(`。
  - `policy.rs:413-818` 是 `#[cfg(test)] mod tests`（`policy.rs:412` 附近 `mod tests`）内的调用。
- Symptom: 文档承诺的五道闸里，第 1/2/3/4 道在生产的 `ZahirFacade` 路径上由 `OwnershipTree` 重新实现（generation / owner_of / is_revoked / is_disposed），**第 5 道（visibility scope 对齐）完全不存在**。`PolicyEngine::resolve` 返回的 `CapabilityLease` 类型在生产里从来没有被签发过——生产用的是 `facade::BackendLease`（`policy_runtime.rs:370` `validate_captured_lease` 甚至是**手工把 `BackendLease` 拼成 `CapabilityLease`** 去喂 `validate_lease`）。
- Hypothesis: **CUT** — 两端都在，中间的调用臂缺失。
- Confidence: **HIGH**

---

## CP-02 — `validate_visibility_scope` 是 `VisibilityScope` 约束字段的唯一执法点，而它只挂在 CP-01 这条断线上

- Form: no-consumer（producer/consumer 都存在，但唯一调用者是断线函数）
- Producer: `src/capability/policy.rs:241` — `fn validate_visibility_scope(&self, capability, visibility, descriptor) -> Result<(), PolicyDenial>`，逐项检查：
  - `policy.rs:248-256` approval → `PolicyDenial::ApprovalRequired`
  - `policy.rs:260-263` allowed_kinds（`format!("{:?}", descriptor.kind)` 拼字符串比较）→ `PolicyDenial::KindNotAllowed`
  - `policy.rs:268-270` allowed_ids（`allowed_ids.contains(&capability.name)`）→ `PolicyDenial::IdNotAllowed`
  - `policy.rs:277-280` principal、`:285-288` workspace、`:293-296` session → `PolicyDenial::VisibilityMismatch`
- Consumer: `src/capability/policy.rs:174` — `self.validate_visibility_scope(capability, visibility, descriptor)?;`（**唯一调用点，在 CP-01 的断线函数体内**）
- Live caller search: `rg -n --glob '*.rs' "validate_visibility_scope" src desktop plugins crates tests` → **恰好 2 个命中**：`policy.rs:174`（调用）与 `policy.rs:241`（定义）。无第三个。
- 被约束的字段定义在 `src/capability/ownership.rs:80-95` — `pub struct VisibilityScope { principal, workspace, session, allowed_kinds: Option<Vec<String>>, allowed_ids: Option<Vec<String>>, approval: Option<String> }`，模块 doc（`ownership.rs:74-79`）明说 "the strings are compared against the kind/name spellings **at describe time**" —— 这个 describe-time 比较在仓库里**只存在于 `policy.rs:261-269`**，而那里是断的。
- 生产侧确实也在**写**这些字段吗？没有：
  - `rg -n --glob '*.rs' "allowed_kinds|allowed_ids" src desktop plugins crates` → 只有 `ownership.rs:89/91`（声明）、`policy.rs:260-270`（断线的检查）、`src/gateway/channel_registry/binding.rs:233-234`（只被塞进 `canonical_schema_fingerprint` 当字节，不参与判定）、以及 `ownership.rs:1461/1485`、`binding.rs:1380/1381`（都在 `#[cfg(test)] mod tests`：`ownership.rs:950-951`、`binding.rs:1228-1229`）。
  - `rg -n --glob '*.rs' "approval:" src desktop plugins crates` → 无任何 capability 侧的 `VisibilityScope.approval` 生产赋值点。
  - 生产 descriptor 一律用 `VisibilityScope::default()`（`grant.rs:583`、`ui.rs:214`、`terminal.rs:228`、`backend.rs:388,480`、`provider_registry.rs:173`、`tool_adapters.rs:778`、`projection_host.rs:2084`）或只带 `principal`（`zahir_facade.rs:555`、`:774`）。
- Symptom: `allowed_kinds` / `allowed_ids` / `approval` / `workspace` / `session` 五个维度在生产里**既不被写、也不被读**——两个方向都是暗的。生产者侧唯一还在消费这些字段的地方是 `binding.rs:233-234` 的 fingerprint，它让人误以为约束在生效。
  另外注意：生产的 `ZahirFacade::resolve_with_capture`（`zahir_facade.rs:1473`）会做 `descriptor.visibility = reference.visibility.clone();`，**把 descriptor 自己的约束覆盖成调用方的 scope**，所以 descriptor 级约束连"绑定键"都算不上——真正的约束完全落空到"调用方 scope 必须字面等于注册 scope"（这是键相等，不是语义比较；与 `cap-special-REPORT.md` C03 是同一根因的另一个面）。
- Hypothesis: **CUT**
- Confidence: **HIGH**

---

## CP-03 — 13 个 `PolicyDenial` 变体里 5 个生产不可达；`observe_refusal` 的 `"approval_required"` 分支是死分支

- Form: stub-far-end（活的映射表里有一支永远走不到）
- Producer（唯一产生点）:
  - `src/capability/policy.rs:252` — `PolicyDenial::ApprovalRequired { .. }`（唯一产生点，在断线的 `validate_visibility_scope` 里）
  - `src/capability/policy.rs:263` — `PolicyDenial::KindNotAllowed { kind }`
  - `src/capability/policy.rs:270` — `PolicyDenial::IdNotAllowed { .. }`
  - `src/capability/policy.rs:280` / `:288` / `:296` — `PolicyDenial::VisibilityMismatch`（principal/workspace/session）
  - `src/capability/policy.rs:343` — `PolicyDenial::VisibilityMismatch`（`check_cross_session`，同样只有测试调用者）
- Consumer（活的映射表）: `src/capability/policy_runtime.rs:487-531` — `observe_refusal` 的 `match denial`：
  - `:508` `ConstraintDenied` → `"constraint_denied"`（活）
  - `:509` `InvalidConstraints` → `"invalid_constraints"`（活）
  - `:510` `AttachmentUnavailable` → `"attachment_unavailable"`（活）
  - `:511` `ApprovalRequired` → `"approval_required"`（**不可达**）
  - `:512` `Revoked | Disposed` → `"source_withdrawn"`（活）
  - `:513` `_ =>` → `"policy_denied"`（`KindNotAllowed` / `IdNotAllowed` / `VisibilityMismatch` 会落这里，但它们本身不可达）
- Live caller search: `rg -n --glob '*.rs' "PolicyDenial::" src desktop plugins crates tests` → 逐变体普查，结果见上；生产产生点只有 `policy.rs:203`（Revoked）、`:207`（Disposed）、`:213/:225`（NotFound）、`:216`（GenerationMismatch）、`:228`（PrincipalMismatch）—— 这 5 个都在 `validate_lease` 里，而 `validate_lease` 经 `policy_runtime.rs:369` `validate_captured_lease` → `:406`（`prepare_attachment`）/`:451`（`admit_runtime`）**是活的**。
- Symptom:
  1. 生产的事件流里 **永远看不到** `PolicyViolation.reason == "approval_required"`；任何按这个字符串建的告警规则/客户端分支是死的。
  2. `KindNotAllowed` / `IdNotAllowed` **根本没有专属 reason 映射**——即便将来接线，也会以 `"policy_denied"` 出现，调用方无法区分"种类不允许"和"id 不允许"。
  3. `PolicyDenial::ApprovalRequired` / `KindNotAllowed` / `IdNotAllowed` 三个变体在生产里是**无法构造**的（`unused` 不会报，因为 enum 变体是 pub）。
- Hypothesis: **CUT**
- Confidence: **HIGH**

---

## CP-04 — `CapabilityAttached` / `CapabilityDetaching` 在生产里没有任何发射点

- Form: no-consumer（订阅侧不存在；更准确说是**发射侧不存在**）
- Producer: `src/capability/activation.rs:163` — `CapabilityEvent::CapabilityAttached { .. }`；`src/capability/activation.rs:293` — `CapabilityEvent::CapabilityDetaching { .. }`
- Consumer: **NONE FOUND** — 仓库里（`.rs` / `.ts` / `.tsx` / `.py` / `.json` / `.md`）除定义外无任何对 `"capability_attached"` / `"capability_detaching"` 的引用；`src/capability/audit.rs:69-94` 的 `action()` / `result()` 会把它们映射成 `"attached"` / `"detaching"`，但 `audit.rs` 本身无生产消费者（见 CP-08）。
- Live caller search:
  - `rg -n --glob '*.rs' "CapabilityAttached|CapabilityDetaching" src desktop plugins crates tests` → 只在 `events.rs`（变体定义 + `event_type()` 映射）、`activation.rs:163,293`（构造点）、`audit.rs`（映射）出现。
  - 根因 grep：`rg -n --glob '*.rs' "new_observed" src desktop plugins crates tests` → **唯一调用者 `src/capability/events_tests.rs:290`（测试）**。`CapabilityActivationController::events` 字段（`src/capability/activation.rs:89`）为 `Some(ActivationEventHook)` 仅当经 `new_observed`（`activation.rs:107-118`）构造；生产用 `CapabilityActivationController::new`（`activation.rs:101`，`events: None`）与 `from_lifecycle`（`activation.rs:131`，`events: None`）。
  - `ActivationEventHook::new`（`events.rs:919`）是唯一给 `ActivationEventHook` 装 emitter 的构造器，其唯一调用者是 `activation.rs:107` 的 `new_observed`。
- Symptom: 事件词汇表里有 `capability_attached` / `capability_detaching` 两种类型（`events.rs:20-30` 的 `CapabilityEventType::Attached` / `Detaching`），但**服务器一次都不会发**。客户端订阅后等 attach/detach 通知会永远等不到；真机 QA 探针 `qa/capability_i3/probe.py:424,439-456` 只检查 `capability_activated` / `capability_revoked`（这两个生产有发），所以 QA **不会**因此变红——缺陷是静默的。
- Hypothesis: **CUT**
- Confidence: **HIGH**

---

## CP-05 — 类型化的 `ConstraintViolation.constraint_type`（"time"/"resource"/"conditional"/"context"）生产不可达；生产只发常量 `"constraint"`

- Form: name-path-drift（同一字段两套拼写，两套里只有一套活着）
- Producer（类型化那套）: `src/capability/events.rs:941-957` — `ActivationEventHook::violation`，把 `ConstraintViolation::InvalidTimeWindow | TimeWindowClosed` → `"time"`、`ResourceLimitExceeded` → `"resource"`、`ConditionNotMet` → `"conditional"`、`InvalidContext | ClockUnavailable` → `"context"`
- Consumer（生产那套）: `src/capability/policy_runtime.rs:515-524` — `observe_refusal` 里 `constraint_type: "constraint".into()`
- Live caller search:
  - `rg -n --glob '*.rs' "constraint_type" src desktop plugins crates tests` → 生产写入点只有 `policy_runtime.rs:521`；类型化写入点 `events.rs:954` 属 `ActivationEventHook`（见 CP-04，整条路径生产不可达）。
  - `rg -n --glob '*.rs' "ActivationEventHook" src desktop plugins crates tests` → `events.rs:910-1000`（定义）、`activation.rs:89,107`（持有/构造），生产构造路径已断。
  - `rg -n --glob '*.rs' "ConstraintViolation" src desktop plugins crates tests` → 生产构造点只有 `policy_runtime.rs:519`。
- Symptom: 消费者若按 `constraint_type` 分桶，生产只能看到唯一一个桶 `"constraint"`；四个语义化取值一次都不会出现。想按"是时间窗挂了还是资源超限了"做告警的规则永远匹配不到。
- Hypothesis: **CUT**（两套拼写需择一，另一套要么接线要么删）
- Confidence: **HIGH**

---

## CP-06 — `CapabilityActivated.constraints_applied` 在生产里恒为 0

- Form: other-static-defect（字段语义与实际取值不符）
- Producer: `src/capability/events.rs:509-512` — `SourceObservation::flush` 硬编码 `CapabilityActivated { constraints_applied: 0, .. }`；字段 doc 在 `events.rs:54` 写的是 "the number of successfully evaluated constraints"
- 真实计数 producer: `src/capability/events.rs:966-969` — `ActivationEventHook::after_activation` 带上真实的 `self.constraints_applied`（**生产不可达**，见 CP-04）
- Live caller search: `rg -n --glob '*.rs' "constraints_applied" src desktop plugins crates tests` → 写入点只有 `events.rs:512`（恒 0）与 `events.rs:969`（死）；`events_tests.rs:333` 断言 `== 1`（测试路径），`audit.rs:248` 造 `2`（audit 无生产消费者）。
- Symptom: 运维/客户端从事件里读到的 "已生效约束条数" 永远是 0，即使该能力真的挂了 N 条 time/resource/conditional 约束。要么改 doc 说"当前恒为 0"，要么把真实计数接上。
- Hypothesis: **CUT**
- Confidence: **HIGH**

---

## CP-07 — `CapabilityRevoked.reason` 在生产里只有 `"source_withdrawn"`；`"requested"` 不可达

- Form: name-path-drift
- Producer: `src/capability/events.rs:518-521` — `SourceObservation::flush` 发 `reason: "source_withdrawn"`；`events.rs:977` / `events.rs:1000` 发 `reason: "requested"`（`ActivationEventHook` 路径，生产不可达）
- Consumer: 事件经 `src/gateway/capability_events.rs:115` 原样转发到 `GatewayEventBus` topic `"capability.event"`；`src/gateway/event_visibility.rs:1078-1082` 做 audience 过滤（不解析 reason）
- Live caller search: `rg -n --glob '*.rs' "CapabilityRevoked" src desktop plugins crates tests` → 生产构造点只有 `events.rs:518`；`rg -n "capability_revoked" --include='*.py' --include='*.ts'` → 只有 `qa/capability_i3/probe.py:439-456`（只判 type，不判 reason）。
- Symptom: 用户主动 revoke 与源被撤回在事件里**不可区分**（reason 都是 "source_withdrawn"）。
- Hypothesis: **CUT**
- Confidence: **HIGH**

---

## CP-08 — `src/capability/audit.rs` 整个模块没有生产消费者

- Form: no-consumer
- Producer: `src/capability/audit.rs:143` — `pub struct AuditCollector`（`:148` `new`，`:152` `query`，`:186` `query_scoped`，`:198` `report`）；`src/capability/audit.rs:39` `AuditLog`、`:118` `AuditQuery`、`:127` `ComplianceReport`
- Consumer: **NONE FOUND** — 唯一的生产侧构造点是 `src/capability/projection_host.rs:869-870` 的 `ProjectionHost::audit_collector()`
- Live caller search:
  - `rg -n --glob '*.rs' "audit_collector" src desktop plugins crates tests` → 只有 `projection_host.rs:869`（定义）与 `src/capability/events_scoped_tests.rs:702`（**测试**）。
  - `rg -n --glob '*.rs' "AuditCollector|AuditQuery|ComplianceReport|AuditLog" src desktop plugins crates tests` → 其余命中全在 `audit.rs` 自身与 `events_tests.rs:641,691` / `events_scoped_tests.rs:187,308,514,528,590,702`（测试）。
  - 注意：网关另有 `src/gateway/security_audit.rs` 的 `SecurityStore` 审计，是**另一套**东西，不消费本模块。
- Symptom: 能力事件的可查询审计轨迹（按时间窗 / event_type / resource / actor 查询 + 合规报告）在代码里完整存在，但**运行中的服务器没有任何出口能拿到它**。运维要查"某资源发生过哪些能力事件"只能去翻内存广播。
- Hypothesis: **DECIDE** — 要么把它接到某个运维/诊断 RPC，要么标记为未上线。
- Confidence: **HIGH**

---

## CP-09 — `EventPersistence` 没有任何生产实现；能力事件在重启后全部丢失

- Form: no-consumer / stub-far-end
- Producer: `src/capability/events.rs:185-190` — `pub trait EventPersistence: Send + Sync { fn persist(...) }`；`src/capability/events.rs:302` — `InMemoryEventStream::with_persistence(...)`
- Consumer: **NONE FOUND** — 生产构造流用的是 `InMemoryEventStream::default()`
- Live caller search:
  - `rg -n --glob '*.rs' "EventPersistence|with_persistence" src desktop plugins crates tests` → 实现点只有测试：`src/bin/aleph-server/commands/start/builder/startup_resources.rs:233`（`ShutdownAudit`，位于该文件的 `#[cfg(test)] mod` 内）、`src/gateway/capability_events_tests.rs:716,1083`、`src/capability/events_tests.rs:241`。
  - 生产构造：`src/bin/aleph-server/commands/start/capability_init.rs:15-17` → `InMemoryEventStream::default()`（**不带** persistence）。
  - 唯一形似生产调用的 `startup_resources.rs:278` 落在该文件的 `#[cfg(test)]` 模块内。
- Symptom: 进程重启后 capability event 历史清零，没有 durable 审计。**注意边界**：AGENTS.md「Capability Phase 4 入口」明说 broadcast 是 live notification（256 槽、lag 可丢）、"durable recovery 只认 `SessionEventStore`"——所以"事件不是 durable recovery 源"是**已文档化的设计**；本条的缺陷只在于 `EventPersistence` / `with_persistence` 这条接口在生产里从未被使用（写了一个 trait 却没有任何生产实现）。
- Hypothesis: **DECIDE**（若 trait 计划用于未来的 durable 事件存储，需在代码里标注"未接线"，否则是接口幻觉）
- Confidence: **HIGH**（事实）/ **MEDIUM**（是否算缺陷，取决于文档口径）

---

## CP-10 — `EventFilter` 的三个字段在生产里全是死旋钮

- Form: inert-config
- Producer: `src/capability/events.rs:146` — `pub struct EventFilter { event_type: Option<CapabilityEventType>, resource: Option<String>, actor: Option<String> }`
- Consumer: 生产唯一的订阅点是 `src/bin/aleph-server/commands/start/capability_init.rs:26` — `events.subscribe(EventFilter::default())`，**三个字段全为 `None`**（不过滤）
- Live caller search:
  - `rg -n --glob '*.rs' "EventFilter" src desktop plugins crates tests` → 设置字段的地方只有 `src/capability/audit.rs:162`（audit 无生产消费者）与 `subscribe_events` 的调用点（全部是测试：`events_scoped_tests.rs:339,418` 等）。
  - `rg -n --glob '*.rs' "EventFilter::default\(\)" src desktop plugins crates tests` → 生产仅 `capability_init.rs:26` 一处；其余均测试。
- Symptom: 事件过滤是**纯客户端**能力——网关桥把所有能力事件无条件转发到 `"capability.event"`，服务端不裁剪。一台机器上大量能力事件会全部推给每个订阅连接。
- Hypothesis: **CUT**（过滤要么下沉到服务端，要么明确标注"客户端过滤"）
- Confidence: **HIGH**

---

## CP-11 — `TrustedEventScope` / `CapabilityEventAccess` / `subscribe_scoped` 整条 scoped 事件链生产不可达

- Form: no-consumer
- Producer:
  - `src/capability/event_audience.rs:113-116` — `pub trait CapabilityEventAccess { async fn admits(&self, record) -> bool }`
  - `src/capability/event_audience.rs:119-129` — `pub struct TrustedEventScope` + `from_access` / `admits`
  - `src/capability/events.rs:330` — `InMemoryEventStream::subscribe_scoped(scope, filter)`
  - `src/capability/facade.rs:399-408` — `Zahir::subscribe_events` 的**默认桩**，返回 `Err(EventError::Unsupported)`
- Consumer: **NONE FOUND**
- Live caller search:
  - `rg -n --glob '*.rs' "subscribe_events" src desktop plugins crates tests` → 只有 `facade.rs:399`（trait 默认）、`events.rs:891-896`（`AuditedZahir` 转发）、`zahir_facade.rs:1858`（`Zahir for ZahirFacade` 实现）+ 全是测试的调用点（`events_tests.rs:108,132`、`events_scoped_tests.rs:684,692`、`gateway/capability_events_tests.rs:779`）。**无任何生产调用者**。
  - `rg -n --glob '*.rs' "subscribe_scoped" src desktop plugins crates tests` → 生产侧唯一是 `zahir_facade.rs:1866`（在上面的 `subscribe_events` 实现体内，同样不可达）+ 全测试。
  - `rg -n --glob '*.rs' "from_access|TrustedEventScope|CapabilityEventAccess" src desktop plugins crates tests` → `src/gateway/capability_events.rs:25` `GatewayCapabilityEventAccess::for_connection` 是唯一的生产实现构造点，而它的调用者只有 `src/gateway/capability_events_tests.rs:172,213,772`（测试）。
- Symptom: "按连接做受众裁剪"的订阅契约是暗的。**但请注意安全性没有被削弱**：生产的受众过滤走的是另一条平行链路——`src/gateway/event_visibility.rs:1026` `capability_audience_admits`（由 `:1082` 与 `src/gateway/capability_events.rs:65` 调用）配合 `EventAudience::from_produced_frame`（`event_audience.rs:40`，生产用于 `src/gateway/event_visibility.rs:1078`），并在 `src/gateway/server/connection/mod.rs:1637-1639` 按 topic `capability.event` / `capability.event.status` / `capabilities.ui.display` 做连接级过滤。`EventAudience::from_call`（`event_audience.rs:95`）是活的（`events.rs:605,713`、`zahir_facade.rs:1444`、`src/gateway/capability_ui.rs:152`）。所以这是**接口不可达**，不是越权泄漏。
- Hypothesis: **DECIDE**（与 `cap-special-REPORT.md` C04 交叉确认；二者指向同一根因：facade 层事件订阅面从未接入网关）
- Confidence: **HIGH**

---

## CP-12 — `PolicyEngine::with_events` → `AuditedPolicyEngine` → `AuditedZahir` 三层审计包装生产零调用

- Form: no-consumer
- Producer: `src/capability/policy.rs:128` — `pub fn with_events(self, events) -> AuditedPolicyEngine`；`src/capability/events.rs:718` — `pub struct AuditedPolicyEngine`（`:724` `new`，`:749` 转发 `validate_lease`，`:761` 发 `PolicyViolation`）；`src/capability/events.rs:776` — `pub struct AuditedZahir`（`:783` `new`，`:891` 转发 `subscribe_events`，`:798` 发 `PolicyViolation`）
- Consumer: **NONE FOUND**
- Live caller search:
  - `rg -n --glob '*.rs' "with_events" src desktop plugins crates tests` → `events_tests.rs:401,427,495,630`（PolicyEngine）+ `events_tests.rs:452,518`（`CapabilityHandlerRegistry` 的**同名但不同**方法）+ `src/capability/backend.rs:147` `LegacyCapabilityHandlerRegistry::with_events`（该字段自带 `#[cfg_attr(not(test), expect(dead_code))]`，**代码自己承认它是死的 legacy 路径**）。生产侧无调用。
  - `rg -n --glob '*.rs' "AuditedPolicyEngine" src desktop plugins crates tests` → 只有 `policy.rs:128`（返回）与 `events.rs:718,723`（定义/构造）。零生产消费者。
  - `rg -n --glob '*.rs' "AuditedZahir" src desktop plugins crates tests` → `resolver_audited_tests.rs:36`、`events_tests.rs:126,578,683`、`events_scoped_tests.rs:686`、`resolver_tests.rs:171` — **全是测试**。
  - `AuditedPolicyEngine::tree`（`events.rs:768`）→ **零调用者**（连测试都没有）。
- Symptom: "给 PolicyEngine 套一层事件审计"的完整机制在生产里从未启用。**好消息**：生产的 `PolicyViolation` 事件是活的——由 `src/capability/policy_runtime.rs:487` `observe_refusal` 发出（`:526-531`），所以"拒绝会被观测到"这个性质成立；断的只是"审计包装器"这条可选路径。
- Hypothesis: **CUT**
- Confidence: **HIGH**

---

## CP-13 — `can_delegate` / `check_cross_session` 生产零调用（跨会话授权整体缺席）

- Form: no-consumer
- Producer: `src/capability/policy.rs:311` — `pub fn can_delegate(&self, ...) -> bool`；`src/capability/policy.rs:334` — `pub fn check_cross_session(...) -> Result<(), PolicyDenial>`（源码注释：`"Cross-session grants are deferred to a later phase"`）
- Consumer: **NONE FOUND**
- Live caller search: `rg -n --glob '*.rs' "can_delegate|check_cross_session" src desktop plugins crates tests` → 只有 `policy.rs:311`（定义）、`:334`（定义）、`:736-757`（`can_delegate` 测试）、`:766,776`（`check_cross_session` 测试），全在 `policy.rs` 的 `#[cfg(test)] mod tests` 内。
- 反证（重要）: `src/capability/events.rs:713-715` 的注释写着 "Resource-free delegation/cross-session helpers remain on the underlying engine." —— 注释暗示它们在用，实际**没有任何生产调用者**。
- Symptom: "谁可以把能力委派给谁"与"跨会话授权"这两条策略在生产里没有任何执行点；任何跨会话/委派路径目前靠调用方自觉。
- Hypothesis: **CUT**（`check_cross_session` 自带"deferred to a later phase"注释 ⇒ 部分已文档化的延期；但 `can_delegate` 无延期标注）
- Confidence: **HIGH**

---

## CP-14 — `PolicyRuntime::withdraw_attachment` 生产零调用；生产只按 fact-source key 撤回

- Form: no-consumer（+ 轻微 name-path-drift）
- Producer: `src/capability/policy_runtime.rs:606-623` — `pub fn withdraw_attachment(&self, attachment: &PolicyAttachment)`（按**单个 attachment 身份**撤回并 `state.revoke()`）
- Consumer: **NONE FOUND**
- Live caller search: `rg -n --glob '*.rs' "withdraw_attachment" src desktop plugins crates tests` → 只有 `policy_runtime.rs:606`（定义）+ `policy_runtime_tests.rs:196,282`（测试）。
  生产实际走的是 `withdraw_fact_source`（`policy_runtime.rs:637-648`），调用点 `src/capability/special.rs:294`、`src/capability/ui_surface.rs:292`。
- Symptom: 两者语义不同——`withdraw_fact_source` 会把**所有** `fact_key` 相同的 attachment 一并 revoke 并删除（`policy_runtime.rs:638-648` 的 `retain`），`withdraw_attachment` 只撤一个具体 attachment。生产因为只用了前者，**无法单独撤回某一个 attachment**（也可能是刻意的：同一 fact source 的约束应整体失效）。接口层面存在一条永远不走的精细化撤回路径。
- Hypothesis: **DECIDE**
- Confidence: **MEDIUM**（是否为缺陷取决于"单 attachment 撤回"是否是需要的能力；证据不足以判定功能缺失）

---

## CP-15 — `observe_refusal` 把 `Revoked` 与 `Disposed` 折叠成同一个 reason `"source_withdrawn"`

- Form: name-path-drift
- Producer: `src/capability/policy_runtime.rs:512` — `PolicyDenial::Revoked | PolicyDenial::Disposed => "source_withdrawn"`
- Consumer: 该 reason 进入 `CapabilityEvent::PolicyViolation { reason }`（`policy_runtime.rs:526-531`），经 `src/gateway/capability_events.rs:115` 原样推给客户端；仓库内**没有**按 reason 分支的生产消费者（`src/capability/audit.rs:69-94` 会映射，但 audit 无生产消费者）
- Live caller search: `rg -n --glob '*.rs' "source_withdrawn" src desktop plugins crates tests` → `policy_runtime.rs:512`（映射）与 `events.rs:518`（真正的"源被撤回"事件用的同一个词）。
- Symptom: 两种语义完全不同的失败（能力被 revoke / 运行时被 dispose）对外呈现为同一个 `"source_withdrawn"`，而且这个词与 `events.rs:518` 真正的源撤回事件撞名 → 客户端无法区分"源没了"和"租约过期/运行时被销毁"。这也是 CP-04 提到的 `reason: "requested"` 缺失的另一面。
- Hypothesis: **DECIDE**
- Confidence: **MEDIUM**（机制确定；是否要拆分 reason 取值属于产品口径）

---

## CP-16 — `CapabilityEventStream::emit_scoped` 的默认实现是静默丢事件的桩

- Form: stub-far-end（当前不可达，属健壮性隐患）
- Producer: `src/capability/events.rs:192-201` — `pub trait CapabilityEventStream` 的 `fn emit_scoped(...)` 默认实现 `Err(EventError::Unsupported)`
- Consumer: `src/capability/events.rs:436-443` — `CapabilityEventEmitter::emit_scoped` 在 `Err` 时只 `tracing::warn!(...)`，**不重试、不上抛**
- Live caller search: `rg -n --glob '*.rs' "impl CapabilityEventStream for" src desktop plugins crates tests` → 只有 `events.rs:350`（`InMemoryEventStream`，真正实现了 `emit_scoped`，`events.rs:355`）与 `events_tests.rs:98`（测试用 `LegacyStream`）。
- Symptom: 当前生产只有 `InMemoryEventStream` 一个实现，所以这个桩**不可达**；但任何未来新增的 `CapabilityEventStream` 实现若忘记写 `emit_scoped`，所有**带受众裁剪**的事件（`ConstraintViolation` / `PolicyViolation` / `CapabilityInvoked`）会被静默丢弃，只留一行 warn 日志——即"看起来在发，其实没发"。
- Hypothesis: **DECIDE**
- Confidence: **MEDIUM**（当前不可达，不构成现行生产缺陷）

---

# 非连线类静态缺陷

（不属于六种连线形态，但同属本切片、真实存在）

## CS-01 — `EffectClaim` 生产侧缺席（与 AGENTS.md 记载一致，**不算缺陷**）

- `src/capability/effect_claim.rs:40` `effect_claim_event_from_session`、`:114` `reconcile_session_events`、`:130` `reconcile_effect_claim` 全无生产调用者；`src/session/events.rs:766-795` 的 `SessionEvent::EffectClaimPrepared/Claimed/Invoking/Terminal` 也没有任何生产构造点。
- 与 AGENTS.md「Capability Phase 4 入口」"EffectClaim producer integration deferred（无生产 `SessionEvent::EffectClaim*` 构造点，下期独立 approval）"**完全一致** → 按 brief 规则 4 记为**已文档化延期，不报为断线**。此处仅记录：代码与文档无矛盾。

## CS-02 — `LegacyCapabilityHandlerRegistry` 是代码自认的 legacy 死路径

- `src/capability/backend.rs:147` `with_events` 所在字段带 `#[cfg_attr(not(test), expect(dead_code))]`，`backend.rs:231` 的 `CapabilityInvoked` 发射同理。
- 这是**显式标注**的 legacy 路径，不构成新缺陷；记录在此以免后续审计重复报。

## CS-03 — `DiagnosticControl` 全部 7 个方法生产可达，**不是缺陷**

（已逐个验证，避免误报）
- 生产构造：`src/bin/aleph-server/commands/start/mod.rs:79` `DiagnosticControl::new(host, tree)`（开关：`start/mod.rs:67` `diagnostics_enabled_from_env`，仅 `ALEPH_CAPABILITY_DIAGNOSTICS == "1"` 精确匹配）
- 注入工具：`src/executor/builtin_registry/config.rs:192` `BuiltinToolConfig::diagnostics_control` → `src/executor/builtin_registry/builder/constructor/mod.rs:940`（catalog 门控）与 `:1484`（存入 registry）→ `src/executor/builtin_registry/registry/tool_registry_impl.rs:1620`（dispatch arm 读 `self.diagnostics_control`，缺失即 fail-closed 报错）
- 生产消费者：`src/builtin_tools/capability_projection_diagnostics.rs:429` `status` / `:433` `bump_runtime` / `:439` `revoke_tool` / `:446` `dispose_runtime` / `:451` `hold` / `:456` `release` / `:460` `close`（该文件 `#[cfg(test)] mod tests` 从 `:568` 开始 ⇒ 以上均为生产代码）
- 结论：`src/capability/diagnostic_control.rs:124/143/163/209/251/284` 全部有活的生产调用者。

## CS-04 — 配置 `policies.capability_constraints` 的每个字段都被生产代码消费，**不是死旋钮**

- `src/config/types/policies/capability_constraints.rs` `CapabilityConstraintRule { time, resource, required_conditions }` → `:51` `compile_constraints` → `Constraint::time_window` / `resource_limit` / `conditional`；`resource.unit` → `CompiledConstraintRule.resource_unit`；`required_conditions` 的 key → `rule.required_conditions`
- 消费点：`src/capability/policy_runtime.rs:395`（`supported_conditions` 不支持 → `InvalidConstraints`）、`:401`（`supported_units` 不支持 → `InvalidConstraints`）
- 生产 `ConstraintFactsSource` 实现只有两个：`src/capability/zahir_facade.rs:122` `ToolPolicyFacts`（conditions `["provider_ready"]`，units `[ActiveCalls]`）、`:167` `SpecialPolicyFacts`（UI → `["provider_ready","attachment_ready"]`；Terminal → units `[ActiveCalls, PtySessions]`）
- 结论：**全部字段有生产读路径**，不属 inert-config。

## CS-05 — `EffectClaimState` / `EffectClaimEvent` / `EffectClaimReconciliation` 三个类型无生产消费者

- 与 CS-01 同源、且已被 AGENTS.md 记为延期 → 不单独计为缺陷。

---

# 已验证"确实接上"的清单（防重复报）

| 符号 | 生产调用者 |
|---|---|
| `PolicyEngine::is_configured` (`policy_runtime.rs:358`) | `capability_init.rs:50,53`；`src/tools/scoped/dispatch.rs:492`；`src/gateway/execution_engine/slash_command.rs:594`；`src/gateway/mcp_face/mod.rs:245`；`src/gateway/handlers/tools_invoke.rs:267`（后者用它在 raw `tools.invoke` 上**拒绝**受约束工具） |
| `PolicyEngine::resource_unit` (`policy_runtime.rs:362`) | `zahir_facade.rs:932` |
| `PolicyEngine::prepare_attachment` (`:380`) / `admit_runtime` (`:410`) | `zahir_facade.rs:1084` / `zahir_facade.rs:938` |
| `PolicyEngine::validate_lease` (`policy.rs:199`) | 经 `policy_runtime.rs:369` `validate_captured_lease` → `:406`/`:451`（活） |
| `PolicyRuntime::install_event_observer` (`:466`) | `zahir_facade.rs:416` |
| `PolicyRuntime::invocation_observation` (`:473`) | `zahir_facade.rs:1047`；`src/tools/scoped/dispatch.rs:1508` |
| `PolicyRuntime::observe_refusal` (`:487`) | `zahir_facade.rs:918,1098` |
| `PolicyRuntime::withdraw_fact_source` (`:637`) | `src/capability/special.rs:294`；`src/capability/ui_surface.rs:292` |
| `PolicyRuntime::stop_admission` (`:563`) / `wait_for_idle` (`:572`) | `src/bin/aleph-server/commands/start/builder/startup_resources.rs:83` / `:101` |
| `EventAudience::from_call` (`event_audience.rs:95`) | `events.rs:605,713`；`zahir_facade.rs:1444`；`src/gateway/capability_ui.rs:152` |
| `EventAudience::from_produced_frame` (`event_audience.rs:40`) | `src/gateway/event_visibility.rs:1078` |
| `invocation_audience` (`events.rs:678`) | `policy_runtime.rs:506`；`events.rs:644` |
| `InvocationObservation` (`events.rs:626`) | `policy_runtime.rs:473` |
| `SourceObservation::flush` (`events.rs:489`) | 生产事件主来源：Created/Activated/Revoked 三类事件的唯一生产发射点 |
| `CapabilityEventBridge::start` | `src/gateway/capability_events.rs:108` ← `capability_init.rs:25-28` |
| `DiagnosticControl` 全部 7 方法 | 见 CS-03 |
| `PolicyEngine::tree` (`policy.rs:353`) | **仅** `events_tests.rs:431`（测试）→ 轻度死接口，见 CP-16 邻接说明 |
| `AuditedPolicyEngine::tree` (`events.rs:768`) | **零调用者** |

---

# 与既有报告的交叉

- `review-results/sev-wire-2026-11-20/cap-special-REPORT.md` **C04**（`Zahir::subscribe_events` 无生产消费者）与本报告 **CP-11** 指向同一根因（facade 层事件订阅面从未接入网关），两份报告可互相印证；CP-11 额外覆盖了 `TrustedEventScope` / `CapabilityEventAccess` / `subscribe_scoped` 三件套。
- `review-results/sev-wire-2026-11-20/cap-special-REPORT.md` **C03**（visibility 按字面键比较、不是通配）与本报告 **CP-02** 是同一根因的两面：C03 讲"键相等不等于通配"，本报告讲"描述符级语义约束整体缺席"。
- `review-results/severed-wire-2026-08-17/capability/REPORT.md`（2026-08-17）与 `review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md` 中**没有**涉及本切片上述符号的条目 → 本报告的 CP-01…CP-16 均为**新发现**，非回归。
- 兄弟切片 `cap-resolve-REPORT.md` CR-01（`CapabilityResolver` 无生产构造入口）与本报告 CP-01 同属"facade 层已上线 / 旧 policy 层未接线"的家族，但对象不同，不重复计数。

---

# 未覆盖的接缝

1. **未运行编译/测试/lint**（brief 硬规则：read-only、不得跑 cargo）。所有"无调用者"结论均基于仓库级 `rg` 的**静态**普查；**没有** `cargo clippy -- -W dead_code` 的编译器二次确认。已知的漏检风险：本仓库存在宏展开生成的调用点（如 `crate::utils::source_scan::production_prefix` 那种"按源码文本断言接线"的测试，`capability_init.rs:94-154` 就是这种形态），它们只能证明"启动代码里有这几个 token"，不能证明"运行时真的走了这条路"。
2. **未审计 `src/gateway/**` 与 `src/tools/**` 的实现体**（只核对了它们对本切片符号的调用点）。特别是 `src/gateway/channel_registry/binding.rs:233-234` 的 fingerprint 用途、以及 `src/gateway/event_visibility.rs` / `src/gateway/capability_events.rs` 的受众裁剪语义，只做了调用关系核对，没有逐行验证其正确性。
3. **未审计 Panel / 前端（TS/TSX）对 capability 事件的消费**：只 grep 了事件类型字符串（`capability_attached` / `capability_detaching` 在非 Rust 文件中零命中）。前端若通过 JSON 字段名而非类型名订阅，仍可能有未覆盖的期待。
4. **`graphify` 语义图未能提供增量信息**：`graphify query "how are capability events published and consumed" --budget 600` 返回的是 `builtin_tools/hooks_manage.rs`、`clarification/session.rs` 等无关节点（250 节点被截断，语义匹配跑偏），本报告结论**不依赖**该工具，全部以 `rg` 为准。
5. **未验证 `docs/` 与实现的一致性之外的口径**：只核对了几处关键声明（`AGENTS.md`「Capability Phase 4 入口」、`docs/superpowers/specs/2026-10-06-capability-phase4-follow-up-gates-design.md:141,:232`）。**没有任何文档声明** descriptor 级 `allowed_kinds` / `allowed_ids` 检查被延期 ⇒ CP-02 是**未文档化的缺口**，不是已记录的延期（区别于 CS-01 的 EffectClaim）。
6. **`src/capability/activation.rs`、`src/capability/zahir_facade.rs`、`src/capability/projection_host.rs` 不在本切片**，我只把它们当作消费端证据引用；这些文件内部的完整断线状况由兄弟切片报告（cap-resolve / cap-special / cap-ownership）负责。
7. **`PolicyDenial` 变体的"应然"语义未与产品口径核对**：CP-03 / CP-15 判定的是"机制上不可达 / 命名撞车"，至于生产**是否应该**能发出 `approval_required`、是否应该区分 `revoked` 与 `disposed`，需要产品决策。
