# chat/config 权限分层 Phase 2 — 对话路径门控 + 角色随 run 注入 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 远程 chat 档设备通过对话让 LLM 调用「修改 Aleph 自身配置」的工具时被硬拒，operator / 本机无鉴权 daemon / 非网关 run（cron 等）不受影响。

**Architecture:** 把发起 run 的网关连接的授权角色（operator / guest / none）以**数据**形式从网关分发循环带进被 spawn 的 agent run：`CALLER_ROLE` task-local（仅在已鉴权分发路径 scope）→ `AgentRunManager::start_run` 读取并写入 `RunRequest.metadata` → `run_loop` 据此构造 `TurnContext.caller_role` → 工具分发唯一咽喉点 `ScopedToolService::execute_inner` 据此对「config 类工具」门控。命中即返回 `ToolError::PermissionDenied`，LLM 如实告知用户需 operator 授权。**live sudo 现场审批属 Phase 2b，本期纯硬拒**（与 B1 的 RPC 路径一致）。

**Tech Stack:** Rust，tokio `task_local!`，既有 `method_authz` 分类器 + `TurnContext` + `ScopedToolService` 既有接缝。

**关键不变量（零回归）：** 本机无鉴权 daemon（`auth_mode` 不要求鉴权）下，网关对**每条** run 注入的 `caller_role` 恒为 `None` → 门控放行。auth 模式下才把连接真实 role 注入。门控本身只做纯角色判断（R10 笨门控），auth-mode 感知留在网关边界。

---

## 背景：复用既有基础设施（已勘察）

- RPC 路径门控已由 **B1** 完成：`src/gateway/server/handler.rs:840-865` 在 `is_auth_required()` 且 `required_privilege(method)==Operator` 且连接非 operator 时回 `PERMISSION_DENIED`。本期**不动** RPC 路径。
- `src/gateway/method_authz.rs`：已有 `MethodPrivilege::{Authenticated, Operator}` 与 `required_privilege(method: &str)`（OPERATOR_NAMESPACES + OPERATOR_METHODS）。本期在**同文件**新增工具名分类（同源，避免漂移）。
- `src/tools/turn_context.rs`：`TurnContext{session_key, channel_id, conversation_id}`，由 `ScopedToolService::execute`（唯一生产分发咽喉点）scope，工具执行内可靠可见。
- `src/tools/scoped/dispatch.rs`：`execute_inner`（`offset 100` 起）—— `is_allowed` 过滤（106-110）→ confirm 门控（119）→ hooks → 路由执行。门控插在 `is_allowed` 之后、confirm 之前。
- `src/tools/service.rs:18`：`ToolError::PermissionDenied { name, reason }` —— 语义正合「permission_required」。
- run 启动链：`handle_send`/`agent.run handler` → `AgentRunManager::start_run`（`src/gateway/handlers/agent.rs:134`，`metadata` 在 201-206 组装）→ `tokio::spawn`（247）→ `run_loop` 构造 `TurnContext`（`src/gateway/execution_engine/run_loop.rs:479-491`）。`start_run` 与 `process_request` 同 task（spawn 在其内部、靠后），故 task-local 可在 `start_run` 内读到并落为数据。
- `process_request` 在已鉴权分发路径的两处调用：`handler.rs:924`（`do_lane_dispatch` 闭包内）与 `handler.rs:971`（幂等 Proceed 路径）。`handler.rs:606` 是 connect/bootstrap 早期路径，**不启动 run**，不 scope。

## config 类工具 SSOT（勘察后确定，name 级门控）

下列工具修改 **Aleph 自身配置/自管理面**，对应 RPC OPERATOR 表的领域（config / pairing / plugins / skills / mcp / cron / heartbeat / agents / secret / clawhub）：

```
self_config self_manage vault_store cron_manage
heartbeat_create heartbeat_update heartbeat_delete heartbeat_toggle
skill_install skill_manage agent_create agent_delete
channel_pairing clawhub
```

**刻意排除**（读类 / 非 Aleph 自配置 / 已知 bash 逃逸权衡）：`config_audit`(只读审计)、`gateway_route`(只读路由查询)、`heartbeat_list`/`heartbeat_report`、`skill_status`/`skill_list`/`skill_read`、`agent_list`/`agent_info`、`permission`(macOS TCC)、`automation`/`system`/`bash`/`code_exec`(能力类，bash 逃逸是 spec §5 已接受残留风险)。

