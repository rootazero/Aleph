# Panel 记忆 Tab 深度重构设计 (Panel Memory Tab — Deep Refactor Design)

- **日期**: 2026-07-26
- **分支**: `worktree-panel-memory-tab-refactor`（基线 `646173c7f`）
- **参考项目**: `/Volumes/TBU4/Github/MemOS/apps/memos-local-plugin/viewer/`（22.7k LOC Preact viewer；`src/views/MemoriesView.tsx` 1474 行是记忆 tab 的直接对位物）
- **涉及红线**: R2（UI 逻辑唯一源）· R3（核心轻量化）· R4（Interface 层纯 I/O）· R7（LLM 主权）· P2（高内聚 / 大文件拆分）· P7（防御性设计）
- **不涉及**: `src/harness/`（R10 零触碰）· 记忆核心算法（`src/memory/notes/*` 的 ingest/dream/link 逻辑一律不动）

---

## 1. 问题陈述

Panel 的记忆 tab（`/memory`，Memory Hub 的 Table 视图）有三类问题叠在一起：

1. **一处结构性重复导致的数据欺骗**：`memory.search` 与 `graph.search` 调用同一个 `db.search_notes_fts()`，但前者把笔记行伪装成 `MemoryEntry` 返回；Panel 把它接进 Raw 表并强制切页签，于是"搜索"会把**笔记文件名**显示成原始对话，且行内删除按钮永久失败而无任何提示。
2. **一批 RPC 字段被静默丢弃 / 一批 RPC 活注册零消费者**：`NoteIndexEntry.tags` / `link_count` / `updated_at` 与 raw 行的 `session_id` 早已从 SQL 查出，handler 却不往下发；`memory.trace`（证据链）与 `graph.neighbors`（316 行）在 Panel 侧零消费者。
3. **误差被静默吞掉**：wide 视图三个 loader 全用 `if let Ok(..)`，任何 RPC 失败都渲染成"空列表"，与真空态不可区分（phone 侧反而有 error + Retry）。

以及一个可维护性问题：`views/memory/mod.rs` 695 行一肩挑 stats / facet / 两张表 / pager / highlight / 搜索提交 6 件事，超过 P2 的 500 行拆分线。

---

## 2. Gap Analysis：memos viewer ↔ Aleph 记忆 tab

| 维度 | memos viewer | Aleph 现状 | 本轮处置 |
|---|---|---|---|
| 列表形态 | 卡片流（1 turn = 1 card）+ meta pill 行 | `<table>` 4–5 列，内容 `line-clamp` | 改卡片流（§6） |
| 搜索 | 200ms debounce，过滤当前列表，不切页签 | Enter 才提交，强制跳 Raw 页签且显示笔记 | 双轨（§5） |
| 筛选 | chips 独立一行 + namespace 下拉 | facet chips 兼任"层切换" | 保留层语义，新增 SearchHits 层 |
| 批量 | 固定 batch-bar：select-page / share / copy-export / delete | 仅 Raw 层有 delete-selected | 升级（§8） |
| 分页 | Pager + page-size 选择器 + total | 固定 50/页，total 取错来源 | 修 + page-size（§8） |
| 加载 / 空 / 错 | skeleton + 三态各带 icon + hint | "Loading..." 文本；**无 error 态** | `Loadable<T>` 三态（§4） |
| 操作反馈 | toast（2.4s 自动消失） | 无 | 模块私有 toast（§8） |
| 深链 | `?q=` + `?id=`（自动翻页定位 + 开抽屉） | 仅 `?view=graph\|table` | `?note=`（§8） |
| 详情抽屉 | 每步可折叠段 + 编辑 / 分享 / 删除 | body 编辑 / 重命名 / 删除 + backlinks | 补溯源证据链（§7） |
| 刷新 | header refresh 按钮 | 无（改完笔记表格不刷新） | 补 refresh + 自动重取（§8） |

**Aleph 领先、本轮不得"对齐"掉的部分**：3D 星系图谱（§6.3）· wikilink 双向导航 · backlinks · 纯函数数据层 + native 单测 · 图 / 表 keep-alive 双视图 · 证据链溯源（memos 无对位物）。

