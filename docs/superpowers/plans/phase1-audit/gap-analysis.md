# gap-analysis.md — Panel & TUI Polish (Phase 1 审计输出)

> **关联 spec**：`docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md`
> **基线**：`docs/superpowers/plans/phase0-baseline/test-results-baseline.json`（22,567 tests passed，4 pre-existing failures 已文档化）
> **worktree branch**：`panel-tui-polish` @ `558509811`
> **生成时间**：2026-09-21
> **L1 范围**：仅修 bug + 补连线 + 补测试；不重写大块模块、不抽新公共面、不动 wire protocol（spec §7）

## 摘要

| 轨道 | 总候选 | Tier-1（Phase 2） | Tier-2（Phase 3） | Tier-3（本轮跳过） |
|------|--------|-------------------|-------------------|--------------------|
| **A** 消息渲染/对话展示 | 9 | 1 | 3 | 5 |
| **B** Composer/输入交互 | 17 | 7 | 2 | 8 |
| **C** 运行时状态机 | 7 | 1 | 6 | 0 |
| **D** 共享契约 | 4 | 1 | 2 | 1 |
| **合计** | **37** | **10** | **13** | **14** |

**总估行数**：Tier-1 ≈ 250-450 行（其中大部分是补测试，真正行为修复 <80 行）；Tier-2 ≈ 130-200 行（几乎全是加固单测）。

## 关键观察

1. **A1/A2/A3/A5/A6/A7 已被 FEATURE_LOCATOR 6 轮 round 至 2026-08-29 全部覆盖**——本轮 Track A 重点是「守住不破」，**不是新功能**。
2. **唯一真正的 Track A 端到端 gap 是 A4（附件/图片渲染）**，且形态是「两端都缺」。**真图片渲染需要 wire 协议改动（超 L1）**；L1 子集是 composer → 「sent-image」chip（Panel）+ TUI attachment 字段名补全（<40 行/端）。
3. **C1-C7 在 FEATURE_LOCATOR 中全部 ✅**——本次审计验证了「✅ 是真绿，不是死绿」。**Track C 没有新 bug**，候选多为加固单测。
4. **D1.1 是最大可见问题**：TUI 状态条不显示 `model_pin`（5 字段 vs 4 字段不对称），与 Panel 不对称。
5. **B 赛道有 3 处「0 测试」薄覆盖区**（voice.rs、approval_card.rs、clarification/ask.rs 密度低）——任何 L1 修复都必须先补测试再修，否则无从证伪。

---

## Tier-1 清单（待 Phase 2 修复）

> 修复顺序：先补薄覆盖区测试，再修行为 bug。

