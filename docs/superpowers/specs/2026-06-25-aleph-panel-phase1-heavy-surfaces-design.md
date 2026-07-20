# Aleph Panel 移动端重型界面适配 (Phase 1) — 设计 / Mobile Heavy-Surface Adaptation

> 状态: Design (待用户 review → writing-plans)
> 日期: 2026-06-25
> 前置: Phase 0.5「移动响应式 Panel 骨架」已 DONE+merged(`cb9aa5ce2` / `4986e0978` / `e68e7c334`)。
> 关联记忆: `project-aleph-ios-panel-design-system`。
> 守则: R2(UI 唯一源, 纯 Leptos/WASM)/ R3(核心轻量化, 最大化复用)/ KISS / 外科改动。纯 WASM/CSS, 与 iOS 工具链解耦, 无需 Mac。

---

## 0. 背景与目标

Phase 0.5 落地了手机端骨架:`ViewportState{is_mobile, drawer_open}`(`state/viewport.rs`, 断点 `MOBILE_BREAKPOINT_PX = 640.0`)、底部 4-tab 栏(`mobile_tab_bar.rs`, Chat/Memory/Agents/Settings)、Chat 顶部 agent pill→抽屉、通知全屏 sheet、Memory 手机默认列表。

Phase 1 把**剩余重型界面**适配到手机,使一部完整 IA(7 个 PanelMode + 21 项设置 + Teams 5 子视图 + 星系画布)在 `<640px` **全部可达,且在手机范围内可用**(Teams 仅读 / Canvas 列表为主 / DAG 分层列表无边可视化——非桌面完全等价)。本 spec 是一份**伞状设计**,拆四个工作流,按依赖排序:**① 导航外壳 → ② Settings → ③ Teams → ④ Canvas**。

**成功判据**:在 390px 浏览器(及后续 iOS 壳)下,
- 从任意 tab 经顶栏 hamburger→抽屉能进入 Teams / Dashboard(及其子页)/ Extensions / Settings;
- Settings 落地为 **21 项分组列表**(6 组;Channels 为 1 项 → 其 overview → Telegram/Discord/WhatsApp/IMessage 子路由),各页单列无横向溢出;
- Teams 5 子视图可读(只读场景:Kanban 单列筛选 / DAG 分层列表 / 其余 reflow);
- 星系画布可双指缩放 + 点节点出底部 sheet + 不支持时回退列表。
- **范围说明**:Voice 是 composer 麦克风启动的 overlay(非 PanelMode,chat 内已可用,不在导航外壳范围);Cron 是 `/dashboard/cron` 子页,随 Dashboard 可达,不另设单独入口。

---

## 1. 已定决策

| # | 决策点 | 选定 | 理由 |
|---|--------|------|------|
| D1 | 手机如何到达完整 IA | **通用顶栏 + 抽屉**(每 tab 顶部 hamburger 复用现有 `ModeSidebar` 抽屉) | 零新增导航体系, 一举打通非-chat tab 入口 + 设置子导航, 复用最大 |
| D2 | Settings 深度 | **分组列表 landing + 1 列表单**(iOS Settings.app 风) | `SettingsTab`/`SETTINGS_GROUPS` 元数据已齐, 零新数据驱动列表 |
| D3 | Teams 深度 | **只读为主 MVP**(Overview/Replay/Workers + Kanban 单列筛选 + DAG 只读分层列表) | 先看后编; Kanban/DAG 天生宽, 编辑留桌面/Phase 2 |
| D4 | Canvas | **列表为主 + 可选触控画布**(切换已存在; 补缩放/sheet/回退) | 单指交互 + 切换已现成, 增量最小 |
| D3b | Kanban 手机布局 | **单列 + 状态筛选下拉**(放弃 2 列分组) | 窄屏认知负担最小;2 列分组留 Phase 2 |

底部 tab 保持 4 个不变(Chat/Memory/Agents/Settings);二级模式(Teams/Dashboard/Extensions)经抽屉进入。

---

## 2. 工作流 ① — 导航外壳 (MobileTopBar + 通用抽屉入口 + 铃铛归位)

**地基**:gate 其余三块——Teams 可达性、设置子页返回、铃铛一致性都依赖它。

