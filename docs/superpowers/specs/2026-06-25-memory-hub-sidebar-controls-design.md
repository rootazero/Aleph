# 记忆 Hub 控件归位 + Fold 滑块修复 — 设计文档

- **日期**: 2026-06-25
- **范围**: Aleph Panel（`interfaces/webchat/`，crate `aleph-panel`）记忆管理页 UI 布局重组
- **状态**: 已与用户确认设计，待写实现计划
- **关联红线/原则**: R2（单一 UI 真源）、R4（Interface 纯 I/O）、P2（高内聚）、P6（简洁）

> 本文档的每一条技术断言均已由只读验证 workflow（6 个并行 agent）对照真实代码核实，证据行号见正文。

---

## 1. 问题陈述 (Problem)

记忆管理页（`/memory`，组件 `MemoryHub`）当前存在两处用户报告的问题：

1. **功能重复**：App 外壳左侧栏（`components/mode_sidebar.rs::MemorySidebar`）渲染的节点详情面板，与 Canvas 视图右下角浮现的方框，**是同一个 `NodeDetailPanel` 组件的两个实例**——「在列表中查看」「编辑」「最近访问」完全一致，纯重复占据左侧栏空间。
2. **Fold 滑块无效**：左侧栏底部的 Fold 滑块拖动后肉眼无任何变化。

附带的布局诉求：把当前散落在 Canvas 顶部工具栏（`views/memory_hub/toolbar.rs::MemoryToolbar`）里的 **agent 选择器、搜索框、图谱/列表切换** 全部收进左侧栏，把右侧区域整块归还给 Canvas 视图。

### 1.1 根因（已定位，非推测）

- **重复根因**：左侧栏 `MemorySidebar`（`mode_sidebar.rs:178`）与 Canvas overlay（`canvas/mod.rs:331-336`）渲染**同一个** `NodeDetailPanel` 组件，故功能字节级一致。
- **Fold 无效根因**：滑块输入范围 `min=0 max=10`（`mode_sidebar.rs:168`），但 LOD 映射（`canvas/mod.rs:302-316`）按 `ft∈[1,1000]` 归一：`lod = 1.0 - (ft-1)/999`。于是 0–10 的滑块只能产出 `lod∈[0.991, 1.0]`——永远卡在"最稀疏 / 仅主干"那 0.9% 的角落，所以看不出疏密变化。`lod` 本身是有效的：`scene.rs:105-149` 的 `set_lod()/recompute_filtered_edges()` 按 `link_count` 百分位剔边，`lod=0` 全部连线、`lod=1` 仅约 90 百分位主干。

---

## 2. 目标与非目标 (Goals / Non-Goals)

### 目标
- G1：删除左侧栏的 `NodeDetailPanel` 实例（消除重复）；Canvas 右下角 overlay 成为唯一节点详情/编辑入口（落实 R2 单一 UI 真源）。
- G2：把 agent 选择器、图谱/列表切换、搜索框从顶部工具栏迁入左侧栏；**新左侧栏从上到下：agent 选择器 → 图谱 → 列表 → 搜索 → Fold 滑块**。
- G3：删除顶部工具栏 `MemoryToolbar`，右侧区域（Canvas / 列表表格）独占全高。
- G4：修复 Fold 滑块，使其全程拖动产生肉眼可见的连线疏密变化；统一其默认值与 agent 切换重置值到单一常量。

### 非目标
- 不改 Canvas 的 WebGL 渲染、节点卡片（`node_card.rs`）、节点详情面板内部逻辑（`node_detail_panel.rs`）。
- 不改记忆列表表格（`views/memory/mod.rs`）的内容、分页、facet。
- 不改 `MemoryState` 的信号语义（`search_query/search_nonce/memory_view/agent_id/fold_threshold` 接线全部沿用）。
- 不重写已死的 cluster-folding 路径（`adapter.rs::to_neighborhood`）——它在 3D 路径中已无运行时调用者，本次不碰。
- 不为 "Fold" 标签引入 i18n（沿用现有字面量）。
- **部署不在本设计范围**：rust_embed 编译期嵌入需重编 `aleph-server`，部署是用户单独拍板的一步。

