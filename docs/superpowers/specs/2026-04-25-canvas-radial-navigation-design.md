# Canvas Radial Navigation — Design Spec

**Date:** 2026-04-25
**Status:** Approved (brainstorming complete, awaiting plan)
**Scope:** Phase 2 — 将 Canvas 知识图谱从"全局 Top-K 力导向"翻转为"以 Active 节点为中心的辐射式邻域导航视图"，配合 2.5D 视觉景深、节点折叠、平滑焦点切换动画。
**Supersedes (partial):** [`2026-04-11-canvas-knowledge-graph-design.md`](./2026-04-11-canvas-knowledge-graph-design.md)

---

## 1. Overview

将 Canvas Knowledge Graph 的展示范式从 **"全局 Top-K 力导向图为主"** 翻转为 **"以当前 Active 节点为中心的辐射式邻域视图为主"**，配合 2.5D 视差、节点折叠、平滑焦点切换动画，使得 Aleph 的记忆图谱在节点数无限增长时仍保持优雅可用。

灵感来源：TheBrain 的 active-thought 导航范式 + Obsidian Graph 的 Local View 视觉语言 + Hyperbolic Tree 的 "focus + context" 思想。

核心设计哲学：**人类的视觉系统永远看不懂一千个节点同时在屏幕上**。真正能装下大不列颠百科全书规模图谱的方案必须接受一个前提——**数据可以无限大，但任意时刻渲染的永远只是当前焦点的局部邻域**。用户**走过**图谱，不是**看到**图谱。

### Goals

1. **导航式架构**：默认进入即聚焦在某个 Active 节点的 1-2 hop 邻域，单击邻居 → 平滑过渡为新 Active
2. **辐射式布局**：Active 钉死中心，邻居按 `relation` 类型分扇区围绕，半径由 hop 距离决定
3. **2.5D 视觉景深**：Z 轴模拟，Active 最近最大最亮，远 hop 节点更小更模糊更冷色
4. **节点折叠**：邻居 >12 同 kind 节点时自动折叠为超级节点，点击展开
5. **平滑焦点切换**：相机 + 节点位置 tween 动画（400ms ease-in-out），保留空间记忆感
6. **导航历史 breadcrumb**：复用现有 breadcrumb 组件，记录 Active 切换路径
7. **保留全局图为可选模式**：toolbar 切换 "Local（默认）↔ Global（极远缩放）"

### Non-Goals

- 真 3D 可旋转视角（方案 B 已否决）
- 节点编辑、连线编辑、自由拖动定位（Obsidian Canvas B 范式留给未来）
- WebGL 渲染、wgpu 切换（方案 2 留作未来 fallback；方案 3 因违反 R2 直接否决）
- 节点数 >1000 的 GPU 加速（导航范式让此需求消失）
- 跨 agent 图谱合并视图
- 布局持久化到 SQLite（仍是临时计算）

### Relationship to 2026-04-11 Spec

| 部分 | 处理方式 |
|---|---|
| Server API（`graph.query/neighbors/node_detail/search`） | **保留**，`graph.neighbors` 上升为主入口 |
| 全局 Top-K 力导向 | **降级为可选模式**，不再是默认 |
| Canvas 2D 渲染栈 | **保留** |
| 自研 Barnes-Hut 力导向 | **改造**为"以 Active 为中心的约束力导向" |
| 节点视觉编码（颜色/图标/大小） | **保留并增强**（加 Z 轴视差/阴影/glow） |
| 右侧 detail panel | **保留**，UI 不变 |
| 双击进 Local View / 单击选中 | **反转**：单击 = 切换 Active；选中状态被"Active"取代；双击保留兼容 |

---

## 2. Architecture

### 2.1 分层架构

```
┌──────────────────────────────────────────────────────┐
│ Frontend (Leptos/WASM) — interfaces/webchat/         │
│ ┌──────────────────────────────────────────────────┐ │
│ │ Navigation State Machine (新增)                   │ │
│ │   Active(id) ─click→ Animating ─done→ Active(id') │ │
│ ├──────────────────────────────────────────────────┤ │
│ │ Radial Layout Engine (改造自 force-directed)      │ │
│ │   active 钉中心 · 扇区分配 · 软约束半径 · 折叠     │ │
│ ├──────────────────────────────────────────────────┤ │
│ │ Tween Engine (新增)                               │ │
│ │   节点位置插值 · viewport 平移缩放 · ease-in-out   │ │
│ ├──────────────────────────────────────────────────┤ │
│ │ Renderer (Canvas 2D, 增强 2.5D)                   │ │
│ │   Z 排序 · 阴影 · 视差 · 远景模糊 · glow           │ │
│ └──────────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────────┤
│         ↕ WebSocket JSON-RPC 2.0 (无变化)             │
├──────────────────────────────────────────────────────┤
│ Server API — graph.neighbors 上升为主入口             │
│   (graph.query / search / node_detail 保留)          │
├──────────────────────────────────────────────────────┤
│ Data Layer (无变化)                                   │
└──────────────────────────────────────────────────────┘
```

### 2.2 红线 / 设计原则合规

| 红线/原则 | 合规说明 |
|---|---|
| **R1** 大脑-四肢分离 | 仅 web_sys Canvas 2D，无平台 API |
| **R2** UI 唯一在 Leptos/WASM | 全部新增逻辑在 `interfaces/webchat/`，无 JS 库 |
| **R3** Core 轻量化 | Server 侧零改动核心，所有展示逻辑前端完成 |
| **R4** Interface 纯 I/O | 前端只调 `graph.*` JSON-RPC，不做持久化或推理 |
| **R8** LLM 主权 | 辐射布局**不预设 relation 层级**，扇区角度由 relation 名 hash 决定（确定性映射，非语义判断） |
| **P1** 低耦合 | Navigation/Layout/Tween/Renderer 通过明确数据契约通信，可独立测试 |
| **P2** 高内聚 | 新增模块各自单一职责，文件 <500 行 |

### 2.3 数据流（典型路径：焦点切换）

