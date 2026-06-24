# 记忆 Canvas 星系图谱 — 视觉/性能/交互打磨设计

**Date**: 2026-06-24
**Status**: Approved (design) → pending implementation plan
**Scope owner**: Panel (Leptos/WASM) `interfaces/webchat/src/views/canvas/`
**Topic**: Memory canvas 3D galaxy — crisp stars, organic edges, highlight wiring, idle perf

---

## 1. 背景与现状 (Background)

记忆管理 canvas 是一个 3D WebGL 星系渲染器，位于 `interfaces/webchat/src/views/canvas/gl/`。
每帧管线：`rAF → 力导引 settling / 闲置漂移 → 场景 Pass(RGBA16F FBO + 加法混合) → Bloom(bright-pass → H/V 高斯模糊 → composite)`。

模块划分已较成熟（近期提交多为退役死代码）。本轮**不动数据流 / RPC / 力导引算法 / LOD 语义 / 交互编排骨架**，只做视觉、性能、交互连线三层打磨。

### 1.1 用户痛点 → 根因定位

| 痛点 | 根因 (file anchor) |
|------|------|
| 星星"模糊状"，不是清晰亮点 | `gl/shaders.rs` `NODE_FRAG`：`a = smoothstep(1.0,0.0,r)` 是软径向渐变球，再叠全屏 bloom → 糊成云 |
| 连线"死板 + 和星星脱离感" | `gl/shaders.rs` `EDGE_VERT/FRAG` + `gl/edges.rs`：等宽 3px 笔直屏幕空间 ribbon，从节点几何中心连到中心，线端戳进软光晕，无收束 |
| 性能 / 闲置烧 GPU | `gl/scene.rs:265-286`：闲置漂移**每帧** CPU 重算所有节点 sine 偏移并重传 position+color+size 三个 buffer（color/size 漂移时未变，纯浪费）；`galaxy_canvas.rs` rAF 永不暂停（`display:none` keep-alive 隐藏时仍渲染） |
| 高亮链路未连（交互脱节） | `mod.rs::compute_highlight_set` 只返回**节点索引**，`scene.rs::set_highlight` 只调暗非高亮**节点**；**边完全不参与**——既不点亮邻居链路也不调暗无关边，聚焦感半截 |

### 1.2 已排除的伪问题

- `detail_panel_excerpts`：`node_detail_panel.rs:44-74` 面板自带 fetch Effect 填充，**非脱节**，不动。
- `breadcrumb` 恒为空、`recent_visited` 可能无人填充：均优雅降级，属历史 prefetch 子系统残留，**本轮不修**（YAGNI，超范围）。

---

## 2. 目标与范围 (Goals & Scope)

### 2.1 In scope

1. **星点**：清晰实心核 + 柔光晕（视觉选择 C）+ 极弱 hub 星芒（视觉选择 B，受严格约束）。
2. **连线**：微弧贝塞尔 + 端点融入（视觉选择 A）+ 选中时邻居链路能量流光（视觉选择 C）。
3. **高亮链路连通**：选中节点 → 邻居边点亮流光 + 非邻居边调暗（修交互脱节 + 喂能量流）。
4. **性能**：漂移搬进顶点着色器（上传一次，每帧只更 u_time）；color/size 仅高亮变化时上传；rAF 可见性门控。
5. **可选打磨**（默认含，可砍）：极淡背景星尘 + 节点微闪（着色器内 u_time，零开销）。

### 2.2 Out of scope (YAGNI 边界)

- 不动数据流 / `GraphApi` RPC / `ForceLayout` 算法 / LOD 语义。
- 不加新依赖（纯 GLSL + Rust，遵守 R3 核心轻量化 / 技术栈禁用清单）。
- 不碰 core（本轮纯 Panel WASM，遵守 R2 UI 唯一源）。
- 不重构交互编排骨架，只**增量**补"高亮边"一条通道。
- 不修 breadcrumb / recent_visited / 其他历史残留。

### 2.3 视觉决策 (已通过视觉伴侣确认)

- **星点 = C 亮核柔晕作底 + B 星芒只给 hub**。
- **星芒硬约束（用户强调）**：必须弱，不能干扰连线，聚集时不能乱 → 仅极少数最高度数 hub、长度短、`alpha ≤ 0.3`、**相机拉近/密集时自动淡出**。
- **连线 = A 微弧 + 端点融入作默认 + C 能量流只在选中/高亮邻居链路点亮**。

