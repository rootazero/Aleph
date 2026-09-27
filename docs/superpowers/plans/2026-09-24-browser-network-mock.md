# browser_network Mock 路由（C1）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给既有 `browser_network` 工具加 mock 路由（mock_add/mock_list/mock_remove/mock_clear，abort 为特例），cdp 后端经手写 Fetch 域包装实现 per-tab 拦截环，SSRF 否决优先于一切规则命中。

**Architecture:** 匹配与决策全在 Aleph 进程内：aleph-cdp 新增 `methods::fetch` 手写包装；`cdp_backend/routes.rs` 新建 `RouteRegistry`（ProfileManager 持有、`CdpBackend` 持 Arc——后端每次调用重建，长命状态必须住 manager）+ per-tab `InterceptLoop` consumer task；工具面扩既有 `browser_network`（加 `action`，默认 `log` 行为不变）。obscura 行值以真机探针实测为准。

**Tech Stack:** Rust / tokio / aleph-cdp（手写 CDP 包装，base64 已是该 crate 依赖）/ FakeCdpServer 测试夹具。

**Spec:** `docs/superpowers/specs/2026-09-24-browser-network-mock-design.md`

## Global Constraints

- **唯一 CDP 真源**：Fetch 域只进 `crates/aleph-cdp/src/methods/fetch.rs` 手写包装（crate 明示拒绝 codegen）；禁第二份 CDP 客户端。
- **SSRF 否决优先于 mock 命中**：每个 `requestPaused` 先过 `BrowserSsrfGuard::check_url`，拒绝→`failRequest`；**SSRF 否决路径的失败绝不回退 continue**（回退=放行到内网）。内部故障（超时/解码错/task 崩溃）才 fail-open 到 continue。
- **fulfillRequest 的 wire body 是 base64**：编码只发生在 `methods::fetch::fulfill_request` 内，签名收 `&[u8]`——没有调用者能发出未编码字节。
- **零规则零开销**：`Fetch.enable` 只在某 tab 有生效规则时下发；规则清空即 `Fetch.disable`；InterceptLoop 惰性启动。
- **R9 字节纪律**：`browser_network` 的 DESCRIPTION 改动受既有字节守卫约束（参考 `capability.rs:23` 提到的 emulate DESCRIPTION guard）；语义细节走 JsonSchema doc comment。
- **R8**：规则匹配用 substring（`url_contains`），禁 regex。
- **能力台账**：恢复 `network_interception` 行（`src/browser/engine/capability.rs:548` 与 `:1090` 两处防护测试同步更新）；行值 chromium=Supported、obscura 先 `Unsupported`（注释 NOT_PROBED），Task 4 实测后翻转或维持；doc comment 点名 `browser_network` 的四个 mock action。
- **内存守卫**：每次 cargo 前 `awk '/MemAvailable/{exit ($2<4194304)?1:0}' /proc/meminfo`（失败 sleep 60 重查）；`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`。
- **TDD + 每条守卫先证伪**（变异→红→恢复）；英文 commit `<scope>: <description>`。
- 已知既存红（非回归信号）：capability census 差1（main 已知）、skill_doc_drift×2、extension validation、btw_wire、windows_separators、extension_stop_gate（后六条 worktree 路径环境性）。
- 工作区：`/home/zou/data/workspace/Aleph-browser-r2`，分支 `browser-native-r2`。禁碰其他检出。

## Review Focus

1. **SSRF 否决应答失败后的回退方向**：`failRequest` 失败若回退 `continueRequest`，等于把请求放行到内网——SSRF 路径只允许重试 fail 或放弃（页面挂死可接受，放行不可接受）。→ Task 2 Step 6 的对抗测试钉住。
2. **决策途中 tab 死亡**：`continueRequest` 打向已死 session 报错——loop 必须识别 session 死亡并退出自洁，不许错误自旋。→ Task 2 Step 7。
3. **规则抖动（有→无→有）**：`Fetch.disable` 后到达的 orphan `requestPaused`（引擎侧已排队）应答会报错——必须容忍不升级为故障。→ Task 2 Step 8。
4. **`url_contains` 空串匹配一切**：输入侧校验拒绝空串（空串规则=全站拦截，几乎必是模型笔误）。→ Task 3 Step 2。
5. **profile 级规则与 tab 首请求的竞态**：规则重放必须发生在新 tab 的首次导航之前，否则首请求漏拦。→ Task 2 Step 9（FakeCdpServer 钉 Fetch.enable 先于 Page.navigate 到达引擎）。

