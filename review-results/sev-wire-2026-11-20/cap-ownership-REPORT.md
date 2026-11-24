# Ownership tree 与 census 断线审计 / Severed-wire audit

基线：`d57791367`。完整通读 `src/capability/ownership.rs` 和 `src/capability/census.rs`；通过指定 graphify 查询定位，再以源码行号和全仓 Rust 引用核实。静态审计，不运行 Cargo，不修改源码。共 **7 项**：**4 项无生产消费者、3 项 census 静态漏检**；处置假设 **CONNECT 3 / CUT 3 / DECIDE 1**。

## 断线 / Severed wires

### CO-01 - PublicApprovalLedger 的显式拒装入口没有调用者
- Form: no-consumer
- Producer: src/gateway/telecom/public_approval.rs:1590 - `public_approval::decline_global`，2026-11 的现存证据；由 CO-05 的 census 检查暴露。
- Consumer: NONE FOUND: `rg -n '\bdecline_global\b' --glob '*.rs' src desktop plugins crates tests -g '!**/census.rs'`
- Live caller search: 上述命令找到该定义及其他模块的同名调用；未找到该函数的调用、导入别名或测试调用。启动的活入口是 `src/bin/aleph-server/commands/start/mod.rs:3586` 的 `public_approval::install_global`；`src/bin/aleph-server/commands/start/mod.rs:3596` 的失败分支记录告警并返回 `None`，未调用该函数。
- Symptom: 生产不会通过这个入口写下“有意拒装”原因。账本的正常安装和使用仍有效；现有失败分支处理的是已安装的实例冲突，不是一个尚未安装的可选分支。不应为满足守卫而在该分支伪造一次拒装。
- Hypothesis: CUT - 当前启动设计没有需要调用该入口的未安装分支。
- Confidence: HIGH

### CO-02 - 精确 incarnation 回滚只有测试调用，非生产撤回路径
- Form: no-consumer
- Producer: src/capability/ownership.rs:456 - `register_if_absent_with_token` 返回 `RegistrationToken`；`src/capability/zahir_facade.rs:809` 将其交给终端/UI 注册者。
- Consumer: src/capability/ownership.rs:535 - `rollback_if_matches`；唯一非测试定义的转接是 `src/capability/zahir_facade.rs:840`，其调用点在 `src/capability/tool_adapters.rs:183` 的 `#[cfg(test)]` 失败注入内（实际调用在 `src/capability/tool_adapters.rs:190`）。
- Live caller search: `rg -n '\b(rollback_if_matches|rollback_terminal_session|bind_terminal_session|commit_session|withdraw_session_deferred)\b' --glob '*.rs' src desktop plugins crates tests`：bind 和 commit 有生产调用；精确回滚只有注入调用和 `src/capability/special_tests.rs:644`、`:664`、`:690` 的单测调用，没有生产调用。
- Symptom: 生产 PTY 发布失败走 `src/gateway/pty/manager.rs:865` 的撤回分支（`src/capability/tool_adapters.rs:244` → `src/capability/zahir_facade.rs:849` 的 `revoke_bound`），不是使用 token 作精确回滚。生产有清理；这里不是漏清理或资源泄漏的证据。UI 也不消费 token：`src/gateway/capability_ui.rs:184` 仅检查 bind 错误，注册失败在 `src/gateway/capability_ui.rs:212` 撤回。
- Hypothesis: DECIDE - 先确定生产语义应是永久撤回 ID，还是仅撤销本次绑定，再决定保留测试专用能力还是接线；不能直接以注入测试替代生产契约。
- Confidence: HIGH

### CO-03 - 显式替换与无条件撤销只由测试及未挂载会话文件调用
- Form: no-consumer
- Producer: src/capability/ownership.rs:409 - `OwnershipTree::register`（可替换），以及 `src/capability/ownership.rs:831` - `OwnershipTree::revoke`（可给未知 ID 建立 tombstone）。
- Consumer: src/capability/session.rs:159、src/capability/session.rs:185 - `SessionCapabilityRegistry` 的定义型调用；`src/capability/mod.rs:40` 起的模块清单没有 `mod session`，未发现别处挂载此文件。其余可解析到该类型的方法调用是测试，例如 `src/gateway/channel_registry/capability.rs:260`、src/gateway/channel_registry/capability.rs:474（测试模块标记在 `src/gateway/channel_registry/capability.rs:213`）。
- Live caller search: `rg -n -U '\b(OwnershipTree::(register|dispose|revoke)|tree\s*\.\s*(register|dispose|revoke)\(|ownership\s*\.\s*(register|dispose|revoke)\()' --glob '*.rs' src desktop plugins crates tests`，并用 `rg -n 'mod session\b|path\s*=.*capability/session' --glob '*.rs' src desktop plugins crates tests` 和 `rg -n '\b(SessionCapabilityRegistry|SessionCapabilitySink|SessionCapabilityMetadata)\b' --glob '*.rs' src desktop plugins crates tests` 核实模块归属。没有找到生产消费者；`src/gateway/channel_registry.rs:2537` 等命中在 `src/gateway/channel_registry.rs:2406` 的测试块内。通用同名注册方法并非 `OwnershipTree`。
- Symptom: 仓内生产没有通过这两个 API 替换绑定或 tombstone 未绑定 ID。现役注册和管理仍使用 `src/extension/hooks/registry.rs:330` 的 `register_if_absent`、`src/capability/projection_host.rs:916` 的 batch 与 `src/capability/zahir_facade.rs:470` 的 `revoke_bound_with_receipt`；未观察到运行时操作损坏。这不是要求挂载额外 CapabilityKind。
- Hypothesis: CUT - 评估公共 API 兼容性之后，将无生产用途的旧入口收敛为测试专用或移除；不要把无条件 tombstone 替代现役的绑定限定撤销。
- Confidence: HIGH（仓内；未核实外部 Rust 客户端）

