# Memory Canvas → WebGL2 3D 知识星云（Design Spec）

- **日期 / Date**: 2026-06-23
- **分支 / Branch**: `worktree-memory-canvas-3d-nebula`（off local `main` HEAD）
- **范围 / Scope**: `interfaces/webchat`（Panel / Leptos+WASM）。**Core 不改动。**
- **参考 / Reference**: `codebase-memory-mcp/graph-ui`（React Three Fiber + Three.js + postprocessing）

---

## 1. 背景与目标 (Background & Goals)

把 Aleph 记忆管理的 **Canvas2D 邻域浏览器** 替换为 **WebGL2 全图 3D 知识星云**，复刻参考项目的视觉：

- 真 3D 旋转（轨道相机 + 阻尼 + 空闲自转）
- 节点/线条精细，**数百~低千节点**清晰，架构上**可扩展至万级**
- 星云般辉光（bloom 后处理 + HDR 配色）
- 不丢任何现有功能：所有交互在新 3D 视图上重新接通

技术约束：Panel 是**全栈 Rust/WASM**，参考项目的 Three.js（JS）不可直接照搬 → **用纯 Rust WebGL2（web-sys + 手写 GLSL）** 实现。

### 已确认的设计决策 (Settled Decisions)

| # | 决策 | 选择 |
|---|------|------|
| D1 | 视图形态 | **替换** Canvas2D 邻域浏览器为 3D 全图星云；邻域 = 相机聚焦 + 高亮子集 |
| D2 | 渲染技术 | **纯 Rust WebGL2**（web-sys + 手写 GLSL），零额外 JS，零重型 crate |
| D3 | 布局位置 | **客户端 WASM** 算 3D 力导向（呈现计算，core 不动；与现有 `compute_target_positions` 同位） |
| D4 | 规模目标 | 数百~低千现实，**万级为设计上限**（实例化渲染免费扩展，不上 Barnes-Hut/分块加载） |
| D5 | 节点几何 | **布告板精灵 sprite**（面向相机软星点，径向衰减） |
| D6 | 辉光 | **真多趟 FBO bloom**（亮度提取 → 分离高斯模糊 → 加色合成） |
| D7 | 布局呈现 | **动态沉降**（力导向实时跑几秒）后进入轻微 idle drift |
| D8 | 交付节奏 | **四阶段**逐步交付，每阶段可验证 |
| D9 | 错误修复/连线 | = 忠实重接所有现有交互到新 3D 视图；不做独立 bug 猎杀审计 |

---

## 2. 现状分析 (Current State)

### 渲染
- `interfaces/webchat/src/canvas_engine/`（~6212 行）：纯 **Canvas2D**（`CanvasRenderingContext2d`）。
- web-sys 仅启用 `CanvasRenderingContext2d`，**未启用任何 WebGL feature**。
- 伪 3D：`renderer.rs::depth_attrs` 用 scale/opacity/blur 模拟 Z 分层，非真 3D。

### 视图模型
- `interfaces/webchat/src/views/canvas/`（~2446 行）：**邻域 radial 导航**（center + one_hop + two_hop + 折叠 cluster）。
- 宿主 `canvas/mod.rs`（768 行 `RadialCanvasView`）编排：agent 选择、entry pick、`graph.query`（全图 limit 500，用于 minimap + orphan ring）、`graph.neighbors`（每个 center depth 3 limit 200）、prefetch 缓存、hover 去抖、in-flight 上限、`NavController` fly-to、详情 excerpt 懒取、搜索、MiniMap、fold 滑块。

### 数据契约（保持不变）
- 服务端 `graph.query` 返回 `{nodes: NoteNodeDto[], edges: NoteLinkDto[]}`，**只有拓扑、无坐标**。
  - `NoteNodeDto`: `id, name, path, category, tags, link_count`
  - `NoteLinkDto`: `from, to, label?, kind?`
- `graph.neighbors` / `graph.node_detail` / `graph.search` / `graph.update_note`：保持不变。
- 3D 坐标由客户端布局算出（D3），**不新增 core RPC、不改 serde 契约**。

---

## 3. 架构 (Architecture)

**原则**：渲染层重写，编排层复用。新增 `views/canvas/gl/` 子模块（每文件 <400 行，高内聚低耦合，P2）。

### 3.1 新增渲染模块 `views/canvas/gl/`

