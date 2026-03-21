# Aleph Linux Desktop Extension

Linux native desktop capabilities for Aleph, providing OS-level integration.

## Status

Not yet implemented. Will gradually replace the Tauri-based desktop shell (`apps/tauri/`).

## Planned Tech Stack

- **Language:** Rust
- **Display Server:** Wayland (primary) + X11 (fallback)
- **System Integration:** D-Bus for inter-process communication
- **Notifications:** libnotify / D-Bus org.freedesktop.Notifications
- **Tray:** StatusNotifierItem (SNI) protocol via D-Bus

## Planned Capabilities

- System tray / status icon
- Desktop notifications
- Window management (focus, position, size)
- Screenshot capture
- Clipboard access
- Global hotkey registration
- File system monitoring (inotify)

## Architecture

This crate implements the `DesktopCapability` trait defined in `crates/desktop/`.
The core never calls Linux APIs directly — it dispatches through the trait (R1: Brain-Limb Separation).

```
core (brain) ──trait──→ crates/desktop/ (contract) ←──impl──── apps/linux/ (muscle)
```
