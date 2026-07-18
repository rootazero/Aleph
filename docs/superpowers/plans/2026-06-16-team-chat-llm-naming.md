# 团队群聊 LLM 命名 + 重命名/删除对齐单聊 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让团队群聊像单聊一样在首条消息后由 LLM 自动命名（留空时），并支持在侧栏重命名 / 删除群聊，样式与操作逻辑与单聊一致。

**Architecture:** 团队保持独立实体。新增与单聊并行的 auto-name / rename / delete；唯一的 LLM 主题逻辑抽取为共享 helper（单聊 + 团队共用）。auto-name 由新增 `teams.name_auto` 标志位（首条消息时 take-and-clear）驱动，完成后经新增 `GatewayEventFrame::TeamChanged`（`team.changed` 主题，镜像 `CronJobChanged`）通知 Panel 刷新。删除复用 `teams.disband` + 侧栏只显示 `active`。

**Tech Stack:** Rust (alephcore lib + aleph-server bin, rusqlite, tokio, async_trait) · Leptos/WASM (aleph-panel) · JSON-RPC gateway。

**Spec:** `docs/superpowers/specs/2026-06-16-team-chat-llm-naming-design.md`

**Conventions for every task:**
- 项目单分支开发：commit 直接进 `main`。Commit message 格式 `<scope>: <description>`（English）。
- 后端测试用 targeted 命令：`cargo test -p alephcore --lib <name>`（**不要**跑全量 suite）。
- 后端编译检查：`cargo build -p alephcore --bin aleph-server`。
- 前端编译：`just wasm`（构建 `aleph-panel` WASM）。
- 部署看效果（仅最终 E2E 任务）：`just wasm` → 重编 `aleph-server` binary → 替换运行中的 daemon（见 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」）。

---

## Task 1: 新增 `GatewayEventFrame::TeamChanged` 事件帧

**Files:**
- Modify: `src/gateway/events/frame.rs`（enum 在 23-218；`topic_name()` 在 399-436；测试在 483-551）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/events/frame.rs` 的 `surface_notify_tests` 模块内（约 551 行 `}` 之前）追加：

```rust
    #[test]
    fn team_changed_topic_and_wire_shape() {
        let f = GatewayEventFrame::TeamChanged {
            team_id: "t1".to_string(),
            change: ChangeKind::Updated,
        };
        // Non-streaming → TopicEvent wire shape (topic + data), no stream method.
        assert_eq!(f.topic_name(), "team.changed");
        assert!(f.stream_method().is_none());

        // serde(tag = "type", rename_all = "snake_case")
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["type"], "team_changed");
        assert_eq!(v["team_id"], "t1");
        assert_eq!(v["change"], "updated");
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib team_changed_topic_and_wire_shape`
Expected: FAIL — `no variant named TeamChanged found for enum GatewayEventFrame`

- [ ] **Step 3: 加 enum 变体**

在 `src/gateway/events/frame.rs` 的 `GatewayEventFrame` enum 内，紧跟 `HeartbeatTaskChanged { .. }` 变体（约 192 行）之后插入：

```rust
    /// Emitted when a team's sidebar-visible metadata changes (LLM
    /// auto-name on first message, manual rename, or disband). Topic:
    /// `team.changed`. Mirrors `CronJobChanged` — payload-minimal; the
    /// Panel re-fetches `agents.teams` to refresh the group-chat list.
    TeamChanged {
        team_id: String,
        change: ChangeKind,
    },
```

- [ ] **Step 4: 加 `topic_name()` 匹配臂**

在 `topic_name()` 的 match（399-435）中，紧跟 `Self::HeartbeatTaskChanged { .. } => "heartbeat.task.changed",`（约 431 行）之后插入：

```rust
            Self::TeamChanged { .. } => "team.changed",
```

（`stream_method()` 的 `_ => None` 通配臂已覆盖新变体，无需改动。）

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p alephcore --lib team_changed_topic_and_wire_shape`
Expected: PASS

- [ ] **Step 6: 校验事件作用域是否需要 allowlist**

某些主题事件需在 `event_scope` 放行才能到达 Panel。检查 cron 先例是否被显式登记：

Run: `rg -n "cron.job.changed|heartbeat.task.changed|fn can_receive|GUARDED" src/gateway/event_scope.rs`
- 若 `event_scope.rs` 对 `cron.job.changed` 之类有**显式 allowlist**，在同处按相同方式加入 `"team.changed"`。
- 若没有显式 per-topic allowlist（cron 主题不在其中即可自由订阅），无需改动。

- [ ] **Step 7: Commit**

```bash
git add src/gateway/events/frame.rs
git commit -m "gateway: add TeamChanged event frame (team.changed topic)"
```

---

## Task 2: `TeamStore` 新增 `name_auto` 列 + `rename_team` / `set_name_auto` / `take_auto_name_flag`

**Files:**
- Modify: `src/teams/store.rs`（helpers 49-69；trait 76-100+；`migrate` 末尾 ~182；`SqliteTeamStore` impl 280+；已有测试模块在文件尾部）

> 注：`Team` 结构（`src/teams/types.rs:57`）**不改**，`name_auto` 不进 `read_team_row`/`Team`——它是纯内部标志位，只通过 SQL 读写（镜像 `protocol` 用 post-create setter、不塞进 `NewTeam` 的做法，避免动 `NewTeam` 的 20+ 字面量）。

- [ ] **Step 1: 写失败测试**

在 `src/teams/store.rs` 末尾的 `#[cfg(test)] mod tests` 内追加（若文件无 tests 模块，在文件尾新建 `#[cfg(test)] mod name_auto_tests { use super::*; ... }`；建库方式照搬同文件已有测试的 `SqliteTeamStore` 构造）：

```rust
    #[tokio::test]
    async fn rename_team_updates_name_and_errors_when_absent() {
        let store = test_store(); // 照搬本文件已有测试的建库 helper
        let team = store
            .create_team(NewTeam {
                name: "Old".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        store.rename_team(&team.id, "New Topic").await.unwrap();
        let got = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(got.name, "New Topic");

        let err = store.rename_team("nope", "X").await;
        assert!(err.is_err(), "rename of missing team must error");
    }

    #[tokio::test]
    async fn take_auto_name_flag_is_a_one_shot_gate() {
        let store = test_store();
        let team = store
            .create_team(NewTeam {
                name: "新群聊".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        // Default: flag is off.
        assert!(!store.take_auto_name_flag(&team.id).await.unwrap());

        // Set it, then take it once → true, second take → false.
        store.set_name_auto(&team.id, true).await.unwrap();
        assert!(store.take_auto_name_flag(&team.id).await.unwrap());
        assert!(!store.take_auto_name_flag(&team.id).await.unwrap());
    }
```

