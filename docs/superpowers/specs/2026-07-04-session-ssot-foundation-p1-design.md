# Phase 1 · SSOT 地基 — 详细设计 (Single Source of Truth Foundation)

> **Date**: 2026-07-04
> **Umbrella**: `2026-07-04-session-lifecycle-contract-design.md`
> **Solves**: G1（双历史裂脑）
> **Risk**: 🔴 高（触及会话写入热点 `SessionManager.add_message`）——收尾须走 §8 重构后检验。

---

## 0. 拓扑修正 (Topology Correction — 2026-07-04 深挖后)

> spec 初稿把 `messages` 表笼统称"SessionStore"。深挖后精确拓扑如下（写计划的真实地基）：

- **`SqliteSessionStore = SessionManager`**（type alias，`session_store/sqlite_backend/mod.rs:13`）——生产默认后端。所以生产其实只有**两个**存储：`messages` 表（SessionManager，同一 `Arc<Mutex<Connection>>`）+ `session_events`（SessionService）。二者同一 `sessions.db` 文件、**两张表、两条独立连接**。
- **Panel 读路径**：`chat.history` → `SessionStore::get_history_before` → SessionManager `messages` 表（`handlers/chat.rs:252`）。默认后端 `FileSessionStore` **不启用**（仅 `general.session_store_backend="file"` 时）。
- **messages 唯一写漏斗**：一切经 `SessionStore::append_message` → `SessionManager.add_message_with_meta`（`sqlite_backend/mod.rs:149`，`#[deprecated]`）。生产调用者 = `AgentInstance::add_message_with_run_id`（`agent_instance.rs:466`）+ `orphan_notice.rs:54`；上游是**执行引擎** `execution_engine/{execute,simple,fast_path}.rs` + `openai_api/completions/agent.rs`。
- **真正的重复**：**执行引擎**把 user/assistant 写进 `messages`，**同时** harness 把同样的 user/assistant 写进 `session_events`（外加 shim 反向镜像 messages→events）。这才是"双写"根因。
- **shim 位置**：`add_message_with_meta` 内部（`crud.rs:244-246`），一次性 messages→events 镜像。

**因此 P1 翻转箭头的精确含义**：① 让 `session_events`（harness 已写）成权威；② **projector 订阅 events → 写 messages**（复用 `append_message`，保留 derived_title/FTS/token 记账/compaction 触发）；③ **移除执行引擎对 `add_message` 的直写**（projector 已覆盖）；④ 删 shim（否则 event→projector→append_message→shim→event 成环）。④ 之后 `append_message` 只被 projector 调用 = single-writer。
- **⚠️ 新增关注点（token 关联）**：执行引擎 `add_message` 携带 `input_tokens`/`output_tokens`/`model`；而 harness 事件里 token 在 `LlmCallEnded`/`BudgetUpdated`、model 在 `LlmCallStarted`。projector 物化 `AssistantMessage` 时须**跨事件聚合**同 turn 的 token/model，否则 `messages`/`sessions` 表 token 记账回退为 0。见计划 Task「投影聚合」。

## 1. 目标 (Goal)

把 `session_events` 确立为**唯一权威的跨-run 会话日志**，`messages`/`SessionStore` 降为**从事件重建的只读投影**。消除有损 shim、assistant 双写、相关性丢失、"从外部 history 再 seed"四个 G1 症状。**Panel 读路径（`chat.history`）零改动**。

## 2. 核心手法：翻转箭头 (Flip the Arrow)

```
【今天 · 裂脑】
  SessionManager.add_message ──► messages 表(SessionStore)         ◄── Panel 读
                             └──► shim(有损:重生 turn_id/call_id) ──► session_events  ◄── harness 读/写
  harness run ─────────────────────────────────────────────────► session_events (assistant/tool)
        ▲ 最终 assistant 双写 ───────────────────────────────────────┘

【P1 · 单一事实源】
  一切写入 ──► SessionService.emit_event ──► session_events (唯一权威, 保留 turn_id/call_id)
                                     │
                             MessageProjector (订阅 SessionService::subscribe)
                                     ▼
                               messages 表(SessionStore)  ◄── Panel 读(read 路径不变)
```

