# Panel「Network」配置页 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 panel 新增合并单页 `/settings/network`,Section 1 完成壳核分离连接切换(A),Section 2 接线集群 `environments.list`+`cluster.enroll` 并为 0c 留占位(B)。

**Architecture:** 纯 panel(`aleph-panel` Leptos CSR/WASM)新增代码,零 core / 零 shell 改动。A 经新建的 WASM→Tauri v2 invoke 桥调用 shell 已注册命令(`get/set/clear_connection_target`),仅桌面 shell 内可交互、浏览器内只读降级;B 经既有 `state.rpc_call` 调 main 已有 RPC。

**Tech Stack:** Rust + Leptos 0.8 (csr) + wasm-bindgen 0.2 / wasm-bindgen-futures 0.4 / js-sys 0.3 / web-sys 0.3 + serde_json。

**Spec:** `docs/superpowers/specs/2026-06-08-panel-network-config-page-design.md`

---

## File Structure

**新增**
- `interfaces/webchat/src/api/tauri_bridge.rs` — `is_shell()` + async `get/set/clear_connection_target` + 纯函数 `normalize_endpoint_preview`(+ 单测)。
- `interfaces/webchat/src/api/cluster.rs` — `Environment` / `CommandDescriptor` / `EnrollResult` 类型 + `ClusterApi::{list_environments, enroll_node}`(+ 解析单测);0c 方法留注释占位。
- `interfaces/webchat/src/views/settings/network/mod.rs` — `NetworkView` 外壳,组合两 section。
- `interfaces/webchat/src/views/settings/network/connection.rs` — Section 1 `ConnectionSection`(A)。
- `interfaces/webchat/src/views/settings/network/cluster.rs` — Section 2 `ClusterSection`(B)。

**修改**
- `interfaces/webchat/src/api.rs` — 注册 `pub mod tauri_bridge;` + `pub mod cluster;`。
- `interfaces/webchat/src/views/settings/mod.rs` — `pub mod network;` + `pub use network::NetworkView;`。
- `interfaces/webchat/src/components/settings_sidebar.rs` — `SettingsTab::Network`(path/label/icon)+ 新分组 `Network`(Advanced 之后)+ 分组 label。
- `interfaces/webchat/src/app.rs` — `/settings/network` 路由。

**测试可行性确认**:`aleph-panel` 已有多处 host-runnable `#[cfg(test)]`(如 `appearance.rs`、`state/*.rs`),故 `cargo test -p aleph-panel` 可在 host 跑纯逻辑单测。

> **提交纪律**:每个 Task 的 commit 步骤**只 `git add` 该 Task 列出的具体文件路径**(显式 pathspec),绝不 `git add -A`。

---

## Task 1: Tauri invoke 桥 + endpoint 预览归一(api/tauri_bridge.rs)

**Files:**
- Create: `interfaces/webchat/src/api/tauri_bridge.rs`
- Modify: `interfaces/webchat/src/api.rs`(加 `pub mod tauri_bridge;`)

- [ ] **Step 1: 写失败测试**(先建文件,只放纯函数 + 测试,bridge 部分下一步补)

