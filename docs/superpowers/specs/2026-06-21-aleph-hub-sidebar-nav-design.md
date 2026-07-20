# Aleph Hub 左栏导航化 + 隐藏 Hub 内切换器

**Date:** 2026-06-21
**Status:** Approved (design)
**Scope:** `interfaces/webchat/` (Leptos/WASM Panel) — 纯前端布局调整，不动 core / model / i18n

## 1. 背景与目标 (Context & Goal)

从 chat 窗口进入 Aleph Hub（`/extensions`）后，左侧菜单栏（`ExtensionsSidebar`）当前是一个空 `<div>`，而右侧主区把「搜索框 + 横向分类 chips + 类型/信任筛选段」三行工具条都挤在卡片网格上方，显得拥挤。

目标：
1. 把**精选 + 分类**导航从右侧横向 chips 迁移为左栏垂直菜单，充分利用空置的左栏、给右侧主区减负。
2. 新增一个**「全部」全局入口**（展示全部扩展的扁平网格）。
3. 删除 Hub 内左栏底部的 section 切换器「tab 弹窗」（`NavMenu`）—— 它只应在回到 chat 窗口后出现。

## 2. 现状 (Current State)

- `interfaces/webchat/src/views/extensions/mod.rs`
  - `ExtensionsView`（主区，`MainContent` 内）：`StoreState::new(); provide_context(store)`，头部含 Back to Chat / 标题 / Installed。
  - `ExtensionsSidebar`（左栏，`ModeSidebar` 内）：空 `<div class="flex flex-col h-full"></div>`。
- `interfaces/webchat/src/views/extensions/browse.rs` — `BrowsePane`：顶部依次渲染 `<StoreSearch /> <CategoryChips /> <FilterSegs />`，随后按 `featured_view` 渲染「Featured + 货架」或扁平网格。
- `interfaces/webchat/src/components/extensions/chips.rs` — `CategoryChips`（横向 chip 行，唯一消费者是 `BrowsePane`）、`FilterSegs`、`StoreSearch`、`pub use category_label`。
- `interfaces/webchat/src/components/mode_sidebar.rs` — `ModeSidebar` 底部无条件渲染 `<NavMenu />`。Extensions 模式下其触发按钮显示当前 section = 「Aleph Hub」。
- `interfaces/webchat/src/components/nav_menu.rs` — `NavMenu`：底部 section 切换器，点击向上弹出 popup（Extensions 已不在 `ALL_MODES` 列表，但触发按钮仍镜像当前 section）。
- `interfaces/webchat/src/views/extensions/model.rs` — `category` 取值 `"featured" | "all" | <CATEGORIES.value>`；`matches()` 已把 `"all"` 当作 pass-through（显示全部）；`category_label(i18n, "all")` → `extensions.cat.all` = 全部 / All（zh.json / en.json:1314 已存在）。
- `interfaces/webchat/src/app.rs` — `AppContent` 在两列（`ModeSidebar` + `MainContent`）的共同父级 provide 了 `MemoryState` / `ChatState` 等；其中 `ChatState` 的注释明确「lives above both the chat sidebar and the chat view so they share one session/agent selection」。

## 3. 关键架构决策：状态共享 (State Sharing)

左栏 `ExtensionsSidebar`（在 `ModeSidebar`）与右侧 `BrowsePane`（在 `MainContent`）是**兄弟**节点，而 `StoreState` 现在由 `ExtensionsView`（主区内）`provide_context`，左栏无法 `expect_context` 到它。

**方案 A（采用）**：把 `StoreState::new() + provide_context(store)` 上提到 `AppContent`，与 `ChatState` 并列。左栏与主区都改用 `expect_context::<StoreState>()`，共享同一个 `category` 信号。

- 这正是仓库已有的 `ChatState` 模式（app.rs:62-67），有先例、低风险。
- 无加载时机变化：`BrowsePane` 因 `MainContent` 的 display-toggling 本就常驻挂载，`load_catalog` 在连接后即触发；上提 provide 不改变这一点。

**方案 B（否决）**：新建只含 `category/kind/trust/query` 的轻量 `ExtensionsNavState`。→ 与 `StoreState` 字段重复，制造第二真相源，违背 KISS / 单一真相源。

## 4. 设计 (Design)

### 4.1 左栏垂直分类导航（新组件 `CategoryNav`）

新建 `interfaces/webchat/src/components/extensions/category_nav.rs`，由 `ExtensionsSidebar` 渲染。结构（自上而下）：

```
★ 精选 (featured)   ← 默认；展示 Featured + 各分类货架
🗂 全部 (all)        ← 新增「全局」入口；展示全部扁平网格
────────── (细分割线)
🔍 搜索  🛠 开发  🗄 数据  ⚡ 效率  ✍ 写作  💬 沟通
📚 知识  📁 文件  🎨 设计  🔁 自动化  💰 财务  🧰 工具  • 其他
```

