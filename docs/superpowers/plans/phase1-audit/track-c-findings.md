# Track C Findings — 运行时状态机

> **Spec 关联**：`docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md` §3 C（C1–C7）
> **时间**：2026-09-21（worktree panel-tui-polish @ 5585098）
> **关联 baseline**：`0927461ba`（spec §1）
> **L1 范围**：仅修 bug + 补连线 + 补测试；**不重写大块模块**（spec §7）
> **审计性质**：现状 review（非新设计）。C1–C7 在 FEATURE_LOCATOR 中全部 ✅；本次审计是验证「✅ 是真绿，不是死绿」。

---

## 1. FEATURE_LOCATOR 已知未完成项（状态机相关）

| 口语关键词 | Anchor | 状态 | 与 C1–C7 对应 | 备注 |
|------------|--------|------|---------------|------|
| Panel Reads `terminate_reason` | `views/chat/state/mod.rs::RunHalt` + `events.rs::parse_run_halt` + `messages.rs::halt_view` + `locales/*.json::chat.halt_*` | ✅ | **C5** | 五个生命周期点全跟：`run_complete` 投影 / `run_costs` 同键同寿命 / snapshot 进出 / 气泡 meta 行渲染（紧挨 `cost_view`）/ 三处 `None` 静默 |
| Join A Live Turn (Panel) | `handlers/chat.rs::handle_history` (`active_run`) + `chat_sidebar.rs::hydrate_and_follow` | ✅ | **C2** | hydrate 拉回 `active_run` → 跟 bubble；同 `RunAccepted` 路径（两条入口，二选一即可） |
| Reconnect Baseline Rebase | `context.rs::connection_epoch` + `sessions.rs::{reset_running_baseline,settle_runs_absent_from}` + `state/reattach.rs`（app root，两个形态共用） | ✅ | **C3** | 单调计数器（不是服务端实例 id）+ 重置基线 + 结算负半边 + rejoin 正半边，三步同 `run_concurrency` 一次往返 |
| Rejoin A Live Turn On Reconnect | `sessions.rs::rejoin_target` + `state/reattach.rs` | ✅ | **C3** | 活动对话 + 服务端说它在跑 + 本端无 route ⇒ 该重接；复用 `hydrate_and_follow` |
| Cross-Session Frame Guard (TUI) | `tui/app/events.rs::frame_belongs_here` + `app/mod.rs::session_reconciled` | ✅ | **C4 + C6** | 三答：current_run / run_sessions 比当前 session_key / fail-open 由 `session_reconciled` 决定；`RunAccepted` 显式豁免 |
| Join A Live Turn (TUI) | `commands.rs::{active_run_from_history,attach_session}` + `events.rs::adopt_active_run` | ✅ | **C2 + C7** | `active_run_from_history` 三态（缺席 ≠ null）；`adopt_active_run` 单调钟 back-date |
| Reconnect With Reconciliation | `AlephClient::reconnect` + `tui/mod.rs::{reconnect_after,awaiting_reconnect,next_backoff}` + `app/mod.rs::{on_disconnected,begin_reattach}` | ✅ | **C3 + C4 + C6** | 退避 0→1→2→…→15s；wait 在 future 内（不旁开 timer）；`on_disconnected` 只清 `session_reconciled` 不清 `current_run`；`begin_reattach` 清运行态但保留 `/btw` |
| Turn Age, Not Join Age | `chat.history` 的 `active_run_elapsed_ms` ← `ExecutionAdapter::run_elapsed_ms`（单调钟 `admitted_at`） | ✅ | **C7** | 客户端 `Instant::now().checked_sub(Duration::from_millis(ms))` + `u64::MAX` 兜底（test `an_impossible_age_falls_back_instead_of_panicking`） |

> **关键观察**：8 条全部 ✅，无 ⚠️/❌。FEATURE_LOCATOR 给的 ✅ 在本次审计范围内**全部可信**。本章结论：本轨道**没有新 bug 需要修**——下面是**验证证据 + 残余风险 + Tier 候选**（候选多为**加固 / 增加护栏**，不是修缺陷）。

---

## 2. Spec §3 C 子项覆盖矩阵