`interfaces/webchat/src/api/tauri_bridge.rs`:
```rust
//! WASM → Tauri v2 invoke 桥(panel 唯一的 shell-command 出口)。
//! 仅在桌面 Tauri shell 内可用(withGlobalTauri=true 暴露
//! window.__TAURI__.core.invoke);纯浏览器内 is_shell()=false,调用方需降级。

/// 仅供 UI 预览/即时提示的 endpoint 归一,镜像 shell 端
/// `ConnectionTarget::parse` 的显示形态(补 http scheme + 默认端口 18790)。
/// 权威解析由 shell 的 `set_connection_target` 完成;此处只为预览,
/// IPv6 等边角由权威解析兜底。
pub fn normalize_endpoint_preview(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("local") {
        return "local".to_string();
    }
    let with_scheme = if t.starts_with("http://") || t.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{t}")
    };
    let (scheme, after) = match with_scheme.split_once("://") {
        Some(p) => p,
        None => return with_scheme,
    };
    let (host, path) = match after.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{p}")),
        None => (after.to_string(), String::new()),
    };
    if host.contains(':') {
        // 已带端口(或 IPv6,预览不强加端口)
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{host}:18790{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_local_map_to_local() {
        assert_eq!(normalize_endpoint_preview(""), "local");
        assert_eq!(normalize_endpoint_preview("  "), "local");
        assert_eq!(normalize_endpoint_preview("LOCAL"), "local");
    }

    #[test]
    fn bare_host_gets_http_and_default_port() {
        assert_eq!(normalize_endpoint_preview("core.example"), "http://core.example:18790");
    }

    #[test]
    fn explicit_port_preserved() {
        assert_eq!(normalize_endpoint_preview("core.example:9000"), "http://core.example:9000");
    }

    #[test]
    fn https_scheme_preserved_and_port_added() {
        assert_eq!(normalize_endpoint_preview("https://core.example"), "https://core.example:18790");
    }

    #[test]
    fn https_with_port_unchanged() {
        assert_eq!(normalize_endpoint_preview("https://core.example:443"), "https://core.example:443");
    }
}
```

- [ ] **Step 2: 注册模块,跑测试确认失败→通过**

`interfaces/webchat/src/api.rs` 在模块声明区(`pub mod browser;` 附近)加一行:
```rust
pub mod tauri_bridge;
```
Run: `cargo test -p aleph-panel tauri_bridge -- --nocapture`
Expected: 5 个测试 PASS(纯函数已实现)。若编译报缺模块,确认 `api.rs` 已加 `pub mod tauri_bridge;`。

- [ ] **Step 3: 补 Tauri 绑定与 async 包装**(追加到 `tauri_bridge.rs` 顶部 `use` 与函数)

文件顶部加:
```rust
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// 是否运行在桌面 Tauri shell 内。
pub fn is_shell() -> bool {
    let Some(win) = web_sys::window() else {
        return false;
    };
    match js_sys::Reflect::get(&win, &JsValue::from_str("__TAURI__")) {
        Ok(v) => !v.is_undefined() && !v.is_null(),
        Err(_) => false,
    }
}

fn js_err(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

/// 当前连接目标:"local" 或远端 origin URL。
pub async fn get_connection_target() -> Result<String, String> {
    let v = tauri_invoke("get_connection_target", JsValue::NULL)
        .await
        .map_err(js_err)?;
    Ok(v.as_string().unwrap_or_default())
}

/// 切换 shell 连接目标。`raw` 接受 "local" / "host" / "host:port" /
/// "http(s)://host[:port]"。成功后 shell 会 reroute webview(本视图随之销毁)。
pub async fn set_connection_target(raw: &str) -> Result<(), String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("raw"), &JsValue::from_str(raw))
        .map_err(js_err)?;
    tauri_invoke("set_connection_target", args.into())
        .await
        .map_err(js_err)?;
    Ok(())
}

/// 重置为本地内置 core。
pub async fn clear_connection_target() -> Result<(), String> {
    tauri_invoke("clear_connection_target", JsValue::NULL)
        .await
        .map_err(js_err)?;
    Ok(())
}
```

- [ ] **Step 4: 编译 + 测试**

Run: `cargo check -p aleph-panel && cargo test -p aleph-panel tauri_bridge`
Expected: check 通过;5 测 PASS。
(注:若 host 链接器对 wasm-bindgen extern 报错——既有 webchat host 测试表明不会;若真发生,把 extern/async 包装移入 `#[cfg(target_arch = "wasm32")]`,纯函数与测试留 host。)