```
1. 用户进入 Canvas tab
   └→ frontend 选起点（默认：当前 agent 最近活跃节点 = max(decay_score × recent_access)）
       └→ 调用 graph.neighbors { node_id, depth: 2, limit: 50 }

2. Server 返回 { center, nodes, edges, hop_depth }

3. RadialLayout 计算位置
   ├→ active 钉死 (0,0)
   ├→ 同 kind 邻居 >12 → 折叠为 ClusterNode
   ├→ 1-hop 邻居按 relation 扇区分配 → 半径 R₁ = 220 px
   └→ 2-hop 邻居挂在对应 1-hop 父节点扇区内 → 半径 R₂ = 400 px

4. Renderer 进入持续渲染循环
   └→ Z 排序（active=Z₀, 1-hop=Z₁, 2-hop=Z₂） · 视差 · 阴影 · glow

5. 用户单击邻居节点 N
   ├→ NavStateMachine: Active(A) → Animating(A→N)
   ├→ 异步调用 graph.neighbors { node_id: N }（hover 已预取则命中缓存）
   ├→ Tween: 旧 active 退到 N 在新布局中的位置 (~400ms)
   ├→ 新邻居淡入，旧邻居淡出
   └→ Animating 完成 → Active(N)，breadcrumb 追加

6. 用户点击 breadcrumb / 浏览器后退键
   └→ 走相同流程，反向动画
```

### 2.4 Server API 唯一变化点

`graph.neighbors` 响应需多带两个字段以避免后续 round trip：

```diff
  pub struct GraphNeighborsResponse {
+     pub center: GraphNodeDto,           // 当前 active 节点详情
      pub nodes: Vec<GraphNodeDto>,
      pub edges: Vec<GraphEdgeDto>,
+     pub hop_depth: HashMap<String, u8>, // node_id → 1 or 2
  }
```

其余 API 完全不动。Serde 默认忽略未识别字段，保证向后兼容。

---

## 3. Navigation State Machine

### 3.1 状态枚举

```rust
enum NavState {
    Idle,
    Loading { target: String, since: Instant },
    Active { node_id: String, neighborhood: Neighborhood },
    Animating {
        from_id: String,
        to_id: String,
        from_neighborhood: Neighborhood,
        to_neighborhood: Neighborhood,
        t: f32,                  // ∈ [0, 1]，每帧推进
        duration_ms: u32,        // 默认 400
        started_at: Instant,
    },
    Error { target: String, reason: String },
}
```

### 3.2 状态转移图

```
Idle ──enter canvas──▶ Loading
Loading ──neighbors fetched──▶ Active
Loading ──rpc fail──▶ Error
Error ──retry──▶ Loading

Active ──click neighbor (data not prefetched)──▶ Loading
Active ──click neighbor (data ready)──▶ Animating
Animating ──t == 1.0──▶ Active
Animating ──click during anim──▶ Animating  (cancel + restart, 锚点为当前 to_id)

Active ──breadcrumb click / back btn──▶ Loading
Active ──leave canvas──▶ Idle
```

### 3.3 起点选择（Idle → Loading）

按优先级回退：

1. URL hash 带 `#node=<id>`（支持深链/分享）→ 用该 id
2. localStorage 里有 per-agent `last_active_canvas_node` → 用上次焦点
3. `graph.query { limit: 1, sort_by: "weight" }` → 取权重最高节点
4. 图谱为空 → 进入 `Active::Empty` 占位态（提示"还没有记忆节点"）

### 3.4 焦点切换核心交互

| 触发 | 行为 |
|---|---|
| 单击 1-hop 邻居 | 立即切换 Active |
| 单击 2-hop 邻居 | 立即切换 Active |
| 单击 ClusterNode | **不切换 Active**，原地展开折叠节点 |
| 单击 active 自身 | 无操作 |
| 单击空白区 | 关闭 detail panel，active 不变 |
| Hover 邻居 ≥150ms | **预取**该邻居的 neighbors（后台 RPC，不阻塞 UI） |
| 双击邻居 | 同单击（保留双击是为了肌肉记忆兼容） |

### 3.5 预取策略

- **Hover 防抖**：鼠标 hover 邻居 ≥150ms 触发后台 `graph.neighbors`，结果缓存
- **缓存策略**：LRU，最多 20 个 neighborhood，TTL 60s
- **去重**：同一 node 1s 内只发一次请求
- **缓存命中** → `Active → Animating` 直接进入（流畅）
- **缓存未命中** → `Active → Loading → Animating`，Loading 期 ≤300ms 通常无感

### 3.6 动画中断处理

用户在 Animating(A→B) 中途又点击了 C：

1. 立即取消当前动画
2. 用"渲染快照"作为新 from_neighborhood（确保从用户当前看到的画面无缝继续）
3. 新建 Animating(B→C)（**锚点是 B 不是 A**）
4. breadcrumb 追加 B 和 C（不丢失中间步）

### 3.7 Breadcrumb 复用与扩展

复用现有 `views/canvas/breadcrumb.rs`，语义改为"导航历史栈"：

```
[🏠 起点] → [Rust] → [Ownership] → [Borrow Checker]  ← 当前 active
                ↑ 点这里 = 跳回该状态
```

- 上限 20 项，超出时折叠中间为 `…`
- 浏览器 back/forward 键映射到 breadcrumb 上下移动（用 `history.pushState` + hash fragment 实现，与 Leptos Router 隔离）
- 切换 agent → breadcrumb 清空

### 3.8 错误处理

| 场景 | 处理 |
|---|---|
| `graph.neighbors` RPC 失败 | 进入 `Error` 状态，画面保持上一帧最后一帧，弹 toast"加载失败，点击重试" |
| 邻居 id 在数据库已被删除 | 服务端返回 `not_found`，前端从缓存/breadcrumb 移除该 id |
| WebSocket 断连 | 全局重连，恢复后回到 `Active`（用最后一次 active id 重新 Loading） |
| 节点为空（孤立点） | 正常进入 Active，仅渲染 active 自身 + 空扇区提示"该节点暂无关联" |

### 3.9 状态持久化

- `last_active_canvas_node` 写入 `localStorage`（per-agent key）
- breadcrumb **不**持久化（关页面清空，避免无效历史）

---

## 4. Radial Layout Algorithm

### 4.1 坐标系

- **世界坐标**：active 节点钉死在 `(0, 0)`
- **极坐标转换**：`(r, θ) → (r·cos θ, r·sin θ)`，θ ∈ [0, 2π)，0 = 正右方，逆时针为正
- **Z 轴**：仅渲染用，不参与布局力学

### 4.2 半径层级

| 层 | 半径（默认值，可配置） | 包含节点 |
|---|---|---|
| `R₀` | 0 | active 节点 |
| `R₁` | 220 px | 1-hop 邻居 + 1-hop ClusterNode |
| `R₂` | 400 px | 2-hop 邻居 |
| `R_pad` | 60 px | 节点间最小间距（碰撞避免） |

半径自适应规则：当 1-hop 邻居数 N ≥ 16 时，`R₁ → 220 + 12·(N-16) px`，避免拥挤。

### 4.3 扇区分配（按 relation 类型）

为保证**同一图谱跨会话扇区相对位置稳定**（用户的空间记忆才有意义），扇区角度由 relation 名的确定性 hash 决定，不是随机：

