# Memory runtime lifecycle execution / Memory 运行时实施记录

**Status / 状态:** Authorized implementation in progress; not a completed Memory switch. / 实施已授权，进行中；尚未完成 Memory 开关。

## Authorization and baseline / 授权与基线

The user approved the written design and selected chat/raw archival continuing while Memory is off, then approved the 11-task implementation plan on 2026-11-01 with “批准”. This is explicit authorization to execute the plan, including production changes, local commits, real temporary-store tests, isolated live-server signals and independent native Pi review. / 用户已批准设计及 11 项实施计划，明确授权生产实施、本地提交、临时真实存储测试、隔离服务信号验证与独立审查。

- Checkout / 检出: current Git root; branch `note-maintenance-drain`.
- Baseline / 基线: `eda68f0e7c842ff4930c47eddc15c28dbe9350d8`, clean before execution; single checkout. Local main remains `c732c2d575d508834cc36090c3f97a89af436ed2`.
- [Approved plan](../plans/2026-11-01-memory-runtime-lifecycle-plan.md) and [approved design](../specs/2026-11-01-memory-runtime-lifecycle-design.md) govern implementation.
- Workflow / 方式: native main agent sole writer, test-first focused slices, fresh read-only native Pi whole-implementation review, concrete fixes and re-review. Preserve local branch; no push or merge.
- Boundaries / 边界: raw/chat remain active; one Memory lifecycle source, canonical Tool metadata source and OwnershipTree generation authority; no new dependencies or generic supervisor.

## Verification discipline / 验证纪律

Before every Cargo command: `python qa/browser_live/check_memory.py`, require ALLOW and at least 4 GiB, `CARGO_BUILD_JOBS=1`, serial execution. Keep exact commands, exits, selected/pass/fail counts and source boundaries. / 每条 Cargo 前独立内存门，jobs=1 串行，保留真实结果。

Runtime failures, not merely missing-type compiler errors, are required RED evidence. Use real temporary Markdown/SQLite and deterministic barriers. Source-only wiring, library behavior, live OS signals and full Memory claims remain separate. / 运行时 RED、真实存储、确定性 barrier；不同证据不得混称。

Known prior blockers / 既有阻塞: workspace tests interrupted with failures/hangs; clippy unsupported negative lookahead at `src/exec/secret_patterns.rs:138`; Linux cross-target Wayland pkg-config sysroot missing. Previous signal 27/27 and bounded 61 tests do not validate this new runtime. Historical details remain in the [expansion record](2026-11-01-memory-lifecycle-expansion-run.md).

## Source census / 源码清单

Initial audited inventory: Memory backend has 71 synchronous connection access sites plus the blocking `with_conn` site; raw Markdown store owns a separate connection to the same file. Recount against source before migration. Inventory rows and per-task evidence are appended during execution. / 初始统计待实施中复核；逐项记录 owner、准入与测试，不以数量冒充接线。

| Domain / 领域 | Source boundary / 源码边界 | Current ownership gap / 当前缺口 | Planned proof / 计划验证 |
|---|---|---|---|
| SQLite | `src/memory/store/sqlite/` sync locks and `with_conn` | connection not closeable; blocking clone outlives caller | actual close, raw continuation, retained connection on error |
| Startup notes | start `mod.rs`, builder `subsystems.rs` | common-run owner exists, early-return ownership incomplete | early-error real handles/effects |
| Compression | `compression/service.rs`, builder handlers | periodic/threshold/profile children | cooperative tick stop and observed children |
| Dream | `dreaming/mod.rs` | global daemon plus discarded task | manual/timer shared admission and join |
| Producer | `extensions/scheduler.rs`, `agent_init/tool_catalog_init.rs` | discarded scheduler handle | complete tick capture/filter/insert |
| Reconciler | `events/handler.rs`, gateway server | abort-only retained handle | current scan completion and cooperative stop |
| Embedding | `reembed.rs`, embedding manager, Memory RPC | migration/forwarder/provider references | both real handles and admitted blocking work |
| Session end | `gateway/session_manager/ops/emit.rs` | raw and derived children interleaved | raw while Off; derived children owned |
| Consumers | builtins, thinker, assembler, retrieval, engine, telecom | concrete service/cache/global captures | explicit admission and cold optional enrichment |
| Canonical dispatch | slot-api metadata, registry, facade, scoped dispatch | no required Resource capture | stale approval and admitted-result preservation |

## Results / 结果

### Task 1 — canonical required services / canonical 依赖服务

