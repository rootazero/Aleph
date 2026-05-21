# OKLCH Design System Migration — Aleph Dashboard

> 从"数字发光"到"自然触感"：将 Aleph Dashboard 从 Slate+Indigo 高饱和度体系迁移至 OKLCH 静奢风 (Quiet Luxury) 设计系统。

**Date**: 2026-02-22
**Status**: Approved
**Scope**: Control Plane UI (`core/ui/control_plane/`)

---

## 1. 核心设计哲学

### 1.1 审美转向

| 维度 | Before | After |
|------|--------|-------|
| 色彩空间 | RGB/HEX | OKLCH |
| 基调 | Slate (冷灰) + Indigo (高饱和蓝紫) | Mist (冷雾) + Mauve (低饱和紫) |
| 视觉效果 | Glass-morphism (毛玻璃 + 辉光) | 纯扁平 (实色 + 细边框) |
| 层级区分 | 阴影 + 透明度 | OKLCH luminance 微差 |
| 模式 | 纯暗色 | 亮色 + 暗色双模式 |

### 1.2 色彩角色分配

- **Mist** — 呼吸感。冷中性底色，负责空间和层级
- **Mauve** — 深度。品牌色和行动点，低饱和紫的克制感
- **Olive** — 平衡。替代 emerald 的自然成功色
- **Taupe** — 温度。大地色调中和 mist 的冷感

---

## 2. 技术架构：方案 A — CSS 变量抽象层

### 2.1 Tailwind v3 → v4.2 升级

| 项目 | v3 | v4.2 |
|------|-----|------|
| 配置文件 | `tailwind.config.js` (JS) | `styles/tailwind.css` 内 `@theme` (CSS) |
| 入口指令 | `@tailwind base/components/utilities` | `@import "tailwindcss"` |
| 构建工具 | `tailwindcss` CLI | `@tailwindcss/cli` |
| 包名 | `tailwindcss@3.4.19` | `tailwindcss@4.2` + `@tailwindcss/cli` |
| 颜色系统 | RGB/HEX | OKLCH 原生 |
| 新色系 | 无 | mauve, olive, mist, taupe 内置 |
| content 扫描 | 显式配置 | 自动检测 + `@source` 覆盖 |

### 2.2 package.json 变更

```json
{
  "devDependencies": {
    "tailwindcss": "^4.2",
    "@tailwindcss/cli": "^4.2"
  },
  "scripts": {
    "build:css": "npx @tailwindcss/cli -i styles/tailwind.css -o dist/tailwind.css --minify"
  }
}
```

删除 `tailwind.config.js`。

---

## 3. 语义化 Token 体系

### 3.1 亮色模式（默认）

```css
@import "tailwindcss";
@source "../src/**/*.rs";
@source "../dist/**/*.html";

@theme {
  /* === Surface 层级 === */
  --color-surface:          oklch(0.97 0.005 220);   /* mist-50   — 主背景 */
  --color-surface-raised:   oklch(1.00 0.000 0);     /* white     — 卡片/容器 */
  --color-surface-sunken:   oklch(0.94 0.008 220);   /* mist-100  — 凹陷区域 */
  --color-surface-overlay:  oklch(0.98 0.004 220);   /* mist-75   — 弹窗覆盖 */

  /* === 文字层级 === */
  --color-text-primary:     oklch(0.20 0.015 310);   /* mauve-950 — 主文字 */
  --color-text-secondary:   oklch(0.45 0.010 220);   /* mist-600  — 次要文字 */
  --color-text-tertiary:    oklch(0.60 0.008 220);   /* mist-400  — 辅助文字 */
  --color-text-inverse:     oklch(0.97 0.005 220);   /* mist-50   — 反色文字 */

  /* === 边框 === */
  --color-border:           oklch(0.88 0.008 220);   /* mist-200  — 主边框 */
  --color-border-subtle:    oklch(0.92 0.006 220);   /* mist-150  — 微弱边框 */
  --color-border-strong:    oklch(0.78 0.010 220);   /* mist-300  — 强调边框 */

  /* === 品牌/主色 (Mauve) === */
  --color-primary:          oklch(0.55 0.120 310);   /* mauve-600 — CTA/链接 */
  --color-primary-hover:    oklch(0.50 0.110 310);   /* mauve-700 — 悬浮 */
  --color-primary-subtle:   oklch(0.95 0.020 310);   /* mauve-50  — 浅底 */

  /* === 成功 (Olive) === */
  --color-success:          oklch(0.55 0.120 130);   /* olive-600 */
  --color-success-subtle:   oklch(0.95 0.025 130);   /* olive-50 */

  /* === 警告 (Taupe) === */
  --color-warning:          oklch(0.60 0.080 70);    /* taupe-500 */
  --color-warning-subtle:   oklch(0.95 0.015 70);    /* taupe-50 */

  /* === 危险 (柔和红) === */
  --color-danger:           oklch(0.55 0.150 25);
  --color-danger-subtle:    oklch(0.95 0.020 25);

  /* === 信息 (Mist 深色) === */
  --color-info:             oklch(0.50 0.030 220);   /* mist-700 */
  --color-info-subtle:      oklch(0.95 0.010 220);   /* mist-50 */

  /* === 图表专用色盘 === */
  --color-chart-1:          oklch(0.55 0.120 310);   /* mauve — 主系列 */
  --color-chart-2:          oklch(0.58 0.120 130);   /* olive — 对比系列 */
  --color-chart-3:          oklch(0.60 0.080 70);    /* taupe — 辅助系列 */
  --color-chart-4:          oklch(0.50 0.030 220);   /* mist — 基准线 */
}
```