**已知权衡（spec 记录）：** name 级门控会一并挡住 `self_config`/`vault_store` 等工具里的**读动作**（如 ReadConfig/list）。chat 档仍可经 Panel 只读 RPC（`config.get` 等，B1 未门控）查看 dashboard 数据。动作级细粒度（区分同一工具内读/写）= YAGNI，留待真有需求。

---

## File Structure

- **Create** `src/gateway/caller_identity.rs` — `CALLER_ROLE` task-local + `current_caller_role()`。单一职责：把网关连接角色跨「分发→start_run」同 task 传递。
- **Modify** `src/gateway/mod.rs` — `pub mod caller_identity;`
- **Modify** `src/gateway/method_authz.rs` — 新增 `OPERATOR_TOOLS` + `pub fn tool_requires_operator(tool: &str) -> bool` + 测试。
- **Modify** `src/tools/turn_context.rs` — `TurnContext` 加 `pub caller_role: Option<String>` + `pub fn caller_is_operator(&self) -> bool` + 测试。
- **Modify** `src/gateway/execution_engine/run_loop.rs:479` — 从 `metadata` 构造 `caller_role`。
- **Modify** `src/gateway/handlers/agent.rs` — `start_run` metadata 组装处读 `current_caller_role()` 落库。
- **Modify** `src/gateway/server/handler.rs` — 计算 `caller_role`（auth-mode 感知）+ 在 924/971 两处 scope `CALLER_ROLE`。
- **Modify** `src/tools/scoped/dispatch.rs` — `execute_inner` 插入 tier 门控 + 测试。
- **Modify** 其余 `TurnContext { .. }` 字面量构造点（补 `caller_role: None`）：`src/tools/scoped/tests.rs:1155,1315`、`src/builtin_tools/select_model.rs:121`、`src/builtin_tools/ask_user.rs:236,270`、`src/builtin_tools/desktop/tests.rs:421,439,465`、`src/approval/adapters.rs:135,201`。

---

## Task 1: SSOT 工具分类器（method_authz）

**Files:**
- Modify: `src/gateway/method_authz.rs`（在 `OPERATOR_METHODS` / `required_privilege` 之后追加）

- [ ] **Step 1: 写失败测试**

在 `method_authz.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
#[test]
fn config_tools_require_operator() {
    for t in [
        "self_config", "self_manage", "vault_store", "cron_manage",
        "heartbeat_create", "heartbeat_update", "heartbeat_delete", "heartbeat_toggle",
        "skill_install", "skill_manage", "agent_create", "agent_delete",
        "channel_pairing", "clawhub",
    ] {
        assert!(tool_requires_operator(t), "{t} must require operator");
    }
}

#[test]
fn chat_safe_tools_stay_open() {
    for t in [
        "search", "web_fetch", "file_read", "config_audit", "gateway_route",
        "heartbeat_list", "skill_list", "agent_list", "memory_search", "ask_user",
        "bash", "code_exec",
    ] {
        assert!(!tool_requires_operator(t), "{t} must stay open to chat tier");
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib method_authz::tests::config_tools_require_operator`
Expected: FAIL —— `tool_requires_operator` 未定义（编译错误）。

- [ ] **Step 3: 实现**

在 `OPERATOR_METHODS` 常量之后、`required_privilege` 之前插入：

```rust
/// Self-management tool names that mutate Aleph's OWN configuration. Sibling to
/// [`OPERATOR_METHODS`] (the RPC table) — kept in this one file so the two stay
/// in sync (the spec's "同源" requirement). A chat-tier connection is rejected
/// from these at the tool-dispatch gate (`ScopedToolService::execute_inner`),
/// mirroring how `required_privilege` rejects config RPCs.
///
/// Read-only self-management tools (`config_audit`, `gateway_route`,
/// `*_list`/`*_status`/`*_read`) are deliberately absent — chat tier keeps them.
const OPERATOR_TOOLS: &[&str] = &[
    "self_config",
    "self_manage",
    "vault_store",
    "cron_manage",
    "heartbeat_create",
    "heartbeat_update",
    "heartbeat_delete",
    "heartbeat_toggle",
    "skill_install",
    "skill_manage",
    "agent_create",
    "agent_delete",
    "channel_pairing",
    "clawhub",
];

/// True when `tool` mutates Aleph's own configuration and therefore requires an
/// operator (config-tier) connection. Names not listed stay open to chat tier.
pub fn tool_requires_operator(tool: &str) -> bool {
    OPERATOR_TOOLS.contains(&tool)
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib method_authz::tests`
Expected: PASS（含既有 RPC 测试 + 2 个新测试）。

