# 记忆 Canvas 星系 — Plan 2: Core 供数据 (WS-2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `graph.query` 供出**边的关系类型**（wikilink/semantic/related/…）、**节点的 community_id 与 updated_at**、以及**按 agent 的总节点数 total**，并在面板加"显示前 N 个"截断提示。为 Plan 3 的视觉编码铺数据。

**Architecture:** Core（`src/memory` + `src/gateway`）供数据、面板渲染（R4）。改动：①`collect_edges_between` 带出 `relation`（get_graph_data 边类型加 `Option<String>`；get_neighbors 内部丢弃 kind，签名不变）②新增两个 `NoteStore` 方法 `count_notes` / `community_ids`（单一 impl `SqliteMemoryBackend`）③`entry_to_dto` 透传 `updated_at`，handler 填 kind/community/total ④wire DTO + 面板 adapter DTO 加 `Option` 字段（additive）⑤面板截断角标消费 `total`。

**Tech Stack:** Rust · rusqlite · serde（`graph_types.rs` 仅 serde，无 schemars）· `#[tokio::test]`。

## Global Constraints

- **wire 字段全部 additive `Option` + `#[serde(default, skip_serializing_if = "Option::is_none")]`**（服务端）/ `#[serde(default)]`（面板 Deserialize），不破旧客户端（P3）。
- **edge `kind` 保持 `Option<String>`**（`compute_hop_depth_direct_edge` 测试用 `kind: None` 字面量，改成非 Option 会破测试）。
- **gateway 红线**（`src/gateway/CLAUDE.md`）：改 handler 必须同步更新/新增测试。**不触认证/Origin**。
- **社区冷缓存**：首次 dream recompute 前 `notes_graph_cache` 为空 → `community_id` 全 `None`（`community_ids` 返回空 map），面板须容忍。
- **编译门**：`cargo check -p alephcore`（Core）/ `cargo check -p aleph-panel`（面板）。测试 `cargo test -p alephcore --lib <filter>`。**极度节制 cargo**：Core 改动（Task1-4）合并为一次 `cargo test -p alephcore --lib graph` 收尾；面板改动（Task5-6）一次 `cargo check -p aleph-panel`。
- **提交**：English `<scope>: <description>`，scope `memory:` / `gateway:` / `canvas:`。不加归属 trailer。

---

## Task 1: 边带出 relation kind（store 层）

**Files:**
- Modify: `src/memory/store/sqlite/notes/helpers.rs:114-164`（`collect_edges_between`）
- Modify: `src/memory/notes/store.rs`（`get_graph_data` trait 签名，约 145-149）
- Modify: `src/memory/store/sqlite/notes/store_impl.rs`（`get_graph_data` 604-640；`get_neighbors` 722 处丢 kind）

**Interfaces:**
- Produces: `collect_edges_between → Vec<(String, String, Option<String>)>`（第三元 = `notes_links.relation`，NULL 表 body wikilink）；`get_graph_data → (Vec<NoteIndexEntry>, Vec<(String, String, Option<String>)>)`。`get_neighbors` 签名不变。

- [ ] **Step 1: 写失败测试（store 层，边 kind）**

在 `src/memory/store/sqlite/notes/store.rs` 的 `#[cfg(test)] mod tests` 里新增（复用该文件既有的 backend 构造与 `add_link_with_relation`/`index_note` 夹具——参照同模块 `co_recall_links_fill_gaps_without_clobbering_semantic_links` 的建库方式）：

```rust
#[tokio::test]
async fn get_graph_data_surfaces_edge_relation_kind() {
    let store = new_test_store().await; // 用本模块既有的测试建库 helper
    // 两个 note + 一条 semantic 关系边
    seed_note(&store, "a", "reference").await;
    seed_note(&store, "b", "reference").await;
    store.add_link_with_relation("a", "b", "semantic", "default").await.unwrap();

    let (_nodes, edges) = store.get_graph_data("default", 100).await.unwrap();
    let e = edges.iter().find(|(f, t, _)| f == "a" && t == "b").expect("edge a->b");
    assert_eq!(e.2.as_deref(), Some("semantic"));
}
```