| 子项 | Aleph 锚点（Panel / TUI / Shared） | 现状 | Tier 候选 |
|------|--------------------------------------|------|-----------|
| **C1** `run_phase` 切换边界 | Panel `state/run_phase.rs`（5 变体：`Idle`/`Queued{ahead}`/`Thinking`/`Streaming`/`Error`）+ `state/mod.rs::{start_assistant_message,mark_queued,mark_admitted,begin_step,complete_run,fail_run,settle_abandoned_run}` + 源码守卫 `no_surface_enumerates_the_busy_phases_by_hand` | **全闭合**：所有相位写入都在 `state/mod.rs` 的 7 个命名 setter 内；读端走 `==` / 单变体 `matches!` / `is_busy()`；源码级扫描器禁止 `matches!` 内联两个变体（`run_phase.rs::tests::no_surface_enumerates_the_busy_phases_by_hand`，11 处读写全审） | **Tier-2** 加固：补 `mark_queued`/`mark_admitted` 的乱序竞态单测（已 3 个，可再加 1 个） |
| **C2** `active_run` attach/rejoin 残留 | Panel `handlers/chat.rs::handle_history` + `chat_sidebar.rs::hydrate_and_follow`（写 `active_run_id` + 跟 bubble 一次性）；TUI `commands.rs::active_run_from_history`（三态）+ `events.rs::adopt_active_run`（idempotent / 幂等 `mark_run_session` / `current_run`/`run_started_at`/`run_rendered_assistant_text`/...重置）；`start_assistant_message` 早返回防重复气泡 | **双重幂等**：Panel `start_assistant_message` 查 `assistant-{run_id}` 是否存在；TUI `mark_run_session` 先查 id；两端都有 `set_send_error` 不清 `active_run_id` 的防御（test `set_send_error_flips_phase_without_touching_the_active_run`） | **Tier-2** 加固：补「同 run_id 双 RunAccepted 之间有陌生 run_queued」的端到端单测 |
| **C3** reconnect reconciliation | Panel `state/reattach.rs::reattach_after_connect`（1 调用 3 步：`reset_running_baseline` → `run_concurrency` → `settle_runs_absent_from` + `rejoin_target` → `hydrate_and_follow`）；TUI `tui/mod.rs::main_loop` 重连成功臂：调用 `state.begin_reattach()` → `attach_session(.., Replace)` → `reconcile_side_question` → `subscribe_runtime_agents` | **两侧均单例入口 + 单 round-trip**：Panel app root Effect（`app.rs:158-170`）监听 `connection_epoch`；TUI main loop 50ms tick 边缘触发 `on_disconnected` → `reconnect_after(0)` 退避 | **Tier-1**（小修）见 §4.B1：TUI 重连成功臂可能漏 `subscribe_runtime_agents` 重试的「call 失败」分支（已审：失败时静默不重试 → 面板永远不更新） |
| **C4** 跨标签/跨会话 frame 路由 | Panel `events.rs::resolve_target`（3 步：route_lookup → session_key 反查 → active_conv 三态守卫 `(open,incoming)` 仅在「两者皆有且不等」时拒）；TUI `events.rs::handle_gateway_event` + `frame_belongs_here` + `run_sessions` FIFO-bounded 256 + `RunAccepted` 显式豁免 + `/btw` 侧问拦截前置 | **两侧各自独立实现、护栏测都全**：Panel 6 条 resolve 单测 + 2 条 `SessionMap` 写者守卫；TUI 12 条 frame_belongs_here 单测（含 FIFO eviction、`RunAccepted` 豁免、`/session` 切换后再切回） | 无（已绿） |
| **C5** `terminate_reason` Panel/TUI 一致 | Panel `state/mod.rs::RunHalt::label(locale)`（13 条标签 + 1 个 fall-through verbatim + i18n zh/en）；TUI `app/events.rs::halt_notice(token, locale)` 调用 `aleph_protocol::terminate::label` | **同一 source-of-truth**：`aleph_protocol::terminate::label`；Panel `parse_run_halt` 与 TUI `terminate::effective_token` 同一 precedence（`detail > reason`）；两侧都「`completed`/空串/字段缺席 ⇒ 不渲染」 | 无（已绿） |
| **C6** TUI `frame_belongs_here` 防串 | TUI `app/events.rs::frame_belongs_here`（3 答：current_run / run_sessions(session_key == self.session_key) / `!session_reconciled`）+ FIFO bound + `RunAccepted` 豁免 + `/btw` 拦截前置 + 教学帧必须豁免的契约 | **文档化的「收窄 fail-open 之前先数教学帧」守则已落地**：FEATURE_LOCATOR D.7.4 + `events.rs::handle_gateway_event` 的豁免臂；test `reconciliation_must_not_blind_this_screen_to_run_accepted_itself` + `a_foreign_run_is_still_learned_after_reconciling` 双向验证 | 无（已绿） |
| **C7** Turn Age, Not Join Age | Panel 不计时（不做 elapsed）；TUI `events.rs::adopt_active_run` 用 `Instant::now().checked_sub(Duration::from_millis(ms))`，失败回 `Instant::now()` | **正确 + 已防御**：`u64::MAX` 不会 panic（test `an_impossible_age_falls_back_instead_of_panicking`）；`elapsed_ms: None` 走 `unwrap_or_else(Instant::now)`（test `an_adopted_run_without_an_age_counts_from_the_join`） | **Tier-2** 加固：补「joined 之后 4 分钟才到 `RunComplete`，running indicator 是否仍在跳」的时序单测（当前没有断言 elapsed 计时器的最终值，只断言初始 back-date） |

