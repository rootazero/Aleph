# Aleph 模型路由与 MoA 整体删除设计与迁移计划

**Aleph Model Routing and MoA Removal Design & Migration Plan**
**日期 / Date:** 2026-10-19
**状态 / Status:** 已获用户批准的设计草案，已通过 builder check；尚未进入实现阶段
**范围 / Scope:** Rust Core、Orchestrator、Harness、Builtin Tools、Gateway、Webchat Panel、TUI、配置、观测与测试

## 1. 意图 / Intent

Aleph 当前同时承担了两种职责：

1. 为 LLM 提供模型和 subagent 执行能力；
2. 替主 Agent 判断任务性质，并强制把不同任务分配给不同模型或多个 advisor。

本次瘦身删除第二种职责。主 Agent 负责拆解任务、选择模型、决定 subagent 的数量与生命周期；Aleph runtime 负责把显式请求可靠地执行完。

The target is not to remove model support. The target is to remove runtime-owned cognitive routing: task classification, role-to-model defaults, learned model recommendations, and multi-model advisory fan-out. The runtime retains explicit model binding, provider resolution, resource limits, retries, failover, cancellation, recovery, and child lifecycle management.

目标运行路径：

```text
主 Agent LLM 决定任务拆分与模型
    -> explicit subagent request(provider/model, task, limits)
    -> runtime validates configured provider/model
    -> one Harness execution path
    -> provider retry/failover only when execution fails
    -> result / cancellation / cleanup
```

## 2. 决策 / Decisions

### 2.1 删除 MoA 整套产品能力

Aleph 中当前名为 MoA（Mixture of Agents）的功能包括：多个 advisor 并行调用、aggregator 合并、sticky/one-shot session activation、`/moa` 命令、`moa` builtin tool、配置 preset、Panel 管理页面、RPC、trace 与 usage 聚合。

这些功能全部删除。它们不是普通的“并行拆任务”：它们会在一次认知任务中隐式制造多个模型调用，并由 runtime 决定如何合并结果。这正是本次要移除的架构责任。

`subagent` 的普通并行、串行、后台任务、团队协作和 subagent tree 继续保留，因为这些是主 Agent 明确发起的任务执行能力。`src/agents/subagent_tool/loop_tool.rs` 中以 `proposer_models` / `synthesize` 为中心的多模型提案聚合路径也属于 MoA 语义，随本次删除；普通 `batch_tasks` 等由主 Agent 明确拆分的能力保留。

### 2.2 删除策略性模型路由

删除以下隐式或静态的认知路由：

- `AgentDef.model_hint` 作为角色默认模型；
- `AgentDef.provider_hint` 作为角色默认 provider；
- routing experience 的 embedding 召回、经验注入和结果记录；
- 任何根据任务、角色、成本、负载或历史经验替主 Agent 选择模型的主动路由；
- 为上述机制提供的 Panel 配置、运行时状态、数据库表和测试。

保留以下显式或可靠性基础设施：

- `list_models`：让主 Agent 发现用户已配置的模型；
- `select_model`：作为 LLM-facing 的显式会话模型选择工具，但移除所有 `moa` 特殊分支；
- subagent request 上的显式 model/provider override；
- provider/model 解析、模型目录、能力与 context window 元数据；
- `ModelOverrideProvider`，但只负责执行显式 pin；
- `FailoverProvider` 的错误分类、retry、cooldown、circuit breaker 和备用 provider/model；
- subagent 的并发、递归深度、取消、child-first disposal、resume 和失败结果；
- 单一 `Orchestrator -> Harness` 执行路径。

失败后的 failover 不属于认知路由。正常路径不应因成本、任务类型或历史经验自动换模型；请求失败时的备用链仍然属于运行时稳定性边界。显式模型选择的既有 failover 语义保持不变：显式 `(provider, model)` 约束 primary；provider 自身配置的备用模型/provider 仍可在可恢复执行失败时接管；不新增跨 provider 的认知重选。每次 run 的 trace、cost attribution 和 resume marker 记录实际服务模型，同时保留原始显式 directive 作为请求意图。resume 继续遵守现有 `RunStarted` 冻结与重放契约：本次删除不得把一次失败后的 failover 变成新的 session pin，也不得在恢复时重新引入任务级模型选择；实现前先固化现有行为测试，改动后逐项对比。

