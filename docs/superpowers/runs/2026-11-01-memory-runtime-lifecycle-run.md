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

Implementation and checks pending. / 实施与验证待完成；本记录的授权提交不代表任何实现测试已经通过。