---

### Task 1: aleph-cdp Fetch 域包装

**Files:**
- Create: `crates/aleph-cdp/src/methods/fetch.rs`
- Modify: `crates/aleph-cdp/src/methods/mod.rs`（挂 `pub mod fetch;`）
- Test: `crates/aleph-cdp/tests/methods.rs`

**Interfaces:**
- Consumes: `CdpConnection::call(session, method, params)`（connection.rs:366）、`super::{decode, field}`、`crate::ids::SessionId`、`base64`（已是 crate 依赖，见 Cargo.toml:28 注释）
- Produces（Task 2 消费这些精确签名）:
  - `pub struct RequestPaused { pub request_id: String, pub request: PausedRequest, pub resource_type: Option<String> }`
  - `pub struct PausedRequest { pub url: String, pub method: String }`
  - `pub fn request_paused(params: &serde_json::Value) -> Result<RequestPaused>`
  - `pub async fn enable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()>`
  - `pub async fn disable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()>`
  - `pub async fn continue_request(conn: &CdpConnection, session: Option<&SessionId>, request_id: &str) -> Result<()>`
  - `pub async fn fulfill_request(conn: &CdpConnection, session: Option<&SessionId>, request_id: &str, status: u16, headers: &[(String, String)], body: &[u8]) -> Result<()>`
  - `pub enum FailReason { Failed, BlockedByClient }` + `pub async fn fail_request(conn, session, request_id, reason: FailReason) -> Result<()>`

- [ ] **Step 1: 写失败测试**（追加到 `crates/aleph-cdp/tests/methods.rs`，沿用 `replying()`/`scripted()` 既有形状）

```rust
#[tokio::test]
async fn fetch_fulfill_request_base64_encodes_the_body_on_the_wire() {
    // Review Focus #2: the wire body MUST be base64 — a caller that sends raw
    // bytes gets a silently garbled page. The server asserts the encoded shape.
    let (server, conn) = replying("Fetch.fulfillRequest", json!({})).await;
    fetch::fulfill_request(
        &conn, None, "r1", 200, &[("content-type".into(), "application/json".into())], b"{}",
    ).await.expect("fulfill");
    let sent = server.last_frame().await; // FakeCdpServer 的帧检查能力按 testkit 既有 API；若无 last_frame，用 Responder 闭包断言
    let params = sent.get("params").cloned().unwrap();
    assert_eq!(params["body"], "e30="); // base64("{}")
    assert_eq!(params["responseCode"], 200);
    assert_eq!(params["responseHeaders"], json!([{"name":"content-type","value":"application/json"}]));
}

#[tokio::test]
async fn fetch_request_paused_decodes_event_params() {
    let paused = fetch::request_paused(&json!({
        "requestId": "r9", "resourceType": "XHR",
        "request": {"url": "https://a.test/api", "method": "POST"}
    })).expect("decode");
    assert_eq!(paused.request_id, "r9");
    assert_eq!(paused.request.url, "https://a.test/api");
    assert_eq!(paused.request.method, "POST");
}

#[tokio::test]
async fn fetch_enable_sends_a_catch_all_pattern() {
    let (server, conn) = replying("Fetch.enable", json!({})).await;
    fetch::enable(&conn, None).await.expect("enable");
    // 粗 patterns 是刻意的（spec §2：匹配在 Aleph 进程内做，引擎语义差异不进正确性面）
    // —— 钉住 `[{"urlPattern":"*"}]` 这一形状。
}

#[tokio::test]
async fn fetch_fail_request_maps_reasons() {
    // FailReason::BlockedByClient → "BlockedByClient"；Failed → "Failed"。两个臂各钉一次。
}
```