> 若本文件已有的测试建库 helper 名称不是 `test_store()`，把上面两处 `test_store()` 换成实际 helper（`rg -n "async fn .*-> .*SqliteTeamStore|fn test_store|SqliteTeamStore::new" src/teams/store.rs` 找）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib rename_team_updates_name_and_errors_when_absent take_auto_name_flag_is_a_one_shot_gate`
Expected: FAIL — `no method named rename_team / set_name_auto / take_auto_name_flag`

- [ ] **Step 3: migrate 加列**

在 `src/teams/store.rs` 的 `migrate`，紧跟 protocol 迁移行（`add_column_if_missing(&conn, "teams", "protocol", "TEXT")?;`，约 182 行）之后插入：

```rust
        // Additive migration: auto-name flag. Teams created from the Panel
        // compose popover with a blank name carry `name_auto = 1`; the first
        // `teams.chat.send` consumes the flag and replaces the provisional
        // name with an LLM-generated topic. Older rows backfill 0 (no-op).
        add_column_if_missing(&conn, "teams", "name_auto", "INTEGER NOT NULL DEFAULT 0")?;
```

- [ ] **Step 4: trait 加三个方法**

在 `src/teams/store.rs` 的 `pub trait TeamStore` 内，紧跟 `set_protocol` 方法声明之后（约 100+ 行，trait 闭合 `}` 之前）插入：

```rust
    /// Rename a team. Errors with `NotFound` when the team does not exist.
    /// Used by both manual rename (`teams.rename`) and first-message auto-name.
    async fn rename_team(&self, id: &str, name: &str) -> crate::error::Result<()>;

    /// Set (or clear) the auto-name flag. Teams created with a blank name set
    /// this to `true` so the first message can replace the provisional name.
    async fn set_name_auto(&self, id: &str, value: bool) -> crate::error::Result<()>;

    /// Atomically check-and-clear the auto-name flag. Returns `true` exactly
    /// once (on the first call when the flag was set), `false` thereafter — so
    /// it doubles as the "first meaningful message" gate, race-safe under
    /// concurrent sends.
    async fn take_auto_name_flag(&self, id: &str) -> crate::error::Result<bool>;
```

- [ ] **Step 5: SqliteTeamStore impl 三个方法**

在 `src/teams/store.rs` 的 `impl TeamStore for SqliteTeamStore` 块内，紧跟 `set_protocol` 的 impl 之后插入（镜像 `disband_team` 的 affected-rows → NotFound 模式）：

```rust
    async fn rename_team(&self, id: &str, name: &str) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute("UPDATE teams SET name = ?1 WHERE id = ?2", params![name, id])
            .map_err(db_err)?;
        if affected == 0 {
            return Err(not_found(format!("team not found: {id}")));
        }
        Ok(())
    }

    async fn set_name_auto(&self, id: &str, value: bool) -> crate::error::Result<()> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute(
                "UPDATE teams SET name_auto = ?1 WHERE id = ?2",
                params![i64::from(value), id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(not_found(format!("team not found: {id}")));
        }
        Ok(())
    }

    async fn take_auto_name_flag(&self, id: &str) -> crate::error::Result<bool> {
        let conn = self.conn.lock().await;
        let affected = conn
            .execute(
                "UPDATE teams SET name_auto = 0 WHERE id = ?1 AND name_auto = 1",
                params![id],
            )
            .map_err(db_err)?;
        Ok(affected > 0)
    }
```

- [ ] **Step 6: 运行测试确认通过**

Run: `cargo test -p alephcore --lib rename_team_updates_name_and_errors_when_absent take_auto_name_flag_is_a_one_shot_gate`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/teams/store.rs
git commit -m "teams: add name_auto column + rename_team/set_name_auto/take_auto_name_flag"
```

---

## Task 3: 共享主题 helper `generate_conversation_topic` + 单聊改用它

**Files:**
- Create: `src/gateway/execution_engine/topic.rs`
- Modify: `src/gateway/execution_engine/mod.rs`（加 `mod topic;`）
- Modify: `src/gateway/execution_engine/execute.rs:534-576`（改调 helper）

- [ ] **Step 1: 写失败测试（helper 的回退分支，host-testable，无需真 LLM）**

新建 `src/gateway/execution_engine/topic.rs`，先只放测试 + 一个会 panic 的桩，确保测试能编译并失败：

```rust
//! Shared conversation-topic generation.
//!
//! Single source of truth for "turn the first user message into a short
//! title". Used by both single chat (`execute.rs` auto-topic) and team
//! group chat (`handlers/teams.rs` first-message auto-name) so the prompt
//! and fallback never drift.

use crate::sync_primitives::Arc;

/// Fallback when the LLM returns nothing usable: truncate the message to 20
/// chars (matching single chat's historical behavior). Pure + host-testable.
fn fallback_topic(message: &str) -> String {
    let msg = message.trim();
    let truncated: String = msg.chars().take(20).collect();
    if msg.chars().count() > 20 {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::fallback_topic;

    #[test]
    fn fallback_truncates_long_messages_with_ellipsis() {
        let long = "a".repeat(30);
        let out = fallback_topic(&long);
        assert_eq!(out.chars().count(), 21); // 20 chars + '…'
        assert!(out.ends_with('…'));
    }

    #[test]
    fn fallback_keeps_short_messages_verbatim() {
        assert_eq!(fallback_topic("  hi there  "), "hi there");
    }
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib gateway::execution_engine::topic`
Expected: FAIL — `module topic not found`（mod 尚未挂载）

- [ ] **Step 3: 挂载模块**

在 `src/gateway/execution_engine/mod.rs` 的模块声明区（与其它 `mod xxx;` 一起）加：

```rust
mod topic;
```

- [ ] **Step 4: 运行测试确认通过（回退分支）**

Run: `cargo test -p alephcore --lib gateway::execution_engine::topic`
Expected: PASS（两个 fallback 测试通过）

- [ ] **Step 5: 加 LLM 调用主函数**