### 2.1 目标
- 抽象一个可复用的 `MobileTopBar`(三槽:左 hamburger|drawer-trigger / 中 title|agent-pill / 右 bell|action),挂到每个 tab 顶部。
- 所有非-chat tab 获得 hamburger → 打开现有 `ModeSidebar` 抽屉(已含 7 个 PanelMode + 设置子导航 + 会话),解决 Phase 1 待办 (a)。
- **可达性范围**:抽屉的 7 个 PanelMode 覆盖 Chat/Dashboard/Memory/Agents/Teams/Extensions/Settings。Voice 是 composer 麦克风 overlay(chat 内已可用),Cron 在 Dashboard 内部导航(`/dashboard/cron`)——二者不靠 hamburger 单独入口(若 Dashboard 侧栏未列 Cron,作 Phase 2 小补)。
- 浮动通知铃铛收进顶栏右槽,跨 tab 一致(待办 b)。
- 二级页(设置单页 / Teams)右上由铃铛改为 **‹ 返回** 语义(导航栈)。

### 2.2 复用(grounded)
- `ViewportState.is_mobile` / `.drawer_open`(`state/viewport.rs:16-54`);resize 离开移动宽度自动关抽屉(`:35-48`)。
- `ModeSidebar` 抽屉(`components/mode_sidebar.rs:77-84`, translate 由 `drawer_open` 驱动;`:71-74` pathname Effect 路由变更自动关;宽 `min(20rem,82vw)` `:80`);7 个 `PanelMode` + `PanelMode::from_path()`(`:38-54`)。
- `MobileTabBar`(`components/mobile_tab_bar.rs:30-68`);`route_of/label_of/icon_of`(`components/nav_menu.rs`)。
- `NotificationCenter`(`components/notification_center.rs`):铃铛 `:68-93`、全屏 sheet `:104-110`、返回箭头 `:120-131`;`NotificationsState` 在 shell root 提供(`app.rs:99`),组件挂在 Router 外(`app.rs:264`)。
- Phase 0.5 Chat pill 顶栏(`views/chat/view.rs:239-260`)= 抽取蓝本。

### 2.3 改动(reflow targets)
| 文件:行 | 现状 | 改为 |
|---------|------|------|
| **新建 `components/mobile_top_bar.rs`** | — | `MobileTopBar` 组件, 三 slot(left/center/right), `max-sm:flex items-center justify-between px-3 pt-[calc(var(--safe-area-top)+0.5rem)] pb-2`, 桌面 `max-sm:hidden` |
| `views/chat/view.rs:239-260` | Chat 内联 pill 顶栏 | 改用 `MobileTopBar`:center = `mobile_agent` memo, left 空(pill 即 drawer trigger 或显式 hamburger), right = bell |
| `components/mode_sidebar.rs` | 抽屉只由 chat pill 触发 | 加 `show_hamburger`(或在 MobileTopBar 内统一 hamburger→`viewport.drawer_open.set(true)`);Chat 传 false(pill 即触发), 其余 tab 顶栏给 hamburger |
| `components/notification_center.rs:68-93` | 铃铛 `fixed right-3 z-[50]` 全局浮动 | 移动端经 mobile-aware wrapper 渲染进 MobileTopBar 右槽;`NotificationsState` 保留在 root(避免生命周期问题), 仅 trigger 按钮位置改 |
| `views/{memory,agents}/...` + Dashboard/Teams/Extensions 顶部 | 无顶栏 | 挂 `MobileTopBar`(title = 模式名 via `label_of`, left = hamburger, right = bell) |
| `app.rs:222-227` | 抽屉 backdrop z-65 | 不变(z-65 在 drawer z-70 下正确) |

### 2.4 风险与缓解
- **R-1 通用顶栏不可假设 agent 上下文**:`MemoryState`/agent_id 只在 Chat 提供;`mobile_agent` memo(`chat/view.rs:43-58`)是 Chat 专属。→ `MobileTopBar` 的 title 一律由 **caller 经 slot 传入**,组件本身无 agent 依赖。
- **R-2 铃铛生命周期**:`NotificationCenter` 在 Router 外、`NotificationsState` 在 root。→ 状态留 root,只把 trigger 按钮**视觉**纳入顶栏(mobile-aware wrapper),不下移状态。
- **R-3 z-index 冲突**:tab z-40 / 旧 band z-50 / 铃铛 z-50 / backdrop z-65 / drawer z-70。→ MobileTopBar 统一为一条 band,铃铛并入其中,消除 z-50 重叠;层级文档化。
- **R-4 单一 `drawer_open` 信号多触发器**:多个 hamburger 共写同一信号——因路由变更已自动关抽屉(`:71-74`),重开安全;每个模式只保留**一个** trigger。
- **R-5 safe-area**:`env(safe-area-inset-*)` 在非 Tauri Web 下为 0,须 `viewport-fit=cover`(Phase 0.5 已设 index.html)。