---

## 3. 当前架构（已核实）

```
App 外壳
├── ModeSidebar (components/mode_sidebar.rs) — w-64 左列，全 App 常驻
│   └── [Memory 模式] MemorySidebar
│        ├── Fold 滑块                         (mode_sidebar.rs:163-177)
│        └── NodeDetailPanel(excerpts)  ←删     (mode_sidebar.rs:178)
│
└── 路由内容区
    └── MemoryHub (views/memory_hub/mod.rs)
        ├── MemoryToolbar  ←删整体              (memory_hub/mod.rs:36, toolbar.rs)
        │    ├── 图谱/列表 二段式切换            (toolbar.rs:23-44)
        │    ├── 搜索框                          (toolbar.rs:48-57)
        │    └── agent 选择器 popover            (toolbar.rs:62-136)
        └── flex-1 内容区
            ├── CanvasView (display:block when Graph)
            │   └── RadialCanvasView
            │        ├── GalaxyCanvas (3D WebGL)
            │        └── NodeDetailPanel overlay  ←保留 (canvas/mod.rs:331-336，右下角方框)
            └── Memory 列表表格 (display:block when Table)
```

消费者核实结论：
- `MemoryToolbar` 仅被 `memory_hub/mod.rs:12-13,36` 引用（其余命中均为注释）→ 删除安全。
- `NodeDetailPanel`/`NodeExcerpt` 恰两个渲染点：`mode_sidebar.rs:178`（删）与 `canvas/mod.rs:336`（留）；`canvas/mod.rs:6` 的 `pub use` re-export 保留。
- Fold 的 `to_neighborhood` cluster-folding 仅在 `#[test]` 中调用（`adapter.rs:396,408,457,495`、`layout.rs:417`），3D 活路径 `build_galaxy → set_graph → set_lod` 不经过它 → 改 Fold 语义安全。

---

## 4. 设计 (Design)

### 4.1 新左侧栏（Memory 模式）

新建组件 **`views/memory_hub/sidebar.rs::MemorySidebar`**，竖排控件，从上到下：

```
┌──────────────────┐
│ ▿ main           │  agent 选择器（popover 下拉，沿用 toolbar.rs:62-136 markup）
├──────────────────┤
│ ◍ 图谱            │  nav-tile，memory_view==Graph 时 .nav-tile-active 高亮
│ ☰ 列表            │  nav-tile，memory_view==Table 时 .nav-tile-active 高亮
├──────────────────┤
│ 🔍 搜索...         │  搜索框（沿用 search_query 实时写 + Enter bump search_nonce）
├──────────────────┤
│ Fold ▭▬▬▬▬       │  滑块（保留，修映射）
└──────────────────┘
```

- **图谱/列表**：两个独立竖排 `nav-tile`（不是横向二段开关），与 `SettingsSidebar`/`NavMenu` 风格一致。
  - class（来自 `mode_sidebar.rs:267-269` 实证）：
    - 激活：`"nav-tile-active flex items-center gap-3 px-3 py-2 rounded-lg text-sm"`
    - 非激活：`"nav-tile flex items-center gap-3 px-3 py-2 rounded-lg text-sm"`
    - 图标 SVG：激活 `"text-sidebar-accent flex-shrink-0"` / 非激活 `"text-text-tertiary flex-shrink-0"`
  - 点击行为：`mem.memory_view.set(MemoryView::Graph)` / `set(MemoryView::Table)`（`state/memory.rs:14-17,48`）。
  - 文案：图谱用 `memory.hub_view_graph`，列表用 `memory.hub_view_table`（en/zh 均已存在）。图标各取一个语义 SVG（图谱=节点连线图标，列表=横线列表图标）。
