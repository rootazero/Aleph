# Aleph Panel — Phone Memory 下钻重设计 (FEATURE 4 修订)

- **Date**: 2026-06-26
- **Status**: Design approved, pending spec review
- **Supersedes the navigation model of**: `2026-06-26-aleph-panel-phone-memory-design.md` (FEATURE 4 "Vault-only v1")
- **Scope**: 仅 Memory tab 的手机屏。其余分屏 tab（Agents/Teams/Dashboard/Extensions）拆为后续独立 spec。

---

## 1. 背景与问题

桌面 Memory tab 是经典的"左侧次级菜单 + 右侧内容"两列布局：

- 左栏 `MemorySidebar`（256px）：Agent 选择器 + **Graph/List 视图切换** + 搜索框 + Fold 滑杆
- 右栏 `MemoryHub`：用 CSS-`display` 在 `CanvasView`（WebGL2 星系图）与 `Memory`（Vault 表格）间切换

当前手机版 `PhoneMemory`（FEATURE 4）是 **Vault-only 列表**：它跳过了那个"次级菜单"层级，也**完全没有 Graph/Canvas**。在连到旧 core（未嵌入 phone Memory 构建）时，手机会回退渲染桌面 `MemoryHub`，256px 侧栏 + 内容并排挤进手机宽度 = **左右分屏**，且无法返回。

用户裁定（2026-06-26）：

> **手机屏坚决不做左右分屏。桌面 panel 里所有左右分栏，全部转成"多层级下钻 + 返回"的屏幕。Memory 的着陆页应当是桌面侧栏菜单内容，点菜单项进入下一屏（例如 canvas 或列表），带返回按钮。**

## 2. 统一导航法则（codify）

本次确立一条适用于**所有 tab** 的手机导航法则（Chat / Settings 已实践，Memory 是下一个）：

> **底部 `PhoneTabBar` = 顶层模式切换。每个模式的"着陆页"（全屏）= 该模式桌面版"左侧次级菜单"的内容。点菜单项 → 下钻到全屏内容页，内容页带 `‹` 返回回到菜单。绝不在手机宽度并排两列。**

- Settings：着陆 = 设置分组列表 → 下钻每个设置页（已实现）
- Chat：着陆 = 会话列表 → 下钻线程（已实现）
- **Memory：着陆 = Graph/List 菜单 → 下钻 Canvas / Vault（本 spec）**

## 3. Memory 手机版四屏结构

| 路由 | 屏幕 | 组件 | 内容 |
|------|------|------|------|
| `/memory` | **菜单着陆页** | 新建 `menu.rs` → `PhoneMemoryMenu` | 镜像 `MemorySidebar`：顶部 Agent 选择器（内联可展开）+ 两个大导航行：**🌌 星系图 (Graph)** → `/memory/graph`；**☰ 列表 (List)** → `/memory/list` |
| `/memory/graph` | **全屏星系图** | 新建 `graph.rs` → `PhoneMemoryGraph` | 复用桌面 `CanvasView`（已是 `absolute inset-0` 全填充 WebGL2）；`PhoneShell` 带 `back="/memory"` `back_label="Memory"`；Fold 滑杆作小浮层；节点详情用 canvas 现有 overlay |
| `/memory/list` | **全屏 Vault** | 现有 `PhoneMemoryList` 改挂此路由 | 搜索 + facet chips + 计数 + 笔记 cells + Load more 全部不变；`PhoneShell` 加 `back="/memory"` `back_label="Memory"` |
| `/memory/note` | **笔记详情** | 现有 `detail.rs` → `PhoneMemoryDetail` | 只读详情不变；返回改指 `/memory/list`（`back_label="List"`） |

**控件归属**：搜索归 List 屏（已自带），Fold 归 Graph 屏。菜单只放 Agent + 两个目的地，保持干净（不在菜单重复搜索/Fold）。

## 4. 路由机制（无需显式注册）

`PanelMode::from_path` 对任何 `path.starts_with("/memory")` 判为 `PanelMode::Memory`，`MainContent` 据此用 `style:display` 显示 `PhoneMemory`（手机）。`PhoneMemory`（`mod.rs`）内部按 `use_location().pathname` 自行分发。因此 `/memory/graph`、`/memory/list` **自动归 Memory 模式**，不需要在任何 `<Routes>` 注册——与现有 `/memory/note` 完全一致。

`PhoneMemory` 分发扩成四路：

```
match pathname:
  "/memory/note"  => PhoneMemoryDetail
  "/memory/graph" => PhoneMemoryGraph
  "/memory/list"  => PhoneMemoryList
  _ (= "/memory") => PhoneMemoryMenu
```