- [ ] **Step 2: 跑测试确认红**

Run: `awk '/MemAvailable/{exit ($2<4194304)?1:0}' /proc/meminfo && CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p aleph-cdp --test methods fetch 2>&1 | tail -5`
Expected: FAIL（`fetch` 模块不存在，编译错误）

- [ ] **Step 3: 实现 `methods/fetch.rs`**

```rust
//! `Fetch.*` — request interception for `browser_network`'s mock routes.
//!
//! Deliberately coarse at the engine: `enable` sends a single catch-all
//! pattern and ALL matching happens in Aleph (spec §2), so an engine whose
//! pattern grammar differs (obscura, unprobed) only changes how many events
//! arrive, never what the decision is.

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};

use super::decode;
use crate::connection::CdpConnection;
use crate::error::Result;
use crate::ids::SessionId;

/// One `Fetch.requestPaused` event, decoded. Fields beyond these (headers,
/// frameId, networkId…) are carried on the wire but not modeled — the
/// decision pipeline needs exactly url/method (判据 §8: 不建模的字段不假装存在)。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestPaused {
    pub request_id: String,
    pub request: PausedRequest,
    #[serde(default)]
    pub resource_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PausedRequest {
    pub url: String,
    pub method: String,
}

/// Decode the params of a `Fetch.requestPaused` event.
pub fn request_paused(params: &Value) -> Result<RequestPaused> {
    decode("Fetch.requestPaused", params.clone())
}

/// Arm interception with a single catch-all pattern (see module doc).
pub async fn enable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Fetch.enable", json!({ "patterns": [{ "urlPattern": "*" }] })).await?;
    Ok(())
}

/// Disarm. Orphan `requestPaused` events already queued engine-side may still
/// arrive afterwards — answering them errors, and that error is NOT a fault
/// (Review Focus #3).
pub async fn disable(conn: &CdpConnection, session: Option<&SessionId>) -> Result<()> {
    conn.call(session, "Fetch.disable", json!({})).await?;
    Ok(())
}

pub async fn continue_request(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    request_id: &str,
) -> Result<()> {
    conn.call(session, "Fetch.continueRequest", json!({ "requestId": request_id })).await?;
    Ok(())
}

/// Answer a paused request with a synthetic response. `body` is RAW bytes;
/// the base64 the wire wants is applied HERE and nowhere else, so no caller
/// can forget it (Review Focus #2).
pub async fn fulfill_request(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    request_id: &str,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<()> {
    let headers: Vec<Value> = headers
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    conn.call(
        session,
        "Fetch.fulfillRequest",
        json!({
            "requestId": request_id,
            "responseCode": status,
            "responseHeaders": headers,
            "body": base64::engine::general_purpose::STANDARD.encode(body),
        }),
    )
    .await?;
    Ok(())
}

/// The two reasons Aleph ever fails a request: a mock `abort` rule (`Failed`)
/// or the SSRF guard (`BlockedByClient` — the browser's own refusal wording,
/// so the page sees the same failure shape as a real block).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FailReason {
    Failed,
    BlockedByClient,
}

pub async fn fail_request(
    conn: &CdpConnection,
    session: Option<&SessionId>,
    request_id: &str,
    reason: FailReason,
) -> Result<()> {
    let reason = match reason {
        FailReason::Failed => "Failed",
        FailReason::BlockedByClient => "BlockedByClient",
    };
    conn.call(session, "Fetch.failRequest", json!({ "requestId": request_id, "errorReason": reason }))
        .await?;
    Ok(())
}
```

`methods/mod.rs` 加 `pub mod fetch;`（按字母序挂到既有模块清单里）。

- [ ] **Step 4: 跑测试确认绿**

Run: `CARGO_BUILD_JOBS=2 cargo test -p aleph-cdp --test methods fetch 2>&1 | tail -5`
Expected: 4 条新测试 PASS

- [ ] **Step 5: 证伪 + commit**