---

## 3. 工作流 ② — Settings 分组列表 landing + 1 列表单

### 3.1 目标
- Settings tab 手机落地 = 由 `SETTINGS_GROUPS` 直接渲染的 **iOS 分组列表**(6 组 / **21 项**;Channels 为 1 项 → 子页含 4 平台,cell = icon + label + ›);点击 → 现有路由 push;单页顶栏 ‹ 返回。
- 所有设置页 `max-w-*` + 多列网格在 `<640px` 降为全宽单列。
- 抽屉里的设置子导航保留为快跳(power-user)。

### 3.2 复用(grounded)
- `SettingsTab`(全部变体)+ `path()` / `i18n_label()` / `icon_svg()` + `SETTINGS_GROUPS`(**6 组 / 21 项渲染**)(`components/settings_sidebar.rs:11-258`)——**元数据完整, 零新数据**即可驱动分组列表。
- `SettingsRouter` 路径分发(`app.rs:449-504`, 23 leaf + channels 通配)——路由全在,只加 landing。
- `DashboardState`(连接态 / API)。

### 3.3 改动(reflow targets, grounded)
**(a) 分组列表 landing**
- **新建 `views/settings/mobile_landing.rs`**:遍历 `SETTINGS_GROUPS` 渲染分组 cell 列表;`max-sm:block`,桌面 `max-sm:hidden`。
- `views/settings/mod.rs:124`:Quick Setup 容器加 `max-sm:hidden`(桌面保留),并在同级**同时挂载**(非条件卸载, 避免 Leptos 响应作用域问题)`MobileSettingsLanding`(`max-sm:block`)。
- **Channels 处理**:Channels 组在分组列表显示为单个 cell → 点击进 `/settings/channels`(overview),其内部已有 Telegram/Discord/WhatsApp/IMessage 子路由(`SettingsRouter` 通配),不在 landing 平铺 4 平台。

**(b) 表单 1 列化 + 安全内边距**(逐项, 全部加 `max-sm:`)。**⚠️ 工作流 ② 的 T0 = 全量审计**:下表是 grounding 已确认的**种子集, 非穷举**;落地前先 `find views/settings -name '*.rs' | xargs grep -nE 'grid-cols-2|px-8|px-6|max-w-'` 枚举所有页(含 `providers/`、`security/`、`acp_harnesses/`、`network/`、`embedding_providers/` 等子目录,与 `memory.rs` 各子组件内嵌网格),补齐遗漏。优选抽一个 `.grid-responsive`(`@apply grid-cols-1 sm:grid-cols-2`)统一类降同步税,并以 grep lint(`grep 'grid-cols' | grep -v max-sm` 应为 0)兜底。
| 文件:行 | 现状 | 改为(追加) |
|---------|------|------------|
| `settings/mod.rs:124` | `px-8 ... max-w-5xl mx-auto` | `max-sm:px-4 max-sm:max-w-none` |
| `settings/general.rs:68` | `px-8 ... max-w-4xl mx-auto` | `max-sm:px-4 max-sm:max-w-none` |
| `settings/appearance.rs:44` | `px-8 ... max-w-4xl mx-auto` | `max-sm:px-4 max-sm:max-w-none` |
| `settings/route.rs:102` | `px-8 ... max-w-5xl mx-auto` | `max-sm:px-4 max-sm:max-w-none` |
| `settings/execution.rs`(最外层) | `px-8 ... max-w-5xl mx-auto` | `max-sm:px-4 max-sm:max-w-none` |
| `settings/behavior.rs:40` | `px-6 ... space-y-6` | `max-sm:px-4` |
| `settings/memory.rs:67` | `px-6 ...` + 内 `max-w-4xl` | `max-sm:px-4` + 内 `max-sm:max-w-none` |
| `settings/memory.rs:178/249/381/561/709` | `grid grid-cols-2 gap-4`(6 处, 含子组件) | `max-sm:grid-cols-1` |
| `settings/route.rs:239` | `grid grid-cols-1 md:grid-cols-2 gap-4` | `max-sm:grid-cols-1`(确保 <640 单列) |
| `settings/channels/overview.rs:67` | `grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3` | 已响应, 校验 gap;必要时 `max-sm:gap-2` |