- Runtime RED / 运行时失败: `cargo test -p aleph-slot-api --lib descriptor::tests` selected 25: 23 passed, 2 failed, 254 filtered. Required services disappeared across canonical conversion, and adding a Memory dependency did not change the Safe replay fingerprint. / 依赖元数据丢失，且新增依赖未影响 Safe 指纹。
- Implementation / 实施: `CapabilityId`, `ServiceRelation` and `Service` now have one definition in slot-api, re-exported by Core. Canonical descriptors and `ToolDefinitionMetadata` round-trip `services`; empty services retain legacy JSON and fingerprint bytes. Nonempty edges use the sorted/deduplicated, length-prefixed `aleph-services-v1` preimage extension in both replay and Core projections. No Memory handlers have been migrated yet. / 单一类型源、双向传播、空依赖兼容；实际 Memory handler 尚未迁移。
- API GREEN / API 通过: `cargo test -p aleph-slot-api --lib`: 279 passed, 0 failed, 0 filtered. The first GREEN attempt had a missing test-fixture field compiler error; that fixture was corrected before this successful run. / 首次绿色尝试的 fixture 编译错误已修正，保留失败记录。
- Core GREEN / Core 通过: `cargo test -p alephcore --lib capability::backend`: 6 passed, 0 failed, 21734 filtered; exit 0 after 445.0 seconds. / Core 投影 6 项通过。
- Full package GREEN / 完整包验证: the plan's exact `cargo test -p aleph-slot-api` also passed: 392 passed, 0 failed, 2 ignored across nine summaries, including integration tests and doctests. Together with Core, 398 distinct selected tests passed; the earlier 279 library tests are included rather than double-counted. / 完整包与 Core 合计 398 项通过，不重复累加先前 lib 运行。
- Formatting and patch validation / 格式与补丁: affected-file `rustfmt --check` and `git diff --check` passed. / 受影响文件格式与补丁检查通过。
- Build observation / 构建观察: the Core command passed its pre-Cargo memory gate; while linking, a separate observation reported `available_bytes=2859057152 required_bytes=4294967296 BLOCK`. No additional Cargo command was launched during that observation. / 启动前门限通过；链接期间低于门限，未启动额外 Cargo。

### Task 2 — targeted rotation and atomic admission / 定向轮换与原子准入

- Runtime RED / 运行时失败: ownership selected 34 (31 passed, 3 failed); registry selected 56 (54 passed, 2 failed); versioned registry selected 16 (14 passed, 2 failed). Six new assertions failed on no-op rotation, first-binding-only validation, unlocked callbacks or missing re-entry protection. One existing registry regression also failed: single-entry resolution constructed 32 callable views instead of one. / 六项新断言及一项既有回归失败。
- Implementation / 实施: rotate only the selected capability across all visibility bindings, invalidate its claims, validate all captured bindings under one tree mutex, and hold the existing publication mutation gate through the synchronous callback without changing revision. The provider ordering is captured plugin provider → registry provider → registry mutation → ownership tree; the channel path retains its distinct source-private provider. / 定向轮换、同锁复数校验与只读发布门；不更改通道独立 authority。
- Bounded prerequisite / 局部前置修复: the pre-existing single-entry regression was source-confirmed against local main; `ZahirFacade::resolve` now captures the requested canonical entry directly instead of building the whole callable projection. This narrow Task 5 capture improvement was pulled forward; its facade suite is included in this task. / 将既有全量构造回归的局部修复提前，并补跑 facade 验证。
- GREEN / 修复验证: `cargo test -p alephcore --lib capability::ownership`: 34 passed; `tools::registry`: 57 passed; `capability::versioned_registry`: 16 passed; `capability::zahir_facade`: 32 passed. All four commands exited 0, with 139 selected tests passed and none failed. Each command had its own ALLOW memory gate and ran serially with jobs=1. / 四条独立内存门命令全部通过，合计 139 项，无失败。
- Read-only bundle census / Bundle 只读清单: a resumed native Pi researcher supplied constructor/input boundaries for backend, embedding, notes, compression, Dream, retrieval, producer and reconciler. The report identifies shared raw storage, extension registry, chat provider and resilience database as external inputs, with no required generic supervisor. A complete report was returned before the child process ended with SIGTERM; it is source research, not a passing implementation review or test. / 构造依赖清单已取得；完整报告后进程 SIGTERM，仅作为源码调查证据。

### Task 3 — closeable Memory connection / 可关闭的 Memory 连接

