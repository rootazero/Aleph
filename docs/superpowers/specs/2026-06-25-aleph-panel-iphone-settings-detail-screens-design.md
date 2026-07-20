# Aleph Panel — iPhone 原生 Settings 详情屏(drill-in)设计

- **日期**: 2026-06-25
- **承接**: iPhone Settings landing 已完成([2026-06-25-aleph-panel-iphone-settings-screen-design.md])。本设计把 landing 上 5 个 cell 能钻入的目标页**重建为原生 iOS 屏**(drill-in + 返回),并修掉 landing 的底部 TabBar 在真机被浏览器工具栏遮挡问题。
- **Crate**: `aleph-panel`(`interfaces/webchat/`),Leptos 0.7 + WASM + Tailwind v4。
- **基线**: landing 改动已在工作树(未提交):`styles/ios.css`、`tailwind.css`、`state/viewport.rs`、`state/mod.rs`、`platform/phone/settings.rs`、`platform/phone/mod.rs`、`app.rs` + 重建的 dist。本设计在其上继续。

---

## 1. 目标与非目标

### 目标
- 为 landing 能钻入的 **5 条 route** 各建一个**原生 iOS 详情屏**,drill-in 进入、顶部 `‹ Settings` 返回、保留底部 TabBar:
  - `/settings/network`(Connection)、`/settings/providers`(Providers)、`/settings/embedding-providers`(Embeddings)、`/settings/model-route`(Model route)、`/settings/appearance`(Appearance)。
- 修复 landing(及所有 iOS 屏)的**底部 TabBar 真机不可见**:`fixed inset-0` 底边在移动浏览器工具栏后面 → 改 `fixed inset-x-0 top-0 h-dvh`(动态视口高度)。
- **绝不复用桌面版表现**:每屏用 iOS 组件语言(`.list`/`.cell`/iOS 行控件)重建;只复用桌面页**同一套数据层**(`crate::api::*` / `appearance.rs`)。

### 非目标
- 不做其余 17 个手机 landing 不暴露的设置 route(它们在手机上无入口,不产生桌面混乱)。
- 重型两屏(Providers/Embeddings)**v1 聚焦**:列出已配置 + 改 key / 启用 / 设活跃;完整的新增供应商 / 选型号 / OAuth / 测试连接 CRUD **留后续轮**。
- Network 的**集群(Cluster)节点管理**留后续轮(节点 CRUD,重型);本轮 Connection 屏只做连接状态展示。
- 不碰 `platform/wide/`;不引新依赖;零 core 改动。

---

## 2. 架构红线对齐

- **R2(UI 唯一源)**: 屏在 Leptos Panel 实现。
- **R4(Interface 纯 I/O)**: 详情屏只做数据加载/保存(调既有 API)+ 路由跳转,不含业务推理。
- **复用数据不复用表现**: iOS 屏调用与桌面页**完全相同**的 API/状态:
  - Connection → 纯 `location.host` + loopback 判定(无 API)。
  - Model route → `RouteConfigApi::{get,update}`。
  - Appearance → `appearance.rs` `read_*` / `apply_*`(本地,无 API)。
  - Providers → `ProvidersApi::{list,set_default,update}`(focused v1)。
  - Embeddings → `EmbeddingProvidersApi::{list,set_active,update}`(focused v1)。
- **隔离**: 全部新代码在 `platform/phone/`;唯一进共享层的改动是 `app.rs` 的 `SettingsRouter` 增加 5 条 route 的 `FormFactor::Phone` 分支。

---

## 3. 导航与外壳

### Drill-in = 路由(沿用现有 route)
`SettingsRouter` 已对 `/settings` 按 `FormFactor::Phone` 分流。本设计对另外 5 条 route 各加同样的 Phone 分支:

```
"/settings/network"             => Phone ? <PhoneConnection/>  : <NetworkView/>
"/settings/providers"           => Phone ? <PhoneProviders/>   : <ProvidersView/>
"/settings/embedding-providers" => Phone ? <PhoneEmbeddings/>  : <EmbeddingProvidersView/>
"/settings/model-route"         => Phone ? <PhoneModelRoute/>  : <RouteView/>
"/settings/appearance"          => Phone ? <PhoneAppearance/>  : <AppearanceView/>
```
其余 route 与桌面分支**字节不变**。返回 = `use_navigate("/settings")`。