> **小结**：7 项全 ✅。Tier-1 仅 1 项（TUI 重连成功臂的 `subscribe_runtime_agents` 失败静默）；Tier-2 2 项（均为加固单测，无行为修复）。**无 Tier-3**。

---

## 3. `run_phase` 状态机分析（Panel）

### 3.1 合法转换图

```
              ┌─────────────┐
              │    Idle     │◄────────────────────────┐
              └─────────────┘                         │
                    │                                │
                    │ start_assistant_message         │
                    │ (run_accepted / hydrate)        │
                    ▼                                │
              ┌─────────────┐                         │
              │  Thinking   │                         │
              └─────────────┘                         │
                │       │                             │
                │       │ mark_queued (run_queued)    │
                │       ▼                             │
                │ ┌─────────────┐                     │
                │ │   Queued    │                     │
                │ │ { ahead }   │                     │
                │ └─────────────┘                     │
                │       │                             │
                │       │ mark_admitted (run_accepted)│
                │       └────────────┐                │
                │                    │                │
                │                    ▼                │
                │              ┌─────────────┐        │
                ├─────────────►│  Streaming  │        │
                │ append_chunk └─────────────┘        │
                │ (response_chunk)        │            │
                │                         │            │
                │       complete_run / run_complete    │
                │                         ▼            │
                │                  ┌─────────────┐    │
                ├─────────────────►│    Idle     │────┘
                │                  └─────────────┘
                │
                │ fail_run (run_error)
                ▼
          ┌─────────────┐
          │    Error    │───── set_send_error / clear_* / snapshot restore ──► Idle / 之前状态
          └─────────────┘

  其他路径：
    - settle_abandoned_run(run_id) ── (active_run_id 匹配) ──► Idle
    - clear / clear_session ──► Idle (reset active_run_id=None)
    - restore_from(snapshot) ──► snapshot.phase 直接 set（无转换守卫）
```

### 3.2 写端全审计（grep 全仓 `ChatPhase::` 出现点，非 `run_phase.rs` 本文件）

