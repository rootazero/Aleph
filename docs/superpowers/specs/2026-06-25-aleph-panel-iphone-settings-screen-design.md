# Aleph Panel — iPhone Settings 屏 (landing) 设计

- **日期**: 2026-06-25
- **范围**: iPhone 重建第一屏 —— Settings landing,1:1 照搬 `docs/design-system/aleph-mobile/screens/4-settings.png`(源码 `docs/design-system/aleph-mobile/screens/exported/Aleph Settings.dc.html`)。**本轮只做这一屏,做完停下给用户 390px 目视对照。**
- **Crate**: `aleph-panel`(`interfaces/webchat/`),Leptos 0.7 + WASM + Tailwind v4。
- **HEAD 基线**: `ce9afb6ca`(platform/{wide,phone,tablet} 重组后)。

---

## 1. 目标与非目标

### 目标
- 在 **<640px**(iPhone)宽度下,`/settings` 渲染一个**全屏 iOS 原生** Settings 屏:glass 顶栏 + 三组 inset 卡片列表 + 底部 TabBar,视觉 == `4-settings.png` 内层(390×844)。
- 桌面/平板(≥640px)`/settings` **字节级不变**,无回归。

### 非目标(本轮明确不做)
- 不建其余 phone 屏(Chat/Memory/Agents/Voice/Notifications)。
- 不接真实 config 数据(值先静态占位,下一步再接)。
- 不 reflow 桌面、不用 `max-sm:` 藏栏、不碰 `platform/wide/`。
- 不引第二 UI 源到原生 Bridge(R2),不引新依赖。

---

## 2. 架构红线对齐

- **R2(UI 逻辑唯一源)**: 屏在 Leptos Panel 实现,非原生 Bridge。
- **R4(Interface 纯 I/O)**: PhoneSettings 不做持久化/记忆/规划;cell/tab 点击只做 `use_navigate` 路由跳转,值为静态展示。
- **P1/P5(低耦合/最小知识)**: 复用 crate-root `state` / 既有路由;iOS 组件类集中在 `styles/ios.css`。
- **隔离**: phone 代码只在 `platform/phone/`;唯一进入共享层的改动是 `src/app.rs` 的一处路由分支 + 一处 context(`app.rs` 是共享根,不属于 `platform/wide/`)。

---

## 3. 集成机制 —— Fixed 全屏覆盖层(已选定)

桌面是两栏 shell(`ModeSidebar` + `<main class="flex-1">`)。要让 PhoneSettings 在 390px 真正全屏不被左侧栏挤:

- `PhoneSettings` 自身用 **`position:fixed; inset:0; z-50`**,脱离 flex 流、盖住整窗。
- 在共享 `src/app.rs` 的 `SettingsRouter` 的 `"/settings"` 分支处分流:

```
"/settings" => if form_factor == Phone { <PhoneSettings/> } else { <Settings/> }
```

- 桌面分支(`<Settings/>`)及其余所有路由分支**一字不动** → 渲染输出对 ≥640px 字节级不变。
- 代价(接受,可逆): 桌面 shell 在 phone 下仍挂载于覆盖层之下(被盖住、不可见);切到非 Settings 路由时覆盖层卸载,会露出 wide UI(本轮只建 Settings 一屏的已知局限,见 §8)。

---

## 4. 改动清单(4 新文件 + 2 处共享微调 + 2 行 mod 登记)

### 4.1 `interfaces/webchat/styles/ios.css`(新文件)
从 `docs/design-system/aleph-mobile/screens/exported/styles/aleph.css` **逐字复制**以下类(及其当前数值):

```
.list-header  .list  .cell  .cell:last-child
.cell-leading  .cell-body  .cell-title  .cell-sub  .cell-value  .cell-chevron
.tabbar  .tabbar.glass  .tabitem  .tabitem-active
.swatch  .swatch-active  .mono
```

- **不复制** `.glass` / `.tabular-nums` —— 已在共享 `tailwind.css`(line 483 / 220),复用。
- 这些类引用的 token(`--color-surface-raised/-border-subtle/-primary-subtle/-primary/-text-tertiary/-surface-overlay/-text-secondary/-border`、`--radius-xl/-md/-full`、`--shadow-sm`、`--safe-area-bottom`、`--font-mono`)在 `tailwind.css` **已全部存在**,无需新增。
- **无类名冲突**: 上述类名在 `tailwind.css` 中均不存在(已核对)。