### 共享外壳 `PhoneShell`(新)
统一 landing 与详情屏的 chrome,并集中承载 dvh 修复:

```
#[component] PhoneShell(
    title: &'static str,
    back: Option<&'static str>,   // Some("/settings") = 详情屏显 ‹返回;None = landing 无返回
    children: Children,
) -> impl IntoView
```
渲染:
- 根 `fixed inset-x-0 top-0 h-dvh z-[70] flex flex-col` + 设计稿渐变背景(dvh 替代 inset-0:移动浏览器布局视口底在工具栏后,inset-0 的 bottom:0 把 TabBar 顶到工具栏后面)。
- glass 顶栏:`back=Some` 时左侧 `‹ Settings` 返回按钮(`use_navigate`),居中标题(20px/700/-0.02em);`back=None` 时左对齐标题(同 landing)。
- body:`flex:1 overflow-y-auto cc-hide-scroll`,承载 `children`。
- 底部 **`PhoneTabBar`**(从现 landing 抽出的共享组件):4 项 Chat/Memory/Agents/Settings,Settings `tabitem-active`,导航 `/ /memory /agents /settings`。详情屏与 landing 都显示(iOS 带 tab 应用标准:tab 内 drill-in 不消失)。

`PhoneSettings`(landing)重构为 `PhoneShell{back:None,title:"Settings"}` 包三组;详情屏为 `PhoneShell{back:Some("/settings"),title:<屏名>}` 包各自 iOS 内容。

### iOS 行控件(`ios.css` + phone 组件,按需增量)
- **选择行(单选)**: `.cell` + 选中态 checkmark(iOS 风,选中显 `--color-primary` 勾)。
- **开关行**: iOS toggle(纯 CSS switch)用于布尔。
- **内联值/输入行**: `.cell` 右侧 `.cell-value` 或内联 `<input>`。
- 三次法则:同一控件第 3 次出现再抽公共组件;否则就地写。

### 文件组织(高内聚)
```
platform/phone/
  mod.rs                 # pub mod shell; pub mod settings;
  shell.rs               # PhoneShell + PhoneTabBar(共享,dvh 修复在此)
  settings/
    mod.rs               # PhoneSettings(landing,重构用 PhoneShell)+ pub mod 子屏
    connection.rs        # PhoneConnection
    model_route.rs       # PhoneModelRoute
    appearance.rs        # PhoneAppearance
    providers.rs         # PhoneProviders(focused v1)
    embeddings.rs        # PhoneEmbeddings(focused v1)
```
现 `platform/phone/settings.rs` → 移为 `platform/phone/settings/mod.rs`(landing 内容不变,改用 PhoneShell)。

---

## 4. 5 屏内容

### 4.1 PhoneConnection(`/settings/network`,轻)
- 复用 connection.rs 的纯逻辑(`current_host` / `is_loopback_host`)。
- 一个 `.list`:连接目标(`host`,mono)+ local/remote 徽章 cell。只读。
- 集群节点管理 → 本轮不做(noted)。

### 4.2 PhoneModelRoute(`/settings/model-route`,轻-中)
- 复用 `RouteConfigApi::{get,update}`。
- 分组:① 模式(Auto/Always Local/Always Cloud)= 3 个**单选 cell**;② 负载均衡 = 选择行;③ 云升级 = **开关行**(仅 Always Local 时有意义);④ 偏好 provider(local/cloud)= 选择行;⑤ 限流 = 每 provider 的 rpm/tpm 内联输入行。保存:iOS 顶栏右侧"保存"或自动保存(实现期定,默认沿用桌面"Apply"语义按钮)。

