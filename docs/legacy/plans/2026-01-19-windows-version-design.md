# Aether Windows Version Design

> Created: 2026-01-19
> Status: Approved
> Author: Claude (brainstorming session)

## Executive Summary

This document outlines the design for Aether's Windows version, targeting **feature parity** with the macOS version. The implementation uses **C# + WinUI 3** for the UI layer, with **csbindgen** for Rust FFI integration.

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | C# + WinUI 3 | csbindgen maturity, development efficiency, MVVM ecosystem |
| FFI Solution | csbindgen (Cysharp) | Production-grade, auto-generated P/Invoke |
| Min Windows Version | Windows 10 21H2+ | Balance coverage (~95%) and modern features |
| Halo Window | WinUI 3 + Win32 Extension | Unified tech stack with P/Invoke for WS_EX_NOACTIVATE |
| Hotkey Implementation | Low-level keyboard hook | Support double-tap Shift detection |
| Halo Trigger | Double-tap Shift | Consistent with macOS |
| Conversation Trigger | Win + Alt + / | Maps to macOS Cmd+Option+/ |
| Goal | Feature parity | Full macOS functionality on Windows |
| Start Strategy | POC first | Validate key technical risks |

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Windows Application                           │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                 C# / WinUI 3 UI Layer                      │  │
│  │  ├─ HaloWindow (no-focus transparent window)               │  │
│  │  ├─ SettingsWindow (settings UI)                           │  │
│  │  ├─ ConversationWindow (multi-turn chat)                   │  │
│  │  └─ Services (hotkey, clipboard, tray)                     │  │
│  └──────────────────────────┬────────────────────────────────┘  │
│                             │ P/Invoke                           │
│  ┌──────────────────────────┴────────────────────────────────┐  │
│  │              AetherCore.cs (csbindgen generated)           │  │
│  │  ├─ NativeMethods.g.cs (auto-generated P/Invoke)          │  │
│  │  └─ CallbackBridge.cs (callback adapter)                  │  │
│  └──────────────────────────┬────────────────────────────────┘  │
└─────────────────────────────┼────────────────────────────────────┘
                              │ C ABI
┌─────────────────────────────┴────────────────────────────────────┐
│                    Rust Core (aethecore.dll)                     │
│  ├─ ffi_cabi.rs (Windows C ABI exports)                         │
│  ├─ Business logic (router, AI calls, Memory, MCP...)           │
│  └─ Platform adaptation (system info, file permissions)         │
└─────────────────────────────────────────────────────────────────┘
```

### Core Principles

- **UI Layer**: Pure presentation and user interaction, no business logic
- **FFI Layer**: csbindgen auto-generated, hand-written high-level wrapper
- **Rust Core**: All business logic, platform differentiation via feature flags

## Directory Structure

```
platforms/windows/
├── Aether.sln                          # Visual Studio solution
├── Aether/
│   ├── Aether.csproj                   # .NET 8 + WinUI 3 project
│   ├── app.manifest                    # Windows manifest (DPI, permissions)
│   │
│   ├── App.xaml                        # Application entry
│   ├── App.xaml.cs                     # Lifecycle management (≈ AppDelegate)
│   │
│   ├── Windows/                        # Window definitions
│   │   ├── HaloWindow.xaml(.cs)        # Halo floating window
│   │   ├── SettingsWindow.xaml(.cs)    # Settings window
│   │   └── ConversationWindow.xaml(.cs)# Multi-turn conversation
│   │
│   ├── Views/                          # XAML views
│   │   ├── Halo/                       # Halo state views
│   │   ├── Settings/                   # Settings pages (10+ tabs)
│   │   └── Conversation/               # Conversation views
│   │
│   ├── ViewModels/                     # MVVM ViewModels
│   │   ├── HaloViewModel.cs
│   │   ├── SettingsViewModel.cs
│   │   └── ConversationViewModel.cs
│   │
│   ├── Services/                       # Platform services
│   │   ├── HotkeyService.cs            # Global hotkey (low-level hook)
│   │   ├── ClipboardService.cs         # Clipboard operations
│   │   ├── KeyboardSimulator.cs        # Input simulation (SendInput)
│   │   ├── TrayIconService.cs          # System tray
│   │   ├── WindowTracker.cs            # Active window detection
│   │   └── CaretTracker.cs             # Caret position (UIAutomation)
│   │
│   ├── Interop/                        # Rust FFI
│   │   ├── NativeMethods.g.cs          # csbindgen auto-generated
│   │   ├── AetherCore.cs               # High-level wrapper
│   │   └─ CallbackBridge.cs            # Callback adapter
│   │
│   ├── Models/                         # Data models
│   ├── Helpers/                        # Utilities
│   └── Resources/                      # Resources
│       ├── Strings/                    # Localization
│       └── Styles/                     # XAML styles
│
└── Aether.Tests/                       # Unit test project
    └── Aether.Tests.csproj
