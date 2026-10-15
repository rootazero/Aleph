# Aleph Panel UI / Aleph 面板界面规范

## Identity / 品牌识别

- Canonical existing mark / 既有标志：`ℵ` + Aleph，渲染位置 `src/components/mode_sidebar.rs::SidebarBrand`；favicon 同源于 `index.html` 的内联 SVG。保留产品现有标志，不用虚构图形或第三方品牌素材替代。
- Product UI / 产品界面：`src/app.rs` + `src/components/mode_sidebar.rs` 构成实际 Panel，而非宣传页；不使用虚构数据或产品截图。
- Reference / 参考：用户提供的 macOS 系统设置、ChatGPT、Claude 截图用于布局节奏和信息层级；Gemini 作为简洁导航参考，不复制其标志、文案或配色。

## Design tokens / 设计令牌

- Color / 色彩：`styles/tailwind.css` 的现有 OKLCH 中性色层级，`--color-primary` 仅用于操作、聚焦与品牌；侧栏选中态取中性底色，保留用户选择的 accent palette。
- Typography / 排版：系统 UI 字体优先，代码使用现有等宽栈；字号在 `styles/tailwind.css` 的 `@theme` 块集中定义，所有 Tailwind 字号工具 (text-base / text-xl / text-nav / text-caption) 派生自此。实际生效的桌面层级（@1.0x text-scale）：
  - 页面标题 text-3xl = 24px（设置页 h1 等根级标题）
  - 区块标题 text-xl = 18px（SettingsSection 标题、表单分组标题 — form grouping tier）
  - 行标签 text-base = 16px（`SettingCard` 的 row label，font-medium；设置页每行的左侧标签；位于 14-16px 行标签 tier）
  - 次级标题 text-2xl = 21px（保留以备大块标题使用）
  - 导航/侧栏行 text-nav = 13px（nav-tile、侧栏按钮、搜索输入、副 UI）
  - 分组小标 text-caption = 11px（侧栏分组小标、徽标）
  - 正文 body = 0.8125rem (13px)，与 text-nav 同尺寸但保留 rem 名称以便基础字号缩放
  现有字号无障碍缩放 (`--control-ui-text-scale`) 继续生效，所有 rem 单位随之联动。
  Form grouping (text-xl, 18px) 与 row label (text-base, 16px) 的层级区分：分组标题描述「这一组控制什么」，行标签描述「这一行控制什么」；0.125rem（默认 2px）的差距把两者拉开，又不脱离 base-line。
- Spacing / 间距：桌面 `--spacing` 0.20rem × 用户密度选项；触屏仍为 0.22rem × 用户密度选项。间距以实际桌面 shell 几何为准，而不是 viewport：
  - 侧栏物理宽度固定 14.5rem (`w-[14.5rem]`，`src/components/mode_sidebar.rs`)；不随窗口宽度变化，避免拖动边缘时导航行重排
  - 侧栏导航行常态约 29px（`py-1.5` + 13px 文字 + 1.25 行高）
  - 设置页根级 margin `mb-6`，外层卡片分组 `space-y-4`，卡片标题与内容间距 `mb-3`
- Radius / 圆角：小半径区分按钮、分组和浮层；保留用户圆角缩放。边框用于面板分隔、输入与浮层，不用于给每个导航项画框。
- Elevation & motion / 层级与动效：导航无阴影高光；弹层保留轻阴影，交互以 120–180ms 颜色过渡为主，遵循 reduced-motion。

## Boundary / 边界

This is a visual-density pass, not a navigation or domain-flow rewrite. / 本次只调整视觉密度，不重写路由或业务流程。保留各模式入口、搜索、会话操作、主题/材质偏好及 macOS 标题栏安全区。