- [ ] **Step 5: 提交**
```bash
git add interfaces/webchat/src/api/tauri_bridge.rs interfaces/webchat/src/api.rs
git commit -m "panel: WASM→Tauri invoke bridge for shell connection switching"
```

---

## Task 2: 集群 API client(api/cluster.rs)

**Files:**
- Create: `interfaces/webchat/src/api/cluster.rs`
- Modify: `interfaces/webchat/src/api.rs`(加 `pub mod cluster;`)

- [ ] **Step 1: 写失败测试 + 类型 + client**

`interfaces/webchat/src/api/cluster.rs`:
```rust
//! 集群 API client。`environments.list`(已认证读)与 `cluster.enroll`
//! (operator-only)已在 main;node_invoke / deregister 属 phase 0c,未合 main。

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDescriptor {
    pub name: String,
    #[serde(default)]
    pub schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
    #[serde(default)]
    pub connected_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollResult {
    pub node_id: String,
    pub token: String,
    #[serde(default)]
    pub signature: String,
}

pub struct ClusterApi;

impl ClusterApi {
    /// 列出已连接节点(每 node = 一个 environment)。RPC `environments.list`。
    pub async fn list_environments(state: &DashboardState) -> Result<Vec<Environment>, String> {
        let result = state.rpc_call("environments.list", Value::Null).await?;
        result
            .get("environments")
            .ok_or_else(|| "Invalid response: missing environments".to_string())
            .and_then(|envs| {
                serde_json::from_value(envs.clone())
                    .map_err(|e| format!("Failed to parse environments: {e}"))
            })
    }

    /// 铸造 node 登记 token。RPC `cluster.enroll`(operator-only)。
    pub async fn enroll_node(
        state: &DashboardState,
        node_name: String,
    ) -> Result<EnrollResult, String> {
        let params = serde_json::json!({ "node_name": node_name });
        let result = state.rpc_call("cluster.enroll", params).await?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse enroll result: {e}"))
    }

    // --- phase 0c(未合 main)占位:node_invoke(node_id, command, params)
    //     与 deregister(node_id) 待 feat/cluster-phase0c-core 合并后接线。 ---
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_list() {
        let payload = serde_json::json!({
            "environments": [
                {"id":"n1","name":"node-a","status":"online",
                 "commands":[{"name":"bash","schema":{}}],"connected_at":1234}
            ]
        });
        let envs: Vec<Environment> =
            serde_json::from_value(payload.get("environments").unwrap().clone()).unwrap();
        assert_eq!(envs.len(), 1);
        assert_eq!(envs[0].name, "node-a");
        assert_eq!(envs[0].commands[0].name, "bash");
    }

    #[test]
    fn parses_enroll_result() {
        let payload = serde_json::json!({"node_id":"n1","token":"tok","signature":"sig"});
        let r: EnrollResult = serde_json::from_value(payload).unwrap();
        assert_eq!(r.token, "tok");
        assert_eq!(r.node_id, "n1");
    }
}
```

- [ ] **Step 2: 注册模块**

`interfaces/webchat/src/api.rs` 加:
```rust
pub mod cluster;
```

- [ ] **Step 3: 编译 + 测试**

Run: `cargo test -p aleph-panel cluster::tests`
Expected: 2 测 PASS。

- [ ] **Step 4: 提交**
```bash
git add interfaces/webchat/src/api/cluster.rs interfaces/webchat/src/api.rs
git commit -m "panel: cluster API client (environments.list + cluster.enroll)"
```

---

## Task 3: network 模块外壳 + 注册(views/settings/network/)

**Files:**
- Create: `interfaces/webchat/src/views/settings/network/mod.rs`
- Create: `interfaces/webchat/src/views/settings/network/connection.rs`(本任务最小占位)
- Create: `interfaces/webchat/src/views/settings/network/cluster.rs`(本任务最小占位)
- Modify: `interfaces/webchat/src/views/settings/mod.rs`

- [ ] **Step 1: 建最小可编译的三文件**

