# Phone Settings 屏 — 新 Session 启动 Prompt

> 复制下面整段到新会话即可开始。

---

## 任务

为 Aleph Panel 构建 **iPhone 版 Settings 屏(landing)**,**1:1 照搬**已有设计稿 `docs/design-system/aleph-mobile/screens/4-settings.png`(源码 `docs/design-system/aleph-mobile/screens/exported/Aleph Settings.dc.html`)。这是 iPhone 端重建的第一屏,做完构建出来给我 390px 目视对照确认,**先不碰其余屏**。

## 背景(必读,别重蹈覆辙)

- 项目:`/Volumes/TBU4/Workspace/Aleph`,单分支 `main`。Panel = crate `aleph-panel`,在 `interfaces/webchat/`,Leptos 0.7 + WASM + Tailwind v4。
- iPhone/iPad **只做纯壳 Panel 版(连远程 core,零 core 代码)**,所有工作只在 `interfaces/webchat/`。
- **此前的教训(务必避免)**:之前的移动适配是"把桌面 master-detail 双栏用 `max-sm:` 藏一栏"硬塞,**完全偏离了设计**、还把 iPhone 样式焊进桌面组件干扰桌面。那批工作已 `git reset` 回退、存档在分支 `archive/mobile-reflow-20260625`。**这次必须以设计稿为唯一规格,iOS 原生重建,不要 reflow 桌面。**
- 已完成的架构重组(HEAD `ce9afb6ca`):UI 按平台分层
  ```
  interfaces/webchat/src/
  ├── lib.rs            # pub mod platform; pub use platform::wide::views;(别名保旧路径)
  ├── app.rs            # 根 + 路由
  ├── api/ state/ context.rs i18n.rs components/ …   # 共享层(平台无关)
  ├── styles/tailwind.css   # 设计 token + 工具类(共享)
  └── platform/
      ├── wide/views/   # 桌面/浏览器屏幕(140 文件,勿动)
      ├── phone/        # ← iPhone 层,目前只有 mod.rs 注释,你在这里建屏
      └── tablet/       # iPad 层(以后)
  ```
- 架构方案全文:`docs/superpowers/specs/2026-06-25-panel-platform-architecture.md`;移动设计简报:`docs/superpowers/specs/2026-06-25-aleph-panel-ios-mobile-design-brief.md`。

## 设计规格(来自 `Aleph Settings.dc.html`,逐字对齐)

屏幕 = 顶栏 + 分组列表 + 底部 TabBar:

- **顶栏**:glass,左标题 `Settings`(20px / weight 700 / letter-spacing -0.02em)。
- **分组滚动区**:`gap: 20px; padding: 16px 16px 18px;`,纵向。
- **三组**(每组 = `.list-header` 标题 + `.list` 卡片容器 + 若干 `.cell`):
  - `连接`:Connection(图标 wifi 风;值 `remote · 10.10.10.4` 用 `.mono` 13px;`›`)
  - `AI`:Providers(值 `Anthropic`)、Embeddings(值 `text-embedding-3` mono 13px)、Model route(值 `Opus 4.8`)
  - `外观`:Theme(值 `System`)、Accent(**不是值+箭头,是 5 个 `.swatch` 圆色块**,第一个 `.swatch-active`,色见下)、Material(值 `Luxe`)
- **`.cell` 结构**:`<.cell-leading>(28px 圆角图标块,内嵌 17px svg)` + `<.cell-body><.cell-title>名称` + `<.cell-value>值`(可选,可加 `.mono`)+ `<svg.cell-chevron>(18px ›,points "9 6 15 12 9 18")`。
- **TabBar**:`.tabbar.glass`,4 个 `.tabitem`(Chat / Memory / Agents / Settings),当前 `Settings` 为 `.tabitem-active`。图标见 dc.html(23px svg)。
- Accent 5 色:`oklch(0.55 0.120 310)` Mauve(active)/ `oklch(0.55 0.130 250)` Ocean / `oklch(0.53 0.115 150)` Forest / `oklch(0.62 0.135 60)` Sunset / `oklch(0.57 0.150 15)` Rose。

> 设计稿里每行有图标 svg、文案、值——**直接从 `Aleph Settings.dc.html` 抄 svg 路径与文案**,不要自己造。

## 实现步骤

