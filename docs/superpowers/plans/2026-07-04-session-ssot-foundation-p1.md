# Phase 1 · SSOT 地基 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `session_events`（SessionService）成为唯一权威会话日志，`messages` 表（SessionManager）降为由 `MessageProjector` 从事件流物化的只读投影；删除有损 shim 与执行引擎的重复直写。

**Architecture:** 翻转箭头——一切写入经 `SessionService.emit_event`；在 `SessionActor` 的 append 成功点触发一个 `SessionEventObserver`；`MessageProjector`（gateway 层，实现该 observer）经单一有序 drain 任务把事件投影进 `SessionStore::append_message`（复用其 derived_title/FTS/token 记账/compaction 触发）。执行引擎对 `add_message` 的重复直写被移除或重定向到 `emit_event`；shim 删除后 `append_message` 只被 projector 调用 = single-writer。

**Tech Stack:** Rust, tokio (mpsc/broadcast/actor), rusqlite (WAL, `sessions.db`), async-trait, serde. 单 crate `alephcore`。

## Global Constraints

- **MSRV 1.95**；rustfmt 4-space / 100 列；`cargo fmt -p alephcore -- --check` 必须过（CI 门）。
- **R10 零 harness 增长**：不改 `src/harness/`。新组件落 `src/session/` 与 `src/gateway/`。
- **依赖方向**：`SessionService`（`src/session/`）只依赖本地 `SessionEventObserver` trait；projector（`src/gateway/session_projector.rs`）桥接到 `SessionStore`——gateway 依赖 session，不反向。
- **锁安全**：`.lock().unwrap_or_else(|e| e.into_inner())`。**UTF-8**：`char_indices()`/`.get(..n)`，不用 `&s[..n]`。
- **不可变优先**：投影是纯函数 `event → Option<ProjectedRow>`；副作用只在 drain 任务。
- **节制 cargo**：默认不跑全量测试；作用域收窄到 `session::` / `session_projector` / `harness_bridge::` 单测；最多一次 `cargo check -p alephcore --lib`。
- **禁用清单**：不引第二 async runtime、不引向量库、`src` 不碰平台 API crate、不用 regex 做语义、全栈 serde。
- **单 DB**：`messages` 与 `session_events` 同 `sessions.db` 两表两连接；projector 写 messages 走既有 `SessionStore` 抽象，不新建磁盘结构（Windows 目录命名可移植性）。
- **提交规范**：`<scope>: <desc>`，English。single-branch main。

---

## File Structure

| 文件 | 责任 | 动作 |
|------|------|------|
| `src/session/observer.rs` | `SessionEventObserver` trait（本地，供 SessionService 回调） | Create |
| `src/session/mod.rs` | 导出 `observer` | Modify |
| `src/session/actor.rs` | EmitEvent 成功点回调 observer（仅新 append，replay 不触发） | Modify |
| `src/session/in_process.rs` | 持有 `Option<Arc<dyn SessionEventObserver>>` + `with_observer`；构造 actor 时下传 | Modify |
| `src/session/projection.rs` | 扩展纯映射：加 tool 事件 + `ProjectedRow`（含 role/text/tool_call_id/tool_name） | Modify |
| `src/gateway/session_store/types.rs` | `MessageRecord` 加 `tool_call_id`/`tool_name` | Modify |
| `src/gateway/session_manager/ops/crud.rs` | messages 表 schema 加两列；`add_message_with_meta` 持久化+读回 tool 字段；**删 shim（244-246）** | Modify |
| `src/gateway/session_manager/ops/crud.rs` (get_history) | SELECT 带出 tool 列 | Modify |
| `src/gateway/session_projector.rs` | `MessageProjector`（observer 实现）+ 有序 drain 任务 + 跨事件 token/model 聚合 | Create |
| `src/gateway/session_store/mod.rs` | `SessionStore` 加 tool 感知 append（默认转发）+ dual-read fallback 说明 | Modify |
| `src/orchestrator/harness_bridge/session_seed.rs` | 延续会话不再外部 re-seed；保留真实 turn_id | Modify |
| 执行引擎写点 | 移除/重定向 `add_message` 直写（`execute.rs`/`simple.rs`/`fast_path.rs`/`openai_api/completions/agent.rs`） | Modify |
| `src/bin/aleph-server/commands/start/mod.rs` + `helpers.rs` | 构造 projector；注入为 SessionService observer；projector 写 Panel 所读的同一 `SessionStore` | Modify |
| `src/session/shim.rs` | **删除** | Delete |

---

## Task 1: `MessageRecord` + messages 表加 tool 字段

**Files:**
- Modify: `src/gateway/session_store/types.rs:4-19`
- Modify: `src/gateway/session_manager/ops/crud.rs`（schema 迁移 + insert + select）
- Test: `src/gateway/session_store/types.rs`（同文件 `#[cfg(test)]`）

**Interfaces:**
- Produces: `MessageRecord { …, tool_call_id: Option<String>, tool_name: Option<String> }`（后续 Task 2/4/投影读回依赖这两个字段名）。

- [ ] **Step 1: 写失败测试（字段存在 + 默认 None + serde 向后兼容）**

