# Panel 记忆 Tab 第二轮 — 实施记录

Spec: [2026-08-21-panel-memory-round2-design.md](../specs/2026-08-21-panel-memory-round2-design.md)
分支：`worktree-panel-memory-deepen`（基 `34b9fbacc`）· 实施提交 `5fc906d56`

## 交付对照（spec §1 缺陷表）

| # | 处置 | 落点 |
|---|---|---|
| C1 curated 双缺席 | ✅ CONNECT | `src/gateway/handlers/memory_curated.rs`（3 handler + 9 测试）· Panel `memory/curated.rs` · facet 首位 |
| C2 write_decisions 收不到 | ✅ CONNECT | `TraceKind::WriteDecision` + `TraceResult.write_decisions` + `WriteDecisionRow`（客户端）· Curated ledger |
| C3 检索透视 | ✅ CONNECT | `memory/xray.rs`（`funnel_scale` 3 测试）· `MemoryConfigApi::retrieve_with_trace` 收敛 Settings 的裸 `rpc_call` |
| C4 修正队列零消费者 | ✅ CONNECT | `MemoryApi::list_corrections` + `memory/corrections.rs`，挂 Feedback facet |
| C5 `match_field` 零渲染 | ✅ CONNECT | `CompressedFact.match_field` + NoteCard chip |
| C6 笔记 offset 恒 0 | ✅ FIX | `loader::load_more_notes` + `merge_note_page`（4 测试）+ 载入/总数行 |
| C7 双抽屉不对称 | ✅ 重构 | `memory/note_links.rs`，galaxy 与 drawer 共用 |
| C8 `validFacts` | ✅ CUT | wire + Panel DTO + CLI 夹具 + 源码级守卫 |
| C9 `ai_output`/`window_title` + Q/A 死设计 | ✅ CUT | `MemoryEntry.content` 单列；卡片、导出、CLI 同步 |
| C10 三个死参数 | ✅ CUT | `SearchParams.window_title` / `ListFactsParams.include_invalid` / `ClearParams` |
| C11 `aleph memory clear` 必然失败 | ✅ CUT | 两 handler + 注册 + census + banner 行；CLI 本地解释、非零退出 |
| C12 `dreaming.list_insights` 缺占位 | ✅ FIX | `handlers/mod.rs` phase-1 占位 |
| C13 Raw 徽标搜索态不一致 | ✅ FIX | 徽标改读 `raws.total` |
| C14 星系截断提示 | ⚠️ **报告不成立** | badge 一直存在；真缺陷是硬编码英文，已随 C15 修 |
| C15 硬编码英文 | ✅ FIX | drawer 2 处 + phone 9 处 + galaxy badge（15 个新 key，en/zh 对称） |
| C16 死参数 `_default_agent_id` | ✅ CUT | `register_graph_handlers` |
| C17 mod.rs 拆层 | ✅（部分） | 四个新功能各自成文件（`curated.rs` 392 · `xray.rs` 243 · `corrections.rs` 114 · `note_links.rs` 96），`LoadMoreNotes` 归 `pager.rs`；`mod.rs` **910 → 980**：三处新编排的净增，不是重复代码。**没有**做到 spec 里写的「≤900 不再膨胀」——如实记下而不是继续凑数字 |

## 明确未做（本轮范围外，非遗忘）

- **漏斗不标注「为什么掉」**：`StageTrace` 只有 `{name, duration_ms, input_count, output_count}`，掉落数是 `input - output`。给每一级贴一个原因（「低于阈值」「去重」）会是 Panel 在**发明一套检索器并不具备的词汇**——MemOS 的 `RetrievalFunnel` 能那么做是因为它的后端逐项发了 `droppedByLlm` / `identifier rejected` 这些具名计数，Aleph 的没有。要做那种漏斗，先加服务端的具名计数，别在客户端猜。
- **`ScoreSnapshot`（每级的逐条分数）仍零渲染**：DTO 有、服务端在 traced 模式下填得出，但一屏漏斗要的是「掉了多少」不是「每条多少分」。第二个真消费者出现再接。
- **phone 端 Curated / ledger / 修正队列**：phone 记忆面保持轻量浏览定位（spec §4 已裁）。phone 的笔记窗口仍是单次 `NOTE_WINDOW`，累载只做在 wide 端。
- **`PhoneShell::back_label` 的 i18n**：类型是 `&'static str`，21 个手机屏全传英文字面量 —— 这是 crate 级的类型改动，不是记忆 tab 的修复。已在代码注释里写明，不是漏做。
- **真机 QA**：本轮**零真机验证**。所有断言来自单测 + 编译（含出厂 wasm 形态）。curated 三动词、累载按钮、修正队列、ledger 的真机行为**未经浏览器验证**。

## 验证

| 命令 | 结果 |
|---|---|
| `cargo test -p alephcore --lib` | 16577 passed / **12 failed** —— 与基线 `34b9fbacc` **逐条相同**（另建 baseline worktree 实测比对，见下） |
| `cargo test -p aleph-panel --lib` | 1051 passed / 0 failed |
| `cargo test -p aleph-cli` | 194 passed / 0 failed |
| `cargo test -p alephcore --features test-helpers --test '*' --no-run` | 编译通过 |
| `cargo check --bin aleph-server` | 通过 |
| `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release` | 通过（出厂形态） |
| `cargo clippy -p aleph-panel --all-targets` | 零警告 |
| `cargo clippy -p alephcore --all-targets` | 4 条警告，全在 `loop_graph/`（未触碰，先于本分支） |
| i18n census + locale 对称 | 通过（en/zh 键集相等） |

**基线比对方法**（值得记的部分）：`--lib` 在 HEAD 上本来就有 12 条红，所以「13 条红」这个读数单独看毫无信息。开了一个 `34b9fbacc` 的 baseline worktree 跑同一条命令，`comm` 出差集 —— **恰好一条是我的**（`two_members_share_one_room_memory_and_nobody_elses`，读的是我改名的 `user_input`，行本身正确）。改测试后差集为空。

**变异证明**：`an_empty_write_decision_target_lists_the_recent_ledger` 手工破坏过一次。第一次变异（删掉 store 侧 `filter(|f| !f.is_empty())`）**没有变红**——因为 `LIKE '%%'` 本就匹配全部，那是个语义 no-op；换成 handler 侧拒空 target 才红，并点名文件行号。

## 值得记住的判据（本轮新增/复现）

1. **一个 handler 若自己组合分区，它就是第二个真源**。curated 三 handler 一行组合都不做，把 base id 交给工具走的同一个函数 —— 于是「读写同一个文件」按构造成立。反面：`session_write_id` 非幂等，组合两次落到幽灵分区。
2. **拒绝形状要连 `limit` 一起对齐**。少对齐一个字段，「被拒」就能与「空的」区分开。
3. **census 守卫只认它被教过的注册形状**。`for (method, verb) in [...]` 的动态注册让扫描器失明，它当场报错。修法是**换成它认得的形状**（三处字面量 `.register("…")`），不是给守卫加豁免。
4. **一个「报告中的 gap」要先回代码确认**（C14 不成立）；一个「基线里的红」也要先确认是不是自己造的（差集比对）。
5. **变异没变红，先怀疑变异**（`LIKE '%%'`），不是先怀疑守卫。
