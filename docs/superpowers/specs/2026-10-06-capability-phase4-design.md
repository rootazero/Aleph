# Capability Phase 4 — 设计规范

| 项目 | 取值 |
| --- | --- |
| 文档性质 | 设计规范（非已实现代码） |
| 适用分支 | `capability-phase4` |
| 文档基线 | `a5300221d27c7a916407d8b96cadccb91cde1bbf` |
| 作者 | Pi subagent（设计阶段委托） |
| 日期 | 2026-10-06 |
| 后续工作 | 实施任务与 reference 文档补全见末尾 §13 |

> 本文档不修改任何源代码。所有 `src/...` 路径只为定位既有的概念基线（Gap Analysis）；下文出现的所有「应当」「将」字样均为设计意图，不是 commit 时已存在的代码。

---

## 1. 目标、问题重述与本期范围

### 1.1 目标

在保留 Phase 3 已经定下的事实源（Tool / MCP / builtin / markdown-skill 不回归）的前提下，把 Aleph 的能力面从「单个 Tool 的注册 + 调用」扩展到「Tool / Skill / Agent / Task / Resource / EventSource / Subscription / Plugin / Hook 的统一描述与生命周期管理」。本期落地的核心是 **CapabilityDescriptor + Zahir facade**，并把现有的散点（Tool/MCP/extension/registry/plugin/gateway session_run/event bus/resilience database/state_database 模块挂载点）统一收敛到一个 ownership tree：**Runtime → Session → Run → Task → effect claim**。

### 1.2 问题重述

Phase 3 之后，仓库中同时存在以下「看起来都是 capability」但**事实上各自为政**的子系统：

| 既有能力面 | 实际承担职责 | 缺失 |
| --- | --- | --- |
| `src/tools/registry.rs` 下的 `ToolHandlerRegistry` | 已有 ToolCapabilityDescriptor、revision、ArcSwap snapshot / change feed、generation-safe disposal | 缺口是跨 kind facade / owner / recovery |
| `src/extension/registry/plugin_registry/mod.rs` | Plugin 模块加载 | 与 Tool registry 完全独立；调用契约不一致 |
| `src/gateway/execution_engine/session_run_registry.rs` | Gateway 维度 Run 注册 | 不知道「这个 Run 用了哪些 capability」 |
| `src/session/service.rs` | SessionService event-log facade | actor/store 已有 `emit_event`/`emit_batch`/`get_events` | 缺口是不把 capability 作为 first-class resource |
| `src/resilience/database/state_database/mod.rs` | 操作型 projection | Recovery 路径未与 Session event store 绑定 |
| `src/acp/manager/persistence.rs` | best-effort ACP session JSON persistence | 是 best-effort 持久化，不担任事实源 |
| `src/event/global_bus.rs` | 跨子系统通知 | 没有 cursor / generation |
| `src/capability/mod.rs` | 启动期 CapabilitySlot，不是动态 facade | 启动期固化能力槽；不解决跨 kind facade / owner / recovery |
| `src/runtimes/ensure.rs` | Runtime 装配 | 不感知 capability 的 generation / revoke |

问题陈述：**事实源分裂**。任何一个客户端（ACP/MCP / Panel / Bot）想要回答「现在系统里有哪些 capability」「这个 capability 现在由谁拥有」「它能否被 replay」都必须联合多个 registry + 数据库 + 事件总线才能拼出来，并且拼出的结果彼此不一致。

### 1.3 本期范围（分阶段）

| 阶段 | 名称 | 交付物 | Gate |
| --- | --- | --- | --- |
| A | **Zahir facade + CapabilityDescriptor** | 新增 `src/capability/{descriptor,facade}.rs`；`CapabilityKind` 9 种；`describe / resolve / subscribe / project` 四方法；不改任何现有事实源 | Gate A |
| B | **Ownership tree** | `src/capability/ownership.rs`；`Runtime → Session → Run → Task → effect claim`；`VisibilityScope` / `LifetimeScope` 分离；`generation / revoke / dispose` | Gate B |
| C | **Session event store 作为 recovery source** | 收敛/标记现有 `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/store.rs` 的 `SessionEventStore`/`SqliteEventStore` 为 recovery source；不新增事件存储子模块/目录；对 `StateDatabase` 加 projection-only 语义标记，对 `GlobalBus` 加 notification-only 语义标记；不改变其现有行为；`effect sandwich`、`fencing token`、`ReplayPermit`/`ReplayPreparer`/闭包校验到位 | Gate C |
| D | **ACP / MCP projection 收紧** | `acp/manager/persistence.rs` 与新 `MCP projection` 模块都标记为「projection-only」；`to_metadata_form` 只能 wrapper | Gate D |

非本期范围见 §12。

---

## 2. 既参考机制与不照搬的原因

### 2.1 Pi Durable

```
┌──────────────┐    durable activity    ┌──────────────┐
│  Client SDK  │  ──────────────────▶  │  Pi Durable  │
│  (tool call) │                       │  Scheduler    │
└──────────────┘                       └──────────────┘
        ▲                                      │
        │           replay  history            ▼
        └──────────────  ◀─────────────────────┘
```

Pi Durable 把 activity 视为不可变事件流，replay = 重新执行历史事件；强调「让 server 看见而服务端再 persist」。Aleph **不照搬**的原因：

- Pi Durable 是 **server-side universal scheduler**。Aleph 受 R10（薄 harness）约束，不能再造一个调度器。Harness 只在 Loop / EventBus / Sandbox / Recorder / Session / Cancel 六处加层。
- Pi Durable 的「所有 effect 都自动 replay」会与 Aleph 的 **LLM 主权**（R7）冲突——replay 一个外部网络调用结果与重新跑一遍 LLM 推理是两件事。
- Aleph 在本项目本期只承诺**内部 committed record 的去重/状态迁移约束**（R8 + R9），不声称所有 recording 路径均 exactly-once；外部 effect 不承诺 exactly-once，详见 §7。