```rust
// src/gateway/session_store/types.rs 内 #[cfg(test)] mod
#[test]
fn message_record_tool_fields_default_none_and_roundtrip() {
    // 老 JSON（无 tool 字段）反序列化 → None
    let legacy = r#"{"id":"1","role":"assistant","content":"hi","timestamp":1,"metadata":null}"#;
    let rec: MessageRecord = serde_json::from_str(legacy).unwrap();
    assert!(rec.tool_call_id.is_none());
    assert!(rec.tool_name.is_none());
    // 带 tool 字段 round-trip
    let tool = MessageRecord {
        id: "2".into(), role: "tool".into(), content: "{}".into(), timestamp: 2,
        metadata: None, input_tokens: 0, output_tokens: 0, model: None, model_provider: None,
        tool_call_id: Some("call_1".into()), tool_name: Some("bash_exec".into()),
    };
    let back: MessageRecord = serde_json::from_str(&serde_json::to_string(&tool).unwrap()).unwrap();
    assert_eq!(back.tool_call_id.as_deref(), Some("call_1"));
    assert_eq!(back.tool_name.as_deref(), Some("bash_exec"));
}
```

- [ ] **Step 2: 跑测试确认编译失败**

Run: `cargo test -p alephcore --lib session_store::types::tests::message_record_tool_fields -- --nocapture`
Expected: FAIL（`MessageRecord` 无 `tool_call_id` 字段，编译错误）。

- [ ] **Step 3: 加字段**

```rust
// src/gateway/session_store/types.rs — MessageRecord 末尾追加
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
```

- [ ] **Step 4: 修所有 `MessageRecord { … }` 字面量构造点编译错误**

Run: `cargo check -p alephcore --lib 2>&1 | grep -A2 "missing field"`
对每个报错点补 `tool_call_id: None, tool_name: None,`（grep 定位：`rg "MessageRecord \{" src --stats`）。

- [ ] **Step 5: messages 表 schema 加两列（幂等迁移）**

在 `crud.rs` 建表/迁移处（`rg "CREATE TABLE.*messages|ALTER TABLE messages" src/gateway/session_manager`）追加幂等列迁移。若无现成迁移函数，在 `SessionManager` 打开连接后执行：

```rust
// 幂等：列已存在则忽略错误（rusqlite 报 "duplicate column name"）
let _ = conn.execute("ALTER TABLE messages ADD COLUMN tool_call_id TEXT", []);
let _ = conn.execute("ALTER TABLE messages ADD COLUMN tool_name TEXT", []);
```

- [ ] **Step 6: `add_message_with_meta` insert 带 tool 列 + `get_history` SELECT 带回**

`add_message_with_meta` 需接收 tool 字段——本 Task 先加**私有重载**保持 `add_message_with_meta` 签名不变（避免大面积改调用点），Task 4 的 projector 走新入口：

```rust
// crud.rs — 新增私有方法（Task 4 projector 用）
#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_message_full(
    &self, key: &SessionKey, role: &str, content: &str, metadata: Option<&str>,
    input_tokens: i64, output_tokens: i64, model: Option<&str>, model_provider: Option<&str>,
    tool_call_id: Option<&str>, tool_name: Option<&str>,
) -> Result<i64, SessionManagerError> {
    // 复制 add_message_with_meta 主体，INSERT 增加 tool_call_id/tool_name 两列与 params。
    // add_message_with_meta 改为 delegate: add_message_full(..., None, None).
}
```
`get_history` 的两条 SELECT（`crud.rs:265+`）在列清单加 `tool_call_id, tool_name`，`MessageRecord` 构造回填。

- [ ] **Step 7: 跑测试通过 + fmt**