### 2.3 保留一个简单的模型绑定汇合点

`src/orchestrator/harness_bridge/runner_impl.rs::effective_model_directive` 不整体删除，而是缩减为单一显式绑定点：

```text
per-turn explicit model
    -> session select_model pin
    -> current provider default
```

实现阶段需要移除其中的 `AgentDef.model_hint`、`provider_hint`、MoA activation、routing recall 和 aggregator identity 分支。不得保留一个“名字变了但仍然暗中替模型做选择”的新路由器。

## 3. 当前实现面 / Current Surface Inventory

以下清单来自当前工作树的源码搜索。实现前必须由归档 manifest 重新生成完整文件列表，不以本清单替代机器校验。

### 3.1 MoA Core 与执行路径

- `src/providers/moa/activation.rs`
- `src/providers/moa/advisor_health.rs`
- `src/providers/moa/advisory_view.rs`
- `src/providers/moa/config_handle.rs`
- `src/providers/moa/fan_out.rs`
- `src/providers/moa/mod.rs`
- `src/providers/moa/preset_store.rs`
- `src/providers/moa/prompts.rs`
- `src/providers/moa/provider.rs`
- `src/providers/session_moa_handle.rs`
- `src/config/types/moa.rs`
- `src/builtin_tools/moa_manage.rs`
- `src/gateway/handlers/moa.rs`
- `interfaces/webchat/src/api/moa.rs`
- `interfaces/webchat/src/platform/wide/views/settings/moa/mod.rs`
- `interfaces/webchat/src/platform/wide/views/settings/moa/options.rs`
- `interfaces/webchat/src/platform/wide/views/settings/moa/preset_editor.rs`

还需要从共享文件中删除 MoA 分支和测试，包括但不限于：

- `src/providers/mod.rs`
- `src/config/structs.rs`
- `src/config/types/mod.rs`
- `src/config/types/tools.rs`
- `src/config/patcher.rs`
- `src/builtin_tools/mod.rs`
- `src/executor/builtin_registry/definitions.rs`
- `src/executor/builtin_registry/groups.rs`
- `src/executor/builtin_registry/builder/core_tools.rs`
- `src/executor/builtin_registry/builder/constructor/mod.rs`
- `src/executor/builtin_registry/registry/inherent.rs`
- `src/executor/builtin_registry/registry/struct_def.rs`
- `src/executor/builtin_registry/registry/tool_registry_impl.rs`
- `src/gateway/execution_engine/execute.rs`
- `src/gateway/execution_engine/slash_command.rs`
- `src/orchestrator/harness_bridge/runner_impl.rs`
- `src/harness/trace.rs`
- `src/gateway/execution_engine/unattended_redacting_sink.rs`
- `src/resilience/database/traces.rs`
- `interfaces/webchat/src/api/mod.rs`、`app.rs`、settings route、locales 和相关 Panel 注册点。

### 3.2 模型策略与学习路由

模型策略的主要生产路径位于：

- `src/agents/types.rs`：`model_hint`、`provider_hint` 及 builder/serde；
- `src/agents/loader.rs`：agent frontmatter 读取；
- `src/agents/runtime.rs`：provider override registry、routing store 和 spawn route；
- `src/agents/subagent_spawner/mod.rs`：显式 model 与 `model_hint` 的优先级；
- `src/agents/subagent_tool/mod.rs`：provider hint override 和 routing store 注入；
- `src/orchestrator/deps_builder/provider_chain.rs`：per-provider hint override registry；
- `src/orchestrator/harness_bridge/mod.rs`、`runner_impl.rs`；
- `src/orchestrator/dispatch.rs`；
- `src/gateway/execution_engine/run_loop/inner.rs`；
- `src/context/compact/cheap_poison.rs`；
- `src/extension/declared_model.rs`、`src/extension/mod.rs`；
- `src/builtin_tools/agent_manage/info.rs`；
- `interfaces/webchat/src/platform/wide/views/settings/routing_rules.rs` 和 agent/provider 表单。

学习路由模块为：

