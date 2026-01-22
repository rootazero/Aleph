# Windows Platform - ARCHIVED

> **Status**: ARCHIVED (January 2025)
> **Reason**: Development focus shifted to macOS native and Tauri cross-platform strategy

## Archive Notice

This Windows platform implementation has been archived and is no longer actively maintained.

### Why Archived?

1. **Tauri Strategy**: Cross-platform support (Windows, Linux) is now handled via Tauri instead of native C#/WinUI 3
2. **Resource Focus**: Development resources concentrated on macOS native experience and Tauri cross-platform
3. **POC Complete**: The proof-of-concept work here validated technical feasibility

### What's Here?

This directory contains proof-of-concept implementations:

- `POC.HaloWindow/` - WinUI 3 transparent window experiments
- `POC.Hotkey/` - Global hotkey registration
- `POC.RustFFI/` - Rust-to-C# FFI via csbindgen
- `Aether/` - Main application skeleton

### Migration Path

For Windows/Linux support, use the Tauri implementation:

```bash
cd platforms/tauri
pnpm install
pnpm tauri dev
```

### Historical Reference

This code may be useful as reference for:
- WinUI 3 transparent window implementation
- Windows global hotkey patterns
- csbindgen FFI patterns

### Do Not

- Do not add new features to this directory
- Do not fix bugs unless critical for reference purposes
- Do not update dependencies

---

*Archived by: Development Team*
*Date: January 2025*
