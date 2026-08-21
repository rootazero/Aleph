# Panel 记忆 Tab 第二轮深化设计 (Panel Memory Tab — Round 2 Deepen)

> 2026-08-21。基线：2026-07-26 首轮重构（§6.7）已交付卡片流 / 双轨搜索 / Loadable 三态 / 批量 / 溯源。
> 本轮 gap 来源：MemOS `apps/memos-local-plugin/viewer` 二次勘查 + Aleph 现状全图扫描。
> 首轮「刻意不做清单」（share / 趋势图 / 无限滚动 / 新建笔记按钮 / 手动压缩 / clear 进 Panel）**全部继续生效，不重提**。

## 1. 缺陷与缺口清单（本轮处置）

| # | 症状 | 根因 | 处置 |
|---|---|---|---|
| C1 | 记忆三支柱只呈现两柱：curated 热区（`remember` 写入的 MEMORY.md）RPC 与 Panel 双缺席 | 从未建读写面 | **CONNECT**：`memory.curated.{list,replace,remove}` + Curated facet |
| C2 | `TraceResult.write_decisions` 服务端序列化、客户端 DTO 收不到；`TraceKind` 客户端缺 `WriteDecision` | 客户端 DTO 落后 | **CONNECT**：DTO 补全 + Curated facet 内「写入台账」区 |
| C3 | `memory.retrieve_with_trace` 完整流水线 trace 只有 Settings 一处裸 `rpc_call` | 记忆 tab 未接 | **CONNECT**：「检索透视」面板（对标 MemOS RetrievalFunnel）+ API 包装收敛单源 |
| C4 | `memory.list_corrections` 服务端+API 包装全好、全 Panel 零消费者 | 未接线 | **CONNECT**：Feedback facet 顶部「修正队列」（pending/distilled） |
| C5 | `match_field` 发了、解析了、零渲染 | 未接线 | **CONNECT**：SearchHits 卡「标题命中」chip |
| C6 | `listFacts` offset 服务端支持、客户端恒传 0 → 第 1001 条笔记 Panel 永不可达 | loader 断线 | **FIX**：窗口累载（加载更多），保住 facet 客户端切片架构 |
| C7 | 双抽屉能力不对称：Vault 有溯源缺 outgoing，Galaxy 有 outgoing 缺溯源 | 各自生长 | **重构**：共享 section 组件，两侧补齐 |
| C8 | `validFacts` 恒等 `totalFacts`（notes 无 invalid 态） | 退役 fact 模型残留 | **CUT**：wire + Panel DTO + CLI 行 |
| C9 | `ai_output`/`window_title` 恒空仍在 wire；Q/A 双半 UI/导出是死设计（`raw_memories` 无独立列） | 退役双列模型残留 | **CUT**：字段改 `content` 单列，Panel/CLI 同步 |
| C10 | `SearchParams.window_title` / `ListFactsParams.include_invalid` / `ClearParams` 解析了从不用 | 残留 | **CUT** |
| C11 | `aleph memory clear` 必然失败（后端恒 INTERNAL_ERROR 墓碑） | CLI 未跟首轮裁决 | **CUT**：CLI 子命令 + `memory.clear`/`memory.clearFacts` handler + census（墓碑理由归 git 史） |
| C12 | `dreaming.list_insights` 缺 phase-1 占位（兄弟 `insights.tools` 有）→ boot phase 2 前 METHOD_NOT_FOUND | 漏登记 | **FIX** |
| C13 | Raw 徽标搜索态读 `stats.total_memories`（不带 query）与列表不一致 | 读错源 | **FIX**：徽标读 `raws.total` |
| C14 | Galaxy `graph.query` limit=500，`total` 解析了无截断提示 | 未渲染 | **FIX**：截断提示行 |
| C15 | `drawer.rs` 硬编码 "Confirm delete?"/"Delete"；phone 端记忆五屏大面积硬编码英文 | 首轮遗留 | **FIX**：全走 `t!` |
| C16 | `register_graph_handlers` `_default_agent_id` 死参数 | 残留 | **CUT** |
| C17 | `mod.rs` 910 行再膨胀 | 新功能将进驻 | **拆层**：新功能各自成文件（curated.rs / xray.rs / corrections.rs），mod.rs 只编排 |

## 2. 新 RPC 契约（一个 RPC 一种形状，延续首轮纪律）

### `memory.curated.list` `{agent_id?}`
→ `{entries: [{text, chars}], usage_chars, usage_pct, limit, legacy}`
- `agent_id` 是 **BASE agent id**（默认 `main`）。分区组合由 `MemoryContextProvider::get_or_load_curated_store` 经 ambient scope 自行完成（P1 curated per-scope instancing）——**handler 不得预组合**（`session_write_id` 非幂等，registry 注释已警告）。
- 已组合的 id（含 `__u-`/`__proj-`/`__p-` 后缀）直接拒为不可见分区同形空响应（no oracle）。
- MCP 未注入（无 agent runtime 的裸 gateway）→ INTERNAL_ERROR "curated store unavailable"。

