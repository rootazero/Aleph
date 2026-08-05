# Note 层 body 保真与嵌入新鲜度（§2.9 / §2.10 深化轮）

- **日期**: 2026-08-05
- **分支**: `worktree-note-layer-fidelity-freshness`
- **覆盖**: FEATURE_LOCATOR §2.9（Note Body Fidelity & Hybrid note_manage）、§2.10（Unified Write Chokepoint & Embed Freshness）
- **参考项目**: EverOS（markdown 真源 + SQLite/LanceDB 双索引 + cascade 同步）、hermes-agent、evolver

---

## 1. 参考项目对照（Gap Analysis）

hermes-agent 的 memory 全部是外部 provider 插件（mem0 / honcho / supermemory / retaindb…），
没有自建笔记层，唯一可移植物是 `plugins/memory/query_rewrite.py`（LLM 查询改写）——**本轮不移植**，
理由见 §5。evolver 的可移植部分（对持久事件的纯折叠）已在 §2.8 消化完毕。

**EverOS 是本轮唯一高价值参照**：它与 Aleph 的笔记层是同一个物种（canonical `.md` +
可 diff + 双索引），且它把「磁盘与索引之间的同步」建模成了显式状态。

| 维度 | EverOS | Aleph 现状 | 判定 |
|---|---|---|---|
| markdown 为真源 | canonical `.md`，可读/可编辑/可 git | `KnowledgeNote.body` verbatim 往返 | 持平 |
| **用户 frontmatter 往返** | 直接编辑 `.md`，watcher 同步 | `Frontmatter` 是**固定字段白名单**，未建模 key 解析即丢 | **缺口 D1** |
| **索引/向量新鲜度状态** | `md_change_state`(mtime, status, change_type, retryable, retry_count) + 纯函数 `reconcile()` | FTS 有 `content_hash`；**向量侧零新鲜度记录** | **缺口 D5** |
| 重建幂等 | mtime 稳定且 `done` ⇒ skip | FTS 按 hash skip；`reembed_all` 每次**全量**重嵌 | **缺口 D5** |
| 删除对账 | 文件消失且上轮不是成功删除 ⇒ 补发 delete | `full_rebuild` prune + `prune_orphan_vectors` | 持平 |
| 失败重试语义 | `retryable` / `retry_count`，不可恢复需改文件才再试 | embed-on-write 失败＝`warn!` 后**永不重试** | **缺口 D5** |
| 混合检索 | BM25 + dense → RRF → cross-encoder rerank | RRF(vector, FTS) + `note_retrieval` 侧可选 LLM rerank | 持平 |
| **降级策略** | 大声失败（`ProviderNotConfiguredError`，HTTP 422） | 承诺回退 FTS，**只覆盖 `embed()` 失败** | **缺口 D3** |
| **诚实查询面** | 每条通道独立 DTO，不混淆 | `mode` 恒报 `"hybrid"`，哪怕向量腿贡献 0 条 | **缺口 D8** |

### 架构映射的关键取舍

EverOS 的 `md_change_state` 是一整套**状态机**（watcher + scanner + reconciler + worker +
retry 策略），因为它的写入者在进程之外（用户直接改文件）。**Aleph 的写入者全部在进程之内**
（`NoteIndexer` 是唯一写路径，§2.10 的立意就是这个），所以移植整套状态机等于为不存在的问题
建基础设施——违 R3（核心轻量化）与 P6（YAGNI）。

**本轮只取那一条 Aleph 真正缺的**：**按内容哈希记账**。它是 EverOS 用 mtime 做的事的
内容寻址版本（更强：mtime 会被 `touch`／同步工具骗到，内容哈希不会），而 Aleph 的
`content_hash` 早就为 FTS 侧算好了——向量侧只是从来没有人记下「我嵌的是哪一版」。

---

## 2. 已核实的缺陷清单

每条都在代码中验证过，不是推测。严重度按「用户能观察到什么」定，不按代码丑陋程度。

### D1 · HIGH · §2.9 · 未建模的 frontmatter 被静默销毁

`src/memory/notes/note/parsing.rs::Frontmatter` 是固定字段集（category / tags / created /
updated / confidence / severity / source_notes / supersedes / superseded_by / permanent /
stale / relations / type / title / aliases）。`serde_yaml` 忽略其余 key，`to_markdown`
逐字段重新生成 frontmatter ⇒ **人手写的、Obsidian 插件写的、外部工具写的任何其它 key
（`cssclass` / `publish` / `id` / `up` / `date` / 自定义）在第一次经过写路径时永久消失**。