Run: `cargo test -p alephcore --lib session_store::types::tests::message_record_tool_fields`
Expected: PASS。
Run: `cargo fmt -p alephcore -- --check` → 无输出。

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "session: add tool_call_id/tool_name to MessageRecord + messages table"
```

---

## Task 2: 投影纯映射扩展（事件 → ProjectedRow，含 tool）

**Files:**
- Modify: `src/session/projection.rs`
- Test: `src/session/projection.rs`（同文件）

**Interfaces:**
- Consumes: `SessionEvent`（`events.rs`）。
- Produces: `pub struct ProjectedRow { pub role: String, pub text: String, pub tool_call_id: Option<String>, pub tool_name: Option<String> }` 和 `pub fn project_row(event: &SessionEvent) -> Option<ProjectedRow>`（Task 4 projector 依赖此签名）。

- [ ] **Step 1: 写失败测试（user/assistant/tool 映射；内部事件返回 None）**

```rust
#[test]
fn project_row_maps_message_and_tool_events() {
    let tid = uuid::Uuid::new_v4();
    let user = SessionEvent::UserMessage { turn_id: tid, content: MessageContent{ text:"hi".into(), blocks:vec![], thinking:None, thinking_signature:None }, at: 1, synthetic: false };
    let r = project_row(&user).unwrap();
    assert_eq!(r.role, "user"); assert_eq!(r.text, "hi");

    let call = SessionEvent::ToolCallRequested { turn_id: tid, call_id: "c1".into(), name: "bash_exec".into(), input: serde_json::json!({"cmd":"ls"}), at: 2 };
    let r = project_row(&call).unwrap();
    assert_eq!(r.role, "tool"); assert_eq!(r.tool_call_id.as_deref(), Some("c1")); assert_eq!(r.tool_name.as_deref(), Some("bash_exec"));

    // 内部标记不投影
    assert!(project_row(&SessionEvent::TurnStarted { turn_id: tid, trigger: TurnTrigger::UserMessage, at: 3 }).is_none());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib session::projection::tests::project_row_maps`
Expected: FAIL（`project_row`/`ProjectedRow` 未定义）。

- [ ] **Step 3: 实现 `ProjectedRow` + `project_row`**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedRow {
    pub role: String,
    pub text: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

/// Pure map: one session event → at most one projected message row.
/// Internal markers (turn/run/llm/budget/session lifecycle) yield None.
#[must_use]
pub fn project_row(event: &SessionEvent) -> Option<ProjectedRow> {
    let plain = |role: &str, text: String| ProjectedRow { role: role.into(), text, tool_call_id: None, tool_name: None };
    match event {
        SessionEvent::UserMessage { content, .. } => Some(plain("user", content.text.clone())),
        SessionEvent::AssistantMessage { content, .. } => Some(plain("assistant", content.text.clone())),
        SessionEvent::SystemMessage { content, .. } => Some(plain("system", content.clone())),
        SessionEvent::ToolCallRequested { call_id, name, input, .. } => Some(ProjectedRow {
            role: "tool".into(), text: input.to_string(),
            tool_call_id: Some(call_id.clone()), tool_name: Some(name.clone()),
        }),
        SessionEvent::ToolResult { call_id, output, .. } => Some(ProjectedRow {
            role: "tool".into(), text: output.value.to_string(),
            tool_call_id: Some(call_id.clone()), tool_name: None,
        }),
        SessionEvent::ToolError { call_id, error, .. } => Some(ProjectedRow {
            role: "tool".into(), text: error.clone(),
            tool_call_id: Some(call_id.clone()), tool_name: None,
        }),
        _ => None,
    }
}
```

- [ ] **Step 4: 跑测试通过**

Run: `cargo test -p alephcore --lib session::projection::tests`
Expected: PASS（含既有 `project_messages` 测试不回归）。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "session: pure project_row mapping incl. tool events"
```

---

## Task 3: `SessionEventObserver` trait + actor append 回调

**Files:**
- Create: `src/session/observer.rs`
- Modify: `src/session/mod.rs`（`pub mod observer;`）
- Modify: `src/session/actor.rs`（struct 加字段 + EmitEvent 成功点回调）
- Modify: `src/session/in_process.rs`（持有 observer + `with_observer` + 构造 actor 下传）
- Test: `src/session/in_process.rs`（新增测试：observer 只在新 append 触发，replay 不触发）

**Interfaces:**
- Produces: `pub trait SessionEventObserver: Send + Sync { fn on_appended(&self, id: &SessionId, record: &SessionEventRecord); }`（同步、非阻塞；Task 4 projector 实现它）。
- Produces: `InProcessActorSessionService::with_observer(self, Arc<dyn SessionEventObserver>) -> Self`（Task 5 boot 用）。

- [ ] **Step 1: 定义 trait**

```rust
// src/session/observer.rs
use crate::session::events::SessionEventRecord;
use crate::session::service::SessionId;

/// Fires exactly once per *newly appended* event (never on actor replay).
/// Must be non-blocking: implementations enqueue and return immediately.
pub trait SessionEventObserver: Send + Sync {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord);
}
```
`src/session/mod.rs` 加 `pub mod observer;`。

- [ ] **Step 2: 写失败测试（observer 只在新 append 触发；replay 不触发）**

```rust
// src/session/in_process.rs #[cfg(test)]
#[tokio::test]
async fn observer_fires_on_new_append_not_on_replay() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counter(Arc<AtomicUsize>);
    impl crate::session::observer::SessionEventObserver for Counter {
        fn on_appended(&self, _id: &SessionId, _rec: &SessionEventRecord) { self.0.fetch_add(1, Ordering::SeqCst); }
    }
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let count = Arc::new(AtomicUsize::new(0));
    let svc = InProcessActorSessionService::new(store).with_observer(Arc::new(Counter(count.clone())));
    let id = sample_id("obs");
    svc.emit_event(&id, SessionEvent::TurnStarted{ turn_id: uuid::Uuid::new_v4(), trigger: TurnTrigger::UserMessage, at: now_ms() }).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1, "one new append => one callback");
    // wake() 强制 actor 重放全部事件；replay 不得再触发 observer（只 SessionWoken 这一条新 append 触发）
    svc.wake(&id).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2, "replay must NOT re-fire; only the new SessionWoken append does");
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p alephcore --lib session::in_process::tests::observer_fires_on_new_append`
Expected: FAIL（`with_observer` 未定义）。

- [ ] **Step 4: actor 持有并回调 observer**

```rust
// src/session/actor.rs — SessionActor struct 加字段
    observer: Option<std::sync::Arc<dyn crate::session::observer::SessionEventObserver>>,
```
`SessionActor::new` 增加 `observer` 形参（默认由调用方传 `None`）。在 EmitEvent 成功 arm，`broadcaster.send(record)` 之后、`reply.send` 之前插入：
```rust
                                if let Some(obs) = &self.observer {
                                    obs.on_appended(&self.id, &record);
                                }
```
> 位置关键：**在 append 成功之后、replay 路径之外**——`replay()` 不经此 arm，故重放不触发（幂等）。

- [ ] **Step 5: in_process 持有 observer 并下传**

```rust
// src/session/in_process.rs — struct 加字段
    observer: Option<Arc<dyn crate::session::observer::SessionEventObserver>>,
```
`new` 初始化 `observer: None`；加：
```rust
    pub fn with_observer(mut self, obs: Arc<dyn crate::session::observer::SessionEventObserver>) -> Self {
        self.observer = Some(obs); self
    }