```rust
fn sector_center_angle(relation: &str) -> f32 {
    let h = fnv1a_32(relation.as_bytes());
    (h as f32 / u32::MAX as f32) * std::f32::consts::TAU
}
```

但纯 hash 会导致扇区重叠。最终扇区分配两步走：

1. **收集本邻域出现的所有 relation 类型**，按 hash 升序
2. **均匀重排**到 `[0, 2π)`：第 i 个 relation 的扇区中心 `θᵢ = i · 2π / K`，其中 K = relation 种类数
3. **保持相对顺序**（hash 决定哪些 relation 在哪些 relation 旁边）

效果：扇区**绝对角度会随邻域变化**（K 不同），但**相对顺序稳定**——用户能形成"references 总是在 part_of 旁边"的空间直觉。

### 4.4 扇区宽度

每个 relation 扇区宽度 `Δθᵢ = 2π · nᵢ / N_total`，其中 nᵢ 是该 relation 的邻居数。

- 邻居多的 relation 占的扇区大（视觉上自然加权）
- 极小扇区（nᵢ = 1）保证最小角度 `Δθ_min = 0.15 rad ≈ 8.6°`，避免节点重叠到角度奇点

### 4.5 扇区内角度分配

扇区内 nᵢ 个节点按 `weight = decay_score × edge_count` **降序**，从扇区中心向两侧交替排列：

```
扇区中心 θᵢ
    │
    ├─ weight 第 1 高 → θᵢ
    ├─ 第 2 高 → θᵢ + Δ
    ├─ 第 3 高 → θᵢ - Δ
    ├─ 第 4 高 → θᵢ + 2Δ
    └─ ...
其中 Δ = Δθᵢ / (nᵢ + 1)
```

权重高的节点更靠近扇区中心 = 视觉锚点。

### 4.6 2-hop 节点挂靠

每个 2-hop 节点 N₂ **挂靠到引导它进入邻域的 1-hop 父节点 N₁ 的扇区**：

- 角度 = N₁ 的角度 ± `δ`（δ ∈ [-0.3, 0.3] rad，由 N₂ 在 N₁ 子节点中的 hash 索引决定）
- 半径 = R₂

边界情况：N₂ 同时被多个 N₁ 引入 → 挂靠到 weight 最高的那个 N₁。

### 4.7 软约束力导向（改造现有 Barnes-Hut）

布局**不是**纯几何摆放，而是几何摆放作为**目标位置**，用力导向逼近以避免重叠：

```rust
struct ForceConfig {
    target_attract: f32,   // 0.15 — 拉向几何目标位置的弹簧力
    repulsion: f32,        // 800 — 节点间斥力（保留 Barnes-Hut）
    damping: f32,          // 0.85
    pin_active: bool,      // true — active 永远钉死 (0,0)
    max_iterations: u32,   // 60 — 进入 Active 后最多 60 帧达到稳定
}

// 每帧：
// F_total[i] = target_attract * (target_pos[i] - pos[i])
//            + Σⱼ repulsion · (pos[i] - pos[j]) / |pos[i] - pos[j]|²
```

收敛后（总动能 < ε）力学停止，渲染不再触发 layout step（省 CPU）。

### 4.8 边界情况

| 场景 | 处理 |
|---|---|
| Active 没有邻居 | 仅渲染 active，画面中心显示，扇区空白处加文字"暂无关联记忆" |
| 所有邻居同一 relation | K = 1，单扇区占 360°，退化为同心圆均匀分布 |
| 邻居数 < 3 | 不做扇区，直接圆周等分 |
| 自环边（from = to = active） | 不渲染 |
| 重复边（同一对节点多 relation） | 取最高 weight 的 relation 决定扇区，其他 relation 作为 hover tooltip 显示 |

### 4.9 与现有 `canvas_engine/layout.rs` 的对应

- 保留：Barnes-Hut 四叉树、velocity 积分、damping
- 改造：力计算从"全图斥力 + 边吸引"改为"目标位置弹簧 + 全图斥力"
- 新增：`compute_target_positions(neighborhood)` 函数（极坐标几何），约 ~120 行
- 删除：边吸引力（不再需要，几何位置已隐式表达连接关系）

---

## 5. Folding & Clustering Mechanism

### 5.1 触发条件

折叠在 `compute_target_positions` 之前执行：

```
对每个 relation 扇区 S:
  按 kind 分组邻居 → groups: HashMap<kind, Vec<Node>>
  对每个 kind 组 G:
    if |G| >= FOLD_THRESHOLD (默认 12):
       折叠 G 为单个 ClusterNode
       ClusterNode 占用 S 的 1 个角度槽位
    else:
       G 中所有节点正常摊开，各占 1 槽位
```

边界细化：

| 场景 | 行为 |
|---|---|
| 一个 relation 扇区内有多个 kind 都 ≥12 | 各自独立折叠为多个 ClusterNode |
| 一个 kind 有 11 个 + 另一 kind 有 11 个 | 都不折叠（阈值是按 kind 算的） |
| 邻居总数 ≥30 但每个 kind 都 <12 | **触发兜底折叠**：按 weight 取 top 20，剩余按 kind 强制折叠 |

### 5.2 ClusterNode 视觉与数据

```rust
struct ClusterNode {
    id: String,                    // "cluster::<relation>::<kind>::<active_id>"
    relation: String,
    kind: String,
    member_ids: Vec<String>,
    representative_names: Vec<String>,  // top 3 weight 的 name
    aggregated_weight: f32,
    radius: f32,                   // 24 + 6·log₂(N) px，上限 60
    world_pos: Vec2,
    z: f32,
    expanded: bool,
}
```

视觉编码：

- **形状**：圆角矩形（区别于普通圆形节点，强暗示"组"）
- **颜色**：该 kind 的标准色 + 30% 透明度叠加，外加 2px 边框
- **标签**：`📚 +{N} {kind_plural}`，例如 `📚 +14 concepts`
- **图标**：使用 kind 图标的"叠加"变体（多个图标错位排列）
- **hover tooltip**：列出代表性 member 名（top 3）+ "点击展开"

### 5.3 展开/折叠交互

```
单击 ClusterNode  →  展开
  ├─ 设置 expanded = true
  ├─ ClusterNode 不消失，而是收缩为半径 ~14px 的"返回锚点"
  ├─ 原本 12+ 成员节点在该 ClusterNode 周围 ±0.4 rad 角度内分布（仅占用本扇区）
  ├─ 这些成员节点位置以 ClusterNode 中心为锚做径向展开动画 (280ms)
  └─ 其他扇区的邻居布局不变（避免全局重排打断空间记忆）

再次单击锚点  →  折叠
  └─ 反向动画
```

