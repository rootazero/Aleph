# Panel 远程连接：Gateway-Token 授权 + 单层权限

> Date: 2026-06-17 · Status: 🚧 目标已定义，代码待对齐 · Trigger: FEATURE_LOCATOR §6.2 与实际架构不符

## 1. 需求（Requirement / 单一真相）

Panel 远程连接 core 的目标模型：

1. **传输**：Panel 远程 = 纯 HTTP/WS 直连，**不走 channel 通道**。
2. **壳 = 浏览器**：壳（thin-shell App）远程连 LAN core 的行为逻辑与「浏览器打开 core IP 网址」**完全一致**——同一条授权路径，壳无特权捷径。
3. **授权 = Gateway token**：单一共享 token（`aleph-<uuid>`，已由 `SharedTokenManager` 在 boot 生成）。两种呈递方式：
   - **token 输入框**：打开 core IP → 未授权 → 弹框 → 输正确 token → 授权成功。
   - **二维码授权**：扫 core 展示的 QR（编码 `http://<ip>:<port>/?token=<gateway-token>`）→ 带 token 打开即授权。
4. **授权后权限 = 同本地**：单层，无 Chat/Config 之分。授权 = operator 全权；未授权 = 登录墙（什么都不能做）。
5. **本机 (loopback)**：自动授权 operator，零配置，免 token。
6. **撤销**：轮换 Gateway token（旧 token 全失效）。无 per-device 会话（YAGNI）。

### 设计决策（已与用户确认）
- 复用策略：**Revive & simplify**——复活被 `6fdd7810f` 删掉的 token 校验路径，但不恢复 `guests.rs`(727L) / 多模式 `AuthMode` / per-device pairing store。
- 撤销模型：**Token rotation only**。
- QR 载荷：**静态 token URL**（非一次性 nonce）。

### 关键复用点（已存在，勿重造）
- `src/gateway/security/shared_token.rs` — `SharedTokenManager::{generate_token, validate, try_load_token_from_db}`。
- `src/gateway/security/store/tokens.rs:78` — `validate_shared_token_hash`。
- boot 已生成并持久化 token：`commands/start/builder/subsystems.rs:86-120`。
- 参考被删的 connect 鉴权路径：`git show 9988fafbe^:src/gateway/handlers/connect.rs` 与 `git show 6fdd7810f^:src/bin/aleph-server/commands/bootstrap_token.rs`（及 `bootstrap_url.rs`）。

### 约束（红线）
- `src/gateway/CLAUDE.md`：改认证/授权/Origin **必须同步更新测试**。
- `ChannelPermissionLevel` 是 channel/inbound_router 系统共用（`inbound_router/{types,executor,mod}.rs`）——**不可删**，只让 panel 解耦。
- loopback 零配置 operator 不可回归。
- 极度节制 cargo 调用（用户偏好）；高风险合并至多一次 `cargo check --lib`。

---

## 2. 代码审计（当前代码 vs 需求的不符合项）

审计基准 = HEAD `ae6d9792c`。两类问题：**A. 两层 tier 需收敛为单层**；**B. token 授权缺失需复活**。

### A. 两层 device tier（`ebf5027a9` 引入）——与「授权后权限同本地」相悖，须收敛