在 `src/gateway/execution_engine/topic.rs` 的 `fallback_topic` 之后、`#[cfg(test)]` 之前插入（逐字搬运 `execute.rs:534-576` 的 prompt / payload / 回退）：

```rust
/// Generate a concise topic title from the first user message, via the given
/// provider. Falls back to a truncated message when the LLM errors or returns
/// empty. Never fails — always returns a non-panicking String.
pub async fn generate_conversation_topic(
    provider: &Arc<dyn crate::providers::AiProvider>,
    message: &str,
) -> String {
    use crate::providers::adapter::RequestPayload;
    use crate::providers::message::UnifiedMessage;

    let prompt = format!(
        "Generate a concise topic title (5-10 characters, same language as the message) \
         for a conversation that starts with: {message}"
    );
    let messages = vec![UnifiedMessage::user(&prompt)];
    let payload = RequestPayload {
        messages: &messages,
        system_prompt: Some("You are a title generator. Output ONLY the title, nothing else."),
        system_blocks: None,
        tools: None,
        think_level: None,
        temperature: Some(0.3),
        max_tokens: None,
        tool_choice: None,
        model: None,
        metadata: None,
    };

    match provider.process(payload).await {
        Ok(resp) => {
            let text = resp.text_content().trim().to_string();
            if text.is_empty() {
                fallback_topic(message)
            } else {
                text
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "topic generation: LLM call failed, using fallback");
            fallback_topic(message)
        }
    }
}
```

- [ ] **Step 6: 单聊改调 helper（消除重复）**

在 `src/gateway/execution_engine/execute.rs`，把 534-576 行那段（从 `let prompt = format!(` 起，到构造出 `let topic_text = topic_text.unwrap_or_else(...)` 结束的整块 prompt/payload/process/fallback）替换为：

```rust
                            let topic_text = super::topic::generate_conversation_topic(
                                &topic_provider,
                                &topic_message,
                            )
                            .await;
```

保留其后的 `sm.set_topic(...)` 与 `eb.publish(...)` 逻辑不动。保留 520-525 行 `topic_provider` / `topic_message` 的解析不动。

> 注意：删除替换块后，`use crate::providers::adapter::RequestPayload;` / `use crate::providers::message::UnifiedMessage;`（530-532 行那两句 `use`）若在该 spawn 块内已无其它用处，一并删除以免 unused-import 警告。

- [ ] **Step 7: 编译确认无回归**

Run: `cargo build -p alephcore --bin aleph-server`
Expected: 编译通过（无 unused import 警告）

- [ ] **Step 8: Commit**

```bash
git add src/gateway/execution_engine/topic.rs src/gateway/execution_engine/mod.rs src/gateway/execution_engine/execute.rs
git commit -m "gateway: extract shared generate_conversation_topic helper; single chat reuses it"
```

---

## Task 4: `teams.rename` RPC handler + 注册

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（imports 14-30；新增 handler + params + test）
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/agents.rs:208`（register_handler! 区）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/teams.rs` 的测试区（文件已有 `handle_create` 相关测试，复用其建库/请求 helper）追加：

```rust
    #[tokio::test]
    async fn handle_rename_updates_name() {
        let store = mk_store().await; // 复用本文件 create 测试用的建库 helper
        let team = store
            .create_team(crate::teams::NewTeam {
                name: "Old".into(),
                description: String::new(),
                leader_id: "main".into(),
            })
            .await
            .unwrap();

        let req = req_with("teams.rename", json!({ "team_id": team.id, "name": "Renamed" }));
        let resp = handle_rename(req, Arc::clone(&store)).await;
        assert!(resp.error.is_none(), "rename should succeed: {resp:?}");

        let got = store.get_team(&team.id).await.unwrap().unwrap();
        assert_eq!(got.name, "Renamed");
    }

    #[tokio::test]
    async fn handle_rename_rejects_blank_name() {
        let store = mk_store().await;
        let req = req_with("teams.rename", json!({ "team_id": "t", "name": "   " }));
        let resp = handle_rename(req, store).await;
        assert!(resp.error.is_some(), "blank name must be rejected");
    }
```

> 把 `mk_store().await` / `req_with(...)` 换成本文件 `handle_create` 测试实际用的 helper 名（`rg -n "fn create_team_req|async fn .*Store|fn req" src/gateway/handlers/teams.rs`）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib handle_rename_updates_name handle_rename_rejects_blank_name`
Expected: FAIL — `cannot find function handle_rename`

- [ ] **Step 3: 加 params + handler**

在 `src/gateway/handlers/teams.rs` 紧邻 `handle_create` 处加（params 放 Request Parameters 区，handler 放 handle_create 附近）：

```rust
#[derive(Debug, Deserialize)]
pub struct RenameTeamParams {
    pub team_id: String,
    pub name: String,
}

/// teams.rename — rename a team. Thin I/O: validates non-blank name, delegates
/// to `TeamStore::rename_team`. Used by the Panel sidebar inline-edit.
pub async fn handle_rename(request: JsonRpcRequest, store: Arc<dyn TeamStore>) -> JsonRpcResponse {
    debug!("Handling teams.rename request");
    let params: RenameTeamParams = match parse_params(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = params.name.trim();
    if params.team_id.trim().is_empty() || name.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "team_id and a non-blank name are required".to_string(),
        );
    }
    match store.rename_team(&params.team_id, name).await {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, RESOURCE_NOT_FOUND, format!("{e}")),
    }
}
```

- [ ] **Step 4: 注册 RPC**

在 `src/bin/aleph-server/commands/start/builder/handlers/agents.rs`，紧跟 `register_handler!(server, "teams.create", teams::handle_create, store);`（208 行）之后插入：

```rust
    register_handler!(server, "teams.rename", teams::handle_rename, store);
```

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p alephcore --lib handle_rename_updates_name handle_rename_rejects_blank_name`
Expected: PASS

- [ ] **Step 6: 编译 bin 确认注册无误**