**关键约束**：展开**不切换 Active**。展开后的成员仍可作为新 Active 单击切换。

### 5.4 展开层级

仅支持**单层展开**——不允许递归展开嵌套 ClusterNode。理由：

- 当前邻域只有 1-hop 和 2-hop，本就两层
- 多层嵌套会让交互复杂化，违背"导航式简洁"原则
- 真要看更深层 → 把展开后的某个成员单击为新 Active（自然进入下一邻域）

### 5.5 与扇区角度的协调

折叠改变扇区内的"槽位数"：

```
未折叠：扇区有 N 个邻居 → N 个槽位
折叠 K 个 kind 组：扇区有 (N - sum(|Gᵢ|) + K) 个槽位
```

槽位数变化 → `Δ = sector_width / (slots + 1)` 重新计算。

视觉上 ClusterNode 与未折叠节点共享同一个角度调度（按 weight 降序，aggregated_weight 用于 ClusterNode 排序），不区别对待。

### 5.6 阈值的可调性

`FOLD_THRESHOLD` 默认 12，但允许两种动态调整：

| 输入 | 调整 |
|---|---|
| Toolbar 添加滑块 "拥挤度 / 详细度" | 滑动改变 [6, 20] 范围 |
| 邻居总数 N 极大（>50） | 自动降到 max(8, FOLD_THRESHOLD - 4) |
| 用户单击空白区双击 | 一次性"全部展开"（仅本邻域内所有 ClusterNode） |

### 5.7 ClusterNode 与缓存的交互

由于 ClusterNode 由前端折叠生成，不存在于 server 数据库：

- `graph.neighbors` 缓存里**只缓存原始 nodes/edges**
- 折叠每次进入 Active 时重新计算（成本可忽略：< 5ms）
- 阈值调整时重新折叠（不重新请求数据）

### 5.8 与 detail panel 的交互

单击 ClusterNode（折叠态）：

- detail panel 显示一个**特殊的 cluster summary 视图**：
  - 标题 `{kind} 群组（共 N 个）`
  - 列出所有成员名（可滚动）
  - 每个成员旁有"→ 跳转"按钮（点击 = 切 Active）
- 不显示 wiki 区（cluster 没有自己的 wiki）

### 5.9 边界情况

| 场景 | 处理 |
|---|---|
| 折叠后某 ClusterNode 只剩 1 个成员（用户改了阈值） | 自动还原为普通节点 |
| ClusterNode 被点击展开后 active 切换走 | 状态丢弃（每次进入新 Active 都从头折叠） |
| 同 kind 跨多 relation 扇区 | **不**跨扇区合并，各扇区独立判断折叠 |

---

## 6. 2.5D 视觉编码与渲染管线

### 6.1 Z 轴映射

Z 值不参与布局力学，仅用于渲染。每个节点的 Z 由其在导航拓扑中的层级决定：

| 节点类型 | Z 值 | 语义 |
|---|---|---|
| Active | `Z₀ = 0`（最近） | 主焦点 |
| 1-hop neighbor | `Z₁ = 60` | 直接关联 |
| 1-hop ClusterNode | `Z₁ = 60` | 同层 |
| 2-hop neighbor | `Z₂ = 140` | 间接关联 |
| 展开后的 cluster 成员 | `Z₁₊ = 75` | 略远于 1-hop（视觉次级） |

Z 值在 Animating 期间也参与插值（旧 active 从 0 退到 60，新 active 从 60 进到 0），形成"焦点切换的纵深感"。

### 6.2 Z 派生视觉变换

每帧渲染时，Z 通过统一函数派生为视觉参数：

```rust
fn depth_attrs(z: f32) -> DepthAttrs {
    let t = (z / 200.0).clamp(0.0, 1.0);   // t ∈ [0, 1]
    DepthAttrs {
        scale:      1.0  - 0.30 * t,        // 1.0 → 0.7
        opacity:    1.0  - 0.45 * t,        // 1.0 → 0.55
        blur_px:    4.0  * t,               // 0 → 4px (Canvas filter)
        sat_mul:    1.0  - 0.40 * t,        // 饱和度 1.0 → 0.6
        glow_alpha: (1.0 - t) * 0.6,        // 0.6 → 0
        shadow_offset_y: 6.0 + 4.0 * (1.0 - t),  // 近层投影更长
    }
}
```

特殊：Active 节点额外加**呼吸式 glow 脉动**（周期 ~2.5s，振幅 ±15%），可在 settings 关闭。

### 6.3 视差效应（Parallax）

用户拖拽 viewport 时，不同 Z 层以不同速度位移，模拟相机运动：

```rust
fn parallax_offset(layer_z: f32, camera_drag: Vec2) -> Vec2 {
    let factor = 1.0 - 0.15 * (layer_z / 200.0);  // 远层移动更慢
    camera_drag * factor
}
```

效果：拖动画面时，Active 跟手最快，2-hop 节点滞后约 11%，产生立体错位感。

**重要约束**：滚轮缩放不应用视差（否则缩放中心错位会非常违和）；仅平移触发视差。

### 6.4 边渲染（核心视觉升级）

| 边类型 | 渲染 |
|---|---|
| Active ↔ 1-hop | 二次贝塞尔曲线，控制点偏移 30px，从 active 端到目标端宽度 `2.5 → 1.0`，颜色 `#a78bfa → #4c1d95` 渐变 |
| 1-hop ↔ 2-hop | 二次贝塞尔，宽度 `1.5 → 0.8`，灰紫渐变 `#6b6b8a → #2a2a3a` |
| `relation == "references"`（wikilink） | 同上但虚线 `[5, 4]` |
| Hover 节点连接的边 | glow 描边（边外加 4px 模糊白色 stroke 半透明） |
| 不连接 Active 的"邻居间相互边" | 极淡 0.25 opacity，避免视觉噪音 |

控制点方向：从 active 出发的边，控制点垂直于"active→target"方向偏移，让多条边自然散开成扇形线束。

### 6.5 节点渲染细节

```
draw_node(node):
  attrs = depth_attrs(node.z)
  pos_screen = world_to_screen(node.world_pos + parallax_offset(node.z, drag))

  1. 投影：在 pos_screen + (0, attrs.shadow_offset_y) 画半透明椭圆（黑色 30%）
  2. Glow（如果是 hover/selected/active）：径向渐变 alpha=attrs.glow_alpha
  3. 主体：填充圆 radius * attrs.scale，颜色 = kind_color × attrs.sat_mul
  4. 边框：1px stroke #1a1a2a（暗背景下增加锐度）
  5. 图标：仅当 radius * scale >= 22px 时绘制
  6. Wiki badge：右下角 📖（如有，半径 ≥ 28px 才显示）
  7. Label：节点下方 12px 处，font 14px * attrs.scale，颜色 white * attrs.opacity
  8. 应用 attrs.blur_px（仅 2-hop 节点用，1-hop 不模糊以免读不清）
```

