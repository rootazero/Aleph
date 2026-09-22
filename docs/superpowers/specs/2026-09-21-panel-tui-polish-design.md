# Panel & TUI 对话窗口打磨设计（2026-09-21）

> **状态**：✅ 设计通过（用户审批 2026-09-21）
> **基线 commit**：`0927461ba`
> **介入级别**：L1 — 仅修 bug + 补连线 + 补缺失测试，不重写大块模块
> **覆盖赛道**：A 消息渲染 + B Composer/输入 + C 运行时状态机 + D 共享契约
> **工作分支**：`panel-tui-polish`（worktree 隔离）

## 1. 背景与目标

Aleph 的 Panel 对话窗口（`interfaces/webchat/src/platform/wide/views/chat/`，~16K 行）和 TUI 对话窗口（`interfaces/tui/src/tui/`，~13K 行）已经具备完整功能（`FEATURE_LOCATOR.md` 中绝大多数条目标 ✅）。
本轮工作的目标是**对用户互动体验做「广而浅」的打磨**——只修 bug、补缺失连线、补缺失测试，不重写模块、不抽新公共面。

参考项目（作为 UX 模式来源）：
- `pi-ask-user`：split-pane、overlay toggle、multi-select、freeform、persistent toggle、keybinding 与 editor 行为、promptSnippet/promptGuidelines、`herdr:blocked` lifecycle、session state 重建 details
- `pi-ask-user-question`：非交互回退、`ctx.ui.select/input` 委托、Escape 兜底
- `pi-claude-code-tui`：工具行折叠、spinner verbs、status bar、shell 块背景、CC 风格 ⏺/⎿ 行
- `desktop-cc-gui`：composer、消息流、附件、@mention、approval 卡片（React + Tauri）

## 2. 设计原则

按 AGENTS.md「Trade-offs > Blind Simplicity」与 CLAUDE.md 架构红线：

| 原则 | 说明 |
|------|------|
| **连线优先** | 编写新代码前先复用现有模块。FEATURE_LOCATOR 中 ✅ 的功能不再造新接口 |
| **不重写** | L1 范围内禁止重写大块模块；只修 bug + 补缺失连线 |
| **熵减** | 每条 fix 同步清理过期/死代码（项目内不允许新增死代码） |
| **分支隔离** | 在 `panel-tui-polish` 分支（worktree）操作，不动 main |
| **commit 粒度** | 每个 fix 一个 commit，message 格式 `<scope>: <description>` |
| **护栏先行** | Phase 0 必须跑通现有测试，Phase 3 必须加 Playwright e2e + TUI snapshot 护栏 |

## 3. 范围（赛道 × 子项）

### A. 消息渲染/对话展示（Panel `messages.rs`/`timeline.rs` + TUI `widgets/chat_area.rs`）

| 子项 | 修复对象 | 参考模式 |
|------|----------|----------|
| A1 | tool 行折叠：长 JSON 行 wrap 后是否刷屏 | `pi-claude-code-tui` 物理行封顶 3 行 |
| A2 | streaming echo 中断恢复（断网/刷屏/重启） | `desktop-cc-gui` 消息流持久化 |
| A3 | reasoning 折叠状态在重连后丢失 | `interfaces/tui/src/tui/widgets/chat_area.rs::reconcile_tools_from_summary` |
| A4 | 附件/图片渲染（无 alt/无尺寸/超长图） | `desktop-cc-gui` `features/chat/AttachmentPreview` |
| A5 | 行间 hover/click/menu（Panel） | `desktop-cc-gui` message-actions |
| A6 | halt/terminate_reason 显示 | `FEATURE_LOCATOR.md` Panel Reads `terminate_reason` ✅（守住不破） |
| A7 | 跨端一致投影（同一条消息在 Panel/TUI 显示一致） | `shared/ui_logic/src/state/` |

### B. Composer/输入交互（Panel `composer/` + TUI `widgets/input_area.rs` + `keys.rs`）