触发者是全部写路径：`note_manage(update|append|rename)`、dream 的 distill/rewrite、
`merge_source_notes_into_note`、`append_relations`。

这与本节自己的契约直接冲突——§2.9 的标题就是 body 保真、职责段写着「markdown 是真源的契约
在写路径闭环」。保真只做了 body 那一半，frontmatter 那一半是有损的，而**用户手写的元数据
恰恰全在 frontmatter 里**。

### D2 · HIGH · §2.9 · 反复 append 把正文打散

- `KnowledgeNote::add_links`：每次调用、只要有新链接，就 `body.push('\n')` + **新起一行
  `Related: …`**。它检查的是「这个链接在不在 body 里」，**不检查「body 里是不是已经有一个
  Related 块」**。
- `KnowledgeNote::append_facts`：把 bullet 追加到 body **末尾**，即已有的 `Related:` 行之后。

而 `src/memory/dreaming/stages/note_weave.rs:498` **每夜**以「只有 links、没有 facts」调
`append_to_note`。于是一篇被反复编织的笔记会长成：

```
prose…
Related: [[A]] [[B]]
- 某个后来 append 的事实
Related: [[C]]
- 又一个事实
Related: [[D]]
```

这正是 §2.9 关键词列表里的第一条：「笔记正文被打散」。

### D3 · HIGH · §2.9 / §2.10 · 降级承诺只兑现了一半

两处同形：

- `src/builtin_tools/note_manage.rs::search_notes` — 只在 `embedder.embed()` 返回 `Err`
  时回退 FTS。`hybrid_search_notes` 自己返回 `Err` 时（例如 `vector_search` →
  `notes_vec_table_for_dim` 拒绝未知维度）整个查询失败。
- `src/memory/note_retrieval/mod.rs:530` — `.await?` 直接上抛，而它**就写在**
  「degrade to FTS-only search instead of failing」这句注释下面三行。这条路是**每个回合的
  自动召回**，它一失败，`<memory-context>` 整段消失。

判据要说清楚：**「本地的笔记还在」这个降级理由，对 store 侧报错同样成立**——报错的是向量腿，
FTS 腿和磁盘上的笔记都完好。只守 `embed()` 那一半，是把「远程不可达」当成了唯一的失败模式。

### D4 · HIGH · §2.10 · 不支持的嵌入维度＝静默全损

维度集合 `{768, 1024, 1536}` 硬编码在**五处**：

1. `sqlite/vec.rs::ALL_NOTES_VEC_TABLES`
2. `sqlite/vec.rs::notes_vec_table_for_dim`
3. `sqlite/vec.rs::routing_exp_vec_table_for_dim`
4. `sqlite/schema/mod.rs::init_notes_vec_tables`
5. `sqlite/schema/mod.rs::init_vec_tables`（routing 那三张）

一个 384 维（`all-MiniLM-L6-v2`，最常见的本地嵌入模型）或 3072 维
（`text-embedding-3-large`）的 embedder 会让：

- **写**：`upsert_embedding` 每次都 `Err` → 被 `refresh_embedding` 的 `warn!` 吞掉 →
  **这个部署永远没有任何笔记向量**，没有任何一处报告这件事；
- **读**：每次 hybrid 查询 `Err` → 经 D3 变成硬失败。

两个症状加起来看起来像「向量检索这个功能是坏的」，而根因只是一张表没建。

### D5 · HIGH · §2.10 · 「嵌入新鲜度」没有新鲜度记录

`NoteIndexer::refresh_embedding` 按设计吞掉失败，注释给的理由是「note 已经在磁盘上，
`reembed_all` 是安全网」。但：

- `reembed_all` **只能手动触发**（RPC / CLI），不在任何自动路径上；
- 它**不做任何跳过**——每次运行重嵌全部笔记（文档却写着 "Idempotent: safe to re-run"，
  这句话在成本意义上是假的）；
- **没有任何东西能区分一个新鲜向量和一个陈旧向量**：`notes_vec_map` 只有
  `(rowid, path, agent_id)`。

于是一次网络抖动 ⇒ 那篇笔记的向量永久停在旧版本，或者根本不存在，而**没有任何面能看见它**。
本节标题叫「嵌入新鲜度」，而新鲜度是这一层唯一没有被表示的东西。

### D6 · MED · §2.9 · 分类规范化只接在 `create`