| # | 轨道 | 文件:行 | 现象 | 修复建议 | 估计行数 | 测试策略 |
|---|------|---------|------|----------|---------|---------|
| **T1.1** | A4 | Panel `composer/attachments.rs::PendingAttachment` + `messages.rs::MessageBubble` user 臂 / TUI `TranscriptEntry::UserText` | 用户发图后看不到图：Panel `m.content` 只有文本不渲染附件；TUI `UserText` 无 attachment 字段 | **L1 子集**：composer 端将 `PendingAttachment` 在消息发送后保留为「sent-image」chip（约 30-40 行/端） | 30-40/端 | snapshot：composer 有 attachment 时消息历史里出现对应 chip |
| **T1.2** | B | `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:1078-1118` `on:keydown` Enter 分支 | **中文 IME 候选窗误触发 send**：候选提交时 `isComposing=false`，Enter 触发 send。desktop-cc-gui 用 100ms "recently settled" window | 加 `compositionstart/compositionend` listener + 100ms 闸 | <20 | 单测：模拟 compositionstart → keydown(Enter) → 不应 send |
| **T1.3** | B | `composer/attachments.rs:144` `<For key=...>` 移除附件时 idx 漂移 | 用户点中间 chip ✕，Leptos 复用错位节点，下一帧错 chip 消失 | key 改为稳定 hash（`name + size`） | <10 | snapshot：移除中间附件后剩余附件顺序正确 |
| **T1.4** | B | `composer/voice.rs:455-475` 录音状态机 | 录音中按 Stop 触发 ghost finish；`finish` 在 state==Starting 调用 `recorder.stop()` on `None`；**0 测试覆盖** | **先补状态机测试**（5-8 个单测，约 30-50 行），**再修复**：timer 清理 + `if state==Starting return` 闸 | 测试 30-50 + 修复 15 | 状态机 5-8 个用例 |
| **T1.5** | B | `interfaces/webchat/src/components/approval_card.rs` 257 行 **0 测试** | 无任何对渲染倒计时、按钮事件、deny-with-reason 的覆盖 | 补组件级 snapshot + `resolve()` 单测 | 100-200 | snapshot 4-6 个 + 单测 2-3 个 |
| **T1.6** | B | `clarification/ask.rs` 743 行 / 10 测试（密度 1/74） | 关键路径未覆盖：4 问题顺序、超时 cleanup_expired 级联 reap、Panel turn 的 `withheld_secret` 形状 | 补 5-10 测试覆盖多 question、超时、撤回场景 | 200-400 | 单测 5-10 个 |
| **T1.7** | B | `composer/mod.rs::send_message` 团队分支 | 团队聊天粘贴附件 → Enter → 看到红色 banner，**但附件 chip 已清空**（与单聊 `failed_send_restores_the_tray` 形状相反） | 团队分支失败时也走 `seed_draft(files)` 恢复 | <20 | 单测：团队 send 失败后 attachment tray 仍含原文件 |
| **T1.8** | C | `interfaces/tui/src/tui/mod.rs:531-534` `subscribe_runtime_agents` | 重连成功后 `subscribe_runtime_agents` 失败时**静默**（错误只 warn，不重试），面板永久冻结 | 失败时设 `state.runtime_agents_refetch_due=true` 走已有重试机制 | <20 | 单测：模拟 subscribe 失败 → 下个 main_loop tick 重试 |
| **T1.9** | D | `interfaces/tui/src/tui/app/mod.rs:648` 本地 `SessionKnobs<'a>` 4 字段 | TUI 状态条**丢掉 `model_pin` 字段**（Panel 5 字段含 model_pin，TUI 仅 4 字段）。FEATURE_LOCATOR §5.23b 已删 Panel 的 SessionRow 但**没删 TUI 的本地 4 字段** | TUI 状态条新增 model_pin 显示（与 Panel `dial_picker` 一致） | <20 | snapshot：设置 pin → TUI 状态条显示模型名 |
| **T1.10** | A4 | Panel `messages.rs::MessageBubble` user 臂（与 T1.1 关联） | user bubble 只显示 `m.content` 文本，不触发 markdown/图片渲染 | 同 T1.1：composer chip + 已发送附件在历史中显示 chip 形态（不要求真图片渲染） | 含 T1.1 | 含 T1.1 |

---

## Tier-2 清单（待 Phase 3 修复）

> 几乎全部是「加固单测」或「冗余防御」，无行为修复。

