# Desktop as Muscle: Server-Driven Desktop Capability Architecture

> *Desktop 不是壳，是肌肉。Server 才是完整的生命体。*

**Date**: 2026-03-01
**Status**: Approved
**Scope**: Desktop capability architecture refactoring — from out-of-process Bridge to in-process crate

---

## 1. Problem Statement

### Current Model: Desktop as Shell

```
Tauri App (outer shell, IS the "application")
  ├── WebView (UI)
  ├── Tray icon
  ├── Bridge UDS Server (desktop capabilities)
  └── spawns / connects to aleph-server
```

**Problems**:
- Tauri is the "owner", Server is the "wrapped organ" — inverted master-slave relationship
- Desktop capabilities require a separate process (IPC overhead, lifecycle management)
- Closing Tauri kills the Server — fragile coupling
- Cannot run as pure background daemon without Tauri
- Desktop capabilities are locked behind the Tauri shell

### Inspiration: OpenClaw Comparison

OpenClaw uses a **distributed Node architecture** where Gateway is a pure router and Nodes are capability hosts. Key insight: their "headless node" can run `system.run` without any UI — desktop capabilities and UI are independent concerns.

Aleph should go further: desktop capabilities should be **compiled into the server** as a feature, not mediated through a separate process.

---

## 2. New Model: Desktop as Muscle

```
aleph-server (single daemon process)
  ├── Gateway (WS)
  ├── Agent Loop
  ├── Memory / Dispatcher / ...
  └── Desktop capabilities (feature = "desktop")
      ├── xcap (screenshot, cross-platform)
      ├── enigo (click/type, cross-platform)
      ├── OCR (platform-specific)
      └── AX tree (platform-specific)

No Tauri. No window. No tray. Just a daemon.
```

**Relationship change**: Desktop is an **optional feature** of the Server, not a separate process.

This aligns with the 1-2-3-4 architecture's "3 Limbs" definition:
- **Native capabilities (The Muscles)** — compiled directly into Server
- No longer "remote controlling" through IPC — muscles are **attached to the body**

### R1 Compliance: Crate-Level Separation

R1 (Brain-Limb Separation) is preserved by shifting the separation boundary from **process-level** to **crate-level**:

```
alephcore (crate)          ← Pure logic, zero platform APIs ✅ R1 compliant
  └── defines traits: DesktopCapability, ScreenCapture, InputControl

aleph-desktop (new crate)  ← Platform implementations: xcap/enigo/Vision/WinRT
  └── impl DesktopCapability for NativeDesktop

aleph-server (bin)         ← Assembly point, feature gated
  └── #[cfg(feature = "desktop")]
      use aleph_desktop::NativeDesktop;
```

Core's traits remain unchanged. Implementation moves from "out-of-process Bridge" to "in-process crate". The spirit of R1 (core never touches platform APIs) is fully preserved.

---

## 3. `aleph-desktop` Crate Design

### 3.1 Crate Structure

```
crates/
  └── desktop/                      # aleph-desktop crate
      ├── Cargo.toml
      └── src/
          ├── lib.rs                 # pub trait + facade
          ├── perception/
          │   ├── mod.rs
          │   ├── screenshot.rs      # xcap (all platforms)
          │   ├── ocr.rs             # macOS: Vision, Windows: WinRT, Linux: tesseract
          │   └── ax_tree.rs         # macOS: AX API, Windows: UI Automation
          ├── action/
          │   ├── mod.rs
          │   ├── input.rs           # enigo (all platforms)
          │   ├── window.rs          # Window list/focus (platform-specific)
          │   └── app.rs             # App launch (platform-specific)
          └── platform/
              ├── mod.rs
              ├── macos.rs           # #[cfg(target_os = "macos")]
              ├── windows.rs         # #[cfg(target_os = "windows")]
              └── linux.rs           # #[cfg(target_os = "linux")]
```

### 3.2 Core Trait (defined in alephcore)

```rust
// core/src/desktop/traits.rs — pure contract, zero dependencies
#[async_trait]
pub trait DesktopCapability: Send + Sync {
    /// List currently available capabilities
    fn capabilities(&self) -> Vec<Capability>;

    /// Take screenshot
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;

    /// OCR on image bytes
    async fn ocr(&self, image: &[u8]) -> Result<OcrResult>;

    /// Click at coordinates
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;

    /// Type text
    async fn type_text(&self, text: &str) -> Result<()>;

    /// Key combination
    async fn key_combo(&self, keys: &[Key]) -> Result<()>;

    /// List visible windows
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;

    /// Focus a window by ID
    async fn focus_window(&self, window_id: &str) -> Result<()>;

    /// Launch an application
    async fn launch_app(&self, app_id: &str) -> Result<()>;

    /// Get accessibility tree
    async fn ax_tree(&self, app_id: Option<&str>) -> Result<AxTreeNode>;
}
```

