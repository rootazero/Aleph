# Browser 配对档位化 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 远程 `/pair` 浏览器配对批准后，铸一个 **chat 档持久设备** 并把其设备 token 投递进浏览器 `localStorage["aleph_device_token"]`，使 Panel 经 connect Case 1 落 role=`guest`。Panel 与 connect.rs 零改动。

**Architecture:** 后端 `pairing.rs` Browser 分支复用 Device 分支的档位铸造路径（强制 Chat）；PairingManager 的浏览器侧表从存 session_id 改存 `(token, device_id)`，`pairing.poll` 首次 approved 即 drain 返回 token（单次发放）；内嵌 `/pair` HTML 把 token 存 localStorage 后跳 `/`。由此 cookie 路径 `fetch_browser_session` + `/auth/bootstrap/from_pairing` 变死代码，确认后清理。

**Tech Stack:** Rust（`src/gateway/security/pairing.rs`、`src/gateway/handlers/auth/pairing.rs`、`src/gateway/auth_middleware.rs`）。Leptos Panel 零改动；`connect.rs` 零改动。

**Spec:** `docs/superpowers/specs/2026-06-08-browser-pairing-tiering-design.md`

**Git 约束（全程）:** 共享单分支 main + 并发提交者——只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；提交信息英文、无 attribution footer；不 push；提交前 `git status` 确认不卷入他人 WIP（工作区有 `interfaces/webchat/dist/*` 产物未暂存，勿 staged）。

---

## File Structure

- `src/gateway/security/pairing.rs` — `PollState::Approved` 改携 `(token, device_id)`；侧表 `approved_browser_sessions` 改存凭证；`record_browser_session`→`record_browser_credential`；`poll_browser_pairing` approved 分支改为 drain 并返回凭证；删 `fetch_browser_session`；更新该文件内单测。
- `src/gateway/handlers/auth/pairing.rs` — Browser 审批分支（:83-142）重写为 chat 档设备铸造；`handle_pairing_poll`（:472-495）approved 返回 `token`+`device_id`；更新 handler 测试 + 新增端到端 connect-as-guest 测试。
- `src/gateway/auth_middleware.rs` — `/pair` 内嵌 HTML 的 poll 回调改为存 localStorage + 跳 `/`；删死代码 `handle_bootstrap_from_pairing` 路由/handler/测试（grep 确认无其它消费者后）。

任务顺序：Task 1（PairingManager 凭证化，基础层）→ Task 2（handler 铸设备 + poll 返回 token + e2e 测试）→ Task 3（/pair HTML + 清理死 cookie 路径）。Task 2 依赖 Task 1 的 `record_browser_credential` 与 `PollState`；Task 3 依赖 Task 2 的 poll 返回 token。

> **行号为快照**：以实现时实际文件为准。

---

### Task 1: PairingManager 浏览器凭证化（存 token，poll 单次 drain）

**Files:**
- Modify: `src/gateway/security/pairing.rs`（`PollState` :160-172、侧表字段 :179-185、`record_browser_session` :410-416、`fetch_browser_session` :418-427、`poll_browser_pairing` :441-459、单测 :720-760）

- [ ] **Step 1: 改 `PollState::Approved` 携带凭证**

把 `src/gateway/security/pairing.rs:160-172` 的 enum 改为：
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollState {
    /// Pairing record still exists in the store and has not been approved.
    Pending,
    /// `pairing.approve` ran; `record_browser_credential` deposited the
    /// chat-tier device token + device_id keyed by the pairing code. The
    /// browser stores the token in `localStorage["aleph_device_token"]` and
    /// connects via Case 1. Single-use: drained on first approved poll.
    Approved { token: String, device_id: String },
    /// `pairing.reject` ran or the record was cancelled by the operator.
    Rejected,
    /// Record was never created, has expired, or the approval TTL elapsed
    /// before the browser polled.
    Expired,
}
```
同时把 :155-159 的 doc-comment 里"freshly-minted `session_id` … `/auth/bootstrap/from_pairing`"那句改为描述 token 投递（一句话：`Approved` carries the chat-tier device token the browser stores and connects with）。

- [ ] **Step 2: 改侧表存凭证**

`src/gateway/security/pairing.rs:179-185` 字段改为存 `(token, device_id, approved_at)`：
```rust
    /// In-memory map: pairing code → (device_token, device_id, approved_at_ms).
    /// Populated by `record_browser_credential` when the operator approves a
    /// Browser pairing; drained by `poll_browser_pairing` on the first
    /// approved poll (single-use). TTL bounded by `APPROVED_SESSION_TTL_MS`
    /// so a never-polled browser doesn't pin memory.
    approved_browser_sessions: DashMap<String, (String, String, i64)>,