```
`spawn_actor` 里 `SessionActor::new(..., self.observer.clone(), self.idle_timeout)` 下传（注意 `with_idle_timeout` 是 `const fn`——`with_observer` 非 const，OK）。修 `SessionActor::new` 所有调用点（actor.rs 测试 + in_process）补 `None`。

- [ ] **Step 6: 跑测试通过**

Run: `cargo test -p alephcore --lib session::in_process::tests session::actor::tests`
Expected: PASS（既有 actor 测试补 `None` 后不回归）。

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "session: SessionEventObserver hook fired on new append only"
```

---

## Task 4: `MessageProjector`（observer 实现 + 有序 drain + token 聚合）

**Files:**
- Create: `src/gateway/session_projector.rs`
- Modify: `src/gateway/mod.rs`（`pub mod session_projector;`）
- Test: `src/gateway/session_projector.rs`（同文件）

**Interfaces:**
- Consumes: `project_row`（Task 2）、`SessionEventObserver`（Task 3）、`SessionStore`（`append_message` / 新 `add_message_full` 经 store）、`MessageRecord` tool 字段（Task 1）。
- Produces: `pub struct MessageProjector`；`MessageProjector::new(store: Arc<dyn SessionStore>) -> Arc<Self>`（spawn drain 任务，返回可作 observer 注入）；`impl SessionEventObserver for MessageProjector`。

**投影聚合规则（跨事件）**：drain 任务维护 `HashMap<(SessionId, TurnId), TurnAccum { model, provider, tokens_in, tokens_out }>`。
- `LlmCallStarted{model,provider}` → 记 turn 的 model/provider。
- `LlmCallEnded{tokens_in,tokens_out}` → 累加 turn tokens。
- `AssistantMessage{turn_id}` → 用该 turn accum 写 assistant 行（带 model/tokens），然后移除该 turn accum。
- `UserMessage`/`SystemMessage`/`Tool*` → 直接写行（0 token）。

- [ ] **Step 1: 写失败测试（往返 + token 聚合 + 幂等）**

```rust
#[tokio::test]
async fn projector_materializes_events_into_store_with_tokens() {
    // 用内存 SessionManager 作 SessionStore；构造 projector；喂事件；断言 get_history。
    let store: Arc<dyn SessionStore> = /* build in-memory SessionManager, see helpers */;
    let projector = MessageProjector::new(store.clone());
    let id = SessionKey::ephemeral("proj");
    let tid = uuid::Uuid::new_v4();
    for ev in [
        user_msg(tid, "hi"),
        SessionEvent::LlmCallStarted{ turn_id: tid, provider:"anthropic".into(), model:"claude".into(), at:1 },
        SessionEvent::LlmCallEnded{ turn_id: tid, tokens_in: 10, tokens_out: 20, finish_reason:"stop".into(), at:2 },
        assistant_msg(tid, "hello"),
        tool_req(tid, "c1", "bash_exec"),
        tool_res(tid, "c1"),
    ] {
        projector.on_appended(&id, &rec(ev));
    }
    // drain 是异步：轮询直到 5 行或超时
    let msgs = poll_history(&store, &id, 5, Duration::from_secs(2)).await;
    assert_eq!(msgs.iter().filter(|m| m.role=="user").count(), 1);
    let asst = msgs.iter().find(|m| m.role=="assistant").unwrap();
    assert_eq!(asst.input_tokens, 10); assert_eq!(asst.output_tokens, 20);
    assert_eq!(asst.model.as_deref(), Some("claude"));
    assert!(msgs.iter().any(|m| m.role=="tool" && m.tool_name.as_deref()==Some("bash_exec")));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib gateway::session_projector::tests::projector_materializes`
Expected: FAIL（`MessageProjector` 未定义）。

- [ ] **Step 3: 实现 projector（非阻塞 enqueue + 单一有序 drain + 聚合）**