### 2.2 Cordis

```
┌──────────────────────────────────────────────────┐
│  Cordis Plugin Bus                                 │
│   ┌──────────┐  ┌──────────┐  ┌──────────┐        │
│   │ Plugin A │  │ Plugin B │  │ Plugin C │        │
│   └──────────┘  └──────────┘  └──────────┘        │
│        ▲              ▲              ▲             │
│        └────── ServiceRegistry (ctx / lifecycle) ───┘│
└──────────────────────────────────────────────────┘
```

Cordis 的核心是 **Service 关系图 + 生命周期作用域**。Aleph **部分借鉴**：

| Cordis 概念 | Aleph 对应 | 处理方式 |
| --- | --- | --- |
| `Service` | `Service`（relation，§5.2） | 借鉴；但 Service 不持有数据，只持有 descriptor + resolve 链 |
| `Context.dispose()` | `Runtime.dispose()` | 借鉴；显式 dispose，禁止隐式 drop |
| `Plugin` | `Plugin`（§5.1 capability kind） | 借鉴；但 Plugin 必须先注册 CapabilityDescriptor |
| 隐式 service 自动注入 | 显式 `resolve(name)` | **不照搬**：避免 Cordis 那种「服务图隐式耦合」 |
| 全局 ctx 单例 | Runtime-scoped ctx | **不照搬**：Aleph Runtime 即唯一承载，不存在多 ctx |

### 2.3 deepseek-harness ACP

```
┌──────────────┐    ACP JSON    ┌──────────────┐
│  ACP client  │  ───────────▶  │  ACP Server  │
│              │  ◀───────────  │  (stateful)  │
└──────────────┘    state       └──────────────┘
                          │
                          ▼
                    ┌──────────────────┐
                    │  ACP projection  │
                    │  (派生视图)       │
                    └──────────────────┘
```

deepseek-harness 的 ACP 把 server 视为有状态的协议端，状态变更通过 ACP JSON 投影回客户端。Aleph **不照搬**的原因：

- deepseek-harness ACP 的「server 持有 state」会让 Aleph 失去 LLM 主权（外部协议反控内部状态）。
- Aleph 必须保持 **projection-only** 原则：ACP/MCP JSON 都是 projection，事实源在 Session event store。
- Aleph 的 fence / generation / ReplayPermit 在 ACP wire 上没有等价物；强行映射会让协议变成实现细节的负担。

### 2.4 总结：本期融合的最小公倍数

| 来源 | 借鉴 | 不借鉴 |
| --- | --- | --- |
| Pi Durable | immutable event stream、generation、fence | universal scheduler、自动 replay |
| Cordis | Service 关系、dispose 生命周期、Plugin 形态 | 隐式 ctx 注入、全局单例 |
| deepseek-harness ACP | JSON projection、describe / resolve 形状 | server-side state、wire-side fence |

---

## 3. Aleph Gap Analysis（既现状）

> 全路径以 `capability-phase4` 基线 `a5300221d` 为准。本节只描述事实，不评价好坏。

### 3.1 路径表

| 路径 | 角色 | 既有事实 | 缺口 |
| --- | --- | --- | --- |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/tools/registry.rs` | Tool handler 注册 | 已有 ToolCapabilityDescriptor、revision、ArcSwap snapshot / change feed、generation-safe disposal | 缺口是跨 kind facade / owner / recovery |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/extension/registry/plugin_registry/mod.rs` | Plugin 加载 | 模块装载 + 钩子 | 与 Tool registry 不互通 |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/gateway/execution_engine/session_run_registry.rs` | Gateway Run | Run ID 索引 | 不记录 capability 用量 |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/service.rs` | SessionService event-log facade | actor/store 已有 `emit_event`/`emit_batch`/`get_events`；Session 生命周期 + 关联 Run | 缺口是不把 capability 作为 first-class resource |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/resilience/database/state_database/mod.rs` | 操作型投影 | 多张 SQLite 表 + vec index | Recovery 未与 Session event store 对齐 |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/acp/manager/persistence.rs` | ACP session JSON | best-effort ACP session JSON persistence | 不是事实源，缺 recovery 路径 |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/event/global_bus.rs` | 通知 | 跨 region 推送 | 无 cursor、无 generation |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/capability/mod.rs` | 启动期 CapabilitySlot | 启动期固化能力槽，不是动态 facade | 跨 kind facade / owner / recovery 缺失 |
| `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/runtimes/ensure.rs` | Runtime 装配 | 启动时初始化 | 不感知 capability generation |

### 3.2 关键耦合

```
src/capability/mod.rs  ── partial ──▶  src/tools/registry.rs
                       ╲                ╱
                        ╲              ╱
                         ▶ src/extension/registry/plugin_registry/mod.rs
                         ╲              ╱
                          ╲            ╱
                           ▶ src/acp/manager/persistence.rs
                                ▲
                                │  (反查)
                                │
src/gateway/execution_engine/session_run_registry.rs
                                ▲
                                │
src/session/service.rs ─────────┘
                                ▲
                                │
src/resilience/database/state_database/mod.rs
                                ▲
                                │
src/event/global_bus.rs ────────┘
```

注释：`partial` 表示 `src/capability/mod.rs` 只引用了部分 Tool 信息，没有把 Plugin / Session / Run / Database 串起来。各节点之间暂无文档化的契约约束；事实源歧义仅在 §3.3 列出的三条（Tool 描述 / Run 状态 / Plugin 元数据）。

