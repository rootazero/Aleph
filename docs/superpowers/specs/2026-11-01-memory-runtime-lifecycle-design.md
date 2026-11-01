# Memory runtime stop/re-enable with live session archival / 保留会话归档的 Memory 运行时停止与重启

**Status / 状态: Draft for design approval / 待设计批准；未实施。**

This document records the user's selected direction on 2026-11-01. It does not claim implementation or authorize an unreviewed architecture. / 本文记录用户已选择的方向，不代表功能已经实现；生产实施须在本设计批准及实施计划完成后进行。

## Intent / 意图

Memory must be independently startable and stoppable while ordinary chat and durable raw session archival continue. Disabled Memory must reject new Memory operations, drain admitted work, release its own resources, and permit later re-enable. Old Memory references must remain invalid after re-enable. Early startup failures must use the same explicit cleanup ownership as normal shutdown. / Memory 必须可独立启停：关闭时聊天与 raw 会话归档继续，新 Memory 请求明确拒绝，已接纳任务排空，专属资源释放；重新启用后旧引用仍失效。启动中途失败同样进入有所有权的清理边界。

User decisions / 用户决定:

- Preserve chat and raw session archival while Memory is off. / 关闭 Memory 时保留聊天与原始会话归档。
- Reject new Memory work; drain already admitted work. / 拒绝新 Memory 工作，等待已接纳工作完成。
- Include early-startup-error cleanup and resource close/reopen. / 包含启动中途失败清理、资源关闭与重开。

Builder proposals requiring approval / 本设计提出、待批准的选择:

- Stop completion has no hard time bound. A caller cancellation stops waiting, not cleanup ownership. / stop 不承诺硬期限；取消调用方等待不丢失清理所有权。
- Runtime off/on is initially process-local and does not edit the boot configuration. Restart uses the persisted startup configuration. / 首版运行时启停仅影响当前进程，不改写启动配置；进程重启采用持久配置。
- Stop is successful only after all Memory work is observed and Memory-owned database connections are actually closed. Partial failure remains an explicit failed/draining state, never a successful off state. / 仅当全部 Memory 工作已观察且专属数据库连接真正关闭，stop 才成功；部分失败保留明确状态，不伪报关闭成功。

## Approaches / 方案比较

1. **Only early-startup cleanup / 仅启动失败清理**: retain startup resources and unwind them on errors. Smallest change, but cannot stop Memory during chat and leaves long-lived workers running. / 改动最小，但不能实现聊天中的 Memory 启停。
2. **Memory off/on, raw archival alive / Memory 启停、raw 归档保留 — selected / 已选择**: separate Memory-owned resources from raw archival; route consumers through one owned lifecycle boundary. Enables independent lifecycle, but requires migration of existing direct `Arc` captures and detached workers. / 满足独立生命周期，代价是必须迁移当前直接捕获的 Arc 与 detached workers。
3. **Close all persistence, including raw sessions / 关闭全部持久化**: simpler final shutdown boundary, but would suspend archival and require chat/session admission changes. Rejected by the user's archival requirement. / 会中断归档，并引入会话停机语义，与用户选择不符。

This is a cross-module project, not an extension of the two startup handles alone. / 这是跨模块项目，不能仅扩展两个启动任务句柄来完成。

## Current source facts / 当前源码事实