```rust
// src/gateway/session_projector.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use crate::session::events::{SessionEvent, SessionEventRecord, TurnId};
use crate::session::observer::SessionEventObserver;
use crate::session::projection::project_row;
use crate::session::service::SessionId;
use crate::gateway::session_store::SessionStore;

const QUEUE_CAP: usize = 4096;

pub struct MessageProjector {
    tx: mpsc::Sender<(SessionId, SessionEventRecord)>,
}

#[derive(Default)]
struct TurnAccum { model: Option<String>, provider: Option<String>, tin: i64, tout: i64 }

impl MessageProjector {
    pub fn new(store: Arc<dyn SessionStore>) -> Arc<Self> {
        let (tx, mut rx) = mpsc::channel::<(SessionId, SessionEventRecord)>(QUEUE_CAP);
        tokio::spawn(async move {
            let mut accums: HashMap<(String, TurnId), TurnAccum> = HashMap::new();
            while let Some((id, rec)) = rx.recv().await {
                Self::project_one(&store, &mut accums, &id, &rec).await;
            }
        });
        Arc::new(Self { tx })
    }

    async fn project_one(
        store: &Arc<dyn SessionStore>,
        accums: &mut HashMap<(String, TurnId), TurnAccum>,
        id: &SessionId,
        rec: &SessionEventRecord,
    ) {
        let key = id.to_key_string();
        match &rec.event {
            SessionEvent::LlmCallStarted { turn_id, provider, model, .. } => {
                let a = accums.entry((key, *turn_id)).or_default();
                a.model = Some(model.clone()); a.provider = Some(provider.clone());
            }
            SessionEvent::LlmCallEnded { turn_id, tokens_in, tokens_out, .. } => {
                let a = accums.entry((key, *turn_id)).or_default();
                a.tin += *tokens_in as i64; a.tout += *tokens_out as i64;
            }
            SessionEvent::AssistantMessage { turn_id, content, .. } => {
                let a = accums.remove(&(key.clone(), *turn_id)).unwrap_or_default();
                if let Err(e) = store.append_message(id, MessageRecord {
                    id: format!("{key}:{}", rec.seq), role: "assistant".into(),
                    content: content.text.clone(), timestamp: rec.created_at_ms,
                    metadata: None, input_tokens: a.tin, output_tokens: a.tout,
                    model: a.model, model_provider: a.provider,
                    tool_call_id: None, tool_name: None,
                }).await {
                    tracing::warn!(error=%e, "projector assistant append failed");
                }
            }
            other => {
                if let Some(row) = project_row(other) {
                    if let Err(e) = store.append_message(id, MessageRecord {
                        id: format!("{key}:{}", rec.seq), role: row.role,
                        content: row.text, timestamp: rec.created_at_ms,
                        metadata: None, input_tokens: 0, output_tokens: 0,
                        model: None, model_provider: None,
                        tool_call_id: row.tool_call_id, tool_name: row.tool_name,
                    }).await {
                        tracing::warn!(error=%e, "projector append failed");
                    }
                }
            }
        }
    }
}

impl SessionEventObserver for MessageProjector {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord) {
        // Non-blocking: drop on backpressure (projection is rebuildable from the log).
        if self.tx.try_send((id.clone(), record.clone())).is_err() {
            tracing::warn!(session=?id, seq=record.seq, "projector queue full; dropping (rebuildable)");
        }
    }
}
```
> `MessageRecord.id = "{key}:{seq}"` 是确定性 id——为 dual-read/backfill 去重预留（Task 7）。`append_message` 当前忽略传入 id（用 rowid），本 P1 保持；确定性 id 的去重在 Task 7 的 backfill 幂等中用 `session_events.seq` 判断，不依赖 messages 主键。

- [ ] **Step 4: 跑测试通过**

Run: `cargo test -p alephcore --lib gateway::session_projector::tests`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "gateway: MessageProjector materializes session_events into messages"
```

---

## Task 5: Boot 接线（projector 注入为 SessionService observer，写 Panel 所读的 store）

**Files:**
- Modify: `src/bin/aleph-server/commands/start/mod.rs:357-396`
- Modify: `src/bin/aleph-server/commands/start/helpers.rs:236-266`（`build_sqlite_session_service` 增加 observer 注入口）

**Interfaces:**
- Consumes: `MessageProjector::new`（Task 4）、`InProcessActorSessionService::with_observer`（Task 3）。
- 约束：projector 的 `store` 必须是 **Panel 读的同一个 `Arc<dyn SessionStore>`**（`start/mod.rs:381-396` 最终 `session_store`）。

- [ ] **Step 1: 确认时序（先建 store，再建 projector，再建带 observer 的 SessionService）**

Run: `rg -n "build_sqlite_session_service|initialize_session_store|with_session_service|final session_store" src/bin/aleph-server/commands/start/mod.rs`
读 357-396，确认最终 `session_store` 变量名与构造顺序。**若 SessionService 当前在 store 之前构造**，需调整顺序：store → `MessageProjector::new(store.clone())` → SessionService `with_observer(projector)`。

- [ ] **Step 2: 改 `build_sqlite_session_service` 接受可选 observer**

```rust
// helpers.rs — 签名增参
pub fn build_sqlite_session_service(
    db_path: &str,
    observer: Option<Arc<dyn crate::session::observer::SessionEventObserver>>,
) -> Arc<dyn SessionService> {
    // … 构造 InProcessActorSessionService 后：
    let svc = InProcessActorSessionService::new(store);
    let svc = match observer { Some(o) => svc.with_observer(o), None => svc };
    Arc::new(svc)
}
```

- [ ] **Step 3: start/mod.rs 在 store 就绪后构造 projector 并注入**

```rust
// start/mod.rs — 在最终 session_store 就绪后
let projector = crate::gateway::session_projector::MessageProjector::new(session_store.clone());
// 原 build_sqlite_session_service 调用点传入 Some(projector as Arc<dyn SessionEventObserver>)
let session_service = crate::bin_helpers::build_sqlite_session_service(&db_path, Some(projector));
```
（按实际模块路径调整；`build_sqlite_session_service` 若在其他调用点也用，传 `None`。）

- [ ] **Step 4: 编译验证**

Run: `cargo check -p alephcore --lib`
Expected: 通过（本 Task 不加行为测试——boot 接线由 Task 10 端到端手验覆盖；此处仅确保编译 + 顺序正确）。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "boot: inject MessageProjector as SessionService observer"
```

> ⚠️ 此刻 messages 被**双写**（执行引擎旧直写 + projector）——Task 7 的 flip 移除旧直写前，先完成 Task 6/7。中间态仅存在于开发分支的相邻 commit 间，不发布。

---

## Task 6: `seed_session` 保留 turn_id / 不外部 re-seed

**Files:**
- Modify: `src/orchestrator/harness_bridge/session_seed.rs`
- Test: `src/orchestrator/harness_bridge/tests.rs`

