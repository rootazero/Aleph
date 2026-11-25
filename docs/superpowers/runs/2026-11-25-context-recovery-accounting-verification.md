# Context recovery and accounting verification / 上下文恢复与计量验证

- Candidate: `refactor/context-compaction`, Windows x64 on Intel Core i9-13900H (~20 GiB visible RAM); base `38d5f213dfa80b0934ffe596444b9ab99e35134a`. / 候选分支、测试机与基线。
- Contract: [design](../specs/2026-11-25-context-recovery-accounting-design.md) and [implementation plan](../plans/2026-11-25-context-recovery-accounting.md). / 设计与计划。

## Three-repository scan / 三仓扫描

| Source / 来源 | Reused or deferred / 复用或延后 |
|---|---|
| context-mode `3a3ef7d5` | Its precedent is separating admitted preview from searchable raw output and scoping the search. Aleph already implements that in `src/tools/scoped/dispatch.rs` and `src/tools/result_store.rs`; this change exercises the real path instead of introducing another index. / 短视图和原文索引已接线；验证现有链路，不建第二索引。 |
| billion-context `a392b529` | Its bounded restoration and separate cost ledger informed this change; global refs, prompt-cache re-pay attribution and provider-proxy mechanics do not carry over. / 借鉴有界恢复与计量分层，不移植全局引用、缓存重付归因或代理结构。 |
| Aleph `38d5f213d` | The old reader loaded the session log, evaluated a correlated FTS witness lookup for each row, and rendered before pagination. The manual compactor compared only the summary body but inserted framing into the event log. / 旧路径逐行查证、整段渲染；手动计量漏算实际落盘的 framing。 |

The three existing `graphify-out` graphs were used for discovery, with findings checked against source. The Aleph graph predated the base (`074fc1c0`); the other two covered the stated reference revisions. / 使用现有三仓图谱定位并回查源码；Aleph 图谱早于基线，其余两份与上述版本一致。

## Targeted probes / 定向探针

- Rust calculation probe copied the actual estimator and old/new gain expressions: before **101**, body **100**, framed **160** estimated tokens. The old expression admitted the non-shrinking summary; the new expression rejected it. This is expression-level RED/GREEN, not the `SessionEventStore` or provider path. / 实际计量表达式的红绿对照；不等同真实事件库或模型调用。
- A standalone Rust probe exercised the production pagination helpers: the 79-token terminal-with-blank-tail case and 46-token continuation-only case both advanced; structured JSON exhaustively checked each UTF-8 offset in its fixture, and 8,256 combinations varied budget, body and sequence digit length. For successful pages it checked the serialized cap, monotone cursor and exact prefixes through complete restoration. / 实际分页函数的定向探针，覆盖两种封套边界、结构化正文的 UTF-8 偏移及 8,256 组预算与正文组合；不替代端到端测试。
- On a synthetic in-memory SQLite DB using the extracted production queries, median of three reads (old → new SQL only) was **335.490 → 0.399 ms** for 1,000 unrelated events and **5,398.435 → 1.204 ms** for 4,000. These numbers do not measure the tool, modeling latency, or the production workload. `EXPLAIN QUERY PLAN` changes from `CORRELATED SCALAR SUBQUERY 1` + `SCAN f VIRTUAL TABLE INDEX 0:` to the existing `(session_id,event_type)` metadata index, `(session_id,seq)` range seeks and one bounded-identity FTS query. FTS still scans the virtual table and can sort matching IDs; its work is not independent of unrelated rows. / 合成内存库只测 SQL；新路径仍有一次 FTS 扫描和可能的临时排序，不是恒定成本。

- On a temporary SQLite file, the actual Rust snapshot methods used the bundled `rusqlite` libraries, with an isolated event-decoder shim. For 1,000 unrelated rows, two reads measured **330.20/391.86 ms** for old SQL versus **0.612/0.632 ms** for the snapshot; for 4,000, **5.315/5.540 s** versus **1.643/1.434 ms**. The shim verified witnessed corruption is deferred until the needed page, a later erased row blocks delivery even with a matching sequence in another session, and a missing FTS table fails closed. These are storage-method probes, not timings from the production decoder or full tool. / 在临时文件上调用实际 Rust 恢复快照及随包 SQLite，事件解码用替身；验证迟到擦除、延迟解码及 FTS 缺失的行为，不等同完整工具基准。

## Full-tool SQLite timing / 完整工具 SQLite 实测

`measure_fold_recovery_with_unrelated_tail` uses the real Rust event store, bundled SQLite, real decoder and `session_decompress` page builder. It runs the removed SQL plus eager decode and the new complete page against the same seeded file; both runs also check that a later erasure denies recovery. The old figure excludes cloning and rendering, so it is a lower bound on the old tool path. Two reads per size, no performance threshold or production-workload claim. / 同一磁盘库上以真实事件库与解码器测量，两次读取并检查迟到擦除拒绝；旧路径未计复制与渲染，不作生产负载承诺。

