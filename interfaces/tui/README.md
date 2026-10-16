# Aleph TUI

Interactive terminal interface for Aleph. Connects to the Aleph Gateway over WebSocket using the shared protocol types. / Aleph 交互式终端界面，通过 WebSocket 和共享协议连接 Gateway。

This crate intentionally does not depend on `alephcore`; it is a pure client built on `aleph-client` and `aleph-protocol`. / 本 crate 不依赖 `alephcore`，仅使用共享客户端与协议。

## Native selection and fonts / 原生选择与字体

Press **F6** to release mouse capture and freeze the current frame for the host terminal's native text selection. Drag to select and use the terminal's platform copy shortcut (for example, **Ctrl+Shift+C** in Windows Terminal); press **F6** or **Esc** to resume. While selection mode is active, Aleph continues receiving gateway events, but application interaction and drawing are temporarily disabled. Only visible terminal cells can be selected: off-screen chat history is unavailable, and selection may include UI chrome or visual line wraps. Selection persistence, resize/reflow behavior, and copy key bindings belong to the terminal emulator. / 按 **F6** 释放鼠标捕获并冻结当前画面，使用终端自身的文本选择功能；拖动选择后使用平台复制快捷键（例如 Windows Terminal 的 **Ctrl+Shift+C**）；按 **F6** 或 **Esc** 恢复。选择模式期间 Aleph 仍会接收 Gateway 事件，但应用交互与绘制会临时禁用。只能选择当前可见的终端单元格，无法选择屏幕外的聊天历史，选择结果可能包含界面装饰或视觉换行。选区持久化、调整窗口大小/重排时的行为以及复制快捷键均由终端模拟器负责。

The TUI does not set a portable font face or size and does not emit OSC 50. By default it inherits the terminal. Customize it in the terminal profile: Windows Terminal **Profile > Appearance > Font face/size**, macOS Terminal **Settings > Profiles > Text > Font**, iTerm2 **Profile > Text**, or your Linux terminal emulator's profile settings. The TUI does not hard-code font dimensions, add a dependency, or modify the system/terminal profile. / TUI 不设置可移植的字体名称或字号，也不会发送 OSC 50；默认继承终端设置。请在终端 profile 中自定义：Windows Terminal **Profile > Appearance > Font face/size**，macOS Terminal **Settings > Profiles > Text > Font**，iTerm2 **Profile > Text**，Linux 终端模拟器则使用其 profile 设置。TUI 不硬编码字体尺寸、不新增依赖，也不修改系统或终端 profile。

## Themes / 主题

Amber is the default; Tide, Jade, Iris and Paper are also built in. `/theme` or `/theme list` opens a local picker; `/theme jade` selects and remembers the choice, and `/theme reload` reloads the active theme file. Picker feedback stays local and never enters chat history. / 默认琥珀，另有潮汐、竹影、鸢尾、素笺。`/theme` 或 `/theme list` 打开本地选择器；`/theme jade` 选择并记忆主题，`/theme reload` 重载当前主题文件。选择器反馈仅保留在本地，不写入聊天记录。

`ALEPH_TUI_THEME` overrides the startup preference. Dark themes inherit the terminal background; Paper supplies an ivory background. Terminal transparency remains a terminal setting. / 环境变量覆盖启动偏好；深色主题继承终端背景，素笺使用象牙白背景，透明度仍由终端控制。

Community authors can distribute versioned JSON themes without executable code. See [theme author guide](../../docs/reference/TUI_THEMES.md) and [theme files](themes/). / 社区作者可发布版本化 JSON 主题，接口与示例见上述文档。
