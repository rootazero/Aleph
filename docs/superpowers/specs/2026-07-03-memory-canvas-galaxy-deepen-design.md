# 记忆管理 Canvas 知识图谱 — 深化设计 (Deepen)

**日期**: 2026-07-03
**范围**: `interfaces/webchat/src/platform/wide/views/canvas/` (3D WebGL 星系) + `interfaces/webchat/src/canvas_engine/` + Core 侧 `src/gateway/handlers/graph.rs` 少量供数据改动
**方向决策** (用户已批准): ①**深化现有 3D 星系**（不做 2D 视图、不重写渲染器）②**允许小的 Core 侧数据改动** ③**删除死掉的 JSON Canvas 转换器**
**落地方式**: 方法 A — 外科分层推进，只在降低风险处做局部重构

---

## 1. 背景与现状

记忆 Hub 的 `Graph` 视图 = 一个成熟的 **3D WebGL2 力导星系**（9 天前刚做过视觉/性能打磨，merged `e47879aa8`）。数据流：

```
MemoryHub → CanvasView/RadialCanvasView (mod.rs, 拥有全部 Effect + GraphApi + intent 信号)
  → GalaxyCanvas (galaxy_canvas.rs, 拥有 <canvas>/rAF/pointer, 桥接信号)
  → Scene (gl/scene.rs) → {NodeRenderer, EdgeRenderer, BloomPipeline, OrbitCamera, ForceLayout, picking}
```

单一加载路径：`graph.query`（上限 500 节点）→ `build_galaxy` 去重无向边 + 力导播种 → `GraphData{nodes, edges:(u32,u32)}` → `Scene::set_graph`。交互经 6 条 `Copy` intent 信号（focus/highlight/highlight_edges/lod/selected/hover）驱动非-Send 的 Scene。唯一写路径 = `NodeDetailPanel` 内联编辑正文 → `graph.update_note`。

### 已验证的问题（grep + `.codegraph/` 双重确认）

**死代码（零外部消费者）**
- `canvas_engine/` 2D 时代整套：`json_canvas/`(654) · `prefetch.rs`(406) · `scatter.rs`(208) · `layout.rs`(472) · `cluster.rs`(122) · `types.rs`(246, `CanvasNode/CanvasEdge/Neighborhood`) + `adapter.rs` 内 `to_neighborhood`/`populate_orphans`/`adapt_graph_response`
- `views/canvas/node_card.rs`(261) — `NodeCard` 从未挂载
- `GraphApi::neighbors` — 零调用者（radial 导航退役遗留）
- `MemoryState::{focus_id, breadcrumb_entries}` — 无读无写
- `mod.rs::all_dtos` — 写但从不读（"孤儿幽灵环"从未接线；且每次加载 clone 达 500 节点）

**真 bug / 断线**
- 🐞 "Recently visited" 永远为空：`push_recent` **0 个调用者**（`.codegraph/` 确认），列表永久显示 hint 空态
- 🐞 breadcrumb 永不显示：`NoteExcerpt.breadcrumb` 恒 `Vec::new()`
- 🐞 backlinks 被丢弃：`node_detail` 拉回 `backlinks` 但 canvas 面板未渲染（表格视图却用了）
- 🐞 500 节点静默截断，超限无任何提示
- `GalaxyNode` 丢弃了本可用的 `path`/`tags`
- 遗留命名 `RadialCanvasView`、失效注释 `active_request`/`fold-cluster`

**数据层天花板**
- `graph.query` 硬编码 `label:None, kind:None`（graph.rs:132）— 边语义（wikilink/semantic/related）在 DB 有、传输时丢，导致**所有边长得一模一样**
- `entry_to_dto` 丢弃 `created_at/updated_at`，且无 `community_id` — 无法按社区着色、按新鲜度编码

**性能**
- `pick`/`screen_pos_of`/`node_name`/`fly_to_node` 每次都 `nodes.iter().find(id)` = O(n)，每帧每次 hover 都扫
- `ForceLayout::step` = O(n²) 全对斥力，无社区分簇（category 只影响颜色不影响空间）

---

## 2. 目标与成功标准

覆盖 `/goal` 的四轴：**显示优化 · 深度架构重构+功能增强 · 细节打磨 · 错误修复和连线**。

