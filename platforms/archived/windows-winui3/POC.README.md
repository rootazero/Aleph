# Aleph Windows POC

This directory contains Proof-of-Concept projects to validate key technical decisions for the Windows version of Aleph.

## Prerequisites

- Windows 10 21H2+ or Windows 11
- .NET 8.0 SDK
- Visual Studio 2022 (or VS Code with C# extension)
- Windows App SDK 1.5+

## POC Projects

### POC-1: HaloWindow

**Purpose**: Validate that WinUI 3 can create a no-focus transparent floating window.

**Key Requirements**:
- Window appears without stealing focus from other applications
- Transparent background with blur effect
- Always on top
- No taskbar entry
- Can be positioned at cursor location

**Test Steps**:
1. Open the POC application
2. Open another application (e.g., Notepad) and start typing
3. Click "Show Halo" button
4. Verify that Halo appears but focus remains in Notepad
5. Continue typing - text should appear in Notepad, not be captured by Halo

**Pass Criteria**: Focus never leaves the original application when Halo is shown.

### POC-2: Hotkey

**Purpose**: Validate global hotkey detection using low-level keyboard hook.

**Key Requirements**:
- Detect double-tap Shift (within 400ms threshold)
- Detect Win + Alt + / combination
- Work regardless of which application has focus

**Test Steps**:
1. Open the POC application
2. Minimize it or switch to another application
3. Double-tap Shift key quickly
4. Press Win + Alt + / together
5. Return to POC and check if events were logged

**Pass Criteria**: Both hotkey combinations are detected when any application is focused.

### POC-3: RustFFI

**Purpose**: Validate Rust to C# FFI callback mechanism.

**Key Requirements**:
- Load alephcore.dll successfully
- Register C# callbacks with Rust
- Callbacks are invoked on correct thread (UI thread)
- No memory leaks or crashes

**Prerequisites**:
```bash
# Build Rust core with C ABI feature
cd ../../core
cargo build --release --features cabi

# Copy DLL to POC output directory
cp target/release/alephcore.dll ../platforms/windows/POC.RustFFI/bin/x64/Release/net8.0-windows10.0.22621.0/
```

**Test Steps**:
1. Build and run the POC
2. Click "Initialize Core"
3. Verify version is displayed
4. Click "Test Callbacks"
5. Verify callback messages appear in log

**Pass Criteria**:
- DLL loads without error
- Version retrieved successfully
- Callbacks registered without crash
- Cleanup completes without error

## Building

### From Command Line

```bash
# Build all POCs
cd platforms/windows
dotnet build Aleph.POC.sln -c Release

# Run individual POC
dotnet run --project POC.HaloWindow
dotnet run --project POC.Hotkey
dotnet run --project POC.RustFFI
```

### From Visual Studio

1. Open `Aleph.POC.sln`
2. Set startup project to desired POC
3. Press F5 to run

## Results

After running all POCs, document results:

| POC | Status | Notes |
|-----|--------|-------|
| HaloWindow | ⬜ Pending | |
| Hotkey | ⬜ Pending | |
| RustFFI | ⬜ Pending | |

### Decision Matrix

| All Pass | Action |
|----------|--------|
| ✅ | Proceed to Phase 1 development |
| HaloWindow fails | Switch to pure Win32 window approach |
| RustFFI fails | Evaluate hand-written FFI or gRPC alternative |
| Hotkey fails | Consider RegisterHotKey as fallback |

## Troubleshooting

### DLL Not Found (POC-3)

1. Ensure Rust is installed: `rustup --version`
2. Build the core: `cargo build --release --features cabi`
3. Copy DLL to output directory
4. Verify DLL architecture matches (x64)

### Hotkey Not Working (POC-2)

1. Run as Administrator (may be required for low-level hooks)
2. Check antivirus software (may block hooks)
3. Verify Windows Defender isn't blocking

### Halo Steals Focus (POC-1)

1. Check if WS_EX_NOACTIVATE is applied correctly
2. Verify ShowWindow uses SW_SHOWNOACTIVATE
3. Ensure Activate() is never called on Halo window