证伪：把 `fulfill_request` 的 `.encode(body)` 临时改成 `String::from_utf8_lossy(body).into_owned()` → Step 1 测试红 → 恢复。

```bash
git add crates/aleph-cdp/src/methods/fetch.rs crates/aleph-cdp/src/methods/mod.rs crates/aleph-cdp/tests/methods.rs
git commit -m "aleph-cdp: hand-written Fetch domain wrappers for request interception"
```

---

### Task 2: RouteRegistry + InterceptLoop（cdp 后端拦截环）

**Files:**
- Create: `src/browser/cdp_backend/routes.rs`
- Modify: `src/browser/cdp_backend/mod.rs`（挂 `pub(super) mod routes;` + CdpBackend 持 `routes: Arc<RouteRegistry>` 字段）
- Modify: `src/browser/manager.rs`（ProfileManager 持 `route_registry: Arc<RouteRegistry>`，与 `tab_registry`（manager.rs:135/305）同构；`route_registry()` 访问器仿 :566）
- Test: `src/browser/cdp_backend/routes.rs` 的 `#[cfg(test)]` 模块（纯函数半）+ FakeCdpServer 集成半

**Interfaces:**
- Consumes: Task 1 的 `fetch::*` 签名；`BrowserSsrfGuard::check_url(&self, url: &str) -> Result<(), PolicyViolation>`（network_policy.rs:193）；`CdpConnection::events() -> EventStream`（connection.rs:493，订阅须在 `Fetch.enable` 之前——events.rs 模块doc：「需要由调用引起的事件的调用者先取流」）；`EngineHandle`/`TabEntry`/`TabTable`（engine/mod.rs，事件泵 events.rs 的既有先例）；`CdpBackend` 的 `ssrf_guard: Arc<BrowserSsrfGuard>`（cdp_backend/mod.rs:83）与 `command_timeout`（:84）
- Produces（Task 3 消费）:
  - `pub struct RouteRule { id: String, url_contains: String, method: Option<String>, kind: RouteKind, scope: RouteScope, note: Option<String> }`（hits/active 为内部原子量，读出经 `RouteRuleInfo`）
  - `pub enum RouteKind { Mock { status: u16, headers: Vec<(String, String)>, body: Vec<u8> }, Abort }`
  - `pub enum RouteScope { Tab, Profile }`
  - `pub struct RouteRuleInfo { pub id: String, pub url_contains: String, pub method: Option<String>, pub kind_label: &'static str, pub scope: RouteScope, pub hits: u64, pub active: bool }`
  - `pub struct MatchedRoute { pub info: RouteRuleInfo, pub kind: RouteKind }`（InterceptLoop 用——命中判定与判决材料一起走；命中计数在 `match_lifo` 内 +1）
  - `impl RouteRegistry`: `add(tab_id, url_contains, method, kind, scope, note) -> RouteRuleInfo`、`remove(id) -> Option<RouteRuleInfo>`、`clear(scope_filter) -> usize`、`list() -> Vec<RouteRuleInfo>`、`pub(crate) fn match_lifo(tab_id, url, method) -> Option<MatchedRoute>`
  - `CdpBackend` 内部：`ensure_intercept_loop(&self, tab_id, session) `（幂等，惰性启动）；规则清空时 `stop_intercept_loop_if_idle(tab_id)`

- [ ] **Step 1: 纯函数半的失败测试**（routes.rs `#[cfg(test)]`）

