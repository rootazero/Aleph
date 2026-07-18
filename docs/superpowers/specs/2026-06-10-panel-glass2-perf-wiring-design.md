# Panel 玻璃二轮收口 + WASM 性能 + 连线修复 — 设计

日期：2026-06-10
范围：`interfaces/webchat/`（Panel 前端）+ `Cargo.toml` profile + `justfile`（构建链）。后端 `src/` 零改动。

## 背景

2026-06-09 玻璃打磨轮（merged `ba646da5c`）建立了 4 级模糊 token 体系并统一了 6 个弹出层 + 5 个遮罩。本轮 gap-analysis（4 路 Explore + 亲读证伪）发现：

- **视觉**：仍有一批瞬时表面未迁移到玻璃材质（设置页 modal 群、聊天侧栏 dropdown、project menu、teams drawer）。
- **性能**：运行时已健康（信号纪律、清理钩子、canvas IntersectionObserver 全部到位——诚实证伪）；真杠杆是 **WASM 体积 19.5MB**：`wasm-release` profile 未做任何尺寸优化，构建链无 wasm-opt。
- **连线/修复**：`api/agent_run.rs` 整文件死代码（`AgentApi` 零消费者 + 后端从未注册 `agent.run/status/cancel/abort` handler）；`routing_rules.rs:44` 留有空 Effect + TODO（config 事件订阅未接）；`context.rs:1037` 裸 `window().unwrap()`；`Tooltip` 组件、`.skeleton*`、`.aleph-chrome-row-h` 零引用死代码。
- **架构**：深度重构需求**被证伪**——依赖单向无循环、api 层封装一致、context.rs 高内聚（123 处消费者，拆分成本 > 收益）、settings 已有 config_template 数据驱动模板。唯一值得做的是 `views/cron.rs`（1625 行，全仓最大）按既有组件边界纯移动拆分（CLAUDE.md P2）。

## A. 玻璃材质二轮收口（视觉）

统一模式（沿用上轮定稿）：瞬时弹出层卡片 = `glass bg-surface-overlay/85 border border-border` + 现有 shadow；全屏遮罩 = 各自 dim bg + `aleph-scrim` 类（只供 backdrop-filter）。

迁移清单（实现期以 `rg 'fixed inset-0'` 全量审计为准，下列为已确认项）：

| 表面 | 文件 | 现状 |
|------|------|------|
| 会话右键菜单 | `components/chat_sidebar.rs:795` | `bg-surface-raised` 不透明 |
| Project 菜单 | `views/chat/project_menu.rs:181` | `bg-surface-base` 不透明 |
| Skill 详情/删除 modal | `views/settings/skills.rs:546,1012` | `bg-surface` 卡 + 裸 `bg-black/50` 遮罩 |
| Plugin modal | `views/settings/plugins.rs:366` | 同上 |
| MCP modal | `views/settings/mcp.rs:377` | 同上 |
| Cluster join modal | `views/settings/network/cluster.rs:144` | 同上 |
| Connection test modal | `views/settings/network/connection.rs:114` | 同上 |
| 配对 modal | `views/pairing_modal.rs:234` | `bg-surface` 卡 + `bg-black/60` 遮罩 |
| Teams 任务抽屉 | `views/teams/components/task_drawer.rs:192` | `bg-surface` 不透明 |
| Teams 创建表单 | `views/teams/components/create_form.rs:119` | 同上 |

**刻意排除**（记录在案，勿"顺手修"）：
1. `views/chat/composer/voice.rs:427` 错误气泡——danger 高对比度是功能性的（错误可见性 > 材质一致性），保持 `bg-danger`。
2. `views/chat/view.rs:156` 拖拽 overlay `backdrop-blur-[1px]`——上轮已刻意排除（归并到 token 会过磨砂）。
3. `components/ui/tooltip.rs`——零引用，删除而非镀玻璃（见 C4）。

性能红线（沿用上轮）：以上全部是**条件渲染的瞬时表面**（关闭即 unmount、GPU 释放），不新增常驻 backdrop-filter；不加 will-change。

## B. WASM 体积优化（性能）

1. **`[profile.wasm-release]`**（Cargo.toml:497）增加尺寸优化：
   ```toml
   opt-level = "z"
   lto = true
   codegen-units = 1
   ```
   保留 `strip = false`（crash-diagnostics name section，见 2026-06-09 spec）。该 profile 仅 `just wasm` 用 `--profile wasm-release` 选中，不影响 aleph-server。代价：panel 发布构建变慢（fat LTO + cgu=1），可接受（仅 dist 构建用）。
