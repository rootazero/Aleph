# Aleph Panel — Phone Agents 下钻设计

- **Date**: 2026-06-27
- **Status**: Design approved, pending spec review
- **Applies the navigation law of**: `2026-06-26-aleph-panel-phone-memory-drilldown-design.md`（手机不分屏 → 多层级下钻）
- **Scope**: 仅 Agents tab 的手机屏。这是「四个分屏 tab 套同一法则」批次的第 1 份（顺序：Agents → More 入口 → Dashboard → Teams → Extensions），其余各自独立 spec。

---

## 1. 背景与问题

桌面 Agents tab 是经典的「左侧次级菜单 + 右侧内容」两列：

- 左栏 `AgentsSidebar`（`components/agents_sidebar.rs`，256px）：「+ New Agent」可折叠创建表单 + 筛选下拉（all / channel / standalone）+ agent 列表（每项 `[emoji] 名 [渠道徽章] [★默认]`，点击 → `/agents/{id}/overview`）+ 底部「默认 agent」下拉。
- 右栏 `AgentsView`（`platform/wide/views/agents/mod.rs`）：**已是单面板 + 横向 tab 条**（Overview / Files / Skills / Channels / Teams），**本身不是左右分屏**。

**关键洞察**：分屏只来自 `ModeSidebar`（agent 列表）与 `AgentsView` 并排。`AgentsView` 自身已是单面板竖向布局 + 横向 inline tab 条（`mod.rs:194–221`）。因此映射极干净——侧栏列表 → 着陆屏，AgentsView → 详情屏。

**手机现状**：`platform/phone/` 下**没有 `agents/` 目录**。底部 `PhoneTabBar`（`platform/phone/shell.rs:34`）有 Agents 按钮指向 `/agents`，但 `app.rs` 的 `MainContent` 对 Agents 臂**没有 form-factor 分支**，直接渲染桌面 `AgentsView` + `ModeSidebar` → 256px 侧栏 + 内容并排挤进手机宽 = **左右分屏**，且无法返回。这是四个分屏 tab 里**唯一在 TabBar 直接可点**的，最显眼。

用户裁定（2026-06-26，见 `feedback-phone-no-split-drilldown-law`）：

> 手机屏坚决不做左右分屏。桌面 panel 里所有左右分栏，全部转成「多层级下钻 + 返回」的屏幕。

## 2. 统一导航法则（沿用）

> 底部 `PhoneTabBar` = 顶层模式切换。每个模式的「着陆页」（全屏）= 该模式桌面版「左侧次级菜单」的内容。点菜单项 → 下钻到全屏内容页，内容页带 `‹` 返回回到菜单。绝不在手机宽度并排两列。

- Settings：着陆 = 设置分组列表 → 下钻每页（已实现）
- Chat：着陆 = 会话列表 → 下钻线程（已实现）
- Memory：着陆 = Graph/List 菜单 → 下钻 Canvas / Vault（已实现）
- **Agents：着陆 = agent 列表 → 下钻单 agent 详情（本 spec）**

## 3. Agents 手机版两屏结构

| 路由 | 屏幕 | 组件 | 内容 |
|------|------|------|------|
| `/agents` | **菜单着陆页** | 新建 `list.rs` → `PhoneAgentsList` | `PhoneShell title="Agents"`（无 back，是着陆）。顶部筛选 chips（All / Channel / Standalone）+「+ New Agent」内联可展开表单（复用 3 字段创建：ID / Display Name / Archetype）+ agent 列表行 `[emoji] 名 [渠道徽章] [★默认]`，点行 `navigate("/agents/{id}")` |
| `/agents/{id}` 及 `/agents/{id}/{tab}` | **单 agent 详情** | 新建 `detail.rs` → `PhoneAgentDetail` | `PhoneShell back="/agents" back_label="Agents"` 包住**复用的 `AgentsView`**（已单面板）。5 横 tab（Overview / Files / Skills / Channels / Teams）做成**可横向滚动 tab 条**（非分屏），tab 内容竖向全屏。返回 → `/agents` |

