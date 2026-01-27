# Debugging Guide

This guide covers debugging techniques for Aether across Rust core and Swift UI layers.

## Rust Core Debugging

### Enable Verbose Logging

Set `RUST_LOG` environment variable to control log levels:

```bash
# Debug level (most verbose)
RUST_LOG=debug cargo run

# Info level
RUST_LOG=info cargo run

# Module-specific logging
RUST_LOG=aether::router=debug,aether::providers=info cargo run

# Trace level (extremely verbose, includes tokio internals)
RUST_LOG=trace cargo run
```

---

### Check UniFFI Bindings Generation

Verify UniFFI bindings are generated correctly:

```bash
cd Aether/core/

# Generate Swift bindings manually
cargo run --bin uniffi-bindgen generate src/aether.udl --language swift

# Output should appear in: ../Sources/Generated/aether.swift

# Check for errors
cargo run --bin uniffi-bindgen generate src/aether.udl --language swift 2>&1 | grep -i error
```

**Common Issues:**
- **Missing types in .udl**: Ensure all types used in interface are defined
- **Mismatched signatures**: Rust implementation must match .udl exactly
- **Missing namespace**: Every .udl file needs `namespace aether { ... }`

---

### Debugging Rust Tests

```bash
cd Aether/core/

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_router_basic -- --nocapture

# Run tests with backtrace on panic
RUST_BACKTRACE=1 cargo test

# Run tests with full backtrace
RUST_BACKTRACE=full cargo test

# Run ignored tests (e.g., integration tests)
cargo test -- --ignored
```

---

### Debugging Async Code

Tokio runtime debugging:

```bash
# Enable tokio console (requires tokio-console dependency)
RUSTFLAGS="--cfg tokio_unstable" cargo build

# Run with console support
RUST_LOG=tokio=trace cargo run
```

**Debugging Tips:**
- Use `tokio::time::sleep(Duration::from_secs(1))` to add delays for debugging
- Add `println!` statements (visible in Xcode console when running from Xcode)
- Use `dbg!()` macro for quick value inspection

---

### Memory Leaks and Performance

```bash
# Run with Valgrind (Linux)
valgrind --leak-check=full ./target/debug/aether

# macOS Instruments
# Build release binary first
cargo build --release
# Then profile with Xcode Instruments

# Profile with cargo-flamegraph
cargo install flamegraph
cargo flamegraph --bin aether
```

---

## Swift Debugging

### Print Debugging

Add debug prints in `EventHandler.swift`:

```swift
func onStateChanged(state: ProcessingState) {
    print("[Aether] State changed to: \(state)")
    print("[Aether] Thread: \(Thread.current)")
    print("[Aether] Timestamp: \(Date())")

    DispatchQueue.main.async {
        self.haloWindow?.setState(state)
    }
}
```

**Best Practices:**
- Prefix logs with `[Aether]` for easy filtering
- Include timestamp and thread info for async debugging
- Use `#function` to print current function name

---

### Xcode Debugger

**Breakpoint Debugging:**

1. Open Xcode: `open Aether.xcodeproj`
2. Navigate to `EventHandler.swift` or target file
3. Click line number to set breakpoint
4. Run app with Cmd+R
5. Trigger breakpoint by pressing global hotkey

**LLDB Commands:**

```lldb
# Print variable value
(lldb) po haloWindow

# Print expression
(lldb) p state == .listening

# Step over (next line)
(lldb) next

# Step into (enter function)
(lldb) step

# Continue execution
(lldb) continue

# Print all local variables
(lldb) frame variable
```

---

### SwiftUI View Hierarchy Debugging

**Xcode View Debugger:**

1. Run app with Cmd+R
2. Open Settings window
3. Click "Debug View Hierarchy" button (3D cube icon in Xcode toolbar)
4. Inspect view tree, constraints, and layout

**Preview Debugging:**

Add `#Preview` macros to SwiftUI views for live preview:

```swift
#Preview("Light Mode") {
    SettingsView()
        .frame(width: 800, height: 600)
        .preferredColorScheme(.light)
}

#Preview("Dark Mode") {
    SettingsView()
        .frame(width: 800, height: 600)
        .preferredColorScheme(.dark)
}
```

---

## FFI Boundary Issues

### Debugging Swift ↔ Rust Communication

**Check Library Loading:**

```bash
# Verify library is in Frameworks directory
ls -lh Aether/Frameworks/libaethecore.dylib

# Check library dependencies
otool -L Aether/Frameworks/libaethecore.dylib

# Expected output should include:
# @rpath/libaethecore.dylib
# /usr/lib/libSystem.B.dylib
```

**Verify Xcode Build Settings:**

1. Open Xcode project
2. Select Aether target → Build Settings
3. Search for "Runpath Search Paths"
4. Ensure `@executable_path/../Frameworks` is present

---

### Common FFI Errors