`ctx.filter = "blur(Npx)"` 在 Chrome/Safari 都支持但慢；**仅 2-hop 节点开 blur**，1-hop 直接清晰渲染。

### 6.6 渲染管线（每帧）

```
1. clear canvas
2. 绘制背景渐变（径向，中心偏暗 #050510，边缘 #0a0a1a）
3. 应用 viewport transform (translate, scale)
4. 按 Z 降序排序：[2-hop nodes, 2-hop edges] → [1-hop edges] → [1-hop nodes] → [Active]
5. 分层绘制：
   layer A (z=140):
     edges (1-hop ↔ 2-hop)
     2-hop nodes
   layer B (z=60):
     edges (Active ↔ 1-hop)
     1-hop nodes 和 ClusterNodes
   layer C (z=0):
     Active node
6. 顶层 overlay（不参与 transform）：
   - Toolbar
   - Breadcrumb
   - Detail panel（若打开）
   - Hover tooltip
   - Mini-map
```

### 6.7 性能优化

| 优化 | 实现 |
|---|---|
| 收敛冻结 | 力导向稳定后停止 layout step；仅 Animating / 用户拖拽 / hover 触发重绘 |
| Off-screen culling | 节点 world_pos 在 viewport 外 + margin → 跳过绘制（edges 仍要画跨界部分） |
| Blur 缓存 | 同一帧内 blur 滤镜值不变 → 一次设置，2-hop 节点批量绘制 |
| Glow 离屏画布 | hover/selected glow 用 OffscreenCanvas 预渲染，避免每帧重算径向渐变 |
| Animating 期间限帧 | RAF 自带帧率匹配显示器 |
| 静态期降帧 | 收敛后停止 RAF 循环，仅事件触发重绘 |

### 6.8 性能预算

由于 TheBrain 范式让屏上节点永远 ≤50：

- 60fps 目标：≤50 节点
- 30fps 目标：≤200 节点（仅"全部展开多个 ClusterNode"才会触及）
- ≥1000 节点不再是目标（导航范式让这种状态不存在）

### 6.9 暗色主题与可访问性

- 默认深色主题（`#0a0a0f` 背景），保留旧 spec 配色
- 计划中：未来加 light theme switch（颜色 token 化即可，不在本 spec 范围）
- 对比度：节点 label 与背景对比度 ≥ 7:1（WCAG AAA），Z 衰减后仍 ≥ 4.5:1（AA）
- 焦点指示不只靠颜色：Active 用呼吸 glow + 居中位置 + 最大尺寸三重信号
- 为色盲：kind 区分还有图标（不只靠颜色）

---

## 7. Animation Engine & Mini-map

### 7.1 动画职责划分

| 模块 | 负责的动画 | 触发 |
|---|---|---|
| **TweenEngine** | 节点位置插值、Z 插值、opacity 淡入淡出 | NavState::Animating |
| **CameraTween** | viewport.offset / viewport.scale 平移缩放 | 焦点切换、breadcrumb 跳转、minimap 点击 |
| **ContinuousAnim** | Active 呼吸 glow、hover 高亮过渡 | 持续运行 |
| **LayoutForce** | 力导向收敛 | Section 4 |

四者在同一 RAF 循环内推进，但有独立的 t 计时器。

### 7.2 焦点切换动画（核心场景）

进入 Animating 时已确定两套 neighborhood 的 target_positions。

**插值规则**（每帧 t ∈ [0, 1] 推进 `dt / 400ms`）：

```rust
fn lerp_node(node_id: &str, t: f32) -> RenderNode {
    let from_pos = old_neighborhood.target_pos(node_id);
    let to_pos   = new_neighborhood.target_pos(node_id);
    let from_z   = old_neighborhood.z(node_id);
    let to_z     = new_neighborhood.z(node_id);

    match (from_pos, to_pos) {
        // 共同节点（两边都有）
        (Some(p1), Some(p2)) => {
            pos:     lerp(p1, p2, ease_in_out(t)),
            z:       lerp(from_z, to_z, ease_in_out(t)),
            opacity: 1.0,
        },
        // 仅旧邻域有（要淡出）
        (Some(p1), None) => {
            pos:     p1 + drift_outward(t),    // 轻微外推 30px
            z:       lerp(from_z, 200.0, t),
            opacity: 1.0 - t,
        },
        // 仅新邻域有（要淡入）
        (None, Some(p2)) => {
            pos:     p2 + drift_outward(1.0 - t),
            z:       lerp(200.0, to_z, t),
            opacity: t,
        },
        _ => unreachable!()
    }
}
```

`ease_in_out(t) = 3t² - 2t³`（标准 smoothstep）。

### 7.3 共享节点的视觉锚点

A → N 切换中，`N` 自己是共享节点：

- 旧邻域中 N 在某个 1-hop 扇区位置
- 新邻域中 N 在中心 (0,0)
- 动画期间 N 的位置从扇区位置插值到中心 → **它"飞向"中心成为新焦点**

这是空间记忆的关键：用户的眼睛追随 N，整个布局以 N 为锚旋转/重排，而不是"画面突变"。

### 7.4 Camera Tween

```
焦点切换：
  起点 camera = 当前用户拖拽到的位置
  终点 camera = 让 (0,0) 居于屏幕正中
  插值：与节点 tween 同步推进 t，使用相同 duration 和 easing
```

Breadcrumb 跳转：相同流程，但起点是当前 active 已居中，终点也是新 active 居中（视觉只看到节点重排，相机不动）。

### 7.5 动画中断（呼应 Section 3.6）

```rust
fn start_new_animation(&mut self, target: String) {
    let snapshot = self.current_render_snapshot();
    self.nav_state = NavState::Animating {
        from_id: self.current_anim.to_id.clone(),
        to_id: target,
        from_neighborhood: snapshot.into_pseudo_neighborhood(),
        to_neighborhood: prefetched(&target),
        t: 0.0,
        duration_ms: 400,
        started_at: Instant::now(),
    };
}
```

### 7.6 动画持续时间

| 动画 | 默认 ms | 可配置 |
|---|---|---|
| 焦点切换 | 400 | ✅ user prefers-reduced-motion 时降到 100 |
| Cluster 展开/折叠 | 280 | ✅ |
| 节点淡入淡出 | 同焦点切换 | - |
| Hover glow | 120 | 不可配 |
| Active 呼吸 | 2500（周期） | ✅ 可关闭 |