### 3.3 Implementation (in aleph-desktop)

```rust
// crates/desktop/src/lib.rs
pub struct NativeDesktop {
    // Platform-specific internal state
}

impl NativeDesktop {
    pub fn new() -> Result<Self> { ... }
}

#[async_trait]
impl DesktopCapability for NativeDesktop {
    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = vec![
            Capability::new("screen_capture", "1.0"),
            Capability::new("keyboard_control", "1.0"),
            Capability::new("mouse_control", "1.0"),
            Capability::new("window_list", "1.0"),
        ];

        #[cfg(target_os = "macos")]
        {
            caps.push(Capability::new("ocr", "1.0"));      // Vision framework
            caps.push(Capability::new("ax_inspect", "1.0")); // AX API
        }

        #[cfg(target_os = "windows")]
        {
            caps.push(Capability::new("ocr", "1.0"));      // WinRT
            caps.push(Capability::new("ax_inspect", "1.0")); // UI Automation
        }

        caps
    }

    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        // xcap — unified cross-platform implementation
        ...
    }
    // ...
}
```

### 3.4 Server Integration

```rust
// server/src/main.rs
#[cfg(feature = "desktop")]
use aleph_desktop::NativeDesktop;

async fn main() {
    let core = AlephCore::builder()
        .with_gateway(config.gateway)
        .with_memory(config.memory);

    #[cfg(feature = "desktop")]
    let core = core.with_desktop(NativeDesktop::new()?);

    let core = core.build().await?;
    core.run_as_daemon().await;
}
```

### 3.5 BuiltinTool Integration

`DesktopTool` changes from IPC-based to trait-based invocation:

```rust
// core/src/builtin_tools/desktop.rs
impl DesktopTool {
    async fn call(&self, args: DesktopArgs, ctx: &ToolContext) -> Result<DesktopOutput> {
        // Approval check (unchanged)
        self.check_approval(&args).await?;

        // Direct trait call — no IPC
        let desktop = ctx.desktop_capability()
            .ok_or(DesktopError::NotAvailable)?;

        match args.action {
            Action::Screenshot { region } => {
                let result = desktop.screenshot(region).await?;
                Ok(DesktopOutput::screenshot(result))
            }
            Action::Click { x, y, button } => {
                desktop.click(x, y, button).await?;
                Ok(DesktopOutput::success("Clicked"))
            }
            // ...
        }
    }
}
```

### 3.6 macOS Main Thread Handling

Some macOS APIs (CGEvent, AX API, Vision) require the main thread. Without Tauri's event loop, this needs explicit handling:

```rust
// crates/desktop/src/platform/macos.rs
#[cfg(target_os = "macos")]
mod macos {
    use dispatch::Queue;

    /// Execute on main thread (required for certain macOS APIs)
    pub fn run_on_main<F, T>(f: F) -> T
    where F: FnOnce() -> T + Send, T: Send {
        Queue::main().exec_sync(f)
    }
}
```

Most operations (xcap screenshots, enigo input) work on any thread. Only macOS Vision OCR and AX API need main thread dispatch.

---

## 4. Daemon Lifecycle

### 4.1 Process Management

```
macOS:     launchd plist → aleph-server --daemon
Linux:     systemd unit  → aleph-server --daemon
Windows:   Windows Service → aleph-server --daemon
Dev mode:  cargo run       → foreground
```

```rust
// server/src/daemon.rs
pub async fn run_as_daemon(config: DaemonConfig) -> Result<()> {
    // 1. Write PID file (~/.aleph/run/aleph.pid)
    write_pid_file(&config.pid_path)?;

    // 2. Register signal handlers
    let shutdown = signal_handler(); // SIGTERM, SIGINT

    // 3. Start all subsystems
    let core = AlephCore::builder()
        .with_gateway(config.gateway)
        .with_memory(config.memory);

    #[cfg(feature = "desktop")]
    let core = core.with_desktop(NativeDesktop::new()?);

    let core = core.build().await?;

    // 4. Run until shutdown signal
    tokio::select! {
        _ = core.serve() => {},
        _ = shutdown => {
            info!("Received shutdown signal, cleaning up...");
            core.shutdown().await;
        }
    }

    // 5. Cleanup PID file
    remove_pid_file(&config.pid_path)?;
    Ok(())
}
```

### 4.2 Build System

