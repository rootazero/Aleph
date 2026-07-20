# Aleph Panel — 平台分层文件夹架构

> 2026-06-25。背景:iPhone 移动适配此前走"往共享桌面组件塞 `max-sm:` 补丁"的路子,导致 ① 偏离 `docs/design-system/aleph-mobile` 六屏设计、② iPhone 样式焊死进桌面组件、干扰桌面/浏览器。已 `reset --hard` 回退到移动工作前(`9a56c3c94`),旧移动工作存档在分支 `archive/mobile-reflow-20260625`。

## 目标

- **平台隔离**:iPhone / iPad 的布局代码物理隔离,改动**永不**触碰桌面/浏览器 UI。
- **仅 Panel**:iPhone / iPad = 纯壳 Panel(连远程 core,**零 core 代码**),全部工作只在 `interfaces/webchat/`。
- **可扩 iPad**:后续只加 `tablet/`。
- **不重复业务逻辑**(不违 R2):数据/RPC、状态、设计 token、叶子组件全共享;只有**表现层(布局/导航/屏幕)**按设备分家。

## 三端关系

- **桌面版 == 浏览器版**:同一套宽屏 UI(`wide`);区别只在外壳(Tauri 完整版 vs 纯壳/浏览器),已由"连接由构建定"处理。
- **iPhone**(`phone`):iOS 原生,按六屏设计 1:1 重做。
- **iPad**(`tablet`):后续,复用 iOS 组件层 + 平板布局。

## 目标结构

```
interfaces/webchat/src/
├── app.rs              # 根 shell + 表单因子路由 → platform::{wide|phone|tablet}
├── lib.rs              # pub mod platform; + 共享 mod; re-export 别名保持现有路径
├── api/  api.rs        # 共享:RPC 客户端
├── state/              # 共享:响应式状态(含表单因子 viewport)
├── context.rs i18n.rs models.rs appearance.rs generation.rs
├── preset_data.rs preset_providers.rs panic_overlay.rs   # 共享
├── canvas_engine/      # 共享:WebGL(记忆星系)
├── components/         # 共享:叶子组件 + (暂留)桌面布局 chrome
├── styles/
│   ├── tailwind.css    # 设计 token + 工具类(共享)
│   └── ios.css         # iOS 组件类(.cell/.list/.tabbar…)仅 phone/tablet
└── platform/
    ├── mod.rs          # pub mod wide; pub mod phone; pub mod tablet;
    ├── wide/
    │   ├── mod.rs      # pub mod views;(宽屏入口)
    │   └── views/      # ← 现有 src/views/ 整体移入(140 文件)
    ├── phone/          # iPhone iOS 原生层(新)
    │   └── mod.rs
    └── tablet/         # iPad 层(后续)
        └── mod.rs
```

## 迁移手法(低风险 / cargo 节制)

物理移动 + **根部 re-export 别名**,避免 140 文件 import 大改写:

1. `git mv src/views src/platform/wide/views`(真实物理重组)。
2. `lib.rs`:删 `pub mod views;` → 加 `pub mod platform;` + `pub use platform::wide::views;`。
   该别名让所有现存 `crate::views::…` 引用**继续解析**(指向移动后的模块),零改动。
3. `views/` 内部 `super::…` 相对引用随整体移动**自动有效**;`crate::components/api/state::…` 不动。
4. 一次 `cargo check -p aleph-panel --lib` 验证。

后续可逐步把 `crate::views::` 收敛为规范路径 `crate::platform::wide::views::`(非必须,别名长期可留)。

## 表单因子路由(下一步)

`state/viewport.rs` 暴露 `FormFactor { Wide, Phone, Tablet }`(由视口宽度 + 触控判定)。`app.rs` 的 `MainContent` 按 `FormFactor` 选树:
- `Wide` → `platform::wide`(现有桌面屏幕)
- `Phone` → `platform::phone`(iOS 原生)
- `Tablet` → `platform::tablet`(后续)

iPhone 代码只活在 `platform::phone`,物理上够不到 `wide` → 结构性保证零干扰。

## phone 层重建(后续分屏推进)

以 `docs/design-system/aleph-mobile/screens/exported/*.dc.html` + `styles/aleph.css` 为唯一视觉规格,逐屏 1:1:Chat / Memory / Agents / Settings / Voice / 通知。iOS 组件类(`.cell`/`.list`/`.cell-leading`/`.tabbar`/`.swatch`…)从 `aleph.css` 移植进 `styles/ios.css`(token 已在 panel 全部存在)。共享数据 hooks 复用 `api/`、`state/`。

## 不做

- 不动桌面/浏览器 UI(`wide` = 回退后的干净桌面)。
- 不为 iPhone/iPad 做带 core 的完整版。
- 不在本阶段拆分 `components/`(叶子组件暂共享,phone 需要时再抽 iOS 专属布局组件)。
