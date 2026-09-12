# 画布入右侧栏 + 手绘/矢量/动画基础设施 — 设计（2026-09-12）

> 状态：经用户批准（2026-09-12，「全部 Phase 1–5」+「删除独立 tab 与手机端只读列表」）。
> 参考：[CANVAS.md](../../reference/CANVAS.md)（现状运行参考）· [浏览器直播视图 spec §4.3](2026-09-05-browser-live-view-design.md)（右栏体的样板）· 参考项目 archify / Cowart / tldraw / srt-whiteboard-animation / hand-drawn-explainer-video-nikola / handraw-style（勘察结论见 §0 与本文末尾的对比表）。

## 0. 决策记录

| # | 决策 | 依据 |
|---|---|---|
| D1 | 画布**不再是独立 tab**；它是右侧工作区栏（`WorkspacePanel`）的一个**体**，与当前会话联动 | 用户裁定；与浏览器 spec D5 同一裁定，画布是第一个真正落地"右栏多体"的功能 |
| D2 | 右栏"体"机制一次做成可扩：`WorkspaceBody` 枚举 + `available(is_team)` 纯函数；浏览器 plan 2 只加一个变体 | 判据 §16 孪生：别给画布造私有开关，那是 browser 的第二个真源 |
| D3 | 删除 `PanelMode::Canvas` / `/canvas` 路由 / nav 条目 / `platform/phone/canvas/` | 熵减；手机端与浏览器 spec 同口径"不做" |
| D4 | 画廊改宿主：左栏 `CanvasSidebar` → 画布体头部的 picker 弹层，复用 `library.rs` 全部纯函数 | CANVAS.md §6 四条纪律不变，尤其"三条 liveness 线不下放" |
| D5 | 自动弹开镜像浏览器 L2：当前会话的 run 里 `canvas` 工具结果携 `canvas_id` → 打开它、`ChatOnly→Split`、选中画布体；用户本 run 手动收起后不再弹 | 用 `WorkspaceState.tool_payloads` 这条已有但近乎休眠的线 |
| D6 | 右栏加左缘 resizer，宽度持久化 localStorage，clamp `[280px, 80vw]` | 画布与浏览器体都需要；每设备 UI 偏好归 Panel 本地（与 `save_view_state` 同判据） |
| D7 | 模型扩展全部 `serde(default)` 向前兼容旧文档；**不**为动画/风格新建第二份文档模型 | 判据 §1 |
| D8 | 动画 v1 = **reveal**（逐形状 draw-on / fade / wipe），**不做 tween**（位移/旋转/缩放） | srt-whiteboard 的 `{startMs,durationMs}` + ink→color 两阶段足够覆盖"白板手绘动画"；GSAP tween 词汇留作 v2 |
| D9 | 手绘风格 = 移植 tldraw `PathBuilder.toDrawD`（种子 xorshift 顶点抖动 + 二次曲线圆角 + 双遍），**不引 rough.js / 不引 JS 依赖** | CANVAS.md §8 tldraw 嵌入已否决；纯算法可移植 |
| D10 | 261 号手绘风格画廊**不进**契约/工具 DESCRIPTION | R9：那是 prompt 知识，留给 skill |

## 1. 右侧栏体机制（Panel）

### 1.1 状态（`interfaces/webchat/src/state/layout.rs`）
```rust
pub enum WorkspaceBody { Artifacts, Deliverables, Tasks, Canvas }
impl WorkspaceBody { pub fn available(is_team: bool) -> &'static [WorkspaceBody] }
// 单 agent: [Artifacts, Canvas]；team: [Deliverables, Tasks, Canvas]
pub struct WorkspaceState {
    …,
    pub body: RwSignal<WorkspaceBody>,
    pub width_px: RwSignal<Option<u32>>,          // None = CSS 默认 40%
    pub auto_reveal_muted_run: RwSignal<Option<String>>,
}
```
- `reveal(body)`：`set_layout(Split)` + `body.set(body)`；`collapse_by_user(run_id)`：`set_layout(ChatOnly)` + 记 `auto_reveal_muted_run`。
- `reset()`（换会话）清 `auto_reveal_muted_run`，**保留** `mode`/`body`/`width_px`。
- 纯决策表 `auto_reveal_decision(mode, muted_run, this_run, canvas_id) -> Option<RevealAction>`，单测钉住每一臂（muted 同 run 不弹、不同 run 弹、已 Split 只切体不动 mode、无 canvas_id 不动）。
- 若 team 切换使当前 `body` 不在 `available` 内，回落到该列表第一项（纯函数 `coerce_body`）。