### 4.2 接入构建 —— 改 `interfaces/webchat/styles/tailwind.css`(1 行)
在文件顶部 `@import "tailwindcss";` 之后插入:

```css
@import "./ios.css";
```

理由: 实际被 serve 的样式表是 `npm run build:css`(`@tailwindcss/cli -i styles/tailwind.css -o dist/tailwind.css`)的产物,运行时 `dist/index.html`(由 `just wasm` 生成)**只**链接 `/tailwind.css`。Trunk 的 `index.html` 与其 `data-trunk` 链接**不是**被 serve 的东西。Tailwind v4(Lightning CSS)会把相对 `@import` 的本地 CSS 内联进 `dist/tailwind.css`。**因此不改 `index.html`、不改 `justfile`、不加第二个 `<link>`。**

### 4.3 `interfaces/webchat/src/state/viewport.rs`(新文件,移植精简版)
```
pub enum FormFactor { Wide, Phone, Tablet }   // <640 Phone, <1024 Tablet, else Wide

pub struct FormFactorState { pub form_factor: RwSignal<FormFactor> }  // Copy
  - new(): 初值取 window.inner_width;注册 resize 监听,变化时 set。
```
- 参考 `git show archive/mobile-reflow-20260625:interfaces/webchat/src/state/viewport.rs`,但**删除 `drawer_open`**(旧 reflow 残留,本方案不需要)。
- `state/mod.rs` 加 `pub mod viewport;`。
- **仅 `== Phone` 会触发分流**;≥640px 一律 Wide/Tablet → 都落 `SettingsRouter` 的 `else` 分支 → 桌面/平板渲染不变。Tablet 变体保留供未来,本轮无 Tablet 屏,渲染等同 Wide。

### 4.4 `interfaces/webchat/src/app.rs`(共享层,2 处)
- `AppContent` 函数体加: `provide_context(FormFactorState::new());`(纯新增 context + resize 监听;Wide 行为零变化)。
- `SettingsRouter` 的 `"/settings"` 分支: 读 `expect_context::<FormFactorState>()`,`Phone → <PhoneSettings/>`,否则 `<Settings/>`(原样)。引入 `use crate::platform::phone::settings::PhoneSettings;`。其余分支不动。

### 4.5 `interfaces/webchat/src/platform/phone/settings.rs`(新文件)+ `platform/phone/mod.rs` 加 `pub mod settings;`
`PhoneSettings` 组件,根容器 `position:fixed; inset:0; z-50`,内部 flex 纵向列(顶栏 / 滚动列表 / TabBar),逐字照 `Aleph Settings.dc.html`:

- **顶栏**: `.glass`,内联 `min-height:50px; padding:4px 14px 8px`,标题 `<span>` `font-size:20px; font-weight:700; letter-spacing:-0.02em`,文案 `Settings`。顶部叠加 `env(safe-area-inset-top)` 内边距(浏览器为 0)。
- **滚动区**: `flex:1; overflow-y:auto; display:flex; flex-direction:column; gap:20px; padding:16px 16px 18px`,`scrollbar-width:none`。
- **三组**(每组 `<div>` 包 `.list-header` + `.list`):
  - **连接**(`.list-header` 文案 `连接`): Connection,值 `remote · 10.10.10.4`(`.cell-value.mono` 13px)。
  - **AI**(`AI`): Providers(`Anthropic`)、Embeddings(`text-embedding-3`,`.mono` 13px)、Model route(`Opus 4.8`)。
  - **外观**(`外观`): Theme(`System`)、Accent(5 个 `.swatch` 26×26,首个 `.swatch-active`)、Material(`Luxe`)。
  - 每个 `.cell` 结构: `.cell-leading`(内嵌 17px SVG,逐字抄 dc.html 的 path)+ `.cell-body>.cell-title` + `.cell-value`(可选 `.mono`)+ `svg.cell-chevron`(18px,`points="9 6 15 12 9 18"`)。Accent 行无 chevron,改放 swatch 行。
  - **5 个 Accent 色**(逐字): Mauve `oklch(0.55 0.120 310)`(active)/ Ocean `oklch(0.55 0.130 250)`/ Forest `oklch(0.53 0.115 150)`/ Sunset `oklch(0.62 0.135 60)`/ Rose `oklch(0.57 0.150 15)`。