**成功标准（可验证）**
1. `canvas_engine/` 与 `views/canvas/` 删除 ~2,400 行死代码后 `cargo check -p aleph-panel` 干净、无 dead_code 警告。
2. `graph.query` 返回的边带真实 `kind`，节点带 `community_id` + `updated_at`（新增 handler 测试通过）。
3. 星系中：不同关系类型的边**视觉可区分**；同社区节点**空间成簇**；越新的节点越亮。
4. "Recently visited" 点击节点后真实累积；backlinks 在详情面板可见；截断有明确提示。
5. `pick`/label 查询 O(1)（id→idx 索引），60fps 不回归。
6. 遗留命名/注释清理；`mod.rs` 的 6-Effect intent 图抽成独立 `intents` 模块。

---

## 3. 方法 A — 5 个工作流（按依赖排序）

### WS-1 · 死代码清除（架构瘦身，最先做，解阻塞）

**删除**
- `canvas_engine/{json_canvas/, prefetch.rs, scatter.rs, layout.rs, cluster.rs, types.rs}` 整文件 + `canvas_engine/mod.rs` 对应 `pub mod` 声明
- `canvas_engine/adapter.rs` 内死函数 `to_neighborhood`/`populate_orphans`/`adapt_graph_response` 及其 `#[cfg(test)]` 测试；保留仍活的 DTO（`NoteNodeDto`/`NoteLinkDto`/`GraphQueryResponse`/`NoteDetailResponse`/`SearchResultDto`/`GraphSearchResponse`）与 `GraphNeighborsResponse`（若 WS-2 不复用则一并删）
- `views/canvas/node_card.rs` 整文件 + `pub mod node_card`
- `views/canvas/mod.rs::all_dtos` 信号（声明/set 三处）
- `MemoryState::{focus_id, breadcrumb_entries}`（先 grep 复核零引用）
- 面板侧 `GraphApi::neighbors`（Core `handle_neighbors_impl` 是否连带死，**记为独立评估、本 WS 不动 gateway**）

**风险**: 低（纯删除）。**验证**: `cargo check -p aleph-panel` 干净。**红线**: R3/R10/P6（YAGNI 撤回，删不留口）。

### WS-2 · Core 侧供数据（R4 合法：Core 供数据、面板渲染）

**改 `NoteLinkDto`**: `graph.query` 的 `handle_query_impl` 让 `get_graph_data` 一并返回每条边的**关系 kind**（wikilink/semantic/related — 来自 `notes_graph_*` / link 表），填入 `NoteLinkDto.kind`，不再硬编码 `None`。需向下追 `MemoryBackend::get_graph_data` 的返回签名（当前 `(entries, Vec<(from,to)>)` → 加一列 kind）。

**改 `NoteNodeDto`**（**additive**、`Option`，不破旧客户端 / schemars 兼容）:
- `community_id: Option<u32>` — 来自 `src/memory/notes/graph/mod.rs` 已有的 community detection（`notes_graph_cache` 物化结果）
- `updated_at: Option<i64>` — `entry_to_dto` 已能拿到 `NoteIndexEntry.updated_at`，透传即可

**风险**: 中（触及 gateway handler + NoteStore 查询）。**验证**: 扩展 `src/gateway/handlers/graph.rs` 既有测试断言 kind/community/updated_at 出现；`cargo test -p alephcore` 相关 filter。**红线**: gateway/CLAUDE.md「改 handler 必须同步测试」；**不触认证/Origin**。

### WS-3 · 视觉编码增强（显示优化 + 功能增强，依赖 WS-2）

**边按 kind 区分**: `GraphData.edges` 从 `(u32,u32)` 升为 `(u32,u32,EdgeKind)`；`EdgeRenderer` 增一条 per-instance `a_kind`（或直接 per-instance 颜色/虚线相位）属性 → GLSL：
- `wikilink` = 实线、亮（结构主干）
- `semantic` = 冷色、半透明（语义近邻）
- `related` = 更弱/点画（结构相关兜底）
（保持"选中流光只在选中/搜索/定位触发、hover 不触发"的既有硬约束不变。）

**社区感知布局**: `ForceLayout::step` 增一项**社区质心引力**——每步先算各 community 的质心，节点受一个朝本社区质心的弱拉力（新常量 `COMMUNITY_PULL`）。效果 = 同社区在空间上成簇，不改斥力/弹簧主体，纯 additive 力项。无 community 的节点该项为 0（退化为现状）。

