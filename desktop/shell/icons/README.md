# App Icons / 应用图标

## ⚠️ macOS dock icon needs safe-area padding / macOS Dock 图标必须留白

macOS draws `.icns` artwork **as-is** — it does not auto-inset or mask the icon
(unlike iOS). Apple's macOS icon grid expects the rounded-square body to occupy
only **824 / 1024 ≈ 80 %** of the canvas, with ~100 px transparent margin on each
side. A **full-bleed** icon (art touching all four edges) renders visibly *larger*
than native dock apps (Finder, Safari, …) — this looked like "the dock icon got
bigger after opening the app".

`icon.icns` is therefore generated **with** that padding and is the file the macOS
dock uses (`CFBundleIconFile`). The other assets are intentionally left full-bleed
(`32x32.png` / `128x128*.png` / `icon.ico` — Windows taskbar & Linux want full-bleed).

macOS 渲染 `.icns` 是**原样绘制**，不像 iOS 会自动缩进/裁切。Apple 网格要求图标主体只占
画布 **824/1024 ≈ 80%**，四周各留 ~100px 透明边距。满幅图标（顶到四边）会比原生 Dock
app 明显**偏大**，即"打开后 Dock 图标变大"的现象。故 `icon.icns` 已带留白；其余 PNG/ICO
故意保持满幅（Windows 任务栏 / Linux 需要满幅）。

## ❌ Do NOT regenerate blindly / 不要盲目重生成

`source.png` is the **full-bleed** master. Running `cargo tauri icon source.png`
regenerates every icon (incl. `icon.icns`) full-bleed and **reintroduces the
oversized macOS dock icon**. If you must regenerate, re-pad `icon.icns` afterwards.

`source.png` 是**满幅**母版。`cargo tauri icon source.png` 会把所有图标（含 `icon.icns`）
重生成为满幅，**会把超大 Dock 图标的 bug 带回来**。若必须重生成，事后请重新给 `icon.icns` 加留白。

## How icon.icns was padded / icon.icns 的留白生成步骤

Composite the full-bleed art onto a 1024 transparent canvas at 824×824 centered
(100 px margins), then build the `.icns` (macOS-only tools):

```bash
# 1. Pad: draw source.png into a 824×824 box centered on a 1024 transparent canvas
#    (use CoreGraphics / any compositor that preserves alpha; sips --padColor
#     cannot produce a TRANSPARENT margin).
# 2. Downscale the padded 1024 master into an .iconset (16→1024, @1x/@2x) via sips.
# 3. iconutil -c icns Aleph.iconset -o icon.icns
```

Effect takes hold only after a macOS rebuild — `just shell-build` (icons are
embedded at Tauri bundle time). / 图标在 Tauri 打包时嵌入，改完需 `just shell-build` 才生效。