- **底部 TabBar**: `.tabbar.glass`,内联 `padding-bottom:calc(0.4rem + 16px)`(或保留 dc.html 原值),4 个 `.tabitem`(SVG + 文案逐字): Chat / Memory / Agents / Settings,Settings 加 `.tabitem-active`。
- **不渲染**: faux 状态栏(`9:41`)、灵动岛、home indicator —— 那是 PNG 的手机模拟外壳,真机由 OS 绘制。

---

## 5. 交互(I/O-only)

| 元素 | 动作 |
|------|------|
| Connection cell | `use_navigate("/settings/network")` |
| Providers cell | `/settings/providers` |
| Embeddings cell | `/settings/embedding-providers` |
| Model route cell | `/settings/model-route` |
| Theme / Accent / Material cell | `/settings/appearance` |
| TabBar · Chat | `/` |
| TabBar · Memory | `/memory` |
| TabBar · Agents | `/agents` |
| TabBar · Settings | 当前态(`.tabitem-active`),不跳 |

TabBar 非 Settings 项**直接跳现有路由**(已确认);跳到的是桌面 wide 页面(见 §8 局限 1)。

---

## 6. 占位项(v1 静态,下一步接真实 config)

所有显示值先**硬编码静态**对齐视觉:

| 项 | v1 静态值 | 下一步数据源 |
|----|-----------|--------------|
| Connection | `remote · 10.10.10.4` | `location.host` |
| Providers | `Anthropic` | providers config API |
| Embeddings | `text-embedding-3` | embedding config API |
| Model route | `Opus 4.8` | route config API |
| Theme | `System` | `appearance.rs` |
| Accent(active swatch) | Mauve(首个) | `appearance.rs` |
| Material | `Luxe` | appearance/material config |

---

## 7. 验证(集中一次,cargo 极度节制)

1. `cargo check -p aleph-panel --lib`(**至多一次**;移动文件后 RA 可能报 `unlinked-file`/`E0583 views` 等**陈旧假错**,以 cargo 实编为准)。
2. `just wasm` 重建 dist(含 `npm run build:css` → ios.css 内联进 `dist/tailwind.css`)。
3. 确认 `aleph-server` 跑在 `:18790`(当前未起 → 实现阶段按需 `cargo run --bin aleph-server` 起 debug,或用既有 daemon)。dev daemon 从盘读 dist,`just wasm` 即生效,无需重编 server。
4. 验证 served wasm == 磁盘: `curl -s :18790/aleph_panel_bg.wasm | wc -c` == `wc -c < interfaces/webchat/dist/aleph_panel_bg.wasm`。
5. chrome-devtools `emulate 390x844x3, mobile, touch` → 开 `/settings` → 截图对照 `4-settings.png`(分组 inset 卡片 + primary 圆角图标块 + 标题/值/`›` + 底部 TabBar)。
6. 桌面回归: `emulate 1280x900x2` → `/settings` 仍是原桌面双栏。

---

## 8. v1 已知局限(交付注明)

1. 点 cell 或非 Settings tab → 覆盖层卸载后露出**桌面 wide 页面**(手机上偏窄)—— 本轮只建 Settings 一屏。
2. 字体走系统回退(`-apple-system`/SF、`ui-monospace`),**非 Inter** —— 与现网 panel 一致(serve 的 `index.html` 不加载 Google fonts;不在本轮"修",避免给桌面 App 引入远程字体拉取)。390px 截图因此字体与 PNG(用 Inter)略有差异,不影响布局/层级对照。
3. 覆盖层之下桌面 shell 仍挂载(被盖住)—— 无害、可逆;未来 phone 屏铺开时可升级为独立 phone shell。

---

## 9. 完成定义(DoD)

- 390px `/settings` 视觉 == `4-settings.png` 内层(三组 inset 卡片 + primary 圆角图标块 + 标题/值/`›` + 底部 TabBar,Settings 高亮)。
- `cargo check -p aleph-panel --lib` 过;`just wasm` 绿;served wasm == 磁盘。
- 1280px 桌面 `/settings` 无回归(原双栏)。
- 变更只落 `interfaces/webchat/`;**未提交、未 push**;交付列出 §6 占位项与 §8 下一步。