- `src/routing/recall.rs`
- `src/routing/experience_store.rs`
- `src/routing/observer.rs`
- `src/memory/store/sqlite/routing_experience.rs`
- `src/memory/store/sqlite/schema/ddl.rs` 中 `routing_experiences` 表及 schema 初始化；
- `src/routing/mod.rs` 中对应导出和 `RoutingAttribution`；
- `src/orchestrator/harness_bridge/runner_impl.rs`、`src/agents/runtime.rs`、`src/agents/subagent_spawner/mod.rs` 中的注入和记录。

`src/routing/` 不能整体删除。channel/peer -> agent/session 的确定性路由仍是 I/O plumbing；只删除 model routing 的 recall、experience、observer 和 attribution 部分。

### 3.3 UI、API 与用户入口

本次删除必须同步修改用户可见设置，不能只删 Core 后留下“点击成功但运行时无效”的入口：

- Webchat Panel：删除 MoA settings 页面、preset editor、save-traces 选项和相关 RPC；删除 `routing_rules` 页面及其 provider/model 选择策略，因为当前页面是模型/provider 路由配置，不是 `src/routing/` 中保留的 channel/peer -> agent/session 路由；保留 configured provider/model catalog、普通 model picker 和显式模型选择；
- Webchat chat 窗口：收缩 `interfaces/webchat/src/components/model_picker.rs` 和 chat state，使选择器只展示真实可用的 provider/model，不展示 `moa` pseudo-provider、MoA preset 或 AgentDef 默认模型来源；切换后仍调用普通 `select_model`；
- Webchat agent 管理页面：删除 `model_hint` / `provider_hint` 的模型职责编辑字段和展示字段，保留 agent prompt、工具、预算、生命周期和权限设置；已有 agent frontmatter 中的这两个旧字段采用“忽略并给出弃用诊断”的兼容行为，不因升级直接拒绝加载；
- Webchat settings/sidebar/app 注册：移除 MoA、routing strategy、model ladder 页面入口及 locales，更新 settings route；其他 generation/embedding/reranking provider 表单中的 `model_hint` 仅表示各自 provider 配置，不属于本次 Agent model routing 删除；
- TUI：同步删除 model routing/MoA 的设置入口、MoA slash/help/status 文案和伪模型行；保留普通 model picker、当前显式 model pin 和实际 serving model 的状态显示；
- CLI/Gateway/API：删除 MoA RPC、slash command、builtin tool 和策略性 routing API；普通 provider catalog、`list_models`、`select_model` 和显式 subagent model 参数继续可用。当前源码搜索未发现 MCP/ACP 直接暴露 MoA 或 routing strategy 的生产入口；实现阶段仍必须把 MCP/ACP schema、tool census 和外部 handler 纳入归档引用闭包，若发现间接注册则按同一删除规则处理。

删除入口的错误边界：旧 `moa` builtin、`/moa` slash command、`moa:*` model request、`moa.*` RPC 和被删除的 Panel/TUI 操作都返回各自现有的 unknown-tool/unknown-method/removed 错误，不成功 no-op；旧 `[moa]` 配置和旧 trace 仅按第 5 节的兼容策略处理。普通 `model` 选择仍必须到达同一个 `effective_model_directive`。

UI 验收要求：用户在 Panel 或 chat selector 中做出的普通模型选择必须到达同一个 `effective_model_directive`；所有被删除入口都应得到明确的 unknown/removed 行为，而不是成功返回 no-op。

### 3.4 不属于本次删除的内容

- provider protocol adapter、provider catalog、configured model discovery；
- `ModelOverrideProvider` 的显式绑定行为；
- session `select_model` 的普通模型 pin；
- provider failover、retry、cooldown 和 availability checks；
- 由主 Agent 明确调用的 subagent、parallel task、team、group chat、background tracker；
- channel/session routing；
- embedding provider 本身及其他非模型选择用途的 memory retrieval；
- 与“多个 Agent 执行独立任务”有关的功能，除非它实际提供的是多模型 proposal/aggregator 模式。

## 4. 归档资产 / Archive Asset

归档必须在删除前生成，且与删除提交分开可审查。建议路径：

```text
docs/archive/removed-model-routing-moa/
  aleph-model-routing-moa-pre-removal-2026-10-19.tar.gz
  MANIFEST.json
  SHA256SUMS
  README.md
```

