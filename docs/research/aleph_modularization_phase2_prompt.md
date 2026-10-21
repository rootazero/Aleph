# Aleph 模块化第二阶段续跑 Prompt / Phase 2 Resume Prompt

> 将下面“可复制 Prompt / Copyable Prompt”标题之后的全部正文直接复制到新会话。
> 本文件是下一实施会话的任务契约，不是当前已实现报告；配套详细计划：`docs/research/aleph_modularization_phase2_plan.md`。

## 可复制 Prompt / Copyable Prompt

你是 Aleph 模块化第二阶段的主 Agent / Lead Orchestrator。请先做 plan review，再默认持续推进至本工作包验收完成或真实阻塞；不要直接从写 Rust 开始，不要把未跑测试说成通过，不要自动 push。

### 1. 目标、目录与已提交基线 / Goal, Directory, Committed Baseline

- 工作目录明确指定为 `/Volumes/TBU4/Workspace/Aleph`，共享工作区。先验证 `pwd`、`git rev-parse --show-toplevel`、HEAD、status，不静默跳转其他卷或建立另一份检出。项目旧路径说明若与本请求冲突，以本次指定工作目录为准，并报告实际 root。
- 第一阶段已完成且已提交：first-phase commit = `0cc141a0d`，完整 SHA `0cc141a0d5ed4e81a18ffcaa2cb6d1c9c336d0b5`；文档编写时 HEAD 同为该提交。不能写成“第一阶段还有未提交变更”。
- 第一阶段已经有 provider lifecycle fencing、generation、调用点 admission；但生产 transition 仍 dormant。不要重复实现第一阶段，也不要把测试里的 Starting 迁移说成生产启动已接线。
- 若当前 HEAD 已前进，先核实该基线仍在历史、查看相关差异，再做 plan review；不要 checkout/reset 到旧提交覆盖别人的工作。基线漂移改变关键契约时暂停说明。
- 详细计划：`docs/research/aleph_modularization_phase2_plan.md`。总方向背景：`docs/research/aleph_modularization_plan.md`。本次目标不是泛化插件，而是分离“声明发现 / declaration discovery”和“运行时启动 / runtime activation”，先做受控 WASM runtime-owner lazy activation。

**两个独立验收单元 / Two separate acceptance units**：

1. **A（本会话默认实施）**：受控 WASM 的 discovery/mount 拆分、Cold 状态、串行控制事务、真实调用请求 owner activation、单飞、失败回滚、安全停止/reload/shutdown、身份与 generation fencing、管理状态投影与真实通知、真实 WASM/barrier 测试。
2. **B（不自动实施）**：canonical plugin producer + per-provider entry fencing + long-lived projection。A 完成并冻结契约后，B 才进入独立工作包与审批 gate。

A 的 E2E 若当前 canonical producer 未接线，只能证明 runtime activation owner → 实际 WASM 执行/清理，**不能声称完整插件工具 callable 接入**。如果我要求模型经 canonical tool surface 完成真实插件调用，B 就是该验收的前置依赖；先指出并确认范围变化，不要用 A 的局部测试冒充完成。

### 2. 先读与实际接线核查 / Read First and Verify Wiring

按需读完整相关文档，不重复扫描已确认的整仓信息。先读：

- 仓库 `AGENTS.md`、本计划及本 prompt。
- `docs/reference/REDLINES.md`、`docs/reference/HARNESS_PHILOSOPHY.md`、`docs/reference/SUBSYSTEM_ROUTING.md`、`docs/reference/DEVELOPMENT.md`、`docs/reference/STYLE.md`。
- `docs/reference/PLUGIN_SYSTEM.md`、`docs/reference/TOOL_SYSTEM.md`、`docs/reference/SECURITY.md`，以及 `docs/reference/FEATURE_LOCATOR.md` §3.5d 和改动相关的附录 C/E 判据。
- 如需修改 `src/gateway/`，先读其局部 `CLAUDE.md` 与 routing 指向的 reference。不要改 `src/harness/`；R10 不可绕过。