```
（字段名 `approved_browser_sessions` 保留不动以缩小改动面；仅元组形状变化。）`new`/`with_expiry` 的 `DashMap::new()` 初始化不变。

- [ ] **Step 3: `record_browser_session` → `record_browser_credential`**

把 :410-416 替换为：
```rust
    /// Stash the chat-tier device token + device_id minted by the operator's
    /// `pairing.approve` for a Browser pairing, keyed by the pairing code.
    /// Drained single-use by the next approved `poll_browser_pairing`.
    pub fn record_browser_credential(&self, code: &str, token: &str, device_id: &str) {
        self.gc_browser_state();
        self.approved_browser_sessions.insert(
            code.to_string(),
            (token.to_string(), device_id.to_string(), current_timestamp_ms()),
        );
    }
```

- [ ] **Step 4: 删 `fetch_browser_session`**

删除 :418-427 整个 `pub fn fetch_browser_session`（其唯一消费者 `/auth/bootstrap/from_pairing` 在 Task 3 一并清理）。

- [ ] **Step 5: `poll_browser_pairing` 改为 drain 返回凭证**

把 :441-459 的 approved 分支从 PEEK 改为 DRAIN：
```rust
    pub fn poll_browser_pairing(&self, code: &str) -> PollState {
        self.gc_browser_state();
        // Single-use: drain the credential on the first approved poll. The
        // /pair page acts on this exact response (stores token + redirects),
        // so a second poll for the same code returns Expired. Handing the
        // device token out exactly once is the fail-closed choice for a real
        // credential (vs. the old peekable session_id).
        if let Some((_, (token, device_id, _))) = self.approved_browser_sessions.remove(code) {
            return PollState::Approved { token, device_id };
        }
        if self.rejected_browser_codes.remove(code).is_some() {
            return PollState::Rejected;
        }
        match self.store.get_pairing_request(code) {
            Ok(Some(row)) if row.pairing_type == "browser" => PollState::Pending,
            _ => PollState::Expired,
        }
    }