**架构映射而非照搬**：memos 的 card 之所以承载 score/tools/steps/reflection，是因为它的 L1 trace 是 step 粒度；Aleph 的笔记是 note 粒度，对应的"可承载信息"是 tags / link_count / category / 双时间戳 —— 这些**已经在数据库行里**，只是被 handler 丢了。所以卡片化在 Aleph 侧不是新增数据面，而是**停止丢弃**。

---

## 3. 缺陷清单（本轮全部处置）

| # | 症状 | 根因 | 位置 | 处置 |
|---|---|---|---|---|
| B1 | 搜索跳 Raw 页签、显示笔记文件名、删除永久失败零提示 | `memory.search` 非空 query 走笔记 FTS 分支；Panel 当 `RawMemory` 渲染；`memory.delete{id=note_path}` → `Ok(false)` → error 被 `.is_ok()` 吞 | `handlers/memory.rs:120-149`、`views/memory/mod.rs:149-157`、`api/memory.rs:107-159` | FIX（§4.1） |
| B2 | 所有 RPC 失败静默变空列表 | 三个 loader `if let Ok(..)`，无 error 信号 | `views/memory/mod.rs:102-144` | FIX（§4.2） |
| B3 | 切非 default agent 时 4 张统计卡说谎 | `count_all_notes()` 跨 agent；graph 计数硬编码 `DEFAULT_AGENT_ID`；`count_raw_memories()` 全局 | `handlers/memory.rs:362-387` | FIX（§4.1） |
| B4 | Raw 分页幻影页（next 永远可点，翻到空页） | total 取全局 `stats.total_memories`，list 却 agent-scoped | `views/memory/mod.rs:239` | FIX（§4.1，count/list 共用 WHERE） |
| B5 | 桌面搜索框不过滤笔记；`filter_notes` 有实现 + 3 单测，仅 phone 消费 | wide 从未接 | `views/memory/data.rs:75` | CONNECT（§5） |
| B6 | 抽屉 Similarity 区永不显示 | 后端两条分支 `similarity_score` 恒 `None` | `api/memory.rs:20` | CUT（§7.2） |
| B7 | `memory.trace` 证据链活注册、Panel 零消费者 | 仅连了 LLM 工具面 | `handlers/memory.rs:527` | CONNECT（§7.1） |
| B8 | `graph.neighbors` 316 行零消费者 | 消费它的 2D radial 引擎已退役 | `handlers/graph/neighbors.rs` | CUT（§4.3） |
| B9 | `listFacts` 不返 total → 超 1000 条 pager 少报 | | `handlers/memory.rs:296` | FIX（§4.1） |
| B10 | `memory.col_confidence` / `memory.canvas_error_prefix` 死 i18n 键 | | `locales/{en,zh}.json` | CUT |
| B11 | `aleph memory search` 恒打印空表 | CLI 读 `result.as_array()`，后端返 `{"memories":[…]}`；且读 `score`/`content`/`source` 三个后端从不发的键 | `interfaces/cli/src/commands/memory_cmd.rs:19-42` | FIX（§4.4） |
| B12 | `aleph memory stats` 每行恒 `-` | CLI 读 `total_facts`/`total_sessions`/`total_graphs`/`storage_size`/`last_compressed`，后端发 camelCase `totalFacts`/`totalMemories`/`validFacts`/`totalGraphNodes`/`totalGraphEdges` | `interfaces/cli/src/commands/memory_cmd.rs:44-93` | FIX（§4.4） |

B11/B12 不是范围扩大：本轮要改的正是这两个响应体，留着客户端读幻影字段等于制造新的谎言。

---

## 4. 服务端：一个 RPC 一种形状

### 4.1 RPC 契约变更

