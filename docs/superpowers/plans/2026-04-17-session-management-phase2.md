# Session Management Phase 2 — Feature-Rich File Backend Completion

> **Date:** 2026-04-17  
> **Branch:** `feat/session-store-trait`  
> **Status:** COMPLETE

## Summary

Phase 2 目标是在 Phase 1 建立的 `SessionStore` trait + 双后端骨架之上，为 `FileSessionStore` 堆满 OpenClaw 级别的功能：checkpoint-based compaction、preview/derived title、实时事件同步、archive hook。所有改动已在本分支完成并通过编译与单元测试。

---

## Deliverables

### 1. 类型扩展 (`src/gateway/session_store/types.rs`)

- `SessionMetadata` 新增字段：
  - `derived_title: Option<String>` — 从第一条用户消息推导的标题
  - `last_message_preview: Option<String>` — 最后一条消息的前 120 字符预览
  - `runtime_ms: i64` — 累计运行时长（毫秒）
  - `estimated_cost_usd: f64` — 预估成本（美元）
  - `checkpoints: Vec<CheckpointSummary>` — 与该 session 关联的 compaction checkpoint 列表
- `CheckpointSummary` 扩展为包含 `message_count` 和 `retained_message_count`
- 新增 `SessionChangedEvent` 结构，用于 `sessions.changed` 事件广播

### 2. File Backend — Checkpoint Compaction (`src/gateway/session_store/file_backend/mod.rs`)

- **自动 checkpoint 创建**：`compact` 在删除旧消息前，将被删除的消息写入 `{session_dir}/checkpoints/{timestamp_ms}.jsonl`，并在 metadata 中记录 checkpoint 摘要
- **list_checkpoints**：读取 metadata 返回 checkpoint 列表
- **restore_checkpoint**：将指定 checkpoint 的 JSONL 内容恢复为当前 transcript
- **branch_from_checkpoint**：从 checkpoint 创建新 session，保留 checkpoint 的完整消息历史，并设置 `parent_session_key`

### 3. Archive Hook

- `delete_session` 不再直接 `remove_dir_all`，而是将 session 目录归档到 `{base_dir}/.archive/{date}/{session_key}/`，保留完整可恢复数据

### 4. Preview & Derived Title

- `append_message` 中实时计算：
  - `derived_title`：当第一条 `role == "user"` 的消息到达时，提取前 60 字符作为标题
  - `last_message_preview`：始终更新为最后一条消息的前 120 字符
- `get_session_preview` 直接通过 metadata 返回这些字段，无需加载完整 history

### 5. Gateway 事件总线集成

- `FileSessionStore` 新增 `event_bus: RwLock<Option<Arc<GatewayEventBus>>>` 和 `with_event_bus` 构造器
- `initialize_session_store` 重构：在 `event_bus` 初始化后调用，file backend 自动附加事件总线
- 在以下操作后自动广播 `sessions.changed` TopicEvent：
  - `create` → `get_or_create`
  - `send` → `append_message`
  - `compact`
  - `delete`
  - `reset`
  - `patch`
  - `close`
  - `checkpoint-branch`
  - `checkpoint-restore`
- 客户端通过现有 `events.subscribe` 订阅 `sessions.changed` 即可接收实时更新

### 6. RPC Handlers 扩展 (`src/gateway/handlers/session/db_handlers.rs`)

- `sessions.preview` 的 `meta_json` 响应已包含新字段（`derived_title`、`last_message_preview`、`runtime_ms`、`estimated_cost_usd`、`checkpoints`）
- 新增三个 handler：
  - `sessions.compaction.list`
  - `sessions.compaction.restore`
  - `sessions.compaction.branch`
- 已在 `register_session_handlers` 中注册并输出到启动日志

---

## Verification

- `cargo check -p alephcore`：**0 errors**
- `cargo test -p alephcore --lib`：**8822 passed; 1 failed**（失败为预先存在的 `gateway::interfaces::discord::resolver::channel::tests::test_resolve_uses_global_default`，与本次重构无关）

---

## Next Step

进入 **Phase 3 — Cutover & Migration**：
1. 将默认 `session_store_backend` 从 `"sqlite"` 切换为 `"file"`
2. 实现 `session_store::migration` 模块，导出旧 SQLite `messages` 表到 JSONL
3. 在首次启动时自动检测并执行迁移
4. 更新 `builtin_tools/sessions/list_tool.rs` 消费新的 `SessionFilter` / `SessionMetadata`
5. 验证前后端完整链路