### 3.2 暗色模式覆盖

```css
.dark {
  --color-surface:          oklch(0.15 0.020 310);   /* mauve-950 */
  --color-surface-raised:   oklch(0.20 0.018 310);   /* mauve-900 */
  --color-surface-sunken:   oklch(0.12 0.015 310);
  --color-surface-overlay:  oklch(0.18 0.020 310);

  --color-text-primary:     oklch(0.97 0.005 220);   /* mist-50 */
  --color-text-secondary:   oklch(0.65 0.008 220);   /* mist-400 */
  --color-text-tertiary:    oklch(0.50 0.006 220);   /* mist-500 */

  --color-border:           oklch(0.28 0.020 310);   /* mauve-800 */
  --color-border-subtle:    oklch(0.22 0.018 310);
  --color-border-strong:    oklch(0.35 0.022 310);   /* mauve-700 */

  --color-primary:          oklch(0.65 0.120 310);   /* mauve-400 提亮 */
  --color-primary-hover:    oklch(0.70 0.110 310);
  --color-primary-subtle:   oklch(0.20 0.040 310);

  --color-success:          oklch(0.65 0.120 130);   /* olive-400 */
  --color-success-subtle:   oklch(0.20 0.030 130);

  --color-warning:          oklch(0.70 0.080 70);    /* taupe-400 */
  --color-warning-subtle:   oklch(0.20 0.020 70);

  --color-danger:           oklch(0.65 0.150 25);
  --color-danger-subtle:    oklch(0.20 0.030 25);

  --color-info:             oklch(0.65 0.030 220);
  --color-info-subtle:      oklch(0.20 0.015 220);
}

/* 跟随系统偏好 */
@media (prefers-color-scheme: dark) {
  :root:not(.light) {
    /* 同 .dark 变量 */
  }
}
```

### 3.3 全局过渡

```css
:root {
  transition: background-color 0.2s ease, color 0.2s ease, border-color 0.2s ease;
}
```

---

## 4. 组件级设计规范

### 4.1 Sidebar

| 元素 | 样式 |
|------|------|
| 容器 | `bg-surface-raised border-r border-border` |
| 导航项 | `text-text-secondary hover:bg-surface-sunken hover:text-text-primary` |
| 激活项 | `bg-primary-subtle text-primary` |
| Logo 徽标 | `bg-primary text-text-inverse` 或 `from-primary to-primary-hover` 微渐变 |
| AlertLevel Info | `bg-info` |
| AlertLevel Warning | `bg-warning` |
| AlertLevel Critical | `bg-danger animate-pulse` |

### 4.2 Button

| 变体 | 样式 |
|------|------|
| Primary | `bg-primary text-text-inverse hover:bg-primary-hover active:scale-[0.98]` |
| Secondary | `bg-surface-sunken text-text-primary border border-border hover:bg-surface-raised` |
| Ghost | `bg-transparent text-text-secondary hover:bg-surface-sunken hover:text-text-primary` |
| Destructive | `bg-danger text-text-inverse hover:brightness-95 active:scale-[0.98]` |

去除: ~~shadow-*~~, ~~backdrop-blur~~

### 4.3 Card

```
Before: bg-slate-900/40 border border-slate-800 rounded-3xl backdrop-blur-sm shadow-glass
After:  bg-surface-raised border border-border rounded-2xl
```

- 悬浮态: `hover:bg-surface-sunken`
- 去除: ~~backdrop-blur-sm~~, ~~shadow-glass~~
- 圆角收敛: 3xl → 2xl

### 4.4 Badge

统一模式: `bg-{semantic}-subtle text-{semantic} border border-{semantic}/20`