```toml
# apps/server/Cargo.toml
[dependencies]
alephcore = { path = "../../core" }

[dependencies.aleph-desktop]
path = "../../crates/desktop"
optional = true

[features]
default = ["gateway"]
desktop = ["aleph-desktop"]
desktop-ocr = ["desktop", "aleph-desktop/ocr"]
desktop-ax = ["desktop", "aleph-desktop/ax"]
control-plane = [...]
```

### 4.3 Build Commands

```bash
# Pure Server (no desktop, for remote/cloud deployment)
cargo build --bin aleph-server

# Server + desktop (local daemon mode)
cargo build --bin aleph-server --features desktop

# Server + desktop + OCR + AX (full local capabilities)
cargo build --bin aleph-server --features desktop,desktop-ocr,desktop-ax

# Server + desktop + Control Plane UI
cargo build --bin aleph-server --features desktop,control-plane

# Release
cargo build --bin aleph-server --features desktop,control-plane --release
```

---

## 5. Tauri's New Role

**Tauri is NOT deleted, but its role changes**:

```
Before: Tauri = Shell (wraps Server + provides desktop + provides UI)
After:  Tauri = Pure UI client (optional, only WebView container for Leptos)
        Desktop capabilities → moved to aleph-desktop crate
        Server → independent daemon
```

Two deployment modes:

```
Mode A (no UI):  aleph-server --features desktop → pure daemon, CLI/Bot interaction
Mode B (with UI): aleph-server --features desktop + Tauri client → connects via WS to Gateway
```

Tauri becomes a **thin client** that connects to the daemon's Gateway WebSocket, like any other client. This fully satisfies R5 (menu bar first) and R7 (one core, many shells).

---

## 6. Canvas / A2UI Strategy

Without Tauri's WebView, Canvas has three paths:

| Option | Description | Use Case |
|--------|-------------|----------|
| **Drop** | No Canvas in daemon mode | Pure background mode |
| **Optional WebView** | Canvas as optional feature, launch standalone WebView process when needed | Occasional display |
| **Terminal UI** | Render with ratatui in terminal | CLI users |

**Decision**: Drop Canvas in Phase 1-2. Reconsider as optional capability in Phase 3.

---

## 7. Migration Strategy

### Phase 1: Extract — Move implementations to standalone crate

- Extract screenshot/OCR/AX from `apps/desktop/src-tauri/src/bridge/perception.rs`
- Extract input/window ops from `apps/desktop/src-tauri/src/bridge/action.rs`
- Place in `crates/desktop/`, zero Tauri dependencies
- Define `DesktopCapability` trait in Core

**Validation**: `cargo test -p aleph-desktop` passes

### Phase 2: Integrate — Server links desktop crate directly

- Add `desktop` feature to `apps/server/Cargo.toml`
- `DesktopTool` calls trait directly (no IPC)
- Keep `DesktopBridgeClient` as fallback (progressive compatibility)

```rust
// Prefer in-process, fall back to IPC
let result = if let Some(native) = ctx.desktop_capability() {
    native.screenshot(region).await
} else if let Some(bridge) = ctx.desktop_bridge_client() {
    bridge.send(DesktopRequest::Screenshot { region }).await
} else {
    Err(DesktopError::NotAvailable)
};
```

**Validation**: `cargo run --bin aleph-server --features desktop` → Agent can take screenshots

### Phase 3: Slim Down — Tauri becomes pure UI client

- Remove bridge UDS server from Tauri
- Remove perception/action modules from Tauri
- Tauri becomes pure WebView client, connects to Gateway via WS
- Evaluate Canvas: keep as optional or defer

### Phase 4: Polish — Platform completion + daemon management

- macOS: Add Vision OCR (via `objc2` or `swift-bridge`)
- macOS: Add AX API (via accessibility-sys or swift-bridge)
- Linux: Consider tesseract-rs for OCR
- Daemon management: launchd plist generator, systemd unit generator

---

## 8. OpenClaw vs Aleph: Comprehensive Comparison

### 8.1 Architecture Philosophy

| Dimension | OpenClaw | Aleph (After Refactor) |
|-----------|----------|----------------------|
| **Core Language** | TypeScript (Node 22+) | Rust + Tokio |
| **Topology** | Star (Gateway hub + Node spokes) | Single daemon (all-in-one) |
| **Desktop Control** | Outsourced (PeekabooBridge) | Built-in (xcap + enigo) |
| **Multi-device** | Native (macOS + iOS + Android + headless) | Not supported (one server = one machine) |
| **Remote** | Supported (SSH/Tailscale/WS) | Not supported (local only) |
| **Capability Discovery** | Dynamic (caps travel with WS connect) | Static (compile-time feature flags) |
| **Routing** | Agent selects Node by capability | Direct (no routing needed) |
| **A2UI Protocol** | Structured JSONL (v0.8/v0.9) | Deferred (Canvas dropped in initial refactor) |