### CO-04 - `dispose` 的布尔兼容入口只在测试使用
- Form: no-consumer
- Producer: src/capability/ownership.rs:895 - `OwnershipTree::dispose`，把 `dispose_with_receipt` 压成“是否移除了绑定”的布尔值。
- Consumer: NONE FOUND: `rg -n -U '\.\s*dispose\(\s*(scope|lifetime|LifetimeScope::\w+)' --glob '*.rs' src desktop plugins crates tests`；真实命中示例 `src/capability/projection_host.rs:2739`、`src/gateway/channel_registry/capability.rs:492` 都是测试。
- Live caller search: 上述命令加 CO-03 的接收者搜索没有找到该包装入口的生产调用。`rg -n 'dispose_with_receipt|dispose_observed_scope|fn dispose_runtime' src/capability/ownership.rs src/capability/session.rs src/capability/zahir_facade.rs src/capability/diagnostic_control.rs` 确认活入口为 `src/capability/diagnostic_control.rs:260` → `src/capability/zahir_facade.rs:474` → `src/capability/ownership.rs:911`。
- Symptom: 无已知生产故障；实际 dispose 仍在线。死的是布尔投影，而不是生命周期终止本身；`false` 也不能描述空树是否刚被永久终止。
- Hypothesis: CUT - 评估公共 API 兼容性后，测试直接断言 receipt 和终止状态即可。
- Confidence: HIGH（仓内）

## 非连线类静态缺陷

### CO-05 - decline 守卫用裸函数名合并不同模块，确实漏报 CO-01
- Form: other-static-defect
- Producer: src/capability/census.rs:2092 - `capability_wrappers` 按短名合并 `declines`，`src/capability/census.rs:2097` 又以相同键覆盖 `decline_file`。
- Consumer: src/capability/census.rs:3164 - `every_decline_wrapper_has_a_production_caller` 命中一次就把裸名加入 `called`；`src/capability/census.rs:3172` 也只按裸名判断孤儿。
- Live caller search: `rg -n 'pub.*fn decline_global\(|tasks::cron::decline_global|tasks::heartbeat::decline_global|codex_token_refresher::decline_global' --glob '*.rs' src desktop plugins crates tests` 找到 `src/tasks/cron/mod.rs:121`、`src/tasks/heartbeat/mod.rs:82`、`src/gateway/codex_token_refresher.rs:69` 以及孤儿 `src/gateway/telecom/public_approval.rs:1590`；CO-01 的全名搜索排除了后者的呼叫。`src/bin/aleph-server/commands/start/mod.rs:1334` 的带路径调用能为整个 `decline_global` 键投票。该守卫自身是测试，不是生产诊断。
- Symptom: 对当前树，孤立的审批账本入口不会被此断言列为 orphan。未修：`src/capability/census.rs:2028` 已记下这一类碰撞；这次核实了真实漏报，不是仅凭可能新增同名函数推断。未运行测试，故不声称整套守卫当前为绿。
- Hypothesis: CONNECT - 按定义身份核对调用；不可解析的调用不能给所有同名定义记账。
- Confidence: HIGH

### CO-06 - 真实 ProjectionHost slot 被 MissingSemantics 专项守卫排除
- Form: other-static-defect
- Producer: src/capability/projection_host.rs:95 - `PROJECTION_HOST`，ID `capability/projection-host`，声明 `MissingSemantics::FailsClosed`。
- Consumer: src/capability/census.rs:3227 - `slot_declarations` 跳过整个 `src/capability/`；`src/capability/census.rs:3325` 的 `every_slot_pins_its_own_missing_semantics` 只查扫描结果。
- Live caller search: `rg -n '\b(PROJECTION_HOST|projection_host_slot|capability/projection-host)\b' --glob '*.rs' src desktop plugins crates tests` 定位该声明及 `src/capability/mod.rs:383` 的诊断 roster 接入；`rg -n '\bset_projection_host\(|\bprojection_host\(\)|\bslot_declarations\(|\bcapability_handles\(' --glob '*.rs' src desktop plugins crates tests` 找到 `src/bin/aleph-server/commands/start/mod.rs:321` 的生产安装、`src/gateway/capability_ui.rs:66` 的生产读取和 `src/capability/census.rs:3326` 的测试调用。
- Symptom: roster 和运行时连线都在；但该生产 slot 的 missing-semantics 专项检查会无条件略过它，不能证实这条声明与专用测试一致。未修：`review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md:114` 的 C05-7，在当前新增生产 slot 上有具体证据。
- Hypothesis: CONNECT - 按真实测试边界筛选 fixture，保留对全体生产 slot 的声明检查。
- Confidence: HIGH