### 3.3 三处事实源歧义

1. **Tool 描述**：Tool registry 内部已有单调 `revision` 与 generation-safe disposal；但 ACP JSON projection / state database capability 表等不同投影/运行状态副本不保证携带同一 `owner_generation`/revision 契约，因此同一 Tool 描述可能在不同副本出现不一致。
2. **Run 状态**：gateway session_run_registry 的 Run 状态、state database 的 session 表、ACP JSON 的 session 字段，各自维护；跨副本没有统一的 generation 契约。
3. **Plugin 元数据**：plugin_registry 内存视图与 state database 的 plugin row 不一致时没有单一 winner；跨副本同样没有统一 generation 契约。

> 说明：上述描述仅记录「不同副本的 contract 不统一」这一事实，不宣称「三个地方都没有 generation」之类的绝对化结论；tool registry 自身已有 generation-safe disposal，其它副本是否携带需逐一对账。

---

## 4. 候选方案与推荐

| 维度 | A：合并为通用动态 registry（排除） | B：Zahir facade + 类型化域 backend（推荐） | C：各域 registry 保持独立，只抽 contract |
| --- | --- | --- | --- |
| 实施面 | 立即把所有 registry 合成一个通用动态 registry | 新增 `src/capability/{descriptor,facade,ownership}.rs` + 各域类型化 backend | 不动既有 registry；只抽 descriptor / owner / effect / recovery contract |
| 动态性 | 单一 dyn Any / 巨大 enum；运行时注册 | 静态 trait + 类型化 backend；启动期装配 | 完全静态；各域 registry 独立 |
| 事实源 | 单一 registry 即事实 | 三层显式分层：event store（事实） / state database（投影） / ACP JSON（投影） | 各域事实源仍独立且没有 canonical facade |
| 兼容性 | 低，破坏 Phase 3 既有契约 | 中，需要 marker 注释三类投影 | 高，零回归 |
| 风险 | 极高：违反 R8「工具即一切」；与 R7「LLM 主权」冲突（动态注册即确定性路由） | 中，主要风险是 facade 与既有事实源的语义对账 | 低，但缺 canonical facade；下游需自行对齐 |
| 对 R7（LLM 主权） | 风险极高 | 接受：facade 仅 describe / resolve | 接受：facade 不存在 |
| 对 R10（薄 harness） | 守住但需要新增 dispatcher | 守住（新增 3 个文件，未触 harness） | 守住 |

**推荐方案 B**。理由：

- 不动既有事实源，避免 Phase 3 回归；Tool / MCP / builtin / markdown-skill 路径完全不回归（§11.4）。
- 通过 marker（`projection_only`、`recovery_source`、`notification_only`）揭示现有事实源分层，并约束其它 projection 不得冒充 recovery source——这与 Aleph 的 **fail-closed 默认**（R10）一致。
- 允许 Phase 5+ 在不重写 Phase 4 的前提下合并存储。

### 4.1 方案 B 的不变量

1. 事实源唯一化：既有 recovery source 已明确为 SessionEventStore（本期不重写 `src/session/store.rs` 既有事实契约）；本期 facade marker 揭示并约束其它 projection 不得冒充 recovery source（§7）。
2. 投影分层：`StateDatabase` 与 ACP JSON 都是可丢弃的投影。
3. 总线降级：`GlobalBus` / Gateway bus 仅通知，不能用来回答「现在是什么状态」。
4. 不新增 harness 文件（§11.3）。
5. 不引入 dyn Any / 巨大 enum payload（§5.5）。

---

## 5. Capability 模型

### 5.1 CapabilityKind（9 种）

| Kind | 含义 | 当前对应 | descriptor 必填 |
| --- | --- | --- | --- |
| `Tool` | 可调用的工具 | `tools/registry.rs` 中 `ToolHandlerRegistry` | schema、handler |
| `Skill` | 提示词技能 | markdown-skill（Phase 3 builtin） | name、prompt |
| `Agent` | 可调用子代理 | `agents/` 模块 | system_prompt、toolset |
| `Task` | 一次性任务槽位 | session_run_registry | kind、budget |
| `Resource` | 资源（文件、URL、向量索引） | plugin_registry 中的 resource 类 | uri、access |
| `EventSource` | 可订阅事件源 | GlobalBus 注册的 source | schema |
| `Subscription` | 订阅句柄 | 由 EventSource 派生 | cursor、generation |
| `Plugin` | 复合能力包 | extension/registry/plugin_registry | composition、entry |
| `Hook` | 生命周期钩子 | plugin_registry 的 hook 类 | phase、priority |

> 现有 `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/extension/plugin_trust.rs` 已有对 `CapabilityKind` 的文档/引用；本期落地必须统一该名称或调整引用，不能留下第二定义。

### 5.2 Service、ACP、MCP 的角色

| 名称 | 角色 | 备注 |
| --- | --- | --- |
| `Service` | **relation**（关系） | 描述 Capability 之间的依赖；持有 `requires / provides / conflicts`，不持有数据 |
| `ACP` | **outbound = projection；inbound = transport** | outbound 把 CapabilityDescriptor / capability 变更序列化为 ACP JSON；inbound 命令经 ACP transport 与 session manager 进入 Session / Run mutation line，**不在此层自建 capability registry** |
| `MCP` | **outbound = projection** | 把 Tool 投影为 MCP tool schema；只读 projection；本期不承接 inbound |

约束：`Service` 不允许直接实现 invocation；ACP / MCP 不允许直接持有可变状态；ACP inbound 不允许绕过 Session / Run mutation line 自建 capability 注册表；三者都只是描述，不允许承担「事实源」角色。

### 5.3 CapabilityDescriptor（核心类型）

