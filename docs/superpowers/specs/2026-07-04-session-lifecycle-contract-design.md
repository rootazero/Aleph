# 会话生命周期契约 — 伞形设计 (Session Lifecycle Contract — Umbrella Design)

> **Date**: 2026-07-04
> **Status**: Approved design (umbrella). Phase 1 deep-spec: `2026-07-04-session-ssot-foundation-p1-design.md`.
> **Scope**: 会话历史持久化与中断恢复的深度架构重构。落地宪法采纳条款 **A3/A4**。
> **Constitution**: R7 / R9 / R10（笨循环零增长）、A1（自有 context）、A3（状态可重建趋向纯 reducer）、A4（统一 Launch/Pause/Resume）。

---

## 1. 问题 (Problem)

**目标**：会话中断（断网、死机、进程重启）后，用户重启会话能**完全恢复现场**——完整历史 + 未完成的后台 run 自动接续 + 断网期间的流式输出追平。

**根因：双历史裂脑 (dual-history split-brain)**。当前有两套并行的会话历史，且写入箭头是反的：

- **`messages` 表**（`SessionStore`，两后端 `sqlite_backend`/`file_backend`）= Gateway `sessions.*` / `chat.history` RPC 的读路径 = **Panel 看到的历史**。
- **`session_events`**（`SessionService`，append-only 事件日志，`(session_id, seq)` 单调序，WAL）= **harness 读/写** + **`ResumeCoordinator` 扫描**的日志。

桥接是有损的：`SessionManager.add_message` 直写 `messages`（主），再经 `shim.rs::mirror_message_by_role` **有损镜像**进 `session_events`——**每条消息重生随机 `turn_id`，tool 结果配随机 `call_id`**（相关性丢失）。而 harness 又直接写 `session_events`，导致**最终 assistant 消息可能被写两次**，tool 事件只存在于 `session_events`（Panel 重载看不到 tool 卡）。

因此"重启不能完全恢复"：**恢复读的日志（`session_events`）可能 ≠ 用户看到的历史（`messages`）**。

### 1.1 现有三套机制（现状锚点）

| 机制 | 职责 | 锚点 | 评价 |
|------|------|------|------|
| `SessionService` 事件日志 | per-session append-only 事件日志；`wake()` 全量重放崩溃恢复 | `src/session/{service,in_process,actor,store,events,projection}.rs` | ✅ 正确底座 |
| `ResumeCoordinator` | 开机扫描"以 `RunStarted` 结尾无 `RunFinished`"的会话 → 补 synthetic `ToolError` → `metadata["resume"]=true` 重触发；24h 时效门 + 3 次崩溃循环上限；默认开启 | `src/gateway/resume_coordinator.rs` · `src/config/types/resume.rs` | ⚠️ 只管开机；持久化修复污染日志 |
| 客户端重连 | WS 5 次重连 + `ConnectionPhase` 状态机 + `replay_run`/`SessionSnapshot` 从已持久化历史重建 | `interfaces/webchat/src/state/connection.rs` 等 | ⚠️ 断网期间进行中 run 的流式输出丢失 |

## 2. 四个结构性缺口 (Gaps)

| # | 缺口 | 严重度 |
|---|------|--------|
| **G1** | 双历史裂脑：两个事实源 + 有损 shim + 相关性丢失 + assistant 双写 | 🔴 高 |
| **G2** | 崩溃修复是"持久化"（写 synthetic `ToolError` 落盘）而非"惰性"（build-time 内存补平）。`projection.rs` 只投影 user/assistant/system，**丢弃 tool 事件**；任何未被 `ResumeCoordinator` 覆盖的崩溃路径会把不平衡的 tool-call 日志发给 provider → 400 | 🟠 中高 |
| **G3** | 断网重连丢失"进行中" run 的流式输出（无 hermes 式 `inflight_snapshot`） | 🟠 中 |
| **G4** | "死机重启" vs "断网重连" 未区分；无统一生命周期契约（A4 未落地） | 🟡 中 |

## 3. 参考项目对照 (Reference Survey)

三个参考实现的共识：**唯一事实源 + build-time 惰性修复（日志保洁）**——正是 Aleph G1+G2 的反面。

- **codex**（`T:\Github\codex`）：pristine append-only JSONL rollout（`~/.codex/sessions/…`）是唯一真相；SQLite 仅作 listing 索引。resume 反向扫描重建；崩溃边界**惰性**修复——build prompt 时 `ensure_call_outputs_present` 给悬空 call 注入 deterministic（UUIDv5）`aborted` 输出 + `remove_orphan_outputs`，**不写回日志**。
- **hermes-agent**（`T:\Github\hermes-agent`）：单一 WAL SQLite (`state.db`)，`active`/`compacted` 软删标志，按 `id`（插入序）重放保 tool 邻接；`sanitize_replay_history` 悬空 tool-call stripper（仅作用于喂模型的历史，不动展示历史）；**`inflight` 内存镜像**让客户端重连贴附进行中 run 的已流式部分（`_inflight_snapshot`）。
- **kimi-cli**（`T:\Github\kimi-cli`）：per-session append-only JSONL + 冻结 system prompt 复用；`asyncio.shield` 原子写 assistant+tool 对，防止撕裂（但无显式 stripper）。