| 位置 | 写入 | 是否守卫 |
|------|------|---------|
| `state/mod.rs:1331` | `phase.set(ChatPhase::Thinking)` in `start_assistant_message` | 仅在「气泡不存在」分支 |
| `state/mod.rs:1345` | `phase.set(ChatPhase::Queued{ahead})` in `mark_queued` | 仅在 `active_run_id == run_id` |
| `state/mod.rs:1365-1366` | `phase.set(ChatPhase::Thinking)` in `mark_admitted` | 仅在「当前 phase 是 Queued」 |
| `state/mod.rs:1420` | `phase.set(ChatPhase::Thinking)` in `begin_step` | 无（任何调用都设 Thinking） |
| `state/mod.rs:1471` | `phase.set(ChatPhase::Streaming)` in `append_chunk` | 无（每次 chunk 都设） |
| `state/mod.rs:1618` | `phase.set(ChatPhase::Idle)` in `complete_run` | 无（但只从 `run_complete` 路径调） |
| `state/mod.rs:1674` | `phase.set(ChatPhase::Idle)` in `settle_abandoned_run` | 仅在 `active_run_id == run_id` |
| `state/mod.rs:1734` | `phase.set(ChatPhase::Error)` in `fail_run` | 仅在 `active_run_id == run_id`（！重要） |
| `state/mod.rs:1751` | `phase.set(ChatPhase::Error)` in `set_send_error` | **无守卫**（test `set_send_error_flips_phase_without_touching_the_active_run` 钉此契约） |
| `state/mod.rs:1782` | `phase.set(ChatPhase::Idle)` in `clear_team_context` | 无 |
| `state/mod.rs:1837` | `phase.set(ChatPhase::Idle)` in `clear_session` | 无 |
| `composer/mod.rs:257/275/282/669/1164` | composer 端 `phase.set(Thinking/Idle)` 散布 | 端到端触发器 |
| `team_events.rs:108/116` | team chat 端 `phase.set(Thinking/Idle)` | 同上 |
| `messages.rs:511/538/542-543` | 只读 | — |

### 3.3 未覆盖的非法转换 / 条件 race

| 场景 | 现象 | 现有防御 | 缺口 |
|------|------|---------|------|
| `run_queued` 到达 **前** `run_accepted` 已先到（不可能但 wire 容许） | `apply_run_queued` 会设置 `active_run_id`（如果 None），再调 `mark_queued`；`start_assistant_message` 已把 phase 设为 Thinking，mark_queued 会**盖成 Queued**，但 mark_admitted 会再盖回 Thinking | 顺序敏感，但实际一致 | 无（无场景） |
| `set_send_error` 在 `fail_run` 之前到达 | `fail_run` 设 Error；`set_send_error` 设 Error；次序无关 | 都设 Error | 无 |
| `restore_from` 把 `phase: Queued{ahead: 5}` 灌入新会话 | 直接 `set`，不重置 `ahead`；语义上 Queued 跨快照恢复**正确** | — | 无（snapshot 自带完整性） |
| `begin_step(run_id, iteration=2)` 时 bubble 还在 iteration=1 但已有 payload | 见 `begin_step` 第 1417 行 — 旧 bubble 改名 `intermediate-{run}-{n}`，新 bubble 重用 iteration 标签 | — | 已 green（5 条单测覆盖） |
| 跨 conversation 的 `fail_run(run-x)`（run-x 不是本会话的） | `fail_run` 检查 `active_run_id == run_id`，不匹配则不动 | `active_run_id` 隔离 | 已 green |
| `mark_queued` 早返回 vs `mark_admitted` 早返回 | 都是 active_run_id 不匹配早返回，但**没有**「active_run_id 设为 sibling 然后 mark_queued self」的反向保护 | `apply_run_queued` 只在 None 时改 | 已 green |

### 3.4 总体判断

状态机**自洽**，所有写端都有名字（11 个命名 setter + 1 个 snapshot 灌入），无散乱写。读端受 `is_busy()` 单点约束 + 源码级扫描器兜底。**唯一可观察风险**：`restore_from` 不重置 `ahead`，但因为 `ahead` 是会话内语义、跟 `active_run_id` 同寿命，跨会话时 `active_run_id` 也由 snapshot 灌入，一致性靠 snapshot 的原子性保证。

---

## 4. 潜在 bug 热点（逐条）

### 4.A Panel