| 文件 | 职责 |
|------|------|
| `context.rs` | 从 `<canvas>` 取 `WebGl2RenderingContext`；封装 GL 资源句柄（program / buffer / VAO / FBO / texture），RAII 释放 |
| `math.rs` | 极简 `Mat4` / `Vec3`（perspective / lookAt / 乘法 / 旋转）。**手写**避免引第三方（工作区无 glam/nalgebra；plan 阶段若手写超 ~150 行可重新评估引 glam） |
| `camera.rs` | 轨道相机：azimuth/elevation/distance + 阻尼 + zoom/pan + 空闲超时自转 + fly-to 缓动 |
| `shaders.rs` | GLSL 源（node sprite / edge line / bright-pass / gaussian blur / composite），`&'static str` 常量 |
| `nodes.rs` | 实例化节点渲染：一次 draw call 画全部节点。Per-instance 属性：`position(vec3)`, `size(f32)`, `color(vec3 HDR)`。billboard quad + 径向 alpha 衰减 |
| `edges.rs` | 批量细线（`GL_LINES`）：per-vertex color/alpha，加色混合，距离淡出 |
| `bloom.rs` | 后处理：scene→FBO → bright-pass → 分离高斯 ping-pong（2~3 级降采样）→ 加色合成回主缓冲 |
| `layout3d.rs` | 3D 力导向：斥力（n²，低千可承受）+ 边弹簧 + 向心力。固定步进沉降 + 收敛判定 |
| `picking.rs` | 屏幕空间投影拾取：节点投影到 NDC，命中最近且在半径内者（CPU，低千足够） |
| `scene.rs` | 每帧编排：推进布局沉降 → 渲染节点+边到 HDR FBO → bloom → 合成 → 拾取叠加。持有全部 GL 状态 |

### 3.2 复用（保留不动）
`canvas_engine/`: `category_color.rs`（类目配色）、`cluster.rs`（聚类着色）、`prefetch.rs`（详情缓存）、`adapter.rs`（数据 DTO 类型）、`fnv1a.rs`（稳定哈希）、`markdown_excerpt.rs`（excerpt 渲染）。
`views/canvas/`: `node_detail_panel.rs`、`node_card.rs`（详情面板，3D hover/select 复用）。

### 3.3 数据流 (Data Flow)

```
agent 切换 / 首次挂载
  → graph.query(agent, limit=ALL)   // 一次拉全图（提高 limit 覆盖全部）
  → 构建 Galaxy { nodes, edges }
  → layout3d 沉降（动态，几秒）→ 每节点 (x,y,z)
  → scene 每帧渲染（rAF，IntersectionObserver 可见性暂停沿用）

用户交互
  → picking 命中节点 → SelectNode/HoverNode
  → camera fly-to + 高亮邻域（用拓扑算邻接，不重取）
  → NodeDetailPanel（node_detail 懒取 + excerpt，复用现有缓存）
```

---

## 4. 渲染管线细节 (Rendering Pipeline)

### 4.1 节点 (Nodes) — D5
- billboard quad（两三角形）实例化 N 次；vertex shader 把 instance position 投影后按 `size` 在屏幕空间张开 quad（始终面向相机）。
- fragment shader：径向距离 → 软 alpha 衰减（高斯/`smoothstep`），产生柔和星点。
- 颜色：`category_color` → **HDR boost >1.0**（越亮，bloom 拾取的辉光越强；复刻参考 `boost = 1.2 + brightness*0.8`）。
- 高亮态：非高亮节点 `color * 0.15` 暗化 + `size` 收缩（复刻参考）。

### 4.2 边 (Edges) — "线条精细"
- `GL_LINES` 批量；per-vertex 颜色取两端节点色插值，alpha 随相机距离淡出。
- 加色混合（`ONE, ONE` 或 `SRC_ALPHA, ONE`）→ 密集处自然增亮如星丝。
- 远处弱边按 LOD 阈值（接管旧 `fold_threshold` 滑块语义）隐藏，保持"几万仍清晰"。

### 4.3 Bloom — D6
- 主场景渲染到 HDR-ish 浮点/半浮点 FBO（`RGBA16F`，回退 `RGBA8` + 手动 tone）。
- bright-pass：`max(color - threshold, 0)`（luminanceThreshold ≈ 0.3，复刻参考）。
- 分离高斯：水平 + 垂直，2~3 级降采样 ping-pong（近似 mipmapBlur）。
- 加色合成：`scene + bloom * intensity`（intensity ≈ 1.2，复刻参考）。

