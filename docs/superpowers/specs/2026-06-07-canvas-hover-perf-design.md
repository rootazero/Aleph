# Memory Canvas — Hover 稳定 / 性能空转 / 死代码清理

Date: 2026-06-07
Branch: `feat/canvas-hover-perf` (worktree `/Volumes/TBU4/Workspace/Aleph-wt-canvas`)
Status: Design approved, ready for implementation plan.

## 背景

记忆管理知识图谱画布位于 `interfaces/webchat/src/views/canvas/`（`RadialCanvasView` →
`GraphCanvas`）与 `interfaces/webchat/src/canvas_engine/`。当前存在三类问题：

1. **Hover 闪烁**：hover 命中区 = 节点圆 `radius + 固定 px 容差`（`viewport.rs::hit_test`）。
   hover 命中后卡片放大为 Full（280px，`node-card-full`），但卡片继承外层
   `pointer-events:none`。鼠标移到卡片正文上即离开节点命中圆 → 卡片瞬间塌缩；命中区
   边界处亚像素移动反复触发 None/Some → **闪烁**。模式切换（Dot→Mini→Full）是整段
   DOM 元素替换（`match mode.get() … .into_any()`），无过渡 → 弹出式 jank。
2. **隐藏页不空转 + 每帧 layout 读取**：rAF 循环故意 leaked（无 `on_cleanup`，因
   `MainContent` 只 `display:none` 不卸载）。隐藏时仍每帧 reschedule rAF 只为读
   `is_visible`（~60Hz 永不真正停）。且每帧 `parent.get_bounding_client_rect()` 强制
   同步 layout 以检测尺寸变化。
3. **死代码**：`NodeCard` 的 `on_click` / `on_card_click` 因卡片 `pointer-events:none`
   **永不触发**，且只 set 本地 `selected_id_sig` 不导航；点击实际由 canvas hit-test →
   `SelectNode` 处理。

参考项目（`/Volumes/TBU4/Github/`）：`tldraw`（hover hysteresis、元素捕获）、
`infinite-canvas-tutorial`（ResizeObserver + 矩阵视口）、`jsoncanvas` / `react-jsoncanvas`
（JSON Canvas 格式——本轮不纳入）。

## 范围

纳入：① Hover 稳定可读 ② 隐藏即空转 + ResizeObserver ③ 死代码清理。
**不纳入**：JSON Canvas 互操作（`canvas_engine/json_canvas/` 已建未连线，留后续独立任务）；
`TODO(memory-events)` 缓存失效连线；后端 / JSON-RPC / 记忆库写入路径一律不动（R4 接口
向后兼容）。

## 模块一：Hover 稳定可读（canvas 单权威滞回）

**决策：路线 B —— canvas 单权威 + 两级滞回**（弃路线 A 卡片 DOM 捕获，避免 canvas
hit-test 与卡片 DOM 事件双权威竞争）。

- `viewport.rs` 新增 hover 保持判定：已 hover 的节点用**退出区域**（rectangle）判定，其余
  节点沿用现有 `radius + tol` **进入**判定。进入半径 < 退出区域 → 滞回，消除边界抖动。
  - 退出区域 = 该节点当前 `CardMode` 的屏幕半尺寸（screen-space，不随 zoom 缩放，因为
    卡片是固定像素 DOM）：
    - Full：半宽 ≈ 150px，半高 ≈ 90px（覆盖 280px 卡片 + excerpt 主体）
    - Mini：半宽 ≈ 75px，半高 ≈ 24px
    - Dot：退化为 `radius + tol`（无放大卡片）
  - 退出区域以节点屏幕中心为基准做轴对齐矩形包含判定（与卡片 `translate3d` 偏移近似
    对齐；不要求像素精确，只需"大于进入半径且覆盖卡片"以消抖）。
- `on_pointermove` 的 hover 分支：先用退出区域测试当前 `hovered_node` 是否仍保持；保持则
  不变；否则用进入判定（`hit_test`）求新 hovered。仅在 hovered 真正变化时 emit
  `HoverNode`（维持现有 edge-triggered 语义）。
