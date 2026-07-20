# Note Wiki-Link 生命周期与关联连线深化 — Design Spec

- **日期**: 2026-07-04
- **状态**: 已与用户逐节确认（4 节全过）
- **前置**: 2026-06-26 note relationship network wiring（merge `f2638ee2d`）已接通 typed/directed/weighted 边、backlinks/结构强边 surface、MinHash 相似边、orientation 健康行。本 spec 在其上深化。
- **参考**: codebase-memory-mcp（置信度解析链 / 悬空一等公民 / 增量入边重连）、SkillOpt-Sleep（门控整合纪律）、hermes-agent holographic（别名归一化 / trust×衰减，后者降级 backlog）

---

## 0. 北极星与体检结论

**北极星**: wikilink 是 note 层的"关联神经"。写入/解析/图谱物化核心已扎实，但生命周期**边缘**（rename 不可达、delete 不一致窗口、悬空回填只在 dream 周期、typed relation 无人能写）与**呈现层**（最富的两个计算产物 `notes_graph_related` 相似边和逐边 `confidence` 从未到达星系图；正文 `[[链接]]` 在 Panel 不可点）大量漏水。瓶颈依旧不在算法在**连线**。

**体检确认的缺口**（2026-07-04 全链路审计，file:line 为审计时锚点）:

| # | 缺口 | 证据锚点 |
|---|------|---------|
| C1 | `rename_note` 死代码：实现正确（含入链 `[[old]]→[[new]]` 级联）但全仓唯一调用者是单测；LLM 工具与 Panel 均无改名入口 | `indexer.rs:626`；`note_manage.rs` 无 rename action |
| C2 | delete 不一致窗口：删除后源笔记正文 `[[已删目标]]` 原封不动且 content_hash 未变永不重索引，等下个 dream 周期 NoteLint 破坏性 purge；Panel 无删除 RPC | `indexer.rs:595`、`note_lint.rs:188`、`store_impl.rs:312` |
| C3 | typed relation 无人能写：检索结构强边强制注入依赖的 `supersedes/contradicts` 边只有后台 ingest 能产出，LLM 工具/Panel 都不能——违 R8 | `note_manage.rs:104`（只有裸 `links`）、`note_retrieval/mod.rs:289` |
| C4 | 悬空回填只在 dream 周期：`relink_unresolved` 仅 `full_rebuild` 与 NoteLint 两个调用者；新建笔记恰好能解析既有 `[[X]]` 时 create 路径不回填 | `store_impl.rs:1134`、`indexer.rs:360`、`note_lint.rs:294` |
| C5 | `[[target\|alias]]` 别名解析时即丢弃；`NoteLinkDto.label` 硬编码 None | `wikilink.rs:17`、`gateway/handlers/graph.rs:147` |
| C6 | 正文内 `[[wikilink]]` 在 Panel 任何视图不可点击；仅星系图 backlink 列表可导航，记忆抽屉/手机详情是纯文本 | `markdown_excerpt.rs:16` 无 `[[]]` 分支、`drawer.rs:209`、`phone/memory/detail.rs:96` |
| C7 | 算而未用：`notes_graph_related`（五信号+MinHash 合并边）canvas 完全不可见（星系图只读 `notes_links`+`notes_graph_cache`）；`notes_links.confidence` 全链路仅 relevance 打分一个读者；bridge/surprising insights 只剩计数无 canvas 强调 | `graph_recompute.rs:85`、`helpers.rs:132`、`fs_orientation.rs:109` |
| C8 | 死占位 RPC 注册（live 版本在 agent_init 重注册，占位纯迷惑源） | `gateway/handlers/mod.rs:719-723` |

---

## 1. 三个已拍板的语义决策

- **D1 · 删除语义 = 墓碑化，不碰正文**: 删除时入链即时标 `tombstone`（一等公民行），源笔记正文 `[[原文]]` 保留作历史痕迹；同名重建自动复活。非破坏、可逆、零不一致窗口。NoteLint 不再对"目标已删"做破坏性 purge。
- **D2 · 未链接提及 = 低置信度软边**: 确定性精确匹配（标题/alias 词边界命中，零 LLM）检出后自动入库 `relation='mention'` 低置信度边，**不改正文**——正文里用户/LLM 手写的才是真 `[[链接]]`。图谱/检索立即受益，可整体降级/删除。
- **D3 · 架构 = C 的方法、A 的目的地**: 新建 `src/memory/notes/links/` 作为链接子系统正式边界（A 的愿景），用渐进方式落地（C）——纯政策函数与生命周期编排收进去，久经测试的薄 SQL 管道留在 `store_impl` 不搬。复刻 `graph/` 纯函数模式。