`shim`（正向有损镜像）→ 由 `MessageProjector`（反向忠实投影）取代。写入从"两处直写"收敛到"一个 `emit_event` 入口"。

## 3. 组件 (Components)

| 组件 | 动作 | 锚点 |
|------|------|------|
| **`SessionService`** | 保持——唯一权威 writer，已有 `emit_event`/`subscribe`/`get_events`/`wake` | `src/session/service.rs` `in_process.rs` |
| **`MessageProjector`（新）** | 订阅 `SessionService::subscribe(id)`，把事件流物化进 `SessionStore::append_message`。boot 子系统，best-effort，可从 `get_events` 全量重建。**不进 `src/harness/`** | `src/session/projector.rs`（新） |
| **`SessionManager.add_message`** | 改：不再直写 `messages`；改为向 `SessionService.emit_event`，物化交给 projector。逐点核对调用者语义 | `src/gateway/session_manager/ops/crud.rs:97,110,245` |
| **`shim.rs`** | **删除**（`mirror_*` 全套 + `mirror_message_by_role`）——由 projector 取代 | `src/session/shim.rs` |
| **`seed_session`** | 改：取消"从外部 history 再 seed"（`FlowInput::History` 分支）；延续会话直接重放日志（=今天 `Resume` 语义）；新用户消息 = 一次 append 保留真实 `turn_id` | `src/orchestrator/harness_bridge/session_seed.rs` |
| **`projection.rs`** | 扩展：投影覆盖 tool 事件（`ToolCallRequested`/`ToolResult`/`ToolError`），不再只投 user/assistant/system | `src/session/projection.rs` |
| **`SessionStore`** | 保持——两后端（`sqlite_backend`/`file_backend`）都只被 projector 写；read 路径 Panel 侧零改动 | `src/gateway/session_store/` |

## 4. 投影映射 (Event → MessageRecord Projection)

| SessionEvent | → MessageRecord | 备注 |
|--------------|-----------------|------|
| `UserMessage{content, synthetic}` | role=`user`（`synthetic` 保留为 meta，投影可选跳过 UI） | text + blocks |
| `AssistantMessage{content}` | role=`assistant` | 含 thinking/signature meta |
| `SystemMessage{content}` | role=`system` | |
| `ToolCallRequested{call_id,name,input}` | role=`tool`（或 tool-card 伴生记录），`call_id` 关联 | **G3 预埋**：重载可见 tool 卡 |
| `ToolResult{call_id,output}` / `ToolError{call_id,error}` | 关联到上面的 call_id | 幂等 upsert |
| `TurnStarted`/`RunStarted`/`Budget…`/`Llm…` | 不投影（内部标记） | 仅日志侧存在 |

> `MessageRecord` 若无 tool-card 形状：P1 加最小字段（`tool_call_id: Option<String>` / `tool_name: Option<String>`，对齐 hermes messages 表）或伴生记录。**该字段决策在实现前定稿（见 §9 开放问题）**。

## 5. 数据流 (Data Flow — 一次用户消息)

1. 用户消息 → `SessionService.emit_event(UserMessage{turn_id})`（单一入口）。
2. Projector 收到广播 → `SessionStore.append_message` → Panel `chat.history` 立即可见。
3. Harness 重放日志 + 跑 → 追加 `AssistantMessage` / `ToolCallRequested` / `ToolResult`（真实 `call_id`）。
4. Projector 增量物化 assistant + tool 卡 → Panel 实时/重载一致。

## 6. 迁移 (Migration — 无大爆炸)

- **双读回退 (dual-read fallback)**：读某 key 时若 `session_events` 为空 → 回退读 legacy `messages`（老会话原地不动）。新事件正向投影，新老共存。落点在 Gateway 读 handler 或 `SessionStore` 适配层。
- **按需反向回填 (on-demand backfill)**：老会话首次被 harness 触碰时，把其 `messages` 一次性投影成 `session_events`（复用 projector 的逆映射），此后走新路径。**不写全库迁移脚本**。
- **Windows 兼容**：file backend 目录按 key 命名，Win 会 sanitize `:`→`_`；projector 复用现有 `SessionStore` 抽象，不新增按 key 命名的磁盘结构，规避 [windows-file-backend-session-dir-portability] 类问题。