- **动画期冻结 hover**：`NavState::Animating` 期间 hit-test 用 `state.nodes`（目标位）
  与插值绘制有偏移；该期间冻结上一次 hovered，消除动画期 hover 漂移。
- **平滑过渡**：`NodeCard` 外层包固定 wrapper（持有 `screen_xy` 定位），内层按 mode 切换
  内容；mode 切换加 `opacity` + 轻微 `transform` scale 的 120ms 交叉淡入（CSS，
  `tailwind.css` 内 `.node-card-*` 增补 `transition`），消除瞬间弹出。

测试（纯 Rust 单测，随提交，不在本会话跑 cargo check）：
- 进入：圆内命中 → Some。
- 保持：已 hover，光标在退出矩形内但出圆 → 仍保持该节点。
- 退出：已 hover，光标移出退出矩形 → 取消。
- 缩放：退出矩形为 screen-space，zoom 变化不改变其像素尺寸。

## 模块二：隐藏即空转 + ResizeObserver

- **rAF 真正空转**：`graph_canvas.rs` rAF 闭包中 `!is_visible` 分支**不再 reschedule**，
  改置 `parked: Rc<Cell<bool>> = true` 后直接返回。IntersectionObserver 回调在
  `is_intersecting == true && parked == true` 时清 `parked` 并重新 `request_animation_frame`
  kick 一帧。加守卫防重复调度（kick 前确认 `parked`）。效果：隐藏 Memory 页 → 该 rAF
  链 CPU 归零，可见即恢复。
- **ResizeObserver 取代每帧 layout 读取**：对 canvas `parent_element` 挂 ResizeObserver，
  回调把最新 `(w, h)` 写入 `Rc<Cell<(f64, f64)>>`（含 dirty 标记）。rAF 每帧只读 Cell
  （廉价），仅当尺寸 dirty 时执行 canvas resize + viewport 更新 + `fit_to_content`。
  删除每帧 `parent.get_bounding_client_rect()` 同步 layout 读取。Observer 回调与 rAF 单线程
  （wasm32），Cell 无竞争。
- 每帧 `edges_snapshot` / `node_world` clone 为次要项（受邻域规模约束，量小）：标注 TODO，
  本轮不强改（避免过度工程 / R6 KISS）。

## 模块三：死代码清理

- 移除 `NodeCard` 的 `on_click: Callback<String>` prop 及 `graph_canvas.rs` 中构造的
  `on_card_click`（永不触发的死代码）。三种 mode 的 `on:click=move |_| on_click.run(...)`
  一并移除。点击导航完全由 canvas hit-test → `CanvasEvent::SelectNode` 承担（不变）。
- 与 `pointer-events:none` 语义对齐：卡片本就非交互层，不再保留虚假点击 handler。
- 保留 `selected_id_sig`：它驱动 `data-selected` 光环（仍有消费者），仅移除点击写入它的
  死路径。实施时确认 `selected_id_sig` 的其他写入点（canvas `SelectNode` 经
  `selected_id_sig.set` 在 rAF publish）仍完整。

## 安全 / 约束

- 分支隔离：全部改动在 worktree `feat/canvas-hover-perf`，不碰 main。
- 向后兼容：不改公共 API / JSON-RPC / 后端；`NodeCard` prop 变更为内部组件，调用方仅
  `graph_canvas.rs` 一处，同步更新。
- 熵减：删除死 handler，不留注释式保留。
- 完成后**不跑 cargo check / 测试**（用户强制约束），直接提交。

## 受影响文件

| 文件 | 改动 |
|------|------|
| `canvas_engine/viewport.rs` | 新增 hover 滞回保持判定 + 单测 |
| `views/canvas/graph_canvas.rs` | rAF 空转、ResizeObserver、hover 调用滞回、动画期冻结、移除 on_card_click |
| `views/canvas/node_card.rs` | 外层 wrapper + mode 交叉淡入、移除 on_click prop |
| `styles/tailwind.css` | `.node-card-*` mode 切换 transition |
