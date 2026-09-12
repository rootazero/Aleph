# 实施计划：画布入右侧栏 + 手绘/矢量/动画基础设施（2026-09-12）

> Spec：[2026-09-12-canvas-right-pane-and-drawing-infra-design.md](../specs/2026-09-12-canvas-right-pane-and-drawing-infra-design.md)。分支 `canvas-right-pane`（worktree）。
> 执行形态：subagent 逐任务串行（本机 16G，cargo 不得并行；见 memory `cargo-memory-limit`）。每个任务自带验证命令，编排者验收后再进下一任务。

## 全局纪律（每个任务都适用）

- 每次 Bash 只跑**一个** cargo，`-j1`，环境 `CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_TARGET_DIR=/home/zou/data/workspace/Aleph/target`；测试加 `-- --test-threads=2`；超过 3 分钟的放后台。
- 禁止 `git stash` / `git checkout --` / `git restore` / `git clean` / `git commit`（编排者提交）。
- 新逻辑 = 纯函数 + native `#[test]`；组件只接线。改动 `interfaces/webchat/` 后必跑 `cargo test -p aleph-panel --lib`（先看警告再看错误：`unused` 说明没有调用者 → CUT）。
- 改动前扫 FEATURE_LOCATOR 附录 E.7（Panel）/ E.3（工具）/ E.0（通用）。
- 删旧代码不留注释、不留 `#[allow(dead_code)]`。

## Task 1 · Panel 搬家 + 连线（只动 `interfaces/webchat/`）

1. `state/layout.rs`：`WorkspaceBody` 枚举 + `available(is_team)` + `coerce_body` + `auto_reveal_decision` 决策表 + `clamp_width`；`WorkspaceState` 新增 `body` / `width_px` / `auto_reveal_muted_run` + `reveal` / `collapse_by_user`；`reset` 语义按 spec §1.1。每个纯函数带测试。
2. `components/workspace_panel.rs`：体切换头 + resizer + 体容器；Canvas 体 `style:display` 切换；`LayoutToggle` 收起走 `collapse_by_user(current_run)`。
3. `views/canvas/mod.rs` / `library.rs`：`CanvasSidebar` 改为 picker 弹层组件（同一文件、同一批纯函数），`CanvasView` 头部 strip；`WelcomePane` 内嵌列表。
4. 删除：`PanelMode::Canvas` 全部臂、`nav_menu.rs` 三处、`app.rs` 容器/import/`provide_context` 位置保留（`CanvasState` 仍需在 `AppContent` 提供一次）、`platform/phone/canvas/`、`views/canvas/editor.rs:588/727` 两处 `PanelMode::from_path != Canvas` 门控（改为"画布体可见"门控）、i18n `nav.canvas`。
5. 联动：`views/chat/events.rs` 在 `record_tool_result` 后调用 `canvas_auto_reveal(...)`（工具名取 `ai.rs` 的常量）；transcript `canvas` 工具卡加"在画布中打开"。
6. `tailwind.css`：`.aleph-workspace-pane` 收起态样式不变；新增 resizer 手柄样式。

验证：`cargo test -p aleph-panel --lib`（全绿，零新 warning）· `cargo test -p aleph-panel --lib i18n`（census）· grep 无 `PanelMode::Canvas` / `phone/canvas` 残留。

## Task 2 · 契约扩展 + 服务端闸 + 最小 Panel 渲染