- `src/bin/aleph-server/commands/start/mod.rs`: creates `SqliteMemoryBackend` and `MarkdownSessionStore` early; orientation begins before subsequent fallible registration/backup setup; the current `NoteMaintenanceOwner` is assembled later. The post-server funnel drains watcher then maintenance, preserving `run_result`.
- `src/memory/store/sqlite/mod.rs`: `SqliteMemoryBackend` owns an `Arc<Mutex<Connection>>`; blocking closures clone it. There is no existing explicit close/reopen contract, and many methods directly lock the connection.
- `src/memory/store/markdown/store_impl.rs`: raw archival uses its own connection to the same `metadata.db`. Closing the Memory connection cannot imply closing the database file or deleting its WAL while raw archival remains live.
- `src/bin/aleph-server/commands/start/builder/handlers/memory.rs`, `src/memory/compression/service.rs`: compression has periodic, threshold, session-end and RPC entry points; retaining only its periodic loop is insufficient.
- `src/memory/dreaming/mod.rs`, `src/memory/extensions/scheduler.rs`, `src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs`: Dream and producer work contain discarded task handles.
- `src/gateway/server/mod.rs`: reconciler handle exists; `abort_reconciler_daemon` is not a joined production teardown contract.
- `src/gateway/handlers/memory/mod.rs`, `src/memory/reembed.rs`, `src/memory/embedding_manager.rs`: re-embedding migration/progress and retained provider clones extend lifetimes past manager replacement.
- `src/gateway/session_manager/ops/emit.rs`: raw archival and Memory-derived work share a session-end entry point; they must be separated without dropping raw writes.
- `src/capability/mod.rs`: existing `CapabilitySlot<T>` is install-once and cannot implement re-enable by itself.
- `src/capability/ownership.rs`: `revoke`/`dispose` are irreversible; scope-wide `bump(Runtime)` affects unrelated bindings; identical `register` preserves generation. None is a valid isolated Memory off/on mechanism as-is.
- `src/capability/projection_host.rs`: the production tool projection host already constructs `ZahirFacade`. Memory is not therefore already a mounted Resource backend. The old no-production-constructor statement in `AGENTS.md`/`CLAUDE.md` needs a synchronized factual correction when implementation lands.

## Ownership and Capability contract / 所有权与 Capability 契约

Introduce a concrete `MemoryRuntime` in the Memory subsystem, injected by startup. It owns only Memory state/resources and implements the existing Capability vocabulary; no generic supervisor, second global registry, new Capability kind, or harness policy. / 在 Memory 子系统引入具体 `MemoryRuntime`，由启动层注入；只负责 Memory 生命周期，复用 Capability 词汇，不增加通用 supervisor、第二全局 registry、新 kind 或 harness 策略。

The runtime is the sole source of lifecycle state, admission counts and retained Memory tasks. Existing `OwnershipTree` remains the sole generation/lease authority. The existing tool registry remains the source of tool descriptors and handlers. / runtime 是状态、准入计数、任务句柄的唯一事实源；OwnershipTree 仍是 generation/lease 唯一权威；工具 descriptor/handler 仍以现有 registry 为准。

A narrow Capability ownership extension is required: rotate only the Memory Resource capability's existing visibility bindings under the tree mutex and notify observers, without revoking its ID permanently or restamping unrelated capability IDs. Each lease still compares its exact `(CapabilityId, VisibilityScope)` binding. The operation covers every existing visibility binding for this Resource ID, not just the default scope; later visibility bindings inherit the new tree epoch. It must preserve current register idempotence and fail closed for revoked/disposed bindings and generation exhaustion. / 需要窄化扩展：在树锁内只轮换 Memory Resource 这个 ID 的全部现存 visibility binding 并通知 observer，不永久 revoke，也不改无关 ID。每个 lease 仍校验精确的 ID/scope；不能只轮换默认 scope 而遗漏其他主体，之后的新 binding 使用新 epoch。保持 register 幂等和现有 fail-closed 约束。

Mount Memory as a concrete Resource lifecycle backend at the existing host composition boundary. Do not convert the Tool-only adapter into a second descriptor store or pretend the Resource backend already exists. Discoverable status/start/stop operations use the existing tool/Capability control path; CLI/Panel remain I/O projections. / 在现有 host 组合边界挂载具体 Resource 生命周期 backend，不另存工具 descriptor，不假定 Resource backend 已存在。状态、启动、停止通过现有工具与 Capability 控制路径暴露，CLI/Panel 只做 I/O。