**(c) i18n(可选, 非阻塞)**:`SettingsTab::i18n_label` 有 4 个**绕过 i18n 的硬编码串**:Appearance=`"外观"`(`:86`)、Browser=`"Browser"`(`:103`)、Execution=`"Execution"`(`:108`)、Network=`"服务与集群"`(`:109`);另 `SettingsGroup::i18n_label` 的 Network 组名亦硬编码 `"服务与集群"` → 全部补 en/zh key。其余分组/页标题已走 i18n;分组列表标题用 `SettingsGroup::i18n_label`。

### 3.4 风险与缓解
- **R-6 `aleph-content-top` 自定义工具类**:须确认其不设横向 padding,否则与 `px-8 max-sm:px-4` 叠加。→ 落地前 grep tailwind.css 定义核验。
- **R-7 `max-sm:max-w-none` 在极窄屏(<320px)内容溢出**:→ 真机 iPhone SE(375)/最窄 320 验,必要时回退 `max-sm:px-3`。
- **R-8 `memory.rs` 6 处独立网格分散在子组件**:任一漏改即露馅。→ 递归扫 `BasicSettings/CompressionSettings/RetrievalPipelineSettings/...` 子组件内网格。
- **R-9 列表与 `SETTINGS_GROUPS` 同步税**:→ landing 直接从 const 生成(零同步)。

---

## 4. 工作流 ③ — Teams 只读为主 MVP(经抽屉进入)

### 4.1 目标
- 5 子视图(Overview/Kanban/Plan/Replay/Workers)在手机可读。
- 桌面"小侧栏切换"→ 顶部横滑 segmented pills。
- Kanban 6 列 → 单列 + 状态筛选;DAG → 只读分层列表;Replay → 上下堆叠;task 详情 → 底部 sheet。
- **入口**:Teams 非底部 tab,经 ModeSidebar 抽屉(已列 Teams)进入——工作流 ① 的 hamburger 落地后即可达。

### 4.2 复用(grounded)
- `TeamsTabState.sub_tab`(`views/teams/mod.rs:45`);切换器更新同一信号。
- `ViewportState.is_mobile`(条件渲染分支)。
- `compute_depths`(`teams/plan_dag.rs:111-140`)+ 分层 grouping(`:159-169`)——纯函数,**直接复用**于分层列表。
- `TaskDetailDrawer`(`teams/components/task_drawer.rs:15`)——kanban(`:136`)+ plan_dag(`:102`)共用,改 sheet 仅 CSS。
- `TeamsApi.list_tasks` / `CoordTaskDto`(全子视图共享, 无破坏)。
- `TeamSelector`(`team_selector.rs`, 已 `w-full` 紧凑, 无需改)。
- Overview(`overview.rs`)/ Workers(`workers.rs`)——已接近响应式,**零改动**。

### 4.3 改动(reflow targets, grounded)
| 文件:行 | 现状 | 改为 |
|---------|------|------|
| `teams/mod.rs:95-136`(`TeamsSidebar`) | 竖向 `<nav> flex-1 overflow-y-auto px-3 space-y-1` | `max-sm:` 横滑 pills:`flex flex-row gap-1 overflow-x-auto`(white-space:nowrap);桌面不变 |
| `teams/components/board.rs:35` | `grid` `repeat(auto-fit, minmax(220px,1fr))`(6 列) | `max-sm:` 单列 `flex flex-col` + **状态筛选下拉**(一次看一列, 见 D3b + §11 P-④);2 列分组方案撤回 Phase 2 |
| `teams/components/task_drawer.rs:191-193` | `fixed inset-0 flex justify-end` + `aside w-96 h-full`(右侧滑出) | `max-sm:` 底部 sheet:`flex items-end` + `aside w-full max-sm:h-[90vh] max-sm:rounded-t-lg max-sm:overflow-y-auto`;桌面 `md:w-96 md:h-full` |
| `teams/plan_dag.rs:156-290` | `<svg>` 固定 `NODE_W=200px` 布局 | `max-sm:` 跳过 SVG,渲染**只读分层列表**(复用 `compute_depths` 分层, 按 depth 缩进 `pl-{depth*4}`;多父节点在其最深 depth 出现一次,依赖以文字 chip 标注)+ 一行"完整 DAG 请在桌面查看";桌面保留 SVG(`md:`)。**T3 加边界单测**:空树/单节点/线性链/宽 DAG |
| `teams/replay.rs`(两栏) | 左任务列表 + 右 trace(flex row,**T3-a 落地前审计实际两栏结构**) | `max-sm:` 单列堆叠:列表 `max-h-[45%]` 在上 + trace `flex-1 overflow-y-auto` 在下 |
| `teams/components/{column,task_card}.rs` | flex column / 紧凑卡片 | 可选 `max-sm:` 密度微调(`space-y-2`→更紧, `p-3`→`p-2`) |

