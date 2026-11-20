# Capability resolution and backends: 断线静态审计 / Severed-wire audit

**基线 / Baseline:** `d57791367`，`Aleph-wt-sev3`。共 **9 项**：6 条断线、3 项尚无生产消费者的静态缺陷；按处置假设分为 **CONNECT 2 / CUT 2 / DECIDE 5**。未改源码，未运行 Cargo。

**范围 / Scope:** 完整阅读 `src/capability/{resolver,provider,provider_registry,provider_backend,backend,versioned_registry}.rs` 和 `src/capability/{resolver_tests,resolver_audited_tests,resolver_source_tests}.rs`；从根目录对导出接口、调用和实现运行 `rg --glob '*.rs' .`，同名方法按接收者和 `#[cfg(test)]` 边界判别。`graphify query "how are capability providers resolved and dispatched" --budget 600` 只用作导航，以下结论均以源码为据。`CUT` 指建议删除或明确隔离未用接口，不是已执行的改动。

## 断线 / Severed wires

### CR-01 - CapabilityResolver 没有生产构造入口
- Form: no-consumer
- Producer: `src/capability/resolver.rs:15` - `CapabilityResolver`；`:24`/`:31` 构造，`:44`/`:56`/`:79`/`:87`/`:99`/`:112` 为发现、解析、校验、失效和 lookup 接口。
- Consumer: NONE FOUND: `rg -n '\bCapabilityResolver\b|\bResolverConfig\b|\bcapability::resolver\b' --glob '*.rs' .`
- Live caller search: 上述命令仅命中 resolver 实现/配置、三个意图测试和 `benches/capability_resolver.rs:125,182`；没有生产构造点，因而也没有生产 `invalidate`/`invalidate_all` 调用链。实际启动从 `src/bin/aleph-server/commands/start/mod.rs:311` 到 `src/capability/projection_host.rs:657`，直接装配 `ZahirFacade`。
- Symptom: 生产仍能使用 facade 发现和解析能力，但不会经过这层的 metadata cache、并发源读取上限和失效 API。`src/capability/cache_policy.rs:21` 是程序化构造配置，不是一个已解析却无人读取的部署配置段。
- Hypothesis: DECIDE - 先决定这是供生产使用的性能/限流层还是实验接口；测试所断言的饱和降级不应被无意引入生产。
- Confidence: HIGH

### CR-02 - 标为完成的 SessionCapability 不在模块树中
- Form: never-compiled-far-end
- Producer: `src/capability/session.rs:150` - `SessionCapabilityRegistry` 实现 `SessionCapabilitySink`（`:151` 注册、`:182` 卸载）；`docs/superpowers/plans/2026-11-05-phase6-capability-system.md:22` 将该注册工作标为完成。
- Consumer: NONE FOUND: `rg -n '\b(SessionCapabilityRegistry|SessionCapabilitySink|ProviderRemovalError|unregister_id|ProviderRegistryChange)\b|capability::session|path\s*=\s*"(capability/)?session\.rs"|\bmod session\b' --glob '*.rs' .`
- Live caller search: 这组特定符号只命中 `src/capability/session.rs` 自身；同名 `mod session` 在其他模块下。`src/capability/mod.rs:57` 至 `:74` 没有这条模块声明，也未找到 `#[path]` 接入或生产 sink 调用。此处不是漏声明 Cargo feature，而是文件本身未被纳入编译。其 `#[cfg(test)]` 测试同样不进测试模块树。
- Symptom: Session 生命周期不会借此注册、撤销或发现 Session Resource（不表示普通 Session RPC 不工作）。仅补 `mod` 也不够：`src/capability/session.rs:18,184,266` 需要当前 `src/capability/provider_registry.rs:87` 的实现不存在的 `ProviderRemovalError`、`unregister_id`、`ProviderRegistryChange`。这是现有通用 Resource 注册路径的未接入工作，不是要求增加另一种 CapabilityKind backend。
- Hypothesis: CONNECT - 恢复并对齐 Session sink 与真实生命周期、注册表卸载和通知接口，验证注册及撤销到达生产 facade。
- Confidence: HIGH

### CR-03 - ProviderTransition 通知没有生产订阅者
- Form: no-consumer
- Producer: `src/capability/provider.rs:310,403` - `ProviderHandle::with_ready_commit` 和 `transition` 发送 `ProviderTransition`。
- Consumer: `src/capability/provider.rs:408` - `ProviderHandle::subscribe_changes` 存在；无生产消费者，只有下列测试。
- Live caller search: `rg -n '\bProviderTransition\b|\.subscribe_changes\(' --glob '*.rs' .`；按 handle 类型核对，订阅在 `src/capability/provider.rs:684,735` 和 `src/tools/registry.rs:3215,3320`，均处于 `#[cfg(test)]`。生产确有提交端，如 `src/tools/registry.rs:1417`、`src/gateway/channel_registry/binding.rs:1008`；工具注册表 publication、ownership 等同名订阅不是此总线。
- Symptom: Provider 状态和代际仍可由同步读端读取，但没有生产组件因这条通知而更新投影或触发动作。不是 durable recovery 缺口；另见 CR-S01 的顺序性质。
- Hypothesis: DECIDE - 先确认是否需要生产生命周期观察者；没有需求时可把观察接口收敛为测试用途。
- Confidence: HIGH