## 7. 错误处理与不变量 (Error Handling & Invariants)

- **Projector 失败绝不阻断写路径**：事件是真相，投影可重建。`tracing::warn` + 下次重建。
- **幂等**：projector 按 `(session_id, seq)` 去重，重放（`wake()` 全量）不产生重复行——修掉今天 assistant 双写。
- **锁安全**：`.lock().unwrap_or_else(|e| e.into_inner())`（处理 poison，P7）。
- **UTF-8 安全**：字符串切片用 `char_indices()`/`.get(..n)`（P7）。
- **单一 writer 不变量**：审查确保 P1 后不再有第二处直写 `messages`（只有 projector）。

## 8. 重构后检验 (Post-Refactor Verification — 硬性收尾门)

> 高风险重构，实现完成后**必须**跑完本节全部检查，任一不过不算完成。分两类：机械验证 + 语义完整性验证。

### 8.1 机械验证 (Mechanical)
- [ ] `cargo check -p alephcore --lib` 通过（一次，遵节制 cargo 习惯）。
- [ ] `cargo test -p alephcore --lib session::` + `harness_bridge::` + `resume_coordinator::` 相关单测全绿（作用域收窄，避免全量 OOM，见 [alephcore-build-memory]）。
- [ ] `cargo fmt -p alephcore -- --check` 通过（防 rustfmt CI 门，见 [windows-ax-write-path] 教训）。
- [ ] `rg` 确认 `shim.rs` / `mirror_*` 零残留引用（删除彻底，无死引用）。

### 8.2 语义完整性验证 (Semantic — 防"改一半")
- [ ] **写入面普查**：`rg` 列出所有 `messages` / `append_message` / `add_message` 直写点，逐点确认已改走 `emit_event` 或属 projector 内部。**无第二 writer 残留**。
- [ ] **相关性保真**：新增测试断言 `turn_id`/`call_id` 穿投影不丢（对比今天 shim 的随机重生）。
- [ ] **往返一致**：append 一串事件 → 投影 → `SessionStore.get_history` == 事件投影（含 tool 卡）。
- [ ] **崩溃重放幂等**：mid-turn 崩 → `wake()` 全量重放 → projector 不产生重复行（断言行数）。
- [ ] **双读回退**：老会话（只有 messages）读得到；新会话（只有 events）读得到；混合会话不重复。
- [ ] **端到端手验**（Windows 部署，见 [windows-deploy-default]）：真实发一轮含 tool 调用的对话 → 刷新 Panel → 历史 + tool 卡完整 → 重启 server → 历史仍完整、未完成 run 接续。
- [ ] **多角色对抗审查**：用事实审查者 / 高级工程师 / 一致性审查者三视角复查 diff，确认无遗漏连线、无孤儿、无回归（呼应全局 agents.md 分角色审查）。

### 8.3 完整性自问三条
1. 还有没有第二处在直写 `messages`？（single-writer 不变量）
2. 删掉 shim 后，是否所有原 shim 覆盖的写入都由 projector 等价覆盖？（无覆盖缺口）
3. 老会话 + 新会话 + 崩溃恢复三条路径是否都往返一致？（无路径遗漏）

## 9. 风险与已定稿决策 (Risks & Resolved Decisions)

- **最大风险**：`SessionManager.add_message` 是热点、多处调用；改写入语义须逐点核对调用者（§8.2 写入面普查）。
- **决策 A（已定稿 2026-07-04）**：tool 卡投影 = **P1 给 `MessageRecord` 加最小 `tool_call_id: Option<String>` / `tool_name: Option<String>` 字段**（对齐 hermes messages 表，直接服务 G3 重载可见 tool 卡）。不走伴生记录、不推 P3。
- **决策 B（已定稿 2026-07-04）**：双读回退落点 = **`SessionStore` 适配层**（对所有读者透明），不散在 Gateway 各读 handler。
- **P1 不做**：惰性归一（P2）、inflight 快照（P3）、facade（P4）。

## 10. 非目标 (Out of Scope)
- 删除 `messages` 表本身（保留为投影读面）。
- 更改 `SessionKey` / 路由语义。
- 跨进程 Session daemon。
