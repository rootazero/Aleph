# Windows WinUI3 Client Archive

Archived: 2026-02-23
Source commit: 70092556~1 (last commit before deletion)
Original path: platforms/windows/

## What's Here

Windows native client (WinUI3 / C# / .NET) removed from Aleph when the platform strategy shifted to macOS native + Tauri cross-platform. The project was originally named "Aether" before the rename to "Aleph".

## Module Inventory

| Module | Files | Description |
|--------|-------|-------------|
| `Aether/` | 71 | Main WinUI3 app — Views, ViewModels, Services, Interop, Assets |
| `Aether.Tests/` | 1 | Test project |
| `POC.HaloWindow/` | 7 | Proof-of-concept: Halo overlay window |
| `POC.Hotkey/` | 7 | Proof-of-concept: Global hotkey registration |
| `POC.RustFFI/` | 7 | Proof-of-concept: Rust FFI via C ABI |
| Root | 4 | Solution files, POC README, ARCHIVED.md |

## Key Components

- **Interop/AetherCore.cs** — Rust FFI bridge via C ABI (DllImport)
- **Services/** — HotkeyService, ClipboardService, CursorService, ScreenCaptureService, TrayIconService, KeyboardSimulator
- **Views/Halo/** — Halo streaming overlay UI
- **Views/Settings/** — Full settings UI (12 pages)
- **Windows/** — HaloWindow, ConversationWindow, SettingsWindow, InitializationDialog

## Reuse Notes

- Interop pattern (AetherCore.cs + NativeMethods.g.cs) is reusable for any Rust-to-WinUI3 bridge
- HotkeyService implements global hotkey registration via Win32 API
- ScreenCaptureService uses Windows.Graphics.Capture API
- Settings pages show complete WinUI3 settings architecture