**用户加强要求**: 本轮选了最完整/最高风险的组合，实现完成后必须多做一次完整性校验（见阶段 ⑤，固定阶段非可选）。

---

## 2. 架构与数据模型

### 2.1 新模块 `src/memory/notes/links/`

| 文件 | 职责 |
|---|---|
| `mod.rs` | 公共类型: `ResolvedLink { target: Option<String>, confidence: f32, resolved_by: ResolveStrategy }`、`LinkStatus { Active, Dangling, Tombstone }`、`ResolveStrategy` 枚举；re-export |
| `resolve.rs` | **解析策略链**（纯函数，输入预取的 filename→paths / alias→paths 候选表 `LinkResolveContext`）: ① 含`/`精确路径 1.0 → ② 精确文件名唯一 0.95 → ③ 精确 alias 唯一 0.85 → ④ 大小写/全半角归一化后唯一 0.7 → ⑤ 悬空 0.0。**多候选绝不猜**（个人笔记连错比不连糟，有意比 codebase-memory-mcp 的 fuzzy 档保守），直接落悬空；候选列表不持久化（可随时重算，YAGNI） |
| `mentions.rs` | **提及扫描器**（纯函数）: 正文 + 标题/alias 词典 → 提及命中列表。护栏: ASCII 词边界匹配 / CJK 子串匹配、短标题不匹配（ASCII≥4 字符 / CJK≥2 字）、跳过正文已 `[[链接]]` 的目标、跳过自指、每笔记 ≤5 提及 |
| `lifecycle.rs` | 生命周期编排（泛型 `S: NoteStore` 自由函数，同 indexer 模式）: `backfill_inbound(agent, filename, aliases)` 定向重解析悬空/墓碑入链；`tombstone_inbound(agent, path)` 删除时标记入链 |

`store_impl::resolve_target` 改为委托 `links::resolve`（只留 SQL 取数）；触发点在 `indexer` / `note_manage` / gateway RPC 就地接线。

### 2.2 Schema 迁移（`notes_links` 加三列）

幂等迁移仿 `migrate_notes_links_confidence` 既有模式：

- `resolved_by TEXT` — 解析策略名（legacy 行 NULL）
- `status TEXT NOT NULL DEFAULT 'active'` — `active | dangling | tombstone`；迁移时把现存悬空行（`to_note==to_raw` 且无 `/`）一次性回填 `dangling`（旧悬空标记语义保留兼容）
- `label TEXT` — `[[target|别名]]` 显示别名（index 时经 `extract_wikilinks_with_alias` 从正文重提取，`KnowledgeNote` 结构不动）

### 2.3 置信度约定（逐边）

| 边来源 | confidence | resolved_by |
|---|---|---|
| 手写 wikilink 精确解析 | 1.0 | `exact_path` / `exact_filename` |
| alias 解析 | 0.85 | `alias` |
| 归一化解析 | 0.7 | `normalized` |
| 工具写 typed relation | 1.0 | （relation 列即语义） |
| ingest typed relation | 模型自评（已有） | 同上 |
| `mention` 软边（status=`active`，两端均为已解析路径） | 0.35 | `mention_scan` |
| `semantic` / `related`（weave） | 沿用现值 | 现值 |

**派生边最终一致性**: mention/co_recalled 等周期物化边被 `index_note` 的 per-from_note reconcile 清掉属**接受的行为**，下个 dream 周期整体重物化补回（与 co_recalled 现行模式一致），不加特殊保留逻辑。

---

## 3. 阶段 ①：生命周期核心行为