Normal Memory operations acquire a source-local admission permit with the current lease in one critical section. Lock order: Memory source gate, OwnershipTree, then entry-local state. No I/O or await under those locks. Stop closes admission and rotates the binding while holding the source gate. / 普通操作在同一临界区验证 lease 并取得准入 permit；锁序为 Memory source gate → OwnershipTree → entry-local，不持锁 I/O/await。stop 持 source gate 关闭准入并轮换 binding。

Lifecycle control must remain callable while data operations are disabled. Keep its control binding outside the disabled data binding. All Memory-dependent Tool descriptors declare a canonical `Service { relation: Requires, target: <Memory Resource id> }` dependency in their existing registry-owned descriptor; `to_descriptor` preserves it. The control/status tools have no such dependency. No manually maintained tool-name list or second dependency registry is allowed. / 数据关闭时控制入口仍可调用；控制 binding 与数据 binding 分离。Memory 数据工具在现有 registry 拥有的 descriptor 中声明统一 Resource 的 Requires 依赖，转换时保留；控制工具不依赖该数据 Resource，不另存工具名单或依赖 registry。

**Resolution of builder question 1 — proposed architecture / builder 问题 1 的架构提案:** a resolved Memory Tool lease captures both its own canonical Tool lease and the required Memory Resource lease at resolution time. Validation and admission check both captures; they must never refresh the captured Memory generation from the current accessor. Stop rotates only the Memory Resource ID's bindings, invalidating every dependent old Tool lease across visibility scopes while leaving unrelated Tool bindings and control leases intact. Direct internal Memory handles capture the same Resource lease. / Memory Tool 在 resolve 时同时捕获自己的 canonical Tool lease 与所需 Resource lease；校验和准入检查两个捕获值，不能临时换成当前 Resource generation。stop 只轮换统一 Resource ID 的各 scope binding，使所有旧数据工具 lease 失效，无关工具与控制 lease 不变；内部 handle 同样捕获 Resource lease。

This explicitly approves extending the public lease/host contract if the user approves this design: replace Tool-only mandatory evidence in `BackendLease` with kind-specific source evidence, retain `CanonicalEntryLease` for Tool leases, and add Resource evidence backed by `MemoryRuntime`. A Resource lease must not fabricate a Tool registry entry. Required-service lease capture is part of that contract, not private implementation detail. Add only this concrete Resource backend to the existing host composition; other deferred kinds remain deferred. / 用户批准本设计即批准必要的公共 lease/host 契约扩展：BackendLease 的源证据按 kind 区分，Tool 保留 CanonicalEntryLease，Resource 由 MemoryRuntime 提供证据，不伪造工具 entry。依赖 lease 的捕获是公共契约，不是私有细节；只挂载这个具体 Resource，其他 kind 仍延期。

For Memory Tool admission, the full lock order is Memory source gate → canonical registry read gate → OwnershipTree → entry-local state. Validate Tool and required Resource ownership together in one tree critical section; preserve the registry's existing canonical-entry admission guarantees. No tree re-entry, I/O or await inside that critical section. Non-Memory dispatch retains its current path. / Memory 工具完整锁序为 source gate → canonical registry read gate → OwnershipTree → entry-local；在一次 tree 临界区共同校验 Tool 和 Resource，保留 canonical-entry 现有保证；不重入树、不在锁内 I/O/await；无关工具路径保持现有行为。

## State and operation semantics / 状态与操作语义

`Off → Starting → Running → Draining → Off`, with `Failed` retaining owned cleanup work and the first failure. / `Off → Starting → Running → Draining → Off`；`Failed` 保留尚待清理工作及首个错误。