1. **移植 iOS 组件类**:把 `docs/design-system/aleph-mobile/screens/exported/styles/aleph.css` 里这些类**逐字复制**进新文件 `interfaces/webchat/styles/ios.css`,并在构建链里引入(检查 `index.html` / Tailwind 输入如何引样式;`ios.css` 仅 phone/tablet 用):
   `.list-header .list .cell .cell:last-child .cell-leading .cell-body .cell-title .cell-sub .cell-value .cell-chevron .tabbar .tabbar.glass .tabitem .tabitem-active .swatch .swatch-active`(`.glass` 已在 panel 存在)。
   **这些类引用的 token(`--color-surface-raised/-border-subtle/-primary-subtle/-primary/-text-tertiary/-surface-overlay`、`--radius-xl/-md`、`--shadow-sm`、`--safe-area-bottom`)在 panel 的 `tailwind.css` 已全部定义**——无需新增 token。
2. **表单因子检测 + 路由**:在 `state/viewport.rs`(已被回退删除,可从 archive 取参考:`git show archive/mobile-reflow-20260625:interfaces/webchat/src/state/viewport.rs`)实现 `FormFactor { Wide, Phone, Tablet }`(Phone = 视口宽 < 640px),挂到 context/RwSignal,随 resize 更新。在 `app.rs` 让 `/settings` 路由在 `Phone` 时渲染新的 `platform::phone::settings::PhoneSettings`,否则保持现有 `wide` 的 `Settings`。
3. **建屏**:`interfaces/webchat/src/platform/phone/settings.rs`(+ 在 `platform/phone/mod.rs` 声明 `pub mod settings;`)。用上面的 `.cell/.list/...` 类把 Settings landing 写出来。
   - **数据/路由**:cell 点击用现有路由跳转(`/settings/providers`、`/settings/embedding-providers`、`/settings/model-route`、`/settings/appearance` 等,见 `app.rs` 的 `SettingsRouter`)。值(Anthropic / Opus 4.8 / System / Luxe…)——**第一版可先用设计稿的静态占位值**对齐视觉,接真实 config 信号留作下一步(在交付时注明哪些是占位)。
   - 底部 TabBar:可从 archive 取 `mobile_tab_bar.rs` 参考,用 `.tabbar/.tabitem` 类重做(Chat/Memory/Agents/Settings,跳 `/`、`/dashboard/memory` 或 `/memory`、`/agents`、`/settings`——以现有路由为准)。
4. **构建 + 验证(守 cargo 节制,集中一次)**:
   - `cargo check -p aleph-panel --lib`(**至多一次**;若 rust-analyzer 报 `unlinked-file`/`E0583 views` 等,多半是移动后 RA 陈旧假错,**以 cargo 实编结果为准**)。
   - `just wasm` 重建 dist。
   - 确保有 `aleph-server` 在 `:18790` 运行;**该 dev daemon 从磁盘读 dist**,`just wasm` 后即生效,**无需重编 server**。验证:`curl -s http://127.0.0.1:18790/aleph_panel_bg.wasm | wc -c` == `wc -c < interfaces/webchat/dist/aleph_panel_bg.wasm`。
   - chrome-devtools MCP:`emulate viewport=390x844x3,mobile,touch` → 打开 `http://127.0.0.1:18790/settings` → 截图,**对照 `4-settings.png`**。
   - **桌面回归**:`emulate 1280x900x2` → `/settings` 仍是原桌面双栏(`platform/wide` 未受影响)。

## 硬约束

- **只改 `interfaces/webchat/`**,零 core `src/` 改动;**不碰 `platform/wide/`**(桌面/浏览器),iPhone 代码只在 `platform/phone/`。**零新依赖**。
- R2(UI 单源:不复制业务逻辑,只分表现层)、R4(Interface 纯 I/O)。
- **以设计稿为唯一视觉规格,iOS 原生重建,严禁 reflow 桌面 / 严禁 `max-sm:` 藏栏那套**。
- cargo 极度节制:不每步构建,编译+视觉验证批到最后一次。
- 回复用中文,代码注释英文;**提交仅在我明确要求时**,**不要 push**,提交无 attribution / 无 Co-Authored-By。
- 做完 Settings 一屏即**停下**给我对照,不要自动铺其余屏。

## 完成定义(DoD)

- 390px 下 `/settings` 视觉与 `4-settings.png` 一致(分组 inset 卡片 + primary 圆角图标块 + 标题/值/`›` + 底部 TabBar)。
- `cargo check` 通过;`just wasm` 绿;served wasm 字节 == 磁盘。
- 1280px 桌面 `/settings` 与改前一致(无回归)。
- 变更只落 `interfaces/webchat/`(新增 `platform/phone/settings.rs`、`styles/ios.css`、`state/viewport.rs`,改 `app.rs`/`platform/phone/mod.rs`/样式引入处),未提交、未 push,交付时列出占位项与下一步。