- [ ] **Step 5: 提交**

```bash
git add src/gateway/method_authz.rs
git commit -m "gateway: add config-tool operator classifier (SSOT for tier gate)"
```

---

## Task 2: TurnContext.caller_role + caller_is_operator()

**Files:**
- Modify: `src/tools/turn_context.rs`
- Modify: `src/gateway/execution_engine/run_loop.rs:479-491`（唯一真实构造点）
- Modify（补字段）: `src/tools/scoped/tests.rs:1155,1315`、`src/builtin_tools/select_model.rs:121`、`src/builtin_tools/ask_user.rs:236,270`、`src/builtin_tools/desktop/tests.rs:421,439,465`、`src/approval/adapters.rs:135,201`

- [ ] **Step 1: 写失败测试**

在 `src/tools/turn_context.rs` 文件末尾追加：

```rust
#[cfg(test)]
mod caller_tier_tests {
    use super::*;
    use crate::routing::session_key::SessionKey;

    fn ctx(role: Option<&str>) -> TurnContext {
        TurnContext {
            session_key: SessionKey::main("t"),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: role.map(String::from),
        }
    }

    #[test]
    fn operator_and_local_are_operator() {
        assert!(ctx(Some("operator")).caller_is_operator());
        assert!(ctx(None).caller_is_operator(), "no role = trusted local/internal run");
    }

    #[test]
    fn chat_tier_is_not_operator() {
        assert!(!ctx(Some("guest")).caller_is_operator());
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib turn_context::caller_tier_tests`
Expected: FAIL —— `TurnContext` 无 `caller_role` 字段 / 无 `caller_is_operator`（编译错误）。

- [ ] **Step 3: 实现 —— 加字段 + 方法**

`src/tools/turn_context.rs`：在 struct 末尾字段后加：

```rust
    /// Originating gateway connection's authorization role (`"operator"` /
    /// `"guest"`), stamped at run start from `CALLER_ROLE`. `None` for
    /// non-gateway runs (cron, internal) and for the local no-auth daemon —
    /// both treated as trusted by the config-tier gate.
    pub caller_role: Option<String>,
```

在 `impl TurnContext` 内加：

```rust
    /// True when the originating connection may mutate Aleph's own config.
    /// Absent role = trusted local/internal run; `"operator"` = config tier;
    /// any other value (e.g. `"guest"`) = chat tier (gated).
    pub fn caller_is_operator(&self) -> bool {
        match self.caller_role.as_deref() {
            None | Some("operator") => true,
            Some(_) => false,
        }
    }
```

- [ ] **Step 4: 真实构造点从 metadata 取值**

`src/gateway/execution_engine/run_loop.rs:479-491`，给 `TurnContext { .. }` 字面量加最后一个字段：

```rust
            caller_role: request.metadata.get("caller_role").cloned(),
```

- [ ] **Step 5: 补齐其余字面量构造点**

对下列每处 `TurnContext { .. }` 字面量加 `caller_role: None,`：
`src/tools/scoped/tests.rs:1155` 与 `:1315`、`src/builtin_tools/select_model.rs:121`、`src/builtin_tools/ask_user.rs:236` 与 `:270`、`src/builtin_tools/desktop/tests.rs:421`/`:439`/`:465`、`src/approval/adapters.rs:135` 与 `:201`。

> 用 `git grep -n "TurnContext {" src` 复核无遗漏（应只剩 struct 定义本身与已改的构造点）。

- [ ] **Step 6: 跑测试 + 全量编译**

Run: `cargo test -p alephcore --lib turn_context::caller_tier_tests`
Expected: PASS
Run: `cargo check -p alephcore --all-targets`
Expected: 编译通过（确认 10 个构造点全部补齐）。

- [ ] **Step 7: 提交**

```bash
git add src/tools/turn_context.rs src/gateway/execution_engine/run_loop.rs \
        src/tools/scoped/tests.rs src/builtin_tools/select_model.rs \
        src/builtin_tools/ask_user.rs src/builtin_tools/desktop/tests.rs \
        src/approval/adapters.rs
git commit -m "tools: thread caller_role through TurnContext for config-tier gate"
```

---

## Task 3: CALLER_ROLE task-local + 网关注入 + start_run 落库