`network/connection.rs`:
```rust
use leptos::prelude::*;

#[component]
pub fn ConnectionSection() -> impl IntoView {
    view! { <section></section> }
}
```

`network/cluster.rs`:
```rust
use leptos::prelude::*;

#[component]
pub fn ClusterSection() -> impl IntoView {
    view! { <section></section> }
}
```

`network/mod.rs`:
```rust
//! Network 设置页 — 合并单页:
//!  · Section 1 上游连接(壳核分离连接切换,Feature A)
//!  · Section 2 下游集群(集群节点管理,Feature B 骨架)

mod cluster;
mod connection;

use cluster::ClusterSection;
use connection::ConnectionSection;
use leptos::prelude::*;

#[component]
pub fn NetworkView() -> impl IntoView {
    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-10">
            <h1 class="text-2xl font-bold text-text-primary">"网络与集群"</h1>
            <ConnectionSection />
            <ClusterSection />
        </div>
    }
}
```

- [ ] **Step 2: 在 settings/mod.rs 注册**

`interfaces/webchat/src/views/settings/mod.rs`:模块声明区(`pub mod memory;` 附近)加:
```rust
pub mod network;
```
re-export 区(`pub use memory::MemoryView;` 附近)加:
```rust
pub use network::NetworkView;
```

- [ ] **Step 3: 编译**

Run: `cargo check -p aleph-panel`
Expected: 通过(`NetworkView` 暂未路由,可能 dead_code 警告,无错)。

- [ ] **Step 4: 提交**
```bash
git add interfaces/webchat/src/views/settings/network/mod.rs interfaces/webchat/src/views/settings/network/connection.rs interfaces/webchat/src/views/settings/network/cluster.rs interfaces/webchat/src/views/settings/mod.rs
git commit -m "panel: NetworkView shell + two-section skeleton"
```

---

## Task 4: Section 1 — 上游连接(connection.rs,Feature A 完整)

**Files:**
- Modify: `interfaces/webchat/src/views/settings/network/connection.rs`(整体替换)

> UI 组件无法 host 单测;本任务验证 = `cargo check` 通过 + 后续手动 e2e。归一逻辑的测试已在 Task 1 覆盖。

