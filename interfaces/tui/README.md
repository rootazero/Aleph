# Aleph TUI

Interactive terminal interface for Aleph. Connects to the Aleph Gateway over WebSocket using the shared protocol types. / Aleph 交互式终端界面，通过 WebSocket 和共享协议连接 Gateway。

This crate intentionally does not depend on `alephcore`; it is a pure client built on `aleph-client` and `aleph-protocol`. / 本 crate 不依赖 `alephcore`，仅使用共享客户端与协议。

## Themes / 主题

Amber is the default; Tide, Jade, Iris and Paper are also built in. `/theme` or `/theme list` opens a local picker; `/theme jade` selects and remembers the choice, and `/theme reload` reloads the active theme file. Picker feedback stays local and never enters chat history. / 默认琥珀，另有潮汐、竹影、鸢尾、素笺。`/theme` 或 `/theme list` 打开本地选择器；`/theme jade` 选择并记忆主题，`/theme reload` 重载当前主题文件。选择器反馈仅保留在本地，不写入聊天记录。

`ALEPH_TUI_THEME` overrides the startup preference. Dark themes inherit the terminal background; Paper supplies an ivory background. Terminal transparency remains a terminal setting. / 环境变量覆盖启动偏好；深色主题继承终端背景，素笺使用象牙白背景，透明度仍由终端控制。

Community authors can distribute versioned JSON themes without executable code. See [theme author guide](../../docs/reference/TUI_THEMES.md) and [theme files](themes/). / 社区作者可发布版本化 JSON 主题，接口与示例见上述文档。
