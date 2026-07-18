# 侧栏菜单项玻璃化视觉刷新 — 设计

**日期**: 2026-06-11
**范围**: Panel 左栏（`.aleph-sidebar`）内所有可选菜单项的选中态/悬停态视觉材质
**前序**: 玻璃语言谱系第 4 轮。前三轮已完成 chrome（modal/menu/sidebar 容器）+ 聊天消息流（`.msg-glass`/`.glass-inset`）。本轮收口最后一处断档：左栏菜单**项**。

## 背景与问题

左栏**容器** `.aleph-sidebar` 早已玻璃化——`::before` 真磨砂层（`--glass-blur-chrome`）、主色渐变、sheen + border 内阴影，与 chrome 一致。

但容器内的**菜单项**（用户实际点击的导航行）仍是扁平实填：
- 选中态 = 实色块 `bg-sidebar-active` + 一条 0.5px accent 左光条
- 悬停态 = `hover:bg-sidebar-active/50`
- 无描边、无高光、无深度、无玻璃材质

观感断档：玻璃外壳包裹一排扁平色块，与精修过的 `.msg-glass`/`.glass-inset` 材质语言脱节。

更糟的是存在**两套不一致的选中词汇**：
- 多数行用 `bg-sidebar-active text-sidebar-accent`（Dashboard/Settings/Agents/Teams/NavMenu 触发器）
- 对话历史会话行与 NavMenu 弹窗却用 `bg-primary/10 text-primary`

## 视觉决策

经浏览器 mockup（A/B/C 三方向并排，真实 `.aleph-sidebar` + 真实 token 渲染）选定 **方案 A：磨砂内嵌瓷砖（frosted inset tile）**。

- A（选定）= accent 着色半透明瓷砖 + 内描边 + 顶部高光，"沉入"质感，材质最贴近 `.glass-inset`
- B = 主色玻璃实填药丸 + 投影，"浮起"，太抢眼会盖过主内容区 —— 否决
- C = accent 光条 + 极淡着色，偏回归现状扁平 —— 否决

**核心原则**：仿玻璃、零 backdrop-filter（与消息气泡同决策——菜单项多、随路由频繁切换，不付模糊合成成本；瓷砖坐在 `.aleph-sidebar` 已有真磨砂层之上）。材质层级：玻璃外壳 → 内嵌瓷砖。

### 显著性调校（本轮精修，2026-06-11）

前序三轮玻璃刷新整体反馈"效果不明显"。本轮在保持方案 A 身份（内嵌、不浮起）的前提下，把选中态显著性上调，确保"一眼可辨"，避免再次落入"太淡看不出"。三个杠杆：

1. **着色 16% → 22%** — 16% 是"效果不明显"会复发的临界点；22% 清晰可辨但仍半透（非实色块）。
2. **描边 26% → 38%** — 清晰的 accent 描边是 **light 主题**下瓷砖能成立的关键（淡着色在亮底易被冲淡，是上轮 spec 自标的薄弱点）。这是不靠投影/"浮起"就达到显著性的真正杠杆。
3. **不加外投影** — 刻意只保留 inset 高光。用户在"更明显"与"浮起"之间选了前者：显著性全部来自 着色 + 描边 + 顶部高光，绝不靠浮起。

## 材质规格（CSS）

新增两个 `@layer components` 组件类（`styles/tailwind.css`），与 `.msg-glass`/`.glass-inset` 同源：

```css
.nav-tile {
  color: var(--color-text-secondary);
  border: 1px solid transparent;          /* 占位:与选中态等高,切换零布局位移 */
  transition: background .15s, color .15s, box-shadow .15s, border-color .15s;
}
.nav-tile:hover {
  background-color: color-mix(in oklch, var(--color-text-primary) 8%, transparent);
  color: var(--color-text-primary);
}

.nav-tile-active {
  color: var(--color-sidebar-accent);
  background-color: color-mix(in oklch, var(--color-sidebar-accent) 22%, transparent);  /* 显著性:16%→22% */
  border: 1px solid color-mix(in oklch, var(--color-sidebar-accent) 38%, transparent);  /* 清晰描边:26%→38% */
  box-shadow: inset 0 1px 0 oklch(1 0 0 / 0.10);   /* 顶部高光:玻璃感来源,0.07→0.10 */
  font-weight: 600;
}
```

**关键性质**：