```

- [ ] **Step 6: 更新该文件内单测**

`src/gateway/security/pairing.rs` 测试（约 :720-760）里所有 `record_browser_session(&code, "session-xyz")` → `record_browser_credential(&code, "tok-abc", "browser-1")`；`PollState::Approved { session_id }` 解构 → `PollState::Approved { token, device_id }` 并断言 `token == "tok-abc"`、`device_id == "browser-1"`。

把"approved 可重复 poll"的断言改为单次语义：第一次 poll 返回 `Approved{token,..}`，**第二次** 同 code poll 返回 `PollState::Expired`（drain 后）。具体改 `*_pending_then_approved` 测试：
```rust
        manager.record_browser_credential(&code, "tok-abc", "browser-1");
        match manager.poll_browser_pairing(&code) {
            PollState::Approved { token, device_id } => {
                assert_eq!(token, "tok-abc");
                assert_eq!(device_id, "browser-1");
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        // single-use: second poll after drain is Expired
        assert_eq!(manager.poll_browser_pairing(&code), PollState::Expired);
```

- [ ] **Step 7: 编译 + 测试**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --lib 2>&1 | tail -20
```
Expected: 编译失败仅来自 `record_browser_session`/`fetch_browser_session`/`PollState::Approved{session_id}` 的旧调用点（`handlers/auth/pairing.rs`、`auth_middleware.rs`）——这些在 Task 2/3 修。**本步只需 `pairing.rs` 自身无语法错**；若 `--lib` 因下游调用点报错，改用：
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib gateway::security::pairing 2>&1 | tail -25
```
Expected: `pairing` 模块单测通过（`*_pending_then_approved`、reject、expired、six_digit_code 等）。若因下游 crate 编译失败而测试跑不起来，记录为"待 Task 2/3 修复的预期下游断裂"，继续。

- [ ] **Step 8: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add src/gateway/security/pairing.rs
git commit -m "pairing: browser side-table stores chat-tier credential, poll drains single-use"
git show --stat HEAD
```

---

### Task 2: Browser 审批铸 chat 档设备 + poll 返回 token + e2e 测试

**Files:**
- Modify: `src/gateway/handlers/auth/pairing.rs`（Browser 分支 :83-142、`handle_pairing_poll` :472-495、测试 :618-674）

- [ ] **Step 1: 写失败的端到端测试（连接落 guest）**

在 `src/gateway/handlers/auth/pairing.rs` 的 `mod tests`（:517）内新增。先确认导入（文件已 `use crate::gateway::handlers::auth::connect::handle_connect;`）。新增测试：
```rust
    #[tokio::test]
    async fn browser_pairing_mints_chat_tier_device_and_connects_as_guest() {
        let ctx = super::super::tests::create_test_context();

        let start = handle_pairing_start_browser(
            JsonRpcRequest::new(
                "pairing.start_browser",
                Some(json!({
                    "origin_label": "Safari on 192.168.1.9",
                    "user_agent": "Mozilla/5.0",
                    "peer_ip": "192.168.1.9"
                })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;
        let code = start
            .result
            .unwrap()
            .get("code")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let approve = handle_pairing_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({ "code": code.clone() })),
                Some(json!(2)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(approve.is_success(), "approve failed: {:?}", approve);

        // poll returns the chat-tier device token + device_id (not session_id)
        let poll = handle_pairing_poll(
            JsonRpcRequest::new("pairing.poll", Some(json!({ "code": code })), Some(json!(3))),
            ctx.clone(),
        )
        .await;
        let body = poll.result.unwrap();
        assert_eq!(body.get("status").unwrap().as_str().unwrap(), "approved");
        let token = body
            .get("token")
            .and_then(|v| v.as_str())
            .expect("approved poll must include a device token")
            .to_string();
        let device_id = body
            .get("device_id")
            .and_then(|v| v.as_str())
            .expect("approved poll must include device_id")
            .to_string();
        assert!(
            body.get("session_id").is_none(),
            "browser pairing no longer returns session_id"
        );

        // the minted device is a persistent chat-tier device
        assert!(ctx.device_store.is_approved(&device_id));
        let device = ctx.device_store.get_device(&device_id).unwrap();
        assert_eq!(
            device.permissions,
            vec!["chat".to_string(), "read".to_string()]
        );

        // connecting with that token resolves to role=guest, no wildcard
        let connect = handle_connect(
            JsonRpcRequest::new(
                "connect",
                Some(json!({ "token": token, "device_name": "Web Panel" })),
                Some(json!(4)),
            ),
            ctx,
        )
        .await;
        assert!(connect.is_success(), "connect failed: {:?}", connect);
        let cr = connect.result.unwrap();
        assert_eq!(cr.get("role").unwrap().as_str().unwrap(), "guest");
        let perms: Vec<String> = cr
            .get("permissions")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(!perms.iter().any(|p| p == "*"), "chat tier must not hold wildcard");
    }
```

- [ ] **Step 2: 运行测试确认失败**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib browser_pairing_mints_chat_tier_device 2>&1 | tail -25
```
Expected: 编译失败或断言失败——当前 Browser 分支铸 operator 会话、poll 返回 session_id，故 `token`/`device_id`/role=guest 断言不满足。

- [ ] **Step 3: 重写 Browser 审批分支**

把 `src/gateway/handlers/auth/pairing.rs:83-142` 的 `PairingRequest::Browser { code, origin_label, .. } => { ... }` 整块替换为复用 Device 分支档位路径、强制 Chat：
```rust
        PairingRequest::Browser {
            code,
            origin_label,
            user_agent,
            ..
        } => {
            // Browser pairing always lands at the Chat tier (chat + read, no
            // config rights). The operator can later elevate it to config via
            // the Devices list (devices.set_level). We mint a *persistent*
            // chat-tier device — identical to the Device branch but tier-forced
            // — and hand its device token to the browser via pairing.poll, so
            // it connects through Case 1 (role derived from permissions =
            // "guest"). This is remote-capable; it does NOT ride the
            // loopback-only shared-token cookie bootstrap.
            let tier = super::tier::Tier::Chat;
            let tier_permissions = tier.permissions();

            let device_id = format!("browser-{}", uuid::Uuid::new_v4());
            let device_name = if !origin_label.is_empty() {
                origin_label.clone()
            } else if !user_agent.is_empty() {
                user_agent.clone()
            } else {
                "Browser".to_string()
            };
            let mut device = ApprovedDevice::new(device_id.clone(), device_name.clone(), None);
            device.permissions = tier_permissions.clone();

            if let Err(e) = ctx.device_store.approve_device(&device) {
                warn!(error = %e, "Failed to store approved browser device");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to store device: {}", e),
                );
            }

            let device_fingerprint: String = device_id.chars().take(16).collect();
            if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: &device_name,
                device_type: None,
                public_key: &[0u8; 32],
                fingerprint: &device_fingerprint,
                role: super::tier::role_for_permissions(&tier_permissions),
                scopes: &tier_permissions,
            }) {
                warn!(error = %e, "Failed to register browser device in security store");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to register device: {}", e),
                );
            }

            let signed_token = match ctx.token_manager.issue_token(
                &device_id,
                DeviceRole::Operator, // memory namespace only; authz uses connect-response role
                tier_permissions.clone(),
            ) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Failed to issue browser device token");
                    return JsonRpcResponse::error(
                        request.id,
                        -32603,
                        format!("Failed to issue token: {}", e),
                    );
                }
            };
            let token = format!("{}:{}", signed_token.token, signed_token.signature);

            ctx.pairing_manager
                .record_browser_credential(code, &token, &device_id);

            if let Err(e) = ctx
                .event_bus
                .publish_frame(&GatewayEventFrame::PairingCompleted {
                    device_id: code.clone(),
                })
            {
                warn!(error = %e, "Failed to publish PairingCompleted frame");
            }

            info!(
                code = %code,
                device_id = %device_id,
                origin = %origin_label,
                "Browser pairing approved as chat-tier device"
            );
            return JsonRpcResponse::success(
                request.id,
                json!({
                    "code": code,
                    "kind": "browser",
                    "device_id": device_id,
                    "approved": true,
                }),
            );
        }