| RPC | 改后职责 | 变更 |
|---|---|---|
| `graph.search` | **笔记 FTS 唯一入口** | `SearchResultDto` 追加 `agent_id` / `created_at` / `updated_at` / `tags` / `link_count`（全部已在 `NoteIndexEntry`，零新查询）；`match_field` 保留 |
| `memory.search` | **原始对话唯一入口**（浏览 + 内容过滤） | 删除笔记 FTS 分支；`query` 改为 raw 内容 `LIKE` 过滤；`MemoryEntry` 追加 `session_id`（SQL 已 SELECT）；`window_title` 恒空字符串的写法保持不变（不新增语义） |
| `memory.listFacts` | 笔记列表 | 响应追加 `total`；`FactEntry` 追加 `tags` / `link_count` / `updated_at` |
| `memory.stats` | 统计 | 参数追加 `agent_id: Option<String>`；提供时做 agent-scoped 计数（笔记 / raw / 图节点 / 图边），响应追加 `scope: "agent" \| "global"` |
| `graph.neighbors` | — | CUT（§4.3） |

**兼容性**：所有追加字段都是新增键，旧客户端 serde 忽略即可。唯一的行为变更是 `memory.search` 带 query 时不再返回笔记 —— 全仓消费者只有 Panel（本轮改）与 CLI（本轮修，且原本就读不出来）。

### 4.2 store 层：count 与 list 共用 WHERE

`src/memory/store/sqlite/mod.rs` 现状：`get_raw_memories_dashboard` 按 agent 存在与否分两条 SQL 字面量，`count_raw_memories` 无 agent 参数 —— B4 幻影页正源于两者可以漂移。

改为一个纯函数做单一真源：

```rust
/// 构造 raw_memories 的 WHERE 子句与绑定值。
/// count 与 list 共用，两者不可能漂移。
fn raw_where(agent_id: Option<&str>, query: Option<&str>) -> (String, Vec<String>)
```

- 基线恒含 `source != 'tool_invocation'`（与现有 dashboard 语义一致，排除工具遥测行）
- `agent_id` 存在 → `AND agent_id = ?`
- `query` 非空 → `AND content LIKE ?`（值为 `%q%`，`%` / `_` / `\` 需转义并配 `ESCAPE '\'`）
- 绑定用 `rusqlite::params_from_iter`

签名同步为：
- `get_raw_memories_dashboard(agent_id: Option<&str>, query: Option<&str>, limit, offset)`
- `count_raw_memories(agent_id: Option<&str>, query: Option<&str>)`

生产调用方只有 `handlers/memory.rs`（各 1 处）+ `sqlite/raw_memories.rs:861` 一个测试断言，改动面极小。

**不为 raw 建 FTS 表**：`raw_memories` 无 fts5 影子表，本轮是浏览 UI 的子串过滤，`LIKE` 足够；建 FTS 是独立特性（涉及 DDL 迁移 + 触发器），超出本轮范围。此取舍写入 handler doc comment，避免后人以为漏了。

### 4.3 CUT graph.neighbors

删除点（全仓已核对，无其它引用）：
- `src/gateway/handlers/graph/neighbors.rs`（316 行）
- `src/gateway/handlers/graph/mod.rs:16` 的 `pub use neighbors::handle_neighbors_impl;` 与 `mod neighbors;`
- `src/gateway/handlers/graph_types.rs`：`GraphNeighborsParams` / `default_depth` / `default_neighbor_limit`（第 15–31 行）与 `GraphNeighborsResponse`（第 186 行起）
- `src/bin/aleph-server/commands/start/builder/handlers/agents.rs:582` 的注册块

**保留** `NoteStore::get_neighbors`（trait + sqlite impl）—— `src/builtin_tools/note_graph_query.rs:254` 是活消费者。

### 4.4 CLI DTO 对齐

`interfaces/cli/src/commands/memory_cmd.rs`：
- `search`：从 `result["memories"]` 取数组；列改为 `Date` / `Agent` / `Content`（对齐 `MemoryEntry` 真实字段 `timestamp` / `agent_id` / `user_input`）
- `stats`：改读 camelCase `totalFacts` / `totalMemories` / `validFacts` / `totalGraphNodes` / `totalGraphEdges`；`storage_size` / `last_compressed` 两行删除（后端从不产出，属幻影行）

---

## 5. 搜索语义：双轨

```
键入 ──200ms debounce──→ 笔记层: filter_notes(当前层切片)     本地即时（接上 B5 的孤儿函数）
                         Raw 层 : memory.search 的 LIKE 过滤（服务端，随 page 重取）

Enter (search_nonce++) ─┬─→ canvas graph.search（图高亮，行为完全不变）
                        └─→ 表视图切到 MemoryFacet::SearchHits，
                            渲染 graph.search 命中为**笔记卡**

MemoryFacet::Raw ──────────────→ 永远只装真正的原始对话
```