| 子项 | 修复对象 | 参考模式 |
|------|----------|----------|
| B1 | @mention palette 渲染一致性 | `interfaces/webchat/src/platform/wide/views/chat/mention_palette.rs` |
| B2 | 斜杠命令面板的 fuzzy 匹配与高亮 | `interfaces/tui/src/tui/widgets/command_palette.rs` |
| B3 | 多行编辑光标/选区在 Panel/TUI 行为差异 | `pi-claude-code-tui/lib/claude-tui-editor.ts` |
| B4 | attachment 拖拽/粘贴路径（Panel） | `desktop-cc-gui` features/chat/Composer |
| B5 | voice 录音按钮状态机 | `composer/voice.rs` |
| B6 | approval 卡片按键反馈（输入框 + approval 串行） | `interfaces/tui/src/tui/approval.rs` |
| B7 | ask_user / clarification 卡片在对话流中的生命周期 | `pi-ask-user` `herdr:blocked` lifecycle + `RetireOnAbandon` RAII |
| B8 | freeform/structured 切换 + 上下文折叠 | `pi-ask-user` 单选 `singleSelectLayout` + `contextExpanded` |

### C. 运行时状态机（Panel `state/` + TUI `app/`）

| 子项 | 修复对象 | 参考模式 |
|------|----------|----------|
| C1 | `run_phase` 切换边界（`Idle→Streaming→AwaitingApproval→Tool→Done`） | `state/run_phase.rs` |
| C2 | `active_run` 在 attach/rejoin 后残留 | `chat_sidebar.rs::hydrate_and_follow` + `sessions.rs::rejoin_target` |
| C3 | `reconnect reconciliation` 后 live turn 续跑 | `AlephClient::reconnect` + `awaiting_reconnect/next_backoff` |
| C4 | 跨标签页 `frame-to-conversation routing` | `events.rs::resolve_target` |
| C5 | `terminate_reason` 在 Panel/TUI 显示一致 | 同 A6 |
| C6 | TUI `frame_belongs_here` 跨会话防串台 | `tui/app/events.rs::frame_belongs_here` |
| C7 | `Turn Age, Not Join Age`（attach 后计时器从 0 开始） | `chat.history::active_run_elapsed_ms` |

### D. 共享契约与复用（`shared/ui_logic/src/state/`、`aleph_protocol`、事件投影）

| 子项 | 修复对象 | 参考模式 |
|------|----------|----------|
| D1 | SessionKnobs / ComposerSessionDials 在两端共用 | `FEATURE_LOCATOR.md` Composer Session Dials ✅ |
| D2 | 事件投影（`StreamEvent` → Panel/TUI 内部类型） | `events.rs::parse_run_halt` 等 |
| D3 | keymap 共享（hotkey 与 TUI keys 对齐） | `interfaces/webchat/src/components/connection_status.rs` 等 |
| D4 | atomic session metadata write（重启后语义对账） | `gateway/session_store/file_backend/mod.rs::write_metadata` |

## 4. 工作流（4 个阶段）

### Phase 0：基线对齐（必须先做）

**目标**：当前所有测试通过；记录对照基线。

**步骤**：
1. 跑通 `cargo test -p alephcore --lib`（内存受限机器：`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`）
2. 跑通 `cargo test -p webchat --lib`、`cargo test -p tui --lib`
3. 跑通 `pnpm test`（vitest）
4. 跑通 `pnpm e2e`（Playwright）
5. 记录 commit hash、`test-results-baseline.json`、耗时

**退出条件**：所有测试通过。

### Phase 1：审计 + 总表（不写代码）

**目标**：输出 `gap-analysis.md`，列出每个候选 fix 的：文件锚点 + 现象 + 修复建议 + Tier（1/2/3）+ 估计行数。