```text
struct CapabilityDescriptor {
    id: CapabilityId,            // 稳定命名空间 + 本地名，e.g. aleph/tools/<tool-name>
    kind: CapabilityKind,        // 9 选 1
    schema: SchemaRef,           // JSON Schema（来自 schemars）
    schema_version: u32,         // schema 的版本号；adapter canonical mapping（与 ToolCapabilityDescriptor.schema_version 一一对应；由 adapter 保证）
    schema_fingerprint: Hash,    // schema 的内容指纹（hash）
    revision: CapabilityRevision,// 本 capability 的修订号；与 owner_generation 解耦；Tool backend 直接透传现有 ToolCapabilityDescriptor.revision；不另立事实
    owner_generation: OwnerGeneration, // 当前 owner 的 generation；与 revision 解耦
    lifetime: LifetimeScope,     // §6.2
    visibility: VisibilityScope, // §6.2
    owner: OwnerRef,             // Runtime / Session / Run / Task
    services: Vec<Service>,      // relation（图）
    metadata: ProjectionMetadata, // projection metadata; 不依赖 to_metadata_form
}
```

### 5.4 Zahir facade（four-method contract）

```text
trait Zahir {
    fn describe(&self, scope: Scope) -> CapabilitySnapshot;
    fn resolve(&self, reference: Reference) -> BackendLease;
    fn subscribe(&self, scope: Scope, cursor: Cursor) -> CapabilityChangeStream;
    fn project(&self, target: TransportTarget, scope: Scope) -> Projection;
}
```

- `describe(scope)` 返回不可变的 `CapabilitySnapshot`；snapshot 包含该 scope 下所有可见 capability 在当前 `owner_generation` 与 `revision` 下的描述；调用方持有 snapshot 期间 snapshot 内容不会变化。
- `resolve(reference)` 返回类型化 backend 的租约（`BackendLease`），并附带 generation check；lease 在 owner 发生 `revoke` / generation bump 时自动失效。
- `subscribe(scope, cursor)` 返回有序 `CapabilityChange` 流；stream 按 committed cursor 单调推进；generation bump 通过 `Invalidated` 事件通知订阅者。
- `project(target, scope)` 是单向只读投影；输出格式由 target 决定；project contract 本身不固定序列化格式，写 transport 不允许回到事实源。

### 5.5 不允许的形状

| 不允许 | 原因 |
| --- | --- |
| `Box<dyn Any>` 持有 handler payload | 与 R8「工具即一切」冲突——任何字段都应是显式 schema |
| 巨大 enum 把 9 类 payload 塞进 `CapabilityDescriptor` | 阻塞 R3「核心轻量化」；每个 kind 走 `SchemaRef` 才是正路 |
| 把 `dyn Trait` 作为 descriptor 字段 | 与 R7「LLM 主权」冲突——描述动态性会变成确定性决策 |

### 5.6 ToolHandlerRegistry 第一个 backend

`ToolHandlerRegistry`（`src/tools/registry.rs`）是 Zahir facade 在本期的 **唯一完整 backend**（Tool kind）。新增的是适配层——保留所有现有 handler 注册路径，不替换任何既有契约：

```
Zahir-facade ──▶ ToolHandlerRegistry (本期 backend #1, Tool kind, 完整 backend)
              ╳ Skill / Agent / Task / Resource / EventSource / Subscription / Plugin / Hook
                (本期不挂载 backend；保留 onboarding contract / deferred adapter)
```

其他八类 capability 在本期只定义 `CapabilityBackend` trait（`lookup / enumerate / generation`）与 onboarding contract；不实现、不挂载、不在 Gate 报告里宣称「已挂载 backend」。任何「Skill backend #2 已挂载」「Agent backend #3 已挂载」之类的话均为错信。

---

## 6. Ownership Tree

### 6.1 五层所有权

```
┌──────────────────────────────────────────────────────────────────┐
│  Runtime                                                         │
│   └─ OwnerGeneration(Runtime) ─ 由 Runtime owner 签发；          │
│      │                          随 owner 创建 / dispose 变更      │
│      └─ Session                                                  │
│         └─ OwnerGeneration(Session) ─ 由 Session owner 签发；    │
│         │                       随 Session 创建 / 终止变更         │
│         └─ Run                                                   │
│            └─ OwnerGeneration(Run) ─ 由 Run owner 签发；         │
│            │                       随 Run 创建 / cancel 变更       │
│            └─ Task                                               │
│               └─ EffectClaim                                     │
│                  FencingToken ─ owner 层签发的不透明值；         │
│                                每次 claim 新增；生成细节          │
│                                不在本期 spec 中规定              │
└──────────────────────────────────────────────────────────────────┘
```

`OwnerGeneration` 是 registry 与 owner 共享的统一 generation 概念，由对应 owner 签发并随 owner 生命周期单调变更；与 registry 内部 / owner 外部归口组合在一起，不再各持一份。`EffectClaim` 持有独立的 `FencingToken`（与 `OwnerGeneration` 解耦），用于 receiver 端幂等拒绝与对账。`revoke` 一层会让所有下层仍处于 active 状态的 `EffectClaim` 标记 `Unknown`；`dispose` 是不可逆的终止（§6.3）。

### 6.2 VisibilityScope / LifetimeScope 分离

| 维度 | 取值 | 语义 |
| --- | --- | --- |
| `VisibilityScope` | `principal/agent` + `workspace/channel` + `session` + `allowed kinds/ids` + `permission/approval context` | 谁在什么 ACL 上下文里「看得见」这个 capability descriptor；任一字段均可缺省，缺省即「不限」 |
| `LifetimeScope` | `Runtime / Session / Run / Task / External` | 谁「承担」这个 capability 的生命周期；`External` 表示不由 Aleph 进程管理（如远端 MCP server），仅持有引用与可见性 |