### CR-04 - 旧同步注册表存在注册与 dispatch，但未挂载
- Form: no-consumer
- Producer: `src/capability/backend.rs:118` - `LegacyCapabilityHandlerRegistry`，`:147` 可接事件 emitter、`:154` 注册、`:192` 同步 dispatch；ambient/UI/terminal 的注册助手分见 `src/capability/ambient.rs:223`、`src/capability/ui.rs:222`、`src/capability/terminal.rs:236`。
- Consumer: NONE FOUND: `rg -n 'LegacyCapabilityHandlerRegistry|\b(register_process_handler|register_user_handler|register_environment_handler)\b' --glob '*.rs' .`
- Live caller search: 上述命令加 `rg -n 'LegacyCapabilityHandlerRegistry::(new|default|with_events)|LegacyCapabilityHandlerRegistry\s*\{|ambient::register_|terminal::register_handler|ui::register_handler' --glob '*.rs' .`。构造仅在 backend/ambient/UI/terminal 测试；同包未限定的 `register_handler`、`registry.dispatch` 也只出现在测试消费端。生产 `src/bin/aleph-server/commands/start/mod.rs:300` 构造的是 `src/capability/special.rs:451` 的异步 `CapabilityHandlerRegistry`，不是旧表。
- Symptom: 往旧表接入 handler 或 emitter，不会让生产 special 分发或订阅看到它。不能据此断言生产 Ambient/UI/Terminal 都未挂载，也不是 RPC 404。
- Hypothesis: CUT - 已有生产异步路径，除非另有仓外兼容承诺，否则隔离旧同步表及其注册助手，避免误接。
- Confidence: HIGH

### CR-05 - Memory 激活 hook 已注册，Zahir 激活没有发起方
- Form: no-consumer
- Producer: `src/memory/runtime/mod.rs:246` - `provider_registration` 在 `:252` 装入 `MemoryActivator`；`src/capability/zahir_facade.rs:398` 是生产注册点。
- Consumer: `src/capability/zahir_facade.rs:2082` - `ZahirFacade::activate` 在 `:2088` 调用 hook。只有定义及转发，未发现生产激活发起方。
- Live caller search: `rg -n '(\.activate\s*\(|\bZahir::activate\b|\bProviderActivator\b)' --glob '*.rs' .`；核对接收者，`src/capability/events.rs:865` 是审计 facade 的转发，`src/capability/events_tests.rs:685` 是测试发起端，`benches/capability_resolver.rs:75` 是代理。策略的 `CapabilityActivationController`、PTY 的 `.activate` 均不是 `Zahir::activate`。
- Symptom: 生产不会经由 Zahir 请求此 hook。并非 Memory 不启动：`src/bin/aleph-server/commands/start/mod.rs:1565` 直接执行 `memory_runtime.start().await?`；现有 Memory hook 在 `src/memory/runtime/mod.rs:728` 只检查 Runtime 已在运行，并不启动它。
- Hypothesis: DECIDE - 确认是否需要显式的按需激活入口；若只允许 owner 自己管理启动，应明确接口是探测/验证还是移除未用的激活层。
- Confidence: HIGH

### CR-06 - Backend 快照 trait 和通用 lookup 绕过于生产路径之外
- Form: no-consumer
- Producer: `src/capability/backend.rs:53` - `CapabilityBackend::snapshot`（Tool 实现 `:358`，调用 `:321`）；`src/capability/provider_backend.rs:50,58` - 通用 `lookup`/`snapshot` 实现。
- Consumer: NONE FOUND: `rg -n 'CapabilityBackend::(lookup|snapshot)|\.adapter\.snapshot\(|provider_adapter\.(snapshot|lookup)\(|snapshot_capabilities\(|&dyn CapabilityBackend|Arc<dyn CapabilityBackend>|: CapabilityBackend' --glob '*.rs' .`
- Live caller search: 该命令及 `rg -n '\b(CapabilityBackend|ProviderBackendAdapter|ToolBackendAdapter)\b' --glob '*.rs' .`，再按接收者审查 `rg -n 'ProviderBackendAdapter|LegacyCapabilityHandlerRegistry|\.snapshot\(|\.lookup\(' --glob '*.rs' src/capability/{ambient,terminal,ui,backend,provider_backend,zahir_facade,special}.rs`。Tool snapshot 的直接调用只在 `src/capability/backend.rs:697`、`src/capability/zahir_facade.rs:2226` 等测试；通用 adapter 的这两个方法没有调用。生产 facade 用 `registry.snapshot_state()`（`:1946`）以及 `provider_adapter.entry`/`enumerate`（`:1531,1626,1929`）。Tool `lookup` 在 `:1311` 有生产消费者，不属于发现。
- Symptom: 修改这个 trait 快照方法并不会改变生产 `describe` 的一致性或范围筛选；修改通用 trait `lookup` 也不会改变当前的 generic metadata fallback。不是整个 backend 未挂载。
- Hypothesis: CUT - 若快照由生产 facade 负责组合，收敛无消费者的契约，防止测试只验证旁路；不要删除已用的 Tool lookup 或 Provider enumerate。
- Confidence: HIGH