1. `shared/protocol/src/canvas.rs`：按 spec §2 新增 `StrokeKind` / `GeoForm` 4 种 / `ArrowHead` / `Arrow.bend,head_start,head_end` / `Shape::Path` / `Reveal` / `Ease` / `RevealMode` / `ShapeCommon.reveal` / `Timeline` / `CanvasDoc.timeline` / `check_color` / `parse_path_d` + `PathCmd` + `cmds_to_d` / 上限常量。契约测试：旧文档解析等价、wire 拼写、`parse_path_d` 正负例、上限。
2. `src/canvas/validate.rs`：`shape_is_well_formed` 调 `check_color` / `parse_path_d` / `bend.is_finite` / reveal 闸；`apply_ops` 对 `SetDocMeta` 不变（timeline 经新 op? **不**——timeline 走 `SetDocMeta { title, timeline: Option<Timeline> }` 加可选字段，`serde(default)`）。store 测试各加一条拒绝例。
3. `src/builtin_tools/canvas.rs` + `definitions.rs`：DESCRIPTION 加运行时事实段；`get(summary)` 加紧凑标记；棘轮账按实测重钉（跑 census 测试看红的名单是不是预期那份）。
4. `interfaces/webchat/`：让 aleph-panel 编译且语义正确——`shape_view.rs` 新增 `Path` 臂（`<path d>` 直出）、新 `GeoForm` 臂（多边形/胶囊）、箭头头种类；`export.rs` 同步（同一函数）；`ops.rs::apply_local` 与服务端对拍测试更新；`interaction.rs` 建形默认值。`ai.rs`/`decks.rs` 等构造点补新字段默认。
5. `tests/canvas_wire.rs`：键集对拍测试覆盖新字段（从契约类型序列化派生期望集，非字面量）。

验证（逐条、串行）：`cargo test -p aleph-protocol`（若 crate 名不同以 `shared/protocol/Cargo.toml` 为准）· `cargo test -p alephcore --lib canvas -j1 -- --test-threads=2` · `cargo test -p alephcore --lib builtin_registry -j1 -- --test-threads=2`（棘轮）· `cargo test -p aleph-panel --lib` · `cargo test -p alephcore --features test-helpers --test canvas_wire -j1`。

## Task 3 · Panel 渲染增强

1. `views/canvas/sketch.rs`：xorshift 种子 RNG + `sketch_d`；测试：同 seed 同输出、不同 seed 不同、闭合路径首尾一致、零长边不 NaN。
2. `views/canvas/geo_path.rs`：六种 form 的 `PathCmd` 表；`shape_view.rs` / `export.rs` 改为消费它（删除两处各自的 rect/ellipse 字面量）。
3. `views/canvas/arrow_geom.rs`：`arc_info` / `clip_to_outline` / `arrow_head_points`（迁入现有实现）；`shape_view::arrow_svg` 与 `export.rs` 共用；测试：`bend=0` 与旧直线路径等价、正负 bend 镜像、端点落在轮廓上。
4. `views/canvas/reveal.rs`：`reveal_css` / `timeline_total_ms`；`present.rs` 加 Play（含 deck 内播放）；`export.rs` 加"动画 SVG"导出（PNG 导出不变）。
5. `toolbar.rs`：样式面板；`CanvasState::style: RwSignal<ShapeStyle>`；`interaction.rs` 建形读它（删除 24 处 `ShapeStyle::default()` 中属于"人建形"的那些；模板/deck/present 里非建形的保留并逐处说明）。
6. `shape_view.rs`：`StrokeKind` 四种描边（Sketch 走 `sketch_d`，dash 走 `stroke-dasharray`）。

验证：`cargo test -p aleph-panel --lib`；grep 确认 `shape_view.rs` 与 `export.rs` 不再各持一份几何。

## Task 4 · 吸附与参考线

`views/canvas/snap.rs` 纯函数 + `interaction.rs` 拖动接入 + overlay `<line>` 参考线；阈值 8px/zoom；测试：阈内吸、阈外不吸、多候选取最近、自身排除。验证同上。

## Task 5 · 文档与提交

- `docs/reference/CANVAS.md`：§1 代码地图（新文件）、§2 数据模型（新变体/字段）、§6 改为"右栏体 + picker"、§8 刻意不做清单增补、§9 QA 十项里第 10 项改写为"右栏体 + picker + 自动弹开 + resizer"。
- `docs/reference/FEATURE_LOCATOR.md` §6.10 现状、§6.8/§4.7 交叉引用、附录 E.7 新触发器（体切换 keep-alive / 自动弹开静音 / resizer token 两个读者）。
- `docs/reference/DESKTOP_SHELL.md` 右栏段落；`docs/superpowers/specs/2026-09-05-browser-live-view-design.md` §4.3 加注记"体机制已由画布轮建成，browser 只加变体"。
- `qa/canvas/README.md` item 10 改写；`run.sh` 清单文案同步。
- 提交（英文 scope 前缀，按任务分多次）。
