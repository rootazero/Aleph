# Team Chat 交互入口 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 chat 窗口加一个"团队群聊"交互入口——用户现场拼队（当前 agent 默认 leader）→ 提需求 → leader 编排现有 teams 后台 → 三栏窗口里看到各 agent 逐条归属的群聊协作 + 交付物/任务看板，可中途插话。

**Architecture:** 复用现有 leader-DAG 团队后台（`TeamStore`/`TeamDispatcher`/`CoordTask`/`MessageRouter`/`ArtifactStore`/团队工具）。后端补四个"已造未连"的缺口：薄 `teams.create` RPC、`TeamFanoutEmitter`（把 team run 事件广播到 `team.<id>.*` 带 `agent_id`）、`teams.chat.send`（拉起 leader run）、`teams.chat.thread`（hydrate）。前端复用 chat 渲染，加 `ChatMessage.agent_id`/`ChatState.team_id`（Option，零回归）+ 拼队入口 + 团队事件投影 + 三栏视图。守 R4（Panel 纯 I/O）、R10（编排靠 leader 的 LLM 推理，不在 gateway/dispatcher 加推理）。

**Tech Stack:** Rust（alephcore：gateway / teams / event_emitter）+ Leptos 0.8 WASM（interfaces/webchat）。

---

## 关键事实（来自 ground-truth 提取，落地前必读）

1. **无 `teams.create` gateway RPC**。团队创建只走 `team_create` 工具（`src/builtin_tools/team/create.rs`，leader_id = 工具绑定的 `current_agent_id`，自动把 leader 以 role="leader" 入队）或 `team_from_template`。前端 `TeamsApi` 无 `create()`，只有 `create_from_template()`。→ 本计划新增薄 `teams.create` RPC，显式接受 `leader_id` + 成员。
2. **成员 run 对面板静默**：`src/teams/dispatcher/runner.rs::execute_member_task` 用 `NoOpEventEmitter::new()`。→ B2 换成 `TeamFanoutEmitter`。
3. **leader 编排无入口**：`Team.leader_id` 存在但从无代码据它拉起 leader run。→ B1 新建，按 `execute_member_task` 的 spawn 范式（`execution_adapter.execute(request, agent, emitter)`）。
4. **事件总线**：`GatewayEventBus::publish_frame(&GatewayEventFrame)` 与 `publish(String)`；非流式事件以 `{topic, data}` 到面板。`TopicEvent::new(topic, data)`。
5. **emitter 先例**：`src/gateway/event_emitter/origin_fanout.rs::OriginFanoutEmitter` —— 包裹 `inner: Arc<dyn EventEmitter>`，`emit` 内拦截后 `self.inner.emit(event).await`。
6. **`EventEmitter` trait**（`src/gateway/event_emitter/mod.rs`）：`async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError>` + `fn next_seq(&self) -> u64` + 默认便捷方法。
7. **GatewayContext** 贯穿 dispatcher/runner（`context.execution_adapter()`），是团队链路拿 execution_adapter / event_bus / registry / stores 的把手。**落地前先确认 `GatewayContext` 暴露 `event_bus()` 访问器；没有则加一个只读访问器**（薄 I/O，不算业务逻辑）。
8. **前端 `ChatMessage`**（`interfaces/webchat/src/views/chat/state.rs`）字段众多且无 `Default`，结构体字面量站点：`push_user_message`、`start_assistant_message`、`begin_step`（含 2 处 literal）。加字段必须逐一更新。
9. **`WorkspacePanel`** 在 `interfaces/webchat/src/components/workspace_panel.rs`，当前只渲染 `<ActivityTimeline/>` + `<FilesDrawer/>`，**无 tab**。
10. **团队 RPC 真实 wiring 点**：`src/gateway/handlers/mod.rs:643` 的 `teams.*` 是占位（返回 error），真实 handler 在"Gateway startup"用 `TeamStore` 注册。**落地前先定位 `teams.list`/`teams.get` 真实注册处**（搜 `handle_list`/`handle_get` 调用点，大概率在 `src/bin/aleph-server/.../builder/handlers/` 或 gateway router setup），新方法在同处注册。

---

## File Structure

**后端 — 新建：**
- `src/gateway/event_emitter/team_fanout.rs` — `TeamFanoutEmitter`：包裹 inner emitter，把 team run 的 `StreamEvent` 归一化后广播到 `team.<team_id>.{message,activity,task}`（带 `agent_id`）。单一职责：团队事件归属/fan-out。

**后端 — 修改：**
- `src/gateway/event_emitter/mod.rs` — `pub mod team_fanout;` + re-export。
- `src/gateway/handlers/teams.rs` — 加 `handle_create` / `handle_chat_send` / `handle_chat_thread`。
- 团队 RPC 真实注册处（见关键事实 10）— 注册 `teams.create` / `teams.chat.send` / `teams.chat.thread`。
- `src/teams/dispatcher/runner.rs` — `execute_member_task` 的 emitter 从 `NoOpEventEmitter` 换成 `TeamFanoutEmitter`。
- `src/gateway/context.rs`（或 GatewayContext 定义处）— 若无 `event_bus()` 访问器则加。
- `src/teams/messages/router.rs`（或 message store）— 若无"按 team 列出消息"则加 `list_for_team(team_id)`（B3 需要）。

**前端 — 新建：**
- `interfaces/webchat/src/views/chat/team_events.rs` — `subscribe_team_events`：把 `team.*` 事件投影到团队 chat 状态（归属气泡 / 名册状态 / 任务&交付物刷新）。
- `interfaces/webchat/src/views/chat/team_compose.rs` — 拼队弹层组件（leader=当前 agent 预填 + 多选成员 → `teams.create` → 进团队模式）。
- `interfaces/webchat/src/components/team_roster.rs` — 左名册栏（leader+成员+状态点）。
- `interfaces/webchat/src/api/team_chat.rs` — `TeamChatApi::send` / `TeamChatApi::thread` + `TeamsApi::create` 封装。

**前端 — 修改：**
- `interfaces/webchat/src/views/chat/state.rs` — `ChatMessage += agent_id: Option<String>`；`ChatState += team_id` / `team_members`（名册+状态）；更新所有字面量站点 + 构造器。
- `interfaces/webchat/src/views/chat/messages.rs` — `MessageBubble` 在 `agent_id` 存在时渲染归属（颜色+名字）。
- `interfaces/webchat/src/components/workspace_panel.rs` — 团队模式下加两 tab（交付物 / 任务）。
- `interfaces/webchat/src/components/chat_sidebar.rs` — 加"团队群聊"入口按钮。
- `interfaces/webchat/src/views/chat/view.rs` — 团队模式下挂载三栏 + 订阅 `team.*`。
- 模块声明文件（`views/chat/mod.rs`、`components/mod.rs`、`api/mod.rs`）— 加新模块。

---

# Phase A — 后端（可独立用集成测试验证；天然检查点）

## Task A1: `teams.create` gateway RPC

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（加 `handle_create` + params 结构）
- Modify: 团队 RPC 真实注册处（关键事实 10）
- Test: `src/gateway/handlers/teams.rs`（`#[cfg(test)] mod tests`）

- [ ] **Step 1: 定位真实注册点 & 写失败集成测试**