- [ ] **Step 1: 整体替换 connection.rs**
```rust
//! Section 1 — 上游连接(Feature A):切换 shell 的 core 连接(本地/远程)。
//! 仅桌面 Tauri shell 内可交互;纯浏览器内只读降级。

use crate::api::tauri_bridge;
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn ConnectionSection() -> impl IntoView {
    let _state = expect_context::<DashboardState>();
    let in_shell = tauri_bridge::is_shell();

    let current = RwSignal::new(String::new());
    let remote_input = RwSignal::new(String::new());
    let use_remote = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let busy = RwSignal::new(false);
    let show_confirm = RwSignal::new(false);

    if in_shell {
        spawn_local(async move {
            if let Ok(t) = tauri_bridge::get_connection_target().await {
                let is_remote = t != "local";
                use_remote.set(is_remote);
                if is_remote {
                    remote_input.set(t.clone());
                }
                current.set(t);
            }
        });
    }

    let apply = move |_| {
        error.set(None);
        let raw = if use_remote.get() {
            remote_input.get()
        } else {
            "local".to_string()
        };
        busy.set(true);
        spawn_local(async move {
            match tauri_bridge::set_connection_target(&raw).await {
                // 成功后 shell reroute webview,本视图销毁
                Ok(()) => {}
                Err(e) => {
                    error.set(Some(e));
                    busy.set(false);
                    show_confirm.set(false);
                }
            }
        });
    };

    view! {
        <section class="space-y-4">
            <div>
                <h2 class="text-lg font-semibold text-text-primary mb-1">"上游连接"</h2>
                <p class="text-sm text-text-secondary">
                    "选择本 Panel 连接的 Aleph core(本地或远程)。"
                </p>
            </div>

            <Show
                when=move || in_shell
                fallback=move || view! {
                    <div class="bg-surface-raised rounded-lg border border-border p-6">
                        <p class="text-sm text-text-secondary">
                            "当前在浏览器中运行,连接切换仅在桌面 App 内可用。"
                        </p>
                    </div>
                }
            >
                <div class="bg-surface-raised rounded-lg border border-border p-6 space-y-4">
                    <label class="flex items-center gap-3">
                        <input type="radio" name="conn"
                            prop:checked=move || !use_remote.get()
                            on:change=move |_| use_remote.set(false) />
                        <span class="text-text-primary">"本地 Local"</span>
                    </label>
                    <label class="flex items-center gap-3">
                        <input type="radio" name="conn"
                            prop:checked=move || use_remote.get()
                            on:change=move |_| use_remote.set(true) />
                        <span class="text-text-primary">"远程 Remote"</span>
                    </label>

                    <Show when=move || use_remote.get()>
                        <input type="text"
                            placeholder="https://core.example:18790"
                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                            prop:value=move || remote_input.get()
                            on:input=move |ev| remote_input.set(event_target_value(&ev)) />
                        <p class="text-xs text-text-tertiary">
                            "预览:" {move || tauri_bridge::normalize_endpoint_preview(&remote_input.get())}
                        </p>
                    </Show>

                    <div class="flex items-center gap-3 pt-2">
                        <button
                            class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50"
                            prop:disabled=move || busy.get()
                            on:click=move |_| show_confirm.set(true)>
                            "应用"
                        </button>
                        <span class="text-xs text-text-tertiary">
                            "当前:" {move || current.get()}
                        </span>
                    </div>

                    {move || error.get().map(|e| view! { <p class="text-sm text-error">{e}</p> })}
                </div>
            </Show>

            <Show when=move || show_confirm.get()>
                <div class="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="bg-surface-raised rounded-lg border border-border p-6 max-w-md space-y-4">
                        <p class="text-text-primary">
                            "将切换到 "
                            {move || if use_remote.get() { remote_input.get() } else { "本地".to_string() }}
                            " 并重新加载 Panel,确认?"
                        </p>
                        <div class="flex justify-end gap-3">
                            <button class="px-3 py-2 text-text-secondary"
                                on:click=move |_| show_confirm.set(false)>"取消"</button>
                            <button class="px-4 py-2 bg-primary text-white rounded-lg"
                                prop:disabled=move || busy.get()
                                on:click=apply>"确认切换"</button>
                        </div>
                    </div>
                </div>
            </Show>
        </section>
    }
}
```

- [ ] **Step 2: 编译**

Run: `cargo check -p aleph-panel`
Expected: 通过。若 Leptos 0.8 对 `Show`/`event_target_value`/`prop:` 语法报错,对照 `interfaces/webchat/src/views/settings/execution.rs` 同款写法微调(本仓库即用这些 API)。

- [ ] **Step 3: 提交**
```bash
git add interfaces/webchat/src/views/settings/network/connection.rs
git commit -m "panel: connection section — local/remote core switch (shell-only, browser read-only)"
```

---

## Task 5: Section 2 — 下游集群(cluster.rs,Feature B 接线 + 占位)

**Files:**
- Modify: `interfaces/webchat/src/views/settings/network/cluster.rs`(整体替换)

> 验证 = `cargo check` + 手动 e2e。