**步骤**：
1. 静态扫描 4 个赛道（messages、composer、state、shared/ui_logic）
2. 对照 `FEATURE_LOCATOR.md` 中标 ❌/⚠️ 的条目
3. 读参考项目：`/home/zou/mnt/macmini/TBU4/Github/pi-ask-user/`, `pi-ask-user-question/`, `pi-claude-code-tui/`, `desktop-cc-gui/` 的关键 UX 模式
4. 输出 gap-analysis.md，划 Tier

**退出条件**：用户审 gap-analysis.md，砍/加项，划定要做的范围。

### Phase 2：Tier-1 修复（用户最直接感受）

**目标**：修复用户能立刻看到的问题。

**步骤**：
1. 按 gap-analysis.md Tier-1 列表逐项修
2. 每条 fix 配最小测试（单元 or snapshot）
3. commit message 格式：`<scope>: <description>`
4. 跑相关模块测试，确认通过

**退出条件**：Tier-1 全部修完且测试通过。

### Phase 3：Tier-2 修复 + 验证护栏

**目标**：修状态机/事件路由/重连/reconciliation 的 bug；加固测试。

**步骤**：
1. Tier-2 fix 逐项修
2. 加 Playwright e2e 关键路径：发送/停止/重连/会话切换
3. 加 TUI snapshot：chat_area / status_bar / dialog / hint_line
4. 跑全测试，确认无回归

**退出条件**：所有新测试通过，无回归。

> **Tier-3（性能/样式）**：本轮**不做**。Phase 1 审计中如有发现，归入下一轮（重构轮）。

### Phase 4：文档同步

**目标**：FEATURE_LOCATOR.md 与 reference 文档同步更新。

**步骤**：
1. 更新 FEATURE_LOCATOR.md 中本轮修过的条目状态（❌/⚠️ → ✅）
2. 同步 reference 文档（CLAUDE.md、AGENTS.md、reference/ARCHITECTURE.md 如有变更）
3. 出 CHANGELOG 条目

**退出条件**：文档同步完成。

## 5. 验证护栏（按用户选择全开）

| 护栏 | 何时跑 | 通过条件 |
|------|--------|----------|
| 现有测试 | Phase 0、Phase 2 每条 fix 后、Phase 3 | 全绿 |
| 关键路径测试 | Phase 2 / Phase 3（补） | 覆盖 chat_area/messages/events/app/mod 的关键路径 |
| Playwright e2e | Phase 3（加固） | happy path + 关键交互（发送/停止/重连/会话切换） |
| TUI snapshot | Phase 3（加固） | chat_area / status_bar / dialog / hint_line 渲染稳定 |

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 范围蔓延（A+B+C+D 同时修） | Phase 1 输出 gap-analysis 后由用户砍/加项；Tier 划分保证优先级 |
| 跨端行为改动导致回归 | 全护栏验证；commit 粒度细；worktree 隔离 |
| 测试在内存受限机器 OOM | 限流编译：`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1` |
| 改动对话运行时状态机 | Phase 2 不碰 Phase C1/C3/C4；这些只在 Phase 3 修，且需 e2e 守护 |

## 7. 不做（State the Negative）

- **不做**模块级重写（保持 L1）
- **不做**新公共面抽取（D 赛道仅做现有合约的 bug + 连线，不抽新类型）
- **不做**wire protocol 改动（事件 schema 不动）
- **不做**新依赖引入
- **不做**新功能（不实现 FEATURE_LOCATOR 中完全没有的条目）

## 8. 关联文档

- `AGENTS.md`：开发规范、commit 格式、Process Management
- `CLAUDE.md`：架构红线 R1–R10
- `docs/reference/ARCHITECTURE.md`：完整架构
- `docs/reference/CODE_ORGANIZATION.md`：模块/文件模式
- `docs/reference/FEATURE_LOCATOR.md`：功能词典（每个 anchor 都是 ✅/⚠️/❌ 状态）

---

**审批记录**
- 2026-09-21：用户审批设计方案 ✅
- 下一步：调用 writing-plans skill 出 Phase 0 起步的实施计划