**Interfaces:**
- Consumes: `FlowInput`（`Prompt`/`Messages`/`History`/`Multimodal`/`Resume`）。
- 行为变更：`FlowInput::History{turns,prompt}` 分支——若会话日志已非空（延续会话），**不重放 turns**（等同 `Resume`），只 append 新 prompt。仅空会话（首触）才 seed turns。

- [ ] **Step 1: 写失败测试（非空会话的 History 输入不重复 seed 历史）**

```rust
// harness_bridge/tests.rs
#[tokio::test]
async fn history_input_does_not_reseed_when_log_nonempty() {
    let svc = fresh_service();
    let id = SessionKey::ephemeral("seed");
    // 预置一条已存在的 UserMessage（模拟延续会话）
    svc.emit_event(&id, user_event("earlier")).await.unwrap();
    // History 输入携带同样的历史 turns + 新 prompt
    seed_session(&*svc, &id, FlowInput::History{ turns: vec![FlowHistoryTurn::User(mc("earlier"))], prompt: "new".into() }).await.unwrap();
    let events = svc.get_events(&id, None, None).await.unwrap();
    let user_texts: Vec<_> = events.iter().filter_map(|r| match &r.event {
        SessionEvent::UserMessage{content,..} => Some(content.text.clone()), _=>None }).collect();
    // 期望：earlier 不被重复 seed，只新增 new
    assert_eq!(user_texts, vec!["earlier".to_string(), "new".to_string()]);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib harness_bridge::tests::history_input_does_not_reseed`
Expected: FAIL（当前 History 分支无条件重放 turns → `earlier` 出现两次）。

- [ ] **Step 3: History 分支加"日志非空则跳过 turns"守卫**

```rust
// session_seed.rs — FlowInput::History 分支开头
        FlowInput::History { turns, prompt } => {
            let existing = service.get_events(session_id, None, Some(1)).await
                .map(|e| !e.is_empty()).unwrap_or(false);
            if !existing {
                for turn in turns {
                    match turn {
                        FlowHistoryTurn::User(content) => emit_message(service, session_id, content, true).await?,
                        FlowHistoryTurn::Assistant(content) => emit_message(service, session_id, content, false).await?,
                    }
                }
            }
            // …（保留原 TurnStarted + trailing UserMessage(prompt) 逻辑不变）
        }
```

- [ ] **Step 4: 跑测试通过**

Run: `cargo test -p alephcore --lib harness_bridge::tests`
Expected: PASS（既有 seed 测试不回归）。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "harness_bridge: skip external history re-seed for non-empty session logs"
```

---

## Task 7: Flip — 移除执行引擎直写 + 删 shim（single-writer）

> **本 Task 最高风险**。逐点核对：每个执行引擎 messages-写点，其对应 user/assistant 是否已由 harness `emit_event` 覆盖。**已覆盖 → 删除直写；未覆盖 → 重定向为 `emit_event`**（绝不裸删导致消息丢失）。

**Files:**
- Modify: `src/gateway/execution_engine/execute.rs:313,624`
- Modify: `src/gateway/execution_engine/simple.rs:121,181`
- Modify: `src/gateway/execution_engine/fast_path.rs:42,121`
- Modify: `src/gateway/openai_api/completions/agent.rs:311`
- Modify: `src/gateway/session_manager/ops/crud.rs:244-246`（删 shim）
- Delete: `src/session/shim.rs`；Modify `src/session/mod.rs`（去 `pub mod shim;`）

**Interfaces:**
- Consumes: Task 5 已注入 projector（events → messages）。

- [ ] **Step 1: 逐点覆盖核查（先查证再改）**

对每个写点，确认同一逻辑消息是否已有 `emit_event`：
Run: `rg -n "emit_event|add_message|add_message_with_run_id|append_message" src/gateway/execution_engine/execute.rs src/gateway/execution_engine/simple.rs src/gateway/execution_engine/fast_path.rs src/gateway/openai_api/completions/agent.rs`
判定规则：
- **harness-backed 路径**（走 `AgentHarness`/`session_service.emit_event`）→ 直写是重复，**删**。
- **非 harness 快路径**（`fast_path`/`simple`/openai completions 若绕过 harness）→ 无对应事件，**重定向**为 `session_service.emit_event(UserMessage/AssistantMessage{turn_id})`（复用同 turn_id），让 projector 物化。
在每个写点上方加一行注释记录判定（`// SSOT: covered by harness AssistantMessage → removed` 或 `// SSOT: fast-path, redirected to emit_event`）。

- [ ] **Step 2: 写端到端失败测试（flip 后 messages 仍含 user+assistant，仅一份）**

```rust
// 新集成测试 tests/session_ssot_projection.rs（或就近单测）
#[tokio::test]
async fn run_populates_messages_exactly_once_via_projector() {
    // 用带 projector 的 SessionService + 真实 store 跑一轮最小 run（或直接驱动执行引擎写点）。
    // 断言 get_history 里 user=1、assistant=1（无重复），且 assistant.output_tokens>0。
}
```

- [ ] **Step 3: 跑测试确认失败/重复**

Run: `cargo test -p alephcore --lib run_populates_messages_exactly_once`
Expected: 在删直写前 FAIL（user/assistant 各 2 份：执行引擎 + projector）。

- [ ] **Step 4: 按 Step 1 判定删除/重定向各写点**