### 1.2 组件
- `components/workspace_panel.rs`：头部 = 体切换 tab 条（由 `available(is_team)` 驱动）+ 左缘 resizer 手柄；`Artifacts`/`Deliverables`/`Tasks` 沿用现有组件；`Canvas` 体 = `views::canvas::CanvasView`，**用 `style:display` 切换而非 `<Show>` 卸载**（keep-alive 硬约束：三条 liveness 线 + undo 栈 + 相机）。其余三体沿用现有挂卸行为。
- resizer：pointerdown/move/up 改 `--aleph-workspace-w` 为 px 值（写在 `ChatView` 根元素 `style` 上；`app.rs` band chrome 与 `view.rs` 的 `pr-[…]` 读同一 token，零改动），持久化 `aleph.panel.workspace_w`。clamp 纯函数 `clamp_width(px, viewport_w)` 单测。
- `CanvasView` 内部：头部 strip = picker 弹层（列表/搜索/新建/重命名/删除，`library.rs` 现有函数）+ 标题（行内重命名）+ 导出/播放；无打开画布时 `WelcomePane` 直接内嵌列表。
- 非 operator：画布体照常提供（可见性谓词在服务端，与浏览器 admin-only 不同）。

### 1.3 删除
`PanelMode::Canvas`（`mode_sidebar.rs` 枚举 / `from_path` / `path` / `all` / `under_more` / 侧栏臂）、`nav_menu.rs` 三处、`app.rs` 主区容器与 import、`platform/phone/canvas/`、i18n `nav.canvas` 键（跑 `i18n_census`）。`/canvas` 旧路径经 `from_path` 回落到默认（Chat）。

### 1.4 联动
- `views/chat/events.rs` 在 `record_tool_result` 处：若工具名 == `canvas` 且结果 JSON 顶层有 `canvas_id` 字符串 → 决策表 → `CanvasState.open_canvas` + `workspace.reveal(Canvas)`。工具名复用 `ai.rs` 已钉住的拼写点，不写第二份字面量。
- transcript 的 `canvas` 工具卡加"在画布中打开"动作（L3 同款）：点击 = 同一条 reveal 路径。

## 2. 契约扩展（`shared/protocol/src/canvas.rs`；服务端零新 RPC、零新 action）