```
> `user_agent` 现需从 `PairingRequest::Browser` 解构（确认该变体有 `user_agent: String` 字段——`handle_pairing_list` :344-352 已读它，存在）。若实际字段是 `Option<String>` 或别名，按真实定义调整解构与空判断。

- [ ] **Step 4: `handle_pairing_poll` approved 返回 token + device_id**

把 `src/gateway/handlers/auth/pairing.rs:486-494` 的 match 改为：
```rust
    match ctx.pairing_manager.poll_browser_pairing(&params.code) {
        PollState::Pending => JsonRpcResponse::success(request.id, json!({"status": "pending"})),
        PollState::Approved { token, device_id } => JsonRpcResponse::success(
            request.id,
            json!({"status": "approved", "token": token, "device_id": device_id}),
        ),
        PollState::Rejected => JsonRpcResponse::success(request.id, json!({"status": "rejected"})),
        PollState::Expired => JsonRpcResponse::success(request.id, json!({"status": "expired"})),
    }
```

- [ ] **Step 5: 更新既有 handler 测试**

`approve_browser_pairing_makes_poll_return_approved`（:618-674）：把 `assert body.get("session_id").is_some()` 改为断言 `body.get("token").is_some()` 且 `body.get("device_id").is_some()`，并把 `status=="approved"` 保留。（若该用例与 Step 1 新增的 e2e 测试高度重叠，可保留两者；e2e 更全。）

- [ ] **Step 6: 运行测试确认通过**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib 'gateway::handlers::auth::pairing' 2>&1 | tail -30
```
Expected: 新增 e2e + 既有 browser 测试全绿。

- [ ] **Step 7: fmt + clippy（本文件）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p alephcore && cargo clippy -p alephcore --lib 2>&1 | grep -A3 "handlers/auth/pairing" | head -20
```
Expected: fmt 无残留；clippy 对本文件无新警告。`cargo fmt -p alephcore` 会格式化整个 crate——提交只 add 本文件。

- [ ] **Step 8: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add src/gateway/handlers/auth/pairing.rs
git commit -m "pairing: browser approval mints chat-tier device, poll returns its token"
git show --stat HEAD
```

---

### Task 3: `/pair` 页存 token + 清理死 cookie 路径

**Files:**
- Modify: `src/gateway/auth_middleware.rs`（`/pair` HTML 内嵌 JS 的 poll 回调、`handle_bootstrap_from_pairing` handler + 路由 + 测试）