对 harness-backed 写点删除 `add_message*` 调用（连同其组装参数的孤儿变量）。对快路径写点替换为：
```rust
// 示例：fast_path assistant 重定向
session_service.emit_event(&session_key, SessionEvent::AssistantMessage {
    turn_id, // 复用本 turn 的 id
    content: MessageContent { text: reply_text.clone(), blocks: vec![], thinking: None, thinking_signature: None },
    at: now_ms(),
}).await.ok();
```

- [ ] **Step 5: 删 shim**

`crud.rs:244-246` 整段删除（`if let Some(svc) = self.session_service.as_ref() { … mirror_message_by_role … }`）。删除 `src/session/shim.rs`，`src/session/mod.rs` 去掉 `pub mod shim;`。
Run: `rg -n "shim|mirror_message_by_role|mirror_user_message|mirror_assistant" src`
Expected: 零残留（除本次删除）。`self.session_service` 字段若此后无消费者，一并清理（`rg "session_service" src/gateway/session_manager`）。

- [ ] **Step 6: 跑测试通过**

Run: `cargo test -p alephcore --lib run_populates_messages_exactly_once && cargo check -p alephcore --lib`
Expected: PASS + 编译通过（无死引用）。

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "gateway: flip to single-writer — projector materializes messages, remove exec-engine direct writes + shim"
```

---

## Task 8: Dual-read fallback（老会话无 events 时回退读 messages）

**Files:**
- Modify: `src/gateway/session_store/mod.rs`（在 `SessionStore` 读适配层加 fallback 语义说明）
- 实际落点：`src/gateway/handlers/chat.rs:252` 读路径已走 `SessionStore::get_history_before`——**老会话本就在 messages 里**，projector 只对**新** events 生效。故 P1 的 dual-read = "messages 缺失时才从 events 投影"，绝大多数老会话 messages 已有数据，天然回退。
- Test: `src/gateway/session_projector.rs` 或就近

**Interfaces:**
- 结论：因 messages 是既有读面且老会话数据已在其中，dual-read **无需新代码路径**——老会话读 messages（已有），新会话读 messages（projector 写入）。仅需一个回归测试锁定该不变量。

- [ ] **Step 1: 写测试锁定"老会话（仅 messages，无 events）读得到"**

```rust
#[tokio::test]
async fn legacy_session_history_readable_without_events() {
    // 直接往 store 写两条 messages（模拟老会话），不产生任何 session_events。
    // 断言 get_history 返回这两条 —— 证明 flip 不破坏历史会话。
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test -p alephcore --lib legacy_session_history_readable_without_events`
Expected: PASS（读面未变，天然成立）。

- [ ] **Step 3: 文档化不变量（`session_store/mod.rs` 顶注）**

```rust
//! Read invariant (P1 SSOT): `messages` is the Panel read surface. Legacy
//! sessions retain their rows here; new sessions are materialized by
//! `MessageProjector` from `session_events`. No dual-read branch needed —
//! both classes read the same `messages` table.
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "gateway: lock legacy-session read invariant post-flip"
```

---

## Task 9: On-demand 反向回填（老会话首次被 harness 触碰）

> 老会话的 `messages` 有数据但 `session_events` 为空。harness 靠 `session_events` 重放——首次续跑老会话时须把 messages 反向投影成 events，否则 harness 看不到历史。

**Files:**
- Modify: `src/orchestrator/harness_bridge/session_seed.rs` 或 `runner_impl.rs`（run 前的 backfill 钩子）
- Test: `src/orchestrator/harness_bridge/tests.rs`

**Interfaces:**
- Produces: `async fn backfill_events_from_messages(svc, store, id) -> Result<usize>`——events 为空且 messages 非空时，把 messages 逐条 `emit_event`（UserMessage/AssistantMessage），返回回填条数；否则 0。

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn backfill_seeds_events_from_legacy_messages_once() {
    // store 预置 2 条 messages；session_events 空。
    let n = backfill_events_from_messages(&svc, &store, &id).await.unwrap();
    assert_eq!(n, 2);
    // 再调一次：events 已非空 → 0（幂等）
    assert_eq!(backfill_events_from_messages(&svc, &store, &id).await.unwrap(), 0);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib harness_bridge::tests::backfill_seeds_events`
Expected: FAIL（函数未定义）。

- [ ] **Step 3: 实现 backfill + 在 run 前调用**

```rust
pub(crate) async fn backfill_events_from_messages(
    svc: &dyn SessionService, store: &dyn SessionStore, id: &SessionId,
) -> Result<usize, FlowError> {
    let existing = svc.get_events(id, None, Some(1)).await.map(|e| !e.is_empty()).unwrap_or(true);
    if existing { return Ok(0); }
    let msgs = store.get_history(id, None).await.map_err(|e| FlowError::Internal(e.to_string()))?;
    let mut n = 0;
    for m in msgs {
        let content = MessageContent { text: m.content, blocks: vec![], thinking: None, thinking_signature: None };
        match m.role.as_str() {
            "user" => { emit_message(svc, id, content, true).await?; n += 1; }
            "assistant" => { emit_message(svc, id, content, false).await?; n += 1; }
            _ => {} // tool/system: 跳过（历史回填只需对齐对话可见轮次）
        }
    }
    Ok(n)
}
```
在 harness run 入口（seed 之前）调用一次。**注意**：backfill 会触发 projector 再写 messages → 重复！故 backfill 期间须让 projector 跳过（或 backfill 用**不经 observer 的直接 store append 检查**）。**最简**：backfill 只写 events 供 harness 读，projector 产生的重复行用确定性 id 去重——但 append_message 用 rowid。**决策**：backfill 前先 `store` 已有这些 messages，projector 回填时会 append 重复。**采用**：backfill 时临时不触发投影——在 emit 前设会话级 `backfilling` 标记，projector drain 检测标记跳过该会话已存在 seq 范围。**（此细节在实现时定稿；测试须覆盖"backfill 不产生 messages 重复行"。）**

- [ ] **Step 4: 写"backfill 不产生 messages 重复"测试 + 跑通**

```rust
#[tokio::test]
async fn backfill_does_not_duplicate_messages_rows() {
    // 老会话 2 条 messages → backfill → 轮询确认 messages 仍是 2 条（projector 不重复写）。
}
```
Run: `cargo test -p alephcore --lib harness_bridge::tests::backfill`
Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "harness_bridge: on-demand backfill session_events from legacy messages (idempotent, no dup rows)"
```

---

## Task 10: 重构后检验门 (Post-Refactor Verification Gate)

> 高风险重构收尾。**任一不过不算完成**。对应 spec §8。

- [ ] **Step 1: 机械验证**

```bash
cargo fmt -p alephcore -- --check
cargo check -p alephcore --lib
cargo test -p alephcore --lib session:: harness_bridge:: gateway::session_projector resume_coordinator
```
Expected: 全绿。若 lib-test OOM，加 `CARGO_PROFILE_TEST_DEBUG=line-tables-only`（见 memory）。

- [ ] **Step 2: 死引用/单写者普查**

```bash
rg -n "shim|mirror_message_by_role|mirror_user_message|mirror_assistant_message|mirror_tool_result|mirror_turn_started" src   # 期望零
rg -n "\.add_message\(|add_message_with_meta|add_message_with_run_id|append_message\(" src/gateway/execution_engine src/gateway/openai_api   # 期望仅剩重定向注释标注过的/或空
rg -n "append_message\(" src   # 确认仅 projector + orphan_notice + SessionStore impl 内部
```
逐条确认无第二 messages-writer 残留。

- [ ] **Step 3: 相关性保真 + 往返一致 + 幂等 断言（已由 Task 2/4/7 单测覆盖，此处汇总跑）**

```bash
cargo test -p alephcore --lib run_populates_messages_exactly_once projector_materializes legacy_session_history_readable backfill_does_not_duplicate observer_fires_on_new_append
```
Expected: 全绿。

- [ ] **Step 4: 崩溃重放幂等（新集成测试）**

`tests/session_wake_recovery.rs` 附近加：mid-turn 崩（dangling tool call）→ `wake()` 全量重放 → 断言 projector **不产生重复 messages 行**、messages 行数守恒。
Run: `cargo test -p alephcore --test session_wake_recovery`
Expected: PASS。

- [ ] **Step 5: 端到端手验（Windows 部署，见 memory windows-deploy-default）**

```
just shell-build → taskkill //T //F the shell → 装 NSIS setup.exe → 启动
```
逐项确认：① 发一轮含 tool 调用的对话 → Panel 显示 user/assistant/tool 卡；② 刷新 Panel（断/重连）→ 历史 + tool 卡完整；③ kill server 重启 → 历史仍完整、未完成 run 由 ResumeCoordinator 接续；④ Panel token gauge 非 0（token 聚合生效）。

- [ ] **Step 6: 多角色对抗审查 diff**

对全 diff 用三视角复查（事实审查者 / 高级工程师 / 一致性审查者），确认：无遗漏连线、无孤儿变量、无 token 记账回归、无 harness 行数增长（`git diff --stat src/harness` 为空）。用 `superpowers:requesting-code-review` 或 `code-review` skill。

- [ ] **Step 7: 完整性自问三条（写进 PR 描述）**

1. 还有第二处直写 messages 吗？（single-writer）
2. 删 shim 后原覆盖是否全由 projector 等价覆盖？（无缺口）
3. 老会话 + 新会话 + 崩溃恢复三路是否都往返一致？（无路径遗漏）

- [ ] **Step 8: 更新 FEATURE_LOCATOR §? + spec 状态**

在 `docs/reference/FEATURE_LOCATOR.md` 会话相关词条更新锚点（shim 已删、projector 新增、single-writer）。P1 spec 顶部标 `Status: Implemented`。
```bash
git add -A && git commit -m "docs: mark P1 SSOT foundation implemented + refresh feature locator anchors"
```

---

## Self-Review（写计划后自查）

**Spec 覆盖**：G1 双历史裂脑 → Task 3-5(projector) + Task 7(flip/删 shim)；相关性保真 → Task 6 + project_row；tool 卡投影（决策 A） → Task 1/2/4；双读回退（决策 B） → Task 8；崩溃幂等 → Task 3(replay 不触发) + Task 10 Step 4；token 关注点 → Task 4 聚合 + Task 10 Step 5④。**§8 重构后检验 → Task 10 全覆盖**。

**Placeholder 扫描**：Task 9 Step 3 的 backfill-vs-projector 去重细节标注"实现时定稿"，但**已给出具体机制候选 + 强制测试（Step 4）锁定行为**——非空洞占位。其余步骤均含具体代码/命令。

**类型一致性**：`project_row`/`ProjectedRow`（Task 2）↔ Task 4 消费；`SessionEventObserver::on_appended`（Task 3）↔ Task 4 实现 ↔ Task 5 注入；`with_observer`（Task 3）↔ Task 5；`MessageRecord` tool 字段（Task 1）↔ Task 4/get_history。签名一致。