1. **单一定义跨全主题** — 全部基于自适应 token（`--color-sidebar-accent`、`--color-text-primary`）做 `color-mix`。light/dark/glass 三态 + ocean/forest/sunset/rose 四 accent 色板**自动跟随**，无需像 `.msg-glass` 那样写 4 套（此处无不透明底色，只有 token 派生着色）。
2. **零布局位移** — 静息态预置 1px 透明边框，选中时仅边框变色，盒模型不变，行与行严格对齐。
3. **无障碍回退** — 复用现有 `prefers-reduced-transparency` 块，追加 `.nav-tile-active` 不透明回退：实底 `var(--color-sidebar-active)` + 去除 `box-shadow` 高光。与 `.aleph-sidebar`/`.msg-glass` 回退策略一致。
4. **职责分离** — 布局类（`flex items-center gap-3 px-3 py-2 rounded-lg text-sm`）留在调用点，组件类只管材质。`rounded-lg` 已是 `--radius-lg` token，随无障碍缩放。

## 落地清单（6 个调用点）

把两套不一致的选中/悬停串统一替换为 `.nav-tile` / `.nav-tile-active`，布局类保留：

| # | 文件 | 当前选中词汇 | 动作 |
|---|------|------------|------|
| 1 | `components/sidebar/sidebar_item.rs` | `bg-sidebar-active text-sidebar-accent` + 绝对定位左光条 | 归一到瓷砖；**删除 0.5px 左光条**（瓷砖已标识选中，留着是双重指示） |
| 2 | `components/mode_sidebar.rs`（SettingsSidebar tabs） | `bg-sidebar-active text-sidebar-accent` | 归一 |
| 3 | `components/agents_sidebar.rs`（智能体列表） | `bg-sidebar-active text-sidebar-accent` | 归一 |
| 4 | `components/chat_sidebar.rs`（对话历史会话行） | `bg-primary/10 text-primary` ← 不一致 | 归一到瓷砖 |
| 5 | `components/nav_menu.rs` | 触发器 `bg-sidebar-active` + 弹窗 `bg-primary/12 text-primary` ← 不一致 | 触发器 open 态 + 弹窗选中项均归一；弹窗本身 `.glass` 真磨砂，瓷砖坐其上 |
| 6 | `views/teams/mod.rs`（TeamsSidebar） | `bg-sidebar-active text-sidebar-accent` | 归一 |

## 明确排除（不在"左栏菜单栏"范围）

- `components/command_palette.rs` — 命令面板浮层，瞬时 `.glass` 表面，仅 hover 无选中态，另一套交互
- `views/agents/files.rs`、`views/teams/replay.rs` — 右侧**内容区**内的文件树/回放时间线，非 section 左导航
- `MemorySidebar`（mode_sidebar.rs 内）— 表单（下拉/搜索/滑块），无 nav 选中行；容器玻璃已继承

## 验证

1. **构建**：`cargo build --target wasm32-unknown-unknown`（强制——上轮教训：native `cargo check` 会漏掉 `#[cfg(target_arch="wasm32")]` 门控代码的编译错）→ 重建 dist 包（wasm-bindgen + wasm-opt，绝对 target 路径，规避 worktree 共享 target-dir 雷）→ 重编 `aleph-server` binary（`rust_embed` 烧入新 dist）→ 替换运行中 .app binary，supervisor 重拉。
2. **视觉核验**（不污染对话历史）：`aleph-server bootstrap-url` 认证 → chrome-devtools 注入 `position:fixed` 覆盖层，用**真实 `.aleph-sidebar` + 真实 token** 渲染六处侧栏选中/悬停态 → 切 **dark/glass/light 三主题**（+ 抽查 accent 色板）截图。
3. **重点核对**：行间对齐无位移；light 主题下 22% accent 着色 + 38% 描边**一眼可辨**（本轮显著性目标，重点验亮底）但不刺眼；选中态明显强于现状中性灰块；reduced-transparency 回退确为不透明。

## 改动量

- 1 个 CSS 段（约 25 行，含 `prefers-reduced-transparency` 回退）
- 6 文件各 1–2 处字符串替换
- `sidebar_item.rs` 删 3 行左光条
- 纯视觉，无逻辑改动

## 成功标准

- 六处左栏菜单项选中/悬停态统一为磨砂内嵌瓷砖材质，两套旧词汇消除（Chat/NavMenu 的 accent-purple 离群消失）
- 选中态**显著性达标**：着色 22% + 描边 38% + 高光，明显强于现状中性灰块，三主题下一眼可辨——修正前序"效果不明显"
- dark/glass/light 三主题 + 四 accent 色板下瓷砖均成立、行对齐、无刺眼
- 整体与 chrome / 消息流玻璃语言连贯，扁平断档消除
- 349+ panel 测试通过、clippy 干净、wasm 构建通过