Run: `cargo build -p alephcore --bin aleph-server`
Expected: 编译通过

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/teams.rs src/bin/aleph-server/commands/start/builder/handlers/agents.rs
git commit -m "teams: add teams.rename RPC handler"
```

---

## Task 5: `teams.create` 接受 `auto_name` → 落 `name_auto` 标志

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（`CreateTeamParams` 182-189；`handle_create` 196+）

- [ ] **Step 1: 写失败测试**

在 `src/gateway/handlers/teams.rs` 测试区追加：

```rust
    #[tokio::test]
    async fn handle_create_with_auto_name_sets_flag() {
        let store = mk_store().await;
        let req = req_with(
            "teams.create",
            json!({ "name": "新群聊", "leader_id": "main", "auto_name": true }),
        );
        let resp = handle_create(req, Arc::clone(&store)).await;
        let team_id = resp.result.unwrap()["team_id"].as_str().unwrap().to_string();

        // Flag was set → first take returns true.
        assert!(store.take_auto_name_flag(&team_id).await.unwrap());
    }

    #[tokio::test]
    async fn handle_create_without_auto_name_leaves_flag_off() {
        let store = mk_store().await;
        let req = req_with(
            "teams.create",
            json!({ "name": "My Team", "leader_id": "main" }),
        );
        let resp = handle_create(req, Arc::clone(&store)).await;
        let team_id = resp.result.unwrap()["team_id"].as_str().unwrap().to_string();
        assert!(!store.take_auto_name_flag(&team_id).await.unwrap());
    }
```

> `resp.result` 的取法以本文件已有 create 测试为准（可能是 `resp.result`/`resp.result.unwrap()`）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p alephcore --lib handle_create_with_auto_name_sets_flag handle_create_without_auto_name_leaves_flag_off`
Expected: FAIL — `unknown field auto_name` / 标志未设置

- [ ] **Step 3: `CreateTeamParams` 加字段**

在 `src/gateway/handlers/teams.rs:182` 的 `CreateTeamParams` 加（在 `members` 之后）：

```rust
    /// When true, the team was created with a blank name from the Panel; the
    /// first `teams.chat.send` will replace the provisional name with an
    /// LLM-generated topic. Defaults false (explicit names are respected).
    #[serde(default)]
    pub auto_name: bool,
```

- [ ] **Step 4: `handle_create` 落标志**

在 `src/gateway/handlers/teams.rs` 的 `handle_create` 里，在 leader 自动入队（`add_member` for leader）成功之后、构造成功响应之前，插入：

```rust
    // Blank-name teams from the Panel carry the auto-name flag so the first
    // message can replace the provisional name with an LLM topic.
    if params.auto_name {
        if let Err(e) = store.set_name_auto(&team.id, true).await {
            tracing::warn!(team_id = %team.id, error = %e, "failed to set name_auto flag");
        }
    }
```

> 确认 `handle_create` 内持有 `team`（`create_team` 的返回）与 `store`；若 leader enrollment 之后 `team` 仍在作用域，直接用 `team.id`。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p alephcore --lib handle_create_with_auto_name_sets_flag handle_create_without_auto_name_leaves_flag_off`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/teams.rs
git commit -m "teams: teams.create accepts auto_name flag for blank-name auto-titling"
```

---

## Task 6: `handle_chat_send` 首条消息 auto-name + 在 agent_init 注入 provider/event_bus