> 若本模块已有 `new_test_store`/`seed_note` 之外命名的 helper，改用实际名字（先 `grep -n "async fn.*test.*store\|fn seed\|index_note" src/memory/store/sqlite/notes/store.rs`）。关键是断言 `edges` 第三元携带 `"semantic"`。

- [ ] **Step 2: 运行确认失败（类型不匹配即为红）**

Run: `cargo test -p alephcore --lib get_graph_data_surfaces_edge_relation_kind`
Expected: 编译失败（`edges` 仍是 2-tuple，`e.2` 不存在）。

- [ ] **Step 3: 改 `collect_edges_between` 带出 relation**

`helpers.rs`：SQL 加 `relation` 列、返回类型加 `Option<String>`：

```rust
pub(crate) fn collect_edges_between(
    conn: &rusqlite::Connection,
    visible: &HashSet<String>,
    agent_id: &str,
) -> Result<Vec<(String, String, Option<String>)>, AlephError> {
```
把 SQL（约 131-134）改为：
```rust
    let sql = format!(
        "SELECT from_note, to_note, relation FROM notes_links \
         WHERE agent_id = ?1 AND from_note IN ({from_clause}) AND to_note IN ({to_clause})"
    );
```
把 `query_map` 闭包（约 154-156）改为：
```rust
        .query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
```

- [ ] **Step 4: 改 `get_graph_data`（trait + impl）**

`store.rs` trait 签名（约 145-149）返回类型：
```rust
    async fn get_graph_data(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String, Option<String>)>), AlephError>;
```
`store_impl.rs` impl 签名（604-608）同步改返回类型为
`Result<(Vec<NoteIndexEntry>, Vec<(String, String, Option<String>)>), AlephError>`。impl body 无需再改（`collect_edges_between` 现已返回 3-tuple，`Ok((entries, edges))` 直接透传，637-639 不变）。

- [ ] **Step 5: `get_neighbors` 丢弃 kind 保持签名不变**

`store_impl.rs:722` 把
```rust
        let edges = collect_edges_between(&conn, &visited, agent_id)?;
```
改为
```rust
        let edges = collect_edges_between(&conn, &visited, agent_id)?
            .into_iter()
            .map(|(f, t, _)| (f, t))
            .collect();
```
（`get_neighbors` trait/impl 返回类型仍 `Vec<(String, String)>`，`handle_neighbors_impl` 不变。）

- [ ] **Step 6: 运行测试转绿**

Run: `cargo test -p alephcore --lib get_graph_data_surfaces_edge_relation_kind`
Expected: PASS。

> 本 Task 不单独提交，与 Task 2-4 合成一次 Core commit（见 Task 4）。

---

## Task 2: 新增 `count_notes` 与 `community_ids`（store 层）

**Files:**
- Modify: `src/memory/notes/store.rs`（trait 加两方法）
- Modify: `src/memory/store/sqlite/notes/store_impl.rs`（impl 两方法）

**Interfaces:**
- Produces:
  - `async fn count_notes(&self, agent_id: &str) -> Result<i64, AlephError>` — 该 agent 的 `notes_index` 行数。
  - `async fn community_ids(&self, agent_id: &str) -> Result<HashMap<String, i64>, AlephError>` — 该 agent `notes_graph_cache` 的 `node_path → community_id` 全表映射（冷缓存 = 空 map）。

- [ ] **Step 1: 写失败测试**

在 `src/memory/store/sqlite/notes/store.rs` 测试模块新增：

```rust
#[tokio::test]
async fn count_notes_is_agent_scoped() {
    let store = new_test_store().await;
    seed_note(&store, "a", "reference").await; // agent "default"
    seed_note(&store, "b", "reference").await;
    assert_eq!(store.count_notes("default").await.unwrap(), 2);
    assert_eq!(store.count_notes("other-agent").await.unwrap(), 0);
}

#[tokio::test]
async fn community_ids_reads_graph_cache() {
    let store = new_test_store().await;
    seed_note(&store, "a", "reference").await;
    // 直接物化一条社区缓存（复用 replace_graph_cache 的行结构）
    store.replace_graph_cache("default", &[("a".to_string(), 7, 0.5, 3)]).await.unwrap();
    let map = store.community_ids("default").await.unwrap();
    assert_eq!(map.get("a").copied(), Some(7));
    assert!(store.community_ids("other-agent").await.unwrap().is_empty());
}
```