**实现约束**：
- debounce 用 `leptos::leptos_dom::helpers::set_timeout_with_handle`（`views/agent_trace.rs:162` 已有同款用法，带取消句柄，不引新依赖）；组件卸载与下一次键入都要 `clear()` 旧句柄
- `MemoryState.search_query` / `search_nonce` 契约**不变**（星系图 `canvas/mod.rs:298-310` 零回归）
- `MemoryFacet` **不改名**（phone 侧 `platform/phone/memory/{cell,list,mod}.rs` 共用），只加 `SearchHits` 变体：
  - `is_notes()` → `true`（笔记形状）
  - `facet_slice()` → 返回空（数据来自独立的 search-hits 信号，不从 window 切）
  - `bucket_counts()` 不受影响（仍是 `[AllNotes, Facts, Feedback, Lessons]` 四元组）
  - phone 侧 `FACETS` 常量只列前四项，`SearchHits` 不进 phone chips —— 无需改动 phone
- SearchHits chip **仅在有命中或搜索进行中时出现**，空搜索不留残留 chip

---

## 6. Panel 架构：模块拆分与卡片形态

### 6.1 目标文件布局

```
interfaces/webchat/src/platform/wide/views/memory/
├── mod.rs         ~200  编排：loader Effects 装配 + 布局 + 事件路由
├── data.rs        ~400  纯数据层（native 单测，无 leptos）：
│                        MemoryFacet(+SearchHits) / Loadable<T> / fact_facet /
│                        bucket_counts / facet_slice / filter_notes / page_slice /
│                        page_count / locate_note / format_ts / notes_to_markdown /
│                        raws_to_markdown
├── loader.rs      ~150  三条取数收敛成 Loadable：notes / raw / search-hits
├── facets.rs      ~120  层 chips（含条件出现的 SearchHits chip）
├── cards.rs       ~280  NoteCard / RawCard / CardList + skeleton / empty / error 三态
├── batch_bar.rs   ~120  计数 / select-page / copy-MD / delete / clear
├── pager.rs       ~120  Pager + page-size (25 / 50 / 100)
├── toast.rs       ~70   模块私有 toast 栈
├── drawer.rs      ~350  （从 426 瘦身，抽出 provenance）
└── provenance.rs  ~150  memory.trace 证据链区
```

每个文件均 ≤ 400 行，满足 P2。

**toast 放模块内而非 `components/ui/`**：现有两处"toast"（`settings/channels/config_template.rs`、`components/extensions/install_flow.rs`）其实是内联 banner，形状不同；提前抽公共组件属投机通用化（P6 / YAGNI 撤回模式）。第二个真消费者出现时再上提。

### 6.2 核心类型：让"静默空列表"不可表示

```rust
/// 一次取数的三态。取代 `(loaded: bool, data: T)` 二元组 +
/// `if let Ok(..)` 吞错的写法 —— 卡片列表 match 三臂穷尽，
/// 编译器保证 error 态必须画出来（B2 的根治，而非补 UI）。
#[derive(Debug, Clone, PartialEq)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}
```

对齐 `rules/rust/patterns.md` 的 enum 状态机与 P7 防御性设计。三个 loader 全部返回 `Loadable`。

### 6.3 卡片形态

笔记卡（左侧色条来自现有 `canvas_engine::category_color`）：

```
┌──────────────────────────────────────────────────────────────┐
│▎☐  Deploy 流程要先跑 smoke test                          [⇱] │
│    facts/deploy-notes.md                                     │
│    [facts] [main] [rust] [ci]  🔗 3   建 2026-07-20 · 改 07-24│
└──────────────────────────────────────────────────────────────┘
```

Raw 卡：

```
┌──────────────────────────────────────────────────────────────┐
│ ☐  Q 这个 pager 为什么会翻到空页                        [🗑] │
│    A 因为 total 取的是全局计数…                              │
│    [main]  session s-77   2026-07-24 14:02                    │
└──────────────────────────────────────────────────────────────┘
```