**新鲜度编码**: 节点亮度/微光随 `updated_at` 调制（近期 = 更亮，走既有 `hdr_boost` 或 shader `u_time` 无关的静态因子），旧节点自然黯淡。节点**填充色仍按 category**（语义可读），社区用**空间成簇**表达 + 可选细描边。

**风险**: 中（触及 GLSL 属性对齐 — 参考记忆 [[project-memory-canvas-galaxy-polish]] 的 `layout(location=N)` ↔ `setup_instanced` 对齐铁律）。**验证**: 原生纯逻辑单测（community 质心、edge-kind 映射、recency 因子）+ 浏览器观感 QA。

### WS-4 · 错误修复与连线（可与 WS-2/3 并行，独立）

- **修 "Recently visited"**: `on_event::SelectNode` 里调 `mem.push_recent(id)`（`push_recent` 已存在、只是没人调）。
- **接 breadcrumb**: 用 note `path` 的目录段填 `NoteExcerpt.breadcrumb`（有意义），或直接删死字段/UI（二选一，倾向填充）。
- **显示 backlinks**: `NodeDetailPanel` 渲染 `node_detail` 返回的 `backlinks`（可点击 fly-to），复用现有点击 → `selected_node` 通路。
- **截断提示**: `graph.query` 命中上限（返回数 == limit）时角标提示"显示前 500 个（按度+新近）"。总数可由 Core 顺带返回或本地推断（倾向 Core 加 `total` 字段，归入 WS-2）。
- **命名/注释**: `RadialCanvasView` → `GalaxyCanvasView`；清 `active_request`/`fold-cluster` 失效注释。

**风险**: 低-中。**验证**: 原生单测（push_recent 累积、breadcrumb 生成）；浏览器点击流。

### WS-5 · 交互与性能打磨（细节打磨，最后）

- **id→idx 索引**: `Scene` 持 `HashMap<String,u32>`，`set_graph` 时建，`pick`/`screen_pos_of`/`node_name`/`fly_to_node` 改 O(1)。
- **真正的 pan**（当前只有 orbit + wheel-zoom，无平移）: 相机增平移（如 shift-drag / 中键拖拽在视平面平移 center），参考 `infinite-canvas-tutorial` 的视图矩阵变换。
- **视口剔除**（可选、大图才启）: bbox/frustum cull 跳过屏外节点上传/绘制。
- **`mod.rs` intent 图抽取**（方法 B 的受限重构）: 把 6 条 intent Effect + `compute_highlight_*` 抽到 `views/canvas/intents.rs`，`mod.rs` 只留数据加载 + 组合。降低 486 行密集 Effect 图的维护成本。
- **T8 视觉微调**（承接 [[project-memory-canvas-galaxy-polish]] 遗留）: 星芒阈值 0.15 阶跃、EDGE_VERT t=1.0 切线回退、edges `hl.unwrap()` → `map_or`。
- **LOD 小优化**: `recompute_filtered_edges` 缓存已排序 counts，避免每次 set_lod tick 重排。

**风险**: 低。**验证**: 原生单测（id-index 命中）+ 浏览器帧率/pan 手感。

---

## 4. 参考项目应用（`/Volumes/TBU4/Github/`）

> 已由子代理精确定位（file:line）。下列为将移植进 3D 星系的具体技术点。

- **code-call-graph-editor (AntV X6)** → WS-3:
  - 边按 kind 一行查表配色 + 中性灰默认（`src/webview-x6/index.ts:143`），选中时加亮 incident 边 + 提 z-order（`:136-145`）→ 映射为 3D emissive。
  - **端点贴节点表面**（`connectionPoint:'boundary'`，边从节点边界起而非中心，`:94-118`）—— 子代理评为"最便宜的最大收益"：把边端点偏移到 `center ± radius·dir`，线不再穿过节点。星系已做 taper/weld，需复核是否等效。
  - 实线箭头=有向关系、`dasharray '5,5'` 虚线=试探/related（`:699-720`, `:833-837`）→ GL 线按 kind 选实/点画。
- **CodeGraphyV3** → WS-3/WS-5:
  - 布局委托 vis-network **Barnes-Hut 八叉树** O(n log n)（`src/view/Graph.vue:83-113`），默认常量可借：`centralGravity 0.3 / springLength 95 / springConstant 0.04 / damping 0.09 / theta 0.5`。**社区聚类两参考项目均无 → 本项目自建**（Core 已有 community detection，WS-3 加质心引力是 greenfield）。
  - 节点色 = 把 type/community-id 哈希进 **LCH chroma ramp**（`src/utils/nodes/colorNodes.ts:30-41`）——比 HSL 感知更均匀，采纳为 community 调色思路。