- 精选 / 全部 作为两个「全局」入口置顶，一条 `border-t border-border` 分割线后是 13 个 `CATEGORIES` 分类（复用 `CATEGORIES` 常量 + 各自 emoji + `category_label`）。
- 样式对齐 `SettingsSidebar` 的 `nav-tile` / `nav-tile-active`（icon + label、圆角、active 高亮），保持各 sidebar 一致观感。
- 每项点击仅 `store.category.set(value)`（与现有 chip 行为字节一致，**不**重置 kind/trust/query）。
- 容器 `flex flex-col h-full`，列表 `overflow-y-auto` 防溢出。
- 「全部」的图标取 `🗂`（可调）；「精选」沿用 `★`。

### 4.2 右侧主区去拥挤

`BrowsePane`（browse.rs）移除顶部的 `<CategoryChips />` 及其 import。主区顶部工具条变为「搜索框 + 类型/信任筛选段」两项，下接网格。

- 这是**方案 A 范围**：仅分类导航进左栏；搜索框 + 类型/信任筛选保留在右侧主区。
- `CategoryChips` 失去唯一消费者 → 作为本次改动产生的孤儿，从 `chips.rs` 删除（连同其在 `browse.rs` 的 import）。`chips.rs` 保留 `FilterSegs` / `StoreSearch` / `pub use category_label`。
- `featured_view` 判定逻辑（category=="featured" 且筛选全清且搜索空）保持不变 —— 精选展示富货架、全部/各分类展示扁平网格的行为天然成立。

### 4.3 隐藏 Hub 内底部切换器

`ModeSidebar`（mode_sidebar.rs:80）把无条件的 `<NavMenu />` 改为按模式条件渲染：

```rust
{move || (mode.get() != PanelMode::Extensions).then(|| view! { <NavMenu /> })}
```

- Extensions 模式下左栏底部不再出现显示「Aleph Hub」的切换器。
- 其它模式（Chat / Dashboard / Memory / Agents / Teams / Settings）照常渲染 `NavMenu`。
- Hub 是全屏专注态，靠主区头部「Back to Chat」退出；回到 chat 后切换器自然重现 —— 对应「tab 弹窗之后返回 chat 窗口才出现」。

## 5. 改动清单 (Files)

| 文件 | 改动 |
|---|---|
| `src/app.rs` | `AppContent` 内上提 `StoreState::new() + provide_context`（仿 `ChatState`，附注释） |
| `src/views/extensions/mod.rs` | `ExtensionsView`：`StoreState::new()`→`expect_context::<StoreState>()`；`ExtensionsSidebar` 渲染 `<CategoryNav />` |
| `src/components/extensions/category_nav.rs` | **新建**：垂直分类导航（精选 / 全部 / 13 分类） |
| `src/components/extensions/mod.rs` | 注册 `pub mod category_nav;`（按现有 mod 声明惯例） |
| `src/views/extensions/browse.rs` | 移除 `<CategoryChips />` 渲染 + import |
| `src/components/extensions/chips.rs` | 删除孤儿 `CategoryChips`（保留 `FilterSegs` / `StoreSearch` / `category_label` 再导出） |
| `src/components/mode_sidebar.rs` | Extensions 模式下不渲染 `NavMenu` |

## 6. 不做 (YAGNI / Out of Scope)

- 不改 `model.rs` / i18n：`"all"` 的过滤逻辑与 `extensions.cat.all` 标签均已齐备。
- 不为左栏折叠态（`mem.sidebar_collapsed`）做移动端 chip 回退 —— 与聊天历史折叠行为一致，展开即可用。
- 不改点击分类时的筛选重置行为 —— 与现有 chip 保持一致。
- 不动右侧搜索框 / 类型/信任筛选段的位置（除非后续要求方案 B）。

## 7. 验证 (Verification)

- 编译：`cargo check -p aleph-panel --lib --target wasm32-unknown-unknown`（节制 cargo，至多一次）。
- 功能预期：
  - 进入 `/extensions` 后，左栏出现「精选 / 全部 / 13 分类」垂直导航，点击切换 = 右侧网格随之变化。
  - 「精选」展示 Featured + 货架；「全部」展示全部扩展的扁平网格；各分类展示对应过滤网格。
  - 右侧主区顶部仅剩搜索框 + 类型/信任筛选段。
  - Hub 内左栏底部不再有 section 切换器；回到 `/chat` 后切换器重现。
- 部署（如需眼见为实）：`just wasm` → 重编 `aleph-server` binary → 替换运行中 binary（CLAUDE.md「Panel ↔ Daemon 资源嵌入链」）。

## 8. 风险 (Risks)

- **Context 漏配**：若 `StoreState` 未成功上提到共同父级，左栏 `expect_context` 会 panic。缓解：严格仿照 `ChatState` 的 provide 位置；编译期 + 运行点验证。
- **孤儿删除误伤**：删 `CategoryChips` 前确认全仓唯一消费者是 `browse.rs`（grep 验证）。