- Initial off is cold for Memory-only workers/connections/caches. Raw archival may still open its own connection. / 初始 off 不创建 Memory 专属 worker/连接/cache；raw 仍可打开自己的连接。
- Start constructs one complete resource bundle before publishing admission. Failure drains any partially started bundle and keeps admission closed. / start 先完成资源 bundle 再开放准入；失败则清理部分 bundle，准入保持关闭。
- Stop closes new admission first, invalidates old leases, signals periodic producers to stop scheduling, and observes admitted work plus retained workers before closing resources. / stop 先关闭准入并失效旧 lease，再停止周期生产，观察既有工作后关闭资源。
- Concurrent stop calls join the same transition; start during draining returns an explicit busy result. Repeated off-stop is idempotent; repeated running-start returns current status without new workers. / 并发 stop 加入同一过渡；draining 时 start 明确 busy；重复启停不产生新 worker。
- New explicit Memory RPC/tool requests return a typed disabled/draining/stale result. Automatic recall during chat omits Memory context and exposes the status; it must not fail ordinary chat or silently resume Memory. / 显式 Memory 请求返回禁用/排空/旧引用错误；聊天自动 recall 跳过 Memory 上下文并暴露状态，不阻断聊天，也不偷偷启动 Memory。
- Already admitted operations may finish and publish their result during draining; new admission with an old lease fails even after re-enable. No replay or exactly-once external-effect promise. / 已准入工作可在 draining 期间完成；旧 lease 重启后仍拒绝，不承诺 replay 或外部效果 exactly-once。
- A cancelled stop caller leaves one source-owned transition and the same retained handles. A later stop/status observes that transition; no replacement drain task. / 取消 stop 调用方不取消源拥有的过渡和句柄；后续调用观察同一过渡，不替换 drain 任务。
- A task panic/close error is retained and reported; siblings are still observed. Failed cleanup cannot publish `Off` or start another bundle while resources remain unsettled. / panic/close 错误保留报告，兄弟任务继续观察；资源尚未清理完毕时，失败清理不发布 Off，也不启动第二 bundle。

**Resolution of builder question 2 — proposed recovery / builder 问题 2 的恢复提案:** `Failed` is retryable, not a process-terminal state. A later stop resumes the same retained cleanup, observes remaining tasks and retries the retained connection close without rerunning completed work. Once all resources are demonstrably released, current state becomes `Off`, including after a fully cleaned-up start failure. Historical first failure remains in status and the transition result; reaching `Off` does not rewrite it as success. A later explicit start may construct a fresh bundle/lease; merely querying status never retries or starts work. / Failed 可恢复，后续 stop 继续同一清理，观察剩余句柄并重试保留的连接关闭，不重跑已完成任务。全部资源已证明释放后进入 Off，start 失败但清理完毕也如此。历史首错仍保留在 status/过渡结果，不能改写为成功；之后显式 start 可创建新 bundle/lease，查询 status 不触发恢复或启动。

## Resource and admission inventory / 资源与准入清单

| Domain / 领域 | Memory-owned work / Memory 拥有 | Off behavior / 关闭行为 |
|---|---|---|
| Notes | orientation, rebuild/category work, watcher, note tools, graph/recall queries | reject new work, stop watcher, observe maintenance/query work |
| Compression | periodic, threshold, manual, session-end and summary/reflection writers | stop scheduling; retain/join all admitted children |
| Dream | timer and each dream execution | cooperative stop at existing safe boundaries; observe active work |
| Producers | scheduler ticks and producer executions | no further ticks/admission; observe children |
| Reconciler | daemon scan/report and any note projection work | stop and join, not fire-and-forget abort |
| Embedding | migration, progress task, embedding requests/provider references | reject new requests; drain requests; drop owned provider/cache refs |
| Store | NoteStore/graph/recall connection and blocking operations | join blocking owners, close dedicated Memory connections |
| Raw sessions | append, raw recovery, archival and metadata connection | stays active through existing session ownership |

Every `spawn` reached from these entry points must be classified as raw, Memory, or unrelated with a source owner. Count spawn sites and direct store captures before editing; completion requires no unclassified Memory writer. / 实施前枚举 spawn 与直接 store 捕获，区分 raw、Memory、无关工作；完成时不能留未归属的 Memory writer。