| 语义 | token |
|------|-------|
| Primary | primary / primary-subtle |
| Success | success / success-subtle |
| Warning | warning / warning-subtle |
| Danger | danger / danger-subtle |
| Neutral | `bg-surface-sunken text-text-secondary border border-border` |

### 4.5 Forms

| 组件 | 样式 |
|------|------|
| TextInput | `bg-surface-raised border border-border text-text-primary focus:ring-2 focus:ring-primary/30 focus:border-primary` |
| SelectInput | 同 TextInput |
| SwitchInput (off) | `bg-border` |
| SwitchInput (on) | `bg-primary` |
| NumberInput slider | `bg-border accent-primary` |
| ErrorMessage | `bg-danger-subtle border border-danger/30 text-danger` |
| SuccessMessage | `bg-success-subtle border border-success/30 text-success` |
| SettingsSection | `bg-surface-raised border border-border rounded-xl p-6` |

### 4.6 ConnectionStatus

- Connected: `bg-success` (无辉光)
- Disconnected: `bg-warning` (无辉光)

### 4.7 全局去除清单

| 去除 | 替代 |
|------|------|
| `backdrop-blur-xl` / `backdrop-blur-sm` | 实色背景 |
| `shadow-glass` / `shadow-lg` / `shadow-xl` | 无阴影或 `border` |
| `bg-xxx-500/5` ~ `/50` 半透明 | 实色 `bg-surface-*` |
| `bg-xxx-500/10 blur-[120px]` 背景辉光 | 完全去除 |

---

## 5. 视图层规范

### 5.1 App Root

```
Before: body class="bg-gray-900 text-gray-100" / app class="bg-slate-950 text-slate-50"
After:  body class="bg-surface text-text-primary" / app class="bg-surface text-text-primary min-h-screen"
```

### 5.2 Home Dashboard

- StatCard: 统一 `bg-surface-raised border border-border`，图标通过语义 token 区分颜色
- 背景辉光球: 完全去除

### 5.3 主题切换

- 机制: CSS class (`dark`/`light` on `<html>`)
- 三档: System / Light / Dark
- 持久化: `localStorage`
- Leptos: `wasm_bindgen` 操作 `document.documentElement.classList`
- 切换动画: 全局 `transition: background-color 0.2s ease, color 0.2s ease, border-color 0.2s ease`

### 5.4 渐变策略

| 场景 | 渐变 |
|------|------|
| Logo 徽标 | `from-primary to-primary-hover` (mauve 内 5° 色相差) |
| 顶部装饰线 | `from-surface-sunken via-surface to-surface-sunken` (luminance 微变) |
| 其他 | 不使用渐变 |

---

## 6. 文件变更清单

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `package.json` | 修改 | 升级 tailwindcss@4.2 + @tailwindcss/cli |
| `tailwind.config.js` | **删除** | 配置迁入 CSS |
| `styles/tailwind.css` | **重写** | @import + @source + @theme + 暗色覆盖 |
| `index.html` | 修改 | body class 改为 token |
| `src/lib.rs` | 修改 | 主题初始化 (读 localStorage 设 dark class) |
| `src/app.rs` | 修改 | 根容器 class 改为 token |
| `src/components/ui/button.rs` | 修改 | 四个变体改为 token |
| `src/components/ui/card.rs` | 修改 | 去 blur/shadow，改为 token |
| `src/components/ui/badge.rs` | 修改 | 五个变体改为 token |
| `src/components/ui/tooltip.rs` | 修改 | 颜色改为 token |
| `src/components/sidebar/sidebar.rs` | 修改 | 背景、边框、logo 改为 token |
| `src/components/sidebar/sidebar_item.rs` | 修改 | hover/active 改为 token |
| `src/components/forms.rs` | 修改 | 8 个表单组件改为 token |
| `src/components/connection_status.rs` | 修改 | 去辉光，改为 token |
| `src/components/layouts/settings_layout.rs` | 修改 | 容器颜色改为 token |
| `src/views/home.rs` | 修改 | StatCard + 去辉光 + token |
| `src/views/system_status.rs` | 修改 | 颜色改为 token |
| `src/views/agent_trace.rs` | 修改 | 颜色改为 token |
| `src/views/memory.rs` | 修改 | 颜色改为 token |
| `src/views/settings/*.rs` | 修改 | 设置页颜色改为 token |

**总计**: ~20 个文件修改, 1 个文件删除, 0 个新增文件

### 不改动

- `Cargo.toml` — Rust 依赖不变，`tailwind_fuse` 保留
- `src/api/` — 纯逻辑层
- `src/context/` — 无颜色相关
- `src/models.rs` — 数据模型
- `dist/` — 构建产物，自动重新生成