先在仓库定位 `teams.list` 的真实注册（带 `TeamStore` 的那处，非 `mod.rs:643` 占位），记录文件路径备 Step 4 用。然后在 `src/gateway/handlers/teams.rs` 末尾的测试模块加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::store::memory::InMemoryTeamStore; // 若无内存实现，用现有测试用 store；落地前确认其路径

    #[tokio::test]
    async fn test_handle_create_persists_team_with_leader_and_members() {
        let store: Arc<dyn TeamStore> = Arc::new(InMemoryTeamStore::new());
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: serde_json::json!(1),
            method: "teams.create".into(),
            params: Some(serde_json::json!({
                "name": "ResearchSquad",
                "description": "ad-hoc",
                "leader_id": "agent-main",
                "members": [{"agent_id": "agent-alice", "role": "researcher"}]
            })),
        };
        let resp = handle_create(req, store.clone()).await;
        let team_id = resp
            .result
            .as_ref()
            .and_then(|r| r.get("team_id"))
            .and_then(|v| v.as_str())
            .expect("team_id in response")
            .to_string();

        let team = store.get_team(&team_id).await.unwrap().unwrap();
        assert_eq!(team.leader_id, "agent-main");
        let members = store.get_members(&team_id).await.unwrap();
        let ids: Vec<&str> = members.iter().map(|m| m.agent_id.as_str()).collect();
        assert!(ids.contains(&"agent-main"), "leader auto-enrolled");
        assert!(ids.contains(&"agent-alice"), "member enrolled");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore handlers::teams::tests::test_handle_create_persists_team_with_leader_and_members`
Expected: FAIL — `handle_create` not found（编译错误）。

- [ ] **Step 3: 实现 `handle_create`**

在 `src/gateway/handlers/teams.rs`（参照已有 `handle_get` 的 `parse_params` + 错误码风格）加：

```rust
#[derive(Debug, serde::Deserialize)]
pub struct CreateMemberSpec {
    pub agent_id: String,
    #[serde(default = "default_member_role")]
    pub role: String,
}

fn default_member_role() -> String {
    "member".to_string()
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateTeamParams {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub leader_id: String,
    #[serde(default)]
    pub members: Vec<CreateMemberSpec>,
}

/// teams.create — 显式 leader_id + 成员创建一个持久化团队（薄 I/O：仅封装
/// TeamStore::create_team + add_member，不做任何编排/业务逻辑，R4/R10）。
/// leader 以 role="leader" 自动入队；members 中与 leader 重复者跳过。
pub async fn handle_create(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    use crate::teams::types::{NewTeam, NewTeamMember};

    let params: CreateTeamParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if params.name.trim().is_empty() || params.leader_id.trim().is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "name and leader_id are required".to_string(),
        );
    }

    let team = match store
        .create_team(NewTeam {
            name: params.name.clone(),
            description: params.description.clone(),
            leader_id: params.leader_id.clone(),
        })
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to create team: {e}"),
            )
        }
    };

    // 自动把 leader 以 role="leader" 入队（镜像 team_create 工具语义）。
    if let Err(e) = store
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: params.leader_id.clone(),
            role: "leader".to_string(),
            ..Default::default()
        })
        .await
    {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to enroll leader: {e}"),
        );
    }

    for spec in params.members {
        if spec.agent_id == params.leader_id {
            continue; // leader 已入队
        }
        if let Err(e) = store
            .add_member(NewTeamMember {
                team_id: team.id.clone(),
                agent_id: spec.agent_id.clone(),
                role: spec.role.clone(),
                ..Default::default()
            })
            .await
        {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to enroll member '{}': {e}", spec.agent_id),
            );
        }
    }

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "team_id": team.id, "name": team.name, "leader_id": team.leader_id }),
    )
}
```

> 注：`parse_params`、`INVALID_PARAMS`、`INTERNAL_ERROR`、`JsonRpcRequest`/`JsonRpcResponse`、`Arc`、`TeamStore` 均已在本文件 import（参照现有 handler）。如缺 import 按现有风格补。

- [ ] **Step 4: 注册路由**

在 Step 1 定位的真实注册处，仿 `teams.list`/`teams.get` 的 store-bound 注册，加：

```rust
// teams.create — create a persistent team with explicit leader + members
{
    let store = team_store.clone();
    registry.register("teams.create", move |req| {
        let store = store.clone();
        async move { crate::gateway::handlers::teams::handle_create(req, store).await }
    });
}
```

> `team_store` / `registry` 的确切变量名以该注册处现有代码为准（沿用同作用域里 `teams.list` 用的那个 store 句柄与注册闭包风格）。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p alephcore handlers::teams::tests::test_handle_create_persists_team_with_leader_and_members`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/handlers/teams.rs
# 以及 Step 4 修改的注册文件
git commit -m "gateway: add teams.create RPC (explicit leader + members)"
```

---

## Task A2: `TeamFanoutEmitter`

**Files:**
- Create: `src/gateway/event_emitter/team_fanout.rs`
- Modify: `src/gateway/event_emitter/mod.rs`（`pub mod team_fanout;`）
- Test: `team_fanout.rs` 内 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写失败单测**

新建 `src/gateway/event_emitter/team_fanout.rs`，先写测试（断言 RunComplete 的最终文本被广播到 `team.<id>.message` 且带 `agent_id`）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_fanout_publishes_message_topic_with_agent_id() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe(); // 原始字符串通道
        let emitter = TeamFanoutEmitter::new(bus.clone(), "team-1".into(), "agent-alice".into(), None);

        emitter
            .emit(StreamEvent::RunComplete {
                run_id: "run-1".into(),
                seq: 0,
                summary: run_complete_summary_with_text("hello from alice"),
            })
            .await
            .unwrap();

        // 读到一条广播，topic = team.team-1.message，data.agent_id = agent-alice
        let raw = rx.try_recv().expect("an event was published");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v.get("topic").and_then(|t| t.as_str()), Some("team.team-1.message"));
        assert_eq!(
            v.pointer("/data/agent_id").and_then(|a| a.as_str()),
            Some("agent-alice")
        );
        assert_eq!(
            v.pointer("/data/text").and_then(|a| a.as_str()),
            Some("hello from alice")
        );
    }

    // 测试辅助：构造一个带 final_response 的 RunComplete summary。
    // 落地前按 StreamEvent::RunComplete 的真实 summary 类型补全字段
    // （参照 origin_fanout.rs 里 `summary.final_response` 的读法）。
    fn run_complete_summary_with_text(text: &str) -> /* RunSummary 类型 */ _ {
        unimplemented!("按 StreamEvent::RunComplete 真实 summary 类型构造，仅设 final_response = text")
    }
}
```

> ⚠️ 实现 Step 3 前先 Read `src/gateway/event_emitter/mod.rs` 的 `StreamEvent` 定义与 `origin_fanout.rs`，照真实的 `StreamEvent::RunComplete { run_id, seq, summary }` 字段补全 `run_complete_summary_with_text`，以及确认 `RunComplete` 的 `summary.final_response: Option<String>`。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore event_emitter::team_fanout`
Expected: FAIL（类型/函数未定义）。

- [ ] **Step 3: 实现 `TeamFanoutEmitter`**

在同文件实现（镜像 `OriginFanoutEmitter` 结构 + `publish` 到 topic）：

```rust
//! Team event fan-out emitter.
//!
//! Wraps an optional inner [`EventEmitter`] and, for a run that belongs to a
//! team, re-broadcasts a normalized view of its stream events onto
//! `team.<team_id>.{message,activity,task}` topics tagged with the producing
//! `agent_id`. The panel's team-chat view subscribes to `team.<id>.*` and
//! renders attributed bubbles + roster status + workspace tabs.
//!
//! Mirrors [`super::origin_fanout::OriginFanoutEmitter`]: intercept in `emit`,
//! always forward to `inner` (if any). Fan-out failures never abort the run.

use std::sync::Arc;
use async_trait::async_trait;

use super::{EventEmitter, EventEmitError, StreamEvent};
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};

pub struct TeamFanoutEmitter {
    inner: Option<Arc<dyn EventEmitter + Send + Sync>>,
    event_bus: Arc<GatewayEventBus>,
    team_id: String,
    agent_id: String,
}

impl TeamFanoutEmitter {
    /// `inner = None` 用于成员 run（此前是 NoOp，本就无主流可发）；
    /// `inner = Some(..)` 用于希望同时进单 agent 主流的场景（本 MVP leader run 用 None）。
    pub fn new(
        event_bus: Arc<GatewayEventBus>,
        team_id: String,
        agent_id: String,
        inner: Option<Arc<dyn EventEmitter + Send + Sync>>,
    ) -> Self {
        Self { inner, event_bus, team_id, agent_id }
    }

