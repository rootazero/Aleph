# R7 桌面切换回执 / R7 desktop cutover receipt

验收基线为合入 R7 候选后的 `main`，切换工作从 `fe979a1d7e4dea2653698df6443e6e138a065434` 开始。2026-11-27，用户确认已完成 Windows 和 Linux 验收并将结果同步到本地 `main`，随后授权在 `main` 退役 Tauri。/ The R7 candidate was already integrated into `main`. On 2026-11-27, the operator confirmed Windows and Linux acceptance and authorized retirement on `main`.

| 平台 / Platform | 整包与启动证据 / Package and launch evidence |
| --- | --- |
| macOS arm64 | NSH 0.6.0 打包 `Aleph-26.9.1-macos-aarch64.tar.gz`，SHA-256 `1078bb6673a26577f426fca5ceb946409bf5a6188f312a4e2fcc697c11c0ff6a`。严格 `codesign` 验证通过；用隔离数据启动随包 Gateway，打开历史列表；另一次走正式 Local 入口，在约 6 秒后就绪。点击 **Quit Aleph and Stop Server** 后，宿主、随包服务进程和监听端口均退出。 / Strict bundle signature verification, isolated-data startup and history, plus native Local service shutdown witnessed. |
| Windows x64 | 用户确认通过；验收步骤见下方提示。早期 LAN 预检遇到 Winsock 拒绝连接耗时超过 2 秒，`5c9ce5d46` 单独为 Windows 把连接预算提高为 4 秒。最终确认来自用户，仓库没有对应的包哈希或测量日志。 / Operator-confirmed acceptance. [Windows QA prompt](2026-11-27-nsh-r7-windows-qa-prompt.md) |
| Linux x64 | 用户确认通过；验收步骤见下方提示。早期 Hyprland 预检见到整包窗口、Gateway、托盘注册和二次启动退出。仓库记录了用户确认，具体包哈希和测量日志未附于本回执。 / Operator-confirmed acceptance. [Linux QA prompt](2026-11-27-nsh-r7-linux-qa-prompt.md) |

macOS 候选包的原生菜单、托盘和图标、热键、深链接、打开系统浏览器、远程切换、受保护配对、登录启动和签名更新均有单独的运行证据。会话夹具同时验证了群聊、原个人会话的历史恢复和实时运行数 `0 → 1 → 0`。TCC 新授权触发仅有状态转换测试，系统授权状态在验收时保留。/ macOS runtime checks covered shell controls, pairing, login launch and a signed update. The fixture witnessed group chat, direct history restoration and live run counts together. A fresh TCC grant transition has unit coverage.

三平台 v26.9.1 回退包已核对 SHA-256，并用该标签中的旧更新公钥通过 minisign 验签。本地保留包、原始签名、解码后的签名和校验清单。切换和恢复按同一份操作说明执行。/ All three original v26.9.1 rollback assets passed SHA-256 and minisign verification against the tagged updater key. [回退步骤与资产哈希 / Rollback steps and asset hashes](../../reference/RELEASE.md#桌面切换与回退--desktop-cutover-and-rollback)