## 4. 北极星：统一生命周期契约 (North Star — A4)

任何长跑单元（run / goal / loop / workflow / team task）都建立在**唯一持久事件日志**之上，日志是**唯一事实源**；恢复是日志的自然推论：

- **服务端重启（死机）** = 进程重放日志 + 重新触发未完成 run。
- **客户端断网/刷新** = 重新贴附到仍在服务端跑的 run，或从日志重建视图。
- **日志永远保洁**：崩溃边界在 build prompt 时惰性补平，不落盘。
- **一个 writer，一份真相**：`session_events` 写，`messages` 只读投影。

### 三条不变量 (Invariants)
1. **Single-writer**：只有 `SessionService` 追加权威事件；其它一切都是它的投影或触发器。
2. **Pristine log**：日志只记真实发生的事；修复是读侧（build-time）关注点。
3. **Reconstructible**：任何 UI 状态都能从日志纯函数式重建（A3 趋向纯 reducer）。

## 5. 路线图 (Roadmap)

| Phase | 名称 | 解决 | 交付物 | 依赖 |
|-------|------|------|--------|------|
| **P1** | **SSOT 地基** | G1 | `session_events` 成唯一权威跨-run 日志；修正 seeding 保留 `turn_id`/`call_id` + 补 tool 事件；`messages`/SessionStore 降为**从事件重建的只读投影**（一个 writer）；删除有损 `shim` 换成忠实 `MessageProjector`；取消"从外部 history 再 seed" | 无 |
| **P2** | **惰性归一 + 日志保洁** | G2 | harness build prompt 时补平悬空 tool call（codex 式 `ensure_call_outputs_present`/`remove_orphan_outputs`，deterministic id）；`ResumeCoordinator` 不再写 synthetic `ToolError`，只重触发 | P1 |
| **P3** | **In-flight 重连贴附** | G3 | 服务端 per-run 活缓冲；断网重连给 Panel `inflight_snapshot` 追平进行中流式；区分"重连（run 未断）" vs "重启（run 需重触发）" | P1 |
| **P4** | **生命周期契约门面** | G4/A4 | 薄 facade 统一 Launch/Pause/Resume/Cancel，收口 `cancellation.rs`+`resume_coordinator.rs`+`steering.rs`+workflow resume；**不进 `src/harness/`** | P1–P3 |

每个 Phase 各自 spec + plan。

## 6. 宪法对齐 (Constitution Alignment)

- **R10 零增长**：P1–P3 落在 `src/session/`、`src/gateway/`、`src/orchestrator/`，**零 `src/harness/` 增长**（harness 已读写 `SessionService`，我们只改其下游投影与上游 seeding）。P4 是薄 facade（审计文档 B §P1-1 已列 backlog），不进 harness。
- **A3**：P1 让状态可从单一持久源（`session_events`）重建；`messages` 变纯投影 = 趋向纯 reduce。
- **A4**：P4 把已存在的取消/续跑/打断/workflow resume 命名为一组生命周期契约。
- **R7/R9**：修复/恢复是脚手架不是认知；崩溃边界补平是机械操作，不做"错误恢复策略选择"（R10 第 5 不 / A2 边界）。

## 7. 每阶段成功判据 (Success Criteria)

- **P1**：往返一致（append 事件 → 投影 → `chat.history` == 事件投影，含 tool）；崩溃重放 projector 幂等无重复行；`turn_id`/`call_id` 穿投影不丢；老会话双读回退可用；`shim.rs` 删除。`cargo check --lib` 通过。
- **P2**：mid-turn 崩溃（resume 关闭）后重启，provider 收到平衡日志（无 400）；`session_events` 无 synthetic `ToolError` 落盘。
- **P3**：断网重连后 Panel 追平进行中 run 的已流式文本 + tool 卡；run 未断则不重触发。
- **P4**：单一 API 面可对 run/goal/loop/workflow/team-task 一致 launch/pause/resume/cancel；harness 行数零增长。

## 8. 非目标 (Non-goals)

- 跨进程 Session daemon（保持进程内 actor）。
- 更改 `SessionKey` 变体或路由语义。
- 引入独立事件溯源框架 / 第二 async runtime / 非 serde 序列化（违禁用清单）。
- P4 之外的生命周期抽象"为未来留口"（YAGNI，零消费者即撤回）。
