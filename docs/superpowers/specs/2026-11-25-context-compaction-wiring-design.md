# Aleph 上下文压缩与按页恢复 / Context compaction and paged recovery

状态 / Status: 方案 B 已选，设计待审批 / Approach B selected; design pending approval. Baseline: `38d5f213d`.

## 目标与方案 / Goals and approaches

在现有 Rust 生产路径上，减少紧急压缩的重复计算，恢复折叠时不再加载整场会话或渲染整段；保住回放、清除和续页语义。只在 `.worktrees/context-compaction-wiring` 交付与提交，不改 `main`。不追求未经验证的 98%／5×，不换摘要模型，不迁移存储，不引入依赖、代理或第二套治理系统。

Reduce repeated emergency-fit work and remove whole-session loading and whole-span rendering from fold recovery. Preserve replay, erase and pagination behavior. Deliver and commit only in the isolated worktree. No new dependencies, model or storage migration, proxy, parallel control plane, or unmeasured savings claim.

| 方案 / Approach | 取舍 / Trade-off |
| --- | --- |
| A · 只修兜底 / Fit only | 改动最小，但不解决恢复路径的读放大。 / Smallest change; leaves restore read amplification intact. |
| **B · 原生修复两个路径 / Repair both native paths** | 复用现有估算器、事件库和折叠元数据；两条线可独立实现。 / Reuses the estimator, event store and fold metadata; allows independent implementation. |
| C · 重做架构 / Replace the architecture | 可重做协议与存储，但迁移、缓存和兼容成本超出已证实的问题。 / Protocol and storage migration costs exceed the demonstrated gaps. |

## 参考与取舍 / Reference findings

两份本地仓库先读 graphify，再核对源码；借鉴设计，不复制代码。 / Both local graphs were consulted before source; borrow the ideas, not the implementation.

| 主题 / Topic | 参考依据 / Reference | Aleph 决策 / Aleph decision |
| --- | --- | --- |
| 大工具结果 / Large tool results | `context-mode@3a3ef7d53`, `src/core/tools/execute.ts:136–167`, `src/core/store/fts.ts:45–86,471,863`: direct small results; index full large output before selecting recoverable excerpts. | 沿用 `src/tools/result_processing.rs`、`src/tools/result_store.rs`、`ctx_search` 和父 epoch 作用域。 / Keep the native offload, search and parent-epoch scoping. |
| 折叠与恢复 / Fold and restore | `billion-context@a392b529d`, `kernel/src/recommend.ts:87,264`, `kernel/src/compress.ts:696–840`, `kernel/src/decompress.ts:158–223`: protect anchors; validate fold ranges; read-only recovery. | 用原有 `FoldRecorded`/`CompactionPerformed` 和事件序号，不引入代理消息坐标、嵌套协议或第二套账本。 / Keep native fold markers and event coordinates; no adapter or parallel ledger. |
| 成本归因 / Cost attribution | `billion-context@a392b529d`, `src/cache-ledger.ts:1275,1324,1855,2154`: separate cache effects from fold economics. | 沿用 `src/diagnostics/checks/fold_economics.rs`；本次主要测 CPU/读放大，保持输出一致时不应宣称省 token。 / Retain fold economics; measure CPU and read work, not token savings from unchanged output. |

## 路径一：紧急压缩 / Lane 1: emergency fit

- 保持 `src/context/compact/fit.rs::compact_to_fit` 的顺序：先走已装配的 `ContextFabric` 与 LLM 摘要，随后按现有校准与压力目标复测，最后才调用确定性兜底。复用 `ReasoningReplay::project_message` 和 `estimate_message_tokens_aware`；不影响预防压缩、split 触发器、模型回放策略或 provider 归一化。 / Keep LLM compaction first, the current calibration and pressure target, and deterministic eviction last. Reuse the wire projection and per-message estimator; do not change preventive compaction, split, replay policy or provider normalization.
- 一次算成本与可驱逐单元，按原有「非用户先、同类从旧到新」选择，再一次保序保留；保持原始消息计数定义的 `protected_tail.max(1)`。对已配对的调用和紧随其后的匹配结果应用同一单元（同一 assistant 多调用共享该单元）；单元触碰尾部就保护整个单元，即使预算因此无法再压低。不得生成合成工具错误。原本已孤立的前导结果仍可按旧规则移除，不吃掉受保护消息。 / Estimate once, evict oldest non-user units before oldest user units, then retain once in source order. Protect at least `protected_tail.max(1)` raw messages. Group an existing assistant call message (including parallel calls) with its matching following results and protect the whole unit if it intersects the tail, even if the target cannot be reached. Do not synthesize errors. Keep cleanup of pre-existing leading orphans when outside the protected tail.
- 正常化输入上对照旧策略，除工具轮必须完整保留的情形外，保留消息、顺序和省下的估算 token 一致。无须为不规范历史新增另一层回放修复。移除本路径旧的反复估算与 `Vec::remove` 循环。 / Check parity on valid histories except for the intended pair-boundary fix, including the returned dropped-token count. Do not build another repair layer for malformed histories. Remove the replaced rescan/remove loop.

## 路径二：按页恢复 / Lane 2: paged recovery

```text
session_decompress → existing session event store (one read snapshot)
   → indexed fold markers → canonical list_folds → validate inclusive range
   → validate entire requested span for hard retirement
   → render one page + first omitted renderable seq → existing tool response
```