- **agent 选择器**：把 `toolbar.rs:62-136` 的自包含 popover 整体搬入，仅依赖 `MemoryState` context，无额外依赖。位于栏顶 → popover 仍向下展开（`absolute top-full left-0 right-0 mt-2`，方向不变）。
- **搜索框**：沿用工具栏搜索语义——`prop:value=mem.search_query`、`on:input` 实时写、`on:keydown` Enter `mem.search_nonce += 1`。占位符 `memory.search_placeholder`。视觉可采用 `SettingsSidebar` 的搜索输入模板（`mode_sidebar.rs:198-216`，带搜索图标）以与其它侧栏统一。
- **Fold 滑块**：从旧 `MemorySidebar` 搬入（标签字面量 `"Fold"` 不变），见 §4.3 修复。

### 4.2 右侧区域归还 Canvas

`views/memory_hub/mod.rs`：
- 删 `mod toolbar; use toolbar::MemoryToolbar;` 与 `<MemoryToolbar />`。
- 外层 `flex flex-col` 退化为单一内容容器（Canvas / 列表表格各 `absolute inset-0`，由 `memory_view` 经 `display` 切换，keep-alive 不变）。
- Canvas（`canvas/mod.rs:319` 根 `relative w-full h-full bg-[#080818]`）满幅占据全高。

### 4.3 Fold 滑块修复

保持滑块 UI 范围 `0–10`，把 LOD 映射改为让全程产生完整 `lod∈[0,1]`：

- 方向：沿用现注释/用户确认的"右=更密"——`lod = 1.0 - ft/10.0`（`ft=0 → lod=1.0` 仅主干；`ft=10 → lod=0.0` 全部连线）。clamp 到 `[0,1]`。
- 删除现有 `clamp(1,1000)` 与 `(ft-1)/999` 的旧映射（`canvas/mod.rs:312-316`）。
- **统一默认/重置常量**：现状 `state/memory.rs:78` 默认 `3`、`canvas/mod.rs:147` agent 切换重置 `12`（超出 0–10），二者不一致。引入单一常量 `DEFAULT_FOLD`（建议落在 0–10 中段，给出可读不过密的初始视图；具体值由实现计划敲定），两处都用它。
- 安全性：cluster-folding 死路径不受影响（§3 已核实）；唯一运行时 `lod` 消费者是 `scene.rs::set_lod()/recompute_filtered_edges()`。

### 4.4 代码组织（已确认方案）

- **新建** `views/memory_hub/sidebar.rs`，导出 `MemorySidebar`（承载 agent + 图谱/列表 tile + 搜索 + Fold）。理由：memory-hub UI 物理聚合（P2），`mode_sidebar.rs` 保持薄。
- `views/memory_hub/mod.rs`：`mod sidebar; pub use sidebar::MemorySidebar;`；删 `toolbar` 模块声明与引用。
- `components/mode_sidebar.rs`：Memory 分支改为渲染 `crate::views::memory_hub::MemorySidebar`；**删除**本文件内私有 `fn MemorySidebar`（连同 Fold 滑块 + `NodeDetailPanel`）及随之失效的 import（见 §5）。
- **删除** `views/memory_hub/toolbar.rs`（内容并入 sidebar）。

---

## 5. 逐文件改动清单 (File-by-file)