| # | 文件:位置 | 现状 | 目标 |
|---|-----------|------|------|
| A1 | `src/gateway/panel_devices.rs`（整模块） | 每设备 Chat/Config tier 存储（`resolve_tier`/`set_tier`/`tier` 列/`record_seen` 默认 chat） | 单 token 模型下 per-device tier 无意义 → **删除该模块**（device_id 仅前端本地用，不参与授权） |
| A2 | `src/gateway/method_authz.rs` | `OPERATOR_TOOLS` + `OPERATOR_RPC_METHODS` 两张 80+ 项 denylist + `tool_requires_operator`/`rpc_requires_operator` | 单层下「授权=全权/未授权=全拦」→ **删两张表与两个谓词**；改由 connect-level token 闸决定 |
| A3 | `src/gateway/server/handler.rs:690-766` | connect 调 `panel_devices::resolve_tier` → 盖 `caller_role`(operator/guest) + echo role | 改为：loopback 或 token 通过 = operator；否则未授权 |
| A4 | `src/gateway/server/handler.rs:497-537` | per-request `rpc_requires_operator` 闸（非 operator + config 方法 → PERMISSION_DENIED） | 改为「未授权 → 登录墙」闸（仅放行 connect/握手；其余全拦），或随 token 闸合并 |
| A5 | `src/tools/scoped/dispatch.rs:135` | `tool_requires_operator(name)` 工具闸 | 单层 → **删除该 call site**（授权后全权） |
| A6 | `src/gateway/handlers/devices.rs`（整文件） | `devices.{list,set_level,revoke}` tier 管理 RPC | token-rotation 撤销 → **删除**（或留只读 + 新增 token rotate RPC） |
| A7 | `src/bin/aleph-server/commands/start/mod.rs:2025-2060` | panel_devices store 建表 + `set_global_store` + `devices.*` 注册 | 随 A1/A6 **删除该 boot 段** |
| A8 | `interfaces/webchat/src/components/permission.rs`（整文件） | `ConfigGate`/`PermissionBanner`/`LockedNotice`/`friendly_error`/`is_permission_denied` | 单层无「应用内 tier」→ **删除**；要么全应用可见，要么 token 墙 |
| A9 | `interfaces/webchat/src/views/settings/security/devices.rs`（整文件） | `PanelDevicesSection`（Chat/Config tier 按钮） | **删除**；改为「显示 Gateway token + QR + 轮换」（见 B3） |
| A10 | `interfaces/webchat/src/app.rs` + `views/settings/network/cluster.rs` | `ConfigGate` 包裹 | **移除 ConfigGate 包裹** |
| A11 | `interfaces/webchat/src/context.rs:34-39,82-86,208-223,344-360` | `role_is_operator`/`is_operator`/`capture_role` 围绕 "operator" tier | 改为 authorized-or-token-wall；保留 device_id 生成 |

### B. Gateway token 授权缺失——须复活

| # | 文件:位置 | 现状 | 目标 |
|---|-----------|------|------|
| B1 | `src/gateway/handlers/connect.rs:23-35` | 「accepts and ignores any legacy params (token/…)」硬返 `role:"operator"` | **校验 token**：loopback 免验=operator；远程读 `params.token` → `SharedTokenManager::validate` → 通过=operator，失败=返回 `needs_token`/AUTH_REQUIRED 状态（不直接断连，便于前端弹框） |
| B2 | `interfaces/webchat/src/context.rs` connect 握手 | 不发 token；不读 URL query | 读 `localStorage` 与 `window.location` 的 `?token=` → connect 带 token；收到 `needs_token` → 渲染 token 输入框；成功后持久化 token |
| B3 | 前端（待建） | 无 token 框 / 无 QR 页 | 新增：① 未授权登录墙（token 输入框，gate 整个 app）② Settings→Security 显示本机 Gateway token + 生成 QR（`http://<lan-ip>:<port>/?token=…`） |
| B4 | CLI（被 `6fdd7810f` 删） | 无 `bootstrap-token` | **复活 `aleph-server bootstrap-token`**（打印 token，供运维/扫码生成）；可选复活简化 `bootstrap-url`（打印带 token 的 LAN URL） |
| B5 | `docs/reference/SECURITY.md` + `src/gateway/CLAUDE.md` | 描述 LAN-trust no-auth / 两层 tier | 重写为：单层 + Gateway-token 授权（loopback 免验；远程 token 框/QR；撤销=轮换） |

### C. 必须保留 / 不可破坏
- `ChannelPermissionLevel`（`inbound_router/*`）：channel 系统共用，仅 panel 解耦，**不删**。
- `SharedTokenManager` / `SecurityStore`：已存在且 boot 生成 token，**复用不重造**。
- loopback 零配置 operator。
- `connect.rs` 的 `legacy_token_params_are_ignored` 测试需改为「token 被校验」语义。

---

## 3. 修复 Prompt（复制到新 session 实施）