`prefers-reduced-motion: reduce`（系统 a11y）→ 全部动画时长缩为 50-100ms，呼吸 glow 关闭。

### 7.7 Mini-map（全局感知补偿）

由于不再有"全局 Top-K"作为默认视图，需要 mini-map 让用户感知图谱总体规模和当前位置。

**位置**：右下角，160×120 px，半透明背景 `rgba(20,20,30,0.7)`。

**渲染**：

- 每次 Active 切换后异步请求 `graph.query { limit: 200 }` 作为全局采样
- 用极简方式画：节点 = 1px 点，颜色按 kind；边不画
- **当前 Active 用红色实心圆 + 1px 白色描边**
- 1-hop 邻居稍亮，其他暗
- 鼠标悬停 → 显示十字定位线
- 单击任意节点 → 切换 Active 到该节点（与单击邻域节点等效，走相同动画）
- 拖拽 mini-map = pan 当前邻域 viewport（视为相机控制）

**性能**：mini-map 的 200 节点采样**单独缓存**，TTL 5 分钟，不每次焦点切换重取。

**降级**：若图谱总节点 ≤ 30，mini-map 隐藏。

### 7.8 Toolbar 演化

```
[🤖 agent ↗] [🔍 Search] [📍 Local | 🌐 Global] [📚 详细度▎▎▎▎] [⚙ Filter]
```

| 元素 | 行为 |
|---|---|
| Agent 标签 | 不变 |
| Search | 不变（结果命中 → 切 Active） |
| **Local/Global 切换** | 默认 Local；切到 Global = 旧 spec 的 Top-K 全局力导向，作为概览模式 |
| **详细度滑块**（新增） | 控制 FOLD_THRESHOLD ∈ [6, 20]，实时重排 |
| Filter | 不变 |

### 7.9 键盘导航（新增）

| 键 | 动作 |
|---|---|
| `Tab` / `Shift+Tab` | 在当前邻居间循环 hover focus |
| `Enter` | 切换到 hover focus 节点为 Active |
| `Esc` | 关闭 detail panel / 折叠所有展开的 ClusterNode |
| `Backspace` / `Alt+←` | breadcrumb 后退 |
| `Alt+→` | breadcrumb 前进 |
| `/` | 聚焦 Search 输入框 |
| `G` | toggle Global / Local 模式 |

---

## 8. File Map · Data Types · Change Scope

### 8.1 文件改动清单

#### 前端：`interfaces/webchat/src/canvas_engine/`

| 文件 | 现行 LOC | 改动 | 预估 LOC | 说明 |
|---|---|---|---|---|
| `mod.rs` | 6 | 修改 | 12 | 新增模块导出 (`navigation`, `tween`, `cluster`, `mini_map`, `prefetch`) |
| `types.rs` | 150 | 修改 | 240 | 新增 `NavState`, `Neighborhood`, `ClusterNode`, `RenderNode`, `DepthAttrs` |
| `adapter.rs` | 92 | 修改 | 130 | 新增 `to_neighborhood()`，处理 hop_depth 字段 |
| `viewport.rs` | 70 | 修改 | 110 | 新增 `parallax_offset()`, `world_to_screen_with_z()` |
| `layout.rs` | 115 | 重写 | 280 | 改为辐射式约束力导向；删除边吸引；新增 `compute_target_positions()`, `assign_sectors()`, `place_two_hop()` |
| `renderer.rs` | 214 | 修改 | 360 | 新增 Z 排序、视差、阴影、blur、glow、贝塞尔边、呼吸 glow |
| `interaction.rs` | 52 | 修改 | 110 | 新增 hover 预取定时器、键盘事件、单击切换语义 |
| **`navigation.rs`** | 0 | 新增 | 200 | NavState 状态机、起点选择、breadcrumb 历史栈 |
| **`tween.rs`** | 0 | 新增 | 180 | 节点 tween、camera tween、easing、中断处理 |
| **`cluster.rs`** | 0 | 新增 | 150 | 折叠规则、ClusterNode 构造、展开/折叠状态 |
| **`mini_map.rs`** | 0 | 新增 | 130 | mini-map 渲染、采样缓存、点击映射 |
| **`prefetch.rs`** | 0 | 新增 | 90 | hover 预取、LRU neighborhood 缓存 |

`canvas_engine/` 总量：**813 → 1992 行**（+1179）

#### 前端：`interfaces/webchat/src/views/canvas/`

| 文件 | 现行 LOC | 改动 | 预估 LOC | 说明 |
|---|---|---|---|---|
| `mod.rs` | 221 | 修改 | 280 | 接入 NavStateMachine、tween 循环、键盘绑定、mini-map 容器 |
| `toolbar.rs` | 81 | 修改 | 130 | 新增"详细度"滑块、Local/Global 切换按钮 |
| `graph_canvas.rs` | 370 | 修改 | 460 | 接入预取、键盘事件、动画中断；renderer 调用面拓展 |
| `detail_panel.rs` | 96 | 修改 | 150 | 新增 cluster summary 视图分支 |
| `breadcrumb.rs` | 41 | 修改 | 90 | 改为导航历史栈语义、加 history.pushState 同步 |

`views/canvas/` 总量：**809 → 1110 行**（+301）

#### 前端：`api/`

| 文件 | 改动 | 说明 |
|---|---|---|
| `api/graph.rs` | 修改（小） | `graph.neighbors` 响应类型加 `center` 和 `hop_depth` 字段反序列化 |

#### 后端：`src/gateway/handlers/`

| 文件 | 改动 | 说明 |
|---|---|---|
| `graph.rs` | 修改（小） | `graph_neighbors` handler 多组装 `center` + `hop_depth` 字段 |
| `graph_types.rs` | 修改（小） | `GraphNeighborsResponse` 加两个字段 |

后端改动 **<50 行**，向后兼容（旧 frontend 不读新字段无影响）。

#### 不动的部分

- 数据库 schema：完全不动
- `MemoryStore` / `GraphStore` 接口：不动
- agent 隔离逻辑：不动
- WebSocket 协议：不动
- 其他 panel（Chat / Dashboard / Agents / Settings）：不动

### 8.2 模块依赖关系

```
graph_canvas.rs (View)
    │
    ├── navigation.rs ────────┐
    │       │                 │
    │       ├── prefetch.rs ──┤
    │       │   │             │
    │       │   └── api/graph.rs (RPC)
    │       │
    │       └── tween.rs
    │           │
    │           └── (no deps; pure math)
    │
    ├── layout.rs
    │       │
    │       └── cluster.rs
    │
    ├── renderer.rs
    │       │
    │       └── viewport.rs
    │
    ├── interaction.rs
    │
    └── mini_map.rs
            │
            └── api/graph.rs (separate cached request)

types.rs ← 所有 module 共享
```