### 8.2 Actual Capabilities

| Capability | OpenClaw | Aleph |
|-----------|----------|-------|
| Screenshot | Node platform API | xcap (cross-platform) |
| OCR | PeekabooBridge (macOS Vision) | WinRT (Windows), Vision (macOS, future) |
| Click/Type | PeekabooBridge (macOS AX) | enigo (cross-platform) |
| AX Tree | PeekabooBridge | UI Automation (Windows), AX API (macOS, future) |
| Camera | Node native (iOS/Android) | Not supported |
| GPS/Location | Node native (iOS/Android) | Not supported |
| Voice TTS/STT | Node native | Not supported |
| Canvas/A2UI | JSONL bidirectional (component-level) | Deferred |
| Approval Flow | Exec approvals + semantic audit | ConfigApprovalPolicy (Allow/Deny/Ask) |
| PIM | Not supported | macOS native (Calendar/Reminders/Notes/Contacts) |
| Remote Control | VPS Gateway + remote Nodes | Not supported |

### 8.3 Aleph's Advantages

1. **Pure Rust performance** — No GC, memory safety, compile-time guarantees
2. **R1 strictly enforced** — Core has zero platform API calls, even after refactor
3. **Mature approval system** — ConfigApprovalPolicy supports allowlist/blocklist/default
4. **Built-in automation** — No dependency on external tools for basic desktop control
5. **Simpler deployment** — Single binary with feature flags, no multi-process coordination
6. **PIM integration** — Direct access to macOS Calendar/Reminders/Notes/Contacts

### 8.4 Aleph's Disadvantages (trade-offs)

1. **No multi-device** — Cannot control phone + tablet + remote server simultaneously
2. **No remote control** — Strictly local, single-machine focus
3. **Canvas deferred** — No agent-controlled visual workspace initially
4. **Mobile absent** — No iOS/Android node equivalent

### 8.5 What Aleph Borrows from OpenClaw

| Borrowed Concept | Adaptation |
|-----------------|------------|
| Headless daemon model | Server IS the daemon, desktop is a feature |
| Capability reporting | `fn capabilities() -> Vec<Capability>` trait method |
| Brain-limb separation | Crate-level (not process-level) separation |
| Feature composability | `--features desktop,desktop-ocr,desktop-ax` |

### 8.6 What Aleph Intentionally Does NOT Borrow

| Rejected Concept | Reason |
|-----------------|--------|
| Distributed Node network | Aleph is personal single-device; complexity not justified |
| PeekabooBridge outsourcing | Aleph prefers built-in control (simpler, fewer dependencies) |
| A2UI JSONL protocol | Deferred, may adopt later if Canvas is restored |
| Remote node pairing | Out of scope for personal assistant |

---

## 9. Key Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Desktop capability location | Standalone crate, Server feature gate | Keep Core pure (R1), flexible deployment |
| IPC vs in-process | In-process calls, IPC as fallback | Eliminate complexity, retain compatibility |
| Tauri role | Degraded to pure UI client | Remove shell constraint (R7) |
| Canvas | Drop in Phase 1, optional restore in Phase 3 | Daemon doesn't need popups |
| R1 compliance | Crate-level separation replaces process-level | Spirit unchanged, implementation simplified |
| Platform code | `#[cfg(target_os)]` in aleph-desktop | Compile-time isolation |
| Daemon management | PID file + signal handling + launchd/systemd | Standard Unix daemon pattern |
| Migration | Progressive (4 phases) | No service interruption |

---

## 10. Architectural Redline Compliance

| Redline | Status | Explanation |
|---------|--------|-------------|
| **R1** Brain-Limb Separation | ✅ | Separation at crate level: `alephcore` (traits) vs `aleph-desktop` (impls) |
| **R2** Single Source of UI Truth | ✅ | UI moves entirely to Leptos/WASM in Tauri client (if used) |
| **R3** Core Minimalism | ✅ | Desktop is optional feature, not in core |
| **R4** I/O-Only Interfaces | ✅ | Tauri becomes pure I/O client connecting via WS |
| **R5** Menu Bar First | ✅ | Daemon has no UI; Tauri client handles menu bar if present |
| **R6** AI Comes to You | ✅ | Desktop capabilities enable proactive agent action |
| **R7** One Core, Many Shells | ✅ | Server IS the core; Tauri/CLI/Bot are all shells |