- **infinite-canvas-tutorial** → WS-5:
  - **真正 pan**（星系当前没有）: dragstart 快照 VP⁻¹ + 光标下世界点，每次 move 令 `camera += startWorld − worldUnderCursorNow`，抓取点钉在指针下（`plugins/CameraControl.ts:76-85, 109-150`）。
  - screen↔world 逆投影（含 y-flip 陷阱，`Camera.ts:297-318`）；**视口剔除** = 反投影 4(3D 为 8) 屏角求世界 AABB → 空间索引查询 → cull（`plugins/Culling.ts:15-92`），"反投影屏角求 min/max 盒"直接可移植 3D。
- **tldraw** → 边曲率 = 单 `bend` 标量的三点圆弧（`shapes/arrow/curved-arrow.ts:44-84`），适合**同一对节点间多条边扇形分开**；星系已做 Bézier 曲边，仅在需要多重边时参考。snap 阈值 = `常量屏幕px / zoom`（`SnapManager.ts:60-62`）。
- **jsoncanvas / react-jsoncanvas** → 因删除 JSON Canvas 转换器，不移植。

---

## 5. 数据模型变更汇总

| 类型 | 字段 | 变更 | 兼容性 |
|------|------|------|--------|
| `NoteLinkDto` | `kind: Option<String>` | 由 Core 真实填充（原恒 None） | additive，旧客户端忽略 |
| `NoteNodeDto` | `community_id: Option<u32>` | 新增，来自 community detection | additive |
| `NoteNodeDto` | `updated_at: Option<i64>` | 新增，透传 index entry | additive |
| `GraphQueryResponse` | `total: Option<usize>` | 新增，供截断提示 | additive |
| `gl::GraphData.edges` | `(u32,u32)` → 带 `EdgeKind` | 面板内部类型 | 内部 |
| `gl::GalaxyNode` | + `community/recency` | 面板内部类型 | 内部 |

所有 wire 层字段均 `Option` + additive，遵守 P3「Schema 驱动，新增字段不破旧客户端」。

---

## 6. 测试策略（尊重「极度节制 cargo」）

- **原生纯逻辑单测**（`cargo test -p aleph-panel --lib <filter>`，一条命令顺带编译整 lib=编译门）: community 质心引力、edge-kind 映射、recency 因子、id→idx 索引、push_recent 累积、breadcrumb 生成、fold_to_lod/dedup 既有回归。
- **Core handler 测试**（WS-2）: 扩展 `src/gateway/handlers/graph.rs` 既有 `#[tokio::test]`，断言 kind/community/updated_at/total 出现。
- **编译门**: 高风险节点至多一次 `cargo check -p aleph-panel` / `cargo check -p alephcore`。
- **浏览器 QA**: 视觉/交互（边区分、社区成簇、pan、label）需 `just wasm` 重建 dist 后在完整 App 内实测 —— ⚠️ **stale-embed 坑**：panel 经 `rust_embed` 编译期嵌入，改完不重建 binary 看不到效果。

---

## 7. 范围外 / 延后 (YAGNI)

- **结构化图编辑**（画布内建/删节点、拖拽重连边）: 无对应 Core RPC，改动大，本次不做（仅保留既有正文内联编辑）。
- **2D JSON Canvas 视图**: 用户已选 3D 方向，删除转换器。
- **Barnes-Hut/四叉树布局**: 500 上限下 O(n²) 可接受，记为 future。
- **提高 500 节点上限**: 先给截断提示；真放大需布局提速（关联上一条），延后。
- **替换 bloom/渲染管线**: 成熟稳定，不动。

---

## 8. 关联红线 / 记忆

- **R3/R10/P6**: WS-1 死代码撤回、不留口；本次全在面板/Core-数据层，不进 `src/harness/`。
- **R4**: WS-2 是 Core 供数据、面板渲染的合法分工。
- **R7/P8**: 不引入 regex/规则做语义判断（本任务本就是渲染/数据，无涉）。
- 记忆参考: [[project-memory-canvas-galaxy-polish]]（GLSL attr 对齐铁律、worktree target-dir 坑、dist stale-embed）、[[feedback-worktree-remove-safe-from-main-checkout]]。