## 非连线类静态缺陷

### CR-S01 - Provider 状态提交次序不等于通知次序（C04-2 未修）
- Form: other-static-defect
- Producer: `src/capability/provider.rs:309` 和 `:402` - 先释放 lifecycle 锁，之后发送通知。
- Consumer: `src/capability/provider.rs:408` - `subscribe_changes`；现仅测试订阅，未发现生产接收者。
- Live caller search: `rg -n '\bProviderTransition\b|\.subscribe_changes\(' --glob '*.rs' .`，以及 `rg -n 'with_ready_commit\(' --glob '*.rs' .`：发送端仍被 `src/tools/registry.rs:1417` 和 `src/gateway/channel_registry/binding.rs:1008` 调用，接收端结果同 CR-03。`review-results/2026-10-30-rust-doctor-canvas-capability-clarification.md:113` 已记为 open。
- Symptom: A 提交 `Starting→Ready` 后在发送前暂停；B 可提交 `Ready→Draining` 并先发送，随后 A 再发旧的 Ready。逐条折叠事件的观察者会看到状态逆序，甚至把同一 generation 的状态回退。当前没有可确认的生产影响；非恢复源并不等同于发送有序。
- Hypothesis: DECIDE - 若接入会折叠状态的订阅方，要绑定提交与发送次序，或明确要求订阅方只把这些消息当作重读提示。
- Confidence: HIGH

### CR-S02 - 通用 Backend 快照可把旧描述符配上新游标
- Form: other-static-defect
- Producer: `src/capability/provider_registry.rs:93` - `register` 在同一写锁下更改映射和 `revision`（`:113`）。
- Consumer: `src/capability/provider_backend.rs:58` - `snapshot` 先复制描述符（`:60`），后读 revision（`:61`）；`src/capability/provider_registry.rs:126,137` 分别取读锁。
- Live caller search: `rg -n 'CapabilityBackend::(lookup|snapshot)|\.adapter\.snapshot\(|provider_adapter\.(snapshot|lookup)\(|snapshot_capabilities\(|&dyn CapabilityBackend|Arc<dyn CapabilityBackend>|: CapabilityBackend' --glob '*.rs' .`，结合实际 adapter 接收者确认没有生产 consumer 调用通用 `snapshot`（见 CR-06）。注册有生产调用 `src/capability/zahir_facade.rs:398`。
- Symptom: 若在两次读之间注册新条目，返回的列表缺少该条目，却带有已经包含它的 `committed_cursor`。用该游标锚定增量并忽略之前变更的客户端可能永久漏掉该条目，直到重新全量读。不是当前生产 facade 快照已经出现此竞态。
- Hypothesis: DECIDE - 依 CR-06 的去留决策：若保留该契约，条目与 revision 必须从同一次读取派生。
- Confidence: HIGH

### CR-S03 - `activate` 可以成功返回尚不可调用的 lease
- Form: other-static-defect
- Producer: `src/capability/provider_registry.rs:42` - `ProviderRegistration::new` 缺省无 hook；`src/capability/provider.rs:114` - `ProviderLifecycle::new` 从 Cold 开始。
- Consumer: `src/capability/zahir_facade.rs:2082` - `activate` 只在有 hook 时调用它，随后读取 generation、登记 ownership、直接返回 lease（`:2091,2102`），不验证 Ready；相对地 `:1583` 的 `validate_provider_lease` 在 `:1594` 检查 `acquire_call`。
- Live caller search: `rg -n '(\.activate\s*\(|\bZahir::activate\b|\bProviderActivator\b)' --glob '*.rs' .`，以及 `rg -n 'validate_provider_lease|\.acquire_call\(' src/capability/zahir_facade.rs`：没有生产激活发起方（见 CR-05）；现行注册的 Memory hook 在 `src/memory/runtime/mod.rs:732` 拒绝非 Running。
- Symptom: 对未撤销、已有或可建匹配 ownership 的 Cold provider，若没有 hook，激活可以返回 `Ok(lease)`，同一 lease 随即被 `validate_lease` 以 `StaleProvider` 拒绝。这是当前 API 的潜伏成功语义错误，不是 Memory 生产运行时的现行故障。
- Hypothesis: CONNECT - 保留激活接口的话，把返回 lease 的成功判定接到实际调用准入条件；不能仅凭 hook 缺省或返回 `Ok` 断言可调用。
- Confidence: HIGH