### CO-07 - Busy queue 持久化目录是活启动句柄，原始 census 不收录
- Form: other-static-defect
- Producer: src/gateway/busy_queue/durable.rs:96 - `STORE_DIR: LazyLock<Mutex<Option<PathBuf>>>`，由 `src/gateway/busy_queue/durable.rs:239` 的 `*STORE_DIR.lock()… = Some(dir)` 写入。
- Consumer: src/capability/census.rs:296 - writer 只认 `set/store/swap`；`src/capability/census.rs:847` 的替代分支只找参数化首调用初始化。以上都不识别锁内赋值，因此 `src/capability/census.rs:917` 的结果忽略此真实 boot handle。
- Live caller search: `rg -n '\b(durable::init|durable::record_enqueued|STORE_DIR|store_dir\()' --glob '*.rs' src desktop plugins crates tests` 找到 `src/bin/aleph-server/commands/start/mod.rs:1831` 的活安装和 `src/gateway/busy_queue/spawn.rs:72`、`src/gateway/inbound_router/executor.rs:525` 的活调用；`src/gateway/busy_queue/durable.rs:286` 读空目录即不落盘。这里没有把同名 `STORE_DIR` 的其他模块当成消费者。
- Symptom: census 不会把这个决定队列能否跨重启持久化的句柄计入启动接线保护；删除该安装不会由这个句柄的 census 覆盖发现。当前生产接线完好，本项不是持久化已损坏。未修：`src/capability/census.rs:97` 已承认 interior-mutable 盲区，此处给出 busy queue 的现存实例。
- Hypothesis: CONNECT - 识别此种安装或将句柄迁往显式 slot，使 census 能保护生产安装路径。
- Confidence: HIGH

## 交叉核对 / Cross-checks

- 注册与字段是两个键空间：`src/capability/ownership.rs:469` 注册的是带精确 `VisibilityScope` 的运行时绑定；`src/capability/census.rs:770` 扫描的是启动时的 Rust `static` 和 slot，`src/capability/mod.rs:77` 明确将 census 编为测试模块。生产 Doctor 实际读 `src/capability/mod.rs:334` 的 `ALL_SLOTS`（`src/diagnostics/checks/capability_wiring.rs:250`）。因此按 slot ID 核对 roster，而不能要求每个动态 ownership 节点或其 generation/owner 字段出现在 census。`HandleSite` 的 file/name/container/is_slot 在 `src/capability/census.rs:935` 的测试使用，slot_id 在 `src/capability/census.rs:992` 的 roster 测试使用。
- 对生产 ownership 核实了共享树从 `src/bin/aleph-server/commands/start/mod.rs:243` 接到 `src/capability/projection_host.rs:703` 的订阅；四种 mutation 事件统一重算，不需要逐 variant 分派（`src/capability/projection_host.rs:1580`）。generation/owner 由 `src/capability/policy.rs:157`、`src/capability/policy.rs:162`、`src/capability/policy.rs:212`、`src/capability/policy.rs:224` 消费，绑定快照也由 `src/capability/zahir_facade.rs:1602` 重验；Memory 在 `src/memory/runtime/mod.rs:462` 确实轮转。`resolve` 有 `src/extension/hooks/registry.rs:112` 的生产消费者。没有把只在文档或测试里构造的值当作生产写入。
- 已查对 `review-results/severed-wire-2026-08-17/capability/REPORT.md:13` 的测试型 census 结论和十月报告的已修/未修分类。未把正常的 `#[cfg(test)]` 模块、明确的测试故障注入、未挂载的其他 CapabilityKind 或 `AGENTS.md` 指明的 EffectClaim producer integration、Safe Replay、外部效果 exactly-once、通用持久调度及完整 ACP server gate 延期算作断线。两文件内未确认另有实际可达的假成功 handler/空 stub、死配置项、找不到 handler 的客户端、错拼生产路径或未声明 feature 导致的 far-end。

## 未覆盖的接缝

- 未执行 Cargo、feature 构建矩阵、真机启动、并发或故障注入；不对审计之外的目标特化、宏生成注册及 Rust 依赖方的外部调用作断言。
- 未穷举 census 的整个文本语法：同名原始 static、函数内 static、字符串中的括号/标识符等十月历史条目未形成独立证据项；无实证不计数。未开展 workspace 全量 slot/诊断或全路由/限流分类器普查，只跟到本 slice 的消费者。
- 未检查审计范围以外的后续清理正确性，也未修改任何源文件；仅写本报告。既有 `graphify-out` 工作树状态保持原样。
