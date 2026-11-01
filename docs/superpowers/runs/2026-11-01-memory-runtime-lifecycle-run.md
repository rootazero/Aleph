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

The full runtime implementation and later checks remain pending. / 完整运行时与后续验证仍待完成。