- [ ] **Step 1: 整体替换 cluster.rs**
```rust
//! Section 2 — 下游集群(Feature B 骨架):列出节点 + Enroll。
//! Invoke / bash / deregister 待 feat/cluster-phase0c-core 合并(此处禁用占位)。

use crate::api::cluster::{ClusterApi, Environment, EnrollResult};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn ClusterSection() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let nodes = RwSignal::new(Vec::<Environment>::new());
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(true);
    let needs_operator = RwSignal::new(false);

    let show_enroll = RwSignal::new(false);
    let enroll_name = RwSignal::new(String::new());
    let enroll_result = RwSignal::new(Option::<EnrollResult>::None);
    let enroll_err = RwSignal::new(Option::<String>::None);

    let load = move || {
        spawn_local(async move {
            loading.set(true);
            match ClusterApi::list_environments(&state).await {
                Ok(list) => {
                    nodes.set(list);
                    error.set(None);
                }
                Err(e) => {
                    let el = e.to_lowercase();
                    if el.contains("operator") || el.contains("permission") || el.contains("unauth") {
                        needs_operator.set(true);
                    } else {
                        error.set(Some(e));
                    }
                }
            }
            loading.set(false);
        });
    };
    load();

    let submit_enroll = move |_| {
        let name = enroll_name.get();
        enroll_err.set(None);
        spawn_local(async move {
            match ClusterApi::enroll_node(&state, name).await {
                Ok(r) => enroll_result.set(Some(r)),
                Err(e) => enroll_err.set(Some(e)),
            }
        });
    };

    view! {
        <section class="space-y-4">
            <div class="flex items-center justify-between">
                <div>
                    <h2 class="text-lg font-semibold text-text-primary mb-1">"下游集群"</h2>
                    <p class="text-sm text-text-secondary">
                        "本 core 作为 center 登记并管理的 node 执行臂。"
                    </p>
                </div>
                <button class="px-4 py-2 bg-primary text-white rounded-lg disabled:opacity-50"
                    prop:disabled=move || needs_operator.get()
                    on:click=move |_| {
                        enroll_result.set(None);
                        enroll_err.set(None);
                        enroll_name.set(String::new());
                        show_enroll.set(true);
                    }>
                    "+ Enroll"
                </button>
            </div>

            <Show when=move || needs_operator.get()>
                <div class="bg-surface-raised rounded-lg border border-border p-6">
                    <p class="text-sm text-text-secondary">"集群管理需要 operator 权限。"</p>
                </div>
            </Show>

            <Show when=move || !needs_operator.get()>
                <div class="bg-surface-raised rounded-lg border border-border p-6">
                    <Show when=move || loading.get()>
                        <p class="text-text-secondary text-sm">"加载中…"</p>
                    </Show>
                    <Show when=move || !loading.get() && nodes.get().is_empty()>
                        <p class="text-text-secondary text-sm">"暂无已登记节点。"</p>
                    </Show>
                    <For each=move || nodes.get() key=|n| n.id.clone() let:node>
                        <div class="flex items-center justify-between py-3 border-b border-border last:border-0">
                            <div class="min-w-0">
                                <div class="text-text-primary font-medium">{node.name.clone()}</div>
                                <div class="text-xs text-text-tertiary font-mono">{node.id.clone()}</div>
                            </div>
                            <div class="flex items-center gap-4 text-xs text-text-secondary">
                                <span class="flex items-center gap-1">
                                    <span class="w-2 h-2 rounded-full bg-success inline-block"></span>
                                    {node.status.clone()}
                                </span>
                                <span>{node.commands.len()} " cmds"</span>
                                <button class="px-2 py-1 rounded border border-border opacity-40 cursor-not-allowed"
                                    disabled=true
                                    title="feat/cluster-phase0c-core 收尾后启用">"Invoke"</button>
                            </div>
                        </div>
                    </For>
                    {move || error.get().map(|e| view! { <p class="text-sm text-error mt-3">{e}</p> })}
                </div>
            </Show>

            <Show when=move || show_enroll.get()>
                <div class="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
                    <div class="bg-surface-raised rounded-lg border border-border p-6 max-w-md w-full space-y-4">
                        <h3 class="text-text-primary font-semibold">"登记新节点"</h3>
                        <Show
                            when=move || enroll_result.get().is_none()
                            fallback=move || {
                                let r = enroll_result.get().unwrap();
                                view! {
                                    <div class="space-y-2">
                                        <p class="text-sm text-text-secondary">"在目标机器上用此 token 加入:"</p>
                                        <textarea readonly=true rows="3"
                                            class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary font-mono text-xs"
                                            prop:value=r.token.clone()></textarea>
                                        <p class="text-xs text-text-tertiary">"node_id: " {r.node_id.clone()}</p>
                                    </div>
                                }
                            }
                        >
                            <input type="text" placeholder="node 名称"
                                class="w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-primary"
                                prop:value=move || enroll_name.get()
                                on:input=move |ev| enroll_name.set(event_target_value(&ev)) />
                            {move || enroll_err.get().map(|e| view! { <p class="text-sm text-error">{e}</p> })}
                        </Show>
                        <div class="flex justify-end gap-3">
                            <button class="px-3 py-2 text-text-secondary"
                                on:click=move |_| { show_enroll.set(false); load(); }>"关闭"</button>
                            <Show when=move || enroll_result.get().is_none()>
                                <button class="px-4 py-2 bg-primary text-white rounded-lg"
                                    on:click=submit_enroll>"生成 token"</button>
                            </Show>
                        </div>
                    </div>
                </div>
            </Show>
        </section>
    }
}
```