**Error: "Symbol not found"**

```
dyld: Symbol not found: _aether_core_new
```

**Solution:**
- Rebuild Rust library: `cd Aether/core && cargo build`
- Regenerate UniFFI bindings: `cargo run --bin uniffi-bindgen generate src/aether.udl --language swift`
- Clean Xcode build: Cmd+Shift+K, then rebuild

---

**Error: "Library not loaded"**

```
dyld: Library not loaded: @rpath/libaethecore.dylib
Reason: image not found
```

**Solution:**
- Copy library to Frameworks: `cp Aether/core/target/debug/libaethecore.dylib Aether/Frameworks/`
- Verify Runpath Search Paths in Xcode Build Settings
- Regenerate Xcode project: `xcodegen generate`

---

**Error: "Type mismatch in FFI call"**

Swift crashes when calling Rust function with error:
```
EXC_BAD_ACCESS (code=1, address=0x0)
```

**Solution:**
- Check .udl interface definition matches Rust implementation exactly
- Ensure all types are properly bridged (no `Option<>` without `?` in .udl)
- Add debug prints in Rust to verify function is called:

```rust
pub fn process_clipboard(&self) -> Result<String> {
    println!("[Rust] process_clipboard called");
    // ... implementation
}
```

---

## Performance Debugging

### Identifying Slow Operations

**Rust Core:**

```rust
use std::time::Instant;

let start = Instant::now();
// ... operation
let duration = start.elapsed();
println!("Operation took: {:?}", duration);
```

**Swift UI:**

```swift
let start = Date()
// ... operation
let duration = Date().timeIntervalSince(start)
print("Operation took: \(duration)s")
```

---

### Profiling with Instruments

1. Build release binary: `xcodebuild -project Aether.xcodeproj -scheme Aether -configuration Release build`
2. Open Instruments: `open /Applications/Xcode.app/Contents/Applications/Instruments.app`
3. Select "Time Profiler" template
4. Choose Aether.app as target
5. Record and trigger hotkey to capture profile

**Look for:**
- Hotkey detection latency (should be <10ms)
- Clipboard read latency (should be <50ms)
- Halo render latency (should be <16ms for 60fps)
- AI provider response time (varies by provider)

---

## Logging and Crash Reports

### View Application Logs

**Console.app (macOS):**

1. Open Console.app
2. Filter by process: `Aether`
3. Filter by subsystem: `com.aether.app`

**Command Line:**

```bash
# View Aether logs in real-time
log stream --predicate 'process == "Aether"' --level debug

# View crash logs
ls ~/Library/Logs/DiagnosticReports/Aether*

# View specific crash log
cat ~/Library/Logs/DiagnosticReports/Aether_2024-12-30_crash.ips
```

---

### Custom Logging

**Rust Core:**

```rust
use log::{debug, info, warn, error};

info!("Hotkey pressed: {:?}", key);
debug!("Clipboard content: {} bytes", content.len());
warn!("API request timeout, retrying...");
error!("Failed to initialize core: {}", e);
```

**Swift:**

```swift
import os.log

let logger = Logger(subsystem: "com.aether.app", category: "EventHandler")

logger.debug("State changed to \(state.rawValue)")
logger.info("Halo window shown at \(position)")
logger.error("Failed to initialize Rust core: \(error)")
```

---

## Common Issues and Solutions

### Issue: Halo Window Not Appearing

**Debugging Steps:**

1. Check if `onHaloShow` callback is triggered:
   ```swift
   func onHaloShow(position: HaloPosition, providerColor: String?) {
       print("[DEBUG] onHaloShow called: x=\(position.x), y=\(position.y)")
       // ...
   }
   ```

2. Verify window level is set correctly:
   ```swift
   haloWindow.level = .floating  // Must be above all apps
   ```

3. Check window collection behavior:
   ```swift
   haloWindow.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
   ```

---

### Issue: Focus Stolen from Active App

**Debugging Steps:**

1. Ensure window never activates:
   ```swift
   // NEVER call this for Halo window:
   // haloWindow.makeKeyAndOrderFront(nil)

   // Use instead:
   haloWindow.orderFrontRegardless()
   ```

2. Verify click-through behavior:
   ```swift
   haloWindow.ignoresMouseEvents = true
   ```

---

### Issue: Config Changes Not Persisting

**Debugging Steps:**

1. Check config file path:
   ```bash
   cat ~/.aether/config.toml
   ```

2. Verify write permissions:
   ```bash
   ls -l ~/.aether/
   ```

3. Add debug logging in config module:
   ```rust
   println!("Writing config to: {:?}", config_path);
   ```

---

## Related Documentation

- See `docs/TESTING_GUIDE.md` for automated testing strategies
- See `docs/PERFORMANCE_GUIDE.md` for performance optimization tips
- See `Aether/core/src/logging/` for logging implementation