**Files:**
- Modify: `src/gateway/handlers/teams.rs`（imports 14-16 加 `warn`；`handle_chat_send` 签名 3031+ 加两个 Option 参数 + auto-name 逻辑）
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1553-1567`（注册处解析 provider + 传 event_bus）

> 本任务无独立单测（需真实 provider/bus）；正确性靠编译 + 最终 E2E（Task 11）。Auto-name 仅当 `topic_provider` 与 `event_bus` 均为 `Some` 且 `take_auto_name_flag` 命中时触发，否则降级为旧行为（与单聊 `Option<sm>/Option<eb>` 降级一致）。

- [ ] **Step 1: imports 加 `warn`**

在 `src/gateway/handlers/teams.rs:16` 把 `use tracing::debug;` 改为：

```rust
use tracing::{debug, warn};
```

- [ ] **Step 2: `handle_chat_send` 签名加两个 Option 参数**

在 `src/gateway/handlers/teams.rs:3031` 把签名改为（在 `context` 之后追加两参）：

```rust
pub async fn handle_chat_send(
    request: JsonRpcRequest,
    store: Arc<dyn TeamStore>,
    msg_store: Option<Arc<dyn crate::teams::messages::MessageStore>>,
    context: Arc<crate::gateway::context::GatewayContext>,
    topic_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
) -> JsonRpcResponse {
```

- [ ] **Step 3: 在持久化用户消息之后、spawn broadcaster 之前插入 auto-name 块**

在 `handle_chat_send` 里，`msg_store.send_message_with_ttl(...)` 那段**之后**、构造 `GroupChatBroadcaster` 之前，插入：

```rust
    // First-message auto-name: if this team was created with a blank name
    // (auto_name flag set), generate an LLM topic now that we have content.
    // The flag is a one-shot gate (atomic take-and-clear), so this fires
    // exactly once — on the first send — and never overrides explicit names.
    if let (Some(provider), Some(bus)) = (topic_provider.as_ref(), event_bus.as_ref()) {
        if store.take_auto_name_flag(&params.team_id).await.unwrap_or(false) {
            let provider = Arc::clone(provider);
            let bus = Arc::clone(bus);
            let store = Arc::clone(&store);
            let team_id = params.team_id.clone();
            let first_message = params.message.clone();
            tokio::spawn(async move {
                let topic = crate::gateway::execution_engine::topic::generate_conversation_topic(
                    &provider,
                    &first_message,
                )
                .await;
                match store.rename_team(&team_id, &topic).await {
                    Ok(()) => {
                        let _ = bus.publish_frame(
                            &crate::gateway::events::GatewayEventFrame::TeamChanged {
                                team_id: team_id.clone(),
                                change: crate::gateway::events::ChangeKind::Updated,
                            },
                        );
                    }
                    Err(e) => warn!(team_id = %team_id, error = %e, "team auto-name: rename failed"),
                }
            });
        }
    }
```

> 确认 `generate_conversation_topic` 在 Task 3 已 `pub`。`publish_frame` 签名见 `src/gateway/event_bus.rs:386`（`pub fn publish_frame(&self, frame: &GatewayEventFrame) -> Result<usize, serde_json::Error>`）。

- [ ] **Step 4: agent_init 注册处解析 provider + 传 event_bus**

在 `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1553-1567` 的 `teams.chat.send` 注册块改为（新增 `topic_provider` 解析 + 把两参传进 `handle_chat_send`）：

```rust
            if let Some(ts) = team_store.clone() {
                let chat_ctx = gateway_ctx.clone();
                let chat_msg_store = message_store.clone();
                // Resolve a cheap provider (haiku → default) for first-message
                // team auto-naming, mirroring single chat's auto-topic provider.
                let chat_topic_provider: Option<Arc<dyn alephcore::providers::AiProvider>> =
                    topic_provider_registry
                        .get("haiku")
                        .or_else(|| Some(topic_provider_registry.default_provider()));
                let chat_event_bus = event_bus.clone();
                server.handlers_mut().register("teams.chat.send", move |req| {
                    let store = ts.clone();
                    let ctx = chat_ctx.clone();
                    let msg_store = chat_msg_store.clone();
                    let provider = chat_topic_provider.clone();
                    let bus = chat_event_bus.clone();
                    async move {
                        alephcore::gateway::handlers::teams::handle_chat_send(
                            req, store, msg_store, ctx, provider, Some(bus),
                        )
                        .await
                    }
                });
            }
```

> 校验：`topic_provider_registry`（定义于 780 行、1322 行再 clone 过）与 `event_bus` 在 1556 行作用域内可用。若 `event_bus` 此处类型为 `Arc<GatewayEventBus>`（非 Option），`Some(bus)` 正确；若已是 `Option`，改传 `bus`。用 `rg -n "let event_bus" src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` 确认类型。

- [ ] **Step 5: 编译确认通过**

Run: `cargo build -p alephcore --bin aleph-server`
Expected: 编译通过

- [ ] **Step 6: 全量后端单测（确认签名改动未破坏其它 handle_chat_send 调用方/测试）**

Run: `cargo test -p alephcore --lib teams::`
Expected: PASS（若有旧测试直接调 `handle_chat_send`，给它补两个 `None` 参数）

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/teams.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "teams: auto-name group chat from first message via shared topic helper"
```

---

## Task 7: 前端 `TeamsApi` — `create` 透传 `auto_name` + 新增 `rename`

**Files:**
- Modify: `interfaces/webchat/src/api/teams.rs`（`create` 113-139；新增 `rename`）

- [ ] **Step 1: `create` 增加 `auto_name` 形参 + 透传**

在 `interfaces/webchat/src/api/teams.rs` 把 `create`（113-139）签名与 body 改为：

```rust
    pub async fn create(
        state: &DashboardState,
        name: &str,
        description: &str,
        leader_id: &str,
        members: &[(String, String)], // (agent_id, role)
        auto_name: bool,
    ) -> Result<String, String> {
        let members_json: Vec<Value> = members
            .iter()
            .map(|(id, role)| json!({ "agent_id": id, "role": role }))
            .collect();
        let result = state
            .rpc_call(
                "teams.create",
                json!({
                    "name": name,
                    "description": description,
                    "leader_id": leader_id,
                    "members": members_json,
                    "auto_name": auto_name,
                }),
            )
            .await?;
        result
            .get("team_id")
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| "teams.create did not return team_id".to_string())
    }
```

- [ ] **Step 2: 新增 `rename`**

在 `interfaces/webchat/src/api/teams.rs` 的 `disband`（75-80）附近加：

```rust
    pub async fn rename(state: &DashboardState, team_id: &str, name: &str) -> Result<(), String> {
        state
            .rpc_call("teams.rename", json!({ "team_id": team_id, "name": name }))
            .await?;
        Ok(())
    }
```

- [ ] **Step 3: 编译（会因 `create` 调用方未传新参而失败 — Task 8 修复）**

Run: `just wasm`
Expected: FAIL — `team_compose.rs` 调 `TeamsApi::create` 缺 `auto_name` 实参（下个 task 修）。**本 task 暂不单独 commit**，与 Task 8 一起编译通过后提交。

---

## Task 8: 前端 compose 弹窗 — 留空发 "新群聊" + `auto_name=true`，移除 `{leader}的群聊`

**Files:**
- Modify: `interfaces/webchat/src/views/chat/team_compose.rs:93-109`
- Modify: `interfaces/webchat/locales/zh.json` + `en.json`（加 `chat.new_group_chat`）

- [ ] **Step 1: i18n 加 `new_group_chat`**

在 `interfaces/webchat/locales/zh.json` 的 `chat` 块（`new_chat` 在 182 行附近）加一行：

```json
    "new_group_chat": "新群聊",
```

在 `interfaces/webchat/locales/en.json` 对应处加：

```json
    "new_group_chat": "New group chat",
```

- [ ] **Step 2: compose 留空分支改用 "新群聊" + auto_name**

在 `interfaces/webchat/src/views/chat/team_compose.rs` 把 `start` 闭包里的命名解析（93-104）+ create 调用（109）改为：

```rust
        // Validate; resolve whether the name is explicit or auto-generated.
        // Blank → provisional "新群聊" + auto_name=true so the backend replaces
        // it with an LLM topic on the first message (mirrors single chat).
        let (name, auto_name) =
            match resolve_team_compose(&leader, &team_name.get_untracked(), members.len()) {
                Ok(Some(n)) => (n, false),
                Ok(None) => (t_string!(i18n, chat.new_group_chat).to_string(), true),
                Err(e) => {
                    let msg = match e {
                        TeamComposeError::EmptyLeader => t_string!(i18n, chat.team_err_no_leader),
                        TeamComposeError::NoMembers => t_string!(i18n, chat.team_err_no_member),
                    };
                    error.set(Some(msg.to_string()));
                    return;
                }
            };
        error.set(None);
```

并把下方 create 调用（约 109 行）改为传 `auto_name`：

```rust
            match TeamsApi::create(&dash, &name, "", &leader, &members, auto_name).await {
```

> `team_default_suffix` i18n key 自此不再被引用（orphan）。**保留**该 key（删除非必须，避免动其它语言文件的潜在引用）；若想清理，可单独再起一轮。`resolve_team_compose` 与其 4 个单测无需改动（语义不变：`Ok(None)` 仍表示「留空，调用方决定默认」）。

- [ ] **Step 2.5（可选健壮性）：确认 compose 弹窗的名称输入框仍在**

`team_compose.rs` 的 `<input ... placeholder=team_name_placeholder>`（169-173 行）**保留**——用户决策是「保留输入框」。无需改动该 input。

- [ ] **Step 3: 编译通过（与 Task 7 一起）**

Run: `just wasm`
Expected: PASS（`create` 新签名 + 唯一调用方已更新）

- [ ] **Step 4: 跑 compose 既有单测确认无回归**

Run: `cargo test -p aleph-panel --lib resolve_team_compose 2>/dev/null || cargo test -p aleph-panel team_compose`
Expected: PASS（4 个既有测试）
> 若 `aleph-panel` 的 host 测试需特定 target，按本仓既有方式跑（`rg -n "cargo test" justfile`）。

- [ ] **Step 5: Commit（Task 7 + 8 合并提交）**

```bash
git add interfaces/webchat/src/api/teams.rs interfaces/webchat/src/views/chat/team_compose.rs interfaces/webchat/locales/zh.json interfaces/webchat/locales/en.json
git commit -m "panel: team compose sends '新群聊' + auto_name when blank; add TeamsApi::rename"
```

---

## Task 9: 前端侧栏群聊行 — 只显示 active + 三态（菜单/重命名/删除）

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`（信号 220-240；新增 do_rename_group/do_delete_group 仿 581-644；群聊行渲染 830-880；过滤 806）

> 这是本计划唯一大块新 UI。它**镜像**单聊会话行三态（963-1148）：normal（头像簇 + 名 + last_msg + 悬停 `⋯` 菜单）/ edit（inline input）/ delete-confirm（红条）。用**独立信号**，单聊行逻辑零改动。Leptos 注意：每个分支/闭包前 `.clone()` 各自的 `group_id`；菜单按钮 `ev.stop_propagation()`。

- [ ] **Step 1: 加群聊行独立信号**

在 `interfaces/webchat/src/components/chat_sidebar.rs:224`（`let is_saving = ...` 之后）插入：

```rust
    // Group-chat row action state — SEPARATE from session-row signals so the
    // single-chat state machine stays untouched. Keyed by team id.
    let group_editing_id = RwSignal::new(Option::<String>::None);
    let group_deleting_id = RwSignal::new(Option::<String>::None);
    let group_edit_text = RwSignal::new(String::new());
    let group_menu_id = RwSignal::new(Option::<String>::None);
```

- [ ] **Step 2: 加 do_rename_group / do_delete_group 闭包**

在 `interfaces/webchat/src/components/chat_sidebar.rs` 的 `do_delete` 闭包定义（614-644）**之后**插入（镜像 do_rename/do_delete，但调 `TeamsApi::rename` / `TeamsApi::disband`）：

```rust
    let reload_for_grename = reload_data.clone();
    let do_rename_group = Arc::new(move |team_id: String, name: String| {
        if is_saving.get_untracked() {
            return;
        }
        let name = name.trim().to_string();
        if name.is_empty() {
            group_editing_id.set(None);
            group_edit_text.set(String::new());
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_grename.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = TeamsApi::rename(&dash, &team_id, &name).await {
                web_sys::console::error_1(&format!("Failed to rename team: {e}").into());
            } else {
                reload(dash);
            }
            is_saving.set(false);
            group_editing_id.set(None);
            group_edit_text.set(String::new());
        });
    });

    let reload_for_gdelete = reload_data.clone();
    let do_delete_group = Arc::new(move |team_id: String| {
        if is_saving.get_untracked() {
            return;
        }
        is_saving.set(true);
        let dash = dashboard;
        let reload = reload_for_gdelete.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = TeamsApi::disband(&dash, &team_id).await {
                web_sys::console::error_1(&format!("Failed to delete team: {e}").into());
            } else {
                // If the disbanded team is the open one, leave team-chat mode.
                if chat.team_id.get_untracked().as_deref() == Some(&team_id) {
                    chat.clear_session();
                }
                reload(dash);
            }
            is_saving.set(false);
            group_deleting_id.set(None);
        });
    });