**Files:**
- Create: `src/gateway/caller_identity.rs`
- Modify: `src/gateway/mod.rs`
- Modify: `src/gateway/handlers/agent.rs`（`start_run`，metadata 组装处，约 206 行之后）
- Modify: `src/gateway/server/handler.rs`（操作 gate 之后约 865 计算；924 与 971 scope）

- [ ] **Step 1: 写失败测试（task-local 往返）**

新建 `src/gateway/caller_identity.rs`，内容含实现 + 测试：

```rust
//! `CALLER_ROLE` task-local — carries the originating gateway connection's
//! authorization role across the in-task hop from the WS dispatch loop into
//! [`AgentRunManager::start_run`](crate::gateway::handlers::agent::AgentRunManager),
//! which copies it into the run's metadata BEFORE `tokio::spawn`. From there it
//! rides [`TurnContext`](crate::tools::turn_context::TurnContext) to the
//! tool-dispatch config-tier gate.
//!
//! Scoped only around `process_request` in the authenticated dispatch path
//! (`server::handler`). Never crosses the run's spawn boundary (start_run reads
//! it while still in-task). Unset for non-gateway callers (cron, internal) and,
//! by design, for the local no-auth daemon — the gate treats absent role as
//! trusted.

use tokio::task_local;

task_local! {
    /// Originating connection role: `Some("operator")` / `Some("guest")`, or
    /// `None` when auth is not required (local daemon) or outside any dispatch.
    pub static CALLER_ROLE: Option<String>;
}

/// The originating connection's role for the current task, or `None` outside a
/// `CALLER_ROLE` scope (non-gateway / internal runs) — trusted by the gate.
pub fn current_caller_role() -> Option<String> {
    CALLER_ROLE.try_with(|r| r.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scope_round_trips() {
        let seen = CALLER_ROLE
            .scope(Some("guest".to_string()), async { current_caller_role() })
            .await;
        assert_eq!(seen.as_deref(), Some("guest"));
    }

    #[tokio::test]
    async fn unset_is_none() {
        assert_eq!(current_caller_role(), None);
    }
}
```

- [ ] **Step 2: 注册模块 + 跑测试确认失败→通过**

`src/gateway/mod.rs` 加（按现有 `pub mod` 字母序就近插入）：

```rust
pub mod caller_identity;
```

Run: `cargo test -p alephcore --lib gateway::caller_identity::tests`
Expected: PASS（新模块自洽，无需先 RED——这是基础设施，正确性由两个往返测试锁定）。

- [ ] **Step 3: start_run 落库 metadata**

`src/gateway/handlers/agent.rs`，在 `metadata` 组装块（`metadata.insert("channel_id"...)` 起，约 201-206）之后、`workspace_override` 之前插入：

```rust
        // Stamp the originating connection's authorization role (set by the
        // gateway dispatch loop via CALLER_ROLE) so the tool-dispatch tier gate
        // can reject config-mutating tools for chat-tier devices. Covers BOTH
        // chat.send and agent.run since both reach here via start_run in the
        // same task. Absent for non-gateway runs (cron/internal) and for the
        // local no-auth daemon → the gate treats those as trusted.
        if let Some(role) = crate::gateway::caller_identity::current_caller_role() {
            metadata.insert("caller_role".to_string(), role);
        }
```

- [ ] **Step 4: 网关计算 caller_role（auth-mode 感知）**

`src/gateway/server/handler.rs`，在操作员门控块结束（`}` at 865）之后、`// Handle events.* methods specially`（867）之前插入：

```rust
                                    // Originating-connection role for the
                                    // config-tier tool gate. ONLY in auth mode:
                                    // a no-auth local daemon leaves this None so
                                    // every run is trusted (zero regression —
                                    // B1's role fallback can be "guest" even
                                    // locally, which must NOT gate config here).
                                    let caller_role: Option<String> =
                                        if ctx.auth_mode.is_auth_required() {
                                            let conns = ctx.connections.read().await;
                                            conns.get(&conn_id).and_then(|s| s.role.clone())
                                        } else {
                                            None
                                        };
```

- [ ] **Step 5: scope CALLER_ROLE（do_lane_dispatch 闭包参数 + 924）**

`handler.rs`，`do_lane_dispatch` 闭包（约 921）签名加形参 `caller_role: Option<String>,`，并把闭包体内 `process_request` 调用（924）改为：

```rust
                                                Ok(_permit) => crate::gateway::caller_identity::CALLER_ROLE
                                                    .scope(caller_role, process_request(&text, &mc))
                                                    .await,
```