- `[⇱]` = 在图中定位（复用现有 `on_locate` → `mem.selected_node` + 切 Graph 视图）
- `tags` / `link_count` / `updated_at` / `session_id` 全部来自 §4.1 的字段直通
- Q/A 分段渲染：`memory.search` 的 `user_input` / `ai_output` 分别成段，取代现在 `api/memory.rs` 里 `format!("Q: {}\nA: {}")` 的字符串拼接（拼接后前端无法分别样式化）
- 三态：`Loading` → 5 条 `animate-pulse` 骨架条（无需新组件，Tailwind 内置）；`Failed` → 警示 icon + 错误文 + Retry 按钮；`Ready(空)` → icon + hint

### 6.4 被取代后删除的旧代码

- `views/memory/mod.rs` 内的 `NotesTable`（~100 行）/ `RawTable`（~110 行）/ `RawRow`（~55 行）/ `Pager`（~45 行，移入 `pager.rs` 并扩展）
- `views/memory/facets.rs` 的 `FacetChip` / `FacetBar` 迁入新 `facets.rs` 并扩展（非重写）

---

## 7. 溯源证据链与 similarity 死链

### 7.1 CONNECT memory.trace

抽屉在 backlinks 之下新增一区：

```
▾ Evidence chain (3)
  • raw#a1b2c3 · session s-77 · via facts/deploy-notes.md
    「先在 18790 上灌一次 smoke test 再 deploy…」      ← 点击展开前 800 字
  • raw#c3d4e5 · session s-81
  • raw#f6g7h8 · pruned（引用存在但行已被清理）
```

- 笔记抽屉 → `memory.trace {kind:"note", target: path}`（往下走到 ground truth）
- Raw 抽屉 → `memory.trace {kind:"raw", target: id}`（往上走到引用它的笔记）
- 响应形状即 `builtin_tools::memory_trace::TraceResult { target, notes, evidence[] }`，`EvidenceItem` 含 `raw_id` / `via_note` / `via_session` / `content` / `pruned`
- `pruned: true` **显式标注**，不伪装成"没有证据"
- 该区自身也用 `Loadable`：trace 失败显示错误而非空区
- `max_results` 传 20（有上限就必须说出来：达到上限时显示"仅显示前 20 条证据"，不做静默截断）

### 7.2 CUT similarity 死链

`MemoryEntry.similarity_score` 在后端两条分支均恒 `None`；改 raw 搜索为 `LIKE` 后语义上更不存在相似度分。删除：
- `api/memory.rs` 的 `RawMemory.similarity` 字段与 `BackendMemoryEntry.similarity_score`
- `drawer.rs` 的 similarity 展示区
- `locales/{en,zh}.json` 的 `memory.similarity`

真需要向量分应走 `memory.rerank`（独立特性，本轮不做）。

---

## 8. 深链 / 批量 / 刷新

- **深链**：`?view=table&note=facts/deploy-notes.md`
  - `?view=` 仍由 `memory_hub/mod.rs` 的 Effect 消费（不动）；`?note=` 由 `views/memory/mod.rs` 自己的 Effect 消费 —— 两个参数各有唯一读者，不互相覆盖
  - 复用现有 `locate_note()` 求 `(facet, page)` → 自动翻页 + 高亮 + 开抽屉
  - 窗口外（超 `NOTE_WINDOW`）时退化为直接 `graph.node_detail` 开抽屉，而非现在只丢一句 `highlight_not_in_window`
  - 消费后清空 `?note=`（`replaceState`，镜像 `context.rs::scrub_credentials_from_url` 的做法），否则刷新页面会反复强开抽屉
  - 卡片 hover 出现"复制链接"，写 `navigator.clipboard`（`chat/messages.rs:736` 已有同款调用）