```rust
pub enum StrokeKind { #[default] Solid, Sketch, Dashed, Dotted }
pub struct ShapeStyle { color: String, fill: bool, size: SizeKind, #[serde(default)] stroke: StrokeKind }
// color：7 个命名槽位 或 `#rrggbb`；契约 `check_color` 校验，validate.rs 调用
pub enum GeoForm { Rect, Ellipse, Diamond, Triangle, Hexagon, Pill }
pub enum ArrowHead { None, Arrow, Triangle, Dot, Bar }
Shape::Arrow { …, #[serde(default)] bend: f64,
               #[serde(default)] head_start: ArrowHead /* None */,
               #[serde(default = "ArrowHead::arrow")] head_end: ArrowHead }
Shape::Path { style: ShapeStyle, d: String, closed: bool }
// d ⊂ SVG path：M L H V Q C Z（绝对/相对），数字有限；MAX_PATH_D_BYTES = 64 KiB
// 契约内 `parse_path_d` 是唯一解析器（Panel 渲染 / 导出 / sketch 全用它）
pub enum RevealMode { #[default] Draw, Fade, Wipe }
pub enum Ease { Linear, #[default] EaseOut, EaseInOut }
pub struct Reveal { start_ms: u32, duration_ms: u32, #[serde(default)] ease: Ease, #[serde(default)] mode: RevealMode }
ShapeCommon { …, #[serde(default, skip_serializing_if = "Option::is_none")] reveal: Option<Reveal> }
pub struct Timeline { #[serde(default)] total_ms: Option<u32>, #[serde(default)] hold_ms: u32 }
CanvasDoc { …, #[serde(default, skip_serializing_if = "Option::is_none")] timeline: Option<Timeline> }
```
- 闸（全在契约模块，`validate.rs::shape_is_well_formed` 调用，只拒不改写）：`check_color`、`parse_path_d`（字节上限、命令集、数字有限、非空）、`bend.is_finite()`、`reveal.duration_ms >= 1`、`start_ms + duration_ms <= MAX_REVEAL_MS`（10 min）。
- 工具 `DESCRIPTION`（`definitions.rs` 常量）加运行时事实：新形状/字段一览 + "reveal 是播放时的 draw-on，不是实时动画" + "sketch 描边由 Panel 按形状 id 播种，同一文档处处一致"；schema 自动从 `CanvasOp` 派生。棘轮账 `CATALOG_DESCRIPTION_CEILING_BYTES` 按实测重钉。
- `get(summary)` 摘要每行附 `stroke` 非 Solid / `reveal` 存在的紧凑标记。
- 契约测试：旧文档（无新字段）解析等价；每个新枚举的 wire 拼写；`parse_path_d` 认得的命令集列举 + 未知命令拒绝 + 上限。

## 3. Panel 渲染（`views/canvas/`）

| 新文件 | 纯函数 | 消费者 |
|---|---|---|
| `sketch.rs` | `Rng::seeded(&str)` xorshift128 · `sketch_d(cmds, seed, stroke_w) -> String`（toDrawD：offset=w/3、roundness=2w、passes=2、角度钳位、短边钳位、close 复用 moveTo 偏移） | `shape_view.rs` Geo/Path/Arrow 描边；`export.rs` |
| `geo_path.rs` | `geo_cmds(form, w, h) -> Vec<PathCmd>` 六种 form 命令表 | `shape_view.rs`、`export.rs`、`sketch.rs`、箭头求交 |
| `arrow_geom.rs` | `arc_info(a, b, bend)` 三点定圆（`|bend| < 8` 视作直线）· `clip_to_outline(...)` 端点回退到轮廓交点（Rect/Ellipse 解析，其余多边形化）· `arrow_head_points(kind, …)`（吸收现有 `arrow_head_points`） | `shape_view.rs::arrow_svg`、`export.rs`（同一函数） |
| `reveal.rs` | `reveal_css(shape_id, reveal, mode) -> String`（Draw：`pathLength="1"` + dasharray/dashoffset，填充第二阶段 opacity 按 2:1；Fade：opacity；Wipe：clip-path inset）· `timeline_total_ms(doc)` | `present.rs` 播放（Play 按钮；deck 播放时按帧过滤）· `export.rs` 动画 SVG（`<style>` 内嵌 `@keyframes`） |
| `snap.rs` | `snap_translate(moving, others, threshold) -> (nudge, guides)`：每形状 4 角+中心；阈值 8px/zoom；不做间隙链 | `interaction.rs` 拖动；overlay 参考线 |
| `toolbar.rs` 扩展 | 样式面板：颜色 7 槽 + hex、size 三档、fill、stroke 四种；**唯一** `CanvasState::style` 写者；建形读它（还清 24 处 `ShapeStyle::default()` 的债） | — |

## 4. 安全与判据自查
- 新 ingress：无。`Path.d` 只经契约解析器变成 `<path d>`，不接受 `url()`/script，无注入面。
- §1 一份表述：几何 recipe / 箭头数学 / sketch 只有一份，`shape_view` 与 `export` 共用。§3：`parse_path_d` 认得的命令集有列举测试 + 未知命令拒绝。§4：自动弹开测"效果到达"（`open_canvas` 与 `body` 都变）。§9：工具面/RPC 面同一校验函数。§17：resizer 宽度能指出读它的那两行（token 已有两个读者）。

## 5. 刻意不做
tween 动画 · rough.js / 任何 JS 依赖 · 间隙吸附链 · 服务端布局引擎（archify 的 workflow 求解器）· 相对锚定放置 action · 手机端画布 · `/canvas/:id` 深链 · Path 的 `A`（椭圆弧）命令 · 每形状旋转 · 全屏独立编辑器。

## 6. 让什么变难了
- 画布只能在 Chat 路由下打开；没有全屏编辑器（`present.rs` 全屏播放仍在，resizer 最宽 80vw）。
- 手机端失去画布。
- `Shape` 新字段对**旧版纯壳 Panel** 是硬解析失败而非降级（同 tag 发版可接受；LAN 纯壳需同步升级）。
- reveal 未来扩 tween 时须保证"一个属性只有一个驱动者"（nikola §4 的不变量），届时 `Reveal` 与 tween 的相互排斥要进闸。

## 附：参考项目对比摘要

| 维度 | 参考 | Aleph 现状 | 取舍 |
|---|---|---|---|
| 画布挂载 | Cowart MCP widget 嵌对话；浏览器 spec D5 右栏体 | 独立 tab；右栏无体枚举 | 建 `WorkspaceBody` |
| IR | archify 闭合类型化 IR + 诊断码；tldraw record+分数索引 | 已有 9 变体 + FracIndex + ops 乐观锁 | 扩展不重写 |
| 自由笔迹 | tldraw perfect-freehand | 已有 `Ink` + `freehand.rs` | 不动 |
| 手绘风格 | tldraw `toDrawD` | 无；样式渲染端完整但 Panel 无写者 | `sketch.rs` + 样式面板 |
| 曲线箭头 | tldraw 三点定圆弧 | 直线 | `bend` + 头种类 |
| 任意矢量 | 都输出 SVG | 无 | `Path` 变体 |
| 动画 | srt-whiteboard reveal 两阶段；GSAP tween | 零 | reveal v1 |
| 吸附 | tldraw 8px、5 点、间隙链 | 无 | 点吸附 v1 |
| 风格画廊 | handraw-style 261 号 | — | 不进模型 |
