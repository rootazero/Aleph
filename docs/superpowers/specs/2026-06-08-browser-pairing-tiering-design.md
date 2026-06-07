# Browser 配对档位化 Design

**Goal:** 让远程 `/pair` 浏览器配对第一次真正可用，且落在 **chat 档持久设备**——批准后浏览器得到一个 chat 档（`["chat","read"]` / role `guest`）的设备身份，而非当前忽略档位、仅 loopback 可用的 shared-operator 会话。operator 之后可在 Devices 列表把它升到 config。

**Tech Stack:** Rust（`src/gateway/handlers/auth/pairing.rs`、`src/gateway/security/pairing.rs`/PairingManager、`src/gateway/auth_middleware.rs` 内嵌 `/pair` HTML）。Leptos Panel（`interfaces/webchat`）**零改动**；`connect.rs` **零改动**。

---

## 背景与关键发现

### 当前缺口（已核实）
1. **远程 `/pair` 今天是坏的**：cookie-bootstrap（`resolve_bootstrap_shared_token`，`server/handler.rs:169`）受 `peer_addr.ip().is_loopback()` 门控；远程浏览器 WS 升级非 loopback → 返回 `None` → connect 不被注入凭证。浏览器 localStorage 又是空的（全新浏览器），于是 connect 落入 Case 3「pairing_required」，永远连不上。
2. **即便能连，也是 operator**：Browser 配对审批分支（`pairing.rs:83-142`）把会话 keyed 到 daemon 自己的 shared-token HMAC（`hmac_sign(secret, shared_token)`），忽略 `level`。同机 loopback 浏览器经此变 operator。
3. Phase 3b-1 Task 5 正因「browser approval mints shared operator token, ignores level」回退了审批卡的双钮选档。

### 已经就位、本期复用（零改动）
- **connect.rs Case 1（device token）与 Case 2（approved device_id）** 都已 `role = role_for_permissions(&permissions)`，即设备 permissions=`["chat","read"]` → role `"guest"`（`connect.rs:382-383`、`:448`）。**connect 侧无需任何改动。**
- **Panel `interfaces/webchat/src/context.rs:305-392`** 已实现凭证读取链：优先读 `localStorage["aleph_device_token"]` → connect 带 `params.token`；connect 返回的新 token 回写 localStorage（`:314`、`:328`）。**Panel 无需任何改动**——只要把 chat 档 token 放进 `localStorage["aleph_device_token"]`。
- **Device 配对分支（`pairing.rs:145-217`）** 已是完整档位路径：`Tier` → `ApprovedDevice` 带 tier permissions → `device_store.approve_device` + `security_store.upsert_device(role=role_for_permissions, scopes=perms)` → `token_manager.issue_token(perms)`。本期 Browser 分支复用同一套，强制 Chat 档。
- **Phase 3b-1 Devices 列表** 数据源 `security_config.list_devices` 读 device_store/security_store → 新铸的 browser 设备自动出现（chat 徽章、可吊销）。
- **Phase 3a `devices.set_level`** 可把该 browser 设备 chat→config 升档（落「默认 chat、之后升档」决策）。

### 用户已决策（brainstorm）
- **身份模型 = 持久化 chat 档设备**（非临时会话、非复用 guest 邀请体系）。
- **档位选择 = 默认 chat，之后经 Devices 升档**（不恢复审批双钮卡；本期审批卡保持单个 Approve 不变，零 Panel 改动）。

---

## 架构（数据流）

```
远程浏览器 → GET /pair (内嵌 HTML)
  → pairing.start_browser → 生成 6 位 code + PairingRequested 事件
  → operator 在 Panel 通知卡点 Approve → pairing.approve {code}
     [后端 Browser 分支]:
       Tier::Chat.permissions() = ["chat","read"]
       device_id = "browser-{uuid}"; name 取 origin_label/user_agent
       device_store.approve_device(perms=chat)
       security_store.upsert_device(role="guest", scopes=["chat","read"])
       token = token_manager.issue_token(device_id, perms=chat)
       pairing_manager 按 code 暂存 (token, device_id)
       发 PairingCompleted 事件
  → /pair 页轮询 pairing.poll {code}
       → {status:"approved", token, device_id}   (单次消费; 二次 poll = expired)
  → /pair 页 JS: localStorage["aleph_device_token"] = token; location = "/"
  → Panel 加载 → context.rs 读 localStorage → connect {token}
  → connect.rs Case 1: validate_token → permissions=["chat","read"] → role="guest"
  → ConnectionState.role="guest" → method-authz 门控 / Phase2 工具门控 / event-scope 全部按 chat 档咬合
```

同机 operator 仍走独立的 `aleph open` / desktop handoff loopback-bootstrap（不变，仍 operator）。`/pair` 是远程/非信任路径，一律 chat 档。

---

## 组件与改动