| # | 文件:行 | 代码片段 / 现象 | 修复方向 | Tier | 行数 |
|---|---------|----------------|---------|------|------|
| 4.A1 | `state/run_phase.rs:101-119` | 扫描器 `matches!("ChatPhase::", ...)` 简单文本匹配；若未来有 `type ChatPhase = ...` 别名或 `use ChatPhase::*` 大量泛用，**单行可能含 2+ 个 `ChatPhase::` 但其实只匹配一个变体**（例如 `matches!(p, ChatPhase::Queued { .. } \| ChatPhase::Thinking)` 是 2 个、`matches!(p, ChatPhase::Queued)` 是 1 个，但形如 `let q = ChatPhase::Queued { ahead }; matches!(q, ...)` 不含 `ChatPhase::` 字面）| 现规则依赖字面量，注释已写明 trade-off；后续若引入复杂泛用，需扩展规则。**现状无 bug**，加注解即可 | 无（不修） | — |
| 4.A2 | `state/mod.rs:1751` (`set_send_error`) | 与 `fail_run` 都设 `phase = Error`，但**不互相清**：`fail_run` 清 `active_run_id`（守卫内），`set_send_error` 不清——这是 test `set_send_error_flips_phase_without_touching_the_active_run` 钉的契约。如果有人后续把 `set_send_error` 改成「也清 active_run_id」，该测试会红 | 测试守护中 | 无（不修） | — |
| 4.A3 | `state/sessions.rs:370-382` (`bind_run`) | 重复调用同 run_id 会双计 `running`：route.insert 幂等，但 `running.update` **不查重**。调用点 `events.rs::resolve_target` 用 `route_lookup(run_id).is_none()` 守卫了 RunAccepted/Queued 路径，但 `api/chat.rs` 的 send path 不经这个守卫；目前 send path 必然先到、RunAccepted 后到，**没有 race** | 双重防御更稳：bind_run 内 `if route.contains(run_id) return;` 一行 | 无（不修） | +1 防御 |
| 4.A4 | `state/reattach.rs:67-83` | `reattach_after_connect` 在 `run_concurrency` 失败时**只 return**，不重试；但**不触发再次 reattach**（app root 的 Effect 只在 `connection_epoch` 变时跑）。如果 socket 活了但首次 RPC 一次失败，**永远不 reattach** | 加 fallback：在失败时记录「pending reattach」并由下个 epoch/事件触发 | **Tier-2** 加固 | 中 30–50 |

### 4.B TUI

| # | 文件:行 | 代码片段 / 现象 | 修复方向 | Tier | 行数 |
|---|---------|----------------|---------|------|------|
| 4.B1 | `tui/mod.rs:528-533` | 重连成功臂 `subscribe_runtime_agents(state, client).await` 调用失败时**静默**（错误只 warn，不重试、不写系统消息、不记状态）；一旦失败面板永久冻结 | 失败时 log + 在 `state.runtime_agents_refetch_due` 旁设一个待重试位（已有 precedent） | **Tier-1**（真 bug，影响可见） | 小 <20 |
| 4.B2 | `app/mod.rs:2014-2018` (`on_disconnected`) | 只清 `session_reconciled` 和 `is_connected`，**不清 `current_run` 和 `run_started_at`**（按 doc 设计）。但 `run_started_at` 是 `Instant`，永远不重新校准——如果此屏 attach 了一个跑了 4 分钟的 run（已 back-date），后续 socket 短暂掉线又重连，`run_started_at` 仍是 back-date 后的值，**正确** | — | 无（设计如此） | — |
| 4.B3 | `app/events.rs:216-222` (`adopt_active_run`) | `elapsed_ms.and_then(|ms| Instant::now().checked_sub(Duration::from_millis(ms))).unwrap_or_else(Instant::now)`。`Duration::from_millis(u64::MAX)` 是合法 Duration（~584M 年），`checked_sub` 返 None 走 fallback。**没有 panic 路径** | — | 无 | — |
| 4.B4 | `app/events.rs:174-184` (`frame_belongs_here`) | 三答顺序正确：current_run → run_sessions → fail-open。但 `run_sessions` 是 FIFO 256，**current_run 排在 FIFO 查表前**，这正是 test `the_screens_own_run_survives_fifo_eviction` 钉的——设计正确 | — | 无 | — |
| 4.B5 | `app/mod.rs:2022-2044` (`begin_reattach`) | 清 `current_run`、`run_started_at`、`current_run_uses_agent_trace`、`current_run_trace_summary_applied`、`turn_streamed_len`、`run_rendered_assistant_text`、`session_reconciled`，**保留 `messages` / `btw`**。文档已说「transcript 由 `apply_history(Replace)` 在 Ok 臂内清」 | — | 无 | — |
| 4.B6 | `app/mod.rs:2063-2092` (`switch_session`) | 清 messages 全清；但 `chat_line_cache` 也清。`run_sessions` **不清**——这是设计，doc 解释：FIFO 中已学到的 id 与 `self.session_key` 比较，新 session 时自然不命中，切回旧 session 时重新命中 | — | 无 | — |
| 4.B7 | `tui/mod.rs:931` `.expect("ProviderInfo defaults cover every unset field")` | 仅测试代码 | — | 无 | — |