2. **justfile `wasm` recipe** 在 wasm-bindgen 之后加**条件** wasm-opt 步骤：
   ```bash
   if command -v wasm-opt >/dev/null; then
       wasm-opt -Oz -g dist/aleph_panel_bg.wasm -o dist/aleph_panel_bg.wasm
   else
       echo "wasm-opt not found; skipping (brew install binaryen)"
   fi
   ```
   `-g` 保留 name section（不破坏 crash diagnostics）。条件化保证无 binaryen 的环境（含 CI）照常构建。
3. 删死 CSS：`.skeleton/.skeleton-line/.skeleton-line-sm/.skeleton-block/.skeleton-avatar`（tailwind.css:637-656，零引用且含 infinite 动画）、`.aleph-chrome-row-h`（:1246）。
4. 验收：记录优化前后 `aleph_panel_bg.wasm` 字节数。预期 ≥25% 缩减。

**诚实证伪记录**：运行时性能候选全部已健康——canvas rAF 有 IntersectionObserver 停车、订阅全有 on_cleanup、chat timeline 用 keyed For + Memo、will-change 仅 3 处且正当、事件分发无重复 parse。不做虚拟化（典型会话 <100 消息，YAGNI）。

## C. 错误修复与功能连线

1. **C1 routing_rules 事件连线**（`views/settings/routing_rules.rs:44`）：兑现 TODO。Panel 全局已订阅 `config.**`（context.rs:628），后端 routing_rules handler 已发 `ConfigChanged{section:"routing_rules"}` → 到达 Panel 为 topic `config.changed`、data 含 `section`（home.rs:77 已有消费先例）。在视图里 `subscribe_events` 匹配 `topic=="config.changed" && data.section=="routing_rules"` → 重载列表；`on_cleanup` 退订。替换现有空 Effect。
2. **C2 unwrap 修复**（`context.rs:1037`）：`web_sys::window().unwrap()` → `if let Some(w) = web_sys::window() { let _ = w.location().reload(); }`。
3. **C3 删除死文件 `api/agent_run.rs`**（73 行）：`AgentApi` 全仓零消费者；后端 gateway 从未 `register("agent.run"|"agent.status"|"agent.cancel"|"agent.abort")`（method_authz/lane 中的字符串是 authz 表与测试，非 handler）。同步删 `api.rs:19` `pub mod` 与 `:46` `pub use`。
4. **C4 删除死组件 `components/ui/tooltip.rs`**：零引用（types.rs 命中是 doc 注释）；同步删 `ui/mod.rs:21` export。

**不做**（记录）：`agent.abort`→`chat.abort` 改名（死代码直接删，无需改名）；`memory.note.changed`（需后端发事件，超出 Panel 范围）；context.rs 顶部 alerts 重构 TODO（功能正常，技术债注释保留）。

## D. 架构：cron.rs 纯移动拆分 + 重构证伪记录

1. **D1 拆分 `views/cron.rs`**（1625 行 → `views/cron/` 目录，纯移动零行为改动）：
   - `mod.rs` — `CronView` 主容器 + re-export
   - `helpers.rs` — 16 个 format_*/parse_* 纯函数（含其单测）
   - `job_list.rs` — `JobList` + `JobListItem`
   - `job_editor.rs` — `JobEditor`
   - `run_history.rs` — `RunHistory`
   - 可见性最小化：跨文件项用 `pub(super)`，对外仅 `CronView`。验收：`cargo test -p aleph-panel --lib` 数量不减、行为零改动（git diff 只有移动 + use 调整）。
2. **证伪记录**（不做）：context.rs（高内聚 123 消费者）、search.rs（API 贯穿拆分增复杂度）、definitions.rs（纯数据自然增长）、config_template.rs（正是三次法则范例）。监控信号见 gap-analysis：context.rs 近 1500 行、settings 出现第 3 个 load_* 重复时再动。

## 测试与验证

- `cargo test -p aleph-panel --lib`（基线 341，cron helpers 测试随文件移动）
- `cargo check -p aleph-panel --target wasm32-unknown-unknown`
- `cargo clippy -p aleph-panel` 触及文件零新警告
- `just wasm` 成功（leptos_i18n 编译期校验；本轮无 i18n 键改动）+ dist 重建提交（rust_embed 链）
- WASM 尺寸 before/after 记录
- 独立 code-review 终审

## 错误处理

C1 订阅失败走既有 `subscribe_events` 返回路径（与 runtimes.rs 模式一致）；玻璃迁移纯 class 改动无错误面；wasm-opt 条件化保证缺工具时构建不破。

## 部署提醒

dist 嵌入 aleph-server 二进制（rust_embed），改动要在运行中 daemon 生效须重编+替换 binary（部署按惯例 DEFERRED，记录于收尾）。