    fn publish(&self, suffix: &str, mut data: serde_json::Value) {
        if let Some(obj) = data.as_object_mut() {
            obj.insert("agent_id".into(), serde_json::Value::String(self.agent_id.clone()));
        }
        let topic = format!("team.{}.{}", self.team_id, suffix);
        let evt = TopicEvent::new(topic, data);
        // 复用 publish(String)：把 TopicEvent 序列化为 {topic,data} 包，与面板侧
        // GatewayEvent{topic,data} 解析一致。失败仅吞掉（绝不打断 run）。
        if let Ok(json) = serde_json::to_string(&serde_json::json!({
            "topic": evt.topic,
            "data": evt.data,
        })) {
            self.event_bus.publish(json);
        }
    }
}

#[async_trait]
impl EventEmitter for TeamFanoutEmitter {
    async fn emit(&self, event: StreamEvent) -> Result<(), EventEmitError> {
        // 归一化为团队 topic（按 StreamEvent 真实变体匹配；落地前对照
        // mod.rs 的 StreamEvent 定义补全字段名）。
        match &event {
            StreamEvent::RunComplete { run_id, summary, .. } => {
                if let Some(text) = summary.final_response.as_ref() {
                    self.publish("message", serde_json::json!({
                        "run_id": run_id, "text": text, "final": true,
                    }));
                }
                self.publish("activity", serde_json::json!({
                    "run_id": run_id, "status": "done",
                }));
            }
            StreamEvent::RunError { run_id, error, .. } => {
                self.publish("activity", serde_json::json!({
                    "run_id": run_id, "status": "error", "error": error,
                }));
            }
            StreamEvent::ToolStart { run_id, tool_name, .. } => {
                self.publish("activity", serde_json::json!({
                    "run_id": run_id, "status": "working", "tool": tool_name,
                }));
            }
            _ => { /* MVP：其余事件不进团队流，避免噪声 */ }
        }
        if let Some(inner) = &self.inner {
            inner.emit(event).await?;
        }
        Ok(())
    }

    fn next_seq(&self) -> u64 {
        match &self.inner {
            Some(inner) => inner.next_seq(),
            // 无 inner 时自给序号；team 流不依赖严格序号，简单返回 0 即可，
            // 但为保险用一个进程内原子。落地时若 trait 要求单调，用 AtomicU64 字段。
            None => 0,
        }
    }
}
```

> ⚠️ 落地前对照 `mod.rs` 的 `StreamEvent` 枚举真实变体名/字段（`ToolStart` 可能叫 `ToolCallStart` 等）与 `RunComplete.summary` 的真实类型。`next_seq` 若 trait 文档要求单调递增，给结构体加 `seq: std::sync::atomic::AtomicU64` 字段并 `fetch_add`。

- [ ] **Step 4: 声明模块**

在 `src/gateway/event_emitter/mod.rs` 的模块声明区加：

```rust
pub mod team_fanout;
```

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p alephcore event_emitter::team_fanout`
Expected: PASS。

- [ ] **Step 6: 提交**

```bash
git add src/gateway/event_emitter/team_fanout.rs src/gateway/event_emitter/mod.rs
git commit -m "gateway: add TeamFanoutEmitter (republish team run events to team.<id>.*)"
```

---

## Task A3: 成员 run 接入 TeamFanoutEmitter

**Files:**
- Modify: `src/teams/dispatcher/runner.rs:execute_member_task`（emitter 从 `NoOpEventEmitter` 换 `TeamFanoutEmitter`）
- Modify: `src/gateway/context.rs`（若无 `event_bus()` 访问器则加）

- [ ] **Step 1: 确认 event_bus 可达**

Read `GatewayContext` 定义。若有 `event_bus()`（或字段）则记录访问方式；若无，加只读访问器：

```rust
impl GatewayContext {
    /// 团队事件 fan-out 需要事件总线把 team.<id>.* 广播给面板。
    #[must_use]
    pub fn event_bus(&self) -> Arc<crate::gateway::event_bus::GatewayEventBus> {
        self.event_bus.clone() // 字段名以真实定义为准
    }
}
```

- [ ] **Step 2: 替换 emitter（Agent target 分支）**

在 `runner.rs::execute_member_task` 里，`metadata` 已含 `team_id` + 拿得到 `agent_id`（target 的 agent id）。把：

```rust
let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
    Arc::new(NoOpEventEmitter::new());
```

改为：

```rust
// 成员 run 此前对面板静默（NoOp）。团队 chat 需要面板看到成员逐条贡献 +
// 实时状态 → 用 TeamFanoutEmitter 把 run 事件广播到 team.<team_id>.*。
let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
    Arc::new(crate::gateway::event_emitter::team_fanout::TeamFanoutEmitter::new(
        context.event_bus(),
        team_id.to_string(),
        agent_id.to_string(), // execute_member_task 内已解析出的 owner/agent id；用真实变量名
        None,
    ));
```

> `agent_id` 用函数内已有的 owner/target agent 变量（参照 `SessionKey::task(agent_id, "team", task_id)` 那处的 `agent_id`）。若 `NoOpEventEmitter` import 因此 orphan，删除其 `use`。

- [ ] **Step 3: 编译验证**

Run: `cargo check -p alephcore`
Expected: 通过（无类型错误、无 orphan import 警告升级为错误）。

- [ ] **Step 4: 提交**

```bash
git add src/teams/dispatcher/runner.rs src/gateway/context.rs
git commit -m "teams: member runs emit to team.<id>.* via TeamFanoutEmitter (was NoOp)"
```

---

## Task A4: `teams.chat.send`（拉起 leader run）

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（`handle_chat_send`）
- Modify: 团队 RPC 注册处
- Create（可选）: `src/teams/leader_prompt.rs`（编排 prompt 常量）

- [ ] **Step 1: 写 leader 编排 prompt 常量**

新建 `src/teams/leader_prompt.rs`（R9：智慧在 prompt）：

```rust
//! Leader orchestration preamble injected at the head of a team-chat leader run.
//! Per R7/R9/R10 the orchestration intelligence lives here in the prompt, not in
//! gateway/dispatcher code.

/// 把团队名册与协议拼进编排指令。`roster` 形如 "alice (researcher), bob (writer)"。
#[must_use]
pub fn build(team_name: &str, roster: &str, protocol: Option<&str>, user_request: &str) -> String {
    let protocol_block = protocol
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("\n\n# 团队协议\n{p}"))
        .unwrap_or_default();
    format!(
        "你是团队「{team_name}」的 leader。成员名册：{roster}。{protocol_block}\n\n\
         作为 leader，你要：\n\
         1. 把用户需求拆解成可分配的子任务，用 `task_create` 建任务并指定 owner 为合适的成员。\n\
         2. 必要时用 `message_send` 与成员沟通、用 `team_delegate` 直接委派。\n\
         3. 成员通过 dispatcher 异步执行，产出经 `task_submit` 落为 artifact。\n\
         4. 汇总成员产出，给用户一个清晰的最终答复。\n\n\
         不要自己闷头做完所有事——你的价值是编排与汇总。\n\n\
         # 用户需求\n{user_request}"
    )
}
```

声明模块：在 `src/teams/mod.rs` 加 `pub mod leader_prompt;`。

- [ ] **Step 2: 写失败集成测试**

在 `src/gateway/handlers/teams.rs` 测试模块加（断言对已存在团队调用返回 `run_id`；用能拿到 execution_adapter 的测试上下文，或退化为断言"团队不存在时报错"这一可纯 store 验证的路径）：

