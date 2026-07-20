# 定时任务 Agent 选择器 + 删除回退 + Channel 只读显示 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 cron 表单的 agent 文本框换成下拉选择器（默认 main），executor 在绑定 agent 被删除时优雅回退到 main 并留痕，表单只读显示真实投递通道。

**Architecture:** 后端 executor 用既有异步 `AgentRegistry::get_default()` 做回退（default_agent 即内建 "main"），回退在运行历史的 output 里前置一行双语标记，不加表字段。Panel 复用既有 `AgentsApi::list()` 渲染 `<select>`，失效绑定显示「（已删除）」标记项。channel 只读，需在网关 JSON 序列化和 panel DTO 各补一个字段。

**Tech Stack:** Rust (alephcore, tokio)、Leptos/WASM (aleph-panel)、leptos_i18n（locales/*.json）。

设计文档：`docs/superpowers/specs/2026-06-06-cron-agent-selector-fallback-design.md`

---

## 关键事实（实现者须知）

- executor 用的是**异步** registry：`crate::gateway::agent_instance::AgentRegistry`，`get(&str).await -> Option<Arc<AgentInstance>>`，并有 `get_default().await`（其 `default_agent` 字段固定为 `"main"`）。**不是** `crate::agents::registry::AgentRegistry`（那个是同步的，别用错）。
- `AgentInstance::id() -> &str`。
- cron 投递（`deliver_to_channel`）只用 `snapshot.source_channel_id` + `source_conversation_id`，**不看** agent↔channel 绑定。本计划**不改投递逻辑**。
- 回退note 只进**持久化 output**（运行历史），**不进投递消息**——所以要在 delivery 之后、构造 `ExecutionResult.output` 时才前置 note。
- panel 是 wasm crate 但含 `rlib`，纯函数可用 `cargo test -p aleph-panel --lib` 跑原生测试（既有测试即 `#[test]` 风格）。
- `cron.list` RPC 由 `src/gateway/handlers/cron/real.rs::job_view_to_json` **手写 JSON** 输出，当前**未**包含 `source_channel_id`，必须补。

---

## File Structure

| 文件 | 改动 |
|------|------|
| `src/tasks/cron/executor.rs` | 新增 `fallback_note` / `prepend_fallback_note` 纯函数 + `resolve_cron_agent` 异步helper；改 `execute_cron_job` 解析与 output 构造；新增 4 个测试 |
| `src/gateway/handlers/cron/real.rs` | `job_view_to_json` 增加 `"source_channel_id"` 字段 |
| `interfaces/webchat/src/api/cron.rs` | `CronJobInfo` 增加 `source_channel_id` 字段 |
| `interfaces/webchat/src/views/cron.rs` | 加载 agent 列表 + default_id 信号；文本框→`<select>`（含失效项 + 纯函数 `stale_agent_option`）；只读 channel 行；修 quick-create 默认 |
| `interfaces/webchat/locales/en.json` | 新增 4 个 cron key |
| `interfaces/webchat/locales/zh.json` | 新增 4 个 cron key |

---

## Task 1: Executor — 回退留痕纯函数

**Files:**
- Modify: `src/tasks/cron/executor.rs`（在 `// ── Tests ──` 之前、文件顶层加两个 fn；测试加到 `mod tests`）

- [ ] **Step 1: 写失败测试**

在 `src/tasks/cron/executor.rs` 的 `mod tests` 内（`use super::*;` 已存在）追加：

```rust
    #[test]
    fn fallback_note_names_requested_agent() {
        let n = fallback_note("oldie");
        assert!(n.contains("oldie"), "note must name the missing agent");
        assert!(n.contains("main"), "note must mention the fallback target");
    }

    #[test]
    fn prepend_fallback_note_prefixes_existing_output() {
        let out = prepend_fallback_note(Some("done".to_string()), "oldie").unwrap();
        assert!(out.starts_with(&fallback_note("oldie")));
        assert!(out.ends_with("done"));
    }

    #[test]
    fn prepend_fallback_note_uses_note_as_output_when_empty() {
        let out = prepend_fallback_note(None, "oldie").unwrap();
        assert_eq!(out, fallback_note("oldie"));
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib tasks::cron::executor::tests::fallback 2>&1 | tail -20`
Expected: 编译失败 / `cannot find function fallback_note`。

- [ ] **Step 3: 实现纯函数**

在 `src/tasks/cron/executor.rs` 顶层（紧挨 `make_error_result` 函数定义之前或之后均可，文件作用域内）加入：

```rust
/// Bilingual note prepended to a cron run's persisted output when the
/// requested agent was missing and the run fell back to the default agent.
/// Kept as a fixed bilingual string because the executor has no panel i18n
/// context; mirrors the `cron.fallback_note` locale entry.
fn fallback_note(requested: &str) -> String {
    format!(
        "原 agent '{requested}' 不存在，已回退到 main / \
         Agent '{requested}' not found, fell back to main"
    )
}

/// Prepend [`fallback_note`] to a run's output. When the run produced no text,
/// the note becomes the entire output so the fallback stays visible in run
/// history. Always returns `Some`.
fn prepend_fallback_note(output: Option<String>, requested: &str) -> Option<String> {
    let note = fallback_note(requested);
    Some(match output {
        Some(text) => format!("{note}\n{text}"),
        None => note,
    })
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib tasks::cron::executor::tests::fallback 2>&1 | tail -20` 然后 `cargo test -p alephcore --lib tasks::cron::executor::tests::prepend 2>&1 | tail -20`
Expected: 3 个测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/tasks/cron/executor.rs
git commit -m "cron: add fallback note helpers for missing-agent degradation"
```

---

## Task 2: Executor — `resolve_cron_agent` 回退解析

**Files:**
- Modify: `src/tasks/cron/executor.rs`（新增异步 helper + 2 个 `#[tokio::test]`）

- [ ] **Step 1: 写失败测试**

在 `mod tests` 内追加（注意 `tempfile` 已是 dev-dep）：

```rust
    async fn test_registry_with_main() -> (tempfile::TempDir, AgentRegistry) {
        use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
        use crate::gateway::session_store::sqlite_backend::{
            SqliteSessionStore, SqliteSessionStoreConfig,
        };
        use crate::gateway::session_store::SessionStore;

        let temp = tempfile::tempdir().unwrap();
        let store: std::sync::Arc<dyn SessionStore> = std::sync::Arc::new(
            SqliteSessionStore::new(SqliteSessionStoreConfig {
                db_path: temp.path().join("s.db"),
                ..Default::default()
            })
            .unwrap(),
        );
        let registry = AgentRegistry::new(); // default_agent = "main"
        let main = AgentInstance::new(
            AgentInstanceConfig {
                agent_id: "main".to_string(),
                workspace: temp.path().join("ws"),
                agent_dir: temp.path().join("agents/main"),
                ..Default::default()
            },
            store,
        )
        .unwrap();
        registry.register(main).await;
        (temp, registry)
    }

    #[tokio::test]
    async fn resolve_uses_requested_agent_when_present() {
        let (_t, registry) = test_registry_with_main().await;
        let (inst, used, fell_back) = resolve_cron_agent(&registry, "main").await.unwrap();
        assert_eq!(inst.id(), "main");
        assert_eq!(used, "main");
        assert!(!fell_back);
    }

    #[tokio::test]
    async fn resolve_falls_back_to_default_when_missing() {
        let (_t, registry) = test_registry_with_main().await;
        let (inst, used, fell_back) = resolve_cron_agent(&registry, "ghost").await.unwrap();
        assert_eq!(inst.id(), "main");
        assert_eq!(used, "main");
        assert!(fell_back);
    }

    #[tokio::test]
    async fn resolve_returns_none_when_default_absent() {
        let registry = AgentRegistry::new(); // empty: no "main"
        assert!(resolve_cron_agent(&registry, "ghost").await.is_none());
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p alephcore --lib tasks::cron::executor::tests::resolve 2>&1 | tail -20`
Expected: 编译失败 / `cannot find function resolve_cron_agent`。

- [ ] **Step 3: 实现 helper**

在 `src/tasks/cron/executor.rs` 顶层（`execute_cron_job` 之前）加入。注意 `AgentRegistry` 已通过 `use crate::gateway::agent_instance::AgentRegistry;` 在文件作用域内；`Arc` 已用；`warn!` 已用：

```rust
/// Resolve which agent instance to run a cron job with. When `requested` is
/// missing from the registry, fall back to the registry's default agent
/// (the built-in "main", which cannot be deleted). Returns the resolved
/// instance, the id it resolved under (for session keying), and whether a
/// fallback occurred. Returns `None` only when even the default is absent
/// (should not happen in production — "main" is built-in).
async fn resolve_cron_agent(
    registry: &AgentRegistry,
    requested: &str,
) -> Option<(Arc<crate::gateway::agent_instance::AgentInstance>, String, bool)> {
    if let Some(agent) = registry.get(requested).await {
        return Some((agent, requested.to_string(), false));
    }
    warn!(requested, "cron agent missing, falling back to default agent");
    let agent = registry.get_default().await?;
    let used = agent.id().to_string();
    Some((agent, used, true))
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p alephcore --lib tasks::cron::executor::tests::resolve 2>&1 | tail -20`
Expected: 3 个 resolve 测试 PASS。

- [ ] **Step 5: 提交**

```bash
git add src/tasks/cron/executor.rs
git commit -m "cron: add resolve_cron_agent with default fallback"
```

---

## Task 3: Executor — 把回退接入 `execute_cron_job`

**Files:**
- Modify: `src/tasks/cron/executor.rs:114-139`（解析块）与成功路径 `output:` 字段（约 281-292 行的 `ExecutionResult { ... status: RunStatus::Ok ... }`）

- [ ] **Step 1: 替换解析块**

把当前（约 114-131 行）：

```rust
    // Resolve agent_id, defaulting to "main"
    let agent_id = snapshot.agent_id.as_deref().unwrap_or("main");

    // Look up agent in registry
    let agent = match registry.get(agent_id).await {
        Some(a) => a,
        None => {
            warn!(job_id = %snapshot.id, agent_id, "cron job agent not found in registry");
            return make_error_result(
                started_at,
                format!("agent not found: {agent_id}"),
                ErrorReason::Permanent(format!("agent '{agent_id}' is not registered")),
                // Missing-agent is never transient — classify returns permanent.
                RetryHint::permanent(),
                snapshot.trigger_source,
            );
        }
    };
```

替换为：

```rust
    // Resolve agent, defaulting to "main" when unset and gracefully falling
    // back to the built-in default when the bound agent was deleted.
    let requested_agent = snapshot.agent_id.as_deref().unwrap_or("main").to_string();
    let (agent, agent_id, fell_back) =
        match resolve_cron_agent(&registry, &requested_agent).await {
            Some(resolved) => resolved,
            None => {
                warn!(job_id = %snapshot.id, requested = %requested_agent,
                    "cron job: neither requested agent nor default 'main' is registered");
                return make_error_result(
                    started_at,
                    "built-in 'main' agent is not registered".to_string(),
                    ErrorReason::Permanent(
                        "built-in 'main' agent is not registered".to_string(),
                    ),
                    RetryHint::permanent(),
                    snapshot.trigger_source,
                );
            }
        };
    let agent_id = agent_id.as_str();
```

> 说明：后续代码已大量使用 `agent_id`（`SessionKey::task(agent_id, ...)`、`info!(... agent_id ...)`），新写法保留了同名 `&str` 绑定，无需改动它们。`agent` 类型仍是 `Arc<AgentInstance>`，与原 `registry.get` 返回一致。

- [ ] **Step 2: 成功路径 output 前置 note**

找到成功分支构造（约 281-292 行）：

```rust
            ExecutionResult {
                started_at,
                ended_at,
                duration_ms: ended_at.saturating_sub(started_at),
                status: RunStatus::Ok,
                output: final_response,
                error: None,
```

把 `output: final_response,` 改为：

```rust
                output: if fell_back {
                    prepend_fallback_note(final_response, &requested_agent)
                } else {
                    final_response
                },
```

> `final_response` 在此前的 carryover / delivery 中均以 `&` 借用，未被移动，此处可安全消费。delivery 用的是干净的 `final_response`，note 不会进投递消息（符合设计：只进运行历史）。

- [ ] **Step 3: 写回退集成断言（复用既有错误测试位）**

executor 当前没有 `execute_cron_job` 全链路测试（需 mock `ExecutionAdapter`，超出本任务范围）。回退解析与 note 已分别由 Task 2 / Task 1 单测覆盖。本步只做**编译与既有测试回归**：

Run: `cargo test -p alephcore --lib tasks::cron::executor 2>&1 | tail -25`
Expected: 全部 PASS（含 Task1/2 新测试 + 既有 `build_cron_prompt` 测试），无编译错误。

- [ ] **Step 4: clippy 检查改动文件**

Run: `cargo clippy -p alephcore --lib 2>&1 | grep -A3 "executor.rs" | head -20`
Expected: executor.rs 无新增 warning。

- [ ] **Step 5: 提交**

```bash
git add src/tasks/cron/executor.rs
git commit -m "cron: gracefully fall back to main when bound agent is deleted"
```

---

## Task 4: 网关 — `cron.list` 输出 source_channel_id

**Files:**
- Modify: `src/gateway/handlers/cron/real.rs`（`job_view_to_json` 函数）

- [ ] **Step 1: 加字段**

在 `job_view_to_json` 的 `json!({ ... })` 里，`"agent_id": view.agent_id,` 之后加一行：

```rust
        "source_channel_id": view.source_channel_id,
```

（`CronJobView.source_channel_id: Option<String>` 已存在，序列化为 string 或 null。）

- [ ] **Step 2: 编译确认**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: 通过，无错误。

- [ ] **Step 3: 提交**

```bash
git add src/gateway/handlers/cron/real.rs
git commit -m "gateway: expose source_channel_id in cron.list response"
```

---

## Task 5: Panel API — CronJobInfo 增加 source_channel_id

**Files:**
- Modify: `interfaces/webchat/src/api/cron.rs`（`CronJobInfo` 结构体，约 15-55 行）

- [ ] **Step 1: 加字段**

在 `CronJobInfo` 中，`pub agent_id: String,`（带其上的 `#[serde(default)]`）之后加：

```rust
    #[serde(default)]
    pub source_channel_id: Option<String>,
```

- [ ] **Step 2: 编译确认**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -15`
Expected: 通过（新增字段带 `#[serde(default)]`，不破坏既有反序列化）。

> 若本机未装 wasm 目标，退用 `cargo check -p aleph-panel 2>&1 | tail -15`（rlib 原生检查）亦可验证类型。

- [ ] **Step 3: 提交**

```bash
git add interfaces/webchat/src/api/cron.rs
git commit -m "panel: add source_channel_id to CronJobInfo DTO"
```

---

## Task 6: Panel — i18n key（en + zh）

**Files:**
- Modify: `interfaces/webchat/locales/en.json`（`"cron"` 段，约 1438 行 `placeholder_agent` 附近）
- Modify: `interfaces/webchat/locales/zh.json`（对应 `"cron"` 段，约 1438 行）

- [ ] **Step 1: en.json 加 key**

在 en.json 的 `cron` 段内（紧接 `"error_name_required": "..."` 之前，注意给前一行补逗号）加入：

```json
    "field_channel": "Delivery channel",
    "channel_none": "None (recorded to run history only)",
    "agent_deleted_suffix": " (deleted)",
```

- [ ] **Step 2: zh.json 加同名 key**

在 zh.json 的 `cron` 段对应位置加入：

```json
    "field_channel": "投递通道",
    "channel_none": "无（仅记录到运行历史）",
    "agent_deleted_suffix": "（已删除）",
```

> 注意：`field_channel` / `channel_none` / `agent_deleted_suffix` 会在 Task 8/9 用到。**故意不加** `fallback_note` locale key——回退留痕由 executor 输出固定双语串（Task 1），panel 不渲染它；且其 `{0}` 占位会被 leptos_i18n 当作变量名 `0`（非法 Rust 标识符）导致代码生成失败。两个 json 的 `cron` 段 key 集合必须一致（都加这 3 个），否则 leptos_i18n 代码生成会报缺键。

- [ ] **Step 3: 验证 JSON 合法 + 代码生成**

Run: `python3 -m json.tool interfaces/webchat/locales/en.json >/dev/null && python3 -m json.tool interfaces/webchat/locales/zh.json >/dev/null && echo JSON_OK`
Expected: 打印 `JSON_OK`。
Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -15`（触发 i18n 代码生成）
Expected: 通过，无 “missing key” 报错。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: add i18n keys for cron channel display and agent fallback"
```

---

## Task 7: Panel — 加载 agent 列表 + default_id 信号

**Files:**
- Modify: `interfaces/webchat/src/views/cron.rs`（imports 约 9 行；表单信号区 约 627 行附近）

- [ ] **Step 1: 引入 AgentsApi + AgentSummary**

在 `interfaces/webchat/src/views/cron.rs` 顶部 import 区（约第 9 行 `use crate::api::cron::...;` 之后）加：

```rust
use crate::api::agents::{AgentsApi, AgentSummary};
```

- [ ] **Step 2: 新增 agent 列表信号 + 加载 Effect**

在表单信号定义区（约 627 行 `let form_agent_id = RwSignal::new(String::new());` 之后）加：

```rust
    // Available agents for the selector (id, display name) + default id.
    let agents = RwSignal::new(Vec::<AgentSummary>::new());
    let default_agent_id = RwSignal::new(String::from("main"));
```

并在该组件已有的「连接后加载数据」逻辑附近（与加载 jobs 的 `Effect`/`spawn_local` 同区域）新增一次性加载：

```rust
    {
        let state = state.clone();
        Effect::new(move || {
            if !state.is_connected.get() {
                return;
            }
            let state = state.clone();
            spawn_local(async move {
                if let Ok(resp) = AgentsApi::list(&state).await {
                    default_agent_id.set(resp.default_id.clone());
                    agents.set(resp.agents);
                }
            });
        });
    }
```

> 若该文件中 `state` 不是 `Clone` 或上下文用法不同，参照 `agent_binding_selector.rs` 里 `let dash = state;` + `Effect::new` 的既有写法对齐（`DashboardState` 在该 crate 内可 clone/copy 使用）。`is_connected` 字段见 `DashboardState`。

- [ ] **Step 3: 编译确认**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -15`
Expected: 通过（agents/default_agent_id 即便暂未在视图使用，也应无 “unused” 致命错误；如有 unused warning 将在 Task 8 消除）。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/views/cron.rs
git commit -m "panel(cron): load available agents and default id for selector"
```

---

## Task 8: Panel — 失效项纯函数 + `<select>` 替换 + quick-create 默认

**Files:**
- Modify: `interfaces/webchat/src/views/cron.rs`（新增纯函数 + `mod tests`；表单 agent `<input>` 约 1144-1151 行；quick-create 默认 约 456 行）

- [ ] **Step 1: 写失败测试（纯函数 stale_agent_option）**

在 `interfaces/webchat/src/views/cron.rs` 文件末尾追加（若已有 `#[cfg(test)] mod tests` 则并入）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::agents::AgentSummary;

    fn agent(id: &str) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: Some(id.to_string()),
            emoji: None,
            description: None,
            model: None,
            is_default: id == "main",
        }
    }

    #[test]
    fn stale_option_none_when_current_in_list() {
        let list = vec![agent("main"), agent("research")];
        assert_eq!(stale_agent_option("research", &list), None);
    }

    #[test]
    fn stale_option_none_when_current_empty() {
        let list = vec![agent("main")];
        assert_eq!(stale_agent_option("", &list), None);
    }

    #[test]
    fn stale_option_some_when_current_deleted() {
        let list = vec![agent("main")];
        assert_eq!(
            stale_agent_option("gone", &list),
            Some("gone".to_string())
        );
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib views::cron::tests 2>&1 | tail -20`
Expected: 编译失败 / `cannot find function stale_agent_option`。

- [ ] **Step 3: 实现纯函数**

在 `cron.rs` 顶层（组件函数之外，文件作用域）加入：

```rust
/// Returns the id to render as a "(deleted)" placeholder option when the job's
/// currently-bound agent is no longer in the available list. `None` when the
/// current id is empty or still present.
fn stale_agent_option(current: &str, available: &[crate::api::agents::AgentSummary]) -> Option<String> {
    if current.is_empty() || available.iter().any(|a| a.id == current) {
        None
    } else {
        Some(current.to_string())
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib views::cron::tests 2>&1 | tail -20`
Expected: 3 个测试 PASS。

- [ ] **Step 5: 替换 agent 文本框为 `<select>`**

把当前 agent 字段（约 1140-1151 行）：

```rust
                                // Agent
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_agent)}
                                    </label>
                                    <input
                                        type="text"
                                        prop:value=move || form_agent_id.get()
                                        on:input=move |ev| form_agent_id.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                        placeholder=move || t_string!(i18n, cron.placeholder_agent).to_string()
                                    />
                                </div>
```

替换为：

```rust
                                // Agent
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_agent)}
                                    </label>
                                    <select
                                        on:change=move |ev| form_agent_id.set(event_target_value(&ev))
                                        class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:border-primary"
                                    >
                                        {move || {
                                            let current = form_agent_id.get();
                                            let list = agents.get();
                                            let deleted_suffix =
                                                t_string!(i18n, cron.agent_deleted_suffix).to_string();
                                            let stale = stale_agent_option(&current, &list);
                                            let mut opts = list
                                                .into_iter()
                                                .map(|a| {
                                                    let id = a.id.clone();
                                                    let label = a.name.clone().unwrap_or_else(|| a.id.clone());
                                                    let sel = id == current;
                                                    view! {
                                                        <option value=id selected=sel>{label}</option>
                                                    }
                                                    .into_any()
                                                })
                                                .collect::<Vec<_>>();
                                            if let Some(stale_id) = stale {
                                                let label = format!("{stale_id}{deleted_suffix}");
                                                opts.push(view! {
                                                    <option value=stale_id selected=true disabled=true>
                                                        {label}
                                                    </option>
                                                }.into_any());
                                            }
                                            opts
                                        }}
                                    </select>
                                </div>
```

> 说明：失效项 `disabled=true` 且 `selected=true`——浏览器会显示它为当前值并标记不可重选，用户改选有效 agent 即覆盖 `form_agent_id`。保存逻辑（`form_agent_id.get()`）不变；若用户不动它，executor 会在运行时兜底回退（Task 3）。

- [ ] **Step 6: 新建任务默认选 default_id（表单重置处）**

在「Reset form for new job」块（约 656 行 `form_agent_id.set(String::new());`）改为：

```rust
                form_agent_id.set(default_agent_id.get_untracked());
```

并把 quick-create 预设（约 456 行）的 `agent_id: "main".to_string(),` 改为：

```rust
                                                    agent_id: default_agent_id.get_untracked(),
```

> 若 quick-create 闭包作用域内访问不到 `default_agent_id`（它定义在主组件作用域），保持 `"main".to_string()` 不变即可——`"main"` 等价于内建默认，无功能差异；此改动为可选优化。优先保证编译通过。

- [ ] **Step 7: 编译 + 跑测试**

Run: `cargo test -p aleph-panel --lib views::cron::tests 2>&1 | tail -20`
Expected: 3 个测试仍 PASS。
Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -15`
Expected: 通过，无 unused（agents/default_agent_id 现已被使用）。

- [ ] **Step 8: 提交**

```bash
git add interfaces/webchat/src/views/cron.rs
git commit -m "panel(cron): replace agent text input with selector + deleted marker"
```

---

## Task 9: Panel — 只读投递通道显示

**Files:**
- Modify: `interfaces/webchat/src/views/cron.rs`（新增 `form_channel` 信号 + edit 时填充 + 视图只读行）

- [ ] **Step 1: 新增 channel 信号**

在表单信号区（Task 7 加的 `default_agent_id` 之后）加：

```rust
    let form_channel = RwSignal::new(Option::<String>::None);
```

- [ ] **Step 2: edit/new 时填充**

在「Reset form for new job」块加：

```rust
                form_channel.set(None);
```

在「Load existing job data」块（约 682 行 `form_agent_id.set(job.agent_id.clone());` 之后）加：

```rust
                    form_channel.set(job.source_channel_id.clone());
```

- [ ] **Step 3: 视图加只读行**

在 Task 8 的 agent `<select>` 那个 `</div>` 之后、`// Prompt` 之前插入：

```rust
                                // Delivery channel (read-only)
                                <div>
                                    <label class="block text-sm font-medium text-text-secondary mb-2">
                                        {t!(i18n, cron.field_channel)}
                                    </label>
                                    <div class="w-full px-4 py-2 bg-surface-sunken border border-border rounded-lg text-text-tertiary text-sm">
                                        {move || match form_channel.get() {
                                            Some(ch) if !ch.is_empty() => ch,
                                            _ => t_string!(i18n, cron.channel_none).to_string(),
                                        }}
                                    </div>
                                </div>
```

- [ ] **Step 4: 编译确认**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -15`
Expected: 通过。`source_channel_id` 经 Task 4/5 已贯通（RPC→DTO→form_channel→视图）。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/views/cron.rs
git commit -m "panel(cron): show read-only delivery channel for awareness"
```

---

## Task 10: 全量验证

**Files:** 无（验证）

- [ ] **Step 1: 后端测试**

Run: `cargo test -p alephcore --lib tasks::cron 2>&1 | tail -25`
Expected: 全部 PASS。

- [ ] **Step 2: 后端 clippy**

Run: `cargo clippy -p alephcore --lib 2>&1 | tail -15`
Expected: 无新增 error/warning（与改动文件相关）。

- [ ] **Step 3: Panel 测试**

Run: `cargo test -p aleph-panel --lib 2>&1 | tail -20`
Expected: 全部 PASS（含新增 6 个：3 stale_agent_option + executor 侧不在此 crate）。

- [ ] **Step 4: Panel 全量 wasm 构建**

Run: `just wasm 2>&1 | tail -20`
Expected: 构建成功，生成 `interfaces/webchat/dist/*`。

- [ ] **Step 5: （可选）手动验证清单**

> 这些需运行 daemon + panel，留给人工：参考 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」重编 binary 并热替换后：
> 1. cron 表单 agent 字段为下拉，新建任务默认选中 main。
> 2. 编辑一个绑定已删除 agent 的旧任务，下拉显示「{id}（已删除）」灰项。
> 3. 表单显示「投递通道：{channel}」或「无（仅记录到运行历史）」。
> 4. 让一个绑定已删 agent 的任务触发执行，运行历史 output 首行出现回退双语标记，状态为成功。

- [ ] **Step 6: 最终提交（如有未提交的格式化等）**

```bash
git status
git add -A && git commit -m "cron: finalize agent selector + fallback + channel display" || echo "nothing to commit"
```

---

## Self-Review 结果

- **Spec 覆盖**：①agent 选择器→Task 7/8；②默认 main→Task 8 Step 6；③失效项标记→Task 8；④executor 回退→Task 2/3；⑤回退留痕→Task 1/3；⑥channel 只读显示→Task 4/5/9；⑦i18n→Task 6。全部有对应任务。
- **Placeholder 扫描**：无 TBD/TODO；每个代码步均含完整代码与命令。
- **类型一致性**：`resolve_cron_agent` 返回 `(Arc<AgentInstance>, String, bool)` 在 Task 2 定义、Task 3 消费一致；`stale_agent_option(&str, &[AgentSummary]) -> Option<String>` 在 Task 8 定义与测试一致；`source_channel_id: Option<String>` 在网关 JSON（Task 4）、DTO（Task 5）、form_channel（Task 9）三处命名一致；i18n key 名（`field_channel`/`channel_none`/`agent_deleted_suffix`）在 Task 6 定义、Task 8/9 使用一致；`fallback_note` 故意不入 locale（executor 固定串，避免 `{0}` codegen 风险）。