### A. `pairing.rs` Browser 分支重写（`:83-142`）
不再铸 shared-token 会话。改为：
1. `let tier = Tier::Chat;`（Browser 配对恒 Chat；档位提升留给 Devices/set_level）。
2. `let perms = tier.permissions();`（`["chat","read"]`）。
3. `device_id = format!("browser-{}", uuid)`；`device_name` 取 `origin_label`（空则 user_agent / "Browser"）。
4. `ApprovedDevice::new(device_id, device_name, device_type=None)`，`device.permissions = perms`。
5. `device_store.approve_device(&device)`（失败 → 返回 error，**不回退 operator**）。
6. `security_store.upsert_device(DeviceUpsertData{ role: role_for_permissions(&perms)=="guest", scopes:&perms, ... })`。
7. `token = token_manager.issue_token(&device_id, DeviceRole::Operator/*内存命名空间，authz 用 connect-response role*/, perms.clone())`（与 Device 分支同款；role 串由 connect 按 permissions 派生）。
8. `pairing_manager.record_browser_credential(code, token, device_id)`（替换 `record_browser_session`：存 token 而非 session_id）。
9. 仍 `event_bus.publish_frame(PairingCompleted{device_id: code})`。
10. 返回 success `{code, kind:"browser", approved:true}`（与今日一致）。

> 不再调用 `shared_token_mgr` / `session_mgr.create_session` / `record_browser_session`。`/pair` 路径与 daemon shared token 彻底解耦。

### B. PairingManager 暂存改 token（`src/gateway/security/pairing.rs`）
- `record_browser_session(code, session_id)` → `record_browser_credential(code, token, device_id)`。
- `fetch_browser_session(code) -> Option<String>` → `fetch_browser_credential(code) -> Option<(String, String)>`（token, device_id）；单次消费语义保持（取后失效，二次取 None）。
- `PollState::Approved { session_id }` → `PollState::Approved { token, device_id }`。
- `poll_browser_pairing` 内部 approved 分支返回新结构。reject/expired/pending 不变。

### C. `pairing.poll` handler（`pairing.rs:472-495`）
- approved 分支：`json!({"status":"approved","token":token,"device_id":device_id})`（去掉 session_id）。

### D. `/pair` 内嵌页（`auth_middleware.rs::pair_page_html`，约 `:252-328`）
poll 回调 approved 分支：
```js
if (s === 'approved') {
    if (r.result.token) { localStorage.setItem('aleph_device_token', r.result.token); }
    window.location.href = '/';
}
```
不再跳 `/auth/bootstrap/from_pairing`（其 cookie 对远程 loopback-无用；token 路径取代之）。

> `from_pairing` handler 与 `fetch_browser_session` 的旧 cookie 路径若不再有其它调用者，可在实现时确认后清理；若仍被同机 bootstrap 复用则保留（实现期 grep 确认，不在本 spec 强制删除）。

### E. Panel / connect.rs
**零改动。**

---

## 错误处理 / 安全（fail-closed）

- 设备铸造 / token 签发任一步失败 → poll **不返回** approved-with-token（返回 error 或维持 pending→expired），**绝不回退 operator**。
- `code` 是短时、operator 批准、单次消费的 bearer 秘密——信任级别与今天返回 session_id（cookie 值）一致。即便 token 被截获，只有 `["chat","read"]`，无 config/operator 权限，爆炸半径受限。
- `/pair` 完全不碰 daemon shared token；同机 operator 路径（`aleph open`/handoff）独立不变。
- 同机用户用 `/pair` 也得 chat 档（安全默认；要 operator 走 `aleph open`）。
- 传输层（LAN 明文）风险与今日返回 session_id 相同，不在本期扩大范围。

---

## 测试

1. **改** `approve_browser_pairing_makes_poll_return_approved`：审批后 poll 返回 `token`+`device_id`（非 session_id）；该 token 经 `token_manager.validate_token` 出 `["chat","read"]`。
2. **新** `browser_pairing_registers_chat_tier_device`：审批后 `device_store.is_approved(device_id)` 为真，设备 permissions=`["chat","read"]`；`security_store` 中 role=`role_for_permissions(chat)`=="guest"、scopes=chat/read。
3. **新** `browser_paired_token_connects_as_guest`：用 poll 返回的 token 走 `handle_connect{token}` → 成功，response `role=="guest"`、`permissions` 不含 `"*"`（端到端档位咬合）。
4. **新** `poll_browser_credential_is_single_use`：二次 poll 同 code 返回 `expired`（不重复发 token）。
5. **保留** `reject_browser_pairing_makes_poll_return_rejected`、`start_browser_then_poll_pending`、`approve_params_*`。

---

## 范围与非目标

- **范围**：`/pair` 远程浏览器配对落 chat 档持久设备 + 投递机制（poll 返回 token + `/pair` 页存 localStorage）。
- **非目标**：审批卡双钮选档（已决策不做）；Panel UI 改动；同机 `aleph open` operator 路径；guest 邀请体系；传输层加密；Spec A 桌面 shell 远程连接（独立子项）。

---

## 部署

后端 + 内嵌 `/pair` HTML（在 .rs 字符串里，运行时生成，**无需 `just wasm`**）：见效需重编 `aleph-server` + 热替换 daemon。可与 Phase 3a/3b 系列一起统一上线。