```rust
// 测试助手（本模块内定义）：
fn mock_json(status: u16, tag: &str) -> RouteKind {
    RouteKind::Mock { status, headers: vec![("content-type".into(), "application/json".into())],
                      body: format!("\"{tag}\"").into_bytes() }
}

#[test]
fn matching_is_lifo_and_tab_rules_beat_profile_rules() {
    let mut reg = RouteRegistry::new();
    reg.add("t1", "api", None, mock_json(200, "profile-old"), RouteScope::Profile, None);
    reg.add("t1", "api", None, mock_json(200, "tab-new"), RouteScope::Tab, None);
    reg.add("t1", "api", None, mock_json(200, "tab-newer"), RouteScope::Tab, None);
    let hit = reg.match_lifo("t1", "https://a.test/api/x", "GET").expect("hit");
    assert_eq!(hit.info.id, "r3"); // LIFO：最后注册的先命中（r1=profile-old, r2=tab-new, r3=tab-newer）
    assert!(matches!(hit.kind, RouteKind::Mock { .. }));
    // 删掉 r3 后轮到 r2；两个 tab 规则都删掉才轮到 profile-old（r1）
}

#[test]
fn method_filter_narrows_and_substring_matches_anywhere() {
    // method: Some("POST") 的规则不命中 GET；url_contains 是 substring 不是锚定（R8）
}

#[test]
fn hits_are_counted_on_the_rule_that_matched() { /* 命中计数 +1，未命中规则不变 */ }
```

- [ ] **Step 2: 跑确认红** → `cargo test -p alephcore --lib browser::cdp_backend::routes 2>&1 | tail -3`（编译错即红）

- [ ] **Step 3: 实现 RouteRegistry 纯函数半**

```rust
//! Mock routes and the per-tab interception loop (spec §2/§3).
//!
//! The registry outlives any `CdpBackend` (backends are rebuilt per call), so
//! it lives on `ProfileManager` next to `TabRegistry` and the backend carries
//! an `Arc` — the same reason `tab_identities` does (mod.rs:85-91).

pub struct RouteRegistry {
    rules: std::sync::Mutex<Vec<RouteRule>>,   // 注册序即 Vec 序；LIFO = 从尾扫
    counter: std::sync::atomic::AtomicU64,     // r1, r2… 单调不复用（spec §4）
    loops: std::sync::Mutex<std::collections::HashMap<String, tokio::task::AbortHandle>>,
}

impl RouteRegistry {
    /// tab 规则先于 profile 规则查，各自内部 LIFO。命中即在锁内给该规则 hits+1。
    pub(crate) fn match_lifo(&self, tab_id: &str, url: &str, method: &str) -> Option<MatchedRoute> { /* 尾扫两遍：先 scope==Tab&&tab_id 匹配，再 scope==Profile */ }
    // add/remove/clear/list 按 Produces 签名；add 时 url_contains 空串拒绝（Review Focus #4
    // 在 Task 3 输入层再拦一次——纵深，不是重复）
}
```

- [ ] **Step 4: 纯函数半绿 + commit `browser: mock route registry with LIFO tab-over-profile matching`**

- [ ] **Step 5: InterceptLoop 的失败集成测试**（FakeCdpServer 驱动真 task——先例：`crates/aleph-cdp/tests/methods.rs` 的 `scripted` 只能应答不能推事件；若 FakeCdpServer 不支持服务端主动推事件，给它加一个 `push_event(method, params, session)` responder 动作，这是夹具的正当扩展）

```rust
#[tokio::test]
async fn a_paused_request_matching_no_rule_is_continued_verbatim() { /* 推 requestPaused(api 不命中) → 断言引擎收到 continueRequest */ }

#[tokio::test]
async fn a_paused_request_matching_a_mock_rule_is_fulfilled() { /* 断言 fulfillRequest 且 body 是 mock 字节的 base64 */ }

#[tokio::test]
async fn ssrf_veto_beats_a_matching_mock_rule() {
    // Review Focus #1 对抗用例：规则命中 http://169.254.169.254/latest，
    // SSRF guard 拒绝 → 引擎收到 failRequest(BlockedByClient)，永远收不到 fulfillRequest
}

#[tokio::test]
async fn an_ssrf_veto_never_falls_back_to_continue() {
    // Review Focus #1 的后半：failRequest 第一次应答报错（注入一次）→ 重试 fail 或放弃，
    // 断言引擎从未收到 continueRequest（放行到内网是唯一不可接受的结局）
}
```

- [ ] **Step 6: 实现 InterceptLoop**