```

### macOS Mapping

| macOS (Swift) | Windows (C#) |
|---------------|--------------|
| AppDelegate.swift | App.xaml.cs |
| Sources/Window/ | Windows/ |
| Sources/Components/ | Views/ |
| Sources/Services/ | Services/ |
| Sources/Generated/ | Interop/ |

## FFI Data Flow & Callback Mechanism

```
┌─────────────────────────────────────────────────────────────────┐
│                        C# UI Layer                               │
│                                                                  │
│  ┌─────────────┐    ┌──────────────┐    ┌─────────────────────┐ │
│  │ HaloWindow  │───▶│ AetherCore   │───▶│ DispatcherQueue     │ │
│  │             │    │ .Process()   │    │ (UI thread dispatch) │ │
│  └─────────────┘    └──────┬───────┘    └──────────┬──────────┘ │
│                            │                        │            │
└────────────────────────────┼────────────────────────┼────────────┘
                             │ P/Invoke               │ Callback
                             ▼                        │
┌────────────────────────────────────────────────────┐│
│                  Rust Core                          ││
│                                                     ││
│  aether_process(input, context, callbacks) ────────┘│
│       │                                             │
│       ▼                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌────────────┐  │
│  │ Router      │─▶│ AI Provider │─▶│ Streaming  │  │
│  └─────────────┘  └─────────────┘  └─────┬──────┘  │
│                                          │         │
│       callbacks.on_stream_chunk(text) ◀──┘         │
│       callbacks.on_complete(result)                │
│       callbacks.on_error(message)                  │
└────────────────────────────────────────────────────┘
```

### Callback Implementation

```csharp
// Interop/CallbackBridge.cs
public static class CallbackBridge
{
    private static AetherCore? _instance;

    // Must keep delegate reference to prevent GC collection
    private static readonly StreamCallback _onStream = OnStreamChunk;
    private static readonly CompleteCallback _onComplete = OnComplete;

    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnStreamChunk(byte* text)
    {
        var str = Marshal.PtrToStringUTF8((IntPtr)text);

        // Critical: dispatch from Rust thread to UI thread
        _instance?.DispatcherQueue.TryEnqueue(() =>
        {
            _instance.OnStreamReceived?.Invoke(str ?? "");
        });
    }
}
```

### Threading Model

- Rust callbacks may fire on **any thread**
- Must use `DispatcherQueue.TryEnqueue` to dispatch to UI thread
- Delegates must be kept in static fields to prevent GC collection (crash otherwise)

## Halo Window Implementation

### Core Challenge

Create a no-focus, transparent, cursor-following floating window.

```csharp
// Windows/HaloWindow.xaml.cs
public sealed partial class HaloWindow : Window
{
    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_NOACTIVATE = 0x08000000;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WS_EX_TOPMOST = 0x00000008;

    [DllImport("user32.dll")]
    private static extern int GetWindowLong(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll")]
    private static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);

    public HaloWindow()
    {
        InitializeComponent();
        ConfigureWindow();
    }

    private void ConfigureWindow()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);

        int exStyle = GetWindowLong(hwnd, GWL_EXSTYLE);
        exStyle |= WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        SetWindowLong(hwnd, GWL_EXSTYLE, exStyle);

        ExtendsContentIntoTitleBar = true;
        SystemBackdrop = new TransparentBackdrop();
    }

    public void ShowAt(int x, int y)
    {
        AppWindow.Move(new PointInt32(x, y));
        AppWindow.Show();
        // Note: Do NOT call Activate() to avoid stealing focus
    }
}
```

### 21-State State Machine

```csharp
public enum HaloState
{
    Idle, Hidden, Appearing, Listening, Thinking, Processing,
    Streaming, Success, Error, Disappearing,
    MultiTurnActive, MultiTurnThinking, MultiTurnStreaming,
    ToolExecuting, ToolSuccess, ToolError,
    ClarificationNeeded, ClarificationReceived,
    AgentPlanning, AgentExecuting, AgentComplete
}
```

## Global Hotkey Service

### Double-tap Shift Detection (Low-level Keyboard Hook)

```csharp
// Services/HotkeyService.cs
public sealed class HotkeyService : IDisposable
{
    private const int WH_KEYBOARD_LL = 13;
    private const int WM_KEYDOWN = 0x0100;
    private const int WM_KEYUP = 0x0101;
    private const int VK_SHIFT = 0x10;
    private const int VK_LWIN = 0x5B;
    private const int VK_MENU = 0x12;  // Alt
    private const int VK_OEM_2 = 0xBF; // /