`canonicalize_category` 在 `note_manage` 里**只有 `handle_create` 调**。
`handle_update` / `handle_append` / `handle_delete` / `handle_rename` / `handle_list`
拿原始字符串直接 `validate_category`。后果：`category="projects"` 时 create 成功
（落在 `project/`），而 update / append / delete 报 "Unknown category"，list 返回空——
**同一个模型在同一个会话里对同一个分类得到互相矛盾的答案**。

§2.13 为 ingest 的 op 边界修过这一类（`canonicalize_note_path`），note_manage 侧只修了 create。

### D7 · MED · §2.9 · FTS 腿自己拼路径（重复真源）

`search_notes` 的 FTS 回退腿用
`self.indexer.memory_dir().join(agent).join(cat).join(format!("{}.md", filename))`
读正文，而 hybrid 腿走的是 `sqlite/notes/helpers.rs::load_note_content_from_disk`
（`utils::paths::get_note_memory_dir()` + `note_md_filename()`）。两处对「这篇笔记在哪」
各有一个答案，且 FTS 腿 `unwrap_or_default()` ⇒ 读不到就返回**空正文的命中**。
目前两者恰好等价（`index_note` 会 `strip_md_ext`），但这是本仓反复被咬的形状。

### D8 · MED · §2.9 · 查询面不诚实

`handle_query` 的 `mode` 只有 `"hybrid"` / `"full-text"` 两个值，而 `"hybrid"` 在
「向量腿返回 0 条」时照报——模型无法区分「语义检索跑了但没匹配」与「语义检索根本没参与」。
§2.13 已经为 `note_graph_query` 立过 `QueryAdvisory{applied_limit, truncated, applied_depth}`
这个惯用法，这里照搬。

### D9 · LOW / perf

- `hybrid_search_notes`：对每个融合后的命中做**串行** `get_note_index`（各自抢一次
  `Mutex<Connection>`）+ **串行**磁盘读。
- `full_rebuild` 的孤儿剪枝：对每个索引行做**串行** `fs::metadata`。

### D10 · 熵 / 文档

- `note_manage.rs` **1935 行**（项目上限 800，P2）——本轮不拆，记为独立轮。
- `note_manage::refresh_embedding` 与 `NoteIndexer::refresh_embedding` 是同一件事的两份实现。
- **FEATURE_LOCATOR §2.10 陈旧**：仍写着 `finalize_write` = reparse→index_note→**orientation**
  →refresh_embedding，以及「`with_embedder` 可选钩子镜像 `with_orientation`」。orientation
  钩子已在 `97397672a`（2026-08-02）随 `[memory.orientation]` / `[memory.profile]` 配置一起
  **有意 CUT**，`NoteIndexer` 上不再有 `with_orientation`。

---

## 3. 设计

### 3.1 D1 — frontmatter 直通（passthrough）

```rust
// parsing.rs
pub(super) struct Frontmatter {
    …已有字段…
    #[serde(flatten)]
    pub(super) extra: BTreeMap<String, serde_yaml::Value>,
}
```

`KnowledgeNote` 新增 `#[serde(default)] pub extra_frontmatter: BTreeMap<String, serde_yaml::Value>`。

`to_markdown` 在已知字段之后、`---` 收尾之前，按 `BTreeMap` 的确定序（字典序）逐条
`serde_yaml` 渲染 extra。

**不变量**：
- 空 map ⇒ 输出**逐字节等价**于本改动之前（legacy parity，与 `relations:` / `permanent:`
  的既有惯用法一致）。
- **已知 key 绝不进 extra**——`#[serde(flatten)]` 天然如此；额外用一条守卫测试钉住
  `stale` 不会因为「解析了但不回写」的既有设计而从 extra 那条路复活
  （`stale` 是 parse-only 字段，它的丢弃是**有意的**，见 `note/mod.rs` 的字段文档）。
- extra 的值来自磁盘上的 YAML，**不是模型自由文本**，但仍经 `serde_yaml` 序列化（自动引号），
  不做手工字符串拼接。

**代价（诚实记下）**：一篇带 extra key 的笔记，其 `to_markdown` 输出会变长；
`content_hash` 随之变化 ⇒ 该笔记在升级后的第一次 `full_rebuild` 会被重新索引一次。
这是一次性的、正确方向的。

### 3.2 D2 — 单一尾部 `Related:` 块

在 `note/mod.rs` 引入两个纯函数（同文件，私有）：

- `split_trailing_related(body) -> (head, Option<existing_related_targets>)`
  —— 从 body 尾部识别**连续的** `Related:` 行块（允许多行，因为存量笔记已经被 D2 打散过）。
- `render_related(targets) -> String`