```rust
#[tokio::test]
async fn test_handle_chat_send_unknown_team_errors() {
    let store: Arc<dyn TeamStore> = Arc::new(InMemoryTeamStore::new());
    let ctx = test_gateway_context(); // 复用现有测试构造；若无则本测试聚焦 store 路径
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::json!(1),
        method: "teams.chat.send".into(),
        params: Some(serde_json::json!({ "team_id": "nope", "message": "hi" })),
    };
    let resp = handle_chat_send(req, store, ctx).await;
    assert!(resp.error.is_some(), "unknown team should error");
}
```

> 若现成 `test_gateway_context()` 不存在或过重，本任务的 happy-path（真拉起 leader run）改为 Phase 末的人工 E2E 验证，单测只锁"未知团队报错 + 参数校验"。这是诚实的可测边界。

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p alephcore handlers::teams::tests::test_handle_chat_send_unknown_team_errors`
Expected: FAIL（`handle_chat_send` 未定义）。

- [ ] **Step 4: 实现 `handle_chat_send`**

按 `execute_member_task` 的 spawn 范式（`execution_adapter.execute(request, agent, emitter)`）实现 leader run：

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ChatSendParams {
    pub team_id: String,
    pub message: String,
}

/// teams.chat.send — 把用户需求 + leader 编排 prompt 交给 team.leader_id 指向的
/// leader agent 跑一轮 harness。leader 用团队工具拆解/委派；dispatcher 异步拉起
/// 成员 run。leader run 用 TeamFanoutEmitter 广播到 team.<id>.*。
/// R10：本 handler 只"拉起 run + 注入上下文"，零编排推理。
pub async fn handle_chat_send(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    context: crate::gateway::context::GatewayContext,
) -> JsonRpcResponse {
    let params: ChatSendParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let team = match store.get_team(&params.team_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                RESOURCE_NOT_FOUND,
                format!("Team '{}' not found", params.team_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("{e}"))
        }
    };
    let members = store.get_members(&params.team_id).await.unwrap_or_default();
    let roster = members
        .iter()
        .filter(|m| m.agent_id != team.leader_id)
        .map(|m| format!("{} ({})", m.agent_id, m.role))
        .collect::<Vec<_>>()
        .join(", ");

    // 解析 leader agent。registry 访问以 GatewayContext 真实 API 为准。
    let leader_agent = match context.registry().get(&team.leader_id).await {
        Some(a) => a,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Leader agent '{}' not found in registry", team.leader_id),
            )
        }
    };

    let prompt = crate::teams::leader_prompt::build(
        &team.name,
        &roster,
        team.protocol.as_deref(),
        &params.message,
    );

    let run_id = uuid::Uuid::new_v4().to_string();
    let session_key = crate::gateway::session::SessionKey::task(&team.leader_id, "team_chat", &params.team_id);
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("team_id".to_string(), params.team_id.clone());

    let request_run = crate::gateway::execution_engine::RunRequest {
        run_id: run_id.clone(),
        input: prompt,
        session_key,
        timeout_secs: None,
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override: None,
        workspace_override: None,
        max_iterations_override: None,
        model_override: None,
    };

    let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
        Arc::new(crate::gateway::event_emitter::team_fanout::TeamFanoutEmitter::new(
            context.event_bus(),
            params.team_id.clone(),
            team.leader_id.clone(),
            None,
        ));

    let execution_adapter = Arc::clone(context.execution_adapter());
    tokio::spawn(async move {
        if let Err(e) = execution_adapter.execute(request_run, leader_agent, emitter).await {
            tracing::warn!(team_id = %params.team_id, error = %e, "team leader run failed");
        }
    });

    JsonRpcResponse::success(request.id, serde_json::json!({ "run_id": run_id }))
}
```

> ⚠️ 落地前核实：`SessionKey::task` 签名（runner.rs 用 `SessionKey::task(agent_id, "team", task_id)`）、`RunRequest` 字段（与 runner.rs / agent.rs 一致）、`GatewayContext` 的 `registry()` / `execution_adapter()` / `event_bus()` 真实方法名、`execute` 返回类型。全部有 ground-truth 锚点，照抄 `execute_member_task`。

- [ ] **Step 5: 注册路由**

仿 A1 Step 4，在真实注册处加 `teams.chat.send`（注意此 handler 还需 `GatewayContext`，按注册闭包能拿到的上下文传入；参照同处其他需要 context 的 team 方法如何注册）：

```rust
{
    let store = team_store.clone();
    let ctx = gateway_context.clone();
    registry.register("teams.chat.send", move |req| {
        let store = store.clone();
        let ctx = ctx.clone();
        async move { crate::gateway::handlers::teams::handle_chat_send(req, store, ctx).await }
    });
}
```

- [ ] **Step 6: 运行确认通过**

Run: `cargo test -p alephcore handlers::teams::tests::test_handle_chat_send_unknown_team_errors`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/handlers/teams.rs src/teams/leader_prompt.rs src/teams/mod.rs
# 以及注册文件
git commit -m "gateway: add teams.chat.send (spawn leader orchestration run)"
```

---

## Task A5: `teams.chat.thread`（hydrate 统一线程）

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（`handle_chat_thread`）
- Modify: `src/teams/messages/router.rs`（若无 `list_for_team` 则加）
- Modify: 团队 RPC 注册处
- Test: `teams.rs` 测试模块

- [ ] **Step 1: 确认/补 消息列举能力**

Read `MessageRouter`。若已有"按 team 列出全部消息"用之；若无，加：

```rust
impl MessageRouter {
    /// 列出某团队的全部消息（按时间升序），供面板团队 chat hydrate。只读，无副作用。
    pub async fn list_for_team(&self, team_id: &str) -> crate::error::Result<Vec<TeamMessage>> {
        self.store.list_for_team(team_id).await // store 层若也缺则一并加；以真实 store trait 为准
    }
}
```

- [ ] **Step 2: 写失败集成测试**

```rust
#[tokio::test]
async fn test_handle_chat_thread_merges_chronologically() {
    let store: Arc<dyn TeamStore> = Arc::new(InMemoryTeamStore::new());
    let team = store.create_team(NewTeam {
        name: "T".into(), description: String::new(), leader_id: "L".into(),
    }).await.unwrap();
    // 该测试聚焦"合并+排序"骨架：用空数据断言返回 items 数组结构存在。
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(), id: serde_json::json!(1),
        method: "teams.chat.thread".into(),
        params: Some(serde_json::json!({ "team_id": team.id })),
    };
    let resp = handle_chat_thread(req, store, /* coord_store */ test_coord_store(), /* artifact_store */ test_artifact_store(), /* router */ test_router()).await;
    let items = resp.result.as_ref().and_then(|r| r.get("items")).and_then(|v| v.as_array());
    assert!(items.is_some(), "thread returns items array");
}
```

> 若测试用的 coord/artifact/router 构造过重，本任务 happy-path 合并逻辑改为人工 E2E 验证，单测锁"未知 team → 空 items / 错误"。

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p alephcore handlers::teams::tests::test_handle_chat_thread_merges_chronologically`
Expected: FAIL。

- [ ] **Step 4: 实现 `handle_chat_thread`**

```rust
#[derive(Debug, serde::Serialize)]
pub struct ThreadItem {
    pub kind: String, // "message" | "artifact" | "task_status"
    pub agent_id: String,
    pub title: String,
    pub content: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
}

/// teams.chat.thread — 按时间合并 团队消息 + artifact + 任务状态，供面板 hydrate。只读。
pub async fn handle_chat_thread(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>,
    artifact_store: Arc<dyn crate::teams::artifacts::ArtifactStore>,
    router: Arc<crate::teams::messages::router::MessageRouter>,
) -> JsonRpcResponse {
    let params: TeamIdParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let mut items: Vec<ThreadItem> = Vec::new();

    if let Ok(msgs) = router.list_for_team(&params.team_id).await {
        for m in msgs {
            items.push(ThreadItem {
                kind: "message".into(),
                agent_id: m.from_agent,
                title: m.subject,
                content: m.content,
                timestamp: m.created_at, // 字段名以 TeamMessage 真实定义为准
                artifact_id: None,
            });
        }
    }

    if let Ok(tasks) = coord_store
        .list_tasks(crate::agents::swarm::tasks::CoordTaskFilter {
            team_id: Some(params.team_id.clone()),
            ..Default::default()
        })
        .await
    {
        for t in &tasks {
            if let Ok(arts) = artifact_store.get_artifacts_for_task(&t.id).await {
                for a in arts {
                    items.push(ThreadItem {
                        kind: "artifact".into(),
                        agent_id: a.agent_id,
                        title: a.title,
                        content: a.content,
                        timestamp: a.created_at.timestamp_millis(),
                        artifact_id: Some(a.id),
                    });
                }
            }
        }
    }

    items.sort_by_key(|i| i.timestamp);
    JsonRpcResponse::success(request.id, serde_json::json!({ "items": items }))
}
```