**分离的原因**：可见性是 ACL（访问控制），生命周期是 RAII（资源管理）。合在一起会导致「plugin 想给 Run 暴露自己，但 lifetime 由 Runtime/Session/Run/Task/External 明确承担」这种语义错乱。两者正交后，可见性可随调用方身份动态收敛，生命周期则独立于可见性随 owner 终结；`External` 与本地 `Runtime/Session/Run/Task` 在 lifetime 维度上对齐 RAII 模型而不假装在 Aleph 进程内被回收。

### 6.3 generation / revoke / dispose

| 操作 | 谁能调用 | 效果 | 是否可逆 |
| --- | --- | --- | --- |
| `generation.bump` | 该层 owner | 子层 `OwnerGeneration` 失效；subscriber 收到 invalidated；既有 snapshot 仍可按 committed cursor 读取（§8） | 否（`OwnerGeneration` 单调） |
| `revoke(id)` | 该层 owner 或更高层 | 阻止后续 `resolve` / `claim`；所有挂在该 id 上的 active `EffectClaim` 置 `Unknown` | 否 |
| `dispose(scope)` | 该层 owner | 释放该层所有 `OwnerGeneration` + 所有下层；阻止下层新的 `resolve` / `claim` | 否 |

约束：`dispose` 不与外部 effect 状态机耦合——已 `Invoking` 的 effect 其 outcome 由当前 `FencingToken` 与 receiver 端幂等保证共同决定；外部副作用状态不可观测则记 `Unknown`。Aleph **不描述、也不承诺**对外部 effect 的 rollback 路径（§10.5）。`dispose` 与 `revoke` 的区别仅在「是否彻底释放该层资源」，对 active `EffectClaim` 的语义上一致。

---

## 7. Session Event Store 作为恢复源

### 7.1 三层定位

```
┌──────────────────────────────────────────────┐
│  Session Event Store  ← 唯一恢复源            │
│  (append-only + soft-retire（Retire::From/Through），
│   committed, cursor-indexed)                   │
└────────────┬─────────────────────────────────┘
             │  projection
             ▼
┌──────────────────────────────────────────────┐
│  StateDatabase  (operational projection)      │
│  + ACP JSON  (transport projection)           │
│  + MCP JSON  (transport projection)           │
└──────────────────────────────────────────────┘
             │
             ▼  notification only
┌──────────────────────────────────────────────┐
│  GlobalBus / Gateway bus  (通知)              │
└──────────────────────────────────────────────┘
```

### 7.2 effect sandwich

| 状态 | 内部记录 | 外部 effect |
| --- | --- | --- |
| `Prepared` | 写 event `prepared` | **未发生**（默认 VerifyOnly） |
| `Claimed` | 写 event `claimed`，附 `fencing_token` | 仍未发生 |
| `Invoking` | 写 event `invoking` | **首次真正发生**（执行外部 side-effect） |
| `Succeeded` | 写 event `succeeded` | 已发生 |
| `Failed` | 写 event `failed` | 外部副作用可能已发生，状态按证据结算；不承诺 rollback |
| `Unknown` | 写 event `unknown`（来自 `revoke` 或 generation bump） | 不可断言 |

### 7.3 dedupe / order / fence

- **内部 committed record**：内部 committed record 的顺序、去重和合法状态迁移由 event-store contract 约束；不把它扩大为外部 effect exactly-once。具体地：`Session event store` 的内部 committed record 由 `(session_id, seq)` 单调主键、`append_batch` 与 `retire`（`Retire::From` / `Retire::Through`）的幂等语义去重；本期不在 event store 上声称有名为 `idempotency key` 的字段，也不把它当作外部 effect 的幂等键。
- **外部 effect** **不承诺** exactly-once：`Invoking` 之后的副作用由 receiver 端幂等保证；Aleph 只保证「在 prepared→claimed→invoking 这条链上不重复 commit」。
- **`request_id`** 与 **`fencing_token`** 必须同时存在；`fencing_token` 是 owner 层签发的不透明值，生成细节归 §7.5 的 `ReplayPermit`，本期不在 spec 中规定位运算格式或 wire 表达。

### 7.4 budget

`budget` 是 scope 上的 owned 资源（不是 capability），挂在 owner 层。超过 budget 的 effect 进入 `Failed(budget_exceeded)`。`budget` 必须可序列化进 event（immutable）。

### 7.5 ReplayPermit / ReplayPreparer 与 EffectClaimClosure

下表区分既有基础与本期适配/对账责任：

| 类型 | 来源 | 角色 |
| --- | --- | --- |
| `ReplayPermit` | 既有（`/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/replay.rs`） | 一次性「允许 replay」的票，由 owner 签名；含 scope + fence |
| `ReplayPreparer` | 既有（`/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/replay.rs`） | 把历史 event 流 prepare 成可重放序列；只读 event store |
| `EffectClaimReconciliation` | **本期适配/对账** | 本 spec 定义的「replay / crash 还原后 effect-claim 对账结果」；与 `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/store.rs` 既有 `ReplayClaimResult` **不等价**——后者是既有 replay claim 尝试结果（`HeadChanged` / `GenerationChanged` / `NotDangling` / `LeaseHeld` / `BudgetExhausted` / `Claimed` 等），其语义是「这次 claim 是否被采纳」；`EffectClaimReconciliation` 的语义是「按闭包对账后 effect-claim 收敛到哪个终态」，两者职责不同。`EffectClaimClosure` 校验（见下）的产物即为 `EffectClaimReconciliation`，两者绑定使用，不相互替代。 |

