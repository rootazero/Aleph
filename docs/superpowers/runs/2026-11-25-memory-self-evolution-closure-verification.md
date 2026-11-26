# Memory self-evolution closure: verification / 记忆自我进化闭环验收

Baseline / 基线：`6f08b8f5de72e1d0a6ba26cea45de70508413cea`. All product changes are on `memory-self-evolution`; the baseline checkout was used read-only. / 产品改动仅在隔离分支；基线检出仅用于只读复核。

Design and comparison / 设计与参考项目对照：[closure design](../specs/2026-11-25-memory-self-evolution-closure-design.md) · [implementation plan](../plans/2026-11-25-memory-self-evolution-closure.md).

## Observed boundaries / 实际作用面

| Boundary / 边界 | Verification / 验证 |
|---|---|
| Scoped reflection / 有作用域的反思 | The assembler tests exercise reranking and fallback through the real reflector. Real notes with the same path in different corpora produce separate owner rows, even when the model cites none; a failed owner transaction does not prevent a different owner's write. Raw and synthetic context remain citable without inflating note exposure. / 重排、降级都进入真实反思器；跨语料同名笔记分别入账，无引用仍记录曝光，一组写入失败不吞掉其他归属。 |
| Dream readiness / Dream 就绪性 | Seeded prior run records survive a missing provider, embedder, or both. Scheduled entry skips; forced and mounted RPC entry return an error. Protocol compatibility and the CLI renderer are checked separately from Core admission. / 缺提供商、嵌入模型或两者都缺时，定时跳过、手动及已挂载 RPC 拒绝；原运行记录不变。共享协议兼容和 CLI 文案独立验收。 |
| Correction consumption / 纠正消费 | The real correction tool writes the raw record; the project gate admits it; distillation creates a recallable note; the existing correction RPC reports the settled row without losing scope. Same-second pages, late appends with older timestamps, explicit skips, malformed coverage, corrupt state and external-byte conflicts are tested separately. / 真实纠正工具写入、项目准入、后继笔记召回、纠正列表状态串成一条线；另验同秒分页、回填、显式跳过、错误覆盖、坏状态、外部冲突。 |
| Restart and replacement / 重启与替换 | Reopening real stores checks the same prepared plan on each side of publication, receipt persistence and cursor advancement. The replacement suite additionally reopens six file/index interruption states and checks archive bytes, successor FTS, and removal of the original from disk and recall. / 真实存储重开分别验证发布、回执、游标边界；替换覆盖六个中断状态，核对归档字节、后继全文检索、原文与原索引退役。 |

## Gates / 门禁

The integrated product was verified at `41b38ac2b`. Core tests used isolated `HOME` and `ALEPH_HOME`, one build job, and two test threads. / 集成产品验证点为 `41b38ac2b`；Core 使用隔离的 `HOME`、`ALEPH_HOME`，单编译任务、双测试线程。

| Gate / 门禁 | Result / 结果 |
|---|---|
| Core `memory::` | 1,602 passed, 0 failed, 5 ignored; exact baseline: 1,581 passed, 0 failed, 5 ignored. / 1,602 通过、0 失败、5 忽略；精确基线 1,581 通过、0 失败、5 忽略。 |
| Gateway Dream and Memory handlers / 网关 | 11 Dream and 42 Memory tests passed. / Dream 11 项、Memory 42 项通过。 |
| Scoped builtin and config / 内置工具与配置 | 13 correction-tool tests, 43 note-management tests, and 1 quorum-boundary regression passed. / 纠正工具 13 项、笔记管理 43 项、quorum 边界回归 1 项通过。 |
| Protocol `dreaming::` | 3 passed / 3 项通过。 |
| CLI memory command tests | 10 passed, including separate readiness and last-run rendering. / 10 项通过，含就绪原因与上次运行分别呈现。 |
| Client compilation / 客户端编译检查 | `cargo check --locked -p aleph-cli -p aleph-panel` and `cargo check --locked -p aleph-panel --target wasm32-unknown-unknown` passed. / CLI、Panel 原生编译检查及 Panel WASM 编译检查均通过。 |
| `just check-wiring` | Nonzero on both exact baseline and candidate. The RPC audit output is byte-for-byte identical: `first`, `second`, and `unanswered` in `shared/client/src/connection/local_ipc_tests.rs`. Each other wiring script was run separately because the aggregate stops at the first failure. The candidate doc wiring audit has 0 failures (baseline named obsolete `sandbox_override`); the other 13 scripts pass. / 两边整体验证均因原有 RPC 审计失败，三条记录逐字节一致。独立运行其余门禁：文档连线 0 失败（基线另有旧符号），其余 13 项通过。 |
| Formatting and docs / 格式与文档 | Targeted `rustfmt --check`, `git diff --check`, and `qa/browser_live/check_docs.py` in self-test and normal modes passed; the latter checked 7 files. / Rust 格式、差异检查、文档自测及正常模式通过；文档正常模式检查 7 个文件。 |

Independent review checked provenance, publication order, and receipt/cursor consistency. A review finding showed that a quorum larger than the effective 512-row page could never admit a batch. Config validation now rejects an unreachable quorum using the same limit as selection; the targeted test covers ordinary and capped lookbacks. / 独立审查复核归属、发布顺序及回执与游标一致性，并发现 quorum 超过实际页容量时永远无法准入；现由配置校验与运行时共享容量，回归覆盖普通与触顶两种窗口。

## Limits / 未覆盖

- A historical correction already covered by the old timestamp watermark is not replayed. The frozen boundary and append order apply prospectively. Ratcheting the cursor does not imply that a note was created: a skip, a gate rejection or a verified no-op can settle an item. / 已被旧水位覆盖的历史纠正不回放。冻结边界与追加序面向后续；结算包括跳过、门禁拒绝与无改动。
- Markdown and SQLite are separate stores. The receipt covers required note visibility, not optional vector/link projection. File conflicts stop the batch for inspection; external editors do not participate in Core's lock. Directory fsync was exercised on Linux only. / 正文与 SQLite 不是同一事务；回执不包含可选向量或链接。冲突留待排查，外部编辑器不参与锁；目录 fsync 只在 Linux 验证。
- The client compile checks and CLI renderer test do not prove browser rendering or styling. / 客户端编译检查与 CLI 渲染测试不证明浏览器呈现或样式。
- No live-model quality benchmark, production migration rehearsal, power-loss experiment, automatic rollback, or cross-store exactly-once guarantee. / 未做真实模型质量基准、生产迁移演练、断电实验；不引入自动回滚，也不承诺跨存储恰好一次。
