# Aleph app icons / Aleph 应用图标

- `aleph.png`: the original 1024 px full-bleed artwork, used by the Windows and Linux packages. / 原版 1024 px 满幅图，用于 Windows 和 Linux 打包。
- `aleph-macos.png`: the 1024 px rendition from the former desktop app’s `.icns`, with the artwork inset to 824 px. Keep the transparent margin when updating it: macOS does not add it. / 从旧桌面应用的 `.icns` 提取的 1024 px 图，主体占 824 px；重绘时保留透明边距，macOS 不会自动留白。
- `../assets/aleph-tray.png`: the original 32 px artwork for the macOS menu bar, Windows notification area, and Linux tray. / 用于 macOS 菜单栏、Windows 通知区域和 Linux 托盘的原版 32 px 图。

`nsh.json` gives the app icons to `nsh package`. The tray asset is registered by the Rust host. iOS keeps its opaque AppIcon in `mobile/ios/AlephPaneliOS/Resources/Assets.xcassets`.

应用图标由 `nsh.json` 交给 `nsh package`，托盘图标由 Rust 宿主注册。iOS 继续使用 `mobile/ios/AlephPaneliOS/Resources/Assets.xcassets` 中的不透明 AppIcon。
