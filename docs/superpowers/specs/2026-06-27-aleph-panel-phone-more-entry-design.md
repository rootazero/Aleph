# 手机端 More 入口 设计 (Phone More Entry)

> 批次 #2/4。四个还在左右分屏 / 手机不可达的 tab(Agents/Teams/Dashboard/Extensions)套同一「手机不分屏下钻法则」,各自独立 spec。顺序:Agents(#1,已完成)→ **More 入口(本 spec)** → Dashboard → Teams → Extensions。

## §1 背景与问题

底部 `PhoneTabBar`(`interfaces/webchat/src/platform/phone/shell.rs`)只有 4 格:Chat / Memory / Agents / Settings。桌面切换 section 靠左栏底部的 `NavMenu`(`components/nav_menu.rs`),它渲染在 `ModeSidebar`(256px 桌面左栏)里。手机上 `ModeSidebar` 被各模式的 `PhoneShell`(`fixed h-dvh z-[70]`)覆盖,所以 `NavMenu` 在手机上**几乎不可达** → Dashboard / Teams / Extensions 三个 section 在手机上**没有入口**。

入口决策(2026-06-27,user 已定):**加第 5 个 More(•••)tab** → 全屏 sections 菜单 → 下钻 Dashboard / Teams / Extensions。Chat / Memory / Agents / Settings 保持主 tab。

## §2 导航法则(复用既有约束)

- 手机屏**坚决不做左右分屏**。
- 底部 `PhoneTabBar` = 顶层模式切换。
- 每个模式的手机**着陆页**(全屏)= 该模式桌面版「左侧次级菜单」内容。
- More 是新增的顶层入口:着陆页 = sections 菜单(列出 Dashboard/Teams/Extensions);点一行 = 钻出 More、进入那个 mode。
- 路由驱动:`PanelMode::from_path` 用 `starts_with` 归类,`MainContent` 按 `mode` 渲染对应臂,tab active 由 `mode` 推出。

## §3 范围(只做入口)

**做**:第 5 个 ••• tab + `PhoneMore` sections 菜单屏 + 导航到三个 mode 的路由。

**不做(各自后续独立 spec)**:Dashboard / Teams / Extensions 的手机专属屏。点 section → 先导航;在该 section 自己的 spec 落地前,**仍显示当前桌面布局**(可能仍是分屏)。这**不是回归**——这三个 section 此前在手机上根本不可达,现在变成「可达但暂未优化」。(user 2026-06-27 确认此过渡期行为。)

## §4 路由机制 —— 新增 `PanelMode::More`

`/more` 必须能被 `from_path` 归类,否则 fallthrough 成 `Chat`(导致 Chat tab 与 More tab 同时高亮、`MainContent` 渲染 `PhoneChat`)。给共享枚举 `PanelMode` 加一个 `More` 值是忠实延续既有「路由驱动」模式的唯一干净解。

桌面侧改动**纯增量、桌面永不可达**(`/more` 只有手机 ••• tab 能到;`NavMenu::ALL_MODES` 不含 More,桌面无任何链接指向 `/more`)→ 桌面功能字节级不变,新增的 `match` 臂仅为穷尽性存在:

- `mode_sidebar.rs`:
  - `enum PanelMode` 加 `More`。
  - `from_path`:加 `else if path.starts_with("/more") { Self::More }`(置于 `Chat` fallback 之前;与现有任何前缀不冲突)。
  - `ModeSidebar` 的 `match mode`:加 `PanelMode::More => ().into_any()`(桌面 `/more` 不可达,空次级菜单)。
  - 新增方法 `PanelMode::under_more(self) -> bool`(见 §6)。
- `nav_menu.rs`:`route_of` / `label_of` / `icon_of` 各加 `More` 臂(死臂,仅在 `current==More` 时被读,桌面不可达):
  - `route_of(More) = "/more"`
  - `label_of(More) = "More"`(字面量;与现有 phone tab 文案一致,不走 i18n)
  - `icon_of(More) = "•••" 三点 SVG body`

**否决的备选**:用 overlay 信号代替路由 → 零桌面改动,但脱离「everything is a route + back works」既有模式,且 ••• active 态须手工拼 `more_open || mode∈{…}`,更绕。

## §5 PhoneMore 菜单屏

**新文件**:`interfaces/webchat/src/platform/phone/more.rs`(单文件,单组件 `PhoneMore`)。1:1 套 `PhoneSettings` 着陆页(`platform/phone/settings/mod.rs`)结构:

- `PhoneShell title="More"`(无 `back`,它是着陆页 → 左对齐大标题,无返回箭头)。
- 一个 `.list`,3 行 `.cell`(顺序固定):

| 行 | `.cell-leading` 图标 | `.cell-title` | 点击 |
|----|------|------|------|
| Dashboard | grid(四宫格) | `"Dashboard"` | `navigate("/dashboard")` |
| Teams | people | `"Teams"` | `navigate("/teams")` |
| Extensions | puzzle | `"Extensions"` | `navigate("/extensions")` |

