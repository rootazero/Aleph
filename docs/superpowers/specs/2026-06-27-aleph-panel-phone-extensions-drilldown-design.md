# 手机 Extensions 下钻屏 设计 (split-tab redesign batch #5/5 · 最后一个)

> 套用「手机不分屏下钻法则」(feedback-phone-no-split-drilldown-law)。前序批次:Agents #1、More 入口 #2、Dashboard #3、Teams #4 全 done。Extensions 是四个分屏 tab 的**最后一个**;完成后整套法则全部落地。

## §1 背景:桌面 Extensions 结构

桌面 Extensions(Aleph Hub)是**全屏接管**模式,两列布局:

- **左栏 `ExtensionsSidebar` = `CategoryNav`**(`components/extensions/category_nav.rs`):顶部一个 "Back to Chat" 退出引导(全屏接管的唯一出口)+ `Featured` / `All` 两个全局项 + `CATEGORIES`(12 个功能 facet,各 `{value, label_key, emoji}`)。每项**只设 `store.category`**,过滤同一个 grid。
- **主区 `BrowsePane`**(`platform/wide/views/extensions/browse.rs`):chrome = `StoreSearch`(搜索)+ Installed 按钮(`store.show_installed.set(true)`)+ `FilterSegs`(kind/trust 段过滤);body = loading → empty → Featured shelves(`category=="featured"` 且无过滤时)或 flat filtered grid(`grid-cols-1 sm:grid-cols-2 lg:grid-cols-3`,**手机宽天然单列**)。
- **三个 overlay**(均由 app 级 `StoreState` 信号驱动,渲染于 `ExtensionsView` 内):
  - `ExtensionDetailDrawer`(`components/extensions/detail_drawer.rs`):`fixed inset-0 z-[60]` 右侧抽屉 `aside w-[480px] max-w-[94vw] h-full`。点卡片(`store.selected`)滑入。
  - `InstallFlow`(`components/extensions/install_flow.rs`):`fixed inset-0 z-50` 居中 modal `w-[480px] max-w-[94vw]`,多步安装流程。
  - `InstalledPanel`(`platform/wide/views/extensions/installed.rs`):`fixed inset-0 z-[60]` 右侧抽屉 `max-w-[94vw]`,`store.show_installed` 切换。

**关键事实**:`StoreState` 在 **`app.rs:116` app 级 `provide_context(StoreState::new())`** 提供(doc 注释说"provided by ExtensionsView"已过时),所以**任何子组件(含手机视图)都能 `expect_context::<StoreState>()`**。三个 overlay 的容器全是 `max-w-[94vw]` 的 fixed overlay —— 手机宽度下接近全屏,**不是左右分栏**。

## §2 导航法则

> 手机屏坚决不做左右分屏。桌面 panel 里所有左右分栏,全部转成"多层级下钻 + 返回"的屏幕。

模型:底部 `PhoneTabBar` 顶层模式切换 → 每模式手机着陆页 = 该模式桌面「左栏次级菜单」内容(全屏)→ 点菜单项下钻全屏内容页(带 ‹ 返回)。**绝不**手机宽度并排两列。

## §3 本批次的独特性:侧栏=过滤器而非子页面菜单

前 4 批的桌面侧栏都是**独立子页面/子视图菜单**(Dashboard 6 页、Teams 5 视图),映射成"菜单行→下钻独立内容页"。**Extensions 不同**:左栏 `CategoryNav` 的 14 项**全部渲染同一个 grid**,只是改 `store.category` 过滤值 —— 没有可下钻的独立子页面。

因此 Extensions **不套"侧栏项→菜单行→URL 下钻"的形态**,而是 **Browse 优先 + 横向分类条**(user 经 AskUserQuestion 选定):

- 着陆 `/extensions` = 全屏单列 grid(默认 Featured shelves)。
- 桌面左栏的纵向分类导航 → 手机**顶部横向滚动 chip 条**(恢复历史形态:browse.rs 注释记载分类导航本是 "the old horizontal CategoryChips … relocated to the left column")。
- 点 chip 即时切 `store.category`(in-page 信号,非 URL)。
- 点卡片 → 复用现有 detail drawer(94vw)。Install → 现有居中 modal。Installed → 现有抽屉(94vw)。

法则硬性约束(手机不并排两列)完全满足:单列 grid + 横向 chip 条,左栏被消除。