再核查以下准确路径/符号；以实际代码为准，发现与计划不同要先报告：

| 路径 / Path | 必看符号与问题 / Symbols and questions |
|---|---|
| `src/capability/provider.rs` | `ProviderLifecycle`、`ProviderHandle`、`ProviderState`、`transition`、`acquire_call`；Starting 唯一递增 generation，overflow fail-closed；当前没有 Cold |
| `src/tools/registry.rs` | `ToolHandlerRegistry::new/new_with_provider/provider`、`ProviderCallHandler`、`RegistryEntry::for_provider`、`resolve_entry`、`snapshot_state`、`reconcile_plugin_tools`；全局 `aleph/tools` 是否仍恒 Ready；producer 未挂载错误是否仍在 |
| `src/capability/{backend,facade,zahir_facade}.rs` | `BackendLease`、`resolve`、`validate_lease`、`subscribe`；provider generation、owner generation、descriptor revision 是不同维度；查询不能触发启动 |
| `src/extension/plugin_state.rs` | `PluginsConfig::activation/is_enabled_for/set_enabled/save`，origin 默认与 NotEnabled，atomic write |
| `src/extension/mod.rs` | `ExtensionManager`、`ensure_loaded`、`set_tool_handler_registry`、`sync_runtime_snapshots`、`plugin_tool_revision`、`active_plugin_tools_snapshot`；canonical Arc attachment 不等于 producer |
| `src/extension/lifecycle.rs` | `load_all_locked`、`discover_and_mount`、`scan_plugins_blocking`、`build_record`、`admit`、`mount_inner/mount_parsed/fail_mount`、`unmount_inner/unmount_all`、`reload_plugin/reload`、`watch_server_starts`、`Views::after_transition` |
| `src/extension/plugin_ops.rs` | `call_plugin_tool`、`execute_plugin_hook`、`ensure_runtime_mounted`、`set_plugin_enabled`；当前只调用已 mount VM，toggle bool 吞保存/runtime 失败 |
| `src/extension/loader.rs` | `load_wasm_effect`、`PluginLoader::call_tool/call_bound/unload_wasm_mount/shutdown`；真实 identity capture 与 disposer 比对 |
| `src/extension/runtime/wasm/mod.rs` | `WasmMountIdentity`、`WasmBinding`、`mounted_instance/remove_if_current/binding_is_current/call_bound`；Arc 存活与执行许可分开 |
| `src/extension/effects/{scope,disposer}.rs` | `EffectScope`、`STEP_LABELS`、`DisposeReport`；六步逆序清理、失败继续汇总 |
| `src/extension/{projection,readiness}.rs`、`src/extension/types/plugins.rs` | `is_active()` 当前 Loaded/Pending；write_readiness 只返回 activity flip；Cold 不能复用 Pending |
| `src/gateway/handlers/plugins/handlers/manage.rs` | `set_enabled_via_registry`：不能继续忽略 manager 结果返回 `{ok:true}` |
| `src/gateway/handlers/extensions/lifecycle.rs` | `handle_toggle` plugin 分支：Hub `extensions.toggle` 不得吞失败；MCP/skill 分支不扩展 lazy |
| `src/builtin_tools/plugin_manage.rs` | Enable/Disable bool 文案；需要结构化 desired/actual/persistence 结果 |
| `src/bin/aleph-server/commands/plugins.rs` | CLI 等真实控制入口；避免漏掉同一动词的另一张脸 |
| `shared/protocol/src/plugins.rs` | 状态 wire 契约与消费者，不允许 Cold→Loaded 映射伪装 |
| `src/extension/runtime/wasm/tests/guest_fixture.rs`、`src/extension/loader.rs` 测试、`tests/modularization_contracts.rs`、`tests/modularization_import_guards.rs`、`tests/fixtures/modularization/` | 复用真实 guest/identity/import guard，不重造测试宇宙 |
| `qa/browser_live/check_memory.py` | macOS 实际 page size 与 available 估算、4 GiB 门、自测 |

