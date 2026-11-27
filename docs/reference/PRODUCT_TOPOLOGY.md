# 产品形态与分发拓扑 / Product topology

Aleph 发布两种产物。NSH 桌面整包包含 `aleph-server`，可在运行时选择 Local 或 Remote。独立 `aleph-server` 面向服务器与 NAS。两种产物使用同一 Rust Core 和内嵌的 Leptos/WASM Panel。/ Both deliverables use one Rust Core and one embedded Panel.

## 维护单元 / Source units

| 单元 / Unit | 路径 / Path | 职责 / Role |
| --- | --- | --- |
| Panel | `interfaces/webchat/` | 业务界面，构建资源编译进服务器。 / Business UI embedded in the server. |
| Core | `alephcore` → `aleph-server` | 授权、业务数据、代理和 HTTP/WS Gateway。 / Authorization, business state, agents and Gateway. |
| Desktop shell | `desktop/nsh-shell/` | NSH WebView、平台集成和本机服务管理，桥契约在 `shared/protocol/src/panel_shell.rs`。 / Native host and packaged-service lifecycle. |

## 部署方式 / Deployment

- **Local desktop** 在一台电脑上打开 NSH，随包服务默认监听 `127.0.0.1:18790`。 / The packaged server supplies the UI and data.
- **Remote desktop** 在同一个桌面包里选择 Remote。目标 Gateway 提供 UI 与业务能力；笔记本可保留托盘、系统通知和快捷键。 / The remote Gateway serves the Panel. Saving the target carries it across restarts.
- **Headless server** 在 NAS 或服务器上安装独立 `aleph-server`。客户端通过浏览器或 NSH Remote 接入。 / A browser can open the same Panel directly.

Gateway 默认绑定回环地址。远程访问遵循 Gateway 的认证、Origin 检查与 TLS 配置。桌面 Remote 使用系统信任库。 / Remote deployments use the Gateway trust model. [Security](SECURITY.md) · [Desktop connection](DESKTOP_SHELL.md)

## 构建与部署 / Build and deploy

| 改动 / Change | 产物 / Deliverables |
| --- | --- |
| Panel | `just wasm` 后重建服务器，使 `rust_embed` 重新嵌入生产资源。随包服务与独立服务均需更新。 / Rebuild the server after the Panel. |
| Core | 重建服务器及包含它的整包。 / Rebuild the server and its desktop archive. |
| NSH host | `just shell-build` 重建桌面整包。 / Rebuild the desktop archive. |

`just shell-dev` 启动本机开发宿主，`just verify-build` 构建三平台发版候选。macOS、Windows、Linux 使用各自系统 WebView，检查渲染问题时用目标机对应引擎。 / The release matrix publishes desktop archives and standalone binaries for all three platforms. [Release and rollback](RELEASE.md)