```rust
/// Per-tab consumer: subscribe BEFORE `Fetch.enable` (the connection's
/// broadcast starts at subscription), then decide every paused request in
/// strict order — SSRF first, rules second (spec §3).
async fn run_loop(
    conn: Arc<CdpConnection>, session: SessionId, tab_id: String,
    registry: Arc<RouteRegistry>, ssrf: Arc<BrowserSsrfGuard>, timeout: Duration,
) {
    let mut events = conn.events();
    // Fetch.enable 由启动者（ensure_intercept_loop）在 spawn 前下发；失败则不 spawn。
    let mut last_lagged = 0u64;
    let mut lag_strikes = 0u32;
    while let Some(ev) = events.next().await {
        let lagged = events.lagged();
        if lagged > last_lagged {
            lag_strikes += 1;
            tracing::warn!(tab_id, lagged, "intercept loop lagged — some requests were auto-lost");
            // spec §3 说「对积压 requestId 批量 continue 排干」——**本计划的有意偏离**：
            // broadcast lagged 时事件本体已丢，requestId 无从知晓，「排干」不可行。
            // 诚实行为 = 审计警告 + 不假装排干；被丢的请求由引擎侧自身超时收场。
            if lag_strikes >= 3 { /* audit::log 一次，节流 */ }
            last_lagged = lagged;
        }
        if ev.method != "Fetch.requestPaused" || ev.session.as_ref() != Some(&session) { continue; }
        let paused = match fetch::request_paused(&ev.params) {
            Ok(p) => p,
            Err(_) => { continue; } // 解码错：不应答（引擎侧超时自然失败），记 debug
        };
        // ① SSRF —— 无条件先跑；否决路径绝不回退 continue（Review Focus #1）
        if ssrf.check_url(&paused.request.url).await.is_err() {
            for _ in 0..2 {
                if fetch::fail_request(&conn, Some(&session), &paused.request_id,
                                       fetch::FailReason::BlockedByClient).await.is_ok() { break; }
            }
            // 两次都失败：放弃应答。页面挂死可接受，放行不可接受。
            continue;
        }
        // ② 规则
        match registry.match_lifo(&tab_id, &paused.request.url, &paused.request.method) {
            None => {
                let _ = fetch::continue_request(&conn, Some(&session), &paused.request_id).await;
                // 应答失败（tab 死于决策途中 / orphan after disable）→ 退出条件见下，单条不升级
            }
            Some(rule) => { /* fulfill 或 fail(Failed)；body 来自 RouteKind::Mock */ }
        }
        // session 死亡检测：continue/fulfill 连续报 Disconnected/会话未知 → break 自洁（Review Focus #2）
    }
    // 流 Closed（连接断）→ 自然退出；registry.loops 自清
}
```

- [ ] **Step 7: Review Focus #2 测试**（决策途中 tab 死亡）

```rust
#[tokio::test]
async fn the_loop_exits_when_its_session_dies_mid_decision() {
    // 推 requestPaused → FakeCdpServer 对 continueRequest 回 session-unknown 错 ×N
    // → 断言 loop task 在 N 次内退出（JoinHandle 完成），且 registry.loops 里该 tab 条目被清
}
```

- [ ] **Step 8: Review Focus #3 测试**（orphan 应答容忍）

```rust
#[tokio::test]
async fn an_orphan_paused_event_after_disable_is_tolerated() {
    // disable 后推一个 requestPaused → 应答报错 → loop 仍活着处理下一条事件，不崩溃不退出
}
```

- [ ] **Step 9: Review Focus #5 测试 + 惰性启停**

```rust
#[tokio::test]
async fn profile_rules_are_armed_before_the_tabs_first_navigation() {
    // FakeCdpServer 钉帧序：ensure_tab（带 profile 规则）后，Fetch.enable 必须先于
    // 该 tab 的第一个 Page.navigate 到达引擎
}

#[tokio::test]
async fn zero_rules_means_no_fetch_enable_and_no_loop() {
    // 零规则时 ensure_tab 后引擎从未收到 Fetch.enable（spec §2 零开销条款）
}
```