### `memory.curated.replace` `{agent_id?, old_text, content}` / `memory.curated.remove` `{agent_id?, old_text}`
→ `{entries, usage_chars, usage_pct, limit, legacy, message}`（突变后全量快照，Panel 免二次取数）
- 寻址走 store 既有 `match_unique` 子串语义（与 `remember` 工具同源）。
- **错误分类**（§4.13c 三分法，不是一律 INTERNAL_ERROR）：`NoMatch`/`Ambiguous`/`Empty`/`ContainsDelimiter`/`OverBudget` → INVALID_PARAMS（调用方可改）；`Io` → INTERNAL_ERROR。
- 突变成功后必须 `invalidate_curated_for_agent`（冻结的 per-session envelope 快照要驱逐；store 本体是同一 Arc 无需驱逐）。
- **Panel 不提供 add**：新增记忆归 `remember` 工具/对话（R7/R8，与「新建笔记按钮不做」同一论证）；删除/修正是管理面，Panel 笔记抽屉已有先例。
- Panel 侧突变**不落** `memory_write_decisions` 台账：台账回答「模型写入为什么没落地」，人工管理面不属于那一问。

### 注入方式
`register_memory_handlers` 新增 `mcp: Option<Arc<MemoryContextProvider>>` 参数（构造时注入，P4；不用进程级全局访问器）。

## 3. Panel 架构

```
views/memory/
  mod.rs          编排（目标 ≤900 行不再膨胀；新功能不进驻）
  data.rs         + MemoryFacet::Curated + CuratedState 纯函数 + 单测
  loader.rs       + load_curated / load_corrections / 笔记窗口累载
  curated.rs      NEW: Curated facet 卡片 + 用量条 + 就地编辑/删除 + 写入台账区
  xray.rs         NEW: 检索透视面板（stage 漏斗 pill + 结果列表）
  corrections.rs  NEW: Feedback facet 顶部修正队列
  note_sections.rs NEW: 共享抽屉 section（outgoing links w/ status、供 drawer 与 galaxy 复用）
```

- Curated facet 排 facet bar 首位（热区地位）；徽标 = entries 数（独立 Loadable 槽，与 SearchHits/Raw 同模式）。
- 笔记累载：`NOTE_WINDOW=1000` 逐窗追加，`已载入 X / Y · 加载更多`；替换现有 `notes_truncated` 一行死提示。facet 切片/本地过滤/分页架构不变。
- 检索透视：搜索框旁按钮 → 面板内跑 `memory.retrieve_with_trace`（Loadable），stage 逐级 pill（名称+计数+耗时）+ 最终结果行。

## 4. 刻意不做（本轮新增裁决）

- **`ProfileSection` TraceKind 不进 Panel**——USER.md 分区溯源是工具面场景，Panel 加了就是零消费者 UI（R10）。
- **curated 批量操作**——热区预算封顶（字符上限），条目数十级，批量是投机通用化。
- **phone 端 Curated/透视/修正队列**——phone 记忆面保持轻量浏览定位；本轮 phone 只修 i18n 与窗口累载。
- **`memory.stats` 拒绝路径 `totalGraphNodes: 0` vs 正常失败 `null` 的形状差**——刻意保留：拒绝路径要求与「真实空 agent」逐字节同形（no oracle），失败路径要求「数不出来≠零」。两个判据不同回答的是不同问题，handler doc 补一句说明即可。
- **`listFacts` 服务端 category 过滤参数**——facet 切片留在客户端（窗口累载已让全量可达；服务端加 category 参数会把 bucket_counts 变成第二个真源）。
- **raw_memories FTS5 影子表**——首轮 DEFER 继续成立。

## 5. 验证

- 服务端：curated 三 handler 单测（list/replace/remove/不可见分区同形/无 MCP/错误分类）；stats 无 `validFacts` 形状；search 无死字段形状；census 对账。
- Panel：`cargo test -p aleph-panel --lib`（data.rs 纯函数新增：curated 状态、累载合并、修正队列过滤）+ `cargo build -p aleph-panel --lib --target wasm32-unknown-unknown --profile wasm-release`。
- CLI：`cargo test -p aleph-cli`（stats 行删除、search 键改名、clear 子命令删除）。
- 全量：五命令最小验证集 + i18n census + 真机 smoke（记忆 tab 六 facet + 抽屉 + 透视）。