不要按计划中的符号名字硬猜新 API。计划里的 `request_runtime_activation(...)` 是拟议名称，当前不存在；P0 后选择与现有 owner 结构一致的最小接口。

### 3. Plan Review 必须先交付 / Mandatory Plan Review Before Editing

由主 agent 提交简明 review，再下发写任务：

1. 真实基线、共享未提交文件及其归属；本会话 writer 文件边界。
2. discovery → mount → VM → call → dispose 的当前调用图；catalog/diagnostic/`ensure_loaded` 是否有隐式启动；producer 缺口。
3. A eligibility 推荐：先做显式受控 tool-only WASM，无 startup service、hook、context、memory、MCP 初始化依赖。资格从 manifest 推导。mixed 插件保持既有 eager 兼容并明确原因；不要默默改成 lazy，也不要把该兼容范围说成全量 lazy。
4. 冻结 desired/actual 状态与 Cold 表达、instance/mount identity/generation 契约；推荐在纯 `ProviderState` 中增加 Cold，再准确投影 PluginStatus/wire，不另存可独立写的状态副本。
5. activation 请求入口、授权顺序、单飞所有权、caller 取消与 owner operation 收尾、mount 完成时点。
6. config candidate save→live publish→runtime reconcile 的线性化点、锁顺序和结构化结果 schema；runtime failure 不能撤销已保存意图假装全原子。
7. 停止期间的 in-flight lifetime 策略：复用 owned loader guard/Arc，等待安全边界后卸载；若不成立提出 permit 或明确不 unload 的替代，并说明代价。
8. 哪些 existing projections 在 A 更新、哪些 canonical long-lived projections 属 B；真正通知如何到达在线 observer，不能靠虚增 revision。
9. 每个关键选择写“为什么 / Why”和“这会让后续什么更难 / Trade-off”；只对重大歧义提问，不为普通实施反复索要继续授权。

发现计划错误要修正计划而不是机械执行；若修正需要 B、权限扩大、混合插件启动语义改变、强制卸载或新安全模型，先停下问我。

### 4. 单一权威与不可破坏契约 / Single Authorities and Hard Contracts

- **Desired-state authority**：`PluginsConfig` / `plugin_state.rs` 的 durable enabled 是唯一用户意图权威，origin 默认和显式 false 不混同；trust 是独立 admission，不被 enable 自动授予。
- **Runtime side-effect owner**：`ExtensionManager` / `lifecycle.rs` 拥有启动、停止、scope、operation 单飞协调。loader 不可因 missing VM 自作主张启动；catalog/diagnostic/projection 不得启动 provider。
- **Instance state + generation authority**：`ProviderHandle` / `provider.rs`，纯契约，不启动线程/进程/transport。manager 引用同一个 handle，不复制一个可独立写的 actual state。
- **Callable authority**：`ToolHandlerRegistry` canonical descriptor/handler 唯一权威。`PluginRegistry` 声明、active tool index、catalog、diagnostic 只是投影，不可变成第二 callable registry。
- 当前 registry 只有一个全局 `aleph/tools` provider，不能切换成某插件状态，否则会关闭整个工具面。A 的 per-instance runtime handle 不得赋给这个全局 registry。
- `reconcile_plugin_tools()` 当前返回 `plugin producer reconciliation is not mounted`，必须诚实保留；metadata/catalog 注册不等于 callable producer。B 未做不得说完整工具接线完成。
- `describe/resolve` 保持查询/lease 语义，不偷偷变异步启动入口；Cold 不能签发可执行 lease，不改成 query 触发 VM。
- 明确区分 `discovered / disabled / cold / starting / ready / stopping / failed / stopped`。现有 Pending 代表已 mounted 等 MCP dependency，且 is_active 为 true，不可表示冷态；Loaded 更不能表示未创建 VM。
- generation 只在 Starting 分配，checked overflow fail-closed；mount 全部必要效果完成、scope/identity 装妥、operation 仍 current 才 Ready；失败逆序 rollback scope，初始结果 Failed，不报 Stopped。
- 异步 completion、watcher、disposer 必须绑定 plugin instance + mount identity + generation；旧结果不能复活或删掉同 id 新实例。不要只比较 plugin id/名字/revision。
- `acquire_call` 是 admission，不是 drain/cancel/unload 许可。若 stop 释放模块/transport，须证明 in-flight permit、引用/锁存活或明确不 unload 策略；仅拒绝新调用不够。
- 已准入 guest 可按契约结束；`spawn_blocking` 一旦开始不因 caller future drop 而已停止。stop 先关闭新准入，后过安全边界，再 dispose；cleanup failure 保留 actual Failed/残余信息。
- durable toggle、live 发布、runtime reconcile 串行。clone candidate，先 `candidate.save()` 成功才替换 live；保存失败不得启动/停止或返回成功。保存成功后 runtime 失败保留 desired，向外给结构化 partial outcome。
- 结果至少表达 plugin id、desired、persisted、preference_changed、actual state、generation/operation identity、runtime outcome、error phase、cleanup report；NoChange 不等于 Failed。RPC/Hub/builtin/实际 CLI 调用者共享同一推导，不能吞错误。
- `Views::after_transition` 保持唯一 projection 发布入口或明确唯一委托；readiness 变化要有实际通知/重建，不能只依赖 `write_readiness` 的 is_active flip。
- 不伪造 canonical registry revision。provider generation、owner generation、registry revision、metadata revision 各有权威；broadcast 是 live notification，不是 durable recovery source。lag/reconnect 从当前权威 snapshot 重建；不新增假 replay，durable recovery 只认 `SessionEventStore`。