两个 `do_lane_dispatch(...)` 调用点（约 1000 与 1004）各加末参 `caller_role.clone()`。

- [ ] **Step 6: scope CALLER_ROLE（幂等 Proceed 路径 971）**

`handler.rs:971` 改为：

```rust
                                                                let resp = crate::gateway::caller_identity::CALLER_ROLE
                                                                    .scope(caller_role.clone(), process_request(&text, &ctx.middleware_chain))
                                                                    .await;
```

> 注：`handler.rs:606`（connect/bootstrap 早期路径）**不** scope —— 该路径不启动 run，且 connect 本身正是建立角色的请求。

- [ ] **Step 7: 全量编译**

Run: `cargo check -p alephcore --all-targets`
Expected: 通过。若闭包 `async move` 报 `caller_role` 被移动后再用：确认 924 的 `.scope(caller_role, ...)` 直接消费闭包形参（每次调用传入独立 clone），无二次使用。

- [ ] **Step 8: 提交**

```bash
git add src/gateway/caller_identity.rs src/gateway/mod.rs \
        src/gateway/handlers/agent.rs src/gateway/server/handler.rs
git commit -m "gateway: stamp caller role into runs for config-tier tool gate"
```

---

## Task 4: 工具分发 tier 门控 + 单元测试

**Files:**
- Modify: `src/tools/scoped/dispatch.rs`（`execute_inner`，`is_allowed` 检查之后、confirm 门控之前，约 110-119）
- Modify: `src/tools/scoped/tests.rs`（新增门控测试）

- [ ] **Step 1: 写失败测试**

在 `src/tools/scoped/tests.rs` 末尾追加（沿用文件内既有 `LoopToolRegistry` / probe-tool 模式；`cron_manage` 是 OPERATOR_TOOLS 成员，用一个 stub 工具占该名）：

```rust
#[tokio::test]
async fn chat_tier_blocked_from_config_tool() {
    use crate::routing::session_key::SessionKey;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(EchoTool { name: "cron_manage".to_string() }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("a"),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("guest".to_string()),
        });
    let err = svc.execute("cron_manage", json!({})).await.unwrap_err();
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "chat tier must be denied config tool, got {err:?}"
    );
}

#[tokio::test]
async fn operator_tier_allowed_config_tool() {
    use crate::routing::session_key::SessionKey;
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(EchoTool { name: "cron_manage".to_string() }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_turn_context(crate::tools::turn_context::TurnContext {
            session_key: SessionKey::main("a"),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: Some("operator".to_string()),
        });
    assert!(svc.execute("cron_manage", json!({})).await.is_ok());
}

#[tokio::test]
async fn no_turn_context_allows_config_tool() {
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(EchoTool { name: "cron_manage".to_string() }));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new());
    assert!(svc.execute("cron_manage", json!({})).await.is_ok(),
        "internal/non-gateway run (no turn context) must pass");
}
```