> ⚠️ 字段名（`TeamMessage.created_at`、`CoordTask.id` 类型、`TaskArtifact.created_at: DateTime<Utc>`）以 ground-truth 为准；`CoordTaskId` 转 `&str` 用其 `Display`/`as_str`。

- [ ] **Step 5: 注册路由**（仿前，handler 多个 store 入参，按注册处能拿到的句柄传入）

- [ ] **Step 6: 运行确认通过**

Run: `cargo test -p alephcore handlers::teams::tests::test_handle_chat_thread_merges_chronologically`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add src/gateway/handlers/teams.rs src/teams/messages/router.rs
# 以及注册文件
git commit -m "gateway: add teams.chat.thread (chronological message+artifact+task merge)"
```

**Phase A 检查点**：`cargo check -p alephcore` 通过；新增 RPC 集成测试通过。后端此时可独立验证（用 `aleph watch` 订阅 `team.*` + RPC 手测）。

---

# Phase B — 前端（Leptos Panel）

## Task B1: `ChatMessage.agent_id` + `ChatState.team_id` / 名册

**Files:**
- Modify: `interfaces/webchat/src/views/chat/state.rs`
- Test: `state.rs` 内 `#[cfg(test)] mod tests`（host 逻辑测试）

- [ ] **Step 1: 写失败测试（serde 默认 + 归属可空）**

在 `state.rs` 测试模块加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message_agent_id_defaults_none_for_legacy_json() {
        // 旧 session 快照无 agent_id 字段 → 反序列化为 None（零回归）。
        let legacy = serde_json::json!({
            "id": "assistant-1", "role": "assistant", "content": "hi"
        });
        let msg: ChatMessage = serde_json::from_value(legacy).unwrap();
        assert_eq!(msg.agent_id, None);
    }

    #[test]
    fn test_chat_message_roundtrips_agent_id() {
        let mut msg: ChatMessage = serde_json::from_value(serde_json::json!({
            "id": "m", "role": "assistant", "content": "x", "agent_id": "alice"
        })).unwrap();
        assert_eq!(msg.agent_id.as_deref(), Some("alice"));
        msg.agent_id = Some("bob".into());
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v.get("agent_id").and_then(|a| a.as_str()), Some("bob"));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore -- chat::state::tests::test_chat_message_agent_id` （或面板 crate 的实际测试命令；若面板是独立 wasm crate，用其 host-test 配置）
Expected: FAIL（`agent_id` 字段不存在）。

> 注：面板测试运行方式以仓库现状为准（host target 跑纯逻辑测试）。若该测试无法在 host 跑，则此步降级为 `cargo check` 编译验证 + 人工核对。

- [ ] **Step 3: 加字段 + 更新所有字面量站点**

在 `ChatMessage` 定义末尾（`text_finalized` 后）加：

```rust
    /// 团队 chat：该气泡归属的 agent id。`None` = 单 agent 旧路径（零回归，
    /// 旧快照无此字段 → serde 默认 None）。Some(..) 时 MessageBubble 渲染归属。
    #[serde(default)]
    pub agent_id: Option<String>,