### 5. 实施 DAG、边界与分工 / DAG, Boundaries, Delegation

建立任务追踪，在做每项前标 in_progress，验收后再 completed：

```text
P0 plan review + baseline/writer census + contract freeze
  → P1 serialized control transaction + structured callers
  → P2 A discovery split + explicit owner activation + single-flight
  → P3 A stop/reload/shutdown + cancellation/lifetime/identity fencing
  → P4 A management projections + real notifications + effect tests
  → GA A acceptance + contract freeze
  → P6 documentation + final report（默认结束）

GA → GB independent approval
   → P5 B canonical producer + per-entry provider fencing
        + long-lived projection → B acceptance → P6
```

主 agent 负责分析、架构取舍、契约冻结、任务编排、实际 diff 审查和最终验收；实现/测试执行由 subagent 完成。按环境实际可用工具与模型调度，不虚构已委派。若确无 subagent，报告限制并先确认执行方式，不冒充委派。

每个 TaskSpec 必须含目标、可改/只读路径、接口契约、禁止事项、依赖、交付物、验收命令、执行模型及原因。普通局部实现优先能力足够的低成本模型；DeepSeek 仅 `deepseek-flash`，MiniMax 仅 Flash；失败携带明确证据修复一次，无进展升级，不无意义重复。

允许路径上限（并非全部都要改）：

- P1：`src/extension/{plugin_state,plugin_ops,lifecycle,mod}.rs`；三类薄 caller（plugin RPC、extensions Hub route、builtin plugin_manage），以及真实 CLI/test 消费面。
- P2/P3：`src/extension/{lifecycle,plugin_ops,mod,loader}.rs`、`src/extension/runtime/wasm/`、`src/extension/effects/`、`src/extension/types/plugins.rs`、纯 `src/capability/provider.rs`，对应真实 fixtures/tests。优先复用 existing mount identity/guard，不重造 supervisor。
- P4：`src/extension/{projection,readiness,lifecycle,mod}.rs`、`shared/protocol/src/plugins.rs`，先列清需要同步的最小管理状态 consumer/test。A 不扩展成所有 canonical long-lived adapter 改造。
- P5：默认不准改 canonical producer 成功接线；独立批准后才允许 `src/tools/registry.rs`、相关 capability backend/facade、tool_metadata/registry 与真实 tool-refresh/MCP consumer 的协调修改。
- P6：本计划、prompt、总路线和相关 reference 的事实更新，不篡改第一阶段历史。改 `AGENTS.md` 时同步 `CLAUDE.md`，优先无需修改入口文件。