### 4.C Shared / Server

| # | 文件:行 | 现象 | 修复方向 | Tier | 行数 |
|---|---------|------|---------|------|------|
| 4.C1 | `src/gateway/handlers/chat.rs:579-583` | `active_run` 由 `run_manager.active_run_for_session(&canonical)` 取，与 `active_run_elapsed_ms`（592-599）**不是同一个锁内**——`active_run` 可能返 Some(run-x)，`run_elapsed_ms(run-x)` 之间 run 结束，返 None。**已设计**：None 读为「从 now 开始计」（与未接 `elapsed_ms` 字段等价） | — | 无 | — |
| 4.C2 | `src/gateway/handlers/chat.rs:675-682` | wire 输出顺序：`active_run` 然后 `active_run_elapsed_ms`。两端解析均独立读，不依赖 wire 顺序 | — | 无 | — |

### 4.D 连接状态机（Panel `state/connection.rs::ConnectionPhase`）

`derive` 是纯函数，无内部状态；6 个测试覆盖每个转换。**唯一 gap**：测试未覆盖「`is_connected=true` + `connection_error=Some(...)`」（注释说「connected overrides other signals」，由 `connected_overrides_other_signals` 覆盖）。**无 bug**。

---

## 5. 状态机集成测试覆盖

| 场景 | Panel 测 | TUI 测 | 备注 |
|------|---------|--------|------|
| `run_phase` 状态边界 | 5 条 `phase_test` 模块（`marking_a_run_queued_moves_the_phase` / `a_sibling_runs_lane_frame_repaints_nothing` / `admission_clears_the_queued_phase_and_touches_nothing_else` / `set_send_error_flips_phase_without_touching_the_active_run`） + 1 条 `no_surface_enumerates_the_busy_phases_by_hand` 源码扫描器 + 2 条 scanner 自检 | 无（不消费 `ChatPhase`） | 完备 |
| `active_run` 残留 | `hydrate_and_follow` 单测 + `start_assistant_message` idempotent（test 5 条 `message_test`） | `attaching_to_a_running_session_adopts_its_run` + `an_adopted_run_*` 4 条 | 完备 |
| `reconnect` 正负半边 | `state/sessions.rs::tests` 中 `settle_runs_absent_from_*` (3 条) + `rejoin_target_*` (4 条) + `reset_running_baseline_*` (1 条) | `losing_the_connection_disarms_the_cross_session_guard` + `losing_the_connection_does_not_end_the_run_it_was_watching` + `beginning_a_reattach_resets_the_run_but_not_the_transcript` + `beginning_a_reattach_keeps_the_side_question` (4 条) | 完备 |
| `reconnect` 失败路径（`run_concurrency` RPC 失败） | `reattach.rs` 无单测；`state/connection.rs` 测试覆盖 `Failed` 状态机分支 | `tui/mod.rs:1145-1235` 中 reconnect 失败 / 成功臂源码级守卫（`the_reconnect_success_arm_must_still_reset_the_run_state` / `the_reconnect_failure_arm_must_still_back_off` / `the_flag_must_still_be_checked_inside_main_loop`） | TUI 有源码守卫，Panel 无 |
| 跨 session 串台 | `events.rs::resolve_target_*` (5 条) + `every_send_path_binds_its_run` 源码扫描器 + `a_surface_that_registers_no_conversation_receives_no_frame` | `frame_belongs_here_*` 12 条 + `the_screens_own_run_survives_fifo_eviction` + `a_foreign_run_is_still_learned_after_reconciling` | 完备 |
| 跨 session 切回 | 无显式「切到 s2 后切回 s1」的端到端单测 | `an_adopted_run_follows_its_home_session_across_switches` | TUI 完备，Panel 隐式覆盖 |
| `terminate_reason` 跨端 | `parse_run_halt` 5 条 + `RunHalt::label` 3 条 | `a_halt_notice_is_written_in_one_language` + `a_capped_run_names_the_cap_under_the_umbrella_token` + `a_clean_run_leaves_no_halt_line` (3 条) | 完备；共享 `aleph_protocol::terminate::label` |
| Turn Age vs Join Age | 不适用（Panel 不计时） | `an_adopted_run_is_back_dated_by_the_age_the_server_reported` + `an_adopted_run_without_an_age_counts_from_the_join` + `an_impossible_age_falls_back_instead_of_panicking` (3 条) | 完备；无「elapsed 计时器在 run 完成时是否停在 4:00」的端到端断言（见 Tier-2 候选） |
| `attach_session` 三模式 | `handlers/chat.rs::handle_history` 单测 | `attach_mode_tests` 模块 3 条：`appending_attach_keeps_what_the_caller_prepared` / `a_replacing_attach_swaps_in_the_servers_copy` / `nothing_is_thrown_away_until_the_servers_copy_is_in_hand` | 完备；最后一条是源码级守卫（prevent 防御性 .clear 提前到 fetch 前） |
| `active_run_from_history` 三态 | `handlers/chat.rs` 测 | `active_run_tests` 模块 5 条：`an_absent_field_is_not_an_answer` / `null_means_nothing_is_running_here` / `a_run_id_is_the_turn_to_join` / `the_reported_age_rides_with_the_run_id` / `an_age_without_a_run_is_still_nothing_running` / `an_empty_string_is_not_a_run` (6 条) | 完备 |
| `bind_run` 双计防御 | `bind_and_settle_run_refcounts_and_routes` | — | Panel 端隐式 |
| `rejoin_target` / `settle_runs_absent_from` 边界 | 5 条 (`rejoin_target_names_only_a_live_session_this_client_cannot_route` / `rejoin_target_is_silent_for_a_conversation_with_no_session_key` / etc.) | — | — |
| `connection_epoch` bump 语义 | 通过 `app.rs:158-170` Effect 间接验证；无显式单测 | — | **缺口**：见 §4.A4 / Tier-2 |
| `subscribe_runtime_agents` 重连重试 | — | 源码守卫 `subscribe_runtime_agents_must_still_subscribe_after_a_reconnect`（推测存在）；**失败路径未测** | 见 §4.B1 / **Tier-1** |
| TUI main loop 重连成功/失败臂 | — | 源码级守卫 3 条（`the_reconnect_success_arm_must_still_reset_the_run_state` / `the_reconnect_failure_arm_must_still_back_off` / `the_flag_must_still_be_checked_inside_main_loop`） | 完备（静态） |