```

> 注：`reload_data` 在 614 行被 `let reload_for_delete = reload_data;`（move）消费。改成 `let reload_for_delete = reload_data.clone();`，使 `reload_data` 仍可被上面两个新闭包 `.clone()`。（即把 614 行的 `reload_data` 改为 `reload_data.clone()`。）

- [ ] **Step 3: 群聊行渲染三态化 + 过滤 active**

在 `interfaces/webchat/src/components/chat_sidebar.rs:806`，把 `let group_list = groups.get();` 改为只取 active：

```rust
                    let group_list: Vec<_> = groups.get().into_iter().filter(|g| g.status == "active").collect();
```

并在群聊行的 `<Show when=...>` 容器内（828-882），把每行的 `.map(|group| {...})`（830-880）整体替换为三态渲染。先在闭包顶部读取行动作信号以建立反应性，再分支：

```rust
                                {
                                    let _g_editing = group_editing_id.get();
                                    let _g_deleting = group_deleting_id.get();
                                    let _g_menu = group_menu_id.get();
                                    let do_rename_group = do_rename_group.clone();
                                    let do_delete_group = do_delete_group.clone();
                                    group_list.clone().into_iter().map(move |group| {
                                        let group_id = group.id.clone();
                                        let group_name = group.name.clone();
                                        let last_msg = group.last_message.clone();
                                        let previews = group.members_preview.clone();
                                        let is_g_editing = _g_editing.as_deref() == Some(&group_id);
                                        let is_g_deleting = _g_deleting.as_deref() == Some(&group_id);
                                        let is_g_menu = _g_menu.as_deref() == Some(&group_id);
                                        let do_rename_group = do_rename_group.clone();
                                        let do_delete_group = do_delete_group.clone();

                                        if is_g_editing {
                                            // --- Edit mode (mirrors session edit 963-1014) ---
                                            let id_save = group_id.clone();
                                            let id_blur = group_id.clone();
                                            let r_key = do_rename_group.clone();
                                            let r_blur = do_rename_group;
                                            view! {
                                                <div class="w-full px-3 py-2 rounded-lg bg-surface-sunken border border-primary/40">
                                                    <input
                                                        node_ref=edit_input_ref
                                                        class="w-full bg-transparent text-xs text-text-primary outline-none disabled:opacity-50"
                                                        prop:value=move || group_edit_text.get()
                                                        prop:disabled=move || is_saving.get()
                                                        maxlength=100
                                                        on:input=move |ev| group_edit_text.set(event_target_value(&ev))
                                                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                            match ev.key().as_str() {
                                                                "Enter" => {
                                                                    let t = group_edit_text.get_untracked();
                                                                    if t.trim().is_empty() {
                                                                        group_editing_id.set(None);
                                                                        group_edit_text.set(String::new());
                                                                    } else {
                                                                        r_key(id_save.clone(), t);
                                                                    }
                                                                }
                                                                "Escape" => {
                                                                    group_editing_id.set(None);
                                                                    group_edit_text.set(String::new());
                                                                }
                                                                _ => {}
                                                            }
                                                        }
                                                        on:blur=move |_| {
                                                            let id = id_blur.clone();
                                                            let r = r_blur.clone();
                                                            leptos::task::spawn_local(async move {
                                                                gloo_timers::future::TimeoutFuture::new(100).await;
                                                                if group_editing_id.get_untracked().as_deref() == Some(&id) {
                                                                    let t = group_edit_text.get_untracked();
                                                                    if t.trim().is_empty() {
                                                                        group_editing_id.set(None);
                                                                        group_edit_text.set(String::new());
                                                                    } else {
                                                                        r(id, t);
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    />
                                                </div>
                                            }.into_any()
                                        } else if is_g_deleting {
                                            // --- Delete-confirm (mirrors session 1015-1054) ---
                                            let id_del = group_id.clone();
                                            view! {
                                                <div class="w-full px-3 py-2 rounded-lg bg-red-500/10 border border-red-500/30 flex items-center justify-between text-xs">
                                                    <span class="text-red-400 font-medium">{move || t_string!(i18n, chat.confirm_delete).to_string()}</span>
                                                    <div class="flex items-center gap-1.5">
                                                        <button
                                                            class="px-2 py-0.5 rounded bg-red-500 text-white text-[10px] font-medium hover:bg-red-600 transition-colors disabled:opacity-50"
                                                            prop:disabled=move || is_saving.get()
                                                            on:click=move |ev: web_sys::MouseEvent| { ev.stop_propagation(); do_delete_group(id_del.clone()); }
                                                        >
                                                            {move || t_string!(i18n, common.confirm).to_string()}
                                                        </button>
                                                        <button
                                                            class="px-2 py-0.5 rounded bg-surface-sunken text-text-secondary text-[10px] hover:bg-surface-raised transition-colors"
                                                            on:click=move |ev: web_sys::MouseEvent| { ev.stop_propagation(); group_deleting_id.set(None); }
                                                        >
                                                            {move || t_string!(i18n, common.cancel).to_string()}
                                                        </button>
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            // --- Normal mode: existing avatar-cluster row + ⋯ menu ---
                                            let id_click = group_id.clone();
                                            let id_menu = group_id.clone();
                                            let id_edit = group_id.clone();
                                            let id_del_menu = group_id.clone();
                                            let name_for_edit = group_name.clone();
                                            view! {
                                                <div class="relative group">
                                                    <button
                                                        class="w-full text-left px-2 py-2 rounded-lg text-sm nav-tile flex items-center gap-2"
                                                        on:click=move |_| on_open_group(id_click.clone())
                                                    >
                                                        <div class="flex items-center flex-shrink-0">
                                                            {previews.iter().take(3).enumerate().map(|(i, mp)| {
                                                                let color = agent_color_for_id(&mp.id);
                                                                let glyph = mp.emoji.clone()
                                                                    .filter(|e| !e.is_empty())
                                                                    .or_else(|| mp.name.as_ref().and_then(|n| n.chars().next()).map(|c| c.to_uppercase().to_string()))
                                                                    .or_else(|| mp.id.chars().next().map(|c| c.to_uppercase().to_string()))
                                                                    .unwrap_or_else(|| "?".to_string());
                                                                let margin = if i == 0 { "" } else { "-ml-2" };
                                                                view! {
                                                                    <span
                                                                        class=format!("{margin} w-6 h-6 rounded-full flex items-center justify-center text-[10px] font-bold text-white ring-2 ring-surface-sunken")
                                                                        style=format!("background-color: {color};")
                                                                    >{glyph}</span>
                                                                }
                                                            }).collect::<Vec<_>>()}
                                                        </div>
                                                        <div class="flex-1 min-w-0">
                                                            <div class="truncate text-xs font-medium text-text-primary">{group_name.clone()}</div>
                                                            {last_msg.clone().map(|m| view! { <div class="truncate text-[10px] text-text-tertiary mt-0.5">{m}</div> })}
                                                        </div>
                                                        <button
                                                            class="opacity-0 group-hover:opacity-100 ml-1 px-1.5 py-0.5 rounded text-text-tertiary hover:text-text-primary hover:bg-surface-raised transition-all text-xs flex-shrink-0"
                                                            on:click=move |ev: web_sys::MouseEvent| {
                                                                ev.stop_propagation();
                                                                let cur = group_menu_id.get_untracked();
                                                                if cur.as_deref() == Some(&id_menu) {
                                                                    group_menu_id.set(None);
                                                                } else {
                                                                    group_menu_id.set(Some(id_menu.clone()));
                                                                }
                                                            }
                                                        >"⋯"</button>
                                                    </button>
                                                    {if is_g_menu {
                                                        let name_e = name_for_edit.clone();
                                                        view! {
                                                            <div class="glass absolute right-0 top-full mt-1 z-50 min-w-[120px] bg-surface-overlay/85 border border-border rounded-lg shadow-xl py-1 text-xs">
                                                                <button
                                                                    class="w-full text-left px-3 py-1.5 text-text-secondary hover:bg-surface-sunken hover:text-text-primary transition-colors"
                                                                    on:click=move |ev: web_sys::MouseEvent| {
                                                                        ev.stop_propagation();
                                                                        group_menu_id.set(None);
                                                                        group_edit_text.set(name_e.clone());
                                                                        group_editing_id.set(Some(id_edit.clone()));
                                                                    }
                                                                >{move || t_string!(i18n, chat.rename).to_string()}</button>
                                                                <button
                                                                    class="w-full text-left px-3 py-1.5 text-red-400 hover:bg-red-500/10 transition-colors"
                                                                    on:click=move |ev: web_sys::MouseEvent| {
                                                                        ev.stop_propagation();
                                                                        group_menu_id.set(None);
                                                                        group_deleting_id.set(Some(id_del_menu.clone()));
                                                                    }
                                                                >{move || t_string!(i18n, common.delete).to_string()}</button>
                                                            </div>
                                                        }.into_any()
                                                    } else { view! { <span /> }.into_any() }}
                                                </div>
                                            }.into_any()
                                        }
                                    }).collect::<Vec<_>>()
                                }
```

> `edit_input_ref` 是单聊行复用的 node_ref（972 行同名），群聊 edit 共用它即可（同一时刻只有一行处于 edit）。`agent_color_for_id` / `on_open_group` 均为本文件已有符号（原群聊行就在用）。

- [ ] **Step 4: 编译**

Run: `just wasm`
Expected: PASS（如遇 `Rc<dyn Fn>` 的 `Send+Sync`/`Copy` 报错，按本仓既往做法：`Arc` 闭包在每个使用点 `.clone()`；不要把闭包放进 `<Show>` children——本实现用 `if/else` 直接产 view，已规避）

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: group-chat rows get rename/delete (three-mode) + active-only filter"
```

---

## Task 10: 前端订阅 `team.changed` → 刷新群聊列表

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs`（事件订阅 313-350；topic 订阅 402-432）

- [ ] **Step 1: 订阅 topic**

在 `interfaces/webchat/src/components/chat_sidebar.rs` 的 `subscribe_topic("stream.session_updated")` 那段（约 414 行）附近，追加订阅（镜像 cron/tasks 视图 `subscribe_topic` 用法）：

```rust
            if let Err(e) = dash_for_topic.subscribe_topic("team.changed").await {
                web_sys::console::error_1(&format!("Failed to subscribe to team.changed: {e}").into());
            }
```

> 若该处用循环遍历一个 topics 数组订阅（看 428 行 `for topic in ...`），直接把 `"team.changed"` 加进该数组即可，免重复样板。

- [ ] **Step 2: 事件回调里处理 team.changed → reload**

在 `interfaces/webchat/src/components/chat_sidebar.rs` 的 `subscribe_events` 回调（321-350，处理 `run.session_updated` 的那个）开头，`reload_for_event` 已在闭包内可用。把开头的 early-return 守卫从「只认 run.session_updated」放宽为也认 team.changed：

把：

```rust
        if event.topic != "run.session_updated" {
            return;
        }
        reload_for_event(sub_dash);
```

改为：

```rust
        if event.topic == "team.changed" {
            reload_for_event(sub_dash);
            return;
        }
        if event.topic != "run.session_updated" {
            return;
        }
        reload_for_event(sub_dash);
```

> `reload_for_event` 即 `reload_data` 的 clone（319 行），其 body 会重新拉 `agents.teams` → `groups`（288-298 行），所以群聊名会即时刷新。

- [ ] **Step 3: 编译**

Run: `just wasm`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: sidebar subscribes to team.changed for live group-chat name refresh"
```

---

## Task 11: 部署 + 端到端验证

**Files:** 无（部署 + 手动验证）

- [ ] **Step 1: 全量编译两 crate**

Run: `cargo build -p alephcore --bin aleph-server` 然后 `just wasm`
Expected: 两者均通过

- [ ] **Step 2: 重编 binary + 替换运行中 daemon**

按 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」：
```bash
cargo build --release -p alephcore --bin aleph-server
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
```
（或 .app daemon：`mv` 旧 binary → `cp target/release/aleph-server` 进 .app → `kill <pid>` 让 supervisor relaunch。）

- [ ] **Step 3: E2E — 留空建群自动命名**

1. Panel 打开 team compose，选 leader + ≥1 成员，**名称留空**，Start。
2. 侧栏群聊区出现「新群聊」。
3. 在群聊发首条有内容的消息（如「帮我调研下 Rust 异步运行时」）。
4. **预期**：数秒内侧栏该群名自动变为 LLM 主题（非「新群聊」、非「main的群聊」），无需手动刷新（`team.changed` 驱动）。

- [ ] **Step 4: E2E — 显式命名不被覆盖**

1. 再建一个群，名称填「发布小组」，Start，发消息。
2. **预期**：名称保持「发布小组」，不被 LLM 覆盖。

- [ ] **Step 5: E2E — 重命名**

1. 悬停某群聊行 → `⋯` → 重命名 → 输入新名 → Enter。
2. **预期**：名称即时更新并持久化（刷新 Panel 后仍是新名）。

- [ ] **Step 6: E2E — 删除（软删 + 隐藏）**

1. 悬停某群聊行 → `⋯` → 删除 → 确认。
2. **预期**：该群从侧栏群聊区消失；若它是当前打开的群，聊天区退出团队模式。团队管理页仍可见其 disbanded 记录。

- [ ] **Step 7: 最终 Commit（如有手动微调）**

```bash
git add -A
git commit -m "teams: finalize group-chat LLM naming + rename/delete parity"
```

---

## Self-Review (plan author)

**Spec coverage:**
- §A 自动命名 → Task 1（事件帧）+ Task 2（标志位/rename）+ Task 3（共享 helper）+ Task 5（create auto_name）+ Task 6（首条消息触发）+ Task 8（前端留空→新群聊+auto_name）。✓
- §B 重命名 → Task 2（rename_team）+ Task 4（teams.rename RPC）+ Task 7（TeamsApi::rename）+ Task 9（侧栏 inline-edit）。✓
- §C 删除 → Task 9（do_delete_group=disband + status=="active" 过滤）。✓
- §D 侧栏 UX → Task 9（三态/独立信号）+ Task 10（team.changed 刷新）+ Task 8（"新群聊" 占位）。✓
- §E 测试 → Task 1/2/3/4/5 各含 host 单测；Task 11 手动 E2E。✓

**Placeholder scan:** 无 TBD/TODO。少量「以本文件已有 helper 名为准」是因测试 helper 名因文件而异，已给出 `rg` 定位命令 + 确切替换点，非开放需求。

**Type consistency:** `name_auto`(SQL INTEGER) / `take_auto_name_flag`→`bool` / `rename_team(id,name)` / `generate_conversation_topic(&Arc<dyn AiProvider>, &str)->String` / `GatewayEventFrame::TeamChanged{team_id,change}` / topic `"team.changed"` / `TeamsApi::create(...,auto_name)` / `TeamsApi::rename(team_id,name)` — 各 task 间签名一致。`do_rename_group`/`do_delete_group`/`group_editing_id` 等命名在 Task 9 内自洽。

**已知风险（已在步骤内置校验）:**
- Task 1 Step 6：`team.changed` 若被 `event_scope` allowlist 拦截 → 同处加入。
- Task 6 Step 4：`event_bus` 实际类型（`Arc` vs `Option<Arc>`）→ `rg` 确认后决定 `Some(bus)`/`bus`。
- Task 9 Step 2：`reload_data` 的 move→clone 改动（614 行）必须做，否则后续闭包无法再 clone。
