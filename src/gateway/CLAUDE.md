# src/gateway/ — 安全边界护栏

> 本目录是 Aleph 的网络信任边界。改动认证 / 授权 / Origin 逻辑高风险，
> 编辑前必读。完整模型见 [SECURITY.md#auth-ux](../../docs/reference/SECURITY.md#auth-ux)。

## 信任模型 = 网络边界 + Gateway token

- **网络边界**：默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放整个局域网。
- **本机 (loopback)**：免 token 自动 operator（零配置，勿回归）。
- **远程 (LAN)**：纯 WS 直连（**非 channel 通道**），行为等同浏览器打开 core IP。授权凭据
  按优先级（`connect::resolve_connect_auth` 4 级）：① `device_token`（`aleph-dt-*` 长效绑设备）
  ② `bootstrap_ticket`（`aleph-bt-*` 5min 一次性配对票，扫 `?bt=` QR，connect 时换取 device token）
  ③ legacy 共享 **Gateway token**（`aleph-<uuid>`，`SharedTokenManager`）。校验通过 = operator，
  权限与本地**完全一致**（单层）；未通过 = 登录墙（WS 派发仅放行 `connect`）。**长效凭据不进
  URL/QR**——QR 只编码一次性配对票，修复 `?token=` 泄露向量。
- **撤销**：① `gateway.token.rotate` = 核弹级（重生共享 token **并** `revoke_all_panel_devices`，
  cluster 节点不受影响）。② `gateway.devices.revoke {device_id}` = 单设备（下次重连即拒）；
  清单 `gateway.devices.list`（仅 `device_type='panel'`）。两者纯 I/O，scope 守卫不碰 cluster 节点。
  设备令牌/配对票逻辑在 `security/device_token_manager.rs`。

## 两道护栏

- **登录墙**（`server::handler` + `handlers::connect::connect_authorized`）：远程未授权
  连接只能发 `connect`；授权（loopback 或有效 token）= operator 全权，与本地一致。
- **channel 工具闸**（`method_authz.rs::tool_requires_operator` + `tools/scoped/dispatch.rs`）：
  **仅治理 channel**（Telegram / Slack…）——`inbound_router` 按 `ChannelPermissionLevel`
  （默认 Chat ⇒ `guest`）盖 `caller_role`，禁 chat-tier channel 跑自配置类工具。Panel
  授权后恒 operator，此闸对 Panel 自然全过。
- **WS Origin 校验**（`origin_policy.rs`）：挡公网恶意网页跨源驱动 agent。域名部署须把
  origin 加进 `[gateway] allowed_origins`。

## 红线

- 改认证 / 授权 / Origin 逻辑**必须同步更新测试**，不得只改实现。
- 不在 Gateway/Interface 层处理业务逻辑（R4：纯 I/O）。