| # | 轨道 | 文件:行 | 现象 | 修复建议 | 估计行数 | 测试策略 |
|---|------|---------|------|----------|---------|---------|
| **T2.1** | A2 | `panel events.rs` / `tui app/events.rs` | streaming echo 中断恢复**端到端未测过**（FEATURE_LOCATOR ✅ 但只有单元级） | Phase 3 加 Playwright e2e：发送 → 强断网 → 重连 → 续跑内容完整 | Playwright spec 5-10 步 | Playwright |
| **T2.2** | A | TUI `tui/app/events.rs::halt_notice` 启动期 locale | 启动期 locale 未确定时 `halt_notice` 走 fallback | 默认 locale 兜底逻辑 + 测试 | <20 | 单测 2-3 |
| **T2.3** | A | wire `summary` 不带 attachment | 已知；超 L1 范围 | — | — | — |
| **T2.4** | B | `widgets/command_palette.rs` snapshot 缺失 | 边框、focus、选中高亮、空 namespace 标题无测试 | 补 ratatui TestBackend snapshot（参考 `widgets/input_area.rs`） | 30-60 | snapshot 3-5 |
| **T2.5** | B | `commands.rs` sent-history 滚动 | sent-history 翻页边界未测 | 加翻页边界测试 | <20 | 单测 3 |
| **T2.6** | C | Panel `state/reattach.rs:67-83` | `reattach_after_connect` 在 `run_concurrency` 失败时只 return，不触发再次 reattach；失败后**永远不恢复** | 失败时设 `pending_reattach: RwSignal<bool>`，由下次 `connection_epoch` 触发 | 30-50 | 单测：模拟 RPC 失败 → 下个 epoch 自动重试 |
| **T2.7** | C | Panel `context.rs::connection_epoch` | 单调性无显式单测 | 加单测：connect→disconnect→reconnect，断言 epoch +3 | <20 | 单测 1 |
| **T2.8** | C | Panel `state/sessions.rs:370-382` `bind_run` | 重复调用可能双计 `running`（当前无 race 触发） | 入口加 `if route.contains(run_id) return;` + 单测 | +1-3 | 单测 1 |
| **T2.9** | C | TUI elapsed 计时器在 run 完成时是否停在正确值 | **当前行为是「RunComplete 时 run_started_at 被清为 None，elapsed 显示消失」** | 审前确认产品意图；若是 bug 则修，否则加单测钉行为 | <20 | 单测 1 |
| **T2.10** | C | Panel `state/run_phase.rs` `mark_queued` 乱序单测 | 同 run_id `run_accepted` 在 `run_queued` 之后的相位覆盖未测 | 加单测 | <20 | 单测 1 |
| **T2.11** | C | Panel 跨会话切回端到端单测 | 切到 s2 再切回 s1 时 `route_lookup` / `is_running` 未测 | 加单测 | <20 | 单测 1 |
| **T2.12** | D | Panel string-dispatch 投影 | `events.rs::parse_run_halt` 等 13 处 string 分支无静态覆盖断言 | 加`every_panel_string_dispatch_variant_has_a_unit_test`类似守护 | +20-40 | 单测 1（守卫式） |
| **T2.13** | D | Panel 端 `StreamEvent::ReasoningBlock` / `UncertaintySignal` 未消费 | TUI 已处理，Panel 未消费（grep 0 hit） | 与产品确认是否真为事件需要；若需要则补消费；若不需要则文档化「Panel 不显示」 | TBD | TBD |

---

## Tier-3 清单（本轮跳过，下轮重构）

> 不在 L1 范围。集中于：wire 协议改动、跨端大重构、新功能。

| # | 轨道 | 标题 | 不做的原因 |
|---|------|------|-----------|
| T3.1 | A3 | reasoning 跨会话持久化 | 需要新增 wire 字段，超 L1 |
| T3.2 | A4 | 真图片渲染（Panel data URL + ImageLightbox；TUI ratatui image protocol） | 需要 wire 协议 + 多个 surface 改动，超 L1 |
| T3.3 | A | pi-claude-code-tui 的 spinner glyph + 190 动词 | 视觉/样式，spec Tier-3 性能/样式 |
| T3.4 | A | desktop-cc-gui `MessageAnchorRail` / `CollapsibleMessage` / `ToolPayloadViewer` | 重构级新功能，超 L1 |
| T3.5 | A | Panel `messages.rs::halt_view` user bubble 中残留 `run_for_halt` 派生 | 现状不破，dead-ish code |
| T3.6 | B1 | TUI 独立 @mention overlay | 新功能，超 L1 |
| T3.7 | B2 | fuzzy 匹配 | 体验而非 bug |
| T3.8 | B | pi-ask-user 的 split-pane / overlay toggle / contextExpanded / allowComment / singleSelectLayout / displayMode | 新功能（7 项均属新功能） |
| T3.9 | B | TUI 半开圆角边框 + 闪烁光标 + 滚动提示 | 视觉/样式 |
| T3.10 | B | Panel mention palette 选中保持 | 体验微调 |
| T3.11 | B | TUI Dialog 选项 >3 时滚动指示 | 边缘场景 |
| T3.12 | B | TUI 上箭头光标 vs 历史浏览边界 | 设计意图 |
| T3.13 | C | 无 Tier-3 | — |
| T3.14 | D | `truncate_messages` / `retire_from` / `restore_checkpoint` / `branch_from_checkpoint` operator-initiated 重写不持 `lock_metadata` | 已知 gap 但已被代码注释标注为「bounded gap」 |