    private IntPtr _hookId = IntPtr.Zero;
    private readonly LowLevelKeyboardProc _proc;

    private DateTime _lastShiftUp = DateTime.MinValue;
    private const int DoubleClickMs = 400;

    public event Action? OnHaloHotkeyPressed;         // Double-tap Shift
    public event Action? OnConversationHotkeyPressed; // Win + Alt + /

    private IntPtr HookCallback(int nCode, IntPtr wParam, IntPtr lParam)
    {
        if (nCode >= 0)
        {
            int vkCode = Marshal.ReadInt32(lParam);

            // Double-tap Shift detection
            if (vkCode == VK_SHIFT && wParam == WM_KEYUP)
            {
                var now = DateTime.Now;
                if ((now - _lastShiftUp).TotalMilliseconds < DoubleClickMs)
                {
                    OnHaloHotkeyPressed?.Invoke();
                    _lastShiftUp = DateTime.MinValue;
                }
                else
                {
                    _lastShiftUp = now;
                }
            }

            // Win + Alt + / detection
            if (vkCode == VK_OEM_2 && wParam == WM_KEYDOWN)
            {
                if (IsKeyDown(VK_LWIN) && IsKeyDown(VK_MENU))
                {
                    OnConversationHotkeyPressed?.Invoke();
                }
            }
        }
        return CallNextHookEx(_hookId, nCode, wParam, lParam);
    }
}
```

### Hotkey Configuration

| Function | Default | Config Key |
|----------|---------|------------|
| Halo | Double-tap Shift | `shortcuts.halo` |
| Conversation | Win + Alt + / | `shortcuts.conversation` |
| Hide Halo | Escape | Hardcoded |

## Rust Core Windows Adaptation

### Module Organization

```rust
// core/src/services/system_info/mod.rs
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::MacOSSystemInfo as PlatformSystemInfo;
#[cfg(target_os = "windows")]
pub use windows::WindowsSystemInfo as PlatformSystemInfo;
```

### Windows System Info Implementation

```rust
// core/src/services/system_info/windows.rs
use windows::Win32::System::SystemInformation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct WindowsSystemInfo;

impl SystemInfoProvider for WindowsSystemInfo {
    fn get_total_memory(&self) -> Result<u64> {
        unsafe {
            let mut mem_info = MEMORYSTATUSEX::default();
            mem_info.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            GlobalMemoryStatusEx(&mut mem_info)?;
            Ok(mem_info.ullTotalPhys)
        }
    }

    fn get_active_window_info(&self) -> Result<Option<ActiveWindowInfo>> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0 == 0 { return Ok(None); }