依赖方向严格单向，无循环。`tween.rs` 是纯函数模块（不依赖 web_sys，可单元测试）。

### 8.3 关键数据类型完整定义

```rust
// types.rs

pub enum NavState {
    Idle,
    Loading { target: String, since: Instant },
    Active { node_id: String, neighborhood: Neighborhood },
    Animating {
        from_id: String,
        to_id: String,
        from_neighborhood: Neighborhood,
        to_neighborhood: Neighborhood,
        t: f32,
        duration_ms: u32,
        started_at: Instant,
    },
    Error { target: String, reason: String },
}

pub struct Neighborhood {
    pub center: CanvasNode,
    pub one_hop: Vec<CanvasNode>,
    pub two_hop: Vec<CanvasNode>,
    pub clusters: Vec<ClusterNode>,
    pub edges: Vec<CanvasEdge>,
    pub target_positions: HashMap<String, Vec3>,
    pub fetched_at: Instant,
}

pub struct CanvasNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub icon: char,
    pub color: Color,
    pub radius: f32,
    pub has_wiki: bool,
    pub decay_score: f32,
    pub edge_count: usize,
    pub world_pos: Vec2,
    pub velocity: Vec2,
    pub z: f32,
    pub hop: u8,                        // 0=active, 1, 2
    pub pinned: bool,
}

pub struct ClusterNode {
    pub id: String,
    pub relation: String,
    pub kind: String,
    pub member_ids: Vec<String>,
    pub representative_names: Vec<String>,
    pub aggregated_weight: f32,
    pub radius: f32,
    pub world_pos: Vec2,
    pub z: f32,
    pub expanded: bool,
}

pub struct CanvasEdge {
    pub from_idx: usize,
    pub to_idx: usize,
    pub relation: String,
    pub weight: f32,
    pub is_wikilink: bool,
    pub is_active_link: bool,           // 是否连接 Active 节点
}

pub struct DepthAttrs {
    pub scale: f32,
    pub opacity: f32,
    pub blur_px: f32,
    pub sat_mul: f32,
    pub glow_alpha: f32,
    pub shadow_offset_y: f32,
}

pub struct Vec3 { pub x: f32, pub y: f32, pub z: f32 }
```

### 8.4 改动占比估算

| 类别 | 行数 | 占现行 |
|---|---|---|
| 现行总量 | ~1622 行（1508 + ~110 后端） | 100% |
| 保留不动 | ~580 行 | 36% |
| 修改（同文件内编辑） | ~1042 行 | 64% |
| 新增（新文件） | ~750 行 | — |
| 删除 | ~30 行（layout 边吸引等） | 2% |

**实际"重写"比例约 30-35%。**

### 8.5 文件大小检查

按 Aleph 规范（CLAUDE.md P2：单文件 < 500 行），所有新增/修改文件均控制在：

- `renderer.rs`: 360 ✅
- `graph_canvas.rs`: 460 ✅（接近上限，实施时若超出考虑拆分）
- `layout.rs`: 280 ✅
- `mod.rs (views)`: 280 ✅
- `types.rs`: 240 ✅
- 其他全部 < 200 ✅

---

## 9. Testing · Migration · Risks & Rollback

### 9.1 测试策略

#### 单元测试（纯逻辑模块，无 web_sys 依赖）

| 模块 | 关键测试用例 |
|---|---|
| `tween.rs` | smoothstep easing 边界（t=0/0.5/1）、节点 lerp 三种分支（共享/淡出/淡入）、动画中断快照插值连续性 |
| `layout.rs` | 扇区角度 hash 确定性（同 input 同 output）、扇区相对顺序稳定（hash 升序）、扇区宽度按 nᵢ 加权、2-hop 挂靠到正确父节点、收敛性（60 iter 内 KE < ε） |
| `cluster.rs` | 折叠阈值 12 边界、兜底折叠（≥30 邻居）、单层不嵌套、ClusterNode 半径公式 `24 + 6·log₂(N)` |
| `navigation.rs` | 状态转移合法性（Idle→Loading→Active）、breadcrumb 上限 20 截断、动画中断锚点为 B 而非 A |
| `prefetch.rs` | LRU 容量 20、TTL 60s 过期、hover 防抖 150ms |
| `viewport.rs` | parallax 公式（layer_z=0 时 factor=1.0；layer_z=200 时 factor=0.85）、world↔screen 互逆 |

目标覆盖率：纯逻辑模块 ≥ 85%（高于全局 80% 要求）。

#### 集成测试（Server API）

| 用例 | 验证 |
|---|---|
| `graph.neighbors` 响应包含 `center` 字段 | 字段存在且 = 请求 node_id |
| `graph.neighbors` 响应包含 `hop_depth` | 1-hop 节点 = 1，2-hop 节点 = 2 |
| 旧 frontend 调 `graph.neighbors` 不读新字段 | 向后兼容 |
| agent 隔离仍生效 | 不同 agent 返回不同邻域 |

#### 视觉/交互回归测试（手动 walkthrough）

- [ ] 进入 Canvas → 默认聚焦最近活跃节点
- [ ] 单击 1-hop 邻居 → 平滑动画切换 Active
- [ ] 动画中途再点击 → 不卡顿，新动画从当前快照继续
- [ ] hover 邻居 ≥150ms → 网络面板可见预取请求
- [ ] 点击 ClusterNode → 原地展开，Active 不变
- [ ] Mini-map 点击远处节点 → 切到 Active 该节点
- [ ] 切换 agent → breadcrumb 清空，邻域重置
- [ ] `prefers-reduced-motion: reduce` → 动画 ≤100ms，呼吸 glow 关闭
- [ ] 100 节点邻域（含展开）→ 60fps 流畅

#### 性能基准

`tests/canvas_perf.rs`（可选 nightly）：

```rust
#[test]
fn layout_converges_under_60_iter() {
    let nbhd = mock_neighborhood(50);
    let mut layout = RadialLayout::new(nbhd);
    for _ in 0..60 { layout.step(0.016); }
    assert!(layout.kinetic_energy() < 1.0);
}
```

### 9.2 Migration / Rollout 策略

#### 阶段 0：feature flag 切换

新增运行时配置（不是编译期 feature flag——Aleph 规约只保留测试 features）：

```rust
// shared-ui-logic 里的 user prefs
struct CanvasMode {
    radial_navigation: bool,    // false = 旧 Top-K 全局图，true = 新辐射邻域
}
```