> `replace_graph_cache` 的实参形状先核对：`grep -n "fn replace_graph_cache" src/memory/notes/store.rs src/memory/store/sqlite/notes/store_impl.rs`，按其真实签名（`&[(String, i64, f64, i64)]` 或类似）调整元组。若签名不同，用真实字段顺序。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore --lib "count_notes_is_agent_scoped|community_ids_reads_graph_cache"`
Expected: 编译失败（方法不存在）。

- [ ] **Step 3: trait 加两方法声明（`store.rs`，紧挨 `count_all_notes` 之后）**

```rust
    /// Count notes for a single agent (scoped, unlike `count_all_notes`).
    async fn count_notes(&self, agent_id: &str) -> Result<i64, AlephError>;

    /// Map every cached note path to its community id for one agent
    /// (`notes_graph_cache`). Empty before the first dream graph-recompute.
    async fn community_ids(
        &self,
        agent_id: &str,
    ) -> Result<std::collections::HashMap<String, i64>, AlephError>;
```

- [ ] **Step 4: impl 两方法（`store_impl.rs`，紧挨 `count_all_notes` impl 之后，镜像其错误处理）**

```rust
    async fn count_notes(&self, agent_id: &str) -> Result<i64, AlephError> {
        let conn = lock_conn!(self)?;
        conn.query_row(
            "SELECT COUNT(*) FROM notes_index WHERE agent_id = ?1",
            params![agent_id],
            |row| row.get(0),
        )
        .map_err(|e| AlephError::config(format!("count_notes failed: {e}")))
    }

    async fn community_ids(
        &self,
        agent_id: &str,
    ) -> Result<std::collections::HashMap<String, i64>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare("SELECT node_path, community_id FROM notes_graph_cache WHERE agent_id = ?1")
            .map_err(|e| AlephError::config(format!("community_ids prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| AlephError::config(format!("community_ids query: {e}")))?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (path, cid) =
                row.map_err(|e| AlephError::config(format!("community_ids row: {e}")))?;
            map.insert(path, cid);
        }
        Ok(map)
    }
```

> 若 `store_impl.rs` 顶部未 `use std::collections::HashMap;`，可用全限定 `std::collections::HashMap`（上面已全限定，无需改 import）。

- [ ] **Step 5: 运行测试转绿**

Run: `cargo test -p alephcore --lib "count_notes_is_agent_scoped|community_ids_reads_graph_cache"`
Expected: PASS。

---

## Task 3: wire DTO 加字段（`graph_types.rs`）

**Files:**
- Modify: `src/gateway/handlers/graph_types.rs:67-100`

**Interfaces:**
- Produces: `NoteNodeDto` 多 `community_id: Option<u32>` + `updated_at: Option<i64>`；`GraphQueryResponse` 多 `total: Option<usize>`。`NoteLinkDto.kind` 已存在，不改。

- [ ] **Step 1: `NoteNodeDto` 加两字段**

把 `graph_types.rs:67-75` 的 `NoteNodeDto` 改为：

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,   // path: "wiki/rust-ownership"
    pub name: String, // display: "rust-ownership" (filename only)
    pub path: String, // full relative path
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
    /// Community id from `notes_graph_cache` (Louvain). `None` on a cold cache
    /// (before the first dream graph-recompute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub community_id: Option<u32>,
    /// Note last-modified epoch seconds — drives recency-based visual encoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}
```

- [ ] **Step 2: `GraphQueryResponse` 加 `total`**

把 `graph_types.rs:96-100` 改为：

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
    /// Total notes for the agent (nodes may be truncated to `limit`); lets the
    /// panel show a "showing top N of M" indicator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
}
```

> 其它构造 `NoteNodeDto` / `GraphQueryResponse` 的处（`handle_neighbors_impl` 等）在 Task 4 一并补字段初始化，否则本文件改完 Core 暂不编译——故 Task 3+4 连编。

---

## Task 4: handler 填充 kind / community / updated_at / total（`graph.rs`）

**Files:**
- Modify: `src/gateway/handlers/graph.rs`（`entry_to_dto` 19-28；`handle_query_impl` 103-143；`handle_neighbors_impl` 边映射 202-210 + center/nodes 构造）
- Modify: `src/gateway/handlers/graph.rs` 测试（`graph_query_uses_explicit_agent_id` 680-700）

**Interfaces:**
- Consumes: `db.get_graph_data`（3-tuple 边）、`db.count_notes`、`db.community_ids`（Task 1/2）。
- Produces: `graph.query` 响应节点带 `updated_at`（+ `community_id` 若缓存有）、边带 `kind`（NULL→`"wikilink"`）、响应带 `total`。

- [ ] **Step 1: `entry_to_dto` 透传 `updated_at`，community 默认 None**

把 `graph.rs:19-28` 改为：

```rust
/// Convert a `NoteIndexEntry` into a `NoteNodeDto`.
/// `community_id` is left `None` here; callers that have the community map
/// (graph.query) fill it in a second pass.
fn entry_to_dto(entry: &NoteIndexEntry) -> NoteNodeDto {
    NoteNodeDto {
        id: entry.path.clone(),
        name: entry.filename.clone(),
        path: entry.path.clone(),
        category: entry.category.clone(),
        tags: entry.tags.clone(),
        link_count: entry.link_count,
        community_id: None,
        updated_at: Some(entry.updated_at),
    }
}
```

- [ ] **Step 2: `handle_query_impl` 填 community / kind / total**

把 `graph.rs:119-137`（`let (entries, links) = ...` 到 `GraphQueryResponse { nodes, edges }`）改为：

```rust
    let (entries, links) = match db.get_graph_data(agent_id, params.limit).await {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("NoteStore error: {e}"))
        }
    };

    // Community map (cold cache => empty) + agent-scoped total for truncation.
    let communities = db.community_ids(agent_id).await.unwrap_or_default();
    let total = db.count_notes(agent_id).await.ok().map(|t| t.max(0) as usize);

    let nodes: Vec<NoteNodeDto> = entries
        .iter()
        .map(|e| {
            let mut dto = entry_to_dto(e);
            dto.community_id = communities.get(&e.path).map(|&c| c.max(0) as u32);
            dto
        })
        .collect();
    let edges: Vec<NoteLinkDto> = links
        .into_iter()
        .map(|(from, to, relation)| NoteLinkDto {
            from,
            to,
            label: None,
            // NULL relation = plain body wikilink.
            kind: Some(relation.unwrap_or_else(|| "wikilink".to_string())),
        })
        .collect();

    let response = GraphQueryResponse {
        nodes,
        edges,
        total,
    };