- [ ] **Step 2: 编译**

Run: `cargo check -p aleph-panel`
Expected: 通过。
注:`load` 闭包被立即调用且被关闭按钮再次调用——依赖 `DashboardState` 与所有 `RwSignal` 均为 `Copy`(本仓库 `execution.rs` 即在多个闭包复用 `state`)。若 `state` 报 move 错,改为在每个 `spawn_local` 前 `let state = state;`。

- [ ] **Step 3: 提交**
```bash
git add interfaces/webchat/src/views/settings/network/cluster.rs
git commit -m "panel: cluster section — node list + enroll (0c invoke stubbed)"
```

---

## Task 6: 侧栏 tab + 分组 + 路由(导航接线)

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs`
- Modify: `interfaces/webchat/src/app.rs`

- [ ] **Step 1: settings_sidebar.rs — 加 `Network` 变体**

`SettingsTab` enum 在 `// Advanced` 块尾(`Execution,` 之后)加:
```rust
    // Network
    Network,
```

`path()` match 加:
```rust
            Self::Network => "/settings/network",
```

`i18n_label()` match 加(沿用 Appearance/Browser/Execution 的硬编码先例,避免触碰编译期校验的 i18n JSON):
```rust
            Self::Network => "网络".to_string(),
```

`icon_svg()` match 加:
```rust
            Self::Network => {
                r#"<circle cx="5" cy="6" r="2"/><circle cx="5" cy="18" r="2"/><circle cx="19" cy="12" r="2"/><path d="M7 6h6a3 3 0 0 1 3 3v0M7 18h6a3 3 0 0 0 3-3v0"/>"#
            }
```

- [ ] **Step 2: settings_sidebar.rs — 新分组(Advanced 之后)**

`SETTINGS_GROUPS` 数组末尾(`Advanced` 分组之后)追加:
```rust
    SettingsGroup {
        label: "Network",
        tabs: &[SettingsTab::Network],
    },
```

`SettingsGroup::i18n_label()` 的 match,在 `other =>` 之前加:
```rust
            "Network" => "网络".to_string(),
```

- [ ] **Step 3: app.rs — 注册路由**

`interfaces/webchat/src/app.rs` 的 settings `match path.as_str()` 中(如 `"/settings/browser"` 行附近)加:
```rust
            "/settings/network" => view! { <NetworkView /> }.into_any(),
```
确认 `NetworkView` 已在该文件 use 范围内(settings views 通常 `use crate::views::settings::*;`;若非,加 `use crate::views::settings::NetworkView;`)。