A source permit must live through the actual worker/blocking closure, not merely the request future. Cancelling an HTTP/RPC future must not decrement the count while its `spawn_blocking` still holds a connection. Internal children inherit the admitted operation's ownership rather than obtaining new admission during draining. / permit 必须活到实际 worker/blocking closure 完成；取消 RPC future 不能在 blocking 仍持连接时减少计数。内部子任务继承原准入，不能在 draining 时重新抢准入。

## Storage close and re-open / 存储关闭与重开

Migrate long-lived consumers from owning a concrete backend/provider `Arc` to the runtime's checked access boundary. After admission, an operation may borrow/clone the current bundle only for its owned execution. No raw connection or permanent concrete backend clone escapes to tools, graph, recall, compression or globals. / 长寿命消费者改为持经过检查的 runtime 边界；准入后仅在本次拥有的执行内使用当前 bundle，不向工具/graph/recall/compression/global 泄露永久具体 backend/连接。

Refactor the SQLite connection holder so all Memory-side direct locks and blocking paths use one checked, closeable connection boundary. Close runs after admitted and blocking work drains; return close errors with the connection retained for truthful diagnosis/retry. Do not use `Arc::strong_count` as completion evidence. / SQLite 所有直接锁与 blocking 路径迁移到统一可关闭边界；排空后关闭；关闭失败保留连接并报错，不以 Arc 计数作为完成证据。

Raw and notes may continue sharing the file through separate connections. `Off` means the Memory connection is closed, not that the file has no open handles, no WAL, no writer, or that process RSS falls. Re-enable opens a new Memory connection, rebuilds Memory-owned wiring and publishes a fresh lease. / raw 和 notes 可继续各自连接同一文件；Off 只代表 Memory 连接关闭，不代表文件无句柄、无 WAL、无写者或 RSS 下降。重启新建 Memory 连接与 wiring，再发布新 lease。

Persistent Memory helper globals must become stable accessors to the injected runtime, not install-once concrete service owners. Existing static unrelated capability slots remain unchanged. / Memory 全局 helper 改为稳定 runtime 访问边界，不能持 install-once 具体服务；无关 slot 不改。

## Startup and shutdown cleanup / 启停清理

Construct a concrete startup resource owner before the first resource-producing operation. Put fallible startup inside an owned async result boundary and funnel both error and server return through cleanup. Retain each real worker handle as soon as spawned, not at the later all-resources-ready point. / 首个资源创建前构造具体启动 owner；可失败启动进入有 owner 的 async Result 边界，启动失败和 server 返回都进入清理；spawn 当场保留真实句柄，不等全部就绪。

Cleanup stops new Memory admission/schedulers, drains watcher/maintenance and all other Memory workers, then releases Memory resources; raw/server shared resources follow their existing shutdown needs. Preserve the original startup/server error; cleanup errors are additionally observable. Drop may warn but must not claim asynchronous cleanup happened. / 清理先关闭准入/调度，排空 watcher、maintenance 及其他 Memory worker，再释放资源；raw/server 共享资源按自身停机需要处理。保留原始启动/server 错误，额外报告清理错误；Drop 不能冒充异步清理完成。

Early cleanup covers already acquired resources, including the earlier tool projection host when owned by this boot attempt. Do not close another live host or process-global raw/session authority through a convenience guard. / 启动失败清理覆盖本次启动已取得的资源，包括更早挂载的 projection host；不能误关其他活 host 或全局 raw/session authority。

## Verification and rollout / 验证与实施顺序

Implement as dependency-ordered slices in one approved project; intermediate commits must state incomplete lifecycle coverage rather than advertise a working global switch. / 依赖顺序分片实施，中间提交明确覆盖不完整，不宣传已实现全局开关。