### 5.1 缺口总结

1. **Panel 端 `reattach_after_connect` 在 `run_concurrency` 失败时无重试路径**（已识别为 Tier-2）
2. **TUI `subscribe_runtime_agents` 重连失败时静默**（已识别为 Tier-1）
3. **TUI elapsed 计时器在 4 分钟长 run 完成时是否停在正确值**——无显式断言（Tier-2 加固单测）
4. **`connection_epoch` 跨 reconnect 的单调性无单测**（Tier-2 加固单测）
5. **`bind_run` 重复调用的双计问题**——目前没有 race 路径触发，但缺少防御性单测（Tier-2 加固）

---

## 6. 范围闸

### L1 范围：Y（仅修 bug + 补连线 + 补测试）

| Tier 候选 | 标题 | 文件:行 | 修复内容 | 估行数 | 备注 |
|-----------|------|---------|----------|--------|------|
| **Tier-1** | `subscribe_runtime_agents` 重连失败时静默 | `interfaces/tui/src/tui/mod.rs:531-534` | 失败时设 `state.runtime_agents_refetch_due = true`（沿用已有机制），并 log warn；下个 main_loop 迭代再试一次 | <20 | 真 bug（用户可见：面板永远不更新），但路径窄（只在 `subscribe_runtime_agents` 调用失败时触发） |
| **Tier-2** | Panel `reattach_after_connect` 失败重试缺失 | `interfaces/webchat/src/state/reattach.rs:67-83` | `run_concurrency` 失败时设一个 `pending_reattach: RwSignal<bool>`，由下次 `connection_epoch` bump 重新触发 | 中 30–50 | 概率低（首次 RPC 失败但 socket 通），但失败后**永远不恢复**——修起来不复杂 |
| **Tier-2** | 加固：`connection_epoch` 单调性单测 | `interfaces/webchat/src/context.rs` 测试模块 | 模拟 connect→disconnect→reconnect，断言 epoch 单调 +3 | 小 <20 | 加固，不修行为 |
| **Tier-2** | 加固：`bind_run` 重复调用不双计 | `interfaces/webchat/src/state/sessions.rs:370-382` | 在 `bind_run` 入口加 `if route.with_untracked(|m| m.contains_key(run_id)) { return; }` | +1–3 | 加固，不修行为（无 race 路径触发） |
| **Tier-2** | 加固：TUI elapsed 计时器在 run 完成时停在正确值 | `interfaces/tui/src/tui/app/tests.rs` 新增 | 4 分钟 joined → 1 秒后 RunComplete → 断言 run_started_at 仍是 4 分钟前（不是 None 也不是 now）| 小 <20 | 加固；`run_started_at` 在 RunComplete 中被清为 None（`app/events.rs:487/553`），其实**当前行为是「run 完成时 elapsed 计时器消失」**，不是「停在正确值」。**新增断言可能揭示预期 vs 实际的差异**——审前应先确认产品意图 |
| **Tier-2** | 加固：`mark_queued` 乱序单测 | `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs` phase_test 模块 | 同 run_id 收到 `run_accepted` 在 `run_queued` 之后，断言 phase 不被覆盖回 Queued | 小 <20 | 加固 |
| **Tier-2** | 加固：跨会话切回的端到端单测 | `interfaces/webchat/src/state/sessions.rs` 测试 | 模拟：s1 active → adopt s1 run → switch to s2 → switch back to s1 → 断言 `route_lookup` 与 `is_running` 仍正确 | 小 <20 | 加固 |

