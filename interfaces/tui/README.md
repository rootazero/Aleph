# Aleph TUI

Interactive terminal interface for Aleph. Connects to the Aleph Gateway over WebSocket using the shared protocol types. / Aleph 交互式终端界面，通过 WebSocket 和共享协议连接 Gateway。

This crate intentionally does not depend on `alephcore`; it is a pure client built on `aleph-client` and `aleph-protocol`. / 本 crate 不依赖 `alephcore`，仅使用共享客户端与协议。

## Native selection and fonts / 原生选择与字体

The TUI supports direct drag selection in the visible chat pane: press the left mouse button, drag across cells, and release. The selected cells are highlighted without entering a separate F6 mode; `Ctrl+C`, `Ctrl+Shift+C`, `Super+C`, `Ctrl+Insert`, or right-click copy uses the platform clipboard helper. `Esc` clears the selection. Selection is limited to visible terminal cells, may include UI chrome or visual line wraps, and is cleared when the transcript, focus, or pane geometry changes. / TUI 支持直接在可见聊天区域拖动选字：按住鼠标左键，跨单元格拖动后释放；不需要进入额外的 F6 模式，选中单元格会直接高亮。`Ctrl+C`、`Ctrl+Shift+C`、`Super+C`、`Ctrl+Insert` 或右键复制使用平台剪贴板辅助程序，`Esc` 清除选区。选区仅限当前可见终端单元格，可能包含界面装饰或视觉换行；转录内容、焦点或面板几何变化时会清除。

Mouse capture remains enabled so wheel scrolling and drag events reach the TUI. Windows Terminal direct drag selection and post-drag input recovery are verified with real `user32.SendInput`; clipboard integration is covered separately by bounded platform-helper tests, and this does not certify every terminal host. Legacy conhost, macOS, and Linux terminal emulators still need manual acceptance. / 鼠标捕获会保持启用，使滚轮和拖动事件进入 TUI。Windows Terminal 已用真实 `user32.SendInput` 验证直接拖选和拖动后的输入恢复；剪贴板集成另由有界的平台辅助程序测试覆盖，但不代表所有终端宿主均已通过；传统 conhost、macOS 和 Linux 终端模拟器仍需人工验收。

The TUI does not set a portable font face or size and does not emit OSC 50. By default it inherits the terminal. Customize it in the terminal profile: Windows Terminal **Profile > Appearance > Font face/size**, macOS Terminal **Settings > Profiles > Text > Font**, iTerm2 **Profile > Text**, or your Linux terminal emulator's profile settings. The TUI does not hard-code font dimensions, add a dependency, or modify the system/terminal profile. / TUI 不设置可移植的字体名称或字号，也不会发送 OSC 50；默认继承终端设置。请在终端 profile 中自定义：Windows Terminal **Profile > Appearance > Font face/size**，macOS Terminal **Settings > Profiles > Text > Font**，iTerm2 **Profile > Text**，Linux 终端模拟器则使用其 profile 设置。TUI 不硬编码字体尺寸、不新增依赖，也不修改系统或终端 profile。

## Conversation reset / 对话重置

`/new [topic]` calls the Gateway's `sessions.new` operation and switches to the returned new session only after success. The TUI clears its draft and shows the welcome screen without a success receipt in chat; failures remain visible. `/new --help` explains the command without creating a session. `/clear` resets only the local display and draft: it keeps server history and does not cancel live runs or pending approvals, so later output can reappear. / `/new [topic]` 调用 Gateway 的 `sessions.new`，仅在成功后切换到返回的新会话、清空草稿并显示欢迎页，不在聊天中插入成功回执；失败仍会显示。`/new --help` 只展示帮助，不创建会话。`/clear` 只重置本地显示与草稿，保留服务端历史，不取消正在执行的任务或待审批，因此后续输出仍可能重新出现。

## Themes / 主题

Amber is the default; Tide, Jade, Iris and Paper are also built in. `/theme` or `/theme list` opens a local picker; `/theme jade` selects and remembers the choice, and `/theme reload` reloads the active theme file. Picker feedback stays local and never enters chat history. / 默认琥珀，另有潮汐、竹影、鸢尾、素笺。`/theme` 或 `/theme list` 打开本地选择器；`/theme jade` 选择并记忆主题，`/theme reload` 重载当前主题文件。选择器反馈仅保留在本地，不写入聊天记录。

`ALEPH_TUI_THEME` overrides the startup preference. Dark themes inherit the terminal background; Paper supplies an ivory background. Terminal transparency remains a terminal setting. / 环境变量覆盖启动偏好；深色主题继承终端背景，素笺使用象牙白背景，透明度仍由终端控制。

Community authors can distribute versioned JSON themes without executable code. See [theme author guide](../../docs/reference/TUI_THEMES.md) and [theme files](themes/). / 社区作者可发布版本化 JSON 主题，接口与示例见上述文档。