- Test setup / 测试准备: real SQLite statement keeps the connection busy for a failed-close retry; a separate real Markdown store writes raw records before and after Memory close, checks integrity, and verifies that opening a replacement backend cannot reopen the old one. / 真实未完成 statement 与独立 raw 连接验证关闭、重试及新旧实例隔离。
- Corrected compile attempt / 编译修正: the first attempt imported `NoteStore` from `memory::notes` instead of `memory::notes::store`. It produced import/method-resolution errors and was stopped before tests; its log is retained as `task3-red-import-compile.log`. Both imports were corrected before rerunning. This is not runtime RED evidence. / 首次导入路径错误已修正，停止无效编译并保留日志；不计作运行时 RED。
- Runtime RED / 运行时失败: corrected `cargo test -p alephcore --lib memory::store::sqlite` exited 101 with 105 passed, 2 failed, 21645 filtered; `memory::store::markdown` exited 101 with 19 passed, 1 failed, 21732 filtered. The failures show that the no-op close neither rejects subsequent access nor reports a busy connection, and the old backend remains usable after creating a replacement. / 三个新断言实际捕获空关闭实现的错误。
- Implementation / 实施: `Arc<Mutex<Option<Connection>>>`, one checked SQL boundary, 71 migrated synchronous sites and the blocking boundary. Actual close failure restores the returned original connection; success leaves the slot empty. Existing transactions and SQL bodies were retained mechanically. Raw Markdown connection ownership is unchanged. / 单一可关闭连接 slot 与检查边界；保留原有事务、SQL 及 raw authority。
- GREEN / 修复验证: `cargo test -p alephcore --lib memory::store::sqlite`: 107 passed, 0 failed, 21645 filtered; `memory::store::markdown`: 20 passed, 0 failed, 21732 filtered. Both exited 0 after independent ALLOW gates, serial jobs=1. Affected-file rustfmt and diff checks passed. / 合计 127 项通过，两个命令均独立通过内存门并串行执行，格式与补丁检查通过。
- Migration audit / 迁移审计: all 71 SQL method tails matched the original after ignoring whitespace, commas and formatter-added closure braces; the 12 initial comparison differences were formatter wrapping of error closures. Production helper files no longer access the connection directly. This static check supplements the real store tests, not a separate semantic proof. / 71 处原 SQL 逻辑机械保留；静态比对作为真实存储测试的补充，不单独冒充语义证明。
- Iteration build / 迭代构建: the local ignored runner now additionally sets `CARGO_INCREMENTAL=1`, retaining jobs=1 and a fresh memory gate for each Cargo command. Repository build profiles are unchanged. / 仅本地 runner 开启增量缓存，仓库构建配置未修改。

### Task 4 — source-owned runtime foundation / 源拥有的运行时基础

- Runtime RED / 运行时失败: `cargo test -p alephcore --lib memory::runtime` exited 101: 0 passed, 6 failed, 21752 filtered. The probes falsely reported Off, lost the actual blocking join, did not join concurrent drains, revived an old handle after restart, and ignored genuine SQLite close failure. / 六个真实存储与 barrier 测试捕获生命周期探针错误；编译错误不计入本轮 RED。
- Implementation boundary / 实施边界: one concrete runtime retains the real lifecycle driver and child/blocking handles; one shared admission guard survives caller cancellation, and the Running driver observes completed children. Bundled SQL requires that same admitted operation; standalone stores keep their independent contract. The resource binding is captured once and checked against OwnershipTree at admission. This foundation is not yet mounted into production and does not yet migrate the existing Memory workers or consumers. / 单一 source 持有真实句柄、共享准入与回收任务；生产挂载、旧 worker/consumer 迁移仍在后续任务。
- Error semantics / 错误语义: the approved design requires a fully cleaned-up retry to enter Off while preserving the historical first error in both status and the stop result. The busy-close test's initial retry-`unwrap` expectation was corrected to assert Err plus actual Off and a closed connection. A subsequent explicit start may create a fresh bundle. / 按已批准设计修正重试测试：资源释放后 Off，但不能把首错洗成成功；之后显式 start 才能新建 bundle。
- Additional private evidence / 额外私有验证: initialization barrier retains the same driver through cancelled start/stop; a real note-directory failure exercises partial-backend cleanup; child panic still observes its sibling; completed children are reaped while Running and operation clones share one admission. The existing six tests additionally inspect the real connection slot and exact driver identity. / 额外覆盖部分初始化、panic/兄弟任务、运行中回收、同一次准入与真实连接关闭。
- GREEN / 修复验证: `cargo test -p alephcore --lib memory::runtime` — 10 passed, 0 failed, 21752 filtered; `memory::store::sqlite` — 107 passed, 0 failed, 21655 filtered; `memory::store::markdown` — 20 passed, 0 failed, 21742 filtered. All three exited 0 with independent ALLOW memory gates, `CARGO_BUILD_JOBS=1` and local incremental caching; 137 distinct selected tests passed in this verification. The store regressions preserve standalone SQL and raw archival behavior. / 三条串行命令均通过，共 137 项；真实 gate/命令日志保留在本轮 ignored workspace，未把旧测试重复计作新增覆盖。

The full runtime implementation and later checks remain pending. / 完整运行时与后续验证仍待完成。