---

## 范围闸（scope gate）

**每条候选是否落在 A/B/C/D 四个赛道之一**：✅ 全部命中。

**每条是否在 L1 范围**（仅修 bug + 补连线 + 不重写）：

| 候选类型 | 在 L1 范围？ | 处理 |
|----------|---------------|------|
| 修 bug（行为不正确） | ✅ Y | 进 Tier-1 |
| 补测试（覆盖率薄） | ✅ Y | 进 Tier-1（先补后修） |
| 加固单测（行为正确但缺护栏） | ✅ Y | 进 Tier-2 |
| 加冗余防御（无 race 触发但稳） | ✅ Y | 进 Tier-2 |
| 重写大块模块 | ❌ N | 移到下轮 |
| 抽新公共面 / 新类型 | ❌ N | 移到下轮 |
| 改 wire protocol | ❌ N | 移到下轮 |
| 新功能（新 UX 模式） | ❌ N | 移到下轮 |
| 视觉/样式打磨 | ❌ N | 移到下轮 |

**最终 L1 范围**：Tier-1 (10 项) + Tier-2 (13 项) = 23 项，总估 ≈ 400-650 行（含测试）。

---

## 范围裁决记录

### Phase 2 起点推荐

**先做 T1.4 + T1.5 + T1.6**（voice / approval_card / ask.rs 测试补强）—— 薄覆盖区先填，否则后续修 Tier-1 bug 无快速反馈。

**再做 T1.8 + T1.9 + T1.2 + T1.3**（小、行为明显）：
- T1.8 TUI 重连重试（<20 行）
- T1.9 TUI SessionKnobs 缺 model_pin（<20 行）
- T1.2 IME 闸（<20 行）
- T1.3 attachment For key（<10 行）

**最后做 T1.1 + T1.7 + T1.10**（A4 附件 chip + 团队附件恢复）：
- A4 chip 是用户最直观看得到的修复
- 团队附件恢复是与 T1.7 验证同一个 `failed_send_restores_the_tray` 形状

**总 Tier-1 工作量**：约 30% 测试补强（薄覆盖区）+ 70% 行为修复（含 chip 形态）。

### Phase 3 重点

- T2.1 Playwright e2e（streaming echo 续跑）
- T2.6 Panel reattach 重试
- T2.4 + T2.5 TUI snapshot 补强
- 其余 Tier-2 加固单测

---

## 与 baseline 的对应

**baseline Phase 0 发现的 pre-existing failures**（不属于本轮）：

| 失败项 | 归属 | 是否纳入本轮 |
|--------|------|----------------|
| `capability::census::tests::every_installed_global_is_a_capability_slot` | capability 盘点 | 否（文档预期） |
| `tools::concurrency::tests::windows_separators_fold_onto_the_same_scope` | Windows-only | 否（平台限制） |
| `verification::extension_stop_gate::tests::consecutive_veto_ceiling_unwedges_the_loop` | 真 bug，**候选 Tier-2** | 待 Track C 5 扩展时纳入；本轮优先 23 项 |
| `tui::widgets::header::tests::the_version_comes_from_the_version_file_not_the_cargo_copy` | test infra bug | **候选 Tier-1**（<5 行：改用 `env!("CARGO_MANIFEST_DIR")`）— **追加为 T1.11** |

**追加 T1.11**：TUI header VERSION path test。修复用绝对路径替换相对路径。

---

## 关联文档

- Spec: `docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md`
- 计划: `docs/superpowers/plans/2026-09-21-panel-tui-polish.md`
- Baseline: `docs/superpowers/plans/phase0-baseline/`
- 各轨道详细 findings:
  - `track-a-findings.md` (237 行)
  - `track-b-findings.md` (161 行)
  - `track-c-findings.md` (230 行)
  - `track-d-findings.md` (223 行)