            let mut title = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut title);
            let title = String::from_utf16_lossy(&title[..len as usize]);

            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let process_name = get_process_name(pid)?;

            Ok(Some(ActiveWindowInfo { title, process_name }))
        }
    }
}
```

### Cargo.toml Dependencies

```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = [
    "Win32_System_SystemInformation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_Foundation",
]}
```

## POC Validation Plan

### POC Items

| POC | Goal | Duration | Pass Criteria |
|-----|------|----------|---------------|
| POC-1: Halo Window | WinUI 3 no-focus transparent window | 4h | Focus not stolen when typing in VS Code |
| POC-2: Hotkey | Low-level keyboard hook | 3h | Triggers in any application |
| POC-3: Rust FFI | csbindgen callback validation | 5h | Rust callback updates C# UI correctly |

**Total: ~12h (1.5 days)**

### POC Project Structure

```
platforms/windows/
├── Aether.POC.sln
├── POC.HaloWindow/
├── POC.Hotkey/
└── POC.RustFFI/
```

### Post-POC Decision Matrix

| POC Result | Action |
|------------|--------|
| All 3 pass | Proceed to Phase 1 |
| Halo fails | Switch to Plan B (pure Win32 window) |
| FFI fails | Evaluate hand-written FFI or gRPC |

## Development Phases

### Phase 1: Foundation (P0)

| Task | File | Description |
|------|------|-------------|
| App framework | App.xaml.cs | Single instance, lifecycle |
| System tray | TrayIconService.cs | Context menu, icon states |
| Rust FFI | Interop/*.cs | Based on POC-3 |
| Global hotkey | HotkeyService.cs | Based on POC-2 |
| Halo window | HaloWindow.xaml | Based on POC-1 |
| Basic state machine | HaloViewModel.cs | 6 core states |

### Phase 2: Core Interaction (P0)

| Task | File | Description |
|------|------|-------------|
| Clipboard | ClipboardService.cs | Text, image, rich text |
| Keyboard simulation | KeyboardSimulator.cs | SendInput API |
| Full state machine | HaloViewModel.cs | Expand to 21 states |
| Streaming UI | HaloStreamingView.xaml | Typewriter effect |
| Tool execution UI | HaloToolView.xaml | Tool status display |

### Phase 3: Complete UI (P1)

| Task | File | Description |
|------|------|-------------|
| Settings framework | SettingsWindow.xaml | NavigationView layout |
| 10+ settings pages | Settings/*.xaml | Match macOS settings |
| Conversation window | ConversationWindow.xaml | Chat interface |
| Localization | Resources/Strings/ | en, zh-Hans |

### Phase 4: Advanced Features (P2)

| Task | File | Description |
|------|------|-------------|
| Caret position | CaretTracker.cs | UIAutomation |
| Screen capture | ScreenCaptureService.cs | Windows.Graphics.Capture |
| Startup | StartupService.cs | Registry Run key |
| Auto-update | UpdateService.cs | Check + download + install |

## Error Handling

```csharp
// Interop/AetherCore.cs
public class AetherException : Exception
{
    public int ErrorCode { get; }
    public AetherException(int code, string message) : base(message)
        => ErrorCode = code;
}

public void Process(string input, string? context)
{
    int result = NativeMethods.aether_process(...);

    if (result != 0)
    {
        throw result switch
        {
            -1 => new AetherException(result, "Invalid argument"),
            -2 => new AetherException(result, "Invalid UTF-8"),
            -3 => new AetherException(result, "Core not initialized"),
            _ => new AetherException(result, $"Unknown error: {result}")
        };
    }
}
```

## Testing Strategy

| Level | Tool | Coverage |
|-------|------|----------|
| Rust Core Unit | `cargo test` | Business logic |
| C# Unit | xUnit + Moq | ViewModel, Services |
| FFI Integration | xUnit | C# ↔ Rust calls |
| UI Automation | WinAppDriver | Window interactions |
| Manual | Checklist | Hotkey, focus, clipboard |

## Risks & Mitigation

| Risk | Probability | Mitigation |
|------|-------------|------------|
| Halo no-focus unstable | Medium | POC validation + Plan B backup |
| csbindgen callback issues | Low | POC validation + hand-written FFI backup |
| UIAutomation caret tracking complex | High | Defer to Phase 4, optional |

## Timeline

```
Week 1:   POC Validation (3 items)
          │
          ▼ POC Pass
Week 2-3: Phase 1 - Foundation
          │
          ▼
Week 4-5: Phase 2 - Core Interaction
          │
          ▼ MVP Ready
Week 6-8: Phase 3 - Complete UI
          │
          ▼
Week 9+:  Phase 4 - Advanced Features
```

## Appendix: macOS ↔ Windows API Mapping

| Function | macOS | Windows |
|----------|-------|---------|
| No-focus window | NSWindow + .canBecomeKey = false | WS_EX_NOACTIVATE |
| Global hotkey | CGEventTap | SetWindowsHookEx (WH_KEYBOARD_LL) |
| System tray | NSStatusBar | NotifyIcon |
| Clipboard | NSPasteboard | Clipboard (WinRT) |
| Input simulation | CGEvent | SendInput |
| Caret position | AXUIElement | UIAutomation |
| Screen capture | ScreenCaptureKit | Windows.Graphics.Capture |
| Startup | SMLoginItemSetEnabled | Registry Run key |
| Keychain | Security.framework | Windows Credential Manager |