归档规则：

1. `git status --short` 必须为空，先记录 source commit、branch、repository root、Cargo.lock hash 和生成工具版本。
2. archive 包含所有被删除的完整文件；对于保留文件中被删除的代码块，包含这些文件的 pre-removal 完整快照，并在 manifest 中标记 `modified_source_snapshot`。
3. 每个文件保留原始相对路径，不把代码平铺到 archive 根目录。
4. 包含测试、配置 schema、Panel/API、locale、共享协议兼容代码、文档和迁移辅助代码；不包含 `target/`、构建产物、密钥、用户配置、数据库、日志和临时文件。
5. `MANIFEST.json` 的输入集合由实现脚本生成：先枚举所有被删除的 module/type/function/tool/event symbol，再对 Rust、serde tag、字符串注册表、宏/feature-gated 引用和跨 crate wire 引用递归搜索，直到集合不再增加。生成器同时输出未能静态解析的动态引用列表；实现者必须逐项人工确认，并把确认结果写入 manifest。测试、文档、归档工具和生成文件按类别分别记录，不能因为不在生产编译图就静默漏掉。集合必须包含 `shared/protocol/src/events.rs`、`shared/protocol/src/trace_presentation.rs`、`shared/protocol/src/trace_replay.rs` 以及实际命中的前端共享文件。手工候选清单只能作为审查索引，不能作为归档完整性的权威来源。
6. `MANIFEST.json` 记录每个文件的原路径、archive 路径、类别、SHA-256、是否完整文件、删除原因和生产/测试属性。
7. `SHA256SUMS` 对压缩包、manifest 和 README 做校验。
8. `README.md` 说明该代码已从生产编译图删除，恢复它需要重新审查当前 provider、session、wire 和安全契约；归档不是备用 crate，也不应加入 workspace。
9. archive 文件必须作为普通仓库资产提交，不能依靠本地未跟踪文件保存。若压缩包过大，必须在实现前报告大小并调整为分卷或 source-only 归档，不能静默跳过。

实现期间每次删除前先运行 manifest 校验；删除后用 archive 解压到临时目录，对比 manifest，确保没有漏档。

## 5. 配置、session 与数据库迁移 / Migration

### 5.1 `[moa]` 配置

删除 `Config.moa` 和 `MoaToml` 类型后，旧 `config.toml` 中的 `[moa]` 因 `Config` 不使用 `deny_unknown_fields`，应继续能够启动，且不再激活任何行为。

实现阶段不自动重写用户配置。启动诊断应在发现原始配置中存在 `[moa]` 时发出一次明确提示，说明该 section 已废弃、不会执行，并建议用户手动删除。这样可以避免升级时静默写坏用户配置；配置下次由 Aleph 序列化保存时，未知的 `[moa]` section 会自然消失。提示本身不得加载或执行 MoA 类型。

### 5.2 session activation

`session_moa_handle` 是进程内状态，不是持久 session 数据。删除它后，重启自然丢弃旧 MoA activation；正在运行的旧进程不能与新二进制混用。run construction 必须删除 `take_for_run`、`restore_one_shot` 和 MoA activation 分支，普通 `select_model` session pin 继续保留并按 `RunStarted` 恢复。

### 5.3 trace 与 usage 历史数据

旧 trace 中的 MoA 事件不再进入新的运行时路径，但历史解码必须保持可读。用户已选择保留最小的 deprecated typed variants：`shared/protocol/src/events.rs` 保留 `MoaAdvisor`、`MoaAggregating`、`MoaAdvisorSpend`、`MoaTurnTrace` 的历史 wire 变体，`trace_presentation.rs`、`trace_replay.rs` 和前端共享渲染路径继续能够读取并展示其历史字段；这些变体没有任何 producer、usage 聚合、MoA 操作按钮或执行语义。未知事件 tag 和格式损坏仍按现有 trace reader 的错误/跳过契约处理，不借此引入新的隐式恢复策略。该兼容层不是 MoA runtime 模块，不能重新暴露 `MoaProvider`、advisor 或 aggregator。新 trace 不得生成这些事件。