### 4.4 相机 (Camera) — D7
- 透视投影（fov ≈ 50，near 0.1，far 大）。
- 轨道：拖拽改 azimuth/elevation，滚轮改 distance，阻尼平滑（dampingFactor ≈ 0.08）。
- 空闲自转：无交互 > 阈值（参考 60s）后缓慢自转，任何指针/滚轮事件复位。
- fly-to：选中/搜索/反向跳转时缓动相机至目标（ease-out cubic，复刻参考 `CameraAnimator`）。

### 4.5 布局 (Layout) — D3/D7
- 3D 力导向：斥力（库仑式 n²）+ 边弹簧（胡克）+ 向心力（防飘散）。
- 动态沉降：rAF 每帧推进若干步，能量收敛或达上限步数后停，转入轻微 idle drift（复用 `renderer.rs` drift 概念）。
- 万级上限：n² 斥力在低千 OK；若未来上万，`layout3d` 接口设计为可替换为 Barnes-Hut（**本期不实现**，YAGNI）。

### 4.6 拾取 (Picking)
- CPU：每节点世界坐标 → 投影到屏幕 → 与指针比距离，命中半径内最近者。
- 低千节点每帧/每指针事件投影成本可忽略；万级时改 GPU color-picking（**本期不实现**）。

---

## 5. 交互重接映射 (Interaction Rewiring) — D9

| 现有交互 | 来源信号 / 事件 | 3D 新映射 |
|---------|----------------|-----------|
| 选中节点 | `CanvasEvent::SelectNode` | picking 命中 → camera fly-to + 高亮邻域 + 打开 `NodeDetailPanel` |
| hover | `CanvasEvent::HoverNode` | tooltip + 邻域微高亮；dwell 去抖（`hover_intent` Effect）复用 |
| 详情/excerpt 懒取 | `selected ∪ hovered` Effect | **原样复用**（`GraphApi::node_detail` + `render_excerpt` + `detail_cache`） |
| 搜索 | `mem.search_query` / `search_nonce` | `graph.search` → fly-to 首结果 + 高亮 |
| agent 切换 | `mem.agent_id` reset Effect | 重载星云（reset 逻辑复用：清状态 + 重新 `graph.query` + 重新沉降） |
| 列表→图谱反向跳转 | `mem.selected_node` / `mem.highlight_note_id` | fly-to 目标节点 + 高亮 |
| 图谱→列表 | `on_locate` → `mem.memory_view=Graph` 反向 | 节点详情面板"在列表查看"→ 写 `mem.highlight_note_id`（既有反向链路） |
| 笔记编辑 | `GraphApi::update_note` | **原样复用** |
| 折叠滑块 `fold_threshold` | `mem.fold_threshold` | 转为 **LOD / 视觉密度阈值**（远处隐藏标签、淡出弱边） |
| MiniMap | `GlobalMiniMap` overlay | 重做为 3D 概览盒或后期实现；**P1~P3 先移除 2D minimap，P4 评估** |

> `MemoryState` 共享信号（`agent_id`/`selected_node`/`highlight_note_id`/`search_query`/`search_nonce`/`fold_threshold`/`memory_view`）契约不变，Memory Hub 切换 Graph/Table 不变。

---

## 6. 退役清单 (Retirement) — P6 删优于注释

重接验证通过后删除 Canvas2D 专用件（先确认无消费者）：
`renderer.rs`（587）、`edge_curve.rs`、`drag.rs`、`tween.rs`、`viewport.rs`(2D)、`mini_map.rs`(2D，待 P4 决定)、`scatter.rs`、`align_guides.rs`、`interaction.rs`(2D 命中)、`navigation.rs`(radial 导航)。

> `renderer.rs` 的 idle drift 算法在删除前移植进 `gl/layout3d.rs` 或 `gl/scene.rs`。`navigation.rs` 的 breadcrumb/历史若有价值移植为相机历史栈（否则删）。每个删除在 PR 描述说明无消费者。

web-sys 需新增 features：`WebGl2RenderingContext`, `WebGlProgram`, `WebGlShader`, `WebGlBuffer`, `WebGlVertexArrayObject`, `WebGlFramebuffer`, `WebGlTexture`, `WebGlUniformLocation`, `WebGlRenderingContext`(常量)。

---

## 7. 分阶段实施 (Phasing) — D8