默认 **false**（保持旧体验），用户在 Settings 里手动开启。两套代码共存于同一个 `views/canvas/mod.rs`。

#### 阶段 1：Beta（2 周）

- 收集开启 flag 用户的反馈
- 重点观察：是否有"找不到全局图"的迷失感、动画流畅度、folding 阈值合理性

#### 阶段 2：默认切换

- `radial_navigation = true` 成为默认
- 旧 Top-K 全局图保留为 toolbar 的 "Global" 模式（不删代码）

#### 阶段 3：清理（≥1 个月稳定后）

- 删除 feature flag（双模式都保留，不再有"切换"）
- 旧 spec 标记为 superseded

#### 数据兼容性

零数据库 migration。所有改动在前端 + 一处后端字段追加。**回滚 = revert commits**，无残留状态。

### 9.3 风险登记表

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 用户找不到"全局图"，迷失方向 | 中 | 中 | mini-map 永久可见；toolbar Local/Global 切换显眼；初次进入 onboarding tooltip |
| 焦点切换 400ms 动画在低端机卡顿 | 中 | 中 | 检测帧率 <30 自动缩短动画到 200ms；`prefers-reduced-motion` 兜底 |
| 扇区 hash 在某些 relation 集合下视觉拥挤 | 低 | 中 | 兜底"均匀重排"已在 Section 4.3；测试覆盖极端 relation 数 |
| Canvas 2D blur filter 在 Safari 性能差 | 中 | 低 | 仅 2-hop 节点 blur；检测 UA 后可降级为半透明叠加 |
| ClusterNode 阈值不合用户口味 | 高 | 低 | toolbar 详细度滑块开放调节 |
| 预取请求暴增打爆 server | 低 | 中 | LRU 缓存 + 同一 node 1s 内只发一次请求（去重） |
| 力导向不收敛抖动 | 低 | 中 | 60 帧硬上限；上限到达后强制 freeze 到目标位置 |
| breadcrumb history.pushState 与 URL 路由冲突 | 中 | 低 | 用 hash fragment（`#node=xxx`）而不是 path |
| 双击/单击语义变更让老用户困惑 | 中 | 低 | 单击 = 切 Active（新）；双击保留同样行为（兼容肌肉记忆） |

### 9.4 回退策略

| 触发条件 | 回退动作 |
|---|---|
| Beta 阶段严重 bug（崩溃/数据错误） | revert 整个 PR；feature flag 默认值无影响 |
| 性能不达标（<30fps 持续） | 关闭呼吸 glow + 关闭 blur + 关闭 parallax，三档降级开关 |
| 用户大量反馈不喜欢导航式 | 长期保留双模式，不强制切换默认；用户偏好持久化 |
| 扇区/折叠算法有 bug | 单独 revert `layout.rs` + `cluster.rs`，回到旧 force-directed |

### 9.5 实施顺序建议

按依赖与风险排序，预估 10-12 个工作日：

1. **Day 1-2**：types.rs 拓展 + adapter.rs 改造 + server 字段追加（解锁所有后续）
2. **Day 2-4**：layout.rs 辐射布局算法 + cluster.rs（核心数学，纯逻辑可单元测试）
3. **Day 4-5**：navigation.rs + prefetch.rs（状态机 + 缓存）
4. **Day 5-7**：tween.rs + renderer.rs 增强（动画 + 视觉）
5. **Day 7-8**：mini_map.rs + viewport.rs 视差
6. **Day 8-9**：interaction.rs 键盘 + hover；views/canvas 接线
7. **Day 9-10**：toolbar 详细度滑块 + Local/Global 切换；feature flag 接入
8. **Day 10-12**：手动 walkthrough、性能调优、bug 修复

并行机会：layout/cluster 与 tween/renderer 两条线可在 Day 4 后并行。

### 9.6 成功判据

完成本 spec 实施后须满足：

1. ✅ 进入 Canvas 默认看到辐射式邻域（非全局图）
2. ✅ 单击邻居 400ms 内平滑切换 Active，无白屏/抖动
3. ✅ 100 节点邻域（含展开）保持 60fps
4. ✅ Beta 用户主观评价"看起来更专业/更易用"
5. ✅ 手动 walkthrough 全部通过
6. ✅ 单元测试覆盖率 ≥ 85%（纯逻辑模块）
7. ✅ 后端零数据库改动；前端可独立回滚
8. ✅ 所有红线（R1/R2/R3/R4/R8）合规

---

## Appendix A: 参考项目

实施过程中可参考的开源项目（按价值排序）：

**TheBrain 范式直接对标**
- [`vasturiano/3d-force-graph`](https://github.com/vasturiano/3d-force-graph) — 自带 `focusOnNode` / `cameraPosition` API，看 demo 与源码学习交互范式（不抄渲染）
- [`vasturiano/react-force-graph`](https://github.com/vasturiano/react-force-graph) — 2D 版本，含 cluster API
- TheBrain 官方文档 + YouTube demo — 商业产品但机制公开

**焦点+上下文奠基方案（学术起源）**
- Hyperbolic Tree / H3Viewer / StarTree（Inxight） — "中心放大、边缘指数缩小"思想
- D3 `d3-hierarchy` + `d3-zoom` 组合 — radial tree + focus zoom 的现成数学

**大规模图渲染（如未来要 10k+ 节点）**
- [`Sigma.js`](https://www.sigmajs.org/) — WebGL，专为大规模图设计
- [`deck.gl` GraphLayer](https://deck.gl/) — GPU 加速，10万+ 节点
- Cytoscape.js + `cytoscape-expand-collapse` + `cytoscape-cose-bilkent`

**Rust/WASM 原生（保住 R2/R3）**
- [`force_graph`](https://crates.io/crates/force_graph) crate
- [`fdg-sim`](https://crates.io/crates/fdg) crate — 2D/3D 力导向都支持
- `petgraph` — 图数据结构

---

## Appendix B: 名词表

| 名词 | 定义 |
|---|---|
| Active (节点) | 当前焦点，钉死在画布中心 (0,0) |
| 邻域 (Neighborhood) | Active 节点 + 其 1-hop 邻居 + 2-hop 邻居 + 它们之间的边 |
| 扇区 (Sector) | Active 周围按 relation 类型划分的角度区间 |
| ClusterNode | 同 relation 同 kind 邻居 ≥12 时的折叠"超级节点" |
| Hop | 节点到 Active 的最短路径长度（0/1/2） |
| 视差 (Parallax) | 不同 Z 层在用户拖拽时位移速度不同的视觉效果 |
| Mini-map | 右下角的全局图缩略图，作为导航辅助 |
| Breadcrumb | 导航历史栈，记录 Active 切换路径 |