> 术语区分：本节所谓 `EffectClaimClosure` 指本 spec 定义的「合法状态迁移闭包」，与 `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/marker_balance.rs` 既有的「`RunStarted` / `RunFinished` run 范围闭合」**不是同一个东西**——后者是 run-anchor 配对的尾部 close，前者是 effect-claim 的状态迁移闭包。本 spec 不把 `marker_balance` 的 `RunStarted`/`RunFinished` 等式当作 effect-claim 不变量来源；任何「三行计数等式」式的简化都是不充分不变量。

**EffectClaimClosure 规则**（合法状态迁移闭包）：

每个 intent（由 `request_id` + 初始 owner 标识）满足下列不变量；不变量失败一律 fail-closed 为 `Unknown`，由 `EffectClaimReconciliation` 显式标注。

- **唯一活跃 claim**：同一个 intent 至多存在一个 active（即非终态）的 `EffectClaim`；跨 owner 重复 claim 必须被 `EffectClaimReconciliation` 拒绝。
- **合法前驱**：每个状态的进入必须由闭包内允许的前驱状态触发。闭包内的迁移集合：
  - `Prepared → Claimed`
  - `Prepared → Unknown`（Prepare 路径失败 / revoke / dispose 在 `Prepared` 阶段发生）
  - `Claimed → Invoking`
  - `Claimed → Unknown`（revoke / dispose 在 `Claimed` 阶段发生 / `FencingToken` 落后）
  - `Invoking → Succeeded`
  - `Invoking → Failed`
  - `Invoking → Unknown`（dispose 在 `Invoking` 阶段发生 / external effect 不可观测）
  - 任何其他迁移（包括直接进入 `Invoking` 而无 `Claimed`、从 `Succeeded`/`Failed` 再迁移等）一律非法。
- **终态唯一**：每个 `EffectClaim` 终结时必须处于 `Succeeded` / `Failed` / `Unknown` 三态之一，且不可再次写入。
- **失败闭合**：reducer 在 replay / crash 还原后若发现任一条记录缺少合法前驱、终态冲突、或重复 active claim，则记为 `Unknown`，并不假装 `Succeeded`。

**校验路径**：

- `reducer` 测试：覆盖闭包内全部合法迁移 + 至少一例每条非法迁移的反例。
- `property` 测试：随机事件流 + reducer 必须给出单值状态；不会出现「同一 intent 多终态」「无前驱的 active claim」「active claim 跨 owner 重复」三种状态。
- `crash` 测试：在合法序列任意点注入 crash；恢复后状态必须收敛到闭包允许的某条终态或 `Unknown`，且 reducer 输出与 audit log 一致。

本期不规定具体闭包以外的“计数等式”。任何「三行计数等式」式简化都是不充分不变量，不得代替上述闭包校验。

### 7.6 approval / hook durable memo

`approval` 与 `hook` 调用必须留下 **durable memo**：即使 hook 实现重启，memo 仍可被 audit。

- `approval memo` = 决策 + request_id + fence + 决策时间 + approver identity。
- `hook memo` = 触发 phase + capability + 跳过/通过 + 触发时间。

两条 memo 都进入 `Session event store`，可被 `describe` 时附带审计字段返回。

---

## 8. 订阅：immutable snapshot + committed cursor

### 8.1 模型

```
Subscriber ──subscribe(kind, generation=0)──▶ SubscriptionHandle
                                                       │
                                                       ▼
                                              committed_cursor = 0
                                                       │
                                                       ▼
Subscriber  ◀── snapshot_at(gen) + deltas(cursor→HEAD) ──▶ event store
```

- **immutable snapshot**：订阅开始时返回 capability 在该 generation 的完整 descriptor 列表。
- **committed cursor**：每次 commit 后单调推进；订阅者按 cursor 读取后续 delta。
- **generation bump**：订阅者收到 `Invalidated` 通知后，**保留原 committed cursor**，以 snapshot + delta 从该 cursor 重同步；重同步期间按 `OwnerGeneration` / `CapabilityRevision` 对事件去重，丢弃跨 owner / 跨 generation 的错位事件。本 spec **不得写**「从 0 重新订阅」之类的表述——从 0 重订意味着放弃 cursor 与去重保障，会把失效窗口放大到全量。

### 8.2 ACP / MCP 必须 projection-only

- ACP JSON：只能在 snapshot 之后产出 projection；不能写回事件流。
- MCP JSON：同上，且必须**对当前 9 类 capability 中的 Tool / Resource 子集**做窄投影。
- 任何 ACP / MCP 字段如果与 `Session event store` 不一致，以 event store 为准（fail-closed）。

---

## 9. Migration / Onboarding 顺序

| 步 | 动作 | 影响面 | 兼容性 |
| --- | --- | --- | --- |
| 1 | 新增 `src/capability/descriptor.rs` | +1 文件 | 完全兼容；纯增量 |
| 2 | 新增 `src/capability/facade.rs` + backend trait | +1 文件 | 完全兼容；backend 暂不挂载 |
| 3 | 把 `ToolHandlerRegistry` 适配为 backend #1 | `src/tools/registry.rs` 加适配层 | 完全兼容；既有 handler 注册路径不回归 |
| 4 | 新增 `src/capability/ownership.rs` | +1 文件 | 不影响既有事实源 |
| 5 | 适配现有 `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/store.rs` + `SessionService::emit_batch` / actor committed batch（本期不新增事件存储子模块/目录；既有 store / replay / resume 测试不回归） | 0 新文件；仅在既有 store.rs + service.rs 上加适配注释 / marker | 默认 shadow write；既有 store/replay/resume 测试不回归 |
| 6 | `StateDatabase` 标 `projection_only`，`ACP persistence` 标 `projection_only` | 注释 + 类型 marker | 完全兼容 |
| 7 | `GlobalBus` / `Gateway bus` 标 `notification_only` | 注释 + 类型 marker | 完全兼容 |
| 8 | 对接现有 `/Volumes/TBU4/Workspace/Aleph-capability-phase4/src/session/replay.rs` 的 `ReplayPermit` / `ReplayPreparer`、`src/session/store.rs` 的 `ReplayClaimResult`、`src/gateway/resume_coordinator.rs`；本期不新增事件存储子模块/目录，也不开启自动 replay | 0 新文件；现有三处既有 API 仅加 marker / 调用点适配 | 完全兼容；不开启自动 replay |
| 9 | gate A → B → C → D 顺序执行 | 见 §11.5 | 每 gate 之间不破坏既有路径 |