- **批量**：
  - batch-bar 置于列表**上方**（memos 的教训写在其 `MemoriesView.tsx:44-48`：固定底栏会盖住 pager、小屏上 select-all 不可达）
  - 笔记层也可多选（现在只有 Raw 层能选）
  - `Copy as Markdown` 导出选中项为 `# 标题 / path / 正文`。笔记正文需逐条 `graph.node_detail`，因此：**上限 50 条**（超出时按钮禁用并显示"一次最多导出 50 条"——有上限就必须说出来，不静默截断）；导出中显示 `n/N` 进度；取不到正文的条目在输出里写 `<!-- body unavailable: {err} -->` 并在 toast 里报数（不静默丢）
  - 删除仍走现有 `ConfirmButton` 二段确认，**不用 `window.confirm`**（R5 不弹阻塞模态；与 kanban `ConfirmMoveDialog` 同规矩）
  - 笔记层删除走 `graph.delete_note`，Raw 层走 `memory.delete` —— 两条路径不可混用（B1 的教训）
- **刷新**：header 加 refresh 按钮；笔记编辑 / 重命名 / 删除后自动重取窗口（现在只有星系图有 `graph_nonce` 失效机制，表格改完不刷新）
- **toast**：删除 / 保存 / 重命名 / 复制成功或失败均出 toast（2.4s 自动消失）

---

## 9. 熵减清单

| 项 | 净行数 |
|---|---|
| CUT `graph.neighbors` 全链（handler + 两个 DTO + 三个 const + re-export + 注册点） | −~360 |
| CUT `NotesTable` / `RawTable` / `RawRow`（被卡片取代） | −~265 |
| CUT `similarity` 死链（字段 + UI 区 + i18n 键） | −~20 |
| CUT 死 i18n 键 `memory.col_confidence` / `memory.canvas_error_prefix` | −2 |
| CUT CLI 幻影行 `storage_size` / `last_compressed` | −~16 |
| `filter_notes` 从"仅 phone 消费"变为两端共用 | ±0（消灭孤儿） |
| `get_raw_memories_dashboard` 两条 SQL 字面量 → 一个 WHERE 构造器 | −~10 |

---

## 10. 刻意不做（勿重提）

- **memos 的 share / 公开分享**（`ShareScopePill` / `bulkShare` / `?scope=`）—— Aleph 无 hub sharing 模型，移植它等于凭空造一个权限面
- **分析趋势图 / sparkline**（memos `AnalyticsView` + `ActivityDashboard`）—— `insights.tools` 与 `dreaming.list_insights` 已在 Settings → Memory 里有落点（`settings/memory.rs::DreamInsightsPanel`），记忆 tab 再造一份是双源
- **无限滚动** —— memos 自己已从无限滚动退回分页，并在注释里写明原因（batch-bar 不可达）
- **笔记层"新建笔记"按钮** —— 笔记生命周期归 dream daemon 与 `note_manage` 工具（R7 / R8）；抽屉底部现有的 `note_lifecycle_managed` 说明保留
- **为 `raw_memories` 建 fts5 影子表** —— 需要 DDL 迁移 + 同步触发器，是独立特性；本轮浏览 UI 的子串过滤 `LIKE` 足够
- **把 toast 抽成 `components/ui/toast.rs`** —— 现有两处是内联 banner 形状不同，提前抽属投机通用化；第二个真消费者出现再上提
- **`memory.clear` / `memory.clearFacts` 接进 Panel** —— 两者后端都是显式 `INTERNAL_ERROR`（笔记模型不支持批量清空，且注释说明了为何曾伪造成功）；Panel 不该给一个必然失败的按钮
- **`memory.appList` 接进 Panel** —— 后端恒返空数组（window-anchored memory 是 pre-notes 概念）
- **`memory.compress` 手动触发按钮** —— 压缩由 `CompressionService` 后台调度；手动按钮会与调度器双驱动
- **改 `MemoryState` 的 `search_query` / `search_nonce` 契约** —— 星系图与表视图共用，改契约会同时回归两侧

---

## 11. 验证

**纯函数单测（`data.rs`，host target —— 已实测 `cargo test -p aleph-panel --lib` 可在 host 跑，记忆相关基线 17 测全绿）**
- `Loadable::from_rpc(Result<T, String>) -> Loadable<T>`：`Ok` → `Ready`、`Err` → `Failed(msg)` 且**错误文被保留**（这是 B2 的可测断言点：旧代码把 `Err` 映射成"空"，新代码必须映射成 `Failed`）
- `SearchHits` facet 语义：`is_notes()` 为 true、`facet_slice()` 返空、`bucket_counts()` 仍是四元组不受影响
- `filter_notes` 已有 3 测保留；新增"`facet_slice` → `filter_notes` → `page_slice` 三段组合后与直接过滤等价"的组合测
- `notes_to_markdown` / `raws_to_markdown`：导出格式、50 条上限、`<!-- body unavailable -->` 标注
- `locate_note` 已有测保留