### 4.4 风险与缓解
- **R-10 Kanban 单列+筛选 vs 横向滚动**:筛选增交互复杂度。→ MVP 用状态下拉;备选 2 列分组保留多列扫读;留待真机体验定。
- **R-11 DAG 分层列表丢失边可视化**:→ 列表标注每节点依赖项;明确"完整图在桌面",不在手机做平移缩放 SVG(YAGNI)。
- **R-12 sheet header 滚动失焦**:长内容(runs/comments/events)→ sticky header + 滚动 body。
- **R-13 Teams 可发现性**:非 tab,仅抽屉。→ 确保抽屉 Teams 项显眼;文档化(未来若高频再议升 tab)。
- **R-14 Replay 实际布局未在摘录中完全可见**:→ 落地前确认两栏实现再写单列分支。
- **注**:全文件未见拖拽改状态 handler(`task_card.rs:31` 点击即开 drawer),故"手机禁拖拽"无需处理。

---

## 5. 工作流 ④ — Canvas 触控强化(列表为主 + 可选触控画布)

> **范围已收窄**(grounded):`[列表]/[星系]` 切换 + 单指平移/旋转/点选**均已存在**。本工作流只补 4 个真缺口。

### 5.1 目标
1. **双指捏合缩放**(multi-touch)。
2. 手机端**触控半径 ≥44px**(防胖手指误触)。
3. 节点详情 → **底部 sheet**(桌面右侧面板的移动变体)。
4. **WebGL2 不支持时优雅回退列表**(现状硬报错)。
5. 手机性能护栏(bloom/settle 钳制)。

### 5.2 复用(grounded)
- **单指交互已工作**:`galaxy_canvas.rs:210-319` 用 Pointer Events(`on_pointerdown/move/up/wheel`)+ `touch-action:none`(`:314`)+ pointer-capture(`:217/269`)→ 单指 pan/orbit/tap 触屏即用。
- `OrbitCamera.orbit(daz,del)` / `zoom(factor)` / `note_interaction(t)`(`gl/camera.rs:45-62`, 纯 Rust)。
- `Scene.on_drag/on_wheel`(`gl/scene.rs:247-262`)/ `Scene.pick`(`:170-179`)/ `pick_node(...,radius_px)`(`gl/picking.rs:6-40`, 半径是入参)。
- **切换已现成**:`MemoryView{ Graph, Table }`(`state/memory.rs:19-22`)+ memory_hub 工具栏 `display:none` keep-alive 切换(`memory_hub/mod.rs:46-62`)+ 手机默认 Table Effect(`:38-44`)。
- 详情内容 `NodeDetailPanel`(`canvas/node_detail_panel.rs`, 纯 I/O 无布局壳)。
- `ViewportState.is_mobile`。
- `docs/design-system/aleph-mobile/` 的 `.sheet` 组件结构 + 移动间距 token。

### 5.3 改动(reflow targets, grounded)
| 文件:行 | 现状 | 改为 |
|---------|------|------|
| `canvas/galaxy_canvas.rs:203-306` | 仅单指针 Pointer 事件, 无多指 | 加 **两指 pinch**:追踪两个活动 pointerId(或 touchstart/move 双触点),`distance` 比值 → `camera.zoom(factor)`(指数缩放, 同 wheel) |
| `canvas/gl/scene.rs:176`(`pick()` 调 `pick_node(...,18.0)`) | 半径硬编码 `18.0` | 移动端传 `44.0`(WCAG 触控目标, CSS px;高 DPI 由浏览器坐标缩放, 无需手算) |
| `canvas/galaxy_canvas.rs:232-259`(hover pick) | 每 pointermove 触发 picking | 移动端 ~75ms 节流(触控运动粗糙) |
| `canvas/mod.rs:333-337`(详情面板) | `absolute bottom-0 right-0 w-72 max-h-[60%]` | `max-sm:left-0 max-sm:w-full max-sm:max-h-[50%] max-sm:rounded-t-2xl`(底部 sheet, 纯 CSS) |
| `gl/context.rs:11-27` | `from_canvas()` 已返回 `Result<_,String>`(WebGL2 缺失=`Err`) | 不改 context;经 §11 P-⑥ 的 `fallback` 信号 → `CanvasView` watch → `memory_view.set(Table)` + Memory 内联 banner |
| `gl/scene.rs:334`(bloom) | 每帧无条件 `bloom.run` | 信号化 `bloom_level`(见 §11 P-⑦),移动端默认禁/低,真机择定 |
| `gl/scene.rs:17`(settle) | `MAX_SETTLE_STEPS=400` | 移动端降 200-250 加速沉降 |