实现要点：`ensure_intercept_loop` 在 cdp 后端的 tab 接入点（`ensure_tab`/attach 完成处）被调用，幂等（`loops` 里有条目即返回）；`clear`/`remove` 使某 tab 规则归零时 `Fetch.disable` + abort loop；profile 级规则变化时对该 profile 所有活 tab 重放 enable/disable。

- [ ] **Step 10: 全绿 + 证伪（每条守卫变异一次：删 SSRF 优先序 → Step 5 对抗用例红；删 session 死亡退出 → Step 7 红；删 enable 先于 navigate → Step 9 红）+ commit**

```bash
git commit -m "browser: per-tab interception loop with SSRF-first mock route pipeline"
```

---

### Task 3: browser_network 工具扩展 + 台账恢复

**Files:**
- Modify: `src/builtin_tools/browser_tools/network.rs`（加 action 枚举与四个 mock 分支）
- Modify: `src/browser/backend.rs`（trait 默认方法 ×4，紧挨 `network_log`（:163-166））
- Modify: `src/browser/cdp_backend/mod.rs`（覆盖四个方法，委托 RouteRegistry）
- Modify: `src/browser/engine/capability.rs`（恢复 `network_interception` 行 + :548/:1090 两处防护测试更新）
- Modify: `src/builtin_tools/browser_tools/recovery.rs`（若有新错误面；classify 穷尽 match 编译器会强制）
- Modify: `src/approval`（ActionType 新变体，仿 click.rs:138/265 模式）+ 相关 census
- Test: network.rs `#[cfg(test)]` + capability.rs 防护测试

**Interfaces:**
- Consumes: Task 2 的 `RouteRegistry`（经 `manager.route_registry()` 与 backend 的 Arc）与 `RouteRuleInfo`；`make_backend_and_tab_guarded`（mod.rs:429）；`check_browser_approval`（mod.rs:56）
- Produces: `browser_network` 的 `NetworkAction::{Log, MockAdd, MockList, MockRemove, MockClear}`；trait `route_add/route_list/route_remove/route_clear`；`network_interception` 能力行

- [ ] **Step 1: trait 默认方法 + 失败测试**

```rust
// backend.rs，紧挨 network_log：
/// Mock routes (spec: browser-network-mock §4). The default is the honest
/// asymmetry: text-driven backends have no Fetch handshake to serve it.
async fn route_add(&self, tab_id: &str, rule: NewRouteRule) -> Result<RouteRuleInfo, BrowserError> {
    let _ = (tab_id, rule);
    Err(unsupported_by_driver("route_add", BrowserDriver::Managed))
    // ^ backend.rs 既有惯用法（pdf 的默认臂同款，:176）：`unsupported_by_driver(verb,
    //   BrowserDriver)` 自由函数，点名能服务该动词的驱动。
}
// route_list/route_remove/route_clear 同形
```

测试（testkit FakeBackend 走默认臂 → `UnsupportedByDriver`；cdp 后端覆盖臂的判决委托 RouteRegistry）。

- [ ] **Step 2: 工具输入校验的失败测试**

```rust
#[tokio::test]
async fn mock_add_rejects_an_empty_url_contains() {
    // Review Focus #4：空串规则=全站拦截，几乎必是笔误——输入侧拒绝，不进注册表
}

#[tokio::test]
async fn mock_add_rejects_a_body_over_256_kib() { /* spec §4 上限 */ }

#[tokio::test]
async fn mock_clear_requires_an_explicit_scope() { /* 防误清：scope 必填，无默认 */ }
```

- [ ] **Step 3: 实现工具分支**（`BrowserNetworkArgs` 加 `#[serde(default)] action: NetworkAction`，默认 `Log`——现状调用零变化；输出结构加 `rules: Option<Vec<RouteRuleInfo>>`；四个 mock 分支经 `make_backend_and_tab_guarded` → `backend.route_*`；错误走 `backend_error_text` 咽喉自动继承 recovery 拖车；approval：`ActionType::BrowserNetworkMock`（或按 ActionType 家族现状命名）+ `check_browser_approval` 调用点 + 默认 Deny 的测试默认值（click.rs:265 先例）+ 若有 ActionType 穷尽 census 同步）