于是：
- `add_links`：解析尾部 Related 块 → 并入新 target（保序去重）→ **重写为单一行**。
  存量笔记的多行 Related 会在下一次 weave 时被**自愈合并**。
- `append_facts`：把 bullet 插在 `head` 之后、Related 块之前。

**不变量**：body 里非尾部位置出现的 `Related:` 文本（正文中间提到这个词）不受影响——
只识别尾部连续块。无 Related 块的 body 行为逐字节不变。

### 3.3 D3 — 降级谓词单一源

新增 `src/memory/notes/retrieval.rs`（已存在的模块）里的一个说明性 helper 不够——
判据要落在**调用点**，因为两处的回退目标不同（一个回 `SearchRows`，一个回 `Vec<ScoredFact>`）。
做法是把两处的 `match` 形状统一成同一个三分支：

```
embedder 缺席        → FTS（已知稳态，debug 级）
embed() 失败         → FTS（warn，远端不可达）
hybrid store 侧失败  → FTS（warn，向量腿不可用；笔记与 FTS 都还在）
```

第三个分支是新增的。两处各自写，但**共享同一条注释判据**并各有一条回归测试
（注入一个必然让 store 侧失败的维度）。

### 3.4 D4 — 维度单一源 + 补 384 / 3072

`sqlite/vec.rs`：

```rust
/// 唯一真源：每个受支持的嵌入维度。新增一个维度只改这里。
pub const SUPPORTED_EMBEDDING_DIMS: &[u32] = &[384, 768, 1024, 1536, 3072];
```

- `ALL_NOTES_VEC_TABLES` / `init_notes_vec_tables` / `init_vec_tables` 全部由它派生
  （表名由 `notes_vec_{dim}` / `routing_exp_vec_{dim}` 生成，仍是**编译期可枚举的内部
  allowlist**，SQL 注入面不变）。
- 表名生成必须返回 `&'static str`（现有签名），用 `OnceLock<Vec<String>>` 或
  `const` 数组配对——**优先 const 配对表**，保持零运行时分配与现有签名。
- `CREATE VIRTUAL TABLE IF NOT EXISTS` 幂等 ⇒ 存量 DB 下次打开即获得两张新空表，零迁移。

### 3.5 D5 — 向量新鲜度记账（EverOS `md_change_state` 的最小映射）

`notes_vec_map` 增两列（`ALTER TABLE … ADD COLUMN`，沿用 `migrations.rs` 既有惯用法）：

| 列 | 类型 | 含义 |
|---|---|---|
| `embedded_hash` | `TEXT NOT NULL DEFAULT ''` | 嵌入时所用文本对应的笔记 `content_hash` |
| `embedded_at` | `INTEGER NOT NULL DEFAULT 0` | 嵌入完成的 unix 秒 |

`NoteStore` 接口变化：

- `upsert_embedding(path, agent_id, embedding, dim)` 增一个 `content_hash: &str` 参数
  （**不加重载、不加第二个方法**——第二个方法就是第二条可以忘记走的路）。
  空串表示「调用方不知道哈希」，语义等同于「永远算陈旧」，保证任何未来的新调用方
  fail-safe 到「会被重嵌」而不是「假装新鲜」。
- 新增 `stale_vector_paths(agent_id) -> Vec<String>`：`notes_index` 左连
  `notes_vec_map`，返回**没有向量行**或 `embedded_hash != notes_index.content_hash` 的 path。

消费者（必须现在就有，否则按 R10 是零消费者抽象）：

1. **`reembed_all` 跳过新鲜笔记** —— 这是 `reembed_all` 文档里那句 "Idempotent: safe to
   re-run" 第一次在成本意义上成立。同时新增 `ReembedResult.skipped` 让跳过量可见。
   > ⚠️ 换 provider（维度或模型变了）时**必须全量重嵌**，此时 `embedded_hash` 相同但向量
   > 空间不同。故跳过只在**目标维度与该行现有向量维度一致**时生效——判据取自既有的
   > `embedding_signature`：签名变了 ⇒ `force` 全量。这条不能省，否则换模型会静默半迁移。
2. **`full_rebuild` 报告 stale 计数** —— `IndexStats` 增 `stale_vectors: usize`，
   让「这个部署的向量索引有多旧」第一次可被观测（并让 D4 那类静默全损当场可见：
   stale 数 == 笔记总数）。

**刻意不做**：不做自动重嵌扫描。自动重嵌是一次可能很贵的 LLM/API 调用批次，触发时机
（启动？做梦？）是一个带真实成本的产品决定，不是一条待接的线；本轮只让它**可被看见**
和**可被便宜地手动修复**。记为 follow-up。