MoA 专属 usage 聚合函数和测试删除。历史 trace 行不在本次自动删除；它们作为历史资产留在数据库中，但新代码不再查询为当前 usage。`resilience/database/traces.rs` 中的通用 trace 查询必须继续工作。

### 5.4 routing experience 数据

停止写入和读取 routing experience。新数据库不再创建 `routing_experiences`、`routing_exp_vec_map` 以及对应的 `routing_exp_vec_*` 虚表；已有数据库中的这些对象第一阶段全部保留但永不访问，不执行 `DROP TABLE` 或 vector cleanup，避免升级时发生不可逆数据删除。无论 relational 表、mapping 表和 vector 虚表全部存在、部分存在、扩展不可用或完全不存在，Aleph 都应忽略这些对象并继续启动；不得根据部分对象隐式补建、重建索引或做完整性校验。旧表成为惰性遗留数据，不进入运行时查询，也不参与普通 memory 初始化以外的业务路径。无需新增 schema version；schema 初始化应只移除这些 DDL 的创建入口，并覆盖上述数据库 fixture。后续若确实需要回收空间，另做带备份和显式用户操作的数据库清理任务。

## 6. 实施阶段 / Implementation Phases

### Phase 0 — Snapshot and archive

- 冻结 source commit，生成 archive、manifest 和 checksum；
- 用 `rg` 生成完整生产引用图；
- 确认当前工作树干净；
- 先提交 archive 和设计所需的迁移说明。

### Phase 1 — Remove MoA execution

- 删除 `src/providers/moa/`、`session_moa_handle.rs`、MoA config/type、builtin tool、gateway RPC 和 Panel；
- 从 provider registry、builtin registry、slash command、executor、trace、usage、capability census 中移除注册和执行路径；
- 从 `runner_impl.rs` 删除 MoA activation、advisor fan-out、aggregator identity、acting chain 和 restore 分支；
- 删除 `loop_tool.rs` 的 proposal/aggregator MoA actions，只保留普通主 Agent delegation；
- 删除相关测试和旧设计文档的生产引用，但保留设计历史文档本身，标注 superseded。

### Phase 2 — Remove learned and role-based model routing

- 删除 routing recall/experience/observer/attribution 及 SQLite helper；
- 从 runtime、spawner、subagent tool、runner、context compact 和 extension 中移除 routing store 注入；
- 从 `AgentDef`、loader、frontmatter schema、agent info 和 Panel 表单删除 `model_hint` / `provider_hint` 的模型职责语义；旧 frontmatter 字段只忽略并诊断，不阻断 agent 加载；
- 删除 per-provider hint override registry；
- 保留显式 subagent model/provider request，并将其作为唯一 child model override 来源。

### Phase 3 — Simplify explicit model binding

- 让 `effective_model_directive` 只处理显式 per-turn 和 session selection；
- `select_model` 删除 `moa` / `moa:<preset>`，继续支持普通 configured model；
- `list_models` 删除 MoA pseudo-provider 和 MoA preset 输出；
- 保留 `ModelOverrideProvider`、catalog、budget attribution 和 failover；
- 清理 Panel/TUI 中关于角色模型、routing rule、model ladder 和 MoA 的用户入口；同步修改 chat 窗口 model picker、chat state、settings route、agent editor、sidebar、locales 和 TUI status/picker；保留普通 model picker、provider catalog、显式 `select_model` 与实际 serving model 展示。

### Phase 4 — Compatibility, docs, and archive verification

- 加入旧 `[moa]` 配置的一次性诊断，不自动改写；
- 加入旧 trace typed variants 的解码/展示回归测试，并确认新运行不会生产这些事件；
- 加入旧 Agent frontmatter model/provider hint 的忽略与弃用诊断测试；
- 从 schema 初始化中停止创建 `routing_experiences` 及其 mapping/vector 对象，不删除已有对象，并覆盖全存在、部分存在、扩展不可用和全不存在 fixture；
- 对 MCP/ACP schema、tool census、外部 handler 做一次显式负向 census，确认没有间接 MoA/routing surface；
- 更新 `AGENT_SYSTEM.md`、`MODEL_CATALOG.md`、`AGENT_DESIGN_PHILOSOPHY.md`、provider/agent guides 和 stale model router references；
- 解包 archive 并按 manifest 校验；
- 用全仓搜索确认生产源码不再引用已删除 symbol。