- [ ] **Step 4: 能力台账恢复**

`capability.rs`：`EngineCapabilities` 加 `pub network_interception: Cap` 字段（doc comment 点名 `browser_network` 的 mock_add/mock_list/mock_remove/mock_clear 四个 action——判据 §5）；`CAP_FIELDS` 加行；OBSCURA 行 = `Cap::Unsupported`（注释 NOT_PROBED + Task 4 是到期检查）；CHROMIUM 行 = `Cap::Supported`（依据：Fetch 域五方法全链路 Task 1/2 建立在 console/evaluate 已实测的同一连接与事件泵上）；:548 与 :1090 两处防护测试反转断言（行存在且点名动词）。

- [ ] **Step 5: 跑 + 证伪 + commit**

Run: `cargo test -p alephcore --lib browser_tools::network` + `--lib capability` + `--lib browser::backend`
证伪：`CAP_FIELDS` 摘行 → 三条 census 红（T6 先例）；DESCRIPTION 超字节 → 字节守卫红（存在的话）。
```bash
git commit -m "browser: browser_network gains mock routes; network_interception capability row restored"
```

---

### Task 4: obscura 真机探针 + caps 行 + 文档收尾

**Files:**
- Create: `probes/fetch-probe.mjs`（或 qa/browser_dual/ 内合适位置——先看 `probes/` 既有惯例，t0-capture.mjs 是先例）
- Modify: `qa/browser_dual/caps.py`（加 `network_interception` 行探测）
- Modify: `src/browser/engine/capability.rs`（obscura 行按实测翻转或维持，注释写测量日期/方法/版本）
- Modify: `docs/reference/FEATURE_LOCATOR.md` §3.12（Round 2 段）+ `qa/README.md`

- [ ] **Step 1: obscura Fetch 探针**（本机已装 v0.2.2，`~/.aleph/runtimes/obscura/v0.2.2/obscura`）：raw CDP 连真 obscura → `Fetch.enable`（patterns `*`）→ `Page.navigate` 一个本地页 → 断言 `requestPaused` 事件到达 → `fulfillRequest` 应答 → 断言页面拿到 mock body。**三个结局都如实记**：全通→行翻 Supported；enable 报错→Unsupported（启用路径不存在）；事件不到达→Unsupported（noop）。
- [ ] **Step 2: 按实测更新 capability.rs obscura 行**（注释：「Measured on v0.2.2, 2026-09-24, probes/fetch-probe.mjs」）；`cargo test -p alephcore --lib capability` 绿（census 钉值测试同步）
- [ ] **Step 3: caps.py 加 `network_interception` 行探测**（raw CDP 半：Fetch.enable→navigate→requestPaused 到达即 supported；加入 `found` 使 `every claimed row was probed` 覆盖新行）+ `./qa/browser_dual/run.sh caps` 真机跑绿；**证伪**：临时翻转表值 → diff 红
- [ ] **Step 4: 文档**：FL §3.12 加 Round 2 (C1) 段（落地清单、SSRF 优先序红线、决策管线、obscura 实测裁决、刻意不做——HAR/二进制/请求改写/exec 步骤/持久化）；qa/README.md 更新；勾选本计划 checkbox
- [ ] **Step 5: 全量验证集**（七条命令同 Round 1 T8：`--lib --no-run`、`--bins`、`--features test-helpers --test '*' --no-run`、`-p aleph-panel --lib --no-run`、`just test-shared`、clippy workspace、`--lib` 全量；基线 19475 passed/7 既存红，集合不许变大）+ commit `docs: browser network mock — locator entry, caps probe, plan ledger`

---

## 合并与验收

单线单分支（`browser-native-r2`）。检查点：Task 2 后跑一次 `--lib --no-run` + `--lib browser`；Task 4 后全量七条。合并回 main 前确认：无新增红、clippy 净、caps 真机绿。