### 3.6 D6 / D7 / D8 / D9

- **D6**：在 `NoteManageArgs` 的读取处收敛——所有 handler 改用同一个
  `fn resolve_category(&self, args) -> Result<String>`（canonicalize → validate），
  create 现有行为不变，其余五个 handler 接上。
- **D7**：FTS 回退腿改走 store 已有的 `get_notes_with_content(agent_id, &paths)`，
  删掉本地路径拼接与 `unwrap_or_default`。纯复用，负行数。
- **D8**：`NoteManageResult` 增 `search_advisory: Option<SearchAdvisory>`，
  字段 `{ mode, vector_hits, fts_hits, returned, bodies_omitted, degraded_reason }`。
  `mode` 由**实际发生的事**派生，不由配置派生。
- **D9**：`hybrid_search_notes` 的正文加载改 `futures::future::join_all`；
  `full_rebuild` 的孤儿探测同理。索引批量化留作 follow-up（需要新 store 方法，
  收益小于本轮其它项）。

---

## 4. 测试计划

每条缺陷一条**会因为该缺陷而变红**的回归测试（不是「调用发生了」而是「效果到达了」）：

| # | 测试 | 断言的是效果 |
|---|---|---|
| D1 | `unknown_frontmatter_keys_survive_a_write_round_trip` | 带 `cssclass:` 的笔记经 `write_note` 后，磁盘上仍有 `cssclass:` |
| D1 | `a_note_without_extra_frontmatter_serializes_byte_identically` | 空 extra ⇒ 输出与固定期望字符串逐字节相等 |
| D2 | `repeated_link_weaving_keeps_one_related_block` | 三次 `add_links` 后 body 里 `Related:` 出现次数 == 1 |
| D2 | `appended_facts_land_above_the_related_block` | fact bullet 的行号 < Related 行号 |
| D3 | `note_manage_query_degrades_to_fts_when_the_vector_leg_errors` | 注入不支持维度的 embedder ⇒ 仍返回 FTS 命中，且 advisory 报 degraded |
| D3 | `auto_recall_degrades_to_fts_when_the_vector_leg_errors` | 同上，在 `note_retrieval` 侧 |
| D4 | `every_supported_dim_has_a_table_and_a_name` | 遍历 `SUPPORTED_EMBEDDING_DIMS`，建表 + `notes_vec_table_for_dim` 均成功 |
| D4 | `a_384_dim_embedding_round_trips_through_the_vector_index` | upsert → vector_search 找回它 |
| D5 | `reembed_skips_a_note_whose_vector_is_already_fresh` | 第二次 `reembed_all` 的 `skipped` == 笔记数、`facts_updated` == 0 |
| D5 | `a_rewritten_note_becomes_stale_until_reembedded` | 改 body → `stale_vector_paths` 含它；重嵌后不含 |
| D5 | `a_signature_change_forces_a_full_reembed` | 换签名 ⇒ 不跳过 |
| D6 | `every_action_accepts_a_plural_category` | `projects` 对 update/append/delete/list 均落到 `project/` |
| D7 | `the_fts_leg_and_the_hybrid_leg_read_the_same_file` | 两条腿对同一 path 返回相同正文 |
| D8 | `mode_reports_full_text_when_the_vector_leg_contributed_nothing` | advisory.mode != "hybrid" |

---

## 5. 明确不做（含理由）

- **LLM query rewrite**（hermes `query_rewrite.py`）：每次召回多一次 LLM 调用，而 Aleph 的
  auto-recall 在每个回合都跑。收益未证，成本确定。R7 干净但 YAGNI。
- **EverOS 的 watcher / scanner / reconciler / worker 状态机**：Aleph 的笔记写入者全部在
  进程之内（`NoteIndexer` 是唯一写路径），外部编辑由 `full_rebuild` 兜底。移植整套等于
  为不存在的问题建基础设施（R3 / P6）。
- **自动重嵌扫描**：见 §3.5。
- **`note_manage.rs` 拆分**（1935 行 > 800）：与本轮的 diff 会互相遮蔽，评审更难。独立轮。
- **cross-encoder rerank 进 `note_manage(query)`**：`note_retrieval` 已有 LLM rerank；
  给显式工具查询再加一次 LLM 调用违背「工具查询要快且可预期」。
- **存量 plural 笔记的批量迁移**：§2.13 已把它记为 follow-up，本轮不改变该决定。