```
任务：把 Panel 远程连接收敛为「Gateway-token 授权 + 单层权限」模型。
需求与审计见 docs/superpowers/specs/2026-06-17-panel-remote-token-auth-design.md，本提示是其落地指令。

目标模型（务必先读 spec 第 1 节）：
- Panel 远程 = 纯 WS 直连（非 channel）。壳远程连 LAN core 行为 == 浏览器打开 core IP。
- 授权 = 单一 Gateway token（aleph-<uuid>，已由 SharedTokenManager 在 boot 生成）。
  · 本机 loopback：免 token 自动 operator（零配置，勿回归）。
  · 远程 LAN：connect 带 token → SharedTokenManager::validate 通过 → operator 全权（与本地完全一致，单层）；
    未授权 → 前端弹 token 输入框 / 扫 QR（QR 编码 http://<ip>:<port>/?token=<token>）。
- 撤销 = 轮换 token（无 per-device 会话）。

复用（勿重造）：
- src/gateway/security/shared_token.rs（generate_token/validate/try_load_token_from_db）
- src/gateway/security/store/tokens.rs:78 validate_shared_token_hash
- boot 生成：commands/start/builder/subsystems.rs:86-120
- 参考被删旧路径：git show 9988fafbe^:src/gateway/handlers/connect.rs
  与 git show 6fdd7810f^:src/bin/aleph-server/commands/bootstrap_token.rs（及 bootstrap_url.rs）

实施（按依赖顺序，逐项对照 spec 第 2 节编号）：
B1) src/gateway/handlers/connect.rs:23-35 — connect 校验 token：
    loopback 免验=operator；远程读 params.token → validate；失败返回 needs_token 状态（不断连）。
    改 connect 单测（legacy_token_params_are_ignored → token 被校验）。
A3+A4) src/gateway/server/handler.rs:497-537 与 690-766 — 删 resolve_tier/caller_role(operator/guest)
    两层逻辑，改为 authorized(operator)/unauthorized(登录墙：仅放行 connect)。
A2+A5) src/gateway/method_authz.rs 删 OPERATOR_TOOLS/OPERATOR_RPC_METHODS 两表与两谓词；
    src/tools/scoped/dispatch.rs:135 删 tool_requires_operator call site。
A1+A6+A7) 删 src/gateway/panel_devices.rs、src/gateway/handlers/devices.rs；
    清 commands/start/mod.rs:2025-2060 的 store 建表/set_global_store/devices.* 注册。
    ⚠️ ChannelPermissionLevel 是 channel 系统共用，只让 panel 解耦，不可删。
B4) 复活 src/bin/aleph-server/commands/bootstrap_token.rs（打印 token）；接回 commands/mod.rs。
B2+B3+A8~A11) 前端：
    - 删 components/permission.rs（ConfigGate/PermissionBanner/LockedNotice/friendly_error）；
      移除 app.rs + views/settings/network/cluster.rs 的 ConfigGate 包裹。
    - 删 views/settings/security/devices.rs 的 PanelDevicesSection；
      改为 Settings→Security 显示 Gateway token + 生成 QR。
    - context.rs：connect 读 localStorage + window.location ?token=，带 token；
      收到 needs_token → 渲染登录墙 token 框；成功后持久化 token；
      role 改 authorized-or-wall（保留 device_id 生成）。
B5) 重写 docs/reference/SECURITY.md 信任模型节 + src/gateway/CLAUDE.md「两道护栏」节为：
    单层 + Gateway-token 授权（loopback 免验 / 远程 token 框·QR / 撤销=轮换 / 网络边界仍是 host 绑定）。

约束：
- 红线（src/gateway/CLAUDE.md）：改认证/授权必须同步改测试，不得只改实现。
- loopback 零配置 operator 不可回归。
- 极度节制 cargo：默认不跑全量；至多一次 cargo check --lib（注意 aleph-server 在 bin，--lib 不覆盖 connect/handler，可能需一次 --bin aleph-server）+ 必要的 panel wasm check。
- 前端改完需重编 binary（rust_embed 嵌入链，见 CLAUDE.md）才生效。

验收：
- loopback 连接：免 token 即 operator 全权（行为不变）。
- 远程无 token：登录墙，除 connect 外全拦。
- 远程输正确 token / 扫 QR：授权后权限 == 本地（可改配置/装 skill/跑 bash 等）。
- 远程输错 token：仍登录墙。
- 轮换 token：旧 token 失效。
- 单测：connect token 校验、method_authz 删除后无悬挂引用、前端 role/wall。
```

---

## 4. 备注

- 历史：no-auth 方向见 `2026-06-12-lan-trust-architecture-revert-design.md`；两层 tier 见 `2026-06-07-chat-config-permission-tier-*` 与 `2026-06-08-browser-pairing-tiering-design.md`。本设计是对二者的再收敛——回到「网页式 token 登录 + 授权后同本地」。
- `src/gateway/config.rs:639` 有 `mode = "token"` / `token_expiry_hours` 样例（疑似 channel pairing 残留），实施时顺带核实是否相关。