底部 Memory tab 仍 `navigate("/memory")` → 落在菜单，符合"先看菜单"。

## 5. 数据层复用（R4）

不新增任何 core/IPC。沿用现有：

- `PhoneMemoryState`（router-owned，`mod.rs`）：window/loaded/error/facet/query/page/selected/reload_nonce —— **保持不变**，List/detail 继续读它。
- Agent 选择器读写 `MemoryState.agent_id` / `MemoryState.agents`（与桌面共享，菜单切 agent 自动驱动 List 的 note-window loader 重载）。
- Graph 屏复用 `CanvasView` + `MemoryState`（`memory_view` / `fold_threshold` / `search_*`）。Graph 屏挂载时不强制设 `memory_view`——`CanvasView` 自渲染图；Fold 浮层写 `MemoryState.fold_threshold`。

## 6. 组件改动清单

1. **新建** `platform/phone/memory/menu.rs` — `PhoneMemoryMenu`：`PhoneShell title="Memory"`（无 back，是着陆）；单一 wrapper 元素内放 Agent 选择器（内联可展开列表，移植桌面 popover 逻辑为内联）+ 两个 `nav` 行（Graph / List），点行 `navigate` 到对应子路由。
2. **新建** `platform/phone/memory/graph.rs` — `PhoneMemoryGraph`：`PhoneShell back="/memory" back_label="Memory"`；body 内 `CanvasView` 充满 + Fold 浮层。
3. **改** `platform/phone/memory/list.rs` — `PhoneShell` 由 `title="Memory"` 改为 `title="Memory" back="/memory" back_label="Memory"`；其余不动。
4. **改** `platform/phone/memory/detail.rs` — 返回目标由 `/memory` 改为 `/memory/list`，`back_label="List"`。
5. **改** `platform/phone/memory/mod.rs` — pathname 分发扩四路；`pub mod menu; pub mod graph;`。
6. **改** 无需动 `app.rs` 路由 / `shell.rs` TabBar（Memory tab 已指 `/memory`）。

**PhoneShell footgun**：菜单的"静态标题 + 动态 agent 列表"等混合子节点必须包在单个元素内（见 `reference-leptos-phoneshell-dynamic-child-footgun`）。List/menu 均遵循。

## 7. Canvas-on-phone：范围与已知风险

- **本轮做**：`CanvasView` 全屏挂载、渲染星系图、Fold 可调、节点详情 overlay 可用。复用桌面组件，零改动优先。
- **已知风险 / 后续 follow-up**：`canvas/gl/` 输入处理器可能只绑鼠标事件 → 触屏 pan/zoom/tap-select 在手机上可能不响应。本轮**先让它全屏挂载可看**；触屏手势（touch → pan/zoom/pick）作为独立 follow-up，不阻塞本 spec 合入。
- **桌面边界**：`/memory/graph`、`/memory/list` 是 phone-only 导航路径；桌面 `MemoryHub` 只认 `?view=`，桌面用户不会走这两个路径，忽略即可。

## 8. 不在本 spec 范围

- **其余分屏 tab**（Agents / Teams / Dashboard / Extensions）：同样要套第 2 节法则，各自独立 spec/plan。本 spec 不碰。
- **Canvas 触屏手势**：见第 7 节，follow-up。
- Graph 屏的搜索框、Raw facet、笔记内联编辑、排序、多 agent 并列——延后（沿用 FEATURE 4 的 v2 边界）。

## 9. 测试

- **单元**：`PhoneMemory` 四路分发（pathname → 正确屏幕）；菜单 nav 行 `navigate` 目标；agent 选择写入 `MemoryState.agent_id`。沿用现有 `filter_notes` 等数据层测试。
- **构建门**：`just wasm` 三次绿（WASM 编译是唯一真编译信号）。
- **iOS-sim QA（权威运行时门）**：按 `feedback-ios-panel-test-via-full-macos-app` 流程——重编完整版 macOS app（重嵌当前 dist 到 :18790）→ iOS sim 连同一本地 core → 验证：
  1. 底部 Memory tab → 落在**菜单**（非分屏、非直接列表）
  2. 菜单点 List → 全屏 Vault + `‹ Memory` 返回有效
  3. 菜单点 Graph → 全屏星系图 + Fold 可调 + `‹ Memory` 返回有效
  4. List 点笔记 → 详情 + `‹ List` 返回有效
  5. 全程**无任何左右分屏**

## 10. 成功标准

手机 Memory tab 落地即见"菜单着陆页"（镜像桌面侧栏），可下钻 Graph（全屏星系图）与 List（全屏 Vault），每屏带返回，逐级回到菜单；整个 Memory 流程在手机宽度下**零左右分屏**。