## 交叉核验 / Cross-checks

- **注册 / 发现 / 分发：** 生产 `src/bin/aleph-server/commands/start/mod.rs:300,311` 装配 Tool、Memory 和独立异步 Special；通用 Memory provider 的 `entry`/`enumerate` 是活的（`src/capability/zahir_facade.rs:398,1531,1626`），不是无消费者注册表。`src/capability/zahir_facade.rs:1997` 按 special、Memory、其他非 Tool、canonical Tool 分流；`:1893` 的 metadata-only fallback 不发 lease。`src/capability/resolver_source_tests.rs:7` 验证 generic fallback 不冒充 canonical Tool。`capabilities.describe` 分类与注册同名（`src/gateway/method_census.rs:122`、`src/gateway/handlers/mod.rs:331`），handler `src/gateway/handlers/capabilities.rs:30` 实际读 facade。未确认生产 client ghost 或字面 key 漂移。
- **配置 / stub / 编译：** `rg -n 'ProviderRegistry|ProviderLifecycle|ResolverConfig|max_concurrent_loads|special_capabilit' --glob '*.rs' src/config src/capability/production_init.rs` 无命中；未发现已解析未读的 provider 配置。六个主文件中无未声明 feature 守卫、TODO 或 `unimplemented!()` 的生产 far-end。`VersionedRegistry::transact` 的 `Ok(None)`（`src/capability/versioned_registry.rs:219`）是调用者明确返回的不发布分支，不是吞掉已提交结果。
- **生产反证：** Tool/Hook 消费 `VersionedRegistry`，见 `src/tools/registry.rs:1163,1210,1315,2157`、`src/extension/hooks/registry.rs:449,490,513`；`from_snapshot_with_projection` 也由生产构造路径间接调用（`src/capability/versioned_registry.rs:115`）。`ProviderHandle::with_admitted_completion` 有生产调用 `src/gateway/channel_registry/binding.rs:559`，不因其仍带历史 `allow(dead_code)` 而报断线。`ProviderTransport::Bridge` 和 `ProviderLifecycle::transport()` 的独立调用只在 fixture 中发现；transport 是数据标签，不是承诺存在的 Bridge handler。
- **历史：** `review-results/severed-wire-2026-08-17/capability/REPORT.md` 是 `mod.rs`/`census.rs` 近邻切片，未发现可重报项。十月报告中的 C03-3 facets 和 C03-4 namespace 匹配仍有修正（`src/capability/backend.rs:303,396`）；C04-1 poison 恢复、C04-4 ID trim、C04-5 同表重入守卫、C04-6 代际溢出检查仍在（`src/capability/provider.rs:24,188,281`、`src/capability/versioned_registry.rs:206`）。C04-3 的 Tool canonical commit 仍在 `src/tools/registry.rs:1417`；Memory 的局部状态推进（`src/memory/runtime/mod.rs:538`）不等于绕过 Tool 发布。仅 C04-2 在本次作为已复核未修项重报。
- **排除项：** `AGENTS.md` 明示延期的 EffectClaim producer integration、automatic Safe Replay、external-effect exactly-once、universal durable scheduler、complete ACP server gate、其余 CapabilityKind backends 未作为发现上报。

## 未覆盖的接缝

- 未运行编译、单测、并发调度或真实 RPC/启动环境；CR-S01 至 CR-S03 是源码级生命周期/交错推演，不宣称已复现线上故障。
- 不审查仓外 Rust crate 调用、部署生成模块、运行时插件内容、各 transport 的完整 route/订阅表。Gateway 只核到本切片涉及的注册、分类、facade 调用及其返回筛选；未穷举 Gateway/Panel 全部代理调用。
- 未断定通用 metadata 的跨 scope 投影违反产品 ACL：`src/capability/provider_backend.rs:35` 将描述符的 visibility 标成请求值，而实际 lease 在 `src/capability/zahir_facade.rs:1535` 核对绑定；现有测试允许 cold metadata 可见，已核到的 Gateway 使用默认 scope 并筛到 special。要定性需一个真实的非默认 scope 消费者，以及明确的 metadata visibility 合同或覆盖不同 envelope 的契约测试。此项为 UNCERTAIN，未计入候选。