> 若 `tests.rs` 已有可复用的最小回声/probe 工具（如 `SessionProbeTool`），改用之；否则新增一个最小 `EchoTool { name }`，其 `name()` 返回该名、`execute` 返回 `json!({"ok": true})`（参照文件内现有 stub 工具实现 `LoopTool`）。三个测试均通过 `caller_role` 区分档位。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib scoped::tests::chat_tier_blocked_from_config_tool`
Expected: FAIL —— 当前无门控，chat 档也会执行成功（返回 Ok 而非 PermissionDenied）。

- [ ] **Step 3: 实现门控**

`src/tools/scoped/dispatch.rs`，在 `execute_inner` 的 `is_allowed` 检查块（`if !self.is_allowed(name) { return Err(ToolError::NotFound...) }`，约 106-110）之后、confirm 门控（`if self.confirm_tools.contains(name) ...`，约 119）之前插入：

```rust
        // Config-tier authorization gate. A chat-tier connection (remote device
        // paired at "chat" level) may converse and read, but must not mutate
        // Aleph's own configuration through tools (R8: config IS a tool, so the
        // interception must live at the tool-dispatch chokepoint). The
        // originating connection's role rides in TURN_CONTEXT, stamped at run
        // start. Operator devices, the local no-auth daemon, and non-gateway
        // runs (cron/internal) all pass. Live operator approval ("sudo") is
        // Phase 2b — today this hard-rejects, mirroring B1's RPC gate.
        if crate::gateway::method_authz::tool_requires_operator(name) {
            let is_operator = crate::tools::turn_context::current_turn_context()
                .map(|t| t.caller_is_operator())
                .unwrap_or(true);
            if !is_operator {
                return Err(ToolError::PermissionDenied {
                    name: name.to_string(),
                    reason: format!(
                        "`{name}` changes Aleph's own configuration and requires operator \
                         (config) authorization. This device is paired at chat level. Ask the \
                         server operator to grant config access (Devices → 授权配置), then retry. \
                         Do not retry until authorized."
                    ),
                });
            }
        }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib scoped::tests`
Expected: PASS（3 个新测试 + 既有 scoped 测试全绿）。

- [ ] **Step 5: 提交**

```bash
git add src/tools/scoped/dispatch.rs src/tools/scoped/tests.rs
git commit -m "tools: gate config tools for chat-tier connections at dispatch"
```

---

## Task 5: 集成验证 + lint

**Files:** 无新增源码（验证既有 + 收尾）

- [ ] **Step 1: 端到端传播验证（如 AgentRunManager 可测则补，否则跳过并说明）**

若 `src/gateway/handlers/agent.rs` 的测试模块已有 `AgentRunManager` 构造夹具：在 `CALLER_ROLE.scope(Some("guest"), start_run(params))` 后断言所产生 run 的 `RunRequest.metadata["caller_role"] == "guest"`（可注入 stub `execution_adapter` 捕获 `RunRequest`）。若无现成夹具，**不新建重型夹具**——Task 3 的 task-local 往返测试 + Task 4 的真实 `execute` 门控测试已覆盖关键行为；在本 step 注明跳过原因。

- [ ] **Step 2: 全量测试**

Run: `cargo test -p alephcore --lib`
Expected: 全绿。

- [ ] **Step 3: lint + fmt**

Run: `cargo fmt -p alephcore && cargo clippy -p alephcore --all-targets`
Expected: 改动文件零 clippy 警告，fmt 净。

- [ ] **Step 4: 收尾提交（若 fmt 有改动）**

```bash
git add -u
git commit -m "chore: fmt + clippy for chat/config tier Phase 2"
```

---

## 红线对账

| 红线 | 落地 |
|---|---|
| R4 — Interface 无业务逻辑 | tier 判定全在 core（gateway/tools），Panel/Interface 不参与 |
| R7/R9 — LLM 主权 | 门控是确定性安全硬过滤（赋能层允许），不替 LLM 推理 |
| R8 — 工具即一切 | 正因 config 是工具，才在工具分发咽喉点拦截 |
| R10 — 薄 harness | 门控在既有分发点（`execute_inner`），不进 `src/harness/`；auth-mode 智能留在网关边界，工具门控是纯角色判断（笨门控） |

## 范围外（本期不做）

- **live sudo 现场审批（Phase 2b）**：operator 定向投递子系统（事件经 `event_scope` 门控到 operator + `ExecApprovalManager`/`exec.approval.resolve` 回收）+ run 挂起恢复。现有 `ChannelApprovalBridgeAdapter` 只投回发起者自身频道，不可直接复用。
- **Phase 3 Panel UI**：配对授级双钮、Devices「授权配置/降级」、sudo「等待授权」态。
- **动作级细粒度**（同一工具内区分读/写）。
- **真 WS-level RPC e2e**（chat token 发 `config.apply` 断言 `PERMISSION_DENIED`）—— 测的是已发布的 B1 RPC 门控代码，非本期新代码；非阻塞 follow-up。

## Self-Review

- **Spec 覆盖**：缺口 2「工具路径门控」= Task 1+4；缺口 3「角色随 run 注入 ScopedToolService」= Task 2+3。缺口 2 的「硬拒→sudo」按用户决策拆出 Phase 2b。RPC 路径门控 B1 已完成，本期不动。✔
- **Placeholder 扫描**：所有步骤含确切路径/行号/代码；无 TBD。Task 5 step1 显式给出「有夹具则做、无则跳过并说明」的判定，非占位。✔
- **类型一致**：`tool_requires_operator(&str)->bool`（Task1）↔ dispatch 调用（Task4）一致；`caller_role: Option<String>`（Task2 字段）↔ `current_caller_role()->Option<String>`（Task3）↔ metadata `String`（Task3 start_run）↔ run_loop `.cloned()`（Task2）一致；`ToolError::PermissionDenied{name,reason}` 与 service.rs:18 定义一致。✔