| 阶段 | 内容 | 验证标准 |
|------|------|----------|
| **P1 渲染地基** | `context`/`math`/`camera`/`shaders`/`nodes`/`edges`/`scene` 骨架；轨道相机；mock/随机布局渲染全图 | `just wasm` 构建通过；浏览器：能 3D 旋转、能看到全部节点与边 |
| **P2 布局 + 星云** | `layout3d` 动态沉降；`bloom` FBO 管线；HDR 配色；idle drift | 浏览器截图：星云辉光观感；低千节点结构清晰；沉降到位 |
| **P3 交互重接** | `picking`；选中/hover/详情面板/搜索/agent 切换/反向跳转全通；fold→LOD | 逐项手验：功能零丢失（对照第 5 节表） |
| **P4 打磨 + 退役** | fly-to 缓动、空闲自转打磨、LOD 标签、minimap 决策；删死 Canvas2D 模块 | `just wasm` + `cargo check`（按节制策略至多一次）；截图终验；死代码清零 |

每阶段在 worktree 内独立 commit。纯函数（layout/math/camera/picking/bloom 核）随阶段写单测。

---

## 8. 测试策略 (Testing)

- **单元测试（native target，无需 WebGL）**：
  - `math`: mat4 乘法 / perspective / lookAt 已知值。
  - `camera`: 轨道角度→位置、阻尼收敛、fly-to 缓动单调。
  - `layout3d`: 能量随步数下降、收敛判定、确定性（同输入同输出，phase 由 id 哈希）。
  - `picking`: 已知投影下命中正确节点、半径外不命中。
  - `bloom`: 高斯核权重和≈1、bright-pass 阈值边界。
- **GL 渲染**：WASM 无法单测 GL → `just wasm` 构建 + 浏览器截图人验（沿用项目惯例）。
- **回归**：保留并适配编排层既有测试（adapter / prefetch）。

---

## 9. 红线合规 (Redline Compliance)

- **R1/R3 core 不动**：全部改动在 `interfaces/webchat`，core 零改动，无平台 API。
- **R2 UI 唯一源**：复杂 UI 仍在 Leptos Panel。
- **R4 Panel 纯 I/O**：3D 布局是**呈现计算**非业务逻辑（持久化/检索/规划），与现有客户端布局同性质。
- **全栈 Rust / 无 JS**：D2 纯 Rust WebGL2，零 JS 注入。
- **serde 契约不变**：不新增/不改 RPC，数据 DTO 复用。
- **技术栈禁用清单**：不引第二 async runtime、不引向量库、`src` 不依赖平台 crate、不用正则做意图识别 —— 均不触碰。
- **P2 高内聚 / 大文件拆分**：`gl/` 每文件 <400 行。

---

## 10. 风险与缓解 (Risks)

| 风险 | 缓解 |
|------|------|
| `RGBA16F` 浮点 FBO 兼容性 | 检测扩展，回退 `RGBA8` + 手动 tone-map |
| WASM 体积增大（GL 代码 + shaders） | 纯 Rust 无重型 crate；shaders 是字符串常量，增量可控 |
| 手写 mat4 数学出错 | `math` 单测覆盖已知值；必要时再评估引 glam |
| 布局沉降卡顿（首帧） | rAF 增量沉降，不阻塞；可见性暂停沿用 IntersectionObserver |
| picking 在密集区误命中 | 最近 + 半径阈值；P4 可加深度优先 |
| `graph.query` limit 拉全图过大 | 现实低千足够；limit 提至覆盖全部，万级再议分页（YAGNI） |

---

## 11. 非目标 (Non-Goals / YAGNI)

- 不实现 Barnes-Hut、GPU picking、分块/渐进加载（D4：架构留口，本期不做）。
- 不实现参考项目的"卫星星系/跨项目 linked_projects"（Aleph 记忆图谱单 agent，无跨项目概念）。
- 不改 core、不新增 RPC、不改 serde 契约。
- 不做独立 bug 猎杀审计（D9：仅忠实重接）。
- 不碰 main 分支（全程 worktree）。

---

## 12. 验收 (Definition of Done)

1. 四阶段全部通过各自验证标准。
2. 第 5 节交互映射逐项手验：现有功能零丢失。
3. 浏览器截图：3D 旋转 + 星云辉光 + 低千节点清晰。
4. 死代码清零；`just wasm` 构建通过；`cargo check` 一次绿（节制策略）。
5. 红线（第 9 节）全部合规。