- 用 `src/session/store.rs` 现有 `(session_id,event_type)` 读两类标记（含退休记录），按 seq 交给 `src/context/compact/folds.rs::list_folds`；保留正式记录覆盖兼容记录、旧 ID、所有者与降级说明。用生产 SQLite 的 `EXPLAIN QUERY PLAN` 验证确实选类型索引；必要时在此查询显式指定既有索引。复用会话与清除边界，不新增注册或更换工具。 / Read both marker kinds, retired included, via the existing type index. Reuse canonical fold derivation; verify the actual SQLite plan and explicitly select the existing index if necessary. Keep session scoping, the tool registration and foreign-owner degraded recovery unchanged.
- 标记选择、范围校验及页读取必须共享同一个 SQLite 读事务/快照；不能只靠跨多次调用的进程 mutex 防御其他连接。事务内不做生命周期通知或外部调用。一次成功读取可线性化为清除之前的状态；若快照已看到 `Retire::From`，决不能返回已擦除内容。 / Use one SQLite read transaction for marker resolution, range validation and page reads; a process-local mutex across separate queries is insufficient against another connection. No notifications or external calls in the transaction. A read may linearize before a concurrent clear but must not return erased content from a post-clear snapshot.
- 保留全请求范围（不是仅第一页）的 hard-retired 拒绝。依据现有语义：退休、正文非空、FTS 镜像缺失；不得把空正文、控制事件或普通 compaction retirement 判为擦除。复用解码与 `event_content_text` 等既有判定；缺镜像表或无法确认时不得回传正文。镜像见证按会话/区间批量求得或有序合并，不能逐事件扫描 UNINDEXED FTS；仅有需检查的退休行时才查询。流式校验选中范围，不为页之外构造整段渲染字符串。 / Preserve full-request-range refusal. A retired row with nonempty content and no FTS mirror is not recoverable; blank/control rows and soft retirement retain existing semantics. Reuse decoder and content classification; fail closed if the witness is unavailable. Use a batched or ordered join/witness read scoped to the session and range, not an FTS scan per event. Skip FTS work when no retired rows require it. Stream validation without collecting or rendering the whole span.
- 保留 `from_seq`/`to_seq` 闭区间、首条超预算仍返回、`next_from_seq` 指向第一条未返回且可渲染的事件、未知 fold 与空 span 行为、无正文说明、恢复生命周期通知。涉及标记或请求范围内的不可解码记录继续报错；范围外无关正文不再被加载，也不再因其损坏阻断恢复。页之外仍可能需解码以保全范围规则，因此不承诺严格的页大小 I/O 上界；单个超长事件仍可能大于 `max_tokens`。 / Preserve inclusive bounds, first oversized event progress, first omitted renderable cursor, unknown/empty semantics and lifecycle feedback. Corruption in fold markers or inside the requested span still fails; unrelated payloads outside are no longer read and do not block valid recovery. Range validation is not page-sized I/O, and the first oversized event exception remains.
- 复用 `SessionEventStore` 作为唯一入口，在同一 store 上提供窄恢复读取；不为生产专用读引入第二条 FTS/存储写链。仅在核对所有调用点后删除旧的恢复专用读取。若外部扩展需要可发现、替换、调用、管理或订阅，沿用现有 Capability；本轮不新增扩展接口。 / Keep the existing event store as the only entry point, with a narrow restore read on that store. No second FTS/write path. Delete replaced recovery-only reads only after checking all callers. Keep externally discoverable, callable, manageable, subscribable and replaceable extension points on Capability; introduce none here.

## 验收与测量 / Acceptance and measurement

- **回归**：先复现跨尾部的工具轮缺失；覆盖多调用、混合用户/工具优先级、reasoning replay、保护区已超预算、无压力输入和返回 token 差值。恢复测试使用真实 SQLite：新旧 fold、退休标记、兼容键、外国所有者、缺镜像、分页空隙、控制行/空正文、首条超限、范围外损坏、页外擦除与两连接快照。 / Reproduce the tail-pair bug before the fix; cover multi-call units, priorities, replay, already-over-budget protected content, no-pressure input and dropped tokens. Exercise real SQLite for fold formats, retired markers, canonical keys, foreign owners, missing mirror, pagination gaps, blank/control rows, oversized first line, out-of-scope corruption, erasure beyond the page and two-connection snapshot consistency.
- **前后对照**：同一合成 fixture、同一构建模式和机器，仅比较有意义的数据；压缩分别记录估算次数和时间，恢复分别记录解码/渲染行数、FTS 查询形状和时间。至少对照小会话与大历史窄页，外加宽范围清除校验；记录样本、P50/P95、以及响应与估算 token 是否改变。未经实测不报告客户负载或缓存收益。 / Use the same synthetic fixture/build mode/machine on baseline and candidate. Report estimator calls and fit time; decoded and rendered rows, FTS query shape and restore time. Include small session, large history/narrow page and wide erase validation. Record sample counts, P50/P95 and output/token parity; do not infer production or cache gains.
- **交付**：按 `docs/reference/DEVELOPMENT.md` 在每次 Cargo 前运行 4 GiB 门；依赖构建串行避免重复大链接；跑受影响测试、工具/装配验证及独立复审。更新双语 `docs/reference/FEATURE_LOCATOR.md` 与相应 reference/routing 说明，以实际代码和验证结果为准；检查 diff、隔离状态并提交，不合并、不推送。 / Run the repository's 4 GiB pre-Cargo gate, avoid duplicate heavy linking, verify focused tests and production wiring, and obtain independent review. Update bilingual locator and reference/routing documentation from actual code and results. Verify diff and isolation, commit without landing or pushing.

实现计划可自行决定内部类型、SQL 批量实现与 benchmark harness；若需要放宽全范围拒绝、修改公共分页契约、迁移数据库或新增 Capability 接口，先重新审批。 / Leave internal types, batched SQL details and benchmark harness to the implementation plan. Reopen approval if whole-range erase refusal, public pagination, schema migration or new Capability interfaces need to change.