**服务端单测**
- `raw_where(agent, query)` 四种组合的子句与绑定值；`LIKE` 元字符（`%` / `_` / `\`）转义
- `count_raw_memories` 与 `get_raw_memories_dashboard` 在同一 `(agent, query)` 下的一致性（B4 回归）
- `handle_stats` 带 `agent_id` 时四项计数均 agent-scoped 且 `scope == "agent"`；不带时 `scope == "global"`（B3 回归）
- `handle_list_facts` 回传 `total` == 该 agent 全量笔记数（不受 limit/offset 影响）（B9 回归）
- `handle_search` 带 query 时**只返回 raw 行、绝不返回笔记**（B1 回归）
- `graph.search` 响应含新增五字段

**编译 / 静态检查**
- `cargo check -p aleph-panel --target wasm32-unknown-unknown`
- `cargo test -p aleph-panel --lib`（`data.rs` 纯函数走 native）
- `cargo test -p alephcore --lib`
- `cargo check --bin aleph-server`（CLI / handler 注册点不在 `--lib` 里）
- `cargo clippy --all-targets -- -D warnings`
- `rustfmt --edition 2021 <逐文件>`（**不跑无作用域 `cargo fmt`**：baseline 非 fmt-clean，会 churn ~70 个无关文件）

**i18n**
- `locales/en.json` 与 `zh.json` 对称；新增约 30 键，删除 3 键（`col_confidence` / `canvas_error_prefix` / `similarity`）

---

## 12. 影响面清单

**服务端（`src/`）**
- `src/gateway/handlers/memory.rs`（search / list_facts / stats 三个 handler）
- `src/gateway/handlers/graph/search.rs`（DTO 字段直通）
- `src/gateway/handlers/graph/mod.rs`（去 neighbors）
- `src/gateway/handlers/graph/neighbors.rs`（删）
- `src/gateway/handlers/graph_types.rs`（`SearchResultDto` 扩字段；删 neighbors DTO）
- `src/memory/store/sqlite/mod.rs`（`raw_where` + 两个签名）
- `src/memory/store/sqlite/raw_memories.rs`（测试断言跟签名）
- `src/bin/aleph-server/commands/start/builder/handlers/agents.rs`（去 neighbors 注册）

**Panel（`interfaces/webchat/`）**
- `src/api/memory.rs`（DTO：加 `tags`/`link_count`/`updated_at`/`session_id`/`total`/`scope`；删 `similarity`；Q/A 不再拼接）
- `src/api/graph.rs`（`SearchResultDto` 对应字段 + 无新 RPC）
- `src/platform/wide/views/memory/`（10 文件，见 §6.1）
- `src/platform/wide/views/memory_hub/sidebar.rs`（搜索框接 debounce 提示文案）
- `locales/{en,zh}.json`

**CLI（`interfaces/cli/`）**
- `src/commands/memory_cmd.rs`（B11 / B12）

**不动**：`src/harness/`（R10）· `src/memory/notes/{ingest,dreaming,links}`（记忆算法）· `platform/phone/memory/*`（复用数据层，`SearchHits` 不进 phone chips）· `views/canvas/*`（`search_query`/`search_nonce` 契约不变）

---

## 13. 文档后续（实施完成后）

- `docs/reference/FEATURE_LOCATOR.md`
  - 新增 §6.7「记忆 Vault 面板 (Memory Vault Panel)」条目 + 表格行
  - §6.3 的 `graph.neighbors` ⚠️ 段落改为「已 CUT（2026-07-26）」，附录 A #10 同步结案
  - Context 段落里 `memory.trace` 的「✅ 已连(2026-06-27)」补注「工具面 2026-06-27 / Panel 面 2026-07-26」
- `docs/reference/MEMORY_SYSTEM.md`：补 Panel 呈现面与 RPC 形状对照
- `CLAUDE.md`：若无新红线则不动（本轮不设新红线）
