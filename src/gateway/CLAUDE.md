# src/gateway/ — 安全边界护栏

> 本目录是 Aleph 的网络信任边界。改动认证 / 授权 / Origin 逻辑高风险，
> 编辑前必读。完整模型见 [SECURITY.md#auth-ux](../../docs/reference/SECURITY.md#auth-ux)。

## 信任模型 = 网络边界

- LAN-trust：无认证步骤，信任边界就是网络边界。默认只绑 `127.0.0.1`；
  `[gateway] host = "0.0.0.0"` 显式开放整个局域网。

## 两道护栏

- **device tier**（`method_authz.rs`）：远程 Panel 默认 **Chat tier**；对 Aleph
  自身配置的变更（`self_config` / `skill_install` / provider 配置 / `devices.*`
  等 config 类 RPC 与工具）须 operator，经 `devices.set_level` 显式提权；
  本机 (loopback) 始终 operator。
- **WS Origin 校验**（`origin_policy.rs`）：唯一保留的协议护栏，挡公网恶意网页
  跨源驱动 agent。域名部署须把 origin 加进 `[gateway] allowed_origins`。

## 红线

- 改认证 / 授权 / Origin 逻辑**必须同步更新测试**，不得只改实现。
- 不在 Gateway/Interface 层处理业务逻辑（R4：纯 I/O）。