约束：每一步必须可独立 revert；如果某一步破坏既有契约，回滚并重写。

---

## 10. Gate、Crash/Error Matrix、Acceptance

### 10.1 Gate A — Zahir + Descriptor

| 通过条件 | 证据 |
| --- | --- |
| 9 种 `CapabilityKind` 编译通过 | `cargo check` |
| `describe / resolve / subscribe / project` 四个方法签名稳定 | API review |
| `ToolHandlerRegistry` 作为 backend #1 挂载成功 | 单测：Tool name → descriptor round-trip |
| 既有 Phase 3 路径全部 smoke 通过 | 既有 test suite 不回归 |

### 10.2 Gate B — Ownership Tree

| 通过条件 | 证据 |
| --- | --- |
| `Runtime / Session / Run / Task / EffectClaim` 五层生成通过 | 单测：每一层 generation bump 后子层失效 |
| `VisibilityScope / LifetimeScope` 分离通过 | 单测：同 id 不同 scope 共存 |
| `revoke / dispose` 不可逆通过 | property test |
| 既有 Phase 3 路径不回归 | 既有 test suite |

### 10.3 Gate C — Event Store + Sandwich

| 通过条件 | 证据 |
| --- | --- |
| 现有 store / replay / resume fixtures 与 projection marker 的可重复校验；本期新增 facade / closure 差异有明确解释 | 使用固定 crash/replay fixtures；source、projection 与本期新增的 facade/`EffectClaimClosure` 差异要在 commit message / 报告里逐项说明 |
| sandwich 状态转移不破坏 | 单测：Prepared → Claimed → Succeeded |
| `EffectClaimClosure` 校验通过 | 单测：故意制造不满足闭包的事件，触发 `Unknown` |
| `fencing_token` 单调 | property test 验证 fence 不倒退 |
| `approval / hook` durable memo 入库 | 单测：hook 重启后 memo 仍在 |
| 既有 Phase 3 路径不回归 | 既有 test suite |

### 10.4 Gate D — Projection-Only ACP / MCP

| 通过条件 | 证据 |
| --- | --- |
| `ACP / MCP` JSON 与 event store 不一致时 fail-closed | 单测：故意制造不一致 |
| `to_metadata_form` 只允许 wrapper | 静态检查（lint / 类型 marker） |
| 既有 ACP wire 兼容 | 既有 ACP client smoke |

### 10.5 Crash / Error Matrix

| 触发 | 期望 | 触发位置 |
| --- | --- | --- |
| `dispose(scope)` 在 `Prepared → Claimed` 之间 | `Prepared` 阶段撤销后阻止新 claim 并提交 `Invalidated`/`Unknown`；active external effect 不回滚 | `effect_sandwich` |
| `dispose(scope)` 在 `Claimed → Invoking` 之间 | `Claimed` 阶段撤销后阻止新 claim 并提交 `Invalidated`/`Unknown`；active external effect 不回滚 | `effect_sandwich` |
| `dispose(scope)` 在 `Invoking → Succeeded`（race） | `Invoking` race 由 fence/outcome 结算；outcome 未知则记 `Unknown`；已 commit 终态不被覆盖 | `effect_sandwich` |
| generation bump | 所有下层 `EffectClaim` → `Unknown` | `ownership` |
| event store 写失败 | 调用方 `Unknown`；不假装 `Succeeded` | `SessionEventStore` |
| StateDatabase 写失败 | 不影响 event store；下次 recovery 时重投影 | `state_database` |
| ACP JSON 写失败 | 同上 | `acp/persistence` |
| hook 重启 | 重放 memo；不重新跑 hook 决策 | `SessionEventStore` |
| fencing_token 落后 | receiver 拒绝；Aleph 标记 `Unknown(Stale)` | `effect_sandwich` |

### 10.6 Acceptance Criteria

| 项 | 度量 |
| --- | --- |
| Phase 3 既有 test suite | 全绿，无 regression |
| 新增 capability test | 每个已接入 contract 至少有正/负/恢复测试 |
| event store / projection 校验 | 使用固定 crash/replay fixtures；对 source 与 projection 做可重复、无未解释差异的校验 |
| fence 与 effect sandwich 不变量 | property test 与 crash fixtures 覆盖关键不变量 |
| doc coverage | 本文档 + `FEATURE_LOCATOR.md` 更新 + reference docs 更新 |

---

## 11. 兼容边界

### 11.1 `to_metadata_form` 收窄为旧 Tool 兼容 wrapper

`to_metadata_form` 在本期收窄为 **旧 Tool compatibility wrapper**，仅服务于既有 Tool path 的兼容性输出。

- `CapabilityDescriptor` facade **不依赖** `to_metadata_form`；facade 只使用 ProjectionMetadata；该 projection metadata 由 facade/adapter 按 canonical descriptor 生成，不依赖 to_metadata_form。
- wrapper 只允许：复制字段到 `HashMap<String, String>`、序列化 schema、序列化 version / generation。
- wrapper **不允许**：任何 side-effect、RPC 调用、event store mutation、capability 字段写入。
- 任何越界在静态检查阶段被 lint 拒绝。