| 文件 | 改动 | 关键点（已核实） |
|------|------|------|
| `views/memory_hub/sidebar.rs` | **新建** | agent popover（搬自 toolbar.rs:62-136）+ 图谱/列表 nav-tile + 搜索 + Fold 滑块，竖排 |
| `views/memory_hub/toolbar.rs` | **删除** | 仅被 mod.rs 引用，删除零波及 |
| `views/memory_hub/mod.rs` | 改 | 去 `mod toolbar`/`use toolbar::MemoryToolbar`/`<MemoryToolbar/>`；加 `mod sidebar; pub use sidebar::MemorySidebar;`；容器简化 |
| `components/mode_sidebar.rs` | 改 | Memory 分支委托 `memory_hub::MemorySidebar`；删本地 `fn MemorySidebar`；**清理孤儿 import**：行155 `use crate::views::canvas::{NodeDetailPanel, NodeExcerpt};`（删）、行156 `use std::collections::HashMap;`（删）、行154 `use crate::state::memory::MemoryState;`（删，因顶层行16 已导入，属冗余 scoped 重导入） |
| `views/canvas/mod.rs` | 改 | 修 LOD 映射（§4.3）；agent 切换重置（行147）`set(12)` → `set(DEFAULT_FOLD)`；`canvas/mod.rs:6` 的 `pub use ...NodeDetailPanel, NodeExcerpt` **保留** |
| `state/memory.rs` | 改 | 引入 `DEFAULT_FOLD` 常量；`new()` 默认（行78）`3` → `DEFAULT_FOLD` |

> 不新增 i18n 键：复用 `memory.hub_view_graph/hub_view_table/search_placeholder`（en.json:310/311/271、zh.json 对应行均存在）。

---

## 6. macOS 顶部留白说明（已核实，非阻塞）

- 红绿灯仅位于左侧栏区域（由 `.aleph-sidebar-head` 的平台感知 padding 处理，`tailwind.css:1508-1509`），**不在右侧 Canvas 上方**。
- `.aleph-main-drag-band`（macOS 高 30px，`tailwind.css:1990-2002`）是 `<main>` 级**绝对定位**拖拽带，浮于内容之上、不占布局流。删 toolbar 后它自然改为浮在 Canvas 顶部 30px。
- `aleph-content-top`（2.45rem on macOS）是对齐美学，非硬性窗口安全要求。列表表格（`memory/mod.rs:248`）**自带** `aleph-content-top`，切到"列表"时仍保有顶部留白；Canvas 保持满幅。
- **决定**：Canvas 满幅（符合"归还给 Canvas"诉求）。macOS 下顶部 30px 成为窗口拖拽区（galaxy 仍在该区渲染，仅该窄条相机轨道操作让位于窗口拖拽）——可接受，亦提供从星图区域拖窗的便利。无需为 Canvas 加顶部内距。实现后在 macOS `.app` 目视确认即可。

---

## 7. 验证 (Verification)

- `cargo test -p aleph-panel --lib`：`state/memory.rs`（含 `parse_view_param`/`push_recent`）与既有 canvas 纯逻辑测试不回归；如对 Fold 映射抽出纯函数，补一条单测断言"全程 0..10 → lod 覆盖 [0,1] 两端"。
- `just wasm`：干净构建 + js/wasm 配对守卫通过。
- `just dev` 目视清单：
  1. 左侧栏从上到下 = agent 选择器 → 图谱 → 列表 → 搜索 → Fold；无「在列表中查看/编辑」重复块。
  2. 图谱/列表 tile 点击切换右侧视图，激活态高亮正确。
  3. 搜索：图谱模式 Enter 飞向命中节点；列表模式 Enter 提交服务端搜索并切到 Raw facet。
  4. agent 选择器切换驱动 Canvas 与列表重载。
  5. **Fold 滑块拖动可见连线疏密变化**（左端仅主干 ↔ 右端全部连线）。
  6. 右侧 Canvas 满幅；macOS `.app` 顶部无窗口 chrome 冲突。
- 部署（重编 `aleph-server` + 替换运行中 binary）单独一步，用户拍板。

---

## 8. 风险与回退 (Risks)

- 低风险：纯 Panel UI 重组 + 一处映射修复，无 Core/IPC 改动，符合 R4 纯 I/O。
- 命名：新 `memory_hub::MemorySidebar` 与被删的 `mode_sidebar.rs` 私有同名 fn 需同 PR 内删旧增新，避免并存歧义。
- 回退：改动集中在 6 个 Panel 文件，`git revert` 即可整体回退；dist 经 `just wasm` 重建。