双线并行只能在接口稳定后进行，例如 owner core 与薄 caller 适配/独立测试文件。**两条线不能同时改同一个文件**，即使改不同函数也要一个 writer；P2/P3 耦合强，优先顺序。共享工作区出现他人变更，不回滚、不覆盖、不 `git add -A`。cargo 验证集中给一个 worker 串行，避免共享 target 和内存竞争。

### 6. 明确调用协议与停止协议 / Invocation and Stop Protocols

A 的拟议调用路径应能被实际代码画出来：

```text
既有合法调用者/可信 owner test
  → 既有 permission / visibility / trust / desired gates
  → ExtensionManager::call_plugin_tool
  → owner request_runtime_activation（拟议接口，P0 冻结）
  → cold→starting 单飞，或 join 同一实例 operation
  → load_wasm_effect / mount scope 完成
  → 检查 current operation + instance + generation + desired
  → ready
  → bind actual mounted instance + invocation-time revalidation
  → spawn_blocking guest call（资源存活）
  → 返回真实 guest output
```

这是 runtime owner 路径，不能当作 B 的 canonical plugin tool 生产闭环。不得为 A 新增绕过 ToolService 审批的公共工具入口；先审计 `call_plugin_tool` 所有真实 callers。若 canonical 无 producer 而模型无法发现 cold 工具，直接报告这个边界，不用 catalog 偷启动来“修好”。

停止协议：先提交控制意图，失效旧 activation token、封闭新准入；旧调用按存活策略完成；scope 仅清理自己的实例；成功才 Stopped，失败 Failed + cleanup detail。disable 不是 reload/shutdown：后两者不擅自把 durable enabled 改 false；eligible WASM reload 推荐回 Cold，下一真实需要才 Starting。

control/load_guard 与 loader/registry 锁顺序先画后写；不要持有 owner lock 等一个需要该锁才能结束的 watcher，也不要持有 loader guard 再等待 stop 持有的 owner lock。std mutex 不跨 await。Starting 的 owner operation 不应绑定首个 caller 的 future 生死；caller 取消后仍必须收尾、清理、唤醒其他 waiter。

### 7. 必须有的测试与效果断言 / Required Tests and Effect Assertions

优先复用 `src/extension/runtime/wasm/tests/guest_fixture.rs` 的真实 WASM；`EMPTY_WASM_MODULE` 只证明 load/unload，不足以验证真实调用输出。barrier/Notify/oneshot/受控 blocking gate 精确控制交错，timeout 只做死锁上界，禁止 sleep 排序和随机“多跑几次就算过”。

至少覆盖：

1. boot + 反复 list/catalog/diagnostic/describe/resolve：eligible WASM 实际 VM 未创建、generation 不动、没有 runtime effect 泄漏；读取永不 Starting。
2. disabled/NotEnabled/Blocked：全部启动入口拒绝，零 VM 创建，原因准确。
3. 首个获准真实调用：owner 驱动 production transition，mount 完成才 Ready，guest 返回确定 payload；不是 mock success。
4. 并发首调：卡在 load 边界，多个 waiter 只一个实例/一个 generation，最后所有等待者得共享结果。
5. 原子保存失败：disk/live/runtime 不变；RPC、Hub `extensions.toggle`、builtin 都传播错误，无假 ok。
6. 保存成功但启动失败：desired 已持久化，actual Failed，结构化 partial outcome；scope 效果确实消失。
7. 双 toggle、不同插件共写一个 config：无丢失更新/越序，最终磁盘与 live/reconcile 一致。
8. Starting 时 disable/reload/替换：迟到 completion 不 Ready，旧 disposer 不删新 VM，新 binding 输出正确。
9. caller/leader 等待取消、operation panic/join failure：其他 waiter 不饿死，不永远 Starting，不重复启动，不泄漏 effect。
10. 真实 guest in flight 时 stop/shutdown：新调用拒绝，旧执行资源仍有效，实际卸载不早于安全边界；drop `spawn_blocking` waiter 不等于 job 停止。
11. disposer error/panic：后续逆序清理继续，残余可见，actual 不伪报 Stopped。
12. 旧 generation/old handler/old binding 复用被拒；显式 retry 新 generation；overflow fail-closed。
13. 先创建 observer 再变更：能真实看到 cold/starting/ready/stopping/failed；lag 后从 snapshot 重建，不伪造 registry revision；读侧无 activation。
14. mixed WASM/MCP/static 回归：Pending 已 mount 的原语义不被 Cold 取代，未纳入 A 的插件保持原行为。
15. B 未实施时，保留 producer-not-mounted 的 fail-closed 测试；不得把 metadata 注册当 callable PASS。