---

## 3. 详细设计 (Detailed Design)

### 3.1 星点渲染

**`gl/shaders.rs` `NODE_FRAG` 重写**
- 清晰实心核：`core = smoothstep(0.18, 0.0, r)`（紧致硬核，替代原 0.6 软核）。
- 外层柔光晕：低强度宽径向分量，交给 bloom 自然发光，不是星核本体糊。
- 形如 `frag = vec4(coreColor*coreBright + haloColor*halo, coreAlpha + haloAlpha)`，核 alpha 锐利、晕 alpha 平缓。

**`gl/nodes.rs` + `NODE_VERT`：弱 hub 星芒**
- 新增每节点属性 `a_spike`（star-cross 强度，0 表示普通节点）。
- 仅 `link_count` 进图内前 ~5% 的节点 `a_spike > 0`；强度随度数缩放但封顶。
- 星芒在 fragment（或额外薄 quad）内绘制十字光，`alpha ≤ 0.3`。
- **近景淡出**：`a_spike` 有效强度 ×= `clamp(camera_distance / threshold, 0, 1)` 类衰减 → 相机越近越弱，密集时归零。具体衰减曲线实现期调。

**`gl/bloom.rs` / `run()` 参数回调**
- `u_threshold` 0.3 → ~0.5，`u_intensity` 1.2 → ~0.9：让清晰核穿过 composite 存活，光晕不过曝。
- composite 已经是 `scene + bloom`，清晰场景在下层，核必然存活——只需调晕量。

### 3.2 连线渲染

**`gl/edges.rs` + `EDGE_VERT` 微弧**
- 静态 corner buffer 由 6 顶点（单段）改为 **K 段三角条带**（`2*(K+1)` 顶点，`along ∈ [0,1]` 均分，`side ∈ {-1,+1}`）。仍 **1 instance/边、1 draw call**。
- 顶点着色器按 `along` 求二次贝塞尔点：`P(t) = (1-t)²·A + 2(1-t)t·C + t²·B`。
- 控制点 `C = midpoint + perp · (len · sag_factor)`，`perp = normalize(cross(B-A, world_up))`（3D 旋转下一致；`world_up` 平行时退化用备用轴）。
- 端点收束：宽度/亮度沿 `along` 在两端略收（`along→0/1` 时 ×<1），叠在节点柔光晕下 → "从星核长出来"。

**`EDGE_FRAG`**
- 默认安静：细、半透、轻 AA rim（保留现有 crisp-core + depth-fade 思路）。

### 3.3 高亮链路连通（交互脱节修复 + 能量流）

**数据通道**
- `mod.rs::compute_highlight_set` → 扩展/新增伴生函数，除节点高亮集外同时算出**高亮边集**（选中节点的邻接边索引）。
- 新 intent 通道 `highlight_edges_request: RwSignal<Option<HashSet<usize>>>`（边在 `filtered_edges` 中的下标，或边 `(u32,u32)` 集合——实现期选更稳的键）。
- `galaxy_canvas.rs` 加对应 prop + Effect，转交 `Scene`。

**`gl/edges.rs`：每边 highlight 属性**
- 新增 per-instance `a_highlight`（0/1）。
- `Scene` 收到高亮边集 → 重算 `a_highlight` 并上传（**仅选中/取消时**，不每帧）。

**`EDGE_VERT/FRAG`：能量流（C）**
- `a_highlight=1` 的边：fragment 用 `u_time` 沿 `along` 推一束流光（`fract(along*freq - u_time*speed)` 形成移动亮带），叠在安静底色上。
- 非高亮边：维持安静；选中态下**非邻居边调暗**（×<1 亮度），与节点调暗一致 → 聚焦感补全。
- 取消选中：`a_highlight` 全 0，亮度复位。

**触发策略**：默认仅"选中节点 / 搜索命中 / 列表定位"触发（沿用现有三处 `compute_highlight_set` 调用点）。hover 是否触发暂不做（避免每次划过都流光，太闹）——留作后续，若用户要再加。

### 3.4 性能重构