- **B1 · 解析链接通**: `resolve_target` 委托 `links::resolve`，每边落 `confidence + resolved_by + status`。行为变化仅新增第④档归一化匹配；其余现行解析结果字节不变（含"文件名多候选→悬空"）。
- **B2 · 即时回填（修 C4）**: `index_note` 成功后调 `lifecycle::backfill_inbound` —— 按 `to_raw = filename或alias` 定向查悬空/墓碑行（走既有 `idx_notes_links_to` 索引，非全表 relink 扫描），命中即重解析为 active。新建笔记 / rename 新名 / 同名重建复活墓碑，三场景同一入口。best-effort 不 fail 写入（P7）。
- **B3 · rename 接通（修 C1）**: ① `note_manage` 新增 `rename` action（`category`+`filename`+`new_title`）直通既有 `NoteIndexer::rename_note`；② 新增 gateway RPC `graph.rename_note`（Panel；权限档与既有写类 RPC `graph.update_note` 同档）；③ rename 后触发 B2。
- **B4 · delete 墓碑化（修 C2，D1 语义）**: ① `delete_note` 改：出链行照删（`from_note=path`），入链行改 `status='tombstone'` 不删，正文一字不动；② NoteLint 移除"目标已删→purge"分支，保留"从未解析成功"的 fuzzy-repair；③ 新增 RPC `graph.delete_note`（Panel tap-to-confirm）；④ 同名重建经 B2 复活。
- **B5 · typed relation 工具化（修 C3，闭 R8）**: `note_manage` create/update/append 新增可选 `relations: Vec<{to, rel_type}>`，经 `KnowledgeNote` 落 frontmatter `relations:`（工具写入 confidence=1.0）；工具 schema 描述教 LLM `supersedes/superseded_by/contradicts` 是检索强边。Panel 不做 relation 编辑 UI（YAGNI，`graph.update_note` 可直改 frontmatter）。
- **B6 · alias 保真（修 C5）**: `index_note` 用既有 `extract_wikilinks_with_alias` 重提取正文落 `label` 列；`NoteLinkDto.label` 停止硬编码 None。
- **B7 · 死注册清理（修 C8）**: 删 `gateway/handlers/mod.rs:719-723` 占位注册。

## 4. 阶段 ②：surface 连线（算而未用 → 可见）

- **S1 · 相似边进星系图**: `graph.query` 响应新增 `notes_graph_related` 边——每节点 top-K（K=3 + score 阈值），与真实 `notes_links` 边去重后下发，新 edge kind=`related_similarity`；星系图用更暗/更细线渲染，区分"软关联"与"硬链接"。
- **S2 · confidence/label/status 上线**: `collect_edges_between` SELECT 补三列 → `NoteLinkDto` 扩展 → `galaxy_build` 把 confidence 映射为边透明度（1.0 实线醒目 ↔ 0.35 mention 幽灵线）。node_detail 逐链接显示 `confidence · resolved_by · status` 徽标；出链的悬空/墓碑在详情栏可见（星系图不画——目标节点不存在）。
- **S3 · bridge/surprising 强调**: `graph.query` 附带 insights 载荷（bridge 节点路径 + surprising 边对）→ bridge 节点光环、surprising 边提亮。`sparse` insight **有意**只留 orientation 健康行（对可视化无行动价值）。
- **S4 · mention 配色预留**: edge kind 表加 `mention` 色档（阶段 ④ 产出数据后即时可见）。

## 5. 阶段 ③：Panel wiki 交互

- **P1 · 正文 `[[wikilink]]` 可点击（修 C6）**: `markdown_excerpt::render_excerpt` 增加 `[[target]]` / `[[target|label]]` 分支 → 可点击元素 + 各视图注入导航回调：星系图详情栏（fly-to）、记忆抽屉（抽屉内打开目标）、手机详情（push 下钻屏，遵循全局手机导航法则）。截断逻辑不动。
- **P2 · rename/delete 入口**: 星系图详情栏 + 记忆抽屉加改名（内联输入）与删除（tap-to-confirm，同 agents 下钻先例），走 `graph.rename_note` / `graph.delete_note`；权限档与 `graph.update_note` 一致。
- **P3 · backlink 面板统一**: 记忆抽屉与手机详情 backlinks 从纯文本改为可导航 chips（对齐星系图详情栏）。
- **约束**: 全部 UI 逻辑在 Leptos Panel（R2），原生壳零改动；`just wasm` → 重编 server 刷新链照旧。