### 4.3 PhoneAppearance(`/settings/appearance`,轻)
- 复用 `appearance.rs` `read_*` / `apply_*`(纯本地,即时生效)。
- 6 个分组选择行:Theme(ThemeMode)/ Accent(Accent,5 色 swatch 行,沿用 landing 的 swatch)/ Material / 字号(FontScale)/ 圆角(Roundness)/ 紧凑度(Density)。每项单选 cell 或分段。

### 4.4 PhoneProviders(`/settings/providers`,重 → focused v1)
- 复用 `ProvidersApi::{list,set_default,update}`。
- `.list` 列出已配置 provider(名 + 当前/默认徽章 + 启用态);每行点击进入**就地编辑**(改 API key = `provider_key_field` iOS 版 / 启用开关 / "设为默认" = `set_default`)。
- **不做** v1:新增 provider、catalog 选型号、OAuth、test_connection(留后续;noted)。

### 4.5 PhoneEmbeddings(`/settings/embedding-providers`,重 → focused v1)
- 复用 `EmbeddingProvidersApi::{list,set_active,update}`。
- 同 Providers 模式:列出已配置 + 改 key / 启用 / `set_active`。
- **不做** v1:add、presets、reembed、test(留后续;noted)。

---

## 5. 验证

- **首选 iOS Simulator(用户装 Xcode 27 beta 2 后)**: 当前 macOS 27.0 beta + Xcode 26.3/iOS 26.2 runtime **模拟器渲染损坏**(boot 后状态栏重影、内容全黑 → 图形栈不兼容),sim 审查须用户装 **Xcode 27 beta 2 + iOS 27 runtime**(多 GB,绑用户 Apple ID,Claude 无法代装)。装好后:`simctl boot` iPhone → Mobile Safari 开 `http://localhost:18790/<route>`(sim 共享 Mac localhost)→ `simctl io screenshot` 逐屏审查(能准确验 dvh / 安全区 / 底部 tab)。
- **构建不依赖 sim**:design→plan→build 并行推进;最终视觉验证走 sim。
- **过渡期(sim 未就绪)**: Chrome 模拟器审布局 + `evaluate_script` 量 TabBar 是否在视口内(验 dvh 逻辑) + 用户真机 Chrome 抽查。
- 每条 route 桌面 1280px 回归不变(Phone 分支只在 <640px 生效)。
- 单次 cargo 门:`just wasm` 内含 wasm 编译(= 编译门);需重 embed server 才能 sim/真机看新 dist(rust_embed 编译期嵌入)。

---

## 6. 分期(plan 据此切 task)

- **Phase 0 — 外壳与修复**: 抽 `PhoneShell` + `PhoneTabBar`;landing(`settings.rs`→`settings/mod.rs`)改用 PhoneShell;dvh 修复落在 PhoneShell;`SettingsRouter` 加 5 条 route 的 Phone 分支(先指向占位/逐步填)。
- **Phase 1 — 3 轻屏**: PhoneConnection、PhoneAppearance、PhoneModelRoute。
- **Phase 2 — 2 重屏(focused)**: PhoneProviders、PhoneEmbeddings。
- 每 Phase 末可在 sim(就绪后)审查。

---

## 7. v1 已知局限(交付注明)
1. Providers/Embeddings 仅 focused(列出 + 改 key/启用/活跃);完整 CRUD/选型号/OAuth/测试留后续轮。
2. Network 集群节点管理留后续轮(Connection 屏只读连接状态)。
3. sim 视觉验证须用户先装 Xcode 27 beta 2(当前 macOS 27 beta 下旧 sim 渲染损坏)。
4. 字体走系统回退(与现网 panel 一致)。

---

## 8. 完成定义(DoD)
- 5 条 route 在 <640px 渲染原生 iOS 详情屏(`‹ Settings` 返回 + 保留 TabBar + iOS 列表/控件),数据来自既有 API/`appearance.rs`,**无桌面左右分栏**。
- landing 及所有 iOS 屏底部 TabBar 在真机/ sim 可见(dvh 修复)。
- `cargo`(`just wasm`)编译通过;桌面 1280px 各 route 回归不变;`platform/wide/` 未碰;零新依赖。
- 变更只落 `interfaces/webchat/`;未提交、未 push;交付列出 §7 局限与下一步。