**控件归属**：筛选 + 新建归着陆页；**设默认 agent** 移到详情头部（「设为默认」按钮，当前 agent 非默认时显示）—— 保持着陆页干净，且更 iOS 化（在实体详情里管理它）。桌面侧栏底部的「默认 agent 下拉」在手机版不复刻。

**`/agents` → `/agents/{id}` → `/agents/{id}/overview` 归一**：桌面 `parse_agents_path()`（`views/agents/mod.rs:19–26`）已把 `/agents/{id}` 规范化为 `/agents/{id}/overview`。手机详情屏复用 `AgentsView` 时沿用此归一，tab 切换走 `AgentsView` 现有逻辑（pathname 内部分发），URL 在 `/agents/{id}/{tab}` 之间变化但**始终判为详情屏**（见 §4）。

## 4. 路由机制（无需显式注册）

`PanelMode::from_path`（`components/mode_sidebar.rs:40`）对任何 `path.starts_with("/agents")` 判为 `PanelMode::Agents`，`MainContent` 据此用 `style:display` 显示 Agents 内容。`PhoneAgents`（新 `mod.rs`）内部按 `use_location().pathname` 自行分发。因此 `/agents/{id}`、`/agents/{id}/files` 等**自动归 Agents 模式**，不需要在任何 `<Routes>` 注册——与 Memory 的 `/memory/note` 完全一致。

`PhoneAgents` 分发两路（提纯为纯函数，仿 Memory `screen_for_path`）：

```
screen_for_path(path):
  "/agents"        => Menu     // 着陆
  其它 "/agents…"  => Detail   // /agents/{id}、/agents/{id}/{tab}
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentsScreen { Menu, Detail }

#[must_use]
pub(crate) fn screen_for_path(path: &str) -> AgentsScreen {
    if path == "/agents" || path == "/agents/" {
        AgentsScreen::Menu
    } else {
        AgentsScreen::Detail
    }
}
```

底部 Agents tab 仍 `navigate("/agents")` → 落在菜单，符合「先看列表」。详情无 agent（坏 id / 直链）时由 `AgentsView` 现有逻辑兜底（沿用桌面行为），不另设重定向。

## 5. 数据层复用（R4）

不新增任何 core / IPC。沿用现有：

- `AgentsApi`（`api/agents.rs`）：`list` / `get` / `create` / `delete` / `set_default` / `files.{list,get,set,delete}` / `bindings`（经 `WorkspaceApi` 或 agents.bindings）。
- `DashboardState`（context）：`is_connected` + `rpc_call`。
- `AgentSummary`（id / name / emoji / description / model / is_default）、`WorkspaceFile`。
- 详情屏**直接复用** `views/agents/` 的 5 个 tab 内容组件（`OverviewTab` / `FilesTab` / `SkillsTab` / `ChannelsTab` / `TeamsTab`）——它们已是宽度无关的竖向表单，读 context / API。手机只换「外壳」：`PhoneShell` + 横滚 tab 条替代侧栏的角色。同 Memory graph 复用 `CanvasView` 的套路。

着陆页的 agent 列表 + 筛选 + 新建表单，逻辑移植自 `AgentsSidebar`（同样的 `AgentsApi` 调用、同样的 filter 状态机），表现层重写为全屏列表。

## 6. 组件改动清单