### 5.4 风险与缓解
- **R-15 STALE-EMBED**:Panel WASM 经 `rust_embed` 编译期嵌入;改完须 `just wasm` + 重编 server 否则 serve 旧版(项目反复踩)。→ 实现/验证流程显式列重编步骤。
- **R-16 WebGL2 在 ~5% 老安卓(Mali-400/老 Adreno)缺失**:→ 回退信号必须落地,否则崩。
- **R-17 pinch 数学 + 手指中途抬起**:`new = old*(cur_dist/init_dist)`,须按 `touch.identifier`/pointerId 持久追踪两点,优雅处理一指释放。
- **R-18 tap 与 pick 竞态**:tap 后 pointerup 再 pick 同位 → 时间窗(<100ms)去重 / 区分 touch vs mouse。
- **R-19 bloom 在移动 GPU 上掉帧**:半分辨率 2-pass 高斯仍贵 → 真机 Pixel 4a / Galaxy S10 级验。
- **R-20 详情 sheet 安全区**:底部 sheet 须 notch-aware(`safe-area-inset-bottom`);用设计系统 `.sheet` 结构。

---

## 6. 实现顺序与依赖

```
① 导航外壳 ──gate──▶ ② Settings(分组列表需顶栏返回)
              └─gate──▶ ③ Teams(入口靠抽屉 hamburger)
④ Canvas(最独立, 可与 ②/③ 并行)
```

- **① 必须先做**:② 的 ‹返回 与 ③ 的 Teams 入口都依赖它。
- **④ 可并行**:仅依赖 Phase 0.5 已有的切换/列表,不依赖 ①。
- 每步纯 WASM/CSS;每步以 `just wasm` 构建绿 + 390px 浏览器实测(chrome-devtools)收尾;**改 dist 后看效果须重编 server**(rust_embed)。

---

## 7. 范围外 / 延后(YAGNI)

- Teams **完整编辑**(Kanban 拖拽改状态、DAG 平移缩放、完整 plan 编辑)→ Phase 2/桌面。
- Settings **手机专属精简视图**(Providers/Channels/Policies 高级字段折叠)→ 未选(D2 选了纯分组列表+1列)。
- Canvas **全触控画布作默认** / 动态 LOD / haptics / swipe-to-dismiss → 未选/Phase 2。
- 边缘滑动返回(swipe-back)、tab 横向手势 → Phase 2 打磨。
- iOS 原生桥(APNs/音频/safe-area shim)→ 独立 iOS 实施计划(`2026-06-25-aleph-ios-implementation-plan.md`),与本 spec 解耦。

---

## 8. 风险总览

见各工作流 §x.4(R-1…R-20)。横切风险:
- **rust_embed STALE-EMBED**(R-15):贯穿全 Phase, 每次验证前重编。
- **safe-area env() 在 Web 为 0**(R-5/R-20):依赖 `viewport-fit=cover`;iOS 壳须验。
- **`aleph-content-top` 横向 padding 叠加**(R-6):落地前核验定义。

---

## 9. 验证策略

- 每工作流:`cargo check --target wasm32`(廉价)→ 工作流末 `just wasm` 一次 → 重编 server → chrome-devtools 390px(及 320/375 窄屏)实测对应屏。
- 不跑全量 cargo(遵项目"极度节制 cargo 调用");高风险合并至多一次 `cargo check --lib`。
- 像 MVP 一样,具体像素屏可在 Claude Design(`aleph-mobile/` 设计系统)生成对照。
- 验证 dist 嵌入用 served wasm size / `grep -a`,**不**用 `strings`(rust_embed 压缩存储)。