```

- [ ] **Step 3: `handle_neighbors_impl` 补 `total: None`（DTO 已加字段，须补构造）**

`handle_neighbors_impl` 里 `GraphNeighborsResponse` 不含 total（它是另一个类型，无需改）。但其边构造 `NoteLinkDto { from, to, label: None, kind: None }`（约 202-210）**保持不变**（neighbors 不供 kind，`entry_to_dto` 已给 center/nodes 补齐新字段，编译自洽）。确认无遗漏：`GraphNeighborsResponse` 无 `total` 字段，不动。

> 只要 `entry_to_dto` 统一产出带新字段的 `NoteNodeDto`，`handle_neighbors_impl` 与 `handle_node_detail_impl` 无需改字段初始化。

- [ ] **Step 4: 扩展 handler 测试断言新字段**

在 `graph.rs` 的 `graph_query_uses_explicit_agent_id`（680-700）反序列化 `GraphQueryResponse` 后，追加断言：

```rust
    // total is agent-scoped and populated.
    assert!(resp.total.is_some(), "total should be populated");
    // nodes carry updated_at.
    assert!(resp.nodes.iter().all(|n| n.updated_at.is_some()));
    // edges carry a kind (plain wikilinks map to "wikilink").
    assert!(resp.edges.iter().all(|e| e.kind.is_some()));