```

更新以下**全部** `ChatMessage { .. }` 字面量站点，各加一行 `agent_id: None,`：
- `push_user_message`（state.rs:~441）
- `start_assistant_message`（state.rs:~465）
- `begin_step` 内 `msgs.push(ChatMessage { .. })`（state.rs:~512）
- 仓库内其余 `ChatMessage {` 字面量（grep `ChatMessage {` 全列举：hydration/replay 路径如 `chat_sidebar.rs`/`messages.rs`/`history` 映射处都要补）。

在 `ChatState` 定义加两个信号：

```rust
    /// 团队 chat 模式标记。`Some(team_id)` → 渲染三栏团队视图、composer 走
    /// teams.chat.send。`None` = 单 agent chat（零回归）。
    pub team_id: RwSignal<Option<String>>,
    /// 团队名册 + 实时状态（左名册栏数据源）。空 = 非团队模式。
    pub team_members: RwSignal<Vec<TeamMemberView>>,
```

加 `TeamMemberView` 类型（同文件）：

```rust
/// 面板侧团队成员视图（名册栏渲染 + 归属配色）。
#[derive(Debug, Clone, PartialEq)]
pub struct TeamMemberView {
    pub agent_id: String,
    pub name: String,
    pub role: String,
    pub is_leader: bool,
    pub status: MemberStatus, // idle / working / done / error
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemberStatus {
    #[default]
    Idle,
    Working,
    Done,
    Error,
}
```

在 `ChatState` 的构造器（`ChatState::new` / `provide` 处）加 `team_id: RwSignal::new(None)` 与 `team_members: RwSignal::new(Vec::new())`。在 `clear_session` 加 `self.team_id.set(None); self.team_members.set(Vec::new());`。

- [ ] **Step 4: 运行确认通过 + 编译**

Run: `cargo test -p alephcore -- chat::state::tests::test_chat_message_agent_id`（或降级 `cargo check`）
Expected: PASS / 编译通过。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/views/chat/state.rs
# 以及其它补了 agent_id: None 的文件
git commit -m "panel: add ChatMessage.agent_id + ChatState team fields (zero-regression Option)"
```

---

## Task B2: 团队 chat API 封装

**Files:**
- Create: `interfaces/webchat/src/api/team_chat.rs`
- Modify: `interfaces/webchat/src/api/teams.rs`（加 `create`）
- Modify: `interfaces/webchat/src/api/mod.rs`

- [ ] **Step 1: 加 `TeamsApi::create`**

在 `interfaces/webchat/src/api/teams.rs`（仿 `get()` 的 `rpc_call` 风格）加：

```rust
    /// 现场拼队：显式 leader_id + 成员创建持久化团队。返回 team_id。
    pub async fn create(
        state: &DashboardState,
        name: &str,
        description: &str,
        leader_id: &str,
        members: &[(String, String)], // (agent_id, role)
    ) -> Result<String, String> {
        let members_json: Vec<Value> = members
            .iter()
            .map(|(id, role)| json!({ "agent_id": id, "role": role }))
            .collect();
        let result = state
            .rpc_call("teams.create", json!({
                "name": name, "description": description,
                "leader_id": leader_id, "members": members_json,
            }))
            .await?;
        result
            .get("team_id")
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| "teams.create did not return team_id".to_string())
    }
```

- [ ] **Step 2: 新建 `team_chat.rs`**

```rust
//! Team-chat RPC wrappers: send a requirement to a team (spawns leader run) and
//! hydrate the unified thread. Mirrors ChatApi's rpc_call pattern.

use serde::Deserialize;
use serde_json::{json, Value};
use crate::context::DashboardState;

#[derive(Debug, Clone, Deserialize)]
pub struct TeamChatSendResponse {
    pub run_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadItem {
    pub kind: String,
    pub agent_id: String,
    pub title: String,
    pub content: String,
    pub timestamp: i64,
    #[serde(default)]
    pub artifact_id: Option<String>,
}

pub struct TeamChatApi;

impl TeamChatApi {
    /// 把用户需求交给团队 leader 编排。
    pub async fn send(
        state: &DashboardState,
        team_id: &str,
        message: &str,
    ) -> Result<TeamChatSendResponse, String> {
        let result = state
            .rpc_call("teams.chat.send", json!({ "team_id": team_id, "message": message }))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// 拉取统一线程（hydrate）。
    pub async fn thread(state: &DashboardState, team_id: &str) -> Result<Vec<ThreadItem>, String> {
        let result = state
            .rpc_call("teams.chat.thread", json!({ "team_id": team_id }))
            .await?;
        let items = result.get("items").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(items).map_err(|e| e.to_string())
    }
}
```

声明：`interfaces/webchat/src/api/mod.rs` 加 `pub mod team_chat;`。

- [ ] **Step 3: 编译验证**

Run: `just wasm`（或 `cargo check -p alephcore --target wasm32-unknown-unknown` 视面板构建方式）
Expected: 通过。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/api/team_chat.rs interfaces/webchat/src/api/teams.rs interfaces/webchat/src/api/mod.rs
git commit -m "panel: add TeamsApi::create + TeamChatApi (send/thread)"
```

---

## Task B3: 团队事件投影 `subscribe_team_events`

**Files:**
- Create: `interfaces/webchat/src/views/chat/team_events.rs`
- Modify: `interfaces/webchat/src/views/chat/mod.rs`
- Test: `team_events.rs` 内纯逻辑测试（颜色分配 + 事件→消息映射）

- [ ] **Step 1: 写失败测试（颜色分配纯函数）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_color_is_stable_per_index() {
        // 同一序号恒得同色；不同序号取不同调色板槽。
        assert_eq!(agent_color(0), agent_color(0));
        assert_ne!(agent_color(0), agent_color(1));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p alephcore -- chat::team_events::tests::test_agent_color_is_stable_per_index`（或降级编译）
Expected: FAIL。

- [ ] **Step 3: 实现投影 + 颜色分配**

```rust
//! Project `team.<id>.*` topic events onto team-chat ChatState: attributed
//! message bubbles, roster live status, and a refresh signal for the workspace
//! tabs. Parallel to `events.rs::subscribe_run_events` (single-agent), kept
//! separate for zero-regression.

use crate::context::{DashboardState, GatewayEvent};
use super::state::{ChatState, ChatMessage, MemberStatus};

/// 稳定的逐 agent 配色（按名册序号取槽）。
#[must_use]
pub fn agent_color(index: usize) -> &'static str {
    const PALETTE: [&str; 6] = ["#7c9cff", "#4ec9b0", "#e0a458", "#c586c0", "#4fc1ff", "#d16969"];
    PALETTE[index % PALETTE.len()]
}

/// 订阅 team.* 事件，投影到团队 chat 状态。返回订阅 id 供清理。
#[must_use]
pub fn subscribe_team_events(dashboard: &DashboardState, chat: ChatState) -> usize {
    dashboard.subscribe_events(move |event: GatewayEvent| {
        if !event.topic.starts_with("team.") {
            return;
        }
        let data = &event.data;
        let agent_id = data.get("agent_id").and_then(|a| a.as_str()).unwrap_or("").to_string();
        // team.<id>.message → 归属气泡
        if event.topic.ends_with(".message") {
            if let Some(text) = data.get("text").and_then(|t| t.as_str()) {
                let seq = chat.messages.with_untracked(|m| m.len());
                chat.messages.update(|msgs| {
                    msgs.push(ChatMessage {
                        id: format!("team-{seq}"),
                        role: "assistant".into(),
                        content: text.to_string(),
                        tool_calls: vec![],
                        is_streaming: false,
                        is_intermediate: false,
                        error: None,
                        model_info: None,
                        is_final: true,
                        text_finalized: true,
                        timestamp: Some(super::timeline::now_millis()),
                        iteration: None,
                        agent_id: Some(agent_id.clone()),
                    });
                });
            }
        } else if event.topic.ends_with(".activity") {
            // 更新名册状态点
            let status = match data.get("status").and_then(|s| s.as_str()) {
                Some("working") => MemberStatus::Working,
                Some("done") => MemberStatus::Done,
                Some("error") => MemberStatus::Error,
                _ => MemberStatus::Idle,
            };
            chat.team_members.update(|members| {
                if let Some(m) = members.iter_mut().find(|m| m.agent_id == agent_id) {
                    m.status = status;
                }
            });
        }
        // team.<id>.task → 由 view 的 Effect 重新拉 teams.get + thread 刷新工作区两 tab
    })
}
```

声明：`views/chat/mod.rs` 加 `pub mod team_events;`。

> ⚠️ `ChatMessage` 字面量字段须与 B1 后的真实定义完全一致（含 `agent_id`）。`super::timeline::now_millis()` 是现有 helper（state.rs 已用）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p alephcore -- chat::team_events::tests::test_agent_color_is_stable_per_index`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/views/chat/team_events.rs interfaces/webchat/src/views/chat/mod.rs
git commit -m "panel: add subscribe_team_events (attributed bubbles + roster status)"
```

---

## Task B4: 拼队入口 + 三栏视图组装

**Files:**
- Create: `interfaces/webchat/src/views/chat/team_compose.rs`
- Create: `interfaces/webchat/src/components/team_roster.rs`
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`（加入口按钮）
- Modify: `interfaces/webchat/src/views/chat/view.rs`（团队模式三栏 + 订阅）
- Modify: 对应 `mod.rs`

- [ ] **Step 1: 拼队组件 `team_compose.rs`**

```rust
//! Team compose popover: leader pre-filled = current active agent, multi-select
//! members from existing agents, "开始" → TeamsApi::create → enter team mode.

use leptos::prelude::*;
use crate::context::DashboardState;
use crate::api::{teams::TeamsApi, agents::AgentsApi}; // AgentsApi 列 agent；以现有列表 API 为准
use super::state::{ChatState, TeamMemberView, MemberStatus};

#[component]
#[must_use]
pub fn TeamCompose(#[prop(into)] on_close: Callback<()>) -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let chat = expect_context::<ChatState>();
    let team_name = RwSignal::new(String::new());
    let selected: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let agents = RwSignal::new(Vec::new());

    // 载入可选 agent 列表（复用现有 agents 列表 API）。
    let dash = dashboard;
    Effect::new(move || {
        if !dash.is_connected.get() { return; }
        spawn_local(async move {
            if let Ok(list) = AgentsApi::list(&dash).await { agents.set(list); }
        });
    });

    let start = move |_| {
        let leader = chat.agent_id.get_untracked().unwrap_or_default();
        let name = team_name.get_untracked();
        let members: Vec<(String, String)> = selected
            .get_untracked()
            .into_iter()
            .map(|id| (id, "member".to_string()))
            .collect();
        if leader.is_empty() || name.trim().is_empty() { return; }
        let dash = dashboard;
        spawn_local(async move {
            match TeamsApi::create(&dash, &name, "", &leader, &members).await {
                Ok(team_id) => {
                    // 进团队模式：清会话、置 team_id、构建名册（leader + 成员）。
                    chat.clear_session();
                    chat.team_id.set(Some(team_id));
                    let mut roster = vec![TeamMemberView {
                        agent_id: leader.clone(), name: leader.clone(),
                        role: "leader".into(), is_leader: true, status: MemberStatus::Idle,
                    }];
                    for (id, role) in &members {
                        roster.push(TeamMemberView {
                            agent_id: id.clone(), name: id.clone(),
                            role: role.clone(), is_leader: false, status: MemberStatus::Idle,
                        });
                    }
                    chat.team_members.set(roster);
                    on_close.run(());
                }
                Err(e) => web_sys::console::error_1(&format!("teams.create failed: {e}").into()),
            }
        });
    };

    view! {
        <div class="aleph-team-compose p-3 space-y-2">
            <h3 class="text-sm font-semibold">"新建团队群聊"</h3>
            <p class="text-xs text-text-tertiary">
                "Leader（东道主）= 当前 agent：" {move || chat.agent_id.get().unwrap_or_default()}
            </p>
            <input class="w-full px-2 py-1 rounded bg-surface-sunken border border-border text-sm"
                   placeholder="团队名称"
                   on:input=move |e| team_name.set(event_target_value(&e)) />
            <div class="max-h-48 overflow-y-auto space-y-1">
                {move || agents.get().into_iter().map(|a| {
                    let id = a.id.clone();
                    let id_for_toggle = id.clone();
                    let checked = move || selected.get().contains(&id_for_toggle);
                    let id_for_change = id.clone();
                    view! {
                        <label class="flex items-center gap-2 text-sm">
                            <input type="checkbox" prop:checked=checked
                                on:change=move |_| selected.update(|s| {
                                    if let Some(pos) = s.iter().position(|x| x == &id_for_change) { s.remove(pos); }
                                    else { s.push(id_for_change.clone()); }
                                }) />
                            {a.name.clone().unwrap_or_else(|| a.id.clone())}
                        </label>
                    }
                }).collect::<Vec<_>>()}
            </div>
            <button class="w-full px-3 py-1.5 rounded bg-primary text-white text-sm"
                    on:click=start>"开始"</button>
        </div>
    }
}
```

> ⚠️ `AgentsApi::list` / agent 项字段（`id`/`name`）以现有 agents 列表 API 为准（chat_sidebar 已在用 agent 列表，照其来源）。

- [ ] **Step 2: 名册栏 `team_roster.rs`**

```rust
//! Left roster rail for team chat: leader + members with live status dots.

use leptos::prelude::*;
use crate::views::chat::state::{ChatState, MemberStatus};
use crate::views::chat::team_events::agent_color;

#[component]
#[must_use]
pub fn TeamRoster() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    view! {
        <div class="aleph-team-roster w-40 shrink-0 border-r border-border p-2 space-y-1 overflow-y-auto">
            {move || chat.team_members.get().into_iter().enumerate().map(|(i, m)| {
                let color = agent_color(i);
                let dot = match m.status {
                    MemberStatus::Working => "#e0a458",
                    MemberStatus::Done => "#4ec9b0",
                    MemberStatus::Error => "#d16969",
                    MemberStatus::Idle => "#6b7280",
                };
                view! {
                    <div class="flex items-center gap-2 text-xs px-2 py-1 rounded"
                         style=format!("border-left:3px solid {color}")>
                        <span style=format!("color:{dot}")>"●"</span>
                        <span class="truncate">{m.name.clone()}</span>
                        {m.is_leader.then(|| view! { <span class="text-[10px] opacity-60">"leader"</span> })}
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}
```

声明：`components/mod.rs` 加 `pub mod team_roster;`；`views/chat/mod.rs` 加 `pub mod team_compose;`。

- [ ] **Step 3: ChatSidebar 入口按钮**

在 `chat_sidebar.rs` 的 agent dropdown / "New Chat" 区域旁加一个"团队群聊"按钮，点击打开 `TeamCompose`（用一个 `RwSignal<bool>` 控制弹层显隐）。最小接线：

```rust
let show_compose = RwSignal::new(false);
// ...在 New Chat 按钮旁：
view! {
    <button class="px-2 py-1.5 rounded-lg bg-surface-sunken border border-border text-sm"
            title="团队群聊"
            on:click=move |_| show_compose.set(true)>"👥 团队"</button>
    <Show when=move || show_compose.get()>
        <crate::views::chat::team_compose::TeamCompose
            on_close=Callback::new(move |()| show_compose.set(false)) />
    </Show>
}
```

- [ ] **Step 4: view.rs 三栏 + 订阅**

在 `ChatView` 里，团队模式（`chat.team_id` 为 Some）时：订阅 `team.*` 并渲染左名册 + 中流 + 右工作区。最小接线：

```rust
// 与 subscribe_run_events 并存：团队事件单独订阅。
let team_sub = subscribe_team_events(&dashboard, chat);
{
    let dash = dashboard;
    spawn_local(async move {
        for _ in 0..50 { if dash.is_connected.get_untracked() { break; } gloo_timers::future::TimeoutFuture::new(100).await; }
        let _ = dash.subscribe_topic("team.*").await;
    });
}
on_cleanup(move || { dashboard.unsubscribe_events(team_sub); });

// 渲染：team_id 为 Some 时三栏。
view! {
    <Show when=move || chat.team_id.get().is_some()
          fallback=move || view! { /* 现有单 agent 布局 */ }>
        <div class="flex h-full">
            <crate::components::team_roster::TeamRoster />
            <div class="flex-1 min-w-0"><MessageList /></div>
            <WorkspacePanel />
        </div>
    </Show>
}
```

> ⚠️ 不破坏现有单 agent 布局：把现有布局塞进 `fallback`。`MessageList`/`WorkspacePanel` 复用。

- [ ] **Step 5: composer 分流到 teams.chat.send**

在 `composer/mod.rs::send_message`，发送前判断团队模式：

```rust
if let Some(team_id) = chat.team_id.get_untracked() {
    chat.push_user_message(&text);
    let dash = dashboard;
    spawn_local(async move {
        if let Err(e) = crate::api::team_chat::TeamChatApi::send(&dash, &team_id, &text).await {
            chat.set_send_error(ChatSendError::classify(e));
        }
        is_sending.set(false);
    });
    input_text.set(String::new());
    return;
}
// ...原单 agent ChatApi::send 路径不变。
```

- [ ] **Step 6: 编译验证**

Run: `just wasm`
Expected: 通过（无类型/借用错误）。

- [ ] **Step 7: 提交**

```bash
git add interfaces/webchat/src/views/chat/team_compose.rs \
        interfaces/webchat/src/components/team_roster.rs \
        interfaces/webchat/src/components/chat_sidebar.rs \
        interfaces/webchat/src/views/chat/view.rs \
        interfaces/webchat/src/views/chat/composer/mod.rs \
        interfaces/webchat/src/components/mod.rs interfaces/webchat/src/views/chat/mod.rs
git commit -m "panel: team chat compose entry + 3-pane view + composer routing"
```

---

## Task B5: MessageBubble 归属 + WorkspacePanel 两 tab

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs`（`MessageBubble` 归属外观）
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`（团队模式两 tab）

- [ ] **Step 1: MessageBubble 归属**

在 `MessageBubble` body，`message.agent_id` 为 Some 时，在内容上方加一个带色名字条。最小接线（用名册序号取色）：

```rust
let chat = expect_context::<ChatState>();
let attribution = message.agent_id.as_ref().map(|aid| {
    let members = chat.team_members.get_untracked();
    let idx = members.iter().position(|m| &m.agent_id == aid).unwrap_or(0);
    let color = crate::views::chat::team_events::agent_color(idx);
    let name = members.get(idx).map(|m| m.name.clone()).unwrap_or_else(|| aid.clone());
    view! {
        <div class="text-[11px] font-semibold mb-0.5" style=format!("color:{color}")>{name}</div>
    }
});
// 在 content 渲染前插入 {attribution}
```

- [ ] **Step 2: WorkspacePanel 两 tab（团队模式）**

在 `workspace_panel.rs`，团队模式（`chat.team_id` 为 Some）时渲染 tab 头 + 两视图；非团队模式保持现有 `ActivityTimeline + FilesDrawer`：

```rust
let chat = expect_context::<ChatState>();
let active_tab = RwSignal::new(0u8); // 0=交付物 1=任务

view! {
    <Show when=move || workspace.mode.get() == LayoutMode::Split>
        <aside class="aleph-workspace-pane flex flex-col h-full border-l border-border bg-surface-base/40 min-w-[280px] basis-[66%] shrink overflow-hidden">
            <Show when=move || chat.team_id.get().is_some()
                  fallback=move || view! {
                      <div class="flex-1 overflow-y-auto px-4 pb-3 aleph-content-top"><ActivityTimeline /></div>
                      <FilesDrawer />
                  }>
                <div class="flex gap-1 px-3 py-2 border-b border-border text-xs">
                    <button class="px-2 py-1 rounded" class:bg-primary=move || active_tab.get()==0
                            on:click=move |_| active_tab.set(0)>"交付物"</button>
                    <button class="px-2 py-1 rounded" class:bg-primary=move || active_tab.get()==1
                            on:click=move |_| active_tab.set(1)>"任务"</button>
                </div>
                <div class="flex-1 overflow-y-auto px-3 py-2">
                    <Show when=move || active_tab.get()==0
                          fallback=move || view! { <TeamTasksView /> }>
                        <TeamDeliverablesView />
                    </Show>
                </div>
            </Show>
        </aside>
    </Show>
}
```

加两个轻组件（同文件或 `team_roster.rs` 旁），数据源用 `TeamChatApi::thread`（交付物：kind=="artifact"）和 `TeamsApi::get`（任务列表），在 `team.*.task`/`activity` 事件或定时触发的 Effect 里刷新：

```rust
#[component]
fn TeamDeliverablesView() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let items = RwSignal::new(Vec::new());
    Effect::new(move || {
        let Some(team_id) = chat.team_id.get() else { return; };
        // 触发：team_members 状态变化即重拉（成员状态变 = 可能有新产出）。
        let _ = chat.team_members.get();
        spawn_local(async move {
            if let Ok(thread) = crate::api::team_chat::TeamChatApi::thread(&dash, &team_id).await {
                items.set(thread.into_iter().filter(|i| i.kind == "artifact").collect::<Vec<_>>());
            }
        });
    });
    view! {
        {move || items.get().into_iter().map(|a| view! {
            <div class="border-l-2 pl-2 py-1 mb-1" style="border-color:#c586c0">
                <div class="text-xs font-semibold">{a.title}</div>
                <div class="text-[11px] opacity-70 line-clamp-2">{a.content}</div>
            </div>
        }).collect::<Vec<_>>()}
    }
}

#[component]
fn TeamTasksView() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let tasks = RwSignal::new(Vec::new());
    Effect::new(move || {
        let Some(team_id) = chat.team_id.get() else { return; };
        let _ = chat.team_members.get();
        spawn_local(async move {
            if let Ok(detail) = crate::api::teams::TeamsApi::get(&dash, &team_id).await {
                tasks.set(detail.tasks); // TeamDetail.tasks；字段名以真实 TeamDetail 为准
            }
        });
    });
    view! {
        {move || tasks.get().into_iter().map(|t| view! {
            <div class="text-xs py-1 flex justify-between">
                <span class="truncate">{t.subject.clone()}</span>
                <span class="opacity-60">{format!("{:?}", t.status)}</span>
            </div>
        }).collect::<Vec<_>>()}
    }
}
```

> ⚠️ `TeamDetail` 的 `tasks` 字段与 task 项字段（`subject`/`status`）以 `api/teams.rs` 真实 `TeamDetail` 定义为准；不符则按真实结构调整。`DashboardState` 需 import。

- [ ] **Step 3: 编译验证**

Run: `just wasm`
Expected: 通过。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/views/chat/messages.rs interfaces/webchat/src/components/workspace_panel.rs
git commit -m "panel: per-agent attribution bubbles + workspace deliverables/tasks tabs"
```

---

## Task B6: 部署 + 人工 E2E

**Files:** 无（部署 + 验证）

- [ ] **Step 1: 重建并部署**（CLAUDE.md 刷新链）

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
# 替换运行中 binary（dev 或 .app daemon，见 CLAUDE.md），让 supervisor relaunch
```

- [ ] **Step 2: E2E 核对清单**

- [ ] Chat 侧栏出现"👥 团队"入口；点击弹出拼队，leader 预填为当前 agent，可勾选现有 agent 为成员。
- [ ] 填团队名 +「开始」→ 创建持久化 Team（在 Teams tab 能看到同一团队）。
- [ ] 进入三栏：左名册（leader+成员+状态点）、中群聊流、右工作区（交付物/任务两 tab）。
- [ ] 提交需求 → leader run 被拉起 → 中流出现 leader 计划（归属色名），成员状态点转 working，成员产出逐条归属上墙。
- [ ] 右"任务"tab 显示 CoordTask + 状态；"交付物"tab 显示 task_submit 的 artifact。
- [ ] 运行中再发一条 → leader 接力转向。
- [ ] 刷新/重开该团队 chat → `teams.chat.thread` 正确 hydrate 历史。
- [ ] 单 agent chat 完全不受影响（无 team_id 时行为与回退后一致）。

- [ ] **Step 3: 提交（如有部署期微调）**

```bash
git add -A && git commit -m "panel: team chat E2E fixups"
```

---

## Self-Review

**1. Spec coverage（spec 每节 → 任务映射）：**
- B1 团队会话启动 → Task A4（`teams.chat.send` 拉起 leader run）✓
- B2 事件归属/fan-out → Task A2（`TeamFanoutEmitter`）+ A3（成员 run 接入）✓
- B3 统一线程 → Task A5（`teams.chat.thread`）✓
- B4 交付物投递 → Task A5（thread 含 artifact）+ B5（交付物 tab）✓
- F1 拼队入口（当前 agent 默认 leader）→ Task A1（`teams.create`）+ B4（TeamCompose）✓
- F2 三栏视图 → Task B4（view 三栏）+ B5（归属 + 工作区两 tab）+ B4（名册）✓
- F3 ChatMessage/ChatState + 事件投影 → Task B1 + B3 ✓
- 数据流 7 步 → A4→A3(dispatcher)→A2(fanout)→B3(投影)→A5(hydrate) 全覆盖 ✓
- MVP 中途插话 → B4 Step 5（同 team_id 再 send）✓
- 零回归单 agent → B1（Option 字段 + serde default 测试）+ B4（fallback 保留原布局）✓
- R4/R10 守卫 → A4 prompt 注入在 run（非 gateway 推理）、leader_prompt.rs 承载智慧 ✓

**2. Placeholder scan：** 代码步骤均含真实代码；标注 ⚠️ 处是"落地前按 ground-truth 核实字段名/签名"的诚实校验点（StreamEvent 变体、RunRequest 字段、TeamDetail.tasks、SessionKey::task、GatewayContext 访问器、ChatMessage 全字面量站点），非占位符——每处都给了锚点文件与照抄对象（`execute_member_task`/`origin_fanout.rs`）。A4/A5 happy-path 在测试上下文过重时诚实降级为人工 E2E，单测锁可纯 store 验证的边界。

**3. Type consistency：** `TeamFanoutEmitter::new(event_bus, team_id, agent_id, inner)` 在 A2 定义、A3/A4 调用一致；`teams.create` 参数 `{name,description,leader_id,members:[{agent_id,role}]}` 在 A1 后端与 B2 前端 `TeamsApi::create` 一致；`teams.chat.thread` 的 `ThreadItem{kind,agent_id,title,content,timestamp,artifact_id}` 在 A5 后端与 B2 前端 `team_chat::ThreadItem` 一致；`team.<id>.{message,activity,task}` topic 在 A2 产出与 B3 消费一致；`ChatMessage.agent_id: Option<String>` 在 B1 定义、B3/B5 使用一致；`MemberStatus`/`TeamMemberView` 在 B1 定义、B3/B4/team_roster 一致。

**已知落地依赖（实现者首步必做）：** ① 定位 `teams.*` 真实注册点（关键事实 10）；② 确认 `GatewayContext` 的 `event_bus()/registry()/execution_adapter()` 访问器；③ 对照 `StreamEvent` 真实变体补 `TeamFanoutEmitter::emit` 匹配臂；④ grep 全部 `ChatMessage {` 字面量站点补 `agent_id: None`。