## 6. 阶段 ④：自织网

- **M1 · `MentionWeaveStage`**（新 dream stage，确定性零 LLM，D2 语义）: `notes_index` 建标题/alias 词典 → `links::mentions` 扫正文 → 每周期整体重物化 `relation='mention'` 行（replace 模式同 `co_recall_edges`）。护栏全在扫描器（§2.1）+ 每周期总量上限。挂 Consolidate path，`GraphRecompute` 之后、`NoteDecayStage` 之前（新边即时计入 link_weight，同 note_weave 位置理由）。
- **M2 · hub 抑制盘点**: MinHash 每节点上限（`MINHASH_MAX_EDGES_PER_NODE=8`）已存在；本轮把"每节点上限"纪律延伸到 mention 边（M1 护栏）与 canvas related 边（S1 top-3）。**有意不做**同类别过滤——跨类别相似（project↔reference）在个人笔记里恰是高价值关联。

## 7. 错误处理

- 生命周期钩子全部 best-effort：回填/墓碑失败只 log 不 fail 写入/删除（P7，同 orientation/embed 钩子先例）
- 迁移幂等；老库未迁移前 galaxy 优雅降级（related/insights 空 → 现状渲染）
- 派生边被 reconcile 清掉 = 接受的最终一致性（§2.3），文档写明
- RPC 边界：rename/delete 过 `sanitize_title`，诚实报错；delete Panel 侧 tap-to-confirm
- MentionWeaveStage 失败 → 跳过本周期，非致命（dream stage 惯例）

## 8. 测试策略

单测随模块就地放（项目惯例）。关键面：

1. `links::resolve` 逐档策略 + 多候选落悬空 + 归一化档 + confidence 值
2. `links::mentions` 词边界/CJK/短标题/跳过已链接与自指/上限
3. 墓碑生命周期：删→入链 tombstone + 出链删除→同名重建→B2 复活
4. create/rename 触发 `backfill_inbound`（定向命中，非全表）
5. 迁移幂等 + 现存悬空行状态回填
6. `note_manage` rename / relations 参数校验
7. NoteLint 不再 purge 墓碑、仍修真悬空
8. `galaxy_build` 纯变换：confidence→透明度、新 edge kind、insight 徽标
9. `render_excerpt` 的 `[[]]` / `[[t|label]]` 解析
10. RPC rename/delete happy path + 权限档

**cargo 极度节制**: 开发期靠 diagnostics（`<new-diagnostics>` 是 no-cargo 下唯一真编译信号），合并前仅一次 `cargo check -p alephcore --lib`。

## 9. 阶段 ⑤：完整性校验（固定阶段，用户指定加强）

1. **连线审计复检**: 实现完成后派独立 subagent 用本次体检同款方法重扫全链路（ingest→store→graph→consumers），逐条对照 B1–B7 / S1–S4 / P1–P3 / M1–M2 行为清单，确认零断线、零新增算而未用
2. **SDD 逐任务双审 + 全分支 Ready-to-merge 终审**（项目既有惯例）
3. **合并前一次 `cargo check -p alephcore --lib`**
4. **运行时 QA**: 重建 macOS App，浏览器实测——星系图新边种/置信度透明度/insight 光环；正文链接点击导航三视图；rename/delete 流程；跑一个 dream 周期后 mention 边出现

## 10. Backlog（明确不进本轮）

- **trust×衰减自调边权**（hermes 思路）: 触碰检索打分，风险/收益本轮最差
- **共编辑 provenance 边**（codebase-memory-mcp git 共变更思路）: `co_recalled` 已覆盖行为耦合；会话级写踪迹是全新管道
- Panel relation 编辑 UI；悬空候选列表持久化

## 11. 红线自检

- **R10**: 零 `src/harness/` 改动
- **R7/P8**: 提及检测与解析链是确定性机械匹配（同 FTS 性质），不做语义判断；语义关联仍由 LLM（手写链接/typed relation）与既有 weave 产出
- **R2**: Panel 改动全在 Leptos，原生壳零改动
- **R3**: 零新第三方依赖
- **文档同步**: NOTES.md §5/§8/§14、FEATURE_LOCATOR §2.5 锚点随实现更新