| Unrelated rows / 无关行 | Old read and decode / 旧读取及解码 | New complete page / 新完整页 |
|---:|---:|---:|
| 1,000 | 350.017 / 341.069 ms | 1.136 / 1.002 ms |
| 4,000 | 5,416.378 / 5,523.280 ms | 2.515 / 1.813 ms |

These are the two final executable runs per size. An earlier build gave 375.912–432.069 → 0.880–1.810 ms and 5,696.743–6,038.426 → 1.859–2.004 ms respectively. / 表中为最终二进制的实测；早一版的两次对照也保留区间。

Reproduce after the memory preflight / 通过内存检查后复测：

```sh
cargo test -p alephcore --lib measure_fold_recovery_with_unrelated_tail -- --ignored --nocapture --test-threads=1
```

## Build and formatting / 构建与格式

- Windows MSVC Rust `1.99.1`, LLVM `23.1.1`: first Core unit build exited from `rustc` with `0xc0000409, STATUS_STACK_BUFFER_OVERRUN` after passing the memory preflight. It produced no test binary; the failure is not counted as a regression RED. / 首次 Windows 全量单测构建由编译器异常终止，没有测试结果，不能算回归红灯。
- With `RUST_MIN_STACK=33554432`, one Cargo job and dev debuginfo disabled, the next Core unit build completed. That executable passed adjacent suites: compaction (184), `ctx_search` (12), tool-result store (27; one ignored), and fold economics (4). / 加大编译器线程栈后 Core 单测二进制生成，邻接套件通过。
- The first rebuild after correcting the test fixtures hit `rustc-LLVM ERROR: out of memory`. The subsequent test build used the same settings and `--config profile.dev.package.alephcore.codegen-units=256` to reduce per-unit LLVM memory without changing the repository profile. / 修正夹具后首次重编遇到 LLVM 内存不足；下一次仅通过命令行细分 Core 代码生成单元，无项目配置更改。
- `cargo check -p alephcore --tests` and `cargo clippy -p alephcore --tests -- -D warnings` passed with the same build settings. / Core 实现与测试的类型检查、严格 Clippy 均通过。
- Targeted `rustfmt --edition 2021 --config skip_children=true --check`, the installed hook's package-scoped `cargo fmt` command and `git diff --check` passed. `cargo fmt --all --check` reports formatting differences in 28 files identical to the base. / 修改文件格式、提交钩子的包级格式和 diff 检查通过；全仓格式检查在 28 个与基线相同的文件上有差异。

## Focused Core tests / Core 定向测试

The final Core unit executable passed 161 focused tests: manual compaction (24), folds (9), session store (63), restore (17), source retrieval through scoped `ctx_search` (1), Structured RPC (3), and result processing (44). The ignored SQLite measurement ran separately and passed, as did the media-artifact suite (28). No full workspace suite or live provider call was run. / 最终二进制定向测试 161 项通过，另单独通过 SQLite 基准和媒体附件测试 28 项；未跑全仓测试或真实供应商请求。

The real restore registration test verifies untrusted arguments cannot replace the injected session, absent authority is rejected, and compact JSON and its continuation cursor survive Layer 2 without appending events. The real scoped source search finds the test producer's admitted output in that session and not in another. / 真实工具接线验证会话权限、单回包 JSON 与游标、恢复只读及索引隔离。

The pre-fix checks used the targeted probes above, not a full Core binary built with the old implementations. The old production reader is gone; only the ignored benchmark retains its SQL and eager decoder as a lower-bound baseline. / 旧实现的红灯限于探针；仅基准保留旧 SQL 及 eager 解码作下界。

## Review / 审查

The independent reviews checked the single snapshot, framed accounting, canonical session injection, and Layer 2/Layer 3 boundary. Two passes blocked on actual JSON envelopes and eager structured rendering. The final independent pagination review cleared both envelope cases and confirmed rollback, cursor, serialized cap and UTF-8 boundaries. It was static review, not a test run. / 独立审查覆盖恢复快照、计量、会话注入与准入；实现修复了封套和整段序列化问题，最终分页复审无阻塞；未代跑测试。

## Scope limits / 范围边界

- A page decodes one event payload at a time; the rendered body window is budget-bounded. Structured JSON replay to a late byte offset still serializes through the skipped prefix. SQLite may allocate for its filtered ordering. / 不分配整段已渲染正文，但单条事件仍需解码、深偏移仍需重放前缀，SQLite 排序仍可分配内存。
- No durable automatic folding, provider cache attribution, or new event store/tool. Existing fold amounts are not rewritten. Ordinary tool-result persistence and aggregate spill remain unchanged. / 未加自动持久折叠、新事实源或供应商计费归因；不改历史账本与整轮 spill。