- [ ] **Step 4: 编译**

Run: `cargo check -p aleph-panel`
Expected: 通过,无 dead_code 警告(NetworkView 现已被路由引用)。

- [ ] **Step 5: 提交**
```bash
git add interfaces/webchat/src/components/settings_sidebar.rs interfaces/webchat/src/app.rs
git commit -m "panel: wire Network settings tab + group + route"
```

---

## Task 7: 构建 WASM + 手动 e2e 验证

**Files:** 无(仅构建与验证)

- [ ] **Step 1: 构建 panel WASM + 重编 binary**

Run:
```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
```
Expected: 两步均成功。WASM 产物刷新 `interfaces/webchat/dist/*`,binary 经 `rust_embed` 烧入新 dist(参见 CLAUDE.md「Panel ↔ Daemon 资源嵌入链」)。

- [ ] **Step 2: 替换运行中 binary 并 relaunch**

按 CLAUDE.md dev 路径:`./target/release/aleph-server stop` → `cargo run --release -p alephcore --bin aleph-server start`(或 .app daemon 替换路径)。

- [ ] **Step 3: 手动核对清单**

侧栏:
- [ ] 「网络」分组出现在 Advanced 之后,点开进入 `/settings/network`,标题「网络与集群」。

Section 1(上游连接):
- [ ] 桌面 shell 内:显示 Local/Remote 单选;选 Remote 出输入框 + 实时「预览」串;「当前」显示 `get_connection_target` 值。
- [ ] 点「应用」出确认弹窗;确认后 Panel 重载并连到目标(切回 Local 可恢复)。
- [ ] 纯浏览器打开同页:只读提示「连接切换仅在桌面 App 内可用」,无单选控件。

Section 2(下游集群):
- [ ] operator 连接:显示节点列表(无节点时「暂无已登记节点」)。
- [ ] 「+ Enroll」输入名称→「生成 token」→出 token 文本框 + node_id;「关闭」后列表刷新。
- [ ] 非 operator(如 guest):显示「集群管理需要 operator 权限」,Enroll 禁用。
- [ ] 「Invoke」按钮禁用,hover 提示 0c。

- [ ] **Step 4: 提交(若验证中有微调)**
```bash
git add -A interfaces/webchat
git commit -m "panel: Network page e2e fixups"
```
(无改动则跳过。)

---

## Self-Review

**1. Spec 覆盖**
- IA(新分组 Network/Advanced 之后 + tab + 路由)→ Task 3/6 ✅
- A 完整(shell 内切换 / 浏览器只读 / Apply 重载确认 / 预览归一)→ Task 1/4 ✅
- B 骨架(list+enroll 接线 + invoke/deregister 占位 + operator 门)→ Task 2/5 ✅
- 纯 panel 改动(零 core/shell)→ 所有 Task 仅触 `interfaces/webchat/*` ✅
- 测试(纯函数 + 解析 host 单测;UI 手动 e2e)→ Task 1/2 单测 + Task 7 e2e ✅
- 非目标(无 token UI / 无 history / 无 deregister / 浏览器不可切)→ 设计落实,未引入 ✅

**2. 占位扫描**:无 TBD/「稍后实现」。0c 相关均为**有意的禁用占位**(spec 明确范围),非计划占位。

**3. 类型一致性**:`tauri_bridge::{is_shell, get/set/clear_connection_target, normalize_endpoint_preview}`、`ClusterApi::{list_environments, enroll_node}`、`Environment/CommandDescriptor/EnrollResult`、组件 `NetworkView/ConnectionSection/ClusterSection` —— 跨 Task 引用名称一致。

**风险点(已在步骤内标注降级路径)**:① wasm-bindgen extern 在 host 链接(Task 1 Step 4 备注);② Leptos 0.8 语法细节(Task 4 Step 2 对照 execution.rs);③ `DashboardState` Copy 假设(Task 5 Step 2 备注)。