**`gl/nodes.rs` + `NODE_VERT`：GPU 漂移**
- 上传一次：`a_offset`(基准settled位置) + 新 `a_phase`(每节点相位，由 id hash 求，替代 CPU `drift_offset_3d`)。
- 新 uniform `u_time`；顶点着色器内 `pos = base + amplitude * vec3(sin(u_time*ω + phase), sin(...+φ1), sin(...+φ2))`。
- 闲置每帧只 `uniform1f(u_time)` + draw + bloom，**零 buffer 重传、零 CPU sine**。

**`gl/scene.rs`：上传门控**
- color/size 仅在 highlight 变化时 `upload`（不再每帧）。
- settling 期间（位置真变，≤400 步有界）仍每帧上传 base 位置；结束后转纯 GPU 漂移。
- 移除 `drift_scratch` 每帧 CPU 路径（被 GPU 漂移取代）。

**`galaxy_canvas.rs`：rAF 可见性门控**
- render 前判断 canvas 是否可见（`offset_parent().is_none()` 或 IntersectionObserver）；隐藏（`display:none` keep-alive）/不在视口 → 跳过 `s.render(t)`，仍 `request_af` 保活，待可见即恢复。

**picking 不变**：`pick` / `screen_pos_of` 仍用 canonical `node.pos`（与今日 CPU 漂移期行为一致，漂移幅度小 ≪ 18px 容差，无回归）。

### 3.5 可选打磨（默认含，可砍）

- **背景星尘**：远景静态暗点层（单 draw call instanced 暗点，或场景 clear 前一层），增星系纵深。
- **节点微闪**：核亮度 ×= `0.9 + 0.1*sin(u_time*ω + phase)`，着色器内零开销。
- 二者均可在 spec review 阶段砍掉，不影响主线。

---

## 4. 测试与验证 (Testing & Verification)

- **纯逻辑单测**（原生 target，沿用 `bloom.rs` gaussian / `cheap_passes` 模式）：
  - 二次贝塞尔分段顶点生成 / `along` 均分正确性。
  - 每节点相位 hash 稳定性。
  - 高亮边集计算（给定选中节点 → 正确邻接边集）。
- **GL 部分**：`just wasm` 编译门 + 浏览器实测（chrome-devtools MCP）。
  - 视觉验收：清晰核、弱星芒近景淡出、微弧端点融入、选中流光 + 非邻居调暗。
  - 性能验收：performance trace 看闲置帧时长（目标闲置 60fps、隐藏时零渲染）。
- **cargo 节制**（用户 working style）：最多一次 `cargo check`（逻辑）+ 一次 `just wasm`（GL 编译）。不跑全量。

---

## 5. 架构红线核对 (Redline Check)

- **R2 UI 唯一源**：纯 Panel WASM，无原生 Bridge 业务逻辑。✅
- **R3 核心轻量化 / 技术栈禁用清单**：零新依赖，纯 GLSL + 现有 Rust。✅
- **不碰 core / 数据流**：仅渲染层 + 一条高亮边 intent 通道。✅
- **P6 简洁性 / YAGNI**：删 `drift_scratch` CPU 路径；可选打磨可砍；不留假想未来口子。✅

---

## 6. 影响文件清单 (Touched Files)

| 文件 | 改动 |
|------|------|
| `gl/shaders.rs` | NODE_FRAG 重写、NODE_VERT 加漂移/星芒、EDGE_VERT 微弧、EDGE_FRAG 流光 |
| `gl/nodes.rs` | `a_phase`/`a_spike` 属性、`u_time`、上传门控、GPU 漂移 |
| `gl/edges.rs` | K 段条带 corner buffer、`a_highlight` 属性、`u_time` |
| `gl/bloom.rs` | threshold / intensity 参数回调 |
| `gl/scene.rs` | 上传门控、漂移路径切 GPU、高亮边集下发、删 drift_scratch |
| `views/canvas/galaxy_canvas.rs` | rAF 可见性门控、`highlight_edges` prop + Effect |
| `views/canvas/mod.rs` | `compute_highlight_set` 伴生高亮边集、新 intent 通道接线 |

---

## 7. Open Questions

均已在 brainstorm 中解决：
- 性能力度 → 方案② GPU 漂移（已确认）。
- 星芒 → 弱、hub-only、近景淡出（用户强调，已确认）。
- 能量流触发 → 仅选中/搜索/定位，hover 暂不触发（已确认）。
- 可选打磨 → 默认含，spec review 可砍（待用户复核时定）。