```

> 该测试的种子笔记之间有链接（`seed_two_agents` 建的 note 带 body wikilink）；若某 agent 无边则 `all` 对空集恒真，断言仍成立。

- [ ] **Step 5: Core 编译门 + 测试（一次收尾 Task 1-4）**

Run: `cargo test -p alephcore --lib graph`
Expected: 新增 4 个测试 + 既有 graph 测试全 PASS，编译无警告。

- [ ] **Step 6: Commit（Core 侧）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add src/memory/notes/store.rs src/memory/store/sqlite/notes/ src/gateway/handlers/graph.rs src/gateway/handlers/graph_types.rs
git commit -m "gateway: enrich graph.query with edge kind, node community_id/updated_at, agent total"
```

---

## Task 5: 面板 adapter DTO 加字段（additive）

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`（`NoteNodeDto`、`GraphQueryResponse`）

**Interfaces:**
- Produces: 面板 `NoteNodeDto` 多 `community_id: Option<u32>` + `updated_at: Option<i64>`；`GraphQueryResponse` 多 `total: Option<usize>`。全 `#[serde(default)]`，旧响应仍可反序列化。

- [ ] **Step 1: 加字段**

`adapter.rs` 的 `NoteNodeDto` 改为：

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub link_count: usize,
    #[serde(default)]
    pub community_id: Option<u32>,
    #[serde(default)]
    pub updated_at: Option<i64>,
}
```

`GraphQueryResponse` 改为：

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct GraphQueryResponse {
    pub nodes: Vec<NoteNodeDto>,
    pub edges: Vec<NoteLinkDto>,
    #[serde(default)]
    pub total: Option<usize>,
}
```

- [ ] **Step 2: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过（字段仅新增，build_galaxy 暂不消费——Plan 3 才用）。

---

## Task 6: 面板截断提示角标（消费 `total`）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/canvas/mod.rs`

**Interfaces:**
- Consumes: `GraphQueryResponse.total`。
- Produces: 当 `total > nodes.len()` 时，画布右上角显示"显示前 N / 共 M"角标。

- [ ] **Step 1: 新增 truncation 信号并在 galaxy-build Effect 里填**

在 `GalaxyCanvasView` 里，`galaxy_data` 信号声明附近加：

```rust
    // (shown_count, total) when the graph.query hit the node cap; None otherwise.
    let truncation: RwSignal<Option<(usize, usize)>> = RwSignal::new(None);
```

在 galaxy-build Effect 的 `if let Some(ref r) = query_result { ... }` 内、`galaxy_data.set(...)` 之后加：

```rust
                truncation.set(
                    r.total
                        .filter(|&t| t > r.nodes.len())
                        .map(|t| (r.nodes.len(), t)),
                );
```

并在 agent-switch reset Effect 里（清 `galaxy_data.set(None)` 附近）加 `truncation.set(None);`。

- [ ] **Step 2: 在画布渲染角标**

在 `view!` 里 `<GalaxyCanvas .../>` 之后、`NodeDetailPanel` overlay 之前插入：

```rust
            {move || truncation.get().map(|(shown, total)| view! {
                <div class="absolute top-2 right-2 pointer-events-none text-[11px] text-white/70
                            bg-black/40 rounded px-2 py-0.5 select-none">
                    {format!("showing top {shown} of {total}")}
                </div>
            })}
```

- [ ] **Step 3: 编译门**

Run: `cargo check -p aleph-panel`
Expected: 通过。

- [ ] **Step 4: Commit（面板侧）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git add interfaces/webchat/src/canvas_engine/adapter.rs \
        interfaces/webchat/src/platform/wide/views/canvas/mod.rs
git commit -m "canvas: consume enriched graph.query DTOs + show node-cap truncation badge"
```

---

## 完成标准（Plan 2）

- `cargo test -p alephcore --lib graph` 绿（含 4 个新测试：edge kind、count_notes、community_ids、handler 新字段断言）。
- `cargo check -p aleph-panel` 干净。
- `graph.query` 响应实际带 kind/updated_at/total（community_id 冷缓存为 None，dream recompute 后出现）。
- 面板超 500 节点时显示截断角标。
- Plan 3（视觉编码：边按 kind 着色、社区质心引力成簇、新鲜度调亮）消费本 plan 供出的数据。
