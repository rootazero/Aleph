# Aleph Windows Desktop Extension

Windows native desktop capabilities for Aleph, providing OS-level integration.

## Status

Not yet implemented. Will gradually replace the Tauri-based desktop shell (`apps/tauri/`).

## Planned Tech Stack

- **Language:** Rust
- **Windows API:** windows-rs (official Microsoft Rust bindings)
- **UI Framework:** WinRT for modern UI integration
- **Notifications:** Windows Toast Notifications via WinRT
- **Tray:** System tray via Shell_NotifyIcon

## Planned Capabilities

- System tray icon
- Desktop notifications (Toast)
- Window management (Win32 API)
- Screenshot capture (Desktop Duplication API / GDI+)
- Clipboard access (Win32 Clipboard API)
- Global hotkey registration (RegisterHotKey)
- File system monitoring (ReadDirectoryChangesW)

## Architecture

This crate implements the `DesktopCapability` trait defined in `crates/desktop/`.
The core never calls Windows APIs directly — it dispatches through the trait (R1: Brain-Limb Separation).

```
core (brain) ──trait──→ crates/desktop/ (contract) ←──impl──── apps/windows/ (muscle)
```