B 将来另验：真实 WASM→canonical handler→既有 ToolService 权限/调用链；per-entry fencing；停一个插件其他插件/builtin 继续；长寿命 facade/catalog/MCP observer 在不重建测试对象的情况下收到更新；同名冲突/旧 disposer/registry closed 不造成部分提交。

重点断言实际 mounted identity、guest payload、scope 撤回、在线投影更新和其他插件仍工作。mock 计数与 source census 仅辅助，不能替代效果断言。使用隔离 `ALEPH_HOME` 与项目已有全局 manager test guard，不接触用户真实配置/VM。

### 8. 资源检查与验证纪律 / Resource Checks and Verification Discipline

**每次 cargo 前都检查 macOS 内存**；不能一天只看一次，不能假定 page size 为 4096 或 16384。

1. 用 `/usr/bin/vm_stat` 取得当前快照，读取首行实际 page size。
2. 优先复用 `qa/browser_live/check_memory.py`（按仓库 Python/uv/.venv 约定选解释器）。其 macOS available estimate 是 `(Pages free + Pages speculative + Pages inactive) * actual_page_size`；这是估算不是立即分配保证，不叠加 wired/compressed。
3. 先跑 `python qa/browser_live/check_memory.py --self-test`，每个 cargo 命令前再跑 `python qa/browser_live/check_memory.py`。exit 0 才允许，1 阻止，2 探测失败也阻止。
4. **低于 4 GiB = 4294967296 bytes 时等待/不启动 cargo**。记录时间、page size、计数/字节数、正在运行的 build；重链接建议 10 GiB 余量。探测失败也 fail-closed。
5. 资源不足先推进 review/文档，在合理下一检查点复测；不要无限 sleep/poll，不擅自 kill 他人进程。持续不足就报告 BLOCKED/UNRUN，不能标测试失败或成功。
6. 共享 target 的 cargo 不并发；长编译使用当前工具支持的后台执行/完成通知，不占工具短超时后误报失败。

候选验证命令（每一条 cargo 都先过资源门；实际选择按 diff 范围，记录 exit 与有效测试数）：

```bash
cargo fmt --all --check
cargo test -p alephcore --lib capability::provider::
cargo test -p alephcore --lib extension::loader::
cargo test -p alephcore --lib extension::lifecycle::
cargo test -p alephcore --lib extension::plugin_ops::
cargo test -p alephcore --lib tools::registry::
cargo test -p alephcore --lib capability::zahir_facade::
cargo test -p alephcore --lib
cargo check -p alephcore -p aleph-slot-api --all-targets
```

filter 0 tests 不是通过；先核实模块/名字，补跑实际新增的 fixture/barrier 测试。按 `docs/reference/DEVELOPMENT.md` 的最小可信验证集补充测试编译和相关 targets，不用 cargo check 替代 `#[cfg(test)]`：

```bash
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
just test-shared
# If relevant desktop limbs changed:
cargo check -p aleph-desktop-macos -p aleph-desktop-windows -p aleph-desktop-linux
just _stage-shell-placeholders
cargo clippy --workspace --all-targets
```