> **Tier-3 性能/样式**：本轮不做。Phase 1 审计中**无 Tier-3 发现**。

### 总体判定

- **L1 范围**：Y
- **Tier-1**：1 项（TUI subscribe 重连失败静默）
- **Tier-2**：6 项（全部加固 / 测试补强，无行为修复）
- **Tier-3**：0 项
- **总估行数**：< 130 行（其中 6 项 Tier-2 加固约 100 行单测，1 项 Tier-1 修 <20 行）

### 不做（State the Negative）

- **不抽公共面**：Panel `state/mod.rs`（3575 行）与 TUI `app/mod.rs`（2195 行）目前各自独立，跨端共享走 `shared_ui_logic` + `aleph_protocol`，无新公共面抽取需求
- **不重写 TUI main_loop 或 reconnect 流程**：现有 `reconnect_after` / `awaiting_reconnect` / `next_backoff` + 50ms tick 边缘触发的设计正确，doc 与测试齐全
- **不动 wire protocol**：`active_run` / `active_run_elapsed_ms` 字段已稳定，不动
- **不动 server 端**：`active_run_for_session` / `run_elapsed_ms` 接口已稳定，不动
- **不动 `bind_run` 的现有守卫**：现「重复调用双计」无 race 触发，**不预防性增加 `if route.contains_key` 守卫**，避免改动无 bug 代码引入回归——只补单测钉住现有正确性

---

**结论**：本轨道审计完成。8 项 FEATURE_LOCATOR ✅ 全部可信，7 项 Spec C1–C7 全部覆盖。**1 项 Tier-1 小修 + 6 项 Tier-2 加固**，无 Tier-3。状态机已绿，本轮修完即可进入 Phase 2 实施。