1. **新建** `platform/phone/agents/mod.rs` — `PhoneAgents` 路由器：按 `use_location().pathname` 经 `screen_for_path` 分发 Menu / Detail；`pub mod list; pub mod detail;`；`AgentsScreen` enum + `screen_for_path` 纯函数 + `#[cfg(test)]` 单测。OWNS agent 列表的拉取状态（gate on `is_connected`，仿 `chat_sidebar` connect-gated loader），供 list 屏读。
2. **新建** `platform/phone/agents/list.rs` — `PhoneAgentsList`：`PhoneShell title="Agents"`（无 back）；单一 wrapper 元素内放筛选 chips +「+ New Agent」内联表单（`agent_open` RwSignal 控制展开）+ agent 列表 cells（`.cell` 系列 idiom，行级 `navigate`）。
3. **新建** `platform/phone/agents/detail.rs` — `PhoneAgentDetail`：`PhoneShell back="/agents" back_label="Agents"`；body 内嵌 `AgentsView`（复用）；保证横向 tab 条在窄宽下可横滚不分屏；「设为默认」按钮（非默认时）。
4. **改** `app.rs` MainContent 的 Agents 臂 — 加 form-factor 分支：`FormFactor::Phone` → `PhoneAgents`，否则现有 `AgentsView` / `AgentsRouter`（镜像 Chat / Memory / Settings 的 swap）。
5. **改** `platform/phone/mod.rs` — `pub mod agents;`。
6. **无需动** `mode_sidebar.rs`（`from_path` 已含 `/agents`）/ router / `shell.rs` TabBar（Agents tab 已指 `/agents`）。

**PhoneShell footgun**：着陆页「静态筛选条 + 动态 agent 列表」等混合子节点必须包在单个元素内（见 `reference-leptos-phoneshell-dynamic-child-footgun`）。list / detail 均遵循。

## 7. 详情复用：范围与已知风险

- **本轮做**：详情屏全屏挂载复用的 `AgentsView`，5 个 tab 可切、内容竖向全屏可看可编辑（复用现有编辑逻辑）。
- **已知风险 / 后续 follow-up**：
  - `AgentsView` 横向 inline tab 条（5 项）在窄宽下可能溢出 → 本轮加 `overflow-x:auto` 横滚兜底；tab 条美化（吸顶、激活态滚动定位）作为独立 follow-up。
  - `FilesTab` 的多文件编辑网格在桌面是宽栅格 → 手机下竖向堆叠；内联 textarea 编辑可用但精修（如全屏编辑态）延后。
  - `AgentsView` 若有桌面专属固定宽度 / padding，在实现时按需加 `max-sm:` 兜底，不重写组件。
- **桌面边界**：`/agents/{id}` 系列是桌面与手机共用路径；桌面 `MainContent` 用 `AgentsView`，手机用 `PhoneAgents` 包裹同一 `AgentsView`，互不影响。

## 8. 不在本 spec 范围

- **More 标签 + sections 入口**：手机到达 Dashboard / Teams / Extensions 的统一入口，下一份独立 spec。
- **其余三个分屏 tab**（Dashboard / Teams / Extensions）：各自独立 spec/plan。
- **设默认之外的批量操作、agent 排序、tab 条吸顶美化、Files 全屏编辑态、per-tab 触屏精修**——延后。

## 9. 测试

- **单元**：`screen_for_path` 两路分发（`/agents` → Menu；`/agents/x`、`/agents/x/files` → Detail；尾斜杠 `/agents/` → Menu）。沿用现有 `AgentsApi` 相关数据层测试。
- **构建门**：`just wasm` 绿（WASM 编译是唯一真编译信号）。
- **iOS-sim QA（权威运行时门）**：按 `feedback-ios-panel-test-via-full-macos-app` 流程——重编完整版 macOS app（重嵌当前 dist 到 :18790）→ iOS sim 连同一本地 core → 验证：
  1. 底部 Agents tab → 落在 **agent 列表**（非分屏、非桌面侧栏挤压）
  2. 列表点 agent → 全屏详情 + `‹ Agents` 返回有效
  3. 详情 5 个 tab 可切换、可横滚、内容竖向全屏
  4. 「+ New Agent」内联表单可展开、可创建 → 列表刷新
  5. 详情「设为默认」对非默认 agent 可用 → ★ 更新
  6. 全程**无任何左右分屏**

## 10. 成功标准

手机 Agents tab 落地即见「agent 列表」（镜像桌面侧栏内容），可下钻到单 agent 全屏详情（5 横 tab，可滚），带 `‹ Agents` 返回；创建、筛选、设默认均可用；整个 Agents 流程在手机宽度下**零左右分屏**。
