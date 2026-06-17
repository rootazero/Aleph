# src/gateway/ — 安全边界护栏

> 本目录是 Aleph 的网络信任边界。改动认证 / 授权 / Origin 逻辑高风险，
> 编辑前必读。完整模型见 [SECURITY.md#auth-ux](../../docs/reference/SECURITY.md#auth-ux)。

## 信任模型 = 网络边界 + Gateway token

- **网络边界**：默认只绑 `127.0.0.1`；`[gateway] host = "0.0.0.0"` 显式开放整个局域网。
- **本机 (loopback)**：免 token 自动 operator（零配置，勿回归）。
- **远程 (LAN)**：纯 WS 直连（**非 channel 通道**），行为等同浏览器打开 core IP。须在
  `connect` 携带共享 **Gateway token**（`aleph-<uuid>`，boot 由 `SharedTokenManager`
  生成）；校验通过 = operator，权限与本地**完全一致**（单层，无 Chat/Config 之分）；
  未通过 = 登录墙（WS 派发仅放行 `connect`，前端弹 token 框 / 扫 QR）。
- **撤销**：轮换 token（`gateway.token.rotate`，旧 token 全失效）。无 per-device 会话。

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