- 每行结构:`<div class="cell" on:click=…>` → `<span class="cell-leading">{icon}</span>` + `<div class="cell-body"><div class="cell-title">{label}</div></div>` + `<svg class="cell-chevron" …>`(复用 settings 的 chevron polyline)。
- 无 `.cell-value`(无状态副文本)。
- 导航用 `use_navigate()` + `NavigateOptions::default()`,每个 handler 各拿一份 clone(仿 settings `go`)。
- **零新 CSS**:全复用 `ios.css` 现有 `.list` / `.cell` / `.cell-leading` / `.cell-body` / `.cell-title` / `.cell-chevron`。
- **无** state struct / `screen_for_path` / 下钻逻辑——More 内只有一屏。

`platform/phone/mod.rs` 加 `pub mod more;`(按字母序)。

## §6 PhoneTabBar 第 5 格

`shell.rs` 的 `PhoneTabBar` 加第 5 个 `.tabitem`(••• 三点 SVG)→ `on:click` 导航 `/more`。

active 谓词**不同于**其它 4 格(它们是 `mode == X`):More tab 用 `mode.under_more()` —— iOS「More」tab 惯例,钻进 More 下属 section 时 More 保持高亮。

```rust
impl PanelMode {
    #[must_use]
    pub const fn under_more(self) -> bool {
        matches!(self, Self::More | Self::Dashboard | Self::Teams | Self::Extensions)
    }
}
```

`.tabitem { flex: 1 }`(ios.css)→ 5 格自动均分;390px 下每格 ~78px,容得下 23px 图标 + 10px 标签。

## §7 MainContent 接线

`app.rs`:
- `use crate::platform::phone::more::PhoneMore;`
- `MainContent` 加 `More` 臂(置于现有 Settings 臂之后或任意位置):

```rust
<div style:display=move || if mode.get() == PanelMode::More { "contents" } else { "none" }>
    {move || if form_factor.form_factor.get() == FormFactor::Phone {
        view! { <PhoneMore /> }.into_any()
    } else {
        ().into_any()  // 桌面 /more 不可达
    }}
</div>
```

桌面臂渲染空:`/more` 桌面永不可达,但若手动键入也不渲染手机菜单(防错位)。

## §8 变更清单

| 文件 | 改动 |
|------|------|
| `components/mode_sidebar.rs` | `PanelMode` +`More`;`from_path` +`/more`;`ModeSidebar` match +`More=>()`;+`under_more()` 方法;+`#[cfg(test)]` 测 `from_path`/`under_more` |
| `components/nav_menu.rs` | `route_of` / `label_of` / `icon_of` 各 +`More` 死臂 |
| `platform/phone/shell.rs` | `PhoneTabBar` +第 5 个 ••• tab |
| `app.rs` | +`use …more::PhoneMore;`;`MainContent` +`More` 臂 |
| `platform/phone/more.rs` | **新建** `PhoneMore` 组件 |
| `platform/phone/mod.rs` | +`pub mod more;` |

零 core / 零 IPC / 零依赖 / 零新 CSS。桌面功能字节级不变(仅新增穷尽性死臂)。R4(I/O-only:菜单行只导航)。

## §9 测试

**单测**(`mode_sidebar.rs` 的 `#[cfg(test)]`):
- `from_path("/more") == PanelMode::More`;`from_path("/more/x") == More`;且不误伤(`/memory` 仍 Memory 等)。
- `under_more` 真值表:More/Dashboard/Teams/Extensions → true;Chat/Memory/Agents/Settings → false。

**iOS-sim QA(权威运行时门,user-driven)**:按 [[feedback-ios-panel-test-via-full-macos-app]] 重编完整版 app 重嵌 dist → sim 连本地 core →
1. 底部出现 5 个 tab(Chat/Memory/Agents/Settings/More),布局不挤。
2. 点 ••• → 全屏 More 菜单,3 行(Dashboard/Teams/Extensions),无左右分屏。
3. 点任一行 → 导航到该 mode(此时仍显示桌面布局,符合过渡期约定)。
4. 在 `/dashboard`、`/teams`、`/extensions`、`/more` 时 ••• tab 保持高亮;在 Chat/Memory/Agents/Settings 时 ••• 不高亮、对应 tab 高亮。

## §10 成功标准

- [ ] 手机底部 5 tab,••• 为第 5 格。
- [ ] `/more` 渲染全屏 `PhoneMore` 菜单(无左右分屏),3 行导航正确。
- [ ] ••• active 态遵循 `under_more()`(Dashboard/Teams/Extensions/More 高亮)。
- [ ] 桌面功能字节级不变;`cargo`/`just wasm` 编译通过。
- [ ] 单测覆盖 `from_path("/more")` 与 `under_more()`。
- [ ] 三个目标屏仍为各自后续 spec(本 spec 不碰其内部)。

## §11 关联

续 [[feedback-phone-no-split-drilldown-law]]、[[project-aleph-panel-phone-agents-drilldown]]、[[project-aleph-panel-phone-memory-drilldown]]、[[reference-leptos-phoneshell-dynamic-child-footgun]]、[[feedback-ios-panel-test-via-full-macos-app]]。