1. Pin census and invariants; add runtime RED for early startup failure, stale admission and retained cancellation. / 固定清单与不变量，先获得生命周期运行时 RED。
2. Add isolated Resource ownership rotation and Memory Resource/control composition with lease tests; prove unrelated bindings survive and all existing Memory visibility scopes are invalidated. / 建立隔离 Resource generation 轮换与控制组合，证明无关 binding 不受影响、Memory 各 visibility scope 全部失效。
3. Introduce the concrete runtime/access boundary and closeable SQLite connection; migrate all consumers and raw/Memory session-end split. / 引入 runtime/连接边界，迁移所有消费者并拆分 raw/Memory session-end。
4. Retain/join every Memory worker and blocking operation; wire start/stop and early startup cleanup. / 归属并排空所有 worker/blocking 工作，接入启停与启动失败清理。
5. Exercise real stop→raw append/chat→re-enable, stale handles, concurrent/cancelled stop, panics, partial initialization and resource-close failure. / 真实验证停机→raw/聊天继续→重启、旧引用、并发/取消、panic、部分初始化与关闭失败。

Use real temp Markdown and SQLite, deterministic barriers rather than sleep races, and production entry points. Live E2E pins the exact built binary, temporary home/config, local-only provider stub, and sends signals only to its own PID. Assert signal logs, orderly exit without watchdog force, endpoint/listener removal, database integrity and persisted effects. This does not replace deterministic in-flight drain tests. / 使用真实临时 Markdown/SQLite、确定性 barrier 和生产入口。live E2E 固定产物、临时目录与本地 provider stub，只信号本次 PID；同时断言信号日志、无强退、端口/发现文件清理和持久化，不能替代确定性 in-flight 测试。

Before every Cargo command run `python qa/browser_live/check_memory.py`; require `ALLOW` with at least 4 GiB; `CARGO_BUILD_JOBS=1`, serial Cargo. Stage shell placeholders through the existing just recipe. Run affected tests, full workspace, clippy and available cross-target checks; distinguish interrupted suites, compile-only checks and native execution. / 每条 Cargo 前执行内存门，至少 4 GiB，jobs=1 串行；使用现有 placeholder recipe。明确区分中断、交叉编译检查与真机执行。

Fresh read-only native Pi builder check precedes design approval; independent implementation review follows final source/tests. Keep local branch for review; no push, merge, new dependencies or unrelated harness changes. / 设计先经全新 Pi builder check，再交用户批准；实施后独立审查。保留本地分支，不推送、不合并、不加依赖或无关 harness 改动。

## Exclusions and limits / 排除与限制

No raw archival shutdown while chat continues; no whole-file closure claim, native unloading/RSS promise, hard stop deadline, distributed fencing, external-effect exactly-once, embedding provider hot replacement, ChannelRegistry redesign, or generic universal lifecycle scheduler. / 不在聊天继续时关闭 raw，不宣称整个文件关闭、native 卸载/RSS、硬期限、分布式 fencing、外部效果 exactly-once、provider 热替换、ChannelRegistry 重构或通用 scheduler。

Decisions left to the implementation plan / 留给计划的细节: exact file split and private type names, preserving Rust module visibility and repository layout; typed error mapping into existing RPC/tool schemas; test file placement and dependency-ordered commit boundaries. These must implement the admission/resource contract above, not weaken it. / 具体文件拆分、私有命名、错误映射和测试/提交位置交给计划，不能削弱准入与资源契约。

## Builder check / 构建者审查

A fresh read-only native Pi review identified two important planning gaps: dependency lease semantics and recovery from failed cleanup. Both now have explicit proposed contracts above, for the user's approval. The review produced a complete report before its process ended with SIGTERM; it did not run implementation tests or approve an implemented runtime. Main integration also tightened Resource rotation to cover all visibility bindings of the Memory Resource ID. / 独立审阅提出依赖 lease 与失败清理恢复两项缺口，已在上文写为待批准提案。报告完整产出后进程以 SIGTERM 结束；不是实施测试或生产验收。主 Agent 另补强同一 Resource ID 各 visibility binding 的失效覆盖。