## 7. 验收标准 / Acceptance Criteria

### Functional

- 主 Agent 可以通过 `list_models` 发现用户已配置模型，并通过 `select_model` 或 subagent request 显式选择模型；
- 主 Agent 可以显式创建一个或多个 subagent，子任务不会被 AgentDef 的角色 hint 暗中改写；
- 无显式模型时使用当前默认 provider/model；
- provider 请求失败时 retry/failover 仍按既有错误分类工作；
- session restart/resume 能恢复普通 `select_model` pin，不会恢复 MoA 状态；
- Panel、chat model selector 和 TUI picker 只展示真实 configured provider/model；普通选择仍到达 `effective_model_directive`；删除的 MoA/routing 入口不再渲染，也不返回成功 no-op；
- 旧 `[moa]` 配置、旧 MoA trace 和旧 `routing_experiences` 不会阻止启动、普通对话或恢复；
- 不再生成 advisor fan-out、aggregator call、MoA trace、routing recall 或 routing experience 写入。

### Structural

- 生产编译图中不存在 `src/providers/moa`、`session_moa_handle`、`MoaProvider`、`MoaManageTool`、`RoutingRecall`、`RoutingExperienceStore`、`OutcomeObserver` 等已删除 symbol；
- `AgentDef` 不再把 model/provider hint 作为模型职责分配字段；
- `src/routing/` 仍保留 channel/session route resolution；
- `ModelOverrideProvider` 只由显式模型选择路径使用；
- archive manifest 与 SHA-256 校验通过；
- 文档不再描述一个不存在的独立模型路由器。

### Stability

- 保留单一 `Gateway -> Orchestrator -> Harness` 运行拓扑；
- 保留并发、超时、递归深度、取消、child-first 清理、retry、failover 和 fail-closed 恢复；
- 不引入第二套 fallback、第二份模型目录或新的策略性中间件；
- 不自动删除用户数据库中的历史 routing 数据。

## 8. 验证计划 / Verification Plan

按变更阶段运行，而不是等到最后只跑一条大命令：

1. archive manifest、SHA-256、`git diff --check`；
2. `rg` 生产引用 census：MoA、routing experience、model/provider hint、proposal aggregator；
3. Rust format check；
4. Core targeted tests：provider binding、`select_model`、model directive precedence、subagent explicit override、failover、resume、trace reader；
5. `cargo check -p aleph-core` 及相关 workspace crates；
6. `cargo test -p aleph-core`，再运行 `test-helpers` 集成目标；
7. Webchat `--lib --no-run`、相关 Panel settings/chat/agent tests 和 TUI tests；
8. workspace all-target clippy；
9. fresh isolated runtime smoke test：默认模型、显式换模、显式 subagent model、provider failure/failover、cancel、resume、旧 `[moa]` config；
10. 最后运行项目 `DEVELOPMENT.md` 规定的最小可信验证集。

任何失败都要先判断是删除后的真实回归、旧测试仍在断言已删除能力，还是基线环境问题；不能通过保留隐藏兼容路由来让测试变绿。

## 9. 回滚 / Rollback

实现提交可以回滚到删除前 commit；archive 本身不参与编译。已存在的 `[moa]` 配置不会被自动改写，旧数据库 routing table 不会被 drop，因此代码回滚不会因为数据迁移而失去恢复路径。

如果 archive 解包后无法还原删除前文件，或普通 model selection、failover、resume 的任何一项行为无法证明，则停止后续删除，保留已生成 archive 和失败日志，先修正迁移方案。

## 10. Builder handoff / 交给实现者

实现者需要按 Phase 0–4 执行，并把最终文件清单、archive checksum、删除 symbol census、配置兼容结果和验证命令实际输出附在实现报告中。实现者可以决定共享文件中具体 import/测试的整理方式，但不能改变以下边界：MoA 整体删除、策略性模型路由删除、显式模型选择保留、provider failover 与 subagent 生命周期保留、旧数据库不自动 drop。

本设计不包含代码实现，也不等同于实现批准；设计文档通过 builder check 后，还需要用户批准最终设计，随后才能进入 implementation plan 和代码修改。