- [ ] **Step 1: 确认 `from_pairing` 死代码范围**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && rg -n "from_pairing|fetch_browser_session|bootstrap/from_pairing|handle_bootstrap_from_pairing|BootstrapFromPairingQuery" --type rust
```
Expected: 命中应仅在 `auth_middleware.rs`（handler + route 注册 + query struct + 可能的测试）与本计划已删的 `pairing.rs::fetch_browser_session`（Task 1 已删）。**若出现任何其它消费者**（如 desktop shell、CLI、别的 handler），则 from_pairing 仍在用 → **不删**，只改 /pair JS（Step 2），跳过 Step 3 的删除，并在提交信息注明保留原因。

- [ ] **Step 2: 改 `/pair` HTML 的 poll 回调**

在 `src/gateway/auth_middleware.rs` 的 `pair_page_html`（约 :252-328）内，找到 poll 回调里处理 `s === 'approved'` 的分支（当前为 `window.location.href = '/auth/bootstrap/from_pairing?code=' + encodeURIComponent(code);`），替换为：
```js
            if (s === 'approved') {
                if (r.result.token) {
                    try { localStorage.setItem('aleph_device_token', r.result.token); } catch (e) {}
                }
                window.location.href = '/';
            } else if (s === 'rejected') {
```
（保留 rejected/expired/else 分支不变；只改 approved 分支的两行。以文件实际 JS 字符串为准做精确替换。）

- [ ] **Step 3: 删 `handle_bootstrap_from_pairing`（仅当 Step 1 确认死代码）**

若 Step 1 确认无其它消费者：删除 `src/gateway/auth_middleware.rs` 中的 `async fn handle_bootstrap_from_pairing`、其 `BootstrapFromPairingQuery` struct、router 里 `.route("/auth/bootstrap/from_pairing", ...)` 的注册行，以及任何只测它的单测。删除后 `fetch_browser_session` 已无引用（Task 1 已删该方法）。

> 不删除 `/auth/bootstrap`（loopback nonce consume，同机 `aleph open` 仍用）——只删 `from_pairing` 这一条浏览器配对专用路由。grep `/auth/bootstrap` 区分两者，勿误删。

- [ ] **Step 4: 编译全目标**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore --all-targets 2>&1 | tail -20
```
Expected: 整 crate + 测试目标编译通过零错误（Task 1/2/3 的所有调用点已对齐）。

- [ ] **Step 5: 全相关测试**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib 'gateway::security::pairing' 2>&1 | tail -15
cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore --lib 'gateway::handlers::auth::pairing' 2>&1 | tail -20
```
Expected: 两组 pairing 测试全绿。

- [ ] **Step 6: fmt（本文件）**

Run:
```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo fmt -p alephcore && cargo fmt -p alephcore --check 2>&1 | grep auth_middleware | head
```
Expected: 无残留。

- [ ] **Step 7: 提交（显式路径）**

```bash
cd /Volumes/TBU4/Workspace/Aleph
git status
git add src/gateway/auth_middleware.rs
git commit -m "pairing: /pair page stores chat-tier device token, drop dead from_pairing cookie route"
git show --stat HEAD
```

---

## 最终验证（全任务完成后）

- [ ] `cargo check -p alephcore --all-targets` 绿
- [ ] `cargo test -p alephcore --lib gateway::security::pairing` + `gateway::handlers::auth::pairing` 全绿（含新 e2e connect-as-guest）
- [ ] `git diff <base>..HEAD --stat` 只含 `src/gateway/` 下 3 文件 + docs，**无 `interfaces/` 改动**（验证 Panel 零改动），无 `dist/` 产物
- [ ] 派 final code reviewer 审整体：①Browser 分支与 Device 分支档位铸造一致、强制 Chat；②poll 单次 drain 语义正确（二次 expired）；③`/pair` JS token→localStorage→connect Case 1→guest 端到端；④from_pairing 仅在确认死代码后删除、未误删 `/auth/bootstrap` loopback nonce 路径；⑤fail-closed：铸设备/发 token 失败不回退 operator；⑥新 browser 设备出现在 `security_config.list_devices`（Phase 3b-1 Devices 列表）且可经 `devices.set_level` 升档。

## 部署（用户决定时机）

后端 + 内嵌 `/pair` HTML（运行时生成，**无需 `just wasm`**）：见效需重编 `aleph-server` + 热替换 daemon。可与 Phase 3a/3b 系列统一上线。