---

## 10. 新建文件清单(供 plan 拆分)

- `interfaces/webchat/src/components/mobile_top_bar.rs`(工作流 ①)
- `interfaces/webchat/src/components/notification_bell.rs`(从 `notification_center.rs` 抽 trigger 子组件, 工作流 ①, 见 §11 P-②)
- `interfaces/webchat/src/views/settings/mobile_landing.rs`(工作流 ②)
- `interfaces/webchat/src/views/teams/components/status_filter.rs`(工作流 ③, 见 §11 P-④)
- (其余 Teams/Canvas 改动均为现有文件内 `is_mobile`/`max-sm:` 分支 + CSS,无新路由/新视图)
- i18n:`locales/en.json` + `zh.json` 补:4 个设置硬编码 label + Network 组名、segmented/筛选/"桌面查看"/"星系视图"/"本设备不支持"等新串。

---

## 11. 关键接口决策(pinned — 供 plan 直接落 TDD,消除"实现者猜测")

> 对抗式评审挑出 7 处"实现者必须自行发明"的接口缺口。此处一次性钉死,plan 据此写失败测试即可。

- **P-① `MobileTopBar` 签名**:`#[component] fn MobileTopBar(title: Signal<String>, #[prop(optional)] left: Option<Children>, #[prop(optional)] right: Option<Children>)`。
  - `left` 缺省 → 自动渲染 hamburger(`on:click` 写 `viewport.drawer_open.set(true)`);Chat 传 `left = agent pill`(pill 即 trigger)。
  - `right` 缺省 → 自动渲染 `NotificationBell`。
  - `center` 恒为 `title`(纯字符串信号)→ 组件**零 agent/`MemoryState` 依赖**(R-1);Chat 把 agent 名作 title 传入,其余 tab 传 `label_of(mode)`。
  - 安全区 + z-index 烤进设计系统类 `.mobile-top-bar`(`padding-top: calc(env(safe-area-inset-top)+0.5rem)` + 统一 z-band),各 tab 继承,免逐 tab 重写(消 R-3/R-5)。

- **P-② 通知铃铛拆分**:从 `notification_center.rs` 抽 `NotificationBell` 触发子组件(读 root 的 `NotificationsState`,只渲染按钮);popover/sheet 与状态**留在 root**(`app.rs:99/264` 不动)。`MobileTopBar` 右槽挂 `NotificationBell`——状态不进 Router,避免生命周期问题(R-2)。

- **P-③ 返回语义**:SPA 无历史栈 → **显式 `navigate`,不 `history.back()`**。确认 `/settings`(landing:桌面 Quick Setup + 移动分组列表)与 `/teams`(overview)为各自"根";子页 ‹返回 = `navigate("/settings")` / `navigate("/teams")`。

- **P-④ Kanban 状态筛选**:`TeamsTabState` 加 `task_status_filter: RwSignal<Option<TaskStatus>>`(跨子视图重挂保活,呼应 `sub_tab` 模式);新 `teams/components/status_filter.rs` 读写之;`KanbanView` 移动端在列上方渲染,桌面忽略。

- **P-⑤ 双指缩放**:**仅用 Pointer Events**(`galaxy_canvas` 已是)。`pointerdown` 把 `pointerId→(x,y)` 存 map;`pointermove` 若 ≥2 活动指 → 取前两指 `dist/initial_dist` 比值 → `camera.zoom(factor)`(指数,同 wheel 曲线);`pointerup` 移除该 id,<2 停止缩放;一指中途抬起以剩余指重置基线。**不混用 TouchEvent**(R-17)。

- **P-⑥ WebGL2 回退**:`GalaxyCanvas` 加 `fallback: RwSignal<bool>`;`Scene::new`(经 `context::from_canvas` 的 `Err`)失败 → 置 true;`CanvasView` watch → `memory_view.set(MemoryView::Table)` + Memory 视图顶部内联 banner「本设备不支持星系视图」。**挂载时检测,永久切 Table**(可在 Memory 工具栏手动再试)(R-16)。

- **P-⑦ bloom/性能信号化**:`bloom_level: RwSignal<f32>` ∈ [0,1](或 `is_mobile` Memo 驱动)传入 `Scene`,每帧读;移动端默认 0(禁)或 0.5(钳制),真机 Pixel 4a / S10 级择定。`MAX_SETTLE_STEPS` 移动端降 200–250 同理经信号(R-19)。