否决方案:**B′ 分类菜单→下钻 grid**(严格套法则,但强制选分类才看到扩展、Featured 策展被藏一层、且分类本是过滤器非页面);**C 模式菜单 Browse/Installed**(2 项菜单偏薄、去 catalog 多一跳)。

## §4 状态所有权与路由:本批次最简

| 维度 | Dashboard(#3) | Teams(#4) | **Extensions(#5)** |
|---|---|---|---|
| 状态所有权 | 无(叶子各携 app-wide context) | 路由器自持 `TeamsTabState` + 复刻桌面 load | **无**(`StoreState` 已 app 级 provide) |
| 子路由 | 6 个 phone-only 路径 | 5 个 phone-only 路径 | **零**(分类=signal、overlay=signal) |
| `screen_for_path` 纯函数 | 有(可单测) | 有(可单测) | **无**(无可路由子屏) |
| 连接门控 catalog load | 复用桌面视图自带 | 路由器复刻 | **`BrowsePane` 自带 `Effect`**(`is_connected` 触发 `load_catalog`),无需复刻 |

→ **零 `PanelMode` / `mode_sidebar` / `nav_menu` 改动**:`PanelMode::from_path` 用 `starts_with` 已把 `/extensions` 归类为 `PanelMode::Extensions`(`app.rs:429` 现有臂即证)。本批次只有一个路径 `/extensions`,trivially 覆盖。

## §5 组件构成(新模块 `platform/phone/extensions/`,2 文件)

### `bar.rs` · `PhoneCategoryBar`(唯一新增表现层)

横向滚动 chip 条,取代被消除的左栏 `CategoryNav`。

- 容器:`<div class="flex gap-2 overflow-x-auto cc-hide-scroll px-4 py-2">`(`cc-hide-scroll` 隐藏滚动条,ios.css:30 已有)。
- chip 集合:`[("featured", "★"), ("all", "🗂")]` 两个全局项 + `CATEGORIES.iter()` 各 `(c.value, c.emoji)`,共 14 个。
- 每 chip:`<button class=move || if store.category.get()==value {"chip chip-active"} else {"chip"} on:click=move|_| store.category.set(value.to_string())>` 内含 `emoji` + `category_label(i18n, value)` 文案。`.chip`/`.chip-active` 见 ios.css:46/56,**零新 CSS**。
- 不含桌面的 "Back to Chat" 退出项(那是桌面全屏接管专属;手机经 ••• More tab 离开)。
- 依赖:`expect_context::<StoreState>()`、`use_i18n()`、`crate::components::extensions::labels::category_label`、`crate::views::extensions::model::CATEGORIES`、`leptos_router` 不需要(纯信号,无 navigate)。

### `mod.rs` · `PhoneExtensions`(路由器/容器,无 state)

- `pub mod bar;` + `PhoneExtensions` 组件。
- 结构:
  ```
  PhoneShell title="Extensions"  (无 back —— 着陆页,经 ••• More tab 返回,与 Dashboard/Teams 着陆页一致)
    └─ 单个 <div>(footgun 防护:static + 复用组件包在一个普通 DOM 元素内):
         <PhoneCategoryBar/>
         <BrowsePane/>          // 原样复用:search + Installed 按钮 + FilterSegs + 响应式单列 grid + 自带 catalog load Effect
  <ExtensionDetailDrawer/>       // 原样复用(94vw overlay)
  <InstallFlow/>                 // 原样复用(94vw modal)
  <InstalledPanel/>              // 原样复用(94vw overlay)
  ```
- 不持有任何信号(`StoreState` 已 app 级);不订阅流;无 `Effect`(catalog load 在 `BrowsePane` 自带)。
- 依赖复用路径(全部已被 `ExtensionsView` 使用,故可达):
  - `crate::platform::phone::shell::PhoneShell`
  - `crate::platform::phone::extensions::bar::PhoneCategoryBar`
  - `crate::views::extensions::browse::BrowsePane`
  - `crate::components::extensions::detail_drawer::ExtensionDetailDrawer`
  - `crate::components::extensions::install_flow::InstallFlow`
  - `crate::views::extensions::installed::InstalledPanel`

### `platform/phone/mod.rs` 注册

`pub mod extensions;` 按字母序插在 `dashboard` 与 `memory` 之间。

## §6 接线(`app.rs`)

- import:`use crate::platform::phone::extensions::PhoneExtensions;`(phone 段字母序,在 `dashboard`/`memory` 相应位置;参照现有 `use crate::platform::phone::dashboard::PhoneDashboard;` 等)。
- Extensions 臂(`app.rs:429-431`)裸 `<ExtensionsView />` → form-factor swap,与其余 5 臂(Chat/Dashboard/Memory/Agents/Teams)同构:
  ```rust
  <div style:display=move || if mode.get() == PanelMode::Extensions { "contents" } else { "none" }>
      {move || if form_factor.form_factor.get() == FormFactor::Phone {
          view! { <PhoneExtensions /> }.into_any()
      } else {
          view! { <ExtensionsView /> }.into_any()
      }}
  </div>
  ```
- `form_factor` 已在 `MainContent` 可用(`app.rs:391`);`use crate::views::extensions::{ExtensionsView, StoreState};`(`app.rs:12`)保留;桌面 else 臂字节级不变。

## §7 交互流

着陆 `/extensions` → 全屏单列 grid(默认 Featured 策展 shelves)→ 顶部横向 chip 切分类(即时过滤)→ 搜索/段过滤(BrowsePane chrome)→ 点卡片 = detail drawer 滑入(94vw)→ Install = 居中 modal 多步流程 → Installed 按钮 = installed 抽屉(94vw)。全程底部 ••• More tab 高亮(`PanelMode::under_more()` 已含 Extensions)。

## §8 改动清单

| 文件 | 动作 |
|---|---|
| `platform/phone/extensions/bar.rs` | 新建 `PhoneCategoryBar` |
| `platform/phone/extensions/mod.rs` | 新建 `pub mod bar;` + `PhoneExtensions` |
| `platform/phone/mod.rs` | +`pub mod extensions;`(字母序) |
| `app.rs` | +import `PhoneExtensions`;Extensions 臂 form-swap |
| `dist/aleph_panel.js` + `_bg.wasm` | controller `just wasm` 重建(独立 commit) |

零 core/IPC、零依赖、零新 CSS、桌面字节级不变、R4(纯 I/O 渲染)。

## §9 测试 / 验证

- **无单元测试**:本批次无 `screen_for_path` 之类纯函数(分类=信号、overlay=信号、组件复用),纯表现层 + 信号接线,reviewer 逐行追溯。这是与前 4 批的刻意差异(它们测 `screen_for_path` 真值表;此处无路由可测)。
- **构建门**:controller-only `just wasm`(绿即 `✓ WASM dist OK`)。implementer 只转写 + 自审 + commit,不构建。
- **iOS-sim QA(权威运行时门,user-driven)**:按 feedback-ios-panel-test-via-full-macos-app 重编完整版 macOS app(重嵌当前 dist 于 :18790)→ iOS sim 连同一本地 core → 实测:
  1. ••• More → 点 Extensions → 全屏单列 grid(Featured shelves),**无左右分屏**;
  2. 顶部横向 chip 条可滚动,点分类即时过滤 grid,active chip 高亮;
  3. 点卡片 → detail drawer 全屏滑入,关闭返回;
  4. Installed 按钮 → installed 抽屉;Install → modal 流程;
  5. 全程 ••• tab 高亮。

## §10 成功标准

1. 手机 `/extensions` 单列 grid + 横向 chip 条,绝无并排两列。
2. 分类 chip 切换即时过滤(复用 `store.category`)。
3. 卡片/安装/已装三个 overlay 在手机宽度正常(94vw,复用桌面组件)。
4. 桌面 Extensions 字节级不变(仅 `app.rs` import + Extensions 臂 swap)。
5. 零 `PanelMode`/`mode_sidebar`/`nav_menu`/core/IPC/依赖/CSS 改动。
6. `just wasm` 绿。

## §11 延后(均设计内,非 defect)

- chip 条 sticky 固定(v1 随内容滚动);grid 卡片密度 phone 精修(已 `grid-cols-1` 够用);detail drawer / install modal 的 phone-native 重排(现 94vw overlay 够用,Canvas/Dashboard/Teams「宽交互延后」先例)。
- i18n:`category_label` 已走 i18n,无新债。

## §12 关联

收尾批次 —— 完成后「手机不分屏下钻法则」四个分屏 tab 全部落地。续 feedback-phone-no-split-drilldown-law、project-aleph-panel-phone-dashboard-drilldown(#3 无状态路由器先例)、project-aleph-panel-phone-teams-drilldown(#4)、project-aleph-panel-phone-more-entry(#2 Extensions 入口)、reference-leptos-phoneshell-dynamic-child-footgun。