### 11.2 `AlephToolServer` 不成为第二事实源

`AlephToolServer`（既有的 tool gateway 入口）在本期仅作为 **transport adapter**：把 tool call 转给 `ToolHandlerRegistry`（backend #1）。不允许：

- 维护自己的 capability 列表；
- 持有 mutation 状态；
- 接受 write 类请求并直接持久化。

任何违反都将被 Gate A 拒绝。

### 11.3 `src/harness/` 不扩张

按 R10 棘轮：本期不在 `src/harness/` 下新增任何业务文件。所有 harness 相关变更（如果有）必须先获得破例批准。Gate A-D 由 review 检查 diff 不新增 harness 业务文件，而非以文件计数守门。

### 11.4 既有 Phase 3 不回归

Phase 3 已经定下的以下路径不回归：

| 路径 | 现状 | 不允许的改动 |
| --- | --- | --- |
| `src/tools/registry.rs` | Tool handler 注册 | 修改既有 handler 注册契约 |
| MCP builtin | 已接入 | 改 MCP wire |
| markdown-skill | builtin 已接入 | 改 builtin 注册路径 |

验证：Gate A-D 都跑 Phase 3 既有 test suite。

### 11.5 文件分配按 Gate

新增文件按 Gate 分配，每个文件必须有独立责任，落地前走 plan review。

- 每个 Gate 的新增文件清单在该 Gate 的 plan 中明确；不在本 spec 中预声明硬性文件数。
- `src/harness/` 不新增业务文件（§11.3）。
- 其它位置仅允许 marker 注释 / 类型 marker，除非该 Gate plan 显式列入。

---

## 12. Non-Goals / Deferred

| 不做 | 原因 |
| --- | --- |
| 合并所有存储到一个 store | 与 R8「工具即一切」冲突；保留事实源分层 |
| 建 universal durable scheduler | 违反 R10「薄 harness」；亦违反 R7「LLM 主权」 |
| 做 external-effect ledger（外部 side-effect 的总账） | Aleph 不假装能观测 receiver 不可见的 effect |
| 自动 Safe Replay | 默认 VerifyOnly；replay 必须显式 `ReplayPermit` |
| 完整 ACP server gate | 本期只做 projection-only；完整 gate 是 Phase 5+ |
| 把 `Service` 升级为可调用对象 | Service 是 relation，不能持数据 |
| 让 `ACP / MCP` 写回事件流 | projection-only 是硬约束 |
| 让 `GlobalBus` 替代事件流 | bus 是通知，不是事实 |

---

## 13. 后续任务（不在本次提交内）

本 spec 未声称实现完成；本 spec commit 仅交付本设计规范。`FEATURE_LOCATOR.md` 与 `docs/reference/*` 文档补全不在本 spec commit 范围内，由后续任务承担。下列更新是后续实施阶段的工作，不在本 spec 范围内：

1. **`FEATURE_LOCATOR.md`**：增加「Capability Phase 4」节点，指向本 spec + `src/capability/*` 三文件。
2. **reference docs**：
   - `docs/reference/ARCHITECTURE.md`：补 Zahir facade、Session event store、ownership tree 三节。
   - `docs/reference/CODE_ORGANIZATION.md`：补 `src/capability/` 目录、描述现有 `src/session/store.rs` 与 `src/session/replay.rs`。
   - `docs/reference/DESIGN_PATTERNS.md`：补「projection-only」「fail-closed marker」「effect sandwich」三条模式。
   - `docs/reference/DOMAIN_MODELING.md`：补 `CapabilityKind` 9 种枚举与 ownership 关系图。
   - `docs/reference/SANDBOX.md`：在 effect sandwich 章节引用 fence / replay。
   - `docs/reference/REDLINES.md`：在 R8「工具即一切」条目下补 Zahir facade 的边界注解。
3. **`AGENTS.md` / `CLAUDE.md`**：在 Tier 1 增加「Capability 9 类」与「fact source = event store」指针；保持与本 spec 一致。
4. **实施拆分**：先出一份"已存在 vs 待新增"的 delta 清单，明确不重写 Session event store、replay、resume；再据此拆分 Gate A / B / C / D 的实施 plan，并分别走 Brainstorm → Plan → Implement → Review。

阶段 1-3 是文档补全，阶段 4 是实施任务。所有阶段 1-4 的 PR 标题都应回链本 spec 的路径与 commit hash。

---

## 附：本 spec 与既事实源对应表

| Spec 概念 | 既事实源 | 关系 |
| --- | --- | --- |
| `CapabilityDescriptor` | 9 类既有 registry | 抽象层；不替代 |
| `ToolHandlerRegistry` backend #1 | `src/tools/registry.rs` | 适配 |
| `Zahir facade` | `src/capability/mod.rs`（保留 `CapabilitySlot`） | 新增收敛层 |
| `Session event store` | `src/session/service.rs` + `SessionService`/actor committed batch | 已存在，本期通过 SessionService/actor committed batch 适配/标记，不替换 service |
| `ownership tree` | `src/gateway/execution_engine/session_run_registry.rs` | 抽象层 |
| `replay / permit / claim` | `src/session/replay.rs` + `src/session/store.rs` | 已存在，本期适配/标记，不新增模块；不接 `StateDatabase` |
| `approval / hook durable memo` | `src/approval/` + `src/extension/hooks/` | 新增；不接 `GlobalBus` |

---

*结束。本 spec 不可作为实现提交；实现必须另行走 plan → 实施 task → review 流程。*