wire/shared 改动需要 `just test-shared`；`interfaces/webchat/` 有任何改动需跑 `cargo test -p aleph-panel --lib`，Panel 出厂形态用 `just wasm`。缺真实 fixture/toolchain 时记录阻塞，不能换 mock 然后宣称通过。是否跑完整 workspace/跨平台验证按风险决定，并明确所有跳过项。

最后 `git diff --check`、`git diff --stat`、`git status --short`；新文件还未跟踪时 ordinary diff 不会包含它们，要另做显式内容/空白检查和统计，不为获取 stat 擅自 stage 他人文件。主 agent 验收实际 diff、日志/exit code 和测试数量，不把 subagent 的“完成”摘要当事实证明。

### 9. 禁止事项与停止条件 / Prohibitions and Stop Conditions

禁止：

- 不做 MCP lazy、Gateway/Terminal/Memory 统一 backend、sidecar supervisor、Rust dylib/热卸载、Safe Replay/EffectClaim producer、scheduler、跨 Capability kinds。
- 不扩大 `src/harness/`，不让确定性中间件替 LLM 选择恢复策略，不加重依赖/DI/通用插件框架来解决局部问题。
- 不引入第二 callable registry/第二 durable enabled/第二可独立写 actual state；不把全局 `aleph/tools` 绑定单插件；不伪造 revision。
- 不以 Pending/Loaded/is_active 伪装 Cold；不把 broadcast 当 durable recovery；不把 state admission 当 drain；不在旧 generation 上重新授权。
- 不在读侧或 `describe/resolve` 暗启动；不绕过 permission、visibility、trust、sandbox、confirmation、secret 边界。
- 不把 metadata/catalog 注册或 canonical Arc attachment 报成 producer 完成；不在 A 未验收时顺带做 B。
- 不覆盖共享变更、不 reset --hard、不 force push、不擅自删除 worktree/branch、不自动 push；提交也仅在用户当前明确授权时做。

必须停止并给出证据/所需输入：

- 用户验收实际要求完整 callable，A 不够且 B 未获独立授权。
- 必须改变 mixed 插件启动契约、权限/trust 模型、wire 兼容策略或实施强制 unload 才能继续。
- 无法证明在途资源存活；只能靠关闭全局 registry 或伪造状态/revision 才能让测试绿。
- 同文件有无法协调的共享 writer、基线漂移破坏冻结契约。
- 内存持续不足 4 GiB、探针失败、工具链/真实 fixture 不可用、合法实现/验证路径耗尽。
- 实现连续失败无实质进展：最多三次有效尝试是上限不是必须消耗次数；及时升级根因分析，不做模型之间循环重试。

停止报告要含 evidence gathered、attempted paths、实际 blocker、保留的进度、下一输入/资源要求。不要自动扩大范围，不把 BLOCKED 写成完成。

### 10. 最终报告格式 / Final Report Format

用中文简洁报告，路径与关键 identifier 准确：

1. **交付状态 / Status**：A completed/partial/blocked；B deferred/approved separately/implemented 分开写。
2. **完成内容 / Changes**：实际文件、核心调用图与用户可见行为，不写愿景式完成。
3. **关键决策 / Decisions and trade-offs**：为什么这样做、后续更难的是什么；尤其 Cold、activation 入口、事务边界与 in-flight lifetime。
4. **验证 / Evidence**：逐条命令、exit code、有效测试数、真实 WASM/barrier 效果、资源门 page size/字节数、日志路径；PASS/FAIL/BLOCKED/UNRUN 分开。
5. **残余风险 / Risks**：producer 缺口、mixed WASM、wire/平台/内存/物理释放不保证等，按实际列。
6. **明确未做 / Not done**：B（若未实施）、MCP lazy、各 backend 泛化、sidecar、dylib、Safe Replay、scheduler、跨 kinds、跳过的测试、未提交/未 push；不漏负面清单。
7. **续跑入口 / Next action**：若 A 已完成，给出进入 B 所需的冻结契约与独立 gate；若 blocked，给出最小解阻输入，不要求我重复发送“继续”来完成原已授权范围。

默认做到 A 的诚实验收与文档更新即结束，不自动开始 B，不自动 push